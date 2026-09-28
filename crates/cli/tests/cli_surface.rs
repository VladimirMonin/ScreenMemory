#![cfg(windows)]
//! CLI surface against a real in-process host: argument/exit contract, trusted
//! configuration refusal, one explicit autostart, and single-JSON stdout.
use sm_application::credentials::{
    CredentialEnvironment, CredentialKey, Secret, SecretStore, StoreError,
};
use sm_cli::{
    doctor_exit_code, run, CliDeps, HostLauncher, LaunchError, LaunchedHost, EXIT_HOST, EXIT_OK,
    EXIT_POLICY, EXIT_STORAGE, EXIT_USAGE,
};
use sm_domain::{HostInstanceId, LibraryId, ReportId};
use sm_host::{bootstrap, BootstrapMode, BootstrapOptions, DEFAULT_DRAIN};
use sm_protocol::application::{
    BridgeState, CheckOutcome, CheckResult, DoctorReport, HostState, HostStatus, ProtocolVersion,
    APPLICATION_VERSION,
};
use sm_protocol::errors::ErrorCode;
use sm_protocol::values::{BuildId, CheckId, MethodName};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Roots(PathBuf);
impl Roots {
    fn new(label: &str) -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = parent.join(format!("sm-cli-{label}-{}", Uuid::new_v4()));
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

type ServeHandle = tokio::task::JoinHandle<Result<sm_host::ShutdownOutcome, sm_host::HostError>>;

#[derive(Default)]
struct RecordingLauncher {
    calls: AtomicUsize,
    arguments: Mutex<Vec<(PathBuf, PathBuf)>>,
}
impl HostLauncher for RecordingLauncher {
    fn launch(&self, executable: &Path, root: &Path) -> Result<Box<dyn LaunchedHost>, LaunchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.arguments
            .lock()
            .unwrap()
            .push((executable.to_path_buf(), root.to_path_buf()));
        Err(LaunchError::SpawnFailed)
    }
}

/// Test double for the fixed executable: starts the same in-process composition that
/// `shotd` runs, so the CLI's real discovery and retry logic is exercised.
struct InProcessLauncher {
    store: Arc<MemoryStore>,
    calls: AtomicUsize,
    arguments: Mutex<Vec<(PathBuf, PathBuf)>>,
    stop: Arc<Mutex<Option<CancellationToken>>>,
    serve: Arc<Mutex<Option<ServeHandle>>>,
}
impl HostLauncher for InProcessLauncher {
    fn launch(&self, executable: &Path, root: &Path) -> Result<Box<dyn LaunchedHost>, LaunchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.arguments
            .lock()
            .unwrap()
            .push((executable.to_path_buf(), root.to_path_buf()));
        let store = self.store.clone();
        let root = root.to_path_buf();
        let stop_slot = self.stop.clone();
        let serve_slot = self.serve.clone();
        let handle = tokio::spawn(async move {
            let host = bootstrap(BootstrapOptions {
                capture: sm_host::CaptureOptions::Disabled,
                root,
                mode: BootstrapMode::Serve,
                store,
            })
            .await?;
            let stop = host.stop_token();
            stop_slot.lock().unwrap().replace(stop.clone());
            host.serve(DEFAULT_DRAIN).await
        });
        serve_slot.lock().unwrap().replace(handle);
        Ok(Box::new(InProcessChild {
            stop: self.stop.clone(),
            serve: self.serve.clone(),
            established: false,
        }))
    }
}
struct InProcessChild {
    stop: Arc<Mutex<Option<CancellationToken>>>,
    serve: Arc<Mutex<Option<ServeHandle>>>,
    established: bool,
}
impl LaunchedHost for InProcessChild {
    fn has_exited(&mut self) -> bool {
        self.serve
            .lock()
            .unwrap()
            .as_ref()
            .map(|handle| handle.is_finished())
            .unwrap_or(true)
    }
    fn mark_established(&mut self) {
        self.established = true;
    }
    fn terminate_owned(&mut self) {
        if let Some(token) = self.stop.lock().unwrap().clone() {
            token.cancel();
        }
    }
}
impl Drop for InProcessChild {
    fn drop(&mut self) {
        if !self.established {
            self.terminate_owned();
        }
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn write_config(
    roots: &Roots,
    library: LibraryId,
    environment: &str,
    executable: &Path,
) -> PathBuf {
    let path = roots.path().join("preview.json");
    let value = serde_json::json!({
        "schema_version": 1,
        "environment": environment,
        "root": roots.path().to_string_lossy(),
        "library_id": library.as_uuid().to_string(),
        "host_executable": executable.to_string_lossy(),
    });
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    path
}

fn config_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn arguments(config: &Path, rest: &[&str]) -> Vec<String> {
    let mut argv = vec!["--config".to_string(), config_arg(config)];
    argv.extend(rest.iter().map(|value| (*value).to_string()));
    argv
}

fn single_json(stdout: &[u8]) -> serde_json::Value {
    let text = String::from_utf8(stdout.to_vec()).unwrap();
    let lines = text.lines().filter(|line| !line.is_empty()).count();
    assert_eq!(lines, 1, "stdout must carry exactly one JSON value: {text}");
    serde_json::from_str(text.trim_end()).unwrap()
}

fn status_of(library: LibraryId, host: HostInstanceId) -> HostStatus {
    HostStatus {
        host_instance_id: host,
        library_id: library,
        build_id: BuildId::parse("synthetic").unwrap(),
        protocol: APPLICATION_VERSION,
        state: HostState::Ready,
        schema_version: 1,
        ai_enabled: false,
        bridge_state: BridgeState::Stopped,
        available_methods: ["system.status"]
            .iter()
            .map(|name| MethodName::parse(*name).unwrap())
            .collect(),
    }
}

fn check(id: &str, outcome: CheckOutcome, code: Option<ErrorCode>) -> CheckResult {
    CheckResult {
        check_id: CheckId::parse(id).unwrap(),
        outcome,
        safe_code: code,
        duration_ms: Some(1),
        evidence_id: None,
    }
}

#[test]
fn s01_cli_01_version_usage_and_config_refusal() {
    let rt = runtime();
    rt.block_on(async {
        let store = Arc::new(MemoryStore::default());
        let deps = CliDeps {
            store,
            launcher: Arc::new(RecordingLauncher::default()),
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(&["--version".to_string()], &deps, &mut out, &mut err).await;
        assert_eq!(exit, EXIT_OK);
        assert_eq!(
            String::from_utf8(out.clone()).unwrap(),
            format!("shot {} (protocol 1.0)\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(err.is_empty());

        for argv in [
            vec![],
            vec!["status".to_string()],
            vec![
                "--config".to_string(),
                "relative.json".to_string(),
                "status".to_string(),
            ],
            vec!["status".to_string(), "--no-start".to_string()],
            vec!["doctor".to_string(), "--no-start".to_string()],
            vec!["--bogus".to_string()],
            vec!["host".to_string()],
            vec!["host".to_string(), "status".to_string()],
        ] {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let exit = run(&argv, &deps, &mut out, &mut err).await;
            assert_eq!(exit, EXIT_USAGE, "argv {argv:?}");
            assert!(
                out.is_empty(),
                "usage errors keep stdout clean for {argv:?}"
            );
            assert!(!err.is_empty(), "usage errors are diagnosed on stderr");
        }

        // A requested JSON error is one machine-readable object on stdout.
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &["--bogus".to_string(), "--json".to_string()],
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_USAGE);
        let value = single_json(&out);
        assert_eq!(value["code"], "INVALID_REQUEST");

        // Trusted configuration is validated before any host contact.
        let roots = Roots::new("config");
        let library = LibraryId::new(Uuid::new_v4());
        let exe = std::env::current_exe().unwrap();
        for (label, environment, uuid) in [
            (
                "foreign-environment",
                "product",
                library.as_uuid().to_string(),
            ),
            ("dev-environment", "dev", library.as_uuid().to_string()),
            ("bad-uuid", "test", "not-a-uuid".to_string()),
        ] {
            let path = write_config(&roots, library, environment, &exe);
            let mut text = String::from_utf8(fs::read(&path).unwrap()).unwrap();
            text = text.replace(&library.as_uuid().to_string(), &uuid);
            fs::write(&path, text).unwrap();
            let mut out = Vec::new();
            let mut err = Vec::new();
            let exit = run(
                &arguments(&path, &["status", "--no-start", "--json"]),
                &deps,
                &mut out,
                &mut err,
            )
            .await;
            assert_eq!(exit, EXIT_USAGE, "case {label}");
            assert_eq!(single_json(&out)["code"], "INVALID_REQUEST", "case {label}");
        }
        let unknown = roots.path().join("unknown.json");
        fs::write(
            &unknown,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "environment": "test",
                "root": roots.path().to_string_lossy(),
                "library_id": library.as_uuid().to_string(),
                "host_executable": exe.to_string_lossy(),
                "extra": true,
            }))
            .unwrap(),
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            run(
                &arguments(&unknown, &["status", "--no-start", "--json"]),
                &deps,
                &mut out,
                &mut err
            )
            .await,
            EXIT_USAGE
        );
        let missing = roots.path().join("absent.json");
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            run(
                &arguments(&missing, &["status", "--no-start", "--json"]),
                &deps,
                &mut out,
                &mut err
            )
            .await,
            EXIT_USAGE
        );
        // A relative configuration path is refused before reading anything.
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            run(
                &[
                    "--config".to_string(),
                    "preview.json".to_string(),
                    "status".to_string(),
                    "--json".to_string(),
                ],
                &deps,
                &mut out,
                &mut err
            )
            .await,
            EXIT_USAGE
        );
    });
}

#[test]
fn s01_cli_02_no_start_never_starts_or_creates_a_host() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new("nostart");
        let library = LibraryId::new(Uuid::new_v4());
        let exe = std::env::current_exe().unwrap();
        let config = write_config(&roots, library, "test", &exe);
        let launcher = Arc::new(RecordingLauncher::default());
        let deps = CliDeps {
            store: Arc::new(MemoryStore::default()),
            launcher: launcher.clone(),
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &arguments(&config, &["status", "--no-start", "--json"]),
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_HOST);
        let value = single_json(&out);
        assert_eq!(value["code"], "RUNTIME_UNAVAILABLE");
        assert!(
            err.is_empty(),
            "json mode keeps stderr free of duplicate text"
        );
        assert_eq!(launcher.calls.load(Ordering::SeqCst), 0);
        // No host bootstrap side effect happened in the configured owned root.
        assert!(!roots.path().join("data").join("host.lock").exists());
        assert!(!roots.path().join("data").join("library.sqlite3").exists());
        assert!(fs::read_dir(roots.path().join("cache"))
            .unwrap()
            .next()
            .is_none());

        // `host stop` never starts a host either.
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &arguments(&config, &["host", "stop", "--json"]),
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_HOST);
        assert_eq!(single_json(&out)["code"], "RUNTIME_UNAVAILABLE");
        assert_eq!(launcher.calls.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn s01_cli_03_status_doctor_and_stop_share_one_real_host_identity() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new("serve");
        let store = Arc::new(MemoryStore::default());
        let host = bootstrap(BootstrapOptions {
            capture: sm_host::CaptureOptions::Disabled,
            root: roots.path(),
            mode: BootstrapMode::Serve,
            store: store.clone(),
        })
        .await
        .unwrap();
        let library = host.library_id();
        let host_instance = host.host_instance_id();
        let serve = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let exe = std::env::current_exe().unwrap();
        let config = write_config(&roots, library, "test", &exe);
        let launcher = Arc::new(RecordingLauncher::default());
        let deps = CliDeps {
            store: store.clone(),
            launcher: launcher.clone(),
        };

        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &arguments(&config, &["status", "--json"]),
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_OK);
        assert!(err.is_empty());
        let status = single_json(&out);
        assert_eq!(status["library_id"], library.as_uuid().to_string());
        assert_eq!(
            status["host_instance_id"],
            host_instance.as_uuid().to_string()
        );
        assert_eq!(status["state"], "ready");
        assert_eq!(status["ai_enabled"], false);
        assert_eq!(status["bridge_state"], "stopped");
        assert_eq!(
            status["schema_version"], 2,
            "the real host reports the fully migrated library schema"
        );

        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &arguments(&config, &["doctor", "--json"]),
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_OK);
        assert!(err.is_empty());
        let doctor = single_json(&out);
        assert_eq!(doctor["host"]["library_id"], library.as_uuid().to_string());
        assert_eq!(
            doctor["host"]["host_instance_id"],
            host_instance.as_uuid().to_string()
        );
        // CLI status and doctor report the same host/library identity as the real host.
        let checks = doctor["checks"].as_array().unwrap();
        assert!(checks
            .iter()
            .any(|check| check["check_id"] == "foundation.storage_schema"
                && check["outcome"] == "PASSED"));
        assert!(checks
            .iter()
            .any(|check| check["check_id"] == "runtime.lmstudio" && check["outcome"] == "NOT_RUN"));
        assert!(!checks.iter().any(|check| check["outcome"] == "FAILED"));
        // No autostart was needed while the real host answered.
        assert_eq!(launcher.calls.load(Ordering::SeqCst), 0);

        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &arguments(&config, &["host", "stop", "--json"]),
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_OK);
        let receipt = single_json(&out);
        assert_eq!(
            receipt["host_instance_id"],
            host_instance.as_uuid().to_string()
        );
        assert_eq!(receipt["state"], "stopping");
        let outcome = tokio::time::timeout(Duration::from_secs(8), serve)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(outcome.storage_closed);

        // The endpoint is gone: a no-start command now reports an absent host.
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &arguments(&config, &["status", "--no-start", "--json"]),
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_HOST);
        assert_eq!(single_json(&out)["code"], "RUNTIME_UNAVAILABLE");
    });
}

#[test]
fn s01_cli_04_autostart_uses_the_configured_image_once() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new("autostart");
        let store = Arc::new(MemoryStore::default());
        // The library identity of this owned root is created by an explicit first-run
        // bootstrap, exactly as the preview launcher does before writing the config.
        let prepared = bootstrap(BootstrapOptions {
            capture: sm_host::CaptureOptions::Disabled,
            root: roots.path(),
            mode: BootstrapMode::Initialize,
            store: store.clone(),
        })
        .await
        .unwrap();
        let library = prepared.library_id();
        assert!(prepared.shutdown().await.storage_closed);
        let exe = std::env::current_exe().unwrap();
        let config = write_config(&roots, library, "test", &exe);
        let launcher = Arc::new(InProcessLauncher {
            store: store.clone(),
            calls: AtomicUsize::new(0),
            arguments: Mutex::new(Vec::new()),
            stop: Arc::new(Mutex::new(None)),
            serve: Arc::new(Mutex::new(None)),
        });
        let deps = CliDeps {
            store: store.clone(),
            launcher: launcher.clone(),
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &arguments(&config, &["status", "--json"]),
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_OK);
        let status = single_json(&out);
        assert_eq!(status["library_id"], library.as_uuid().to_string());
        assert_eq!(launcher.calls.load(Ordering::SeqCst), 1);
        let calls = launcher.arguments.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        // The launcher receives exactly the configured absolute image and root.
        assert_eq!(calls[0].0, exe);
        assert_eq!(calls[0].1, roots.path());
        // The client credential came from the explicit library-scoped key of the config.
        let key = CredentialKey::owner(CredentialEnvironment::Test, library);
        assert!(store.read(&key).is_ok());
        if let Some(token) = launcher.stop.lock().unwrap().clone() {
            token.cancel();
        }
        let serve = launcher.serve.lock().unwrap().take();
        if let Some(handle) = serve {
            let _ = tokio::time::timeout(Duration::from_secs(8), handle).await;
        }
    });
}

#[test]
fn s01_cli_05_doctor_exit_mapping_is_strict() {
    let library = LibraryId::new(Uuid::new_v4());
    let host = HostInstanceId::new(Uuid::new_v4());
    let report = |checks: Vec<CheckResult>| DoctorReport {
        report_id: ReportId::new(Uuid::new_v4()),
        host: status_of(library, host),
        checks,
    };
    assert_eq!(
        doctor_exit_code(&report(vec![
            check("foundation.root_identity", CheckOutcome::Passed, None),
            check("runtime.lmstudio", CheckOutcome::NotRun, None),
            check("bridge.supervisor", CheckOutcome::Unsupported, None),
        ])),
        EXIT_OK
    );
    assert_eq!(
        doctor_exit_code(&report(vec![
            check("foundation.storage_schema", CheckOutcome::Passed, None),
            check(
                "foundation.sqlite_runtime",
                CheckOutcome::Failed,
                Some(ErrorCode::RuntimeUnavailable)
            ),
        ])),
        EXIT_STORAGE
    );
    assert_eq!(
        doctor_exit_code(&report(vec![check(
            "foundation.credential_store",
            CheckOutcome::Failed,
            Some(ErrorCode::CredentialStoreUnavailable)
        )])),
        EXIT_POLICY
    );
    let _ = ProtocolVersion { major: 1, minor: 0 };
}
