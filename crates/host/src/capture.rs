//! S02 capture command handler: authorized intent, native session, canonical commit.
//!
//! Boundaries:
//! * the handler never accepts a window handle, a monitor, a path or a trigger from the
//!   request: those come from the trusted adapter configuration (S02 §3.2, §5.2);
//! * the native session runs on a blocking thread, so no GDI or overlay work ever runs
//!   inside a Tokio worker (S02 §4.3);
//! * the canonical asset exists only after a verified encode **and** a committed record;
//!   a failure before the commit leaves neither a row nor a file (S02 §8.5).
use crate::services::storage_error;
use async_trait::async_trait;
use sm_application::{
    capture::{
        CaptureApi, CaptureError, CaptureMode, CaptureOutcome, CaptureRequest, CaptureResult,
        CaptureTrigger, NativeCaptureAdapter,
    },
    observability::{
        Component, EventId, Level, Operation, Outcome, SafeCode, SafeEvent, SafeEventSink,
    },
    storage::{
        AssetChunk, AssetType, CommitImageCapture, FoundationRepository, Provenance, StorageError,
        ASSET_CHUNK_BYTES,
    },
    RequestContext, Scope,
};
use sm_codec::{encode_exact_lossless_verified, CodecError, RgbaImage};
use sm_domain::{AssetId, RecordId};
use sm_observability::Logger;
use sm_protocol::{
    application::{
        CaptureCommandParams, CaptureOutcome as WireCaptureOutcome, CaptureReceipt,
        LibraryAssetChunk, LibraryAssetRequest,
    },
    errors::{AppError, ErrorCode},
    values::SafeMessage,
};
use sm_storage::StorageWorker;
use std::{
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// At most two accepted save operations, including the running one (S02 §4.3). A third
/// capture is refused before the freeze instead of queueing an unknown future scene.
pub const MAX_ACCEPTED_SAVES: u8 = 2;
/// Bounded slot count of the hotkey persistence channel; a full queue is backpressure.
pub const HOTKEY_QUEUE: usize = 2;

/// One typed refusal or failure with the stable S01 error envelope.
fn app_error(code: ErrorCode, message: &'static str, retryable: bool) -> AppError {
    AppError {
        code,
        safe_message: SafeMessage::parse(message).expect("static safe message"),
        retryable,
    }
}

/// Total mapping of a native capture failure onto the closed S01 code registry.
pub fn capture_error_code(error: CaptureError) -> ErrorCode {
    match error {
        CaptureError::Busy => ErrorCode::CaptureBusy,
        CaptureError::Backpressure => ErrorCode::CaptureBackpressure,
        CaptureError::TargetLost => ErrorCode::CaptureTargetLost,
        CaptureError::TopologyChanged => ErrorCode::CaptureTopologyChanged,
        CaptureError::FocusUnavailable => ErrorCode::CaptureFocusUnavailable,
        CaptureError::Unavailable => ErrorCode::CaptureUnavailable,
        CaptureError::LimitExceeded => ErrorCode::ImageLimitExceeded,
        CaptureError::Gdi { .. } => ErrorCode::CaptureGdiFailed,
        CaptureError::EmptySelection | CaptureError::Cancelled => ErrorCode::WaitTimeout,
        CaptureError::Internal => ErrorCode::RuntimeUnavailable,
    }
}

/// Total mapping of a codec failure onto the closed S01 code registry.
pub fn codec_error_code(error: CodecError) -> ErrorCode {
    match error {
        CodecError::CanonicalSizeUnsupported { .. } => ErrorCode::CanonicalSizeUnsupported,
        CodecError::ImageLimitExceeded { .. } => ErrorCode::ImageLimitExceeded,
        CodecError::RoundTripMismatch { .. } => ErrorCode::AssetIntegrityFailed,
        CodecError::EncodeFailed { .. } | CodecError::DecodeFailed { .. } => ErrorCode::CodecFailed,
        CodecError::NotImplemented { .. }
        | CodecError::BufferLengthMismatch { .. }
        | CodecError::EmptyInput
        | CodecError::TruncatedInput
        | CodecError::MalformedInput => ErrorCode::ImageInputInvalid,
    }
}

/// Safe event of one capture session; never a path, a title or a window handle.
fn emit(logger: &Logger, event: EventId, outcome: Outcome, code: Option<SafeCode>) {
    let mut safe = SafeEvent::new(Level::Info, Component::Capture, event, Operation::Capture);
    safe.outcome = Some(outcome);
    safe.safe_code = code;
    logger.emit(safe);
}

fn failure_code(error: CaptureError) -> SafeCode {
    match error {
        CaptureError::Busy | CaptureError::Backpressure => SafeCode::QueueFull,
        CaptureError::Gdi { .. }
        | CaptureError::Internal
        | CaptureError::LimitExceeded
        | CaptureError::Unavailable
        | CaptureError::TopologyChanged => SafeCode::InternalFailure,
        CaptureError::FocusUnavailable
        | CaptureError::TargetLost
        | CaptureError::EmptySelection
        | CaptureError::Cancelled => SafeCode::ProbeBlocked,
    }
}

/// Canonical UTC millisecond timestamp of one instant: `YYYY-MM-DDTHH:MM:SS.mmmZ`.
///
/// The stored provenance timestamp must be a real calendar instant; the days-to-civil
/// conversion is written out instead of pulling a calendar dependency into the host.
pub fn canonical_utc_millis(instant: SystemTime) -> String {
    let duration = instant.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let millis = duration.subsec_millis();
    let total_seconds = duration.as_secs();
    let days = (total_seconds / 86_400) as i64;
    let seconds_of_day = total_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// Canonical UTC form of a freeze instant that the frame carried in Unix milliseconds.
///
/// The capture instant of a frame is its own `frozen_at_ms`, measured at the freeze; the
/// persisted provenance timestamp is exactly that instant and never the time of the later
/// encode or commit (S02 §6.3: `captured_at` is the freeze, the record's own `created_at`
/// is the ingested time).
pub fn canonical_utc_millis_from_unix_ms(millis: u64) -> String {
    canonical_utc_millis(UNIX_EPOCH + Duration::from_millis(millis))
}

/// Stored provenance of one accepted frame: the automatic source plus the freeze instant.
/// The frame carries the instant of its own freeze, so a slow overlay selection or a slow
/// canonical encode can never move the recorded capture time (S02 §6.3).
fn capture_provenance(result: &CaptureResult) -> Provenance {
    Provenance::Automatic {
        when: canonical_utc_millis_from_unix_ms(result.frozen_at_ms),
        source: result.context.target,
    }
}

/// Days since the Unix epoch to a proleptic Gregorian date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// The one commit command of an accepted native result: verified canonical bytes, the
/// observed context and the provenance of the frame's own freeze instant.
fn commit_command(result: &CaptureResult, canonical: Vec<u8>) -> CommitImageCapture {
    CommitImageCapture {
        record_id: RecordId::new(Uuid::new_v4()),
        asset_id: AssetId::new(Uuid::new_v4()),
        asset_type: AssetType::Original,
        capture_context: result.context.clone(),
        provenances: vec![capture_provenance(result)],
        canonical,
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("nibble"));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("nibble"));
    }
    out
}

/// Owned capture handler of one host composition.
pub struct CaptureIngest {
    adapter: Arc<dyn NativeCaptureAdapter>,
    storage: StorageWorker,
    logger: Logger,
    slots: Arc<AtomicU8>,
}

impl CaptureIngest {
    pub fn new(
        adapter: Arc<dyn NativeCaptureAdapter>,
        storage: StorageWorker,
        logger: Logger,
    ) -> Arc<Self> {
        Arc::new(Self {
            adapter,
            storage,
            logger,
            slots: Arc::new(AtomicU8::new(0)),
        })
    }

    /// Non-blocking reservation of one accepted-save slot.
    fn reserve(&self) -> bool {
        let mut current = self.slots.load(Ordering::Acquire);
        loop {
            if current >= MAX_ACCEPTED_SAVES {
                return false;
            }
            match self.slots.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn release(&self) {
        self.slots.fetch_sub(1, Ordering::AcqRel);
    }

    /// Encode the accepted pixels and commit them with their context in one transaction.
    async fn commit(&self, result: &CaptureResult) -> Result<CaptureReceipt, AppError> {
        let dib = &result.dib;
        let image =
            RgbaImage::from_rgba(dib.width(), dib.height(), dib.rgba8()).map_err(codec_failure)?;
        // Verification is part of the canonical path: an encode that cannot reproduce its
        // input never becomes a stored asset.
        let canonical = encode_exact_lossless_verified(&image)
            .map_err(codec_failure)?
            .into_canonical();
        let command = commit_command(result, canonical.bytes().to_vec());
        let committed = self
            .storage
            .commit_image_capture(command)
            .await
            .map_err(storage_failure)?;
        Ok(CaptureReceipt {
            operation_id: sm_domain::OperationId::new(result.request.operation_id()),
            record_id: committed.record_id,
            asset_id: committed.asset_id,
            canonical_mime: canonical.mime().to_string(),
            canonical_sha256: committed.sha256,
            width: dib.width(),
            height: dib.height(),
            byte_size: committed.size_bytes.to_string(),
            relative_path: committed.relative_path,
            warnings: result.warnings.clone(),
        })
    }

    /// Full command path: reserve a slot, run the native session, commit the accepted crop.
    async fn run_command(
        &self,
        ctx: RequestContext,
        params: CaptureCommandParams,
        cancel: CancellationToken,
    ) -> Result<WireCaptureOutcome, AppError> {
        if !ctx.allows(Scope::CaptureScreen) {
            return Err(app_error(
                ErrorCode::PermissionDenied,
                "The caller may not capture the screen.",
                false,
            ));
        }
        if !self.reserve() {
            emit(
                &self.logger,
                EventId::CaptureRefused,
                Outcome::Failed,
                Some(SafeCode::QueueFull),
            );
            return Err(app_error(
                ErrorCode::CaptureBackpressure,
                "The library already has the maximum accepted saves in flight.",
                true,
            ));
        }
        let request = CaptureRequest::from_command(params, CaptureTrigger::ClientCommand);
        emit(
            &self.logger,
            EventId::CaptureStarted,
            Outcome::Succeeded,
            None,
        );
        let adapter = self.adapter.clone();
        let outcome = run_blocking_session(adapter, request, cancel).await;
        let answer = self.finish(outcome).await;
        self.release();
        answer
    }

    /// Persistence of an outcome the native hotkey path already produced. The accepted
    /// outcome carries its own trusted request, so no separate intent is needed.
    pub async fn persist(&self, outcome: Result<CaptureOutcome, CaptureError>) {
        if !self.reserve() {
            emit(
                &self.logger,
                EventId::CaptureFailed,
                Outcome::Failed,
                Some(SafeCode::QueueFull),
            );
            return;
        }
        let _ = self.finish(outcome).await;
        self.release();
    }

    async fn finish(
        &self,
        outcome: Result<CaptureOutcome, CaptureError>,
    ) -> Result<WireCaptureOutcome, AppError> {
        match outcome {
            Ok(CaptureOutcome::Accepted(result)) => match self.commit(&result).await {
                Ok(receipt) => {
                    emit(
                        &self.logger,
                        EventId::CaptureCompleted,
                        Outcome::Succeeded,
                        None,
                    );
                    Ok(WireCaptureOutcome::Saved { receipt })
                }
                Err(error) => {
                    emit(
                        &self.logger,
                        EventId::CaptureFailed,
                        Outcome::Failed,
                        Some(SafeCode::InternalFailure),
                    );
                    Err(error)
                }
            },
            Ok(CaptureOutcome::Cancelled {
                reason,
                operation_id,
            }) => Ok(WireCaptureOutcome::Cancelled {
                operation_id: sm_domain::OperationId::new(operation_id),
                reason,
            }),
            Err(error) => {
                emit(
                    &self.logger,
                    EventId::CaptureFailed,
                    Outcome::Failed,
                    Some(failure_code(error)),
                );
                Err(app_error(
                    capture_error_code(error),
                    "The screen capture could not produce a stored image.",
                    matches!(
                        error,
                        CaptureError::Busy | CaptureError::Backpressure | CaptureError::Unavailable
                    ),
                ))
            }
        }
    }

    /// Bounded read of one committed canonical asset.
    async fn read_asset(
        &self,
        ctx: RequestContext,
        request: LibraryAssetRequest,
    ) -> Result<LibraryAssetChunk, AppError> {
        if !ctx.allows(Scope::ReadImages) {
            return Err(app_error(
                ErrorCode::PermissionDenied,
                "The caller may not read stored images.",
                false,
            ));
        }
        let chunk: AssetChunk = self
            .storage
            .read_asset_chunk(
                request.asset_id,
                u64::from(request.offset),
                ASSET_CHUNK_BYTES,
            )
            .await
            .map_err(storage_failure)?;
        Ok(LibraryAssetChunk {
            asset_id: chunk.asset_id,
            offset: chunk.offset as u32,
            next_offset: chunk.next_offset as u32,
            eof: chunk.eof,
            total_size: chunk.total_size.to_string(),
            sha256: chunk.sha256,
            mime: sm_codec::WEBP_MIME.to_string(),
            data_hex: hex_encode(&chunk.bytes),
        })
    }
}

/// Run the native session on a blocking thread and honour cancellation of the *waiting*
/// client without discarding a typed answer.
async fn run_blocking_session(
    adapter: Arc<dyn NativeCaptureAdapter>,
    request: CaptureRequest,
    cancel: CancellationToken,
) -> Result<CaptureOutcome, CaptureError> {
    let operation_id = request.operation_id();
    let cancelling = adapter.clone();
    let mut session = tokio::task::spawn_blocking(move || adapter.capture(request));
    let mut signalled = false;
    loop {
        tokio::select! {
            biased;
            joined = &mut session => {
                return match joined {
                    Ok(outcome) => outcome,
                    Err(_) => Err(CaptureError::Internal),
                };
            }
            _ = cancel.cancelled(), if !signalled => {
                // Only this connection's named session is cancelled; the answer is still the
                // session's own typed outcome.
                signalled = true;
                cancelling.cancel(operation_id);
            }
        }
    }
}

fn codec_failure(error: CodecError) -> AppError {
    app_error(
        codec_error_code(error),
        "The accepted image could not be stored in the canonical format.",
        false,
    )
}

fn storage_failure(error: StorageError) -> AppError {
    storage_error(error)
}

/// Bounded sink of the native hotkey path: it only tries to enqueue, never blocks the
/// native thread, and the consumer persists through the same ingest as a command.
pub struct HotkeyPersistence {
    sender: tokio::sync::mpsc::Sender<Result<CaptureOutcome, CaptureError>>,
}

impl HotkeyPersistence {
    /// Create the bounded channel and the sink consuming it.
    pub fn channel() -> (
        Arc<Self>,
        tokio::sync::mpsc::Receiver<Result<CaptureOutcome, CaptureError>>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::channel(HOTKEY_QUEUE);
        (Arc::new(Self { sender }), receiver)
    }
}

impl sm_capture::HotkeySink for HotkeyPersistence {
    fn on_capture(&self, _mode: CaptureMode, outcome: Result<CaptureOutcome, CaptureError>) {
        // A full slot means the library already has the maximum accepted saves in flight:
        // the event is dropped with a bounded backpressure instead of blocking the native
        // thread or queueing an unknown future scene.
        let _ = self.sender.try_send(outcome);
    }
}

/// Consume the bounded hotkey queue until the host stops.
pub async fn drain_hotkeys(
    ingest: Arc<CaptureIngest>,
    mut receiver: tokio::sync::mpsc::Receiver<Result<CaptureOutcome, CaptureError>>,
) {
    while let Some(outcome) = receiver.recv().await {
        ingest.persist(outcome).await;
    }
}

/// Capture port of the host: it owns the ingest and nothing else. The trait lives in
/// `sm-application`; the host is the only implementation.
pub struct HostCaptureApi {
    ingest: Arc<CaptureIngest>,
}

impl HostCaptureApi {
    pub fn new(ingest: Arc<CaptureIngest>) -> Arc<Self> {
        Arc::new(Self { ingest })
    }
}

#[async_trait]
impl CaptureApi for HostCaptureApi {
    async fn capture_command(
        &self,
        ctx: RequestContext,
        params: CaptureCommandParams,
        cancel: CancellationToken,
    ) -> Result<WireCaptureOutcome, AppError> {
        self.ingest.run_command(ctx, params, cancel).await
    }

    async fn read_asset(
        &self,
        ctx: RequestContext,
        request: LibraryAssetRequest,
    ) -> Result<LibraryAssetChunk, AppError> {
        self.ingest.read_asset(ctx, request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s02_host_01_canonical_timestamp_is_a_real_calendar_instant() {
        // 2026-09-28T09:15:00.000Z
        let instant = UNIX_EPOCH + Duration::from_secs(1_790_586_900);
        let text = canonical_utc_millis(instant);
        assert_eq!(text, "2026-09-28T09:15:00.000Z");
        assert!(sm_application::storage::is_canonical_utc_timestamp(&text));
        // A leap day and a millisecond remainder stay canonical.
        let leap = UNIX_EPOCH + Duration::from_millis(1_709_209_815_123);
        let leap_text = canonical_utc_millis(leap);
        assert_eq!(leap_text, "2024-02-29T12:30:15.123Z");
        assert!(sm_application::storage::is_canonical_utc_timestamp(
            &leap_text
        ));
        // The end of a century that is not a leap year is a real calendar instant too.
        let century = UNIX_EPOCH + Duration::from_secs(4_102_444_800);
        assert_eq!(canonical_utc_millis(century), "2100-01-01T00:00:00.000Z");
        // The epoch itself is a valid instant, never an empty string.
        let epoch = canonical_utc_millis(UNIX_EPOCH);
        assert_eq!(epoch, "1970-01-01T00:00:00.000Z");
        assert!(sm_application::storage::is_canonical_utc_timestamp(&epoch));
    }

    #[test]
    fn s02_host_02_every_native_failure_maps_onto_the_closed_registry() {
        let errors = [
            CaptureError::Busy,
            CaptureError::Backpressure,
            CaptureError::TargetLost,
            CaptureError::TopologyChanged,
            CaptureError::FocusUnavailable,
            CaptureError::Unavailable,
            CaptureError::LimitExceeded,
            CaptureError::Gdi {
                step: sm_application::capture::GdiStep::Blit,
                os_code: 6,
            },
            CaptureError::EmptySelection,
            CaptureError::Cancelled,
            CaptureError::Internal,
        ];
        for error in errors {
            let code = capture_error_code(error);
            let text = serde_json::to_string(&code).expect("a code is serializable");
            assert!(text.len() > 2, "every failure maps onto a registry code");
        }
        // Distinct native failures keep distinct public codes where the registry has one.
        assert_eq!(
            capture_error_code(CaptureError::Busy),
            ErrorCode::CaptureBusy
        );
        assert_eq!(
            capture_error_code(CaptureError::LimitExceeded),
            ErrorCode::ImageLimitExceeded
        );
        assert_eq!(
            codec_error_code(CodecError::CanonicalSizeUnsupported {
                width: 0,
                height: 0
            }),
            ErrorCode::CanonicalSizeUnsupported
        );
    }

    #[test]
    fn s02_host_03_slots_bound_the_accepted_saves() {
        let slots = Arc::new(AtomicU8::new(0));
        let reserve = |slots: &Arc<AtomicU8>| {
            let mut current = slots.load(Ordering::Acquire);
            loop {
                if current >= MAX_ACCEPTED_SAVES {
                    return false;
                }
                match slots.compare_exchange_weak(
                    current,
                    current + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return true,
                    Err(observed) => current = observed,
                }
            }
        };
        assert!(reserve(&slots));
        assert!(reserve(&slots));
        assert!(!reserve(&slots));
        slots.fetch_sub(1, Ordering::AcqRel);
        assert!(reserve(&slots));
    }

    /// One accepted synthetic frame with the freeze instant it really carries.
    fn synthetic_frame(frozen_at_ms: u64) -> CaptureResult {
        let bounds = sm_application::storage::PhysicalBounds {
            x: 10,
            y: 20,
            width: 4,
            height: 2,
        };
        CaptureResult {
            request: CaptureRequest::new(
                Uuid::from_u128(0x5c02_0001),
                CaptureMode::Region,
                CaptureTrigger::ClientCommand,
            ),
            context: sm_application::storage::CaptureContext {
                device: r"\\.\DISPLAY1".to_string(),
                app: "screenmemory-host-test".to_string(),
                title: "synthetic".to_string(),
                target: sm_application::storage::CaptureTarget::Region,
                sources: vec![sm_application::storage::CaptureTarget::Region],
                bounds,
            },
            target: None,
            frame_dpi: 96,
            frozen_at_ms,
            observed_at_ms: frozen_at_ms,
            dib: sm_application::capture::CapturedDib::from_bgra_topdown(4, 2, 16, vec![0x40; 32])
                .expect("synthetic dib"),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn s02_host_04_a_freeze_instant_survives_the_millisecond_projection() {
        // The frame carries its own capture instant in Unix milliseconds; the persisted
        // provenance form is that same instant.
        assert_eq!(
            canonical_utc_millis_from_unix_ms(1_790_586_900_000),
            "2026-09-28T09:15:00.000Z"
        );
        assert_eq!(canonical_utc_millis_from_unix_ms(0), "1970-01-01T00:00:00.000Z");
        // A remainder stays a millisecond remainder, never a rounded second.
        assert_eq!(
            canonical_utc_millis_from_unix_ms(1_709_209_815_123),
            "2024-02-29T12:30:15.123Z"
        );
        assert!(sm_application::storage::is_canonical_utc_timestamp(
            &canonical_utc_millis_from_unix_ms(1_709_209_815_123)
        ));
    }

    #[test]
    fn s02_host_05_committed_provenance_is_the_freeze_not_the_commit_instant() {
        let frame = synthetic_frame(1_790_586_900_000);
        let command = commit_command(&frame, vec![0x01, 0x02, 0x03, 0x04]);
        assert_eq!(command.asset_type, AssetType::Original);
        assert_eq!(command.capture_context, frame.context);
        assert_eq!(command.canonical, vec![0x01, 0x02, 0x03, 0x04]);
        assert_eq!(command.record_id.as_uuid().get_version_num(), 4);
        assert_eq!(command.asset_id.as_uuid().get_version_num(), 4);
        // The stored `when` is the freeze instant of the frame itself.
        assert_eq!(
            command.provenances,
            vec![Provenance::Automatic {
                when: "2026-09-28T09:15:00.000Z".to_string(),
                source: sm_application::storage::CaptureTarget::Region,
            }]
        );
        // A capture time is never the clock of the later encode/commit: an epoch freeze stays
        // an epoch freeze even though this commit happens now.
        let epoch = commit_command(&synthetic_frame(0), Vec::new());
        assert_eq!(
            epoch.provenances,
            vec![Provenance::Automatic {
                when: "1970-01-01T00:00:00.000Z".to_string(),
                source: sm_application::storage::CaptureTarget::Region,
            }]
        );
        assert!(!epoch.provenances.is_empty());
        let canonical = sm_application::storage::provenances_to_json(&epoch.provenances)
            .expect("a canonical instant is storable");
        assert!(canonical.contains("1970-01-01T00:00:00.000Z"));
        assert!(!canonical.contains(&canonical_utc_millis(SystemTime::now())));
    }
}
