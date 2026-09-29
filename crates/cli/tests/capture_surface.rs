#![cfg(windows)]
//! S02 `shot capture` surface: real host composition, synthetic injected adapter, real
//! exit codes and exactly one JSON document on stdout. No desktop pixel is read here.
use sm_application::{
    capture::{
        CancelReason, CaptureError, CaptureOutcome, CaptureRequest, CaptureResult, CapturedDib,
        NativeCaptureAdapter,
    },
    credentials::{CredentialKey, Secret, SecretStore, StoreError},
    storage::{CaptureContext, PhysicalBounds},
};
use sm_cli::{run, CliDeps, HostLauncher, LaunchError, LaunchedHost, EXIT_OK, EXIT_USAGE};
use sm_domain::LibraryId;
use sm_host::{bootstrap, BootstrapMode, BootstrapOptions, CaptureOptions, DEFAULT_DRAIN};
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
        let root = parent.join(format!("sm-cli-capture-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(child)).unwrap();
        }
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for Roots {
    fn drop(&mut self) {
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

/// Deterministic frame source; `cancel_next` flips it into the cancellation answer.
struct SyntheticAdapter {
    cancel_next: Mutex<bool>,
    calls: AtomicUsize,
}

impl SyntheticAdapter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cancel_next: Mutex::new(false),
            calls: AtomicUsize::new(0),
        })
    }
    fn cancel_next(&self) {
        *self.cancel_next.lock().unwrap() = true;
    }
}

impl NativeCaptureAdapter for SyntheticAdapter {
    fn capture(&self, request: CaptureRequest) -> Result<CaptureOutcome, CaptureError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if std::mem::take(&mut *self.cancel_next.lock().unwrap()) {
            return Ok(CaptureOutcome::Cancelled {
                operation_id: request.operation_id(),
                reason: CancelReason::Escape,
            });
        }
        let (width, height) = (32u32, 24u32);
        let mut bytes = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                bytes.extend_from_slice(&[(x % 97) as u8, (y % 89) as u8, 0x33, 0xff]);
            }
        }
        let target = request.target();
        Ok(CaptureOutcome::Accepted(Box::new(CaptureResult {
            context: CaptureContext {
                device: "synthetic-display".to_string(),
                app: "synthetic-cli-adapter".to_string(),
                title: "Synthetic frame".to_string(),
                target,
                sources: vec![target],
                bounds: PhysicalBounds {
                    x: -60,
                    y: 20,
                    width,
                    height,
                },
            },
            target: None,
            frame_dpi: 96,
            frozen_at_ms: 1_790_586_900_000,
            observed_at_ms: 1_790_586_900_000,
            dib: CapturedDib::from_bgra_topdown(width, height, width * 4, bytes).unwrap(),
            warnings: Vec::new(),
            request,
        })))
    }

    fn cancel(&self, _operation_id: Uuid) -> bool {
        false
    }
}

/// Launcher used only if the CLI decided to autostart; the composition is already served.
struct InProcessLauncher {
    store: Arc<MemoryStore>,
    adapter: Arc<SyntheticAdapter>,
    calls: AtomicUsize,
}

impl HostLauncher for InProcessLauncher {
    fn launch(
        &self,
        _executable: &Path,
        root: &Path,
    ) -> Result<Box<dyn LaunchedHost>, LaunchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let store = self.store.clone();
        let adapter = self.adapter.clone();
        let root = root.to_path_buf();
        tokio::spawn(async move {
            let host = match bootstrap(BootstrapOptions {
                capture: CaptureOptions::Injected(adapter),
                root,
                mode: BootstrapMode::Serve,
                store,
            })
            .await
            {
                Ok(host) => host,
                Err(_) => return,
            };
            let _ = host.serve(DEFAULT_DRAIN).await;
        });
        Ok(Box::new(InProcessChild))
    }
}

struct InProcessChild;

impl LaunchedHost for InProcessChild {
    fn has_exited(&mut self) -> bool {
        false
    }
    fn mark_established(&mut self) {}
    fn terminate_owned(&mut self) {}
}

fn write_config(roots: &Roots, library: LibraryId) -> PathBuf {
    let path = roots.path().join("preview.json");
    let value = serde_json::json!({
        "schema_version": 1,
        "environment": "test",
        "root": roots.path().to_string_lossy(),
        "library_id": library.as_uuid().to_string(),
        "host_executable": std::env::current_exe().unwrap().to_string_lossy(),
    });
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    path
}

fn arguments(config: &Path, rest: &[&str]) -> Vec<String> {
    let mut argv = vec![
        "--config".to_string(),
        config.to_string_lossy().into_owned(),
    ];
    argv.extend(rest.iter().map(|value| (*value).to_string()));
    argv
}

async fn shot(deps: &CliDeps, config: &Path, rest: &[&str]) -> (u8, Vec<u8>, Vec<u8>) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let exit = run(&arguments(config, rest), deps, &mut out, &mut err).await;
    (exit, out, err)
}

fn single_json(stdout: &[u8]) -> serde_json::Value {
    let text = String::from_utf8(stdout.to_vec()).unwrap();
    assert_eq!(
        text.lines().filter(|line| !line.is_empty()).count(),
        1,
        "stdout must carry exactly one JSON value: {text}"
    );
    serde_json::from_str(text.trim_end()).unwrap()
}

#[test]
fn s02_cli_06_capture_prints_one_receipt_and_writes_an_explicit_copy() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let adapter = SyntheticAdapter::new();
        let host = bootstrap(BootstrapOptions {
            capture: CaptureOptions::Injected(adapter.clone()),
            root: roots.path(),
            mode: BootstrapMode::Serve,
            store: store.clone(),
        })
        .await
        .unwrap();
        let library = host.library_id();
        let serve = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let config = write_config(&roots, library);
        let launcher = Arc::new(InProcessLauncher {
            store: store.clone(),
            adapter: adapter.clone(),
            calls: AtomicUsize::new(0),
        });
        let deps = CliDeps {
            store: store.clone(),
            launcher: launcher.clone(),
        };
        // One saved capture with the documented JSON shape.
        let (exit, out, err) =
            shot(&deps, &config, &["capture", "--mode", "region", "--json"]).await;
        assert_eq!(exit, EXIT_OK);
        assert!(err.is_empty());
        let value = single_json(&out);
        assert_eq!(value["status"], "saved");
        let receipt = &value["receipt"];
        assert_eq!(receipt["canonical_mime"], "image/webp");
        assert_eq!(receipt["width"], 32);
        assert_eq!(receipt["height"], 24);
        assert!(receipt["relative_path"]
            .as_str()
            .unwrap()
            .starts_with("assets/"));
        assert!(receipt["canonical_sha256"]
            .as_str()
            .unwrap()
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        // `all` is the documented alias of the desktop mode.
        let (exit, out, _) = shot(&deps, &config, &["capture", "--mode", "all", "--json"]).await;
        assert_eq!(exit, EXIT_OK);
        assert_eq!(single_json(&out)["status"], "saved");
        // An explicit destination outside the library receives the canonical bytes.
        // The destination must be outside the owned library root.
        let outside = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&outside).unwrap();
        let destination = outside.join(format!("sm-cli-copy-{}.webp", Uuid::new_v4()));
        let _ = fs::remove_file(&destination);
        let (exit, out, err) = shot(
            &deps,
            &config,
            &[
                "capture",
                "--mode",
                "region",
                "--json",
                "--output",
                &destination.to_string_lossy(),
            ],
        )
        .await;
        assert_eq!(
            exit,
            EXIT_OK,
            "stdout: {} stderr: {}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&err)
        );
        let value = single_json(&out);
        let bytes = fs::read(&destination).expect("the explicit copy exists");
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        assert_eq!(bytes.len().to_string(), value["receipt"]["byte_size"]);
        // The same destination is never overwritten by a second capture.
        let (exit, _, _) = shot(
            &deps,
            &config,
            &[
                "capture",
                "--mode",
                "region",
                "--json",
                "--output",
                &destination.to_string_lossy(),
            ],
        )
        .await;
        assert_eq!(exit, EXIT_USAGE);
        // A destination inside the owned library root is refused before any capture.
        let inside = roots.path().join("copy-inside.webp");
        let (exit, _, _) = shot(
            &deps,
            &config,
            &[
                "capture",
                "--mode",
                "region",
                "--json",
                "--output",
                &inside.to_string_lossy(),
            ],
        )
        .await;
        assert_eq!(exit, EXIT_USAGE);
        assert!(!inside.exists());
        let calls_before_traversal = adapter.calls.load(Ordering::SeqCst);
        // A `..` traversal through a real sibling directory is not a lexical prefix of the
        // root, yet it resolves inside the library: it must be refused too, and refused
        // before any capture happens.
        let sibling = roots
            .path()
            .parent()
            .expect("the owned root has a parent")
            .join(format!("sm-cli-sibling-{}", Uuid::new_v4()));
        fs::create_dir_all(&sibling).expect("owned sibling directory");
        let traversal = sibling
            .join("..")
            .join(roots.path().file_name().expect("root name"))
            .join("copy-traversal.webp");
        let (exit, _, _) = shot(
            &deps,
            &config,
            &[
                "capture",
                "--mode",
                "region",
                "--json",
                "--output",
                &traversal.to_string_lossy(),
            ],
        )
        .await;
        assert_eq!(exit, EXIT_USAGE);
        assert!(!roots.path().join("copy-traversal.webp").exists());
        // Another spelling of the very same directory (here upper-cased) resolves into the
        // library on this volume and is refused by the physical check as well.
        let respelled = PathBuf::from(roots.path().to_string_lossy().to_uppercase())
            .join("copy-respelled.webp");
        let (exit, _, _) = shot(
            &deps,
            &config,
            &[
                "capture",
                "--mode",
                "region",
                "--json",
                "--output",
                &respelled.to_string_lossy(),
            ],
        )
        .await;
        assert_eq!(exit, EXIT_USAGE);
        assert!(!roots.path().join("copy-respelled.webp").exists());
        assert_eq!(
            adapter.calls.load(Ordering::SeqCst),
            calls_before_traversal,
            "a refused destination is refused before the host is asked to capture"
        );
        fs::remove_dir_all(&sibling).expect("owned sibling directory removed");
        // A normal user cancellation is a successful command with its own status.
        adapter.cancel_next();
        let (exit, out, err) =
            shot(&deps, &config, &["capture", "--mode", "region", "--json"]).await;
        assert_eq!(exit, EXIT_OK);
        assert!(err.is_empty());
        let value = single_json(&out);
        assert_eq!(value["status"], "cancelled");
        assert_eq!(value["reason"], "escape");
        assert!(value["record_id"].is_null());
        // Usage errors keep their own exit code and never start a host.
        let (exit, out, _) = shot(&deps, &config, &["capture", "--json"]).await;
        assert_eq!(exit, EXIT_USAGE);
        assert!(single_json(&out)["code"] == "INVALID_REQUEST");
        let (exit, _, _) = shot(&deps, &config, &["capture", "--mode", "everything"]).await;
        assert_eq!(exit, EXIT_USAGE);
        let (exit, _, _) = shot(
            &deps,
            &config,
            &[
                "capture",
                "--mode",
                "region",
                "--operation-id",
                "not-a-uuid",
                "--json",
            ],
        )
        .await;
        assert_eq!(exit, EXIT_USAGE);
        // The whole session ran against the already running host; no autostart happened.
        assert_eq!(launcher.calls.load(Ordering::SeqCst), 0);
        // Stop the composed host and confirm the writer closed.
        let (exit, out, _) = shot(&deps, &config, &["host", "stop", "--json"]).await;
        assert_eq!(exit, EXIT_OK);
        assert_eq!(single_json(&out)["state"], "stopping");
        let outcome = serve.await.unwrap().unwrap();
        assert!(outcome.storage_closed);
    });
}
