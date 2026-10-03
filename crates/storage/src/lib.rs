//! SQLite worker holds one OS lease and a single connection; no SQL escape hatch.
#[cfg(not(all(windows, target_pointer_width = "64")))]
compile_error!("sm-storage requires the audited Windows x64 root adapter");
#[cfg(test)]
mod file_lane_tests;
pub mod image_services;
#[cfg(test)]
mod image_tests;
mod images;
mod import_db;
#[cfg(test)]
mod import_tests;
#[cfg(test)]
mod intent_tests;
mod intents;
mod library_views;
pub mod match_recovery;
mod migration;
pub mod recovery_candidate;
mod recovery_commit;
#[cfg(test)]
mod recovery_commit_tests;
#[cfg(test)]
mod recovery_process_tests;
pub mod staging;
pub mod staging_files;
#[cfg(test)]
mod tests;

use async_trait::async_trait;
pub use recovery_candidate::{
    PrepareCandidateError, PublishCandidateError, SealedPublished, VerifiedCandidate,
    VerifiedPublished,
};
use rusqlite::{Connection, OpenFlags};
use sm_application::storage::{
    warnings_to_json, AssetChunk, CommitImageCapture, CommittedImage, CreateCaptureIntent,
    CreateIntentOutcome, FoundationRepository, LookupOutcome, Page, ReadIntentOutcome, Record,
    RecordPageQuery, SchemaSnapshot, StorageError,
};
use sm_application::{
    capture::{CaptureRequest, MAX_CAPTURE_PIXELS, MAX_CAPTURE_SIDE},
    storage::{CaptureContext, CaptureTarget},
};
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId, RecordId};
use sm_isolation::{HostLease, LeaseError, StagingCandidates, StagingError};
use std::{
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{oneshot, watch};
use uuid::Uuid;

type Response<T> = oneshot::Sender<Result<T, StorageError>>;

/// An owned, path-free staging command. The caller transfers the pixel allocation, never
/// clones it; cancellation of its waiter does not revoke an admitted file write.
pub struct OwnedStageInput {
    pub library_id: LibraryId,
    pub principal_id: PrincipalId,
    pub operation_id: OperationId,
    pub record_id: RecordId,
    pub asset_id: AssetId,
    pub frame_id: Uuid,
    pub mode: CaptureTarget,
    pub context: CaptureContext,
    pub captured_at: String,
    pub encoder_fingerprint: String,
    pub pixels: Vec<u8>,
}

/// Separate owned v2 command. The ordered warnings are transferred with the pixels and
/// validated before file-lane admission, never synthesized from a legacy stage.
pub struct OwnedStageInputV2 {
    pub capture: OwnedStageInput,
    pub warnings: Vec<String>,
}

impl OwnedStageInputV2 {
    fn checked(&self) -> Result<(), StageOwnedError> {
        self.capture.checked()?;
        if self.capture.encoder_fingerprint.is_empty() {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(staging::StagingError::ProfileMismatch),
            ));
        }
        warnings_to_json(&self.warnings).map_err(|_| {
            StageOwnedError::Stage(staging_files::StageFailure::Rejected(
                staging::StagingError::InvalidWarnings,
            ))
        })?;
        Ok(())
    }
}

/// Trusted, owned identity and profile for reopening one known operation. No caller-supplied
/// path, filename, manifest or mutable output buffer is accepted.
pub struct OwnedReopenInputV2 {
    pub library_id: LibraryId,
    pub principal_id: PrincipalId,
    pub operation_id: OperationId,
    pub mode: CaptureTarget,
    pub encoder_fingerprint: String,
}

impl OwnedReopenInputV2 {
    fn checked(&self) -> Result<(), ReopenOwnedError> {
        if self.library_id.as_uuid().is_nil()
            || self.principal_id.as_uuid().is_nil()
            || self.operation_id.as_uuid().is_nil()
        {
            return Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Rejected(staging::StagingError::NilIdentity),
            ));
        }
        if self.encoder_fingerprint.is_empty() || self.encoder_fingerprint.len() > 512 {
            return Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Rejected(staging::StagingError::ProfileMismatch),
            ));
        }
        Ok(())
    }
}

/// Existence of the fixed operation directory, not validity of its manifest or contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagePresence {
    Absent,
    Present,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageOwnedError {
    Unavailable,
    Stage(staging_files::StageFailure),
}

/// Path-free refusal; neither a rejected artifact nor this error exposes its metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReopenOwnedError {
    Unavailable,
    Reopen(staging_files::ReopenFailure),
}

/// Failed discovery never masquerades as a complete (possibly empty) candidate set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnumerateCandidatesError {
    Unavailable,
    Enumeration(StagingError),
}

impl OwnedStageInput {
    fn checked(&self) -> Result<(), StageOwnedError> {
        use staging::StagingError;
        let width = self.context.bounds.width;
        let height = self.context.bounds.height;
        if width == 0 || height == 0 || width > MAX_CAPTURE_SIDE || height > MAX_CAPTURE_SIDE {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::InvalidGeometry),
            ));
        }
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= MAX_CAPTURE_PIXELS * 4)
            .ok_or(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::InvalidGeometry),
            ))?;
        if self.pixels.len() as u64 != bytes {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::PixelLengthMismatch),
            ));
        }
        if self.context.sources.len() > 4 {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::InvalidContext),
            ));
        }
        if self
            .context
            .to_json()
            .map_or(true, |json| json.len() > staging::MAX_CONTEXT_BYTES)
        {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::InvalidContext),
            ));
        }
        if self.encoder_fingerprint.len() > 512 || self.captured_at.len() > 32 {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::ManifestBeyondLimit),
            ));
        }
        Ok(())
    }
    fn borrowed(&self) -> staging_files::StageInput<'_> {
        staging_files::StageInput {
            library_id: self.library_id,
            principal_id: self.principal_id,
            operation_id: self.operation_id,
            record_id: self.record_id,
            asset_id: self.asset_id,
            frame_id: self.frame_id,
            mode: self.mode,
            context: &self.context,
            captured_at: &self.captured_at,
            encoder_fingerprint: &self.encoder_fingerprint,
            pixels: &self.pixels,
        }
    }
}

type StageResponse = oneshot::Sender<Result<staging_files::Staged, StageOwnedError>>;
enum FileRequest {
    Preview(
        oneshot::Sender<Result<Vec<u8>, StorageError>>,
        Arc<sm_image_worker::TrustedWorker>,
        import_db::AssetSnapshot,
    ),
    VerifyImportReport(
        oneshot::Sender<Result<sm_application::images::ImportReport, StorageError>>,
        sm_application::images::ImportReport,
        SyncSender<Request>,
    ),
    StopSql(SyncSender<Request>),
    ExportRead(
        oneshot::Sender<Result<image_services::ExportReadLease, StorageError>>,
        PrincipalId,
        OperationId,
        SyncSender<Request>,
    ),
    CleanupExports(
        oneshot::Sender<Result<u32, StorageError>>,
        PrincipalId,
        SyncSender<Request>,
    ),
    ImportRun(
        Arc<sm_image_worker::TrustedWorker>,
        PrincipalId,
        sm_domain::ImportBatchId,
        Arc<image_services::JobControl>,
        Arc<Mutex<std::collections::HashMap<Uuid, Arc<image_services::JobControl>>>>,
        SyncSender<Request>,
        u32,
    ),
    ImageExport(
        oneshot::Sender<Result<sm_application::images::ExportReceipt, StorageError>>,
        Arc<sm_image_worker::TrustedWorker>,
        PrincipalId,
        sm_application::images::ExportRequest,
        SyncSender<Request>,
    ),
    Stage(StageResponse, Box<OwnedStageInput>),
    StageV2(
        oneshot::Sender<Result<staging::ValidatedRecoveryEnvelope, StageOwnedError>>,
        Box<OwnedStageInputV2>,
    ),
    Probe(
        oneshot::Sender<Result<StagePresence, StageOwnedError>>,
        OperationId,
    ),
    ReopenV2(
        oneshot::Sender<Result<staging_files::ReopenedV2, ReopenOwnedError>>,
        OwnedReopenInputV2,
    ),
    EnumerateCandidates(oneshot::Sender<Result<StagingCandidates, EnumerateCandidatesError>>),
    PrepareCandidate(
        oneshot::Sender<Result<VerifiedCandidate, PrepareCandidateError>>,
        Box<match_recovery::DbValidatedIntent>,
        Box<staging_files::ReopenedV2>,
    ),
    PublishCandidate(
        oneshot::Sender<Result<VerifiedPublished, PublishCandidateError>>,
        VerifiedCandidate,
    ),
    SealPublished(
        oneshot::Sender<Result<SealedPublished, PublishCandidateError>>,
        VerifiedPublished,
    ),
    RevalidateSeal(
        oneshot::Sender<Result<SealedPublished, PublishCandidateError>>,
        SealedPublished,
    ),
    Stop,
}
struct Admission {
    closing: bool,
    stop_sent: bool,
    stopped: watch::Sender<bool>,
}

/// Test-only fault channel between the file write and its commit. The production
/// path always uses `Fault::None`.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    MigrationCommit,
    ImageCommit,
    RecoveryBeforeBegin,
    RecoveryBeforeCommit,
}

enum Request {
    Images(Response<import_db::Reply>, import_db::Command),
    Snapshot(Response<SchemaSnapshot>),
    AiEnabled(Response<bool>),
    SqliteVersion(Response<String>),
    CommitImage(Response<CommittedImage>, Box<CommitImageCapture>),
    LookupOperation(Response<LookupOutcome>, String, String, String),
    CreateIntent(Response<CreateIntentOutcome>, Box<CreateCaptureIntent>),
    ReadIntent(
        Response<ReadIntentOutcome>,
        LibraryId,
        PrincipalId,
        CaptureRequest,
    ),
    ReadValidatedIntent(
        Response<match_recovery::ValidatedIntentRead>,
        LibraryId,
        PrincipalId,
        CaptureRequest,
    ),
    CommitSealedRecovery(Response<CommittedImage>, SealedPublished),
    PageRecords(Response<Page<Record>>, RecordPageQuery),
    LibraryPage(
        Response<sm_protocol::application::LibraryRecordsPage>,
        RecordPageQuery,
    ),
    ReadAssetChunk(Response<AssetChunk>, String, u64, u32),
    #[cfg(test)]
    Pragmas(Response<(String, i64, i64, i64)>),
    #[cfg(test)]
    Pause(Response<()>, Duration),
    Stop,
}

/// Cloneable bounded command endpoint, never a cloneable connection or lease.
#[derive(Clone)]
pub struct StorageWorker {
    sender: SyncSender<Request>,
    file_sender: SyncSender<FileRequest>,
    gate: Arc<Mutex<Admission>>,
    image_active: Arc<Mutex<std::collections::HashMap<Uuid, Arc<image_services::JobControl>>>>,
}
impl StorageWorker {
    pub async fn library_page(
        &self,
        query: RecordPageQuery,
    ) -> Result<sm_protocol::application::LibraryRecordsPage, StorageError> {
        self.ask(|tx| Request::LibraryPage(tx, query)).await
    }
    /// Consumes a real acquired OS lease. No database filename/path input.
    pub fn start(lease: HostLease) -> Result<Self, StorageError> {
        Self::start_inner(
            lease,
            Fault::None,
            #[cfg(test)]
            None,
            #[cfg(test)]
            None,
        )
    }
    fn start_inner(
        lease: HostLease,
        fault: Fault,
        #[cfg(test)] pause: Option<FilePause>,
        #[cfg(test)] crash_gate: Option<std::sync::Arc<recovery_process_tests::CrashGate>>,
    ) -> Result<Self, StorageError> {
        let (sender, receiver) = mpsc::sync_channel(32);
        let (file_sender, file_receiver) = mpsc::sync_channel(1);
        let (stopped, _) = watch::channel(false);
        let gate = Arc::new(Mutex::new(Admission {
            closing: false,
            stop_sent: false,
            stopped,
        }));
        let status = Arc::clone(&gate);
        let file_stop_sender = file_sender.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        #[cfg(test)]
        let file_crash_gate = crash_gate.clone();
        std::thread::Builder::new()
            .name("sm-sqlite-worker".into())
            .spawn(move || {
                let entry = match lease.database_entry() {
                    Ok(entry) => entry,
                    Err(e) => {
                        drop(lease);
                        let _ = ready_tx.send(Err(map_entry(e)));
                        return;
                    }
                };
                // The canonical assets directory is derived from the leased entry only.
                let assets = match images::assets_root(entry.path()) {
                    Ok(assets) => assets,
                    Err(error) => {
                        drop(entry);
                        drop(lease);
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let result = open(&entry, fault == Fault::MigrationCommit);
                match result {
                    Ok(db) => {
                        let completed = std::thread::scope(|scope| {
                            let (started_tx, started_rx) = mpsc::sync_channel(1);
                            let lease_ref = &lease;
                            let file_thread = std::thread::Builder::new()
                                .name("sm-staging-files".into())
                                .spawn_scoped(scope, move || {
                                    let _ = started_tx.send(());
                                    file_loop(
                                        lease_ref,
                                        file_receiver,
                                        #[cfg(test)]
                                        pause,
                                        #[cfg(test)]
                                        file_crash_gate,
                                    );
                                });
                            let Ok(file_thread) = file_thread else {
                                let _ = ready_tx.send(Err(StorageError::Unavailable));
                                drop(db);
                                return false;
                            };
                            if started_rx.recv().is_err() || ready_tx.send(Ok(())).is_err() {
                                drop(db);
                                let _ = file_stop_sender.send(FileRequest::Stop);
                                let _ = file_thread.join();
                                return false;
                            }
                            // A panicking DB loop still sends the file stop and joins before
                            // the scoped borrow and its original lease can be released.
                            let sqlite_result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    run(
                                        db,
                                        receiver,
                                        assets,
                                        fault,
                                        #[cfg(test)]
                                        crash_gate,
                                    )
                                }));
                            let file_stop = file_stop_sender.send(FileRequest::Stop).is_ok();
                            let file_joined = file_thread.join().is_ok();
                            matches!(sqlite_result, Ok(true)) && file_stop && file_joined
                        });
                        drop(entry);
                        drop(lease);
                        if completed {
                            status
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .stopped
                                .send_replace(true);
                        }
                    }
                    Err(error) => {
                        drop(entry);
                        drop(lease);
                        let _ = ready_tx.send(Err(error));
                    }
                }
            })
            .map_err(|_| StorageError::Unavailable)?;
        ready_rx.recv().map_err(|_| StorageError::Unavailable)??;
        Ok(Self {
            sender,
            file_sender,
            gate,
            image_active: Arc::new(Mutex::new(std::collections::HashMap::new())),
        })
    }
    /// Separate, single-slot file queue. Only accepted requests own file-lane memory;
    /// caller-owned pending buffers are outside this queue's budget.
    pub async fn stage_capture_owned(
        &self,
        input: OwnedStageInput,
    ) -> Result<staging_files::Staged, StageOwnedError> {
        input.checked()?;
        let (tx, rx) = oneshot::channel();
        let mut request = FileRequest::Stage(tx, Box::new(input));
        loop {
            let queued = {
                let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
                if gate.closing {
                    return Err(StageOwnedError::Unavailable);
                }
                self.file_sender.try_send(request)
            };
            match queued {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return Err(StageOwnedError::Unavailable),
                Err(TrySendError::Full(pending)) => request = pending,
            }
            // A probe can occupy the one queued slot while another stage executes.
            // Retain this caller-owned allocation until admission; never enqueue a
            // second file request or spin while the file thread is blocked.
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        rx.await.unwrap_or(Err(StageOwnedError::Unavailable))
    }
    /// The v2 request uses the same one-slot channel, gate and original scoped lease as v1.
    /// Waiter cancellation cannot revoke an admitted file write.
    pub async fn stage_capture_owned_v2(
        &self,
        input: OwnedStageInputV2,
    ) -> Result<staging::ValidatedRecoveryEnvelope, StageOwnedError> {
        input.checked()?;
        let (tx, rx) = oneshot::channel();
        let mut request = FileRequest::StageV2(tx, Box::new(input));
        loop {
            let queued = {
                let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
                if gate.closing {
                    return Err(StageOwnedError::Unavailable);
                }
                self.file_sender.try_send(request)
            };
            match queued {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return Err(StageOwnedError::Unavailable),
                Err(TrySendError::Full(pending)) => request = pending,
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        rx.await.unwrap_or(Err(StageOwnedError::Unavailable))
    }
    /// Reopen only a complete recovery-ready v2 for this trusted operation. Admitted
    /// reads remain FIFO on the same bounded file lane; cancellation does not skip them.
    pub async fn reopen_stage_owned_v2(
        &self,
        input: OwnedReopenInputV2,
    ) -> Result<staging_files::ReopenedV2, ReopenOwnedError> {
        input.checked()?;
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(ReopenOwnedError::Unavailable);
            }
            self.file_sender.try_send(FileRequest::ReopenV2(tx, input))
        };
        match queued {
            Ok(()) => rx.await.unwrap_or(Err(ReopenOwnedError::Unavailable)),
            Err(_) => Err(ReopenOwnedError::Unavailable),
        }
    }
    /// Encode only a sealed SQLite witness paired with already reopened v2 pixels.
    /// Accepted work drains FIFO even when the waiter cancels; no SQL/file writes here.
    /// The 1 GiB RSS bound is NOT_PROVEN (native encoder scratch is not bounded).
    /// ```compile_fail
    /// use sm_application::storage::PendingCaptureIntent;
    /// use sm_storage::{StorageWorker, staging_files::ReopenedV2};
    /// async fn forge(worker: &StorageWorker, dto: PendingCaptureIntent, stage: ReopenedV2) {
    ///     let _ = worker.prepare_recovery_candidate(Box::new(dto), stage).await;
    /// }
    /// ```
    /// ```compile_fail
    /// use sm_storage::VerifiedCandidate;
    /// fn forge() -> VerifiedCandidate { VerifiedCandidate { encoded: Vec::new() } }
    /// ```
    pub async fn prepare_recovery_candidate(
        &self,
        witness: Box<match_recovery::DbValidatedIntent>,
        reopened: staging_files::ReopenedV2,
    ) -> Result<VerifiedCandidate, PrepareCandidateError> {
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(PrepareCandidateError::Unavailable);
            }
            self.file_sender.try_send(FileRequest::PrepareCandidate(
                tx,
                witness,
                Box::new(reopened),
            ))
        };
        match queued {
            Ok(()) => rx.await.unwrap_or(Err(PrepareCandidateError::Unavailable)),
            Err(_) => Err(PrepareCandidateError::Unavailable),
        }
    }
    /// File-only canonical publication of a sealed, codec-verified candidate.
    /// Same admission gate and single-slot FIFO as stage/reopen/encode. Cancellation
    /// after admission cannot revoke the write; the returned proof is not Saved.
    pub async fn publish_recovery_candidate(
        &self,
        candidate: VerifiedCandidate,
    ) -> Result<VerifiedPublished, PublishCandidateError> {
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(PublishCandidateError::Unavailable);
            }
            self.file_sender
                .try_send(FileRequest::PublishCandidate(tx, candidate))
        };
        match queued {
            Ok(()) => rx.await.unwrap_or(Err(PublishCandidateError::Unavailable)),
            Err(_) => Err(PublishCandidateError::Unavailable),
        }
    }
    /// One-shot write-denying fixed canonical HANDLE acquisition and byte recheck.
    /// No SQL or Saved; cancellation after admission releases the returned handle.
    /// ```compile_fail
    /// use sm_storage::SealedPublished;
    /// fn forge() -> SealedPublished { SealedPublished { newly_published: true } }
    /// ```
    pub async fn seal_recovery_published(
        &self,
        published: VerifiedPublished,
    ) -> Result<SealedPublished, PublishCandidateError> {
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(PublishCandidateError::Unavailable);
            }
            self.file_sender
                .try_send(FileRequest::SealPublished(tx, published))
        };
        match queued {
            Ok(()) => rx.await.unwrap_or(Err(PublishCandidateError::Unavailable)),
            Err(_) => Err(PublishCandidateError::Unavailable),
        }
    }
    /// Conditional storage-only transition; pre-BEGIN exact file recheck is performed
    /// exclusively on the bounded file lane. The SQL worker then owns the write-denying
    /// handle throughout COMMIT/rollback. This excludes prior hostile writable mappings.
    /// No host/startup wiring or unconditional filesystem/DB atomicity is claimed.
    /// ```compile_fail
    /// use sm_storage::SealedPublished;
    /// fn forge() -> SealedPublished { SealedPublished { newly_published: true } }
    /// ```
    pub async fn commit_sealed_recovery(
        &self,
        sealed: SealedPublished,
    ) -> Result<CommittedImage, StorageError> {
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(StorageError::Unavailable);
            }
            self.file_sender
                .try_send(FileRequest::RevalidateSeal(tx, sealed))
        };
        if queued.is_err() {
            return Err(StorageError::Unavailable);
        }
        let sealed = rx
            .await
            .map_err(|_| StorageError::Unavailable)?
            .map_err(|_| StorageError::IntegrityFailed)?;
        self.ask(|tx| Request::CommitSealedRecovery(tx, sealed))
            .await
    }
    /// Read-only discovery on the scoped file lane, not the SQLite worker. Returned IDs
    /// remain untrusted; neither a manifest nor a DB receipt is validated or adopted.
    pub async fn enumerate_stage_candidates(
        &self,
    ) -> Result<StagingCandidates, EnumerateCandidatesError> {
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(EnumerateCandidatesError::Unavailable);
            }
            self.file_sender
                .try_send(FileRequest::EnumerateCandidates(tx))
        };
        match queued {
            Ok(()) => rx
                .await
                .unwrap_or(Err(EnumerateCandidatesError::Unavailable)),
            Err(_) => Err(EnumerateCandidatesError::Unavailable),
        }
    }
    /// A path-free, file-lane-ordered preflight. Only a genuinely missing fixed
    /// operation directory is Absent; unsafe entries and root IO fail closed.
    pub async fn probe_stage_presence(
        &self,
        operation_id: OperationId,
    ) -> Result<StagePresence, StageOwnedError> {
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(StageOwnedError::Unavailable);
            }
            self.file_sender
                .try_send(FileRequest::Probe(tx, operation_id))
        };
        match queued {
            Ok(()) => rx.await.unwrap_or(Err(StageOwnedError::Unavailable)),
            Err(_) => Err(StageOwnedError::Unavailable),
        }
    }
    async fn ask<T>(&self, build: impl FnOnce(Response<T>) -> Request) -> Result<T, StorageError> {
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(StorageError::Unavailable);
            }
            self.sender.try_send(build(tx))
        };
        match queued {
            Ok(()) => rx.await.map_err(|_| StorageError::Unavailable)?,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                Err(StorageError::Unavailable)
            }
        }
    }
    /// Timeout is unknown/still-owned, never a declaration of Stopped.
    pub async fn shutdown(&self, deadline: Duration) -> Result<(), StorageError> {
        tokio::time::timeout(deadline, async {
            let mut done = {
                let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
                gate.closing = true;
                gate.stopped.subscribe()
            };
            loop {
                if *done.borrow() {
                    return Ok(());
                }
                {
                    let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
                    if !gate.stop_sent {
                        match self
                            .file_sender
                            .try_send(FileRequest::StopSql(self.sender.clone()))
                        {
                            Ok(()) => gate.stop_sent = true,
                            Err(TrySendError::Disconnected(_)) => {
                                return Err(StorageError::Unavailable)
                            }
                            Err(TrySendError::Full(_)) => {}
                        }
                    }
                }
                if *done.borrow() {
                    return Ok(());
                }
                // If Stop has been queued, wait only for completed close + file join.
                let stop_sent = self
                    .gate
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .stop_sent;
                if stop_sent {
                    done.changed()
                        .await
                        .map_err(|_| StorageError::Unavailable)?;
                } else {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        })
        .await
        .map_err(|_| StorageError::Unavailable)?
    }
    /// Scoped SQLite read; unlike application DTOs, Pending carries a sealed DB witness.
    /// The witness is a snapshot, not a lock on future changes to this operation.
    pub async fn read_validated_capture_intent(
        &self,
        library_id: LibraryId,
        principal_id: PrincipalId,
        request: CaptureRequest,
    ) -> Result<match_recovery::ValidatedIntentRead, StorageError> {
        self.ask(|tx| Request::ReadValidatedIntent(tx, library_id, principal_id, request))
            .await
    }
    /// Actual bundled SQLite runtime version; NOT a sqlite-vec query proof.
    pub async fn sqlite_version(&self) -> Result<String, StorageError> {
        self.ask(Request::SqliteVersion).await
    }
}
fn map_entry(err: LeaseError) -> StorageError {
    match err {
        LeaseError::Busy => StorageError::LockConflict,
        LeaseError::UnsafeEntry => StorageError::RootRejected,
    }
}
fn open(entry: &sm_isolation::DatabaseEntry, fail: bool) -> Result<Connection, StorageError> {
    let fresh = migration::preflight(entry.path(), entry.existed())?;
    let mut db = Connection::open_with_flags(
        entry.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| StorageError::PermissionDenied)?;
    if !entry.existed() {
        // Durable provenance precedes WAL and the transactional migration. A
        // failure before this point leaves an unclaimed file: fail closed later.
        migration::mark_new_file(&db)?;
    }
    migration::pragmas(&db)?;
    migration::initialize(&mut db, fresh, fail)?;
    import_db::reconcile_startup(&mut db)?;
    Ok(db)
}
fn run(
    db: Connection,
    receiver: Receiver<Request>,
    assets: PathBuf,
    fault: Fault,
    #[cfg(test)] crash_gate: Option<std::sync::Arc<recovery_process_tests::CrashGate>>,
) -> bool {
    let mut db = db;
    for request in receiver {
        match request {
            Request::Images(tx, command) => {
                let _ = tx.send(import_db::execute(&mut db, command));
            }
            Request::Snapshot(tx) => {
                let _ = tx.send(migration::validate(&db));
            }
            Request::AiEnabled(tx) => {
                let _ = tx.send(migration::validate(&db).map(|_| false));
            }
            Request::SqliteVersion(tx) => {
                let _ = tx.send(migration::sqlite_version(&db));
            }
            Request::CommitImage(tx, command) => {
                let result = images::commit_image_capture(
                    &mut db,
                    &assets,
                    *command,
                    fault == Fault::ImageCommit,
                );
                let _ = tx.send(result);
            }
            Request::LookupOperation(tx, op_id, principal_id, mode) => {
                let result =
                    sm_application::storage::CaptureTarget::parse(&mode).and_then(|mode| {
                        images::lookup_operation(&db, &assets, &op_id, &principal_id, mode)
                    });
                let _ = tx.send(result);
            }
            Request::CreateIntent(tx, input) => {
                let _ = tx.send(intents::create(&mut db, *input));
            }
            Request::ReadIntent(tx, library, principal, request) => {
                let _ = tx.send(intents::read(&db, library, principal, request));
            }
            Request::ReadValidatedIntent(tx, library, principal, request) => {
                let _ = tx.send(match_recovery::read_validated(
                    &db, library, principal, request,
                ));
            }
            Request::CommitSealedRecovery(tx, sealed) => {
                let result = if fault == Fault::RecoveryBeforeBegin {
                    Err(StorageError::Unavailable)
                } else {
                    recovery_commit::commit(
                        &mut db,
                        sealed,
                        fault == Fault::RecoveryBeforeCommit,
                        #[cfg(test)]
                        crash_gate.as_deref(),
                    )
                };
                let _ = tx.send(result);
            }
            Request::LibraryPage(tx, query) => {
                let _ = tx.send(library_views::page(&db, query));
            }
            Request::PageRecords(tx, query) => {
                let _ = tx.send(images::page_records(&db, query));
            }
            Request::ReadAssetChunk(tx, asset_id, offset, length) => {
                let _ = tx.send(images::read_asset_chunk(
                    &db, &assets, &asset_id, offset, length,
                ));
            }
            #[cfg(test)]
            Request::Pragmas(tx) => {
                let result = (|| -> Result<_, StorageError> {
                    Ok((
                        db.query_row("PRAGMA journal_mode", [], |r| r.get(0))
                            .map_err(|_| StorageError::Unavailable)?,
                        db.query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                            .map_err(|_| StorageError::Unavailable)?,
                        db.query_row("PRAGMA synchronous", [], |r| r.get(0))
                            .map_err(|_| StorageError::Unavailable)?,
                        db.query_row("PRAGMA busy_timeout", [], |r| r.get(0))
                            .map_err(|_| StorageError::Unavailable)?,
                    ))
                })();
                let _ = tx.send(result);
            }
            #[cfg(test)]
            Request::Pause(tx, duration) => {
                std::thread::sleep(duration);
                let _ = tx.send(Ok(()));
            }
            Request::Stop => {
                close_confirmed(db);
                return true;
            }
        }
    }
    close_confirmed(db);
    false
}

fn file_loop(
    lease: &HostLease,
    receiver: Receiver<FileRequest>,
    #[cfg(test)] pause: Option<FilePause>,
    #[cfg(test)] crash_gate: Option<std::sync::Arc<recovery_process_tests::CrashGate>>,
) {
    let mut imports = std::collections::VecDeque::new();
    loop {
        // The original single-slot lane wins between image steps. In particular,
        // an admitted capture stage cannot wait behind a whole recursive batch.
        let request = match receiver.try_recv() {
            Ok(request) => request,
            Err(mpsc::TryRecvError::Empty) => match imports.pop_front() {
                Some(request) => request,
                None => match receiver.recv() {
                    Ok(request) => request,
                    Err(_) => break,
                },
            },
            Err(mpsc::TryRecvError::Disconnected) => match imports.pop_front() {
                Some(request) => request,
                None => break,
            },
        };
        match request {
            FileRequest::Stop => break,
            FileRequest::Preview(tx, worker, asset) => {
                let _ = tx.send(image_services::make_preview(lease, &worker, &asset));
            }
            FileRequest::VerifyImportReport(tx, report, sender) => {
                let _ = tx.send(image_services::verify_report(lease, &sender, report));
            }
            FileRequest::StopSql(sender) => {
                // Shutdown drains the current file step, then retains the rest as
                // an explicit paused batch before SQLite close/lease release.
                for request in imports.drain(..) {
                    if let FileRequest::ImportRun(_, p, b, job, active, sql, _) = request {
                        job.stopped
                            .store(true, std::sync::atomic::Ordering::Release);
                        job.cancel.cancel();
                        let _ = image_services::sql(
                            &sql,
                            import_db::Command::Control(
                                p,
                                sm_application::images::ImportControlRequest {
                                    batch_id: b,
                                    control: sm_application::images::ImportControl::Pause,
                                },
                            ),
                        );
                        if let Ok(mut active) = active.lock() {
                            active.remove(&b.as_uuid());
                        }
                    }
                }
                let _ = sender.send(Request::Stop);
            }
            FileRequest::ExportRead(tx, p, id, image_sql_sender) => {
                let _ = tx.send(image_services::begin_export_read(
                    lease,
                    &image_sql_sender,
                    p,
                    id,
                ));
            }
            FileRequest::CleanupExports(tx, p, image_sql_sender) => {
                let _ = tx.send(image_services::cleanup_exports(lease, &image_sql_sender, p));
            }
            FileRequest::ImportRun(
                worker,
                principal,
                batch,
                cancel,
                active,
                image_sql_sender,
                after,
            ) => {
                match image_services::run_import(
                    lease,
                    &image_sql_sender,
                    &worker,
                    principal,
                    batch,
                    &cancel,
                    after,
                ) {
                    Ok(Some(next)) => imports.push_back(FileRequest::ImportRun(
                        worker,
                        principal,
                        batch,
                        cancel,
                        active,
                        image_sql_sender,
                        next,
                    )),
                    _ => {
                        if let Ok(mut active) = active.lock() {
                            active.remove(&batch.as_uuid());
                        }
                    }
                }
            }
            FileRequest::ImageExport(tx, worker, principal, request, image_sql_sender) => {
                let _ = tx.send(image_services::run_export(
                    lease,
                    &image_sql_sender,
                    &worker,
                    principal,
                    request,
                ));
            }
            FileRequest::EnumerateCandidates(tx) => {
                let result = lease
                    .staging_root()
                    .and_then(|root| root.enumerate_candidates())
                    .map_err(EnumerateCandidatesError::Enumeration);
                let _ = tx.send(result);
            }
            FileRequest::PrepareCandidate(tx, witness, reopened) => {
                let _ = tx.send(recovery_candidate::prepare(witness, *reopened));
            }
            FileRequest::PublishCandidate(tx, candidate) => {
                let _ = tx.send(recovery_candidate::publish(lease, candidate));
            }
            FileRequest::SealPublished(tx, published) => {
                let _ = tx.send(published.seal());
            }
            FileRequest::RevalidateSeal(tx, mut sealed) => {
                let result = sealed.revalidate().map(|()| sealed);
                #[cfg(test)]
                if result.is_ok() {
                    if let Some(gate) = crash_gate.as_deref() {
                        gate.checkpoint(recovery_process_tests::CrashPhase::F06);
                    }
                }
                let _ = tx.send(result);
            }
            FileRequest::Probe(tx, operation_id) => {
                let result = lease
                    .staging_root()
                    .and_then(|root| match root.open_operation(operation_id) {
                        Ok(_) => Ok(StagePresence::Present),
                        Err(sm_isolation::StagingError::Missing) => Ok(StagePresence::Absent),
                        Err(error) => Err(error),
                    })
                    .map_err(|e| StageOwnedError::Stage(staging_files::StageFailure::Storage(e)));
                let _ = tx.send(result);
            }
            FileRequest::ReopenV2(tx, input) => {
                let result = lease
                    .staging_root()
                    .map_err(|e| ReopenOwnedError::Reopen(staging_files::ReopenFailure::Storage(e)))
                    .and_then(|root| {
                        staging_files::reopen_staged_v2(
                            &root,
                            staging::StagingExpectations {
                                library_id: input.library_id,
                                principal_id: input.principal_id,
                                operation_id: input.operation_id,
                                mode: input.mode,
                                encoder_fingerprint: &input.encoder_fingerprint,
                            },
                        )
                        .map_err(ReopenOwnedError::Reopen)
                    });
                let _ = tx.send(result);
            }
            FileRequest::StageV2(tx, input) => {
                let result = lease
                    .staging_root()
                    .map_err(|e| StageOwnedError::Stage(staging_files::StageFailure::Storage(e)))
                    .and_then(|root| {
                        let capture = input.capture.borrowed();
                        let v2 = staging_files::StageInputV2 {
                            capture,
                            warnings: &input.warnings,
                        };
                        #[cfg(test)]
                        if let Some(ref pause) = pause {
                            return staging_files::stage_capture_v2_pausing(&root, v2, &|| {
                                let _ = pause.entered.send(());
                                let _ = pause
                                    .release
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .recv();
                            })
                            .map_err(StageOwnedError::Stage);
                        }
                        staging_files::stage_capture_v2(&root, v2).map_err(StageOwnedError::Stage)
                    });
                let _ = tx.send(result);
            }
            FileRequest::Stage(tx, input) => {
                let result = lease
                    .staging_root()
                    .map_err(|e| StageOwnedError::Stage(staging_files::StageFailure::Storage(e)))
                    .and_then(|root| {
                        #[cfg(test)]
                        if let Some(ref pause) = pause {
                            return staging_files::stage_capture_pausing(
                                &root,
                                input.borrowed(),
                                &|| {
                                    let _ = pause.entered.send(());
                                    let _ = pause
                                        .release
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .recv();
                                },
                            )
                            .map_err(StageOwnedError::Stage);
                        }
                        staging_files::stage_capture(&root, input.borrowed())
                            .map_err(StageOwnedError::Stage)
                    });
                let _ = tx.send(result);
            }
        }
    }
}

#[cfg(test)]
struct FilePause {
    entered: SyncSender<()>,
    release: Mutex<Receiver<()>>,
}
/// sqlite3_close may return BUSY together with the *still-open* connection.
/// Retain both the connection and the worker-owned HostLease until closure succeeds;
/// never translate a failed close into a released writer lock or Stopped.
fn close_confirmed(mut db: Connection) {
    loop {
        match db.close() {
            Ok(()) => return,
            Err((still_open, _)) => {
                db = still_open;
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
#[async_trait]
impl FoundationRepository for StorageWorker {
    async fn schema_snapshot(&self) -> Result<SchemaSnapshot, StorageError> {
        self.ask(Request::Snapshot).await
    }
    async fn read_ai_enabled(&self) -> Result<bool, StorageError> {
        self.ask(Request::AiEnabled).await
    }
    async fn commit_image_capture(
        &self,
        command: CommitImageCapture,
    ) -> Result<CommittedImage, StorageError> {
        self.ask(|tx| Request::CommitImage(tx, Box::new(command)))
            .await
    }
    async fn lookup_operation(
        &self,
        operation_id: sm_domain::OperationId,
        principal_id: sm_domain::PrincipalId,
        mode: sm_application::storage::CaptureTarget,
    ) -> Result<LookupOutcome, StorageError> {
        self.ask(|tx| {
            Request::LookupOperation(
                tx,
                operation_id.as_uuid().to_string(),
                principal_id.as_uuid().to_string(),
                mode.as_str().to_owned(),
            )
        })
        .await
    }
    async fn create_capture_intent(
        &self,
        input: CreateCaptureIntent,
    ) -> Result<CreateIntentOutcome, StorageError> {
        self.ask(|tx| Request::CreateIntent(tx, Box::new(input)))
            .await
    }
    async fn read_capture_intent(
        &self,
        library_id: LibraryId,
        principal_id: PrincipalId,
        request: CaptureRequest,
    ) -> Result<ReadIntentOutcome, StorageError> {
        self.ask(|tx| Request::ReadIntent(tx, library_id, principal_id, request))
            .await
    }
    async fn page_records(&self, query: RecordPageQuery) -> Result<Page<Record>, StorageError> {
        self.ask(|tx| Request::PageRecords(tx, query)).await
    }
    async fn read_asset_chunk(
        &self,
        asset_id: sm_domain::AssetId,
        offset: u64,
        length: u32,
    ) -> Result<AssetChunk, StorageError> {
        // Only the canonical UUID text of the named asset reaches the worker.
        let raw = asset_id.as_uuid().to_string();
        self.ask(|tx| Request::ReadAssetChunk(tx, raw, offset, length))
            .await
    }
}
#[cfg(test)]
impl StorageWorker {
    pub(crate) fn start_with_fault(lease: HostLease) -> Result<Self, StorageError> {
        Self::start_inner(lease, Fault::MigrationCommit, None, None)
    }
    pub(crate) fn start_with_image_fault(lease: HostLease) -> Result<Self, StorageError> {
        Self::start_inner(lease, Fault::ImageCommit, None, None)
    }
    pub(crate) fn start_with_recovery_fault(
        lease: HostLease,
        after_inserts: bool,
    ) -> Result<Self, StorageError> {
        Self::start_inner(
            lease,
            if after_inserts {
                Fault::RecoveryBeforeCommit
            } else {
                Fault::RecoveryBeforeBegin
            },
            None,
            None,
        )
    }
    pub(crate) fn start_with_process_crash_gate(
        lease: HostLease,
        gate: std::sync::Arc<recovery_process_tests::CrashGate>,
    ) -> Result<Self, StorageError> {
        Self::start_inner(lease, Fault::None, None, Some(gate))
    }
    pub(crate) fn start_with_file_pause(
        lease: HostLease,
        entered: SyncSender<()>,
        release: Receiver<()>,
    ) -> Result<Self, StorageError> {
        Self::start_inner(
            lease,
            Fault::None,
            Some(FilePause {
                entered,
                release: Mutex::new(release),
            }),
            None,
        )
    }
    pub(crate) async fn read_pragmas(&self) -> Result<(String, i64, i64, i64), StorageError> {
        self.ask(Request::Pragmas).await
    }
    pub(crate) fn pause_for_test(
        &self,
        duration: Duration,
    ) -> oneshot::Receiver<Result<(), StorageError>> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .try_send(Request::Pause(tx, duration))
            .unwrap_or_else(|_| panic!("test queue empty"));
        rx
    }
}
