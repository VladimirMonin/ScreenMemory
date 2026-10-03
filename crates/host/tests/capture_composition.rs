#![cfg(windows)]
//! S02 capture composition: a real host, a synthetic *injected* adapter and the real
//! canonical commit path. No desktop pixel is read here; the native owner is covered by
//! `sm-capture`'s own live suite.
use sm_application::{
    capture::{
        CancelReason, CaptureError, CaptureMode, CaptureOutcome, CaptureRequest, CaptureResult,
        CaptureTrigger, CapturedDib, NativeCaptureAdapter,
    },
    credentials::{
        Authenticator, CredentialEnvironment, CredentialKey, Secret, SecretStore, StoreError,
    },
    storage::{
        AssetType, CaptureContext, CaptureTarget, CommitImageCapture, FoundationRepository,
        LookupOutcome, PhysicalBounds, Provenance,
    },
};
use sm_capture::HotkeySink;
use sm_domain::{AssetId, LibraryId, PrincipalId, RecordId};
use sm_framing::BufferBudget;
use sm_host::{bootstrap, BootstrapMode, BootstrapOptions, CaptureOptions, DEFAULT_DRAIN};
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use sm_pipe::{Listener, TrustedServerImage};
use sm_protocol::{
    application::{
        CaptureCommandParams, CaptureOutcome as WireCaptureOutcome, LibraryAssetRequest,
    },
    errors::ErrorCode,
};
use sm_session::{ApplicationClient, ClientError, CredentialSource};
use sm_storage::{
    staging::{validate_recovery_envelope, StagingExpectations, RECOVERY_PATH_POLICY},
    StorageWorker,
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Roots(PathBuf);

impl Roots {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = parent.join(Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(child)).unwrap();
        }
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.clone()
    }
    fn layout(&self) -> sm_isolation::NamespaceLayout {
        LibraryRootResolver::resolve(RootSource::Test {
            root: &self.0,
            forbidden_donor_roots: &[],
        })
        .unwrap()
    }
    fn endpoint(&self) -> sm_pipe::Endpoint {
        sm_pipe::Endpoint::for_layout(&self.layout()).unwrap()
    }
    fn assets(&self) -> PathBuf {
        self.0.join("assets")
    }
    fn lease(&self) -> HostLease {
        HostLease::acquire(self.layout()).unwrap()
    }
    /// Every canonical file really published below the leased assets root.
    fn published_files(&self) -> Vec<PathBuf> {
        fn walk(directory: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(directory) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().and_then(|value| value.to_str()) == Some("webp") {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.assets(), &mut out);
        out.sort();
        out
    }
}

impl Drop for Roots {
    fn drop(&mut self) {
        // Best effort only: a still-open handle of a failing test must not turn a first
        // assertion failure into a process abort during unwinding.
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct MemoryStore(Mutex<HashMap<String, Vec<u8>>>);

impl SecretStore for MemoryStore {
    fn read(&self, key: &CredentialKey) -> Result<Secret, StoreError> {
        self.0
            .lock()
            .unwrap()
            .get(&key.target())
            .cloned()
            .map(Secret::from_bytes)
            .ok_or(StoreError::Missing)
    }
    fn create_new(&self, key: &CredentialKey, value: &Secret) -> Result<(), StoreError> {
        let mut map = self.0.lock().unwrap();
        if map.contains_key(&key.target()) {
            return Err(StoreError::Unavailable);
        }
        map.insert(key.target(), value.as_bytes().to_vec());
        Ok(())
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn image() -> TrustedServerImage {
    TrustedServerImage::from_configuration(&std::env::current_exe().unwrap()).unwrap()
}

async fn connect(roots: &Roots, library: LibraryId, store: &Arc<MemoryStore>) -> ApplicationClient {
    let key = CredentialKey::owner(CredentialEnvironment::Test, library);
    ApplicationClient::connect(
        &roots.endpoint(),
        &image(),
        library,
        None,
        &CredentialSource::Store {
            store: store.clone(),
            key,
        },
        BufferBudget::new(),
    )
    .await
    .unwrap()
}

/// Deterministic synthetic frame source: real geometry and bytes, no screen access.
struct SyntheticAdapter {
    width: u32,
    height: u32,
    mode_answer: Answer,
    warnings: Vec<String>,
    calls: AtomicUsize,
    cancel_calls: AtomicUsize,
    cancelled: Mutex<Option<Uuid>>,
    /// Optional hold: a gated adapter keeps its *first* session inside `capture` until the
    /// test releases it, so a competing request can be observed while one native session
    /// really is in flight. `None` means the session completes immediately.
    gate: Option<Arc<Gate>>,
}

/// Two-way synthetic rendezvous used only by the concurrency test.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    wait_open: Condvar,
    entered: Mutex<bool>,
}

impl Gate {
    /// Called from inside a native session: announce entry and wait for release.
    fn enter_and_wait(&self) {
        *self.entered.lock().unwrap() = true;
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.wait_open.wait(open).unwrap();
        }
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.wait_open.notify_all();
    }

    /// Asynchronous observation of the entry: it yields to the runtime instead of
    /// blocking the only worker thread, so the competing request really is dispatched.
    async fn wait_entered_async(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !*self.entered.lock().unwrap() {
            assert!(
                Instant::now() < deadline,
                "the native session never entered the synthetic adapter"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Accept,
    Cancel,
}

impl SyntheticAdapter {
    fn new(width: u32, height: u32, mode_answer: Answer) -> Arc<Self> {
        Self::with_warnings(width, height, mode_answer, vec!["synthetic_adapter".into()])
    }

    fn with_warnings(
        width: u32,
        height: u32,
        mode_answer: Answer,
        warnings: Vec<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            width,
            height,
            mode_answer,
            warnings,
            calls: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
            cancelled: Mutex::new(None),
            gate: None,
        })
    }

    fn gated(width: u32, height: u32, mode_answer: Answer) -> (Arc<Self>, Arc<Gate>) {
        let gate = Arc::new(Gate::default());
        let adapter = Arc::new(Self {
            width,
            height,
            mode_answer,
            warnings: vec!["synthetic_adapter".into()],
            calls: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
            cancelled: Mutex::new(None),
            gate: Some(gate.clone()),
        });
        (adapter, gate)
    }

    fn frame(&self, request: &CaptureRequest) -> CapturedDib {
        // A deterministic gradient: the canonical round trip must reproduce it byte for byte.
        let mut bytes = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height {
            for x in 0..self.width {
                bytes.extend_from_slice(&[(x % 251) as u8, (y % 241) as u8, 0x40, 0xff]);
            }
        }
        assert_eq!(request.mode(), CaptureMode::Region);
        CapturedDib::from_bgra_topdown(self.width, self.height, self.width * 4, bytes)
            .expect("synthetic dib")
    }
}

impl NativeCaptureAdapter for SyntheticAdapter {
    fn capture(&self, request: CaptureRequest) -> Result<CaptureOutcome, CaptureError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.cancelled.lock().unwrap() = Some(request.operation_id());
        if let Some(gate) = &self.gate {
            gate.enter_and_wait();
        }
        match self.mode_answer {
            Answer::Accept => Ok(CaptureOutcome::Accepted(Box::new(CaptureResult {
                context: CaptureContext {
                    device: "synthetic-display".to_string(),
                    app: "synthetic-adapter".to_string(),
                    title: "Synthetic frame".to_string(),
                    target: CaptureTarget::Region,
                    sources: vec![CaptureTarget::Region],
                    bounds: PhysicalBounds {
                        x: -120,
                        y: 40,
                        width: self.width,
                        height: self.height,
                    },
                },
                target: None,
                frame_dpi: 96,
                frozen_at_ms: 1_790_586_900_000,
                observed_at_ms: 1_790_586_900_010,
                dib: self.frame(&request),
                warnings: self.warnings.clone(),
                request,
            }))),
            Answer::Cancel => Ok(CaptureOutcome::Cancelled {
                operation_id: request.operation_id(),
                reason: CancelReason::Escape,
            }),
        }
    }

    fn cancel(&self, operation_id: Uuid) -> bool {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        *self.cancelled.lock().unwrap() == Some(operation_id)
    }
}

/// A native adapter that accepts pixels but substitutes the returned intent. It uses the
/// real synthetic frame source and composition, not a mocked storage or command handler.
#[derive(Clone, Copy)]
enum Spoof {
    Operation,
    Mode,
    Trigger,
}

struct SpoofingAdapter {
    inner: Arc<SyntheticAdapter>,
    spoof: Spoof,
}

impl NativeCaptureAdapter for SpoofingAdapter {
    fn capture(&self, request: CaptureRequest) -> Result<CaptureOutcome, CaptureError> {
        let outcome = self.inner.capture(request.clone())?;
        match outcome {
            CaptureOutcome::Accepted(mut result) => {
                result.request = match self.spoof {
                    Spoof::Operation => {
                        CaptureRequest::new(Uuid::new_v4(), request.mode(), request.trigger())
                    }
                    Spoof::Mode => CaptureRequest::new(
                        request.operation_id(),
                        CaptureMode::Desktop,
                        request.trigger(),
                    ),
                    Spoof::Trigger => CaptureRequest::new(
                        request.operation_id(),
                        request.mode(),
                        CaptureTrigger::Hotkey,
                    ),
                };
                Ok(CaptureOutcome::Accepted(result))
            }
            other => Ok(other),
        }
    }

    fn cancel(&self, operation_id: Uuid) -> bool {
        self.inner.cancel(operation_id)
    }
}

fn options(root: &Path, capture: CaptureOptions, store: Arc<MemoryStore>) -> BootstrapOptions {
    BootstrapOptions {
        root: root.to_path_buf(),
        mode: BootstrapMode::Serve,
        store,
        capture,
    }
}

/// Reopen the real published stage and validate its fixed files against bootstrap and
/// authenticated owner values, not merely a mock or a self-reported host receipt.
fn check_stage(
    roots: &Roots,
    library: LibraryId,
    principal: PrincipalId,
    operation: sm_domain::OperationId,
    identity: (RecordId, AssetId),
    dimensions: (u32, u32),
    expected_warnings: &[String],
) {
    let (width, height) = dimensions;
    let dir = roots
        .path()
        .join("staging")
        .join(operation.as_uuid().to_string());
    let manifest = fs::read(dir.join("manifest.json")).unwrap();
    let context = fs::read(dir.join("context.json")).unwrap();
    let pixels = fs::read(dir.join("pixels.rgba")).unwrap();
    let canonical_files = roots.published_files();
    assert_eq!(canonical_files.len(), 1);
    let canonical = fs::read(&canonical_files[0]).unwrap();
    let decoded = sm_codec::decode_rgba(&canonical).unwrap();
    assert_eq!(
        decoded.pixels(),
        pixels,
        "verified canonical is this staged frame"
    );
    let profile = sm_codec::encoder_fingerprint();
    let envelope = validate_recovery_envelope(
        &manifest,
        &context,
        &pixels,
        StagingExpectations {
            library_id: library,
            principal_id: principal,
            operation_id: operation,
            mode: CaptureTarget::Region,
            encoder_fingerprint: &profile,
        },
    )
    .unwrap();
    assert_eq!(envelope.schema_version(), 2);
    assert_eq!(envelope.warnings(), Some(expected_warnings));
    assert_eq!(envelope.path_policy(), Some(RECOVERY_PATH_POLICY));
    let validated = envelope.staging();
    assert_eq!(
        envelope.automatic_provenance(),
        Some(Provenance::Automatic {
            when: validated.captured_at().to_owned(),
            source: validated.context().target,
        })
    );
    assert_eq!(validated.record_id(), identity.0);
    assert_eq!(validated.asset_id(), identity.1);
    assert_eq!(validated.frame_id().get_version_num(), 4);
    assert_eq!((validated.width(), validated.height()), (width, height));
    assert_eq!(validated.stride(), width * 4);
    assert_eq!(
        validated.byte_length(),
        u64::from(width) * u64::from(height) * 4
    );
    assert_eq!(validated.captured_at(), "2026-09-28T09:15:00.000Z");
    assert_eq!(
        validated.context().bounds,
        PhysicalBounds {
            x: -120,
            y: 40,
            width,
            height,
        }
    );
    assert_eq!(validated.context().app, "synthetic-adapter");
    for y in 0..height {
        for x in 0..width {
            let offset = ((y * width + x) * 4) as usize;
            assert_eq!(
                &pixels[offset..offset + 4],
                &[0x40, (y % 241) as u8, (x % 251) as u8, 0xff]
            );
        }
    }
}

fn command(mode: CaptureMode) -> CaptureCommandParams {
    CaptureCommandParams {
        operation_id: sm_domain::OperationId::new(Uuid::new_v4()),
        mode,
    }
}

#[test]
fn s04_processing_shared_client_saved_policy_plan_controls_disconnect_and_reopen() {
    use sm_domain::{OperationId, Revision};
    use sm_protocol::processing::*;
    runtime().block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let adapter = SyntheticAdapter::new(64, 48, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let client = connect(&roots, library, &store).await;
        assert!(!client.status().await.unwrap().ai_enabled);
        let receipt = match client
            .capture_command(command(CaptureMode::Region), 10_000, None)
            .await
            .unwrap()
        {
            WireCaptureOutcome::Saved { receipt } => receipt,
            _ => panic!("saved"),
        };
        let policy = client
            .processing(ProcessingRequest::UpdatePolicy {
                operation_id: OperationId::new(Uuid::new_v4()),
                expected_revision: Revision::new(1),
                value: ProcessingPolicy {
                    mode: ProcessingMode::Manual,
                    ..ProcessingPolicy::default()
                },
            })
            .await
            .unwrap();
        let pol = match policy {
            ProcessingReply::Policy(p) => p,
            _ => panic!("policy"),
        };
        let plan = ProcessingRequest::Plan {
            operation_id: OperationId::new(Uuid::new_v4()),
            expected_policy_revision: pol.revision,
            records: vec![receipt.record_id],
            kind: WorkKind::Analyze,
            profile_id: None,
        };
        let b = match client.processing(plan.clone()).await.unwrap() {
            ProcessingReply::Batch(b) => b,
            _ => panic!("batch"),
        };
        assert_eq!(b.jobs[0].reason, Some(BlockReason::HandlerUnavailable));
        assert_eq!(b.jobs[0].asset_id, receipt.asset_id);
        drop(client);
        let client = connect(&roots, library, &store).await;
        let replay = match client.processing(plan).await.unwrap() {
            ProcessingReply::Batch(b) => b,
            _ => panic!("batch"),
        };
        assert_eq!(replay.jobs[0].id, b.jobs[0].id);
        let paused = match client
            .processing(ProcessingRequest::ControlBatch {
                id: b.id,
                expected_revision: b.revision,
                control: BatchControl::Pause,
            })
            .await
            .unwrap()
        {
            ProcessingReply::Batch(b) => b,
            _ => panic!("batch"),
        };
        assert_eq!(paused.control, BatchControl::Pause);
        client
            .processing(ProcessingRequest::UpdatePolicy {
                operation_id: OperationId::new(Uuid::new_v4()),
                expected_revision: pol.revision,
                value: ProcessingPolicy::default(),
            })
            .await
            .unwrap();
        assert!(!client.status().await.unwrap().ai_enabled);
        let bytes = client
            .read_asset(LibraryAssetRequest {
                asset_id: receipt.asset_id,
                offset: 0,
            })
            .await
            .unwrap();
        assert_eq!(bytes.sha256, receipt.canonical_sha256);
        client.request_shutdown().await.unwrap();
        drop(client);
        assert_eq!(handle.await.unwrap().unwrap().exit_code(), 0);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Disabled,
            store.clone(),
        ))
        .await
        .unwrap();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let client = connect(&roots, library, &store).await;
        let reopened = match client
            .processing(ProcessingRequest::Batch { id: b.id })
            .await
            .unwrap()
        {
            ProcessingReply::Batch(b) => b,
            _ => panic!("batch"),
        };
        assert_eq!(reopened.jobs[0].id, b.jobs[0].id);
        assert_eq!(reopened.jobs[0].reason, Some(BlockReason::AiDisabled));
        assert_eq!(reopened.jobs[0].attempt_count, 0);
        client.request_shutdown().await.unwrap();
        drop(client);
        assert_eq!(handle.await.unwrap().unwrap().exit_code(), 0);
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn s02_capture_host_01_command_commits_a_canonical_record_and_serves_its_bytes() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let adapter = SyntheticAdapter::new(64, 48, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let client = connect(&roots, library, &store).await;
        // The advertised method list of *this* composition includes the two S02 handlers.
        let status = client.status().await.unwrap();
        let methods: Vec<&str> = status
            .available_methods
            .iter()
            .map(|name| name.as_str())
            .collect();
        assert!(methods.contains(&"capture.command"));
        assert!(methods.contains(&"library.asset.read"));
        assert_eq!(methods.len(), 7);
        let params = command(CaptureMode::Region);
        let outcome = client
            .capture_command(params.clone(), 10_000, None)
            .await
            .unwrap();
        let receipt = match outcome {
            WireCaptureOutcome::Saved { receipt } => receipt,
            other => panic!("the injected adapter must produce a committed record: {other:?}"),
        };
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        let principal = Authenticator::enroll_owner(
            store.as_ref(),
            CredentialKey::owner(CredentialEnvironment::Test, library),
        )
        .unwrap()
        .principal();
        check_stage(
            &roots,
            library,
            principal,
            receipt.operation_id,
            (receipt.record_id, receipt.asset_id),
            (64, 48),
            &receipt.warnings,
        );
        // The receipt is the committed truth: library-relative path, no absolute path.
        assert_eq!(receipt.canonical_mime, "image/webp");
        assert_eq!((receipt.width, receipt.height), (64, 48));
        assert!(!receipt.canonical_sha256.is_empty());
        assert!(receipt.relative_path.starts_with("assets/"));
        assert!(receipt
            .relative_path
            .ends_with(&format!("{}/original.webp", receipt.record_id.as_uuid())));
        assert_eq!(receipt.warnings, vec!["synthetic_adapter".to_string()]);
        let relative = receipt
            .relative_path
            .strip_prefix("assets/")
            .expect("library-relative path");
        let published = roots
            .assets()
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let bytes = fs::read(&published).expect("the canonical file exists");
        assert_eq!(bytes.len().to_string(), receipt.byte_size);
        // The stored container really decodes back to the accepted synthetic pixels.
        let decoded = sm_codec::decode_rgba(&bytes).expect("a real canonical webp container");
        assert_eq!(
            (decoded.dimensions().width, decoded.dimensions().height),
            (64, 48)
        );
        for y in 0..48u32 {
            for x in 0..64u32 {
                let offset = ((y * 64 + x) * 4) as usize;
                // The DIB was BGRX; the decoded canonical image is RGBA of the same pixels.
                assert_eq!(decoded.pixels()[offset], 0x40, "red channel of ({x},{y})");
                assert_eq!(
                    decoded.pixels()[offset + 1],
                    (y % 241) as u8,
                    "green channel of ({x},{y})"
                );
                assert_eq!(
                    decoded.pixels()[offset + 2],
                    (x % 251) as u8,
                    "blue channel of ({x},{y})"
                );
                assert_eq!(decoded.pixels()[offset + 3], 0xff);
            }
        }
        // The bounded asset read returns exactly those committed bytes, chunk by chunk.
        let mut collected: Vec<u8> = Vec::new();
        let mut offset = 0u32;
        loop {
            let chunk = client
                .read_asset(LibraryAssetRequest {
                    asset_id: receipt.asset_id,
                    offset,
                })
                .await
                .unwrap();
            assert_eq!(chunk.sha256, receipt.canonical_sha256);
            assert_eq!(chunk.mime, "image/webp");
            assert_eq!(chunk.total_size, receipt.byte_size);
            let decoded_hex: Vec<u8> = chunk
                .data_hex
                .as_bytes()
                .chunks(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            collected.extend_from_slice(&decoded_hex);
            if chunk.eof {
                break;
            }
            assert!(chunk.next_offset > offset);
            offset = chunk.next_offset;
        }
        assert_eq!(collected, bytes);
        // Simulate a lost first reply: discard the original connection, reconnect with a
        // new transport request id, and demand the identical committed wire receipt.
        drop(client);
        let client = connect(&roots, library, &store).await;
        let replayed = client.capture_command(params, 10_000, None).await.unwrap();
        let replayed = match replayed {
            WireCaptureOutcome::Saved { receipt } => receipt,
            other => panic!("reconnect must replay the saved receipt: {other:?}"),
        };
        assert_eq!(
            serde_json::to_vec(&replayed).unwrap(),
            serde_json::to_vec(&receipt).unwrap(),
            "the replay must be byte-equivalent even across a new session"
        );
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(roots.published_files(), vec![published]);
        // A missing asset is a typed refusal, never an empty success.
        match client
            .read_asset(LibraryAssetRequest {
                asset_id: sm_domain::AssetId::new(Uuid::from_u128(0xbad)),
                offset: 0,
            })
            .await
        {
            Err(ClientError::Application(error)) => {
                assert_eq!(error.code, ErrorCode::AssetNotFound)
            }
            other => panic!("an unknown asset must be refused: {other:?}"),
        }
        stop(&client, handle).await;
    });
}

#[test]
fn s02_t09d_host_rejects_spoofed_accepted_request_without_saving_or_replaying() {
    let rt = runtime();
    rt.block_on(async {
        for spoof in [Spoof::Operation, Spoof::Mode, Spoof::Trigger] {
            let roots = Roots::new();
            let store = Arc::new(MemoryStore::default());
            let inner = SyntheticAdapter::new(4, 4, Answer::Accept);
            let adapter = Arc::new(SpoofingAdapter {
                inner: inner.clone(),
                spoof,
            });
            let host = bootstrap(options(
                &roots.path(),
                CaptureOptions::Injected(adapter),
                store.clone(),
            ))
            .await
            .unwrap();
            let library = host.library_id();
            let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
            let client = connect(&roots, library, &store).await;
            let params = command(CaptureMode::Region);
            // A second invocation after a failed session may try capture again; it must
            // never replay a spoofed receipt or trigger extra native work per invocation.
            for expected_calls in 1..=2 {
                match client.capture_command(params.clone(), 10_000, None).await {
                    Err(ClientError::Application(error)) => {
                        assert_eq!(error.code, ErrorCode::InvalidRequest);
                        assert!(!error.retryable);
                        let safe = serde_json::to_string(&error).unwrap();
                        assert!(!safe.contains(&params.operation_id.as_uuid().to_string()));
                        assert!(!safe.contains("assets/"));
                    }
                    other => panic!("spoofed native request must be refused: {other:?}"),
                }
                assert_eq!(inner.calls.load(Ordering::SeqCst), expected_calls);
                assert!(roots.published_files().is_empty());
            }
            stop(&client, handle).await;
            let worker = StorageWorker::start(roots.lease()).unwrap();
            assert!(matches!(
                worker
                    .lookup_operation(
                        params.operation_id,
                        PrincipalId::new(Uuid::new_v4()),
                        CaptureTarget::Region,
                    )
                    .await
                    .unwrap(),
                LookupOutcome::Unknown
            ));
            worker.shutdown(Duration::from_secs(5)).await.unwrap();
        }
    });
}

#[test]
fn s02_capture_host_02_cancelled_session_is_not_a_stored_record() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let adapter = SyntheticAdapter::new(16, 16, Answer::Cancel);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let client = connect(&roots, library, &store).await;
        let params = command(CaptureMode::Region);
        let operation = params.operation_id;
        let outcome = client.capture_command(params, 10_000, None).await.unwrap();
        match outcome {
            WireCaptureOutcome::Cancelled {
                operation_id,
                reason,
            } => {
                assert_eq!(operation_id, operation);
                assert_eq!(reason, sm_protocol::application::CancelReason::Escape);
            }
            other => panic!("a cancelled session must not store a record: {other:?}"),
        }
        // No canonical file was published by the cancelled session.
        assert!(!roots.assets().exists() || fs::read_dir(roots.assets()).unwrap().count() == 0);
        stop(&client, handle).await;
    });
}

#[test]
fn s02_capture_host_03_composition_without_a_capture_owner_supports_no_capture() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Disabled,
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let client = connect(&roots, library, &store).await;
        let status = client.status().await.unwrap();
        assert_eq!(status.available_methods.len(), 5);
        assert!(!status
            .available_methods
            .iter()
            .any(|name| name.as_str() == "capture.command"));
        // The handler exists in the registry but is not owned by this composition.
        match client
            .capture_command(command(CaptureMode::Desktop), 5_000, None)
            .await
        {
            Err(ClientError::Application(error)) => {
                assert_eq!(error.code, ErrorCode::MethodNotSupported)
            }
            other => panic!("a composition without a capture owner must refuse: {other:?}"),
        }
        stop(&client, handle).await;
    });
}

#[test]
fn s02_capture_host_05_mode_and_foreign_principal_conflict_without_metadata() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        // Seed a foreign journal entry through the real SQLite writer in this owned
        // synthetic root. The host's owner credential is enrolled only after reopening.
        let foreign = command(CaptureMode::Region);
        let worker = StorageWorker::start(roots.lease()).unwrap();
        let image = sm_codec::RgbaImage::from_rgba(2, 2, vec![0x77; 16]).unwrap();
        let canonical = sm_codec::encode_exact_lossless_verified(&image)
            .unwrap()
            .into_canonical();
        worker
            .commit_image_capture(CommitImageCapture {
                operation_id: foreign.operation_id,
                principal_id: PrincipalId::new(Uuid::new_v4()),
                record_id: RecordId::new(Uuid::new_v4()),
                asset_id: AssetId::new(Uuid::new_v4()),
                asset_type: AssetType::Original,
                mode: CaptureTarget::Region,
                width: 2,
                height: 2,
                capture_context: CaptureContext {
                    device: "synthetic-display".into(),
                    app: "synthetic-adapter".into(),
                    title: "Synthetic frame".into(),
                    target: CaptureTarget::Region,
                    sources: vec![CaptureTarget::Region],
                    bounds: PhysicalBounds {
                        x: 0,
                        y: 0,
                        width: 2,
                        height: 2,
                    },
                },
                provenances: vec![Provenance::Manual {
                    when: "2026-09-28T09:15:00.000Z".into(),
                }],
                warnings: vec!["synthetic_adapter".into()],
                canonical: canonical.bytes().to_vec(),
            })
            .await
            .unwrap();
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        // The DB ownership conflict must win even when a stage directory also exists.
        fs::create_dir_all(
            roots
                .path()
                .join("staging")
                .join(foreign.operation_id.as_uuid().to_string()),
        )
        .unwrap();
        let adapter = SyntheticAdapter::new(4, 4, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let client = connect(&roots, library, &store).await;
        let own = command(CaptureMode::Region);
        assert!(matches!(
            client
                .capture_command(own.clone(), 10_000, None)
                .await
                .unwrap(),
            WireCaptureOutcome::Saved { .. }
        ));
        let changed_mode = client
            .capture_command(
                CaptureCommandParams {
                    operation_id: own.operation_id,
                    mode: CaptureMode::Desktop,
                },
                10_000,
                None,
            )
            .await;
        let foreign_principal = client.capture_command(foreign, 10_000, None).await;
        let conflicts: Vec<_> = [changed_mode, foreign_principal]
            .into_iter()
            .map(|outcome| match outcome {
                Err(ClientError::Application(error)) => error,
                other => panic!("conflict must reveal no receipt: {other:?}"),
            })
            .collect();
        assert_eq!(conflicts[0].code, ErrorCode::InvalidRequest);
        assert_eq!(
            serde_json::to_vec(&conflicts[0]).unwrap(),
            serde_json::to_vec(&conflicts[1]).unwrap(),
            "foreign and changed-mode conflicts must have identical generic envelopes"
        );
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(roots.published_files().len(), 2);
        stop(&client, handle).await;
    });
}

#[test]
fn s02_t09d_host_unknown_staged_ids_refuse_on_reconnect_before_native() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let incomplete = command(CaptureMode::Region);
        let complete = command(CaptureMode::Region);
        // Hand-placed synthetic stage entries have no DB journal rows. Neither contents
        // nor the presence of a manifest may grant a second screen freeze.
        let stage_root = roots.path().join("staging");
        fs::create_dir(&stage_root).unwrap();
        let incomplete_dir = stage_root.join(incomplete.operation_id.as_uuid().to_string());
        fs::create_dir(&incomplete_dir).unwrap();
        fs::write(incomplete_dir.join("pixels.rgba.tmp"), b"synthetic").unwrap();
        let complete_dir = stage_root.join(complete.operation_id.as_uuid().to_string());
        fs::create_dir(&complete_dir).unwrap();
        for (name, contents) in [
            ("pixels.rgba", b"synthetic".as_slice()),
            ("context.json", b"{}".as_slice()),
            ("manifest.json", b"not-parsed-by-probe".as_slice()),
        ] {
            fs::write(complete_dir.join(name), contents).unwrap();
        }
        let adapter = SyntheticAdapter::new(4, 4, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        assert!(matches!(
            HostLease::acquire(roots.layout()),
            Err(sm_isolation::LeaseError::Busy)
        ));
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        for params in [incomplete, complete] {
            for _ in 0..2 {
                let client = connect(&roots, library, &store).await;
                match client.capture_command(params.clone(), 10_000, None).await {
                    Err(ClientError::Application(error)) => {
                        assert_eq!(error.code, ErrorCode::InvalidRequest);
                        assert!(!error.retryable);
                        assert!(!serde_json::to_string(&error).unwrap().contains("principal"));
                    }
                    other => panic!("unknown staged operation must be refused: {other:?}"),
                }
                assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
            }
        }
        assert_eq!(roots.published_files().len(), 0);
        let client = connect(&roots, library, &store).await;
        stop(&client, handle).await;
    });
}

#[test]
fn s02_t09d_host_unsafe_stage_root_refuses_unknown_before_native() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let sentinel = roots.path().join("staging");
        fs::write(&sentinel, b"unsafe-stage-root").unwrap();
        let adapter = SyntheticAdapter::new(4, 4, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let client = connect(&roots, library, &store).await;
        match client
            .capture_command(command(CaptureMode::Region), 10_000, None)
            .await
        {
            Err(ClientError::Application(error)) => {
                assert_eq!(error.code, ErrorCode::RuntimeUnavailable);
            }
            other => panic!("unsafe stage root must fail closed: {other:?}"),
        }
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read(sentinel).unwrap(), b"unsafe-stage-root");
        stop(&client, handle).await;
    });
}

#[test]
fn s02_capture_host_06_concurrent_same_id_never_runs_two_native_sessions() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let (adapter, gate) = SyntheticAdapter::gated(8, 8, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let first = connect(&roots, library, &store).await;
        let second = connect(&roots, library, &store).await;
        let params = command(CaptureMode::Region);
        let first_call = tokio::spawn({
            let params = params.clone();
            async move { first.capture_command(params, 10_000, None).await }
        });
        gate.wait_entered_async(Duration::from_secs(5)).await;
        let concurrent = tokio::time::timeout(
            Duration::from_secs(5),
            second.capture_command(params.clone(), 10_000, None),
        )
        .await;
        // Always unblock the native thread before inspecting the competing result.
        gate.release();
        match concurrent.expect("duplicate must get a bounded response") {
            Err(ClientError::Application(error)) => {
                assert_eq!(error.code, ErrorCode::CaptureBusy);
                assert!(error.retryable);
            }
            Ok(WireCaptureOutcome::Saved { .. }) => {}
            other => panic!("duplicate must be Busy or saved: {other:?}"),
        }
        let saved = first_call.await.unwrap().unwrap();
        assert!(matches!(saved, WireCaptureOutcome::Saved { .. }));
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(roots.published_files().len(), 1);
        let replay = second.capture_command(params, 10_000, None).await.unwrap();
        assert_eq!(
            serde_json::to_vec(&replay).unwrap(),
            serde_json::to_vec(&saved).unwrap()
        );
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        stop(&second, handle).await;
    });
}

#[test]
fn s02_t09e_disconnected_waiter_keeps_command_save_and_exact_replay() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let sentinel = roots.path().join("sentinel.txt");
        fs::write(&sentinel, b"owned-test-sentinel").unwrap();
        let store = Arc::new(MemoryStore::default());
        let (adapter, gate) = SyntheticAdapter::gated(16, 16, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let waiter = connect(&roots, library, &store).await;
        let params = command(CaptureMode::Region);
        let first = tokio::spawn({
            let params = params.clone();
            async move { waiter.capture_command(params, 10_000, None).await }
        });
        gate.wait_entered_async(Duration::from_secs(5)).await;
        // Abort the connection-owned waiter before the native adapter returns Accepted.
        // The task registered at admission must still own its result and commit.
        first.abort();
        let _ = first.await;
        let cancel_deadline = Instant::now() + Duration::from_secs(5);
        while adapter.cancel_calls.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < cancel_deadline,
                "disconnect must signal native cancel"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        gate.release();
        let client = connect(&roots, library, &store).await;
        let deadline = Instant::now() + Duration::from_secs(5);
        let receipt = loop {
            match client.capture_command(params.clone(), 10_000, None).await {
                Ok(WireCaptureOutcome::Saved { receipt }) => break receipt,
                Err(ClientError::Application(error)) if error.code == ErrorCode::CaptureBusy => {
                    assert!(Instant::now() < deadline, "accepted task must finish");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                other => panic!("only Busy then Saved replay is allowed: {other:?}"),
            }
        };
        assert_eq!(receipt.operation_id, params.operation_id);
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(roots.published_files().len(), 1);
        assert_eq!(fs::read(sentinel).unwrap(), b"owned-test-sentinel");
        let replay = client.capture_command(params, 10_000, None).await.unwrap();
        assert_eq!(
            serde_json::to_vec(&replay).unwrap(),
            serde_json::to_vec(&WireCaptureOutcome::Saved { receipt }).unwrap()
        );
        stop(&client, handle).await;
    });
}

#[test]
fn s02_t09e_two_registered_commands_refuse_third_before_freeze() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let (adapter, gate) = SyntheticAdapter::gated(8, 8, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let mut waiting = Vec::new();
        for _ in 0..2 {
            let client = connect(&roots, library, &store).await;
            let params = command(CaptureMode::Region);
            waiting.push(tokio::spawn(async move {
                client.capture_command(params, 10_000, None).await
            }));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while adapter.calls.load(Ordering::SeqCst) != 2 {
            assert!(
                Instant::now() < deadline,
                "both admitted sessions must enter"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let third = connect(&roots, library, &store).await;
        match third
            .capture_command(command(CaptureMode::Region), 10_000, None)
            .await
        {
            Err(ClientError::Application(error)) => {
                assert_eq!(error.code, ErrorCode::CaptureBackpressure)
            }
            other => panic!("third command must be refused before freeze: {other:?}"),
        }
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
        gate.release();
        for waiter in waiting {
            assert!(matches!(
                waiter.await.unwrap().unwrap(),
                WireCaptureOutcome::Saved { .. }
            ));
        }
        assert_eq!(roots.published_files().len(), 2);
        stop(&third, handle).await;
    });
}

#[test]
fn s02_t09e_shutdown_timeout_keeps_lease_until_admitted_command_finishes() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let sentinel = roots.path().join("sentinel.txt");
        fs::write(&sentinel, b"preserve-on-timeout").unwrap();
        let store = Arc::new(MemoryStore::default());
        let (adapter, gate) = SyntheticAdapter::gated(8, 8, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let waiter = connect(&roots, library, &store).await;
        let params = command(CaptureMode::Region);
        let request = tokio::spawn({
            let params = params.clone();
            async move { waiter.capture_command(params, 10_000, None).await }
        });
        gate.wait_entered_async(Duration::from_secs(5)).await;
        let control = connect(&roots, library, &store).await;
        assert!(matches!(
            control.request_shutdown().await.unwrap().state,
            sm_protocol::application::HostState::Stopping
        ));
        request.abort();
        let _ = request.await;
        tokio::time::sleep(sm_host::SHUTDOWN_BUDGET + DEFAULT_DRAIN + Duration::from_millis(250))
            .await;
        assert!(
            !handle.is_finished(),
            "stop cannot close while native result is pending"
        );
        assert!(matches!(
            HostLease::acquire(roots.layout()),
            Err(sm_isolation::LeaseError::Busy)
        ));
        gate.release();
        let outcome = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("admitted command must drain after release")
            .unwrap()
            .unwrap();
        assert!(outcome.storage_closed);
        assert!(
            outcome.serve_ok,
            "the listener itself drained without failure"
        );
        assert!(!outcome.command_drain_in_budget);
        assert_ne!(outcome.exit_code(), sm_host::EXIT_OK);
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(roots.published_files().len(), 1);
        assert_eq!(fs::read(sentinel).unwrap(), b"preserve-on-timeout");
        // Reopen the DB under the same enrolled principal and verify the committed
        // identity, rather than inferring success from the asset alone.
        let principal = Authenticator::enroll_owner(
            store.as_ref(),
            CredentialKey::owner(CredentialEnvironment::Test, library),
        )
        .unwrap()
        .principal();
        let worker = StorageWorker::start(roots.lease()).unwrap();
        let stored = worker
            .lookup_operation(params.operation_id, principal, CaptureTarget::Region)
            .await
            .unwrap();
        let stored = match stored {
            LookupOutcome::Saved(stored) => stored,
            other => panic!("the accepted image must survive shutdown: {other:?}"),
        };
        assert_eq!(stored.operation_id, params.operation_id);
        assert_eq!(stored.principal_id, principal);
        let asset = worker
            .read_asset_chunk(stored.asset_id, 0, 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(asset.bytes, fs::read(&roots.published_files()[0]).unwrap());
        assert!(stored.relative_path.starts_with("assets/"));
        assert!(stored
            .relative_path
            .ends_with(&format!("/{}/original.webp", stored.record_id.as_uuid())));
        assert_eq!(
            stored.record_id.as_uuid().to_string(),
            roots.published_files()[0]
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}

#[test]
fn s02_t09g_command_stage_failure_after_preflight_never_saves_or_refreezes() {
    runtime().block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let (adapter, gate) = SyntheticAdapter::gated(8, 8, Answer::Accept);
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::Injected(adapter.clone()),
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let waiter = connect(&roots, library, &store).await;
        let params = command(CaptureMode::Region);
        let request = tokio::spawn({
            let params = params.clone();
            async move { waiter.capture_command(params, 10_000, None).await }
        });
        gate.wait_entered_async(Duration::from_secs(5)).await;
        // Preflight already returned Absent. Own a synthetic operation directory now;
        // stage's exclusive create must fail, preserving its sentinel untouched.
        let dir = roots
            .path()
            .join("staging")
            .join(params.operation_id.as_uuid().to_string());
        fs::create_dir_all(&dir).unwrap();
        let sentinel = dir.join("keep.sentinel");
        fs::write(&sentinel, b"owned-stage-sentinel").unwrap();
        gate.release();
        let failure = request.await.unwrap();
        assert!(matches!(failure, Err(ClientError::Application(_))));
        assert_eq!(fs::read(&sentinel).unwrap(), b"owned-stage-sentinel");
        assert!(!dir.join("manifest.json").exists());
        assert!(roots.published_files().is_empty());
        let client = connect(&roots, library, &store).await;
        assert!(matches!(
            client.capture_command(params.clone(), 10_000, None).await,
            Err(ClientError::Application(_))
        ));
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        stop(&client, handle).await;
        let principal = Authenticator::enroll_owner(
            store.as_ref(),
            CredentialKey::owner(CredentialEnvironment::Test, library),
        )
        .unwrap()
        .principal();
        let worker = StorageWorker::start(roots.lease()).unwrap();
        assert!(matches!(
            worker
                .lookup_operation(params.operation_id, principal, CaptureTarget::Region)
                .await
                .unwrap(),
            LookupOutcome::Unknown
        ));
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        assert_eq!(fs::read(sentinel).unwrap(), b"owned-stage-sentinel");
    });
}

#[test]
fn s02_t09f_hotkey_consumer_persists_without_shutdown() {
    runtime().block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let adapter = SyntheticAdapter::new(8, 8, Answer::Accept);
        let outlet = Arc::new(OnceLock::<Arc<dyn HotkeySink>>::new());
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::InjectedHotkey {
                adapter: adapter.clone(),
                sink: outlet.clone(),
            },
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let sink = outlet.get().unwrap();
        let reservation = sink.try_reserve(CaptureMode::Region).unwrap();
        let operation_id = Uuid::new_v4();
        let request =
            CaptureRequest::new(operation_id, CaptureMode::Region, CaptureTrigger::Hotkey);
        reservation.deliver(adapter.capture(request));
        let deadline = Instant::now() + Duration::from_secs(5);
        while roots.published_files().is_empty() {
            assert!(Instant::now() < deadline, "hotkey consumer did not persist");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let control = connect(&roots, library, &store).await;
        control.request_shutdown().await.unwrap();
        let outcome = handle.await.unwrap().unwrap();
        assert_eq!(outcome.exit_code(), sm_host::EXIT_OK);
        let principal = Authenticator::enroll_owner(
            store.as_ref(),
            CredentialKey::owner(CredentialEnvironment::Test, library),
        )
        .unwrap()
        .principal();
        let worker = StorageWorker::start(roots.lease()).unwrap();
        assert!(matches!(
            worker
                .lookup_operation(
                    sm_domain::OperationId::new(operation_id),
                    principal,
                    CaptureTarget::Region
                )
                .await
                .unwrap(),
            LookupOutcome::Saved(_)
        ));
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}

#[test]
fn s02_t09g_hotkey_stage_failure_preserves_incomplete_input_without_saved() {
    runtime().block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let adapter = SyntheticAdapter::new(8, 8, Answer::Accept);
        let outlet = Arc::new(OnceLock::<Arc<dyn HotkeySink>>::new());
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::InjectedHotkey {
                adapter: adapter.clone(),
                sink: outlet.clone(),
            },
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let operation = Uuid::new_v4();
        let dir = roots.path().join("staging").join(operation.to_string());
        fs::create_dir_all(&dir).unwrap();
        let sentinel = dir.join("keep.sentinel");
        fs::write(&sentinel, b"hotkey-incomplete-sentinel").unwrap();
        let reservation = outlet
            .get()
            .unwrap()
            .try_reserve(CaptureMode::Region)
            .unwrap();
        reservation.deliver(adapter.capture(CaptureRequest::new(
            operation,
            CaptureMode::Region,
            CaptureTrigger::Hotkey,
        )));
        let client = connect(&roots, library, &store).await;
        stop(&client, handle).await; // joins the hotkey consumer and its file lane
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fs::read(sentinel).unwrap(), b"hotkey-incomplete-sentinel");
        assert!(!dir.join("manifest.json").exists());
        assert!(roots.published_files().is_empty());
        let principal = Authenticator::enroll_owner(
            store.as_ref(),
            CredentialKey::owner(CredentialEnvironment::Test, library),
        )
        .unwrap()
        .principal();
        let worker = StorageWorker::start(roots.lease()).unwrap();
        assert!(matches!(
            worker
                .lookup_operation(
                    sm_domain::OperationId::new(operation),
                    principal,
                    CaptureTarget::Region,
                )
                .await
                .unwrap(),
            LookupOutcome::Unknown
        ));
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}

#[test]
fn s02_t09f_hotkey_permits_survive_shutdown_and_commit_under_owner() {
    runtime().block_on(async {
        let roots = Roots::new();
        let sentinel = roots.path().join("sentinel.txt");
        fs::write(&sentinel, b"keep-hotkey-sentinel").unwrap();
        let store = Arc::new(MemoryStore::default());
        let adapter = SyntheticAdapter::with_warnings(8, 8, Answer::Accept, vec![]);
        let outlet = Arc::new(OnceLock::<Arc<dyn HotkeySink>>::new());
        let host = bootstrap(options(
            &roots.path(),
            CaptureOptions::InjectedHotkey {
                adapter: adapter.clone(),
                sink: outlet.clone(),
            },
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        let sink = outlet.get().unwrap().clone();
        let first = sink.try_reserve(CaptureMode::Region).unwrap();
        let second = sink.try_reserve(CaptureMode::Region).unwrap();
        assert!(
            sink.try_reserve(CaptureMode::Region).is_none(),
            "full queue refuses pre-freeze"
        );
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let control = connect(&roots, library, &store).await;
        control.request_shutdown().await.unwrap();
        tokio::time::sleep(sm_host::SHUTDOWN_BUDGET + DEFAULT_DRAIN + Duration::from_millis(250))
            .await;
        assert!(
            !handle.is_finished(),
            "issued native permits keep the host and lease alive"
        );
        assert!(matches!(
            HostLease::acquire(roots.layout()),
            Err(sm_isolation::LeaseError::Busy)
        ));
        let operation_id = Uuid::new_v4();
        assert_eq!(operation_id.get_version_num(), 4);
        let request =
            CaptureRequest::new(operation_id, CaptureMode::Region, CaptureTrigger::Hotkey);
        first.deliver(adapter.capture(request));
        // The second reservation has no accepted pixels; dropping it releases exactly one slot.
        drop(second);
        let outcome = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(outcome.storage_closed);
        assert!(!outcome.command_drain_in_budget || !outcome.hotkey_drain_in_budget);
        assert_ne!(outcome.exit_code(), sm_host::EXIT_OK);
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fs::read(sentinel).unwrap(), b"keep-hotkey-sentinel");
        assert_eq!(roots.published_files().len(), 1);
        let principal = Authenticator::enroll_owner(
            store.as_ref(),
            CredentialKey::owner(CredentialEnvironment::Test, library),
        )
        .unwrap()
        .principal();
        let worker = StorageWorker::start(roots.lease()).unwrap();
        let receipt = worker
            .lookup_operation(
                sm_domain::OperationId::new(operation_id),
                principal,
                CaptureTarget::Region,
            )
            .await
            .unwrap();
        let saved = match receipt {
            LookupOutcome::Saved(saved) => saved,
            other => panic!("accepted hotkey must have canonical DB receipt: {other:?}"),
        };
        assert_eq!(saved.principal_id, principal);
        assert_eq!(saved.operation_id.as_uuid(), operation_id);
        assert!(saved.warnings.is_empty());
        check_stage(
            &roots,
            library,
            principal,
            saved.operation_id,
            (saved.record_id, saved.asset_id),
            (8, 8),
            &saved.warnings,
        );
        let asset = worker
            .read_asset_chunk(saved.asset_id, 0, 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(asset.bytes, fs::read(&roots.published_files()[0]).unwrap());
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}

#[test]
fn s02_t09f_bind_failure_joins_hotkey_consumer_and_releases_lease() {
    runtime().block_on(async {
        let roots = Roots::new();
        let sentinel = roots.path().join("sentinel.txt");
        fs::write(&sentinel, b"keep-bind-sentinel").unwrap();
        // A sealed listener owns the endpoint but not the library lease.
        let occupied = Listener::bind(roots.endpoint()).unwrap();
        let store = Arc::new(MemoryStore::default());
        let outlet = Arc::new(OnceLock::<Arc<dyn HotkeySink>>::new());
        let adapter = SyntheticAdapter::new(8, 8, Answer::Accept);
        let result = bootstrap(options(
            &roots.path(),
            CaptureOptions::InjectedHotkey {
                adapter,
                sink: outlet.clone(),
            },
            store,
        ))
        .await;
        assert!(matches!(result, Err(sm_host::HostError::Endpoint)));
        assert!(
            outlet.get().is_some(),
            "consumer was spawned before bind failed"
        );
        let lease = roots.lease();
        drop(lease);
        assert_eq!(fs::read(sentinel).unwrap(), b"keep-bind-sentinel");
        drop(occupied);
    });
}

/// The trigger of a wire command is never the caller's to choose: the host records a
/// client command, and the adapter port has no way to declare a hotkey.
#[test]
fn s02_capture_host_04_adapter_port_carries_no_client_supplied_identity() {
    let adapter = SyntheticAdapter::new(4, 4, Answer::Accept);
    let operation = Uuid::from_u128(0x51);
    let outcome = adapter
        .capture(CaptureRequest::new(
            operation,
            CaptureMode::Region,
            CaptureTrigger::ClientCommand,
        ))
        .unwrap();
    match outcome {
        CaptureOutcome::Accepted(result) => {
            assert_eq!(result.request.operation_id(), operation);
            assert_eq!(result.request.trigger(), CaptureTrigger::ClientCommand);
            assert_eq!(result.request.trigger().as_str(), "client_command");
        }
        other => panic!("expected an accepted frame: {other:?}"),
    }
    assert!(adapter.cancel(operation));
    assert!(!adapter.cancel(Uuid::from_u128(0x52)));
}

/// Shut the composed host down through the ordinary authorized control path, then join the
/// serve task and confirm that the writer really closed (which releases the lease).
async fn stop(
    client: &ApplicationClient,
    handle: tokio::task::JoinHandle<Result<sm_host::ShutdownOutcome, sm_host::HostError>>,
) {
    let receipt = client.request_shutdown().await.expect("stop is authorized");
    assert!(matches!(
        receipt.state,
        sm_protocol::application::HostState::Stopping
    ));
    let outcome = handle
        .await
        .expect("the serve task joins")
        .expect("serve ends");
    assert!(
        outcome.storage_closed,
        "the writer must close before the test root is released"
    );
}
