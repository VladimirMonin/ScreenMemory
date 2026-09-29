#![cfg(windows)]
//! S02 capture composition: a real host, a synthetic *injected* adapter and the real
//! canonical commit path. No desktop pixel is read here; the native owner is covered by
//! `sm-capture`'s own live suite.
use sm_application::{
    capture::{
        CancelReason, CaptureError, CaptureMode, CaptureOutcome, CaptureRequest, CaptureResult,
        CaptureTrigger, CapturedDib, NativeCaptureAdapter,
    },
    credentials::{CredentialEnvironment, CredentialKey, Secret, SecretStore, StoreError},
    storage::{CaptureContext, CaptureTarget, PhysicalBounds},
};
use sm_domain::LibraryId;
use sm_framing::BufferBudget;
use sm_host::{bootstrap, BootstrapMode, BootstrapOptions, CaptureOptions, DEFAULT_DRAIN};
use sm_isolation::{LibraryRootResolver, RootSource};
use sm_pipe::TrustedServerImage;
use sm_protocol::{
    application::{
        CaptureCommandParams, CaptureOutcome as WireCaptureOutcome, LibraryAssetRequest,
    },
    errors::ErrorCode,
};
use sm_session::{ApplicationClient, ClientError, CredentialSource};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use uuid::Uuid;

struct Roots(PathBuf);

impl Roots {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = parent.join(format!("sm-capture-host-{}", Uuid::new_v4()));
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
    calls: AtomicUsize,
    cancelled: Mutex<Option<Uuid>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Accept,
    Cancel,
}

impl SyntheticAdapter {
    fn new(width: u32, height: u32, mode_answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            width,
            height,
            mode_answer,
            calls: AtomicUsize::new(0),
            cancelled: Mutex::new(None),
        })
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
                warnings: vec!["synthetic_adapter".to_string()],
                request,
            }))),
            Answer::Cancel => Ok(CaptureOutcome::Cancelled {
                operation_id: request.operation_id(),
                reason: CancelReason::Escape,
            }),
        }
    }

    fn cancel(&self, operation_id: Uuid) -> bool {
        let matches = *self.cancelled.lock().unwrap() == Some(operation_id);
        matches
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

fn command(mode: CaptureMode) -> CaptureCommandParams {
    CaptureCommandParams {
        operation_id: sm_domain::OperationId::new(Uuid::new_v4()),
        mode,
    }
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
        assert_eq!(methods.len(), 6);
        let outcome = client
            .capture_command(command(CaptureMode::Region), 10_000, None)
            .await
            .unwrap();
        let receipt = match outcome {
            WireCaptureOutcome::Saved { receipt } => receipt,
            other => panic!("the injected adapter must produce a committed record: {other:?}"),
        };
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
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
        assert_eq!(status.available_methods.len(), 4);
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
