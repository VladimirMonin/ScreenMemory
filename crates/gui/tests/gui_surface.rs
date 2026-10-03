#![cfg(windows)]
//! Window surface against real and synthetic hosts: argument/config refusal, bounded
//! no-start, real status/doctor through the shared client, CLI parity, protocol/identity
//! mismatch, validated report opening and the headless self-check contract.
use sm_application::credentials::{
    Authenticator, CredentialEnvironment, CredentialKey, Secret, SecretStore, StoreError,
};
use sm_cli::{
    run, CliDeps, HostLauncher, LaunchError, LaunchedHost, EXIT_HOST, EXIT_OK, EXIT_POLICY,
    EXIT_USAGE,
};
use sm_domain::{HostInstanceId, LibraryId};
use sm_framing::{BufferBudget, Phase, Reader, Writer};
use sm_gui::{
    load_window_config, run_self_check, runtime_available, state_for_failure, state_for_status,
    GuiCaptureAnswer, GuiCaptureState, GuiSession, GuiState, OpenReportError, ReportOpener,
    CAPTURE_MODES, NOT_IMPLEMENTED,
};
use sm_host::{bootstrap, BootstrapMode, BootstrapOptions, DEFAULT_DRAIN};
use sm_isolation::{LibraryRootResolver, RootSource};
use sm_pipe::{Endpoint, Listener};
use sm_protocol::{
    application::{BridgeState, HostStatus, APPLICATION_VERSION},
    errors::ErrorCode,
    values::BuildId,
};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Roots(PathBuf);
impl Roots {
    fn new(_label: &str) -> Self {
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

/// Counts launch attempts and fails to spawn: proves that no-start starts nothing and
/// that an authorized start uses exactly the configured absolute image and root.
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

type ServeHandle = tokio::task::JoinHandle<Result<sm_host::ShutdownOutcome, sm_host::HostError>>;

/// Test double for the fixed executable: runs the same in-process composition as
/// `shotd`, so the shared discovery/retry path of the window is really exercised.
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
impl InProcessLauncher {
    fn new(store: Arc<MemoryStore>) -> Self {
        Self {
            store,
            calls: AtomicUsize::new(0),
            arguments: Mutex::new(Vec::new()),
            stop: Arc::new(Mutex::new(None)),
            serve: Arc::new(Mutex::new(None)),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn launched(&self) -> Vec<(PathBuf, PathBuf)> {
        self.arguments.lock().unwrap().clone()
    }
    async fn shutdown(&self) {
        if let Some(token) = self.stop.lock().unwrap().clone() {
            token.cancel();
        }
        let serve = self.serve.lock().unwrap().take();
        if let Some(handle) = serve {
            let _ = tokio::time::timeout(Duration::from_secs(8), handle).await;
        }
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

#[derive(Default)]
struct RecordingOpener {
    opened: Mutex<Vec<PathBuf>>,
    refuse: bool,
}
impl ReportOpener for RecordingOpener {
    fn open(&self, directory: &Path) -> Result<(), OpenReportError> {
        self.opened.lock().unwrap().push(directory.to_path_buf());
        if self.refuse {
            return Err(OpenReportError::Shell);
        }
        Ok(())
    }
}

/// A synthetic same-image pipe server that answers the handshake with a forged welcome.
/// A real version/capability/identity mismatch cannot be produced by the real host, so
/// the client-side refusal of a hostile or older server is proven here.
struct ForgedHost {
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    connections: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}
impl ForgedHost {
    async fn start(root: &Path, welcome: serde_json::Value) -> Self {
        let layout = LibraryRootResolver::resolve(RootSource::Test {
            root,
            forbidden_donor_roots: &[],
        })
        .unwrap();
        let stop = CancellationToken::new();
        let token = stop.clone();
        // Bind synchronously before exposing the fixture: no scheduler/sleep race. A
        // previous forged host releases its instances only when its accepted connection
        // handles are closed, so the bind is retried within an explicit bounded window
        // instead of depending on a sleep; the endpoint identity is re-derived per attempt.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut listener = loop {
            match Listener::bind(Endpoint::for_layout(&layout).unwrap()) {
                Ok(listener) => break listener,
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("forged host cannot bind its owned endpoint: {error:?}"),
            }
        };
        let connections = Arc::new(Mutex::new(Vec::new()));
        let slots = connections.clone();
        let task = tokio::spawn(async move {
            loop {
                let connection = tokio::select! {
                    _ = token.cancelled() => break,
                    accepted = listener.accept() => match accepted {
                        Ok(connection) => connection,
                        Err(_) => break,
                    },
                };
                let welcome = welcome.clone();
                let handle = tokio::spawn(async move {
                    let (read, write) = tokio::io::split(connection);
                    let mut reader = Reader::new(read, BufferBudget::new());
                    if reader.receive(Phase::Handshake).await.is_err() {
                        return;
                    }
                    let writer = Writer::new(write, BufferBudget::new());
                    let _ = writer.send(Phase::Handshake, &welcome).await;
                    tokio::time::sleep(Duration::from_millis(250)).await;
                });
                slots.lock().unwrap().push(handle);
            }
        });
        Self {
            stop,
            task,
            connections,
        }
    }
    /// Cancel, then close every server-side handle of this fixture before returning, so a
    /// following forged host provably binds the same owned endpoint without a stale name.
    async fn shutdown(self) {
        self.stop.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.task).await;
        let handles = std::mem::take(&mut *self.connections.lock().unwrap());
        for handle in handles {
            handle.abort();
        }
    }
}

fn welcome(version: (u32, u32), library: LibraryId, capabilities: Vec<&str>) -> serde_json::Value {
    serde_json::json!({
        "version": {"major": version.0, "minor": version.1},
        "library_id": library.as_uuid().to_string(),
        "host_instance_id": HostInstanceId::new(Uuid::new_v4()).as_uuid().to_string(),
        "capabilities": capabilities,
    })
}

fn all_capabilities() -> Vec<&'static str> {
    vec!["system.status", "system.doctor", "system.request_shutdown"]
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn write_config_at(
    path: &Path,
    library: LibraryId,
    environment: &str,
    root: &Path,
    executable: &Path,
) -> PathBuf {
    let value = serde_json::json!({
        "schema_version": 1,
        "environment": environment,
        "root": root.to_string_lossy(),
        "library_id": library.as_uuid().to_string(),
        "host_executable": executable.to_string_lossy(),
    });
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    path.to_path_buf()
}

fn write_config(
    roots: &Roots,
    library: LibraryId,
    environment: &str,
    executable: &Path,
) -> PathBuf {
    let root = roots.path();
    write_config_at(
        &root.join("preview.json"),
        library,
        environment,
        &root,
        executable,
    )
}

fn config_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn single_json(stdout: &[u8]) -> serde_json::Value {
    let text = String::from_utf8(stdout.to_vec()).unwrap();
    let lines = text.lines().filter(|line| !line.is_empty()).count();
    assert_eq!(lines, 1, "stdout must carry exactly one JSON value: {text}");
    serde_json::from_str(text.trim_end()).unwrap()
}

fn self_check_argv(config: &Path, no_start: bool) -> Vec<String> {
    let mut argv = vec![
        "--self-check".to_string(),
        "--config".to_string(),
        config_arg(config),
    ];
    if no_start {
        argv.push("--no-start".to_string());
    }
    argv
}

fn status_fixture() -> HostStatus {
    HostStatus {
        host_instance_id: HostInstanceId::new(Uuid::new_v4()),
        library_id: LibraryId::new(Uuid::new_v4()),
        build_id: BuildId::parse("synthetic").unwrap(),
        protocol: APPLICATION_VERSION,
        state: sm_protocol::application::HostState::Ready,
        schema_version: 1,
        ai_enabled: false,
        bridge_state: BridgeState::Stopped,
        available_methods: Vec::new(),
    }
}

fn normalize_checks(checks: serde_json::Value) -> serde_json::Value {
    let mut sorted = checks.as_array().cloned().unwrap_or_default();
    for check in &mut sorted {
        if let Some(object) = check.as_object_mut() {
            object.remove("duration_ms");
        }
    }
    sorted.sort_by_key(|check| check["check_id"].as_str().unwrap_or("").to_string());
    serde_json::Value::Array(sorted)
}

/// One owned root whose library identity is created by the explicit first-run bootstrap,
/// exactly as the preview launcher does before it writes the trusted configuration.
fn prepared_roots(
    label: &str,
    store: &Arc<MemoryStore>,
) -> (tokio::runtime::Runtime, Roots, LibraryId) {
    let rt = runtime();
    let roots = Roots::new(label);
    let library = rt.block_on(async {
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
        library
    });
    (rt, roots, library)
}

#[test]
fn s04_interfaces_shared_policy_profile_status_and_refusal_parity() {
    use sm_domain::{ModelProfileId, OperationId, Revision};
    use sm_protocol::processing::*;
    let store = Arc::new(MemoryStore::default());
    let (rt, roots, library) = prepared_roots("s04", &store);
    let launcher = Arc::new(InProcessLauncher::new(store.clone()));
    let deps = CliDeps {
        store: store.clone(),
        launcher: launcher.clone(),
    };
    let config = write_config(&roots, library, "test", &std::env::current_exe().unwrap());
    rt.block_on(async {
        let mut gui = GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        assert_eq!(gui.connect().await.state, GuiState::Ready);
        let target = sm_cli::resolve_target(&load_window_config(&config).unwrap()).unwrap();
        let backend_key = CredentialKey::runtime_backend(CredentialEnvironment::Test, library);
        let refused = sm_session::ApplicationClient::connect(
            &target.endpoint,
            &target.image,
            library,
            None,
            &sm_session::CredentialSource::Store {
                store: store.clone(),
                key: backend_key,
            },
            BufferBudget::new(),
        )
        .await;
        assert!(
            matches!(refused, Err(sm_session::SessionError::PermissionDenied)),
            "backend profile never authenticates application session"
        );
        let ProcessingReply::Overview(initial) = gui
            .processing(ProcessingRequest::Overview { before: None })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(initial.policy.value.mode, ProcessingMode::Off);
        assert!(
            !initial.runtime_configured
                && !initial.model_autostart
                && !initial.physical_unload_supported
        );
        assert!(initial.jobs.is_empty());
        for (mode, background, query) in [
            ("manual", "forbid", "ask"),
            ("automatic", "allow", "forbid"),
            ("off", "forbid", "allow"),
        ] {
            let argv = [
                "--config",
                config_arg(&config).as_str(),
                "processing",
                "policy",
                "--mode",
                mode,
                "--background",
                background,
                "--query",
                query,
            ]
            .map(str::to_owned)
            .to_vec();
            let mut out = Vec::new();
            let mut err = Vec::new();
            assert_eq!(run(&argv, &deps, &mut out, &mut err).await, 0);
            let cli = single_json(&out);
            let gui_value =
                serde_json::to_value(gui.processing(ProcessingRequest::Policy {}).await.unwrap())
                    .unwrap();
            assert_eq!(cli, gui_value);
        }
        let ProcessingReply::Policy(old) =
            gui.processing(ProcessingRequest::Policy {}).await.unwrap()
        else {
            panic!()
        };
        let mut unsupported = old.value.clone();
        unsupported.retention = HostRetention::KeepLoaded;
        assert_eq!(
            gui.processing(ProcessingRequest::UpdatePolicy {
                operation_id: OperationId::new(Uuid::new_v4()),
                expected_revision: old.revision,
                value: unsupported
            })
            .await
            .unwrap_err(),
            "MODEL_CONFIG_UNSUPPORTED"
        );
        let profile = ModelProfile {
            id: ModelProfileId::new(Uuid::new_v4()),
            model_key: "synthetic/vision.gguf".into(),
            context_length: 4096,
            eval_batch_size: 256,
            flash_attention: true,
            offload_kv_cache_to_gpu: true,
            k_cache: CacheType::F16,
            v_cache: CacheType::Q8,
            native_ttl_seconds: Some(900),
            max_generation_tokens: 4000,
            temperature: 0.2,
            reasoning: ReasoningMode::Off,
        };
        for reasoning in [ReasoningMode::Off, ReasoningMode::Budget100] {
            let mut refused = profile.clone();
            refused.reasoning = reasoning;
            assert_eq!(
                gui.processing(ProcessingRequest::SaveProfile {
                    operation_id: OperationId::new(Uuid::new_v4()),
                    expected_revision: Revision::new(0),
                    value: refused
                })
                .await
                .unwrap_err(),
                "MODEL_CONFIG_UNSUPPORTED"
            );
        }
        let mut supported = profile;
        supported.reasoning = ReasoningMode::BackendDefault;
        let json = serde_json::to_string(&supported).unwrap();
        let argv = vec![
            "--config".into(),
            config_arg(&config),
            "processing".into(),
            "save-profile".into(),
            "--revision".into(),
            "0".into(),
            "--profile-json".into(),
            json,
        ];
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(run(&argv, &deps, &mut out, &mut err).await, 0);
        assert_eq!(
            single_json(&out),
            serde_json::to_value(
                gui.processing(ProcessingRequest::Profile { id: supported.id })
                    .await
                    .unwrap()
            )
            .unwrap()
        );
        assert_eq!(
            gui.processing(ProcessingRequest::Inventory {})
                .await
                .unwrap_err(),
            "RUNTIME_UNAVAILABLE"
        );
        assert!(matches!(
            gui.status(false).await.unwrap().bridge_state,
            BridgeState::Stopped
        ));
        let host = gui.status(false).await.unwrap().host_instance_id;
        drop(gui);
        let mut reopened = GuiSession::open(load_window_config(&config).unwrap(), deps).unwrap();
        assert_eq!(reopened.status(false).await.unwrap().host_instance_id, host);
        let ProcessingReply::Overview(after) = reopened
            .processing(ProcessingRequest::Overview { before: None })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(after.profiles.len(), 1);
        launcher.shutdown().await;
    });
}

#[test]
fn t06d_gui_01_arguments_and_config_refusals_are_stable() {
    let store = Arc::new(MemoryStore::default());
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new("config");
        let library = LibraryId::new(Uuid::new_v4());
        let exe = std::env::current_exe().unwrap();
        let deps = CliDeps {
            store: store.clone(),
            launcher: Arc::new(RecordingLauncher::default()),
        };
        let config = write_config(&roots, library, "test", &exe);
        // A valid explicit configuration resolves the same endpoint/image as the CLI.
        assert!(GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).is_ok());

        for (label, environment, uuid) in [
            (
                "foreign-environment",
                "product",
                library.as_uuid().to_string(),
            ),
            ("dev-environment", "dev", library.as_uuid().to_string()),
            ("non-canonical-uuid", "test", "not-a-uuid".to_string()),
            ("empty-uuid", "test", String::new()),
        ] {
            let path = roots.path().join(format!("case-{label}.json"));
            write_config_at(&path, library, environment, &roots.path(), &exe);
            let text = String::from_utf8(fs::read(&path).unwrap())
                .unwrap()
                .replace(&library.as_uuid().to_string(), &uuid);
            fs::write(&path, text).unwrap();
            let mut out = Vec::new();
            let mut err = Vec::new();
            let exit =
                run_self_check(&self_check_argv(&path, false), &deps, &mut out, &mut err).await;
            assert_eq!(exit, EXIT_USAGE, "case {label}");
            let value = single_json(&out);
            assert_eq!(value["error"]["code"], "INVALID_REQUEST", "case {label}");
            assert_eq!(
                value["diagnostics"],
                serde_json::Value::Null,
                "case {label}"
            );
            assert!(load_window_config(&path).is_err(), "case {label}");
        }

        // Unknown fields, an absent file and a non-absolute/foreign root are refused too.
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
        let missing = roots.path().join("absent.json");
        let relative_root = roots.path().join("relative-root.json");
        write_config_at(&relative_root, library, "test", Path::new("owned"), &exe);
        let unc_root = roots.path().join("unc-root.json");
        write_config_at(
            &unc_root,
            library,
            "test",
            Path::new(r"\\server\share\owned"),
            &exe,
        );
        for path in [unknown, missing, relative_root, unc_root] {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let exit =
                run_self_check(&self_check_argv(&path, false), &deps, &mut out, &mut err).await;
            assert_eq!(exit, EXIT_USAGE, "path {}", path.display());
            assert_eq!(single_json(&out)["error"]["code"], "INVALID_REQUEST");
        }
        // A relative --config argument never reaches the file system.
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            run_self_check(
                &[
                    "--self-check".to_string(),
                    "--config".to_string(),
                    "preview.json".to_string(),
                ],
                &deps,
                &mut out,
                &mut err
            )
            .await,
            EXIT_USAGE
        );
        // A configured image that is not a trusted local file is refused before any spawn.
        let absent_image = roots.path().join("absent-shotd.exe");
        let image_config = write_config(&roots, library, "test", &absent_image);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run_self_check(
            &self_check_argv(&image_config, false),
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_HOST);
        assert_eq!(single_json(&out)["error"]["code"], "RUNTIME_UNAVAILABLE");
    });
}

#[test]
fn t06d_gui_02_no_start_is_bounded_and_never_launches_a_host() {
    let store = Arc::new(MemoryStore::default());
    let (rt, roots, library) = prepared_roots("nostart", &store);
    let exe = std::env::current_exe().unwrap();
    let config = write_config(&roots, library, "test", &exe);
    let lock_before = fs::read(roots.path().join("data/host.lock")).unwrap();
    let cache_before: Vec<_> = fs::read_dir(roots.path().join("cache"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    let launcher = Arc::new(RecordingLauncher::default());
    let deps = CliDeps {
        store: store.clone(),
        launcher: launcher.clone(),
    };
    rt.block_on(async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run_self_check(&self_check_argv(&config, true), &deps, &mut out, &mut err).await;
        assert_eq!(exit, EXIT_HOST);
        let value = single_json(&out);
        assert_eq!(value["error"]["code"], "RUNTIME_UNAVAILABLE");
        assert_eq!(value["diagnostics"]["state"], "host_not_running");
        assert_eq!(value["diagnostics"]["safe_code"], "RUNTIME_UNAVAILABLE");
        assert_eq!(value["diagnostics"]["bounded_ms"], 5000);
        assert_eq!(value["diagnostics"]["report_available"], true);
        assert_eq!(
            launcher.calls.load(Ordering::SeqCst),
            0,
            "a no-start probe never launches the host image"
        );
        // Initialization already created these entries; no-start must preserve them.
        assert_eq!(
            fs::read(roots.path().join("data/host.lock")).unwrap(),
            lock_before
        );
        let cache_after: Vec<_> = fs::read_dir(roots.path().join("cache"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(cache_after, cache_before);
        // The window command without a start request behaves identically.
        let mut session =
            GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        let diagnostics = session.probe().await;
        assert_eq!(diagnostics.state, GuiState::HostNotRunning);
        assert!(diagnostics.host.is_none());
        assert_eq!(launcher.calls.load(Ordering::SeqCst), 0);
        // `--self-check` missing, or an unknown argument, stays a usage error.
        for argv in [
            vec!["--config".to_string(), config_arg(&config)],
            vec![
                "--self-check".to_string(),
                "--config".to_string(),
                config_arg(&config),
                "--bogus".to_string(),
            ],
        ] {
            let mut out = Vec::new();
            let mut err = Vec::new();
            assert_eq!(
                run_self_check(&argv, &deps, &mut out, &mut err).await,
                EXIT_USAGE
            );
            assert!(!err.is_empty());
        }
    });
}

#[test]
fn t06d_gui_03_real_host_states_and_instance_reuse_on_reopen() {
    let store = Arc::new(MemoryStore::default());
    let (rt, roots, library) = prepared_roots("states", &store);
    let launcher = Arc::new(InProcessLauncher::new(store.clone()));
    let deps = CliDeps {
        store: store.clone(),
        launcher: launcher.clone(),
    };
    let exe = std::env::current_exe().unwrap();
    let config = write_config(&roots, library, "test", &exe);
    rt.block_on(async {
        // Window opens: a no-start probe reports the absent host without starting one.
        let mut session =
            GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        assert_eq!(session.probe().await.state, GuiState::HostNotRunning);
        assert_eq!(launcher.calls(), 0);

        // «Подключиться»: exactly one start of the configured absolute image.
        let diagnostics = session.connect().await;
        assert_eq!(diagnostics.state, GuiState::Ready);
        assert_eq!(launcher.calls(), 1);
        let calls = launcher.launched();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, exe);
        assert_eq!(calls[0].1, roots.path());
        let host = diagnostics.host.as_ref().expect("status payload");
        assert_eq!(host.library_id.as_uuid(), library.as_uuid());
        assert!(matches!(
            host.state,
            sm_protocol::application::HostState::Ready
        ));
        assert_eq!(host.protocol, APPLICATION_VERSION);
        // All shipped migrations are applied; the images contract remains version 2.
        assert_eq!(host.schema_version, 6);
        assert!(!host.ai_enabled);
        assert!(matches!(host.bridge_state, BridgeState::Stopped));
        assert!(!runtime_available(host));
        assert!(diagnostics.report_available);
        assert!(diagnostics.doctor.is_none());
        let instance = host.host_instance_id;

        // «Проверить основу»: the ordinary doctor; no model load and no inventory call.
        let doctor = session.doctor().await;
        assert_eq!(doctor.state, GuiState::Ready);
        let report = doctor.doctor.as_ref().expect("doctor report");
        assert!(report
            .checks
            .iter()
            .any(|check| check.check_id.as_str() == "foundation.storage_schema"));
        assert!(report
            .checks
            .iter()
            .any(|check| check.check_id.as_str() == "runtime.lmstudio"
                && matches!(
                    check.outcome,
                    sm_protocol::application::CheckOutcome::NotRun
                )));
        assert!(!report.checks.iter().any(|check| matches!(
            check.outcome,
            sm_protocol::application::CheckOutcome::Failed
        )));
        // An unfinished capability is never reported as PASSED.
        for unfinished in ["runtime.lmstudio", "codec.webp_avif", "index.fts_vec"] {
            assert!(
                report
                    .checks
                    .iter()
                    .filter(|check| check.check_id.as_str() == unfinished)
                    .all(|check| !matches!(
                        check.outcome,
                        sm_protocol::application::CheckOutcome::Passed
                    )),
                "{unfinished} must not be PASSED"
            );
        }

        // Closing the window closes only this connection; the host keeps running and a
        // second window attaches to the *same* instance without another start.
        session.close();
        drop(session);
        assert_eq!(launcher.calls(), 1);
        let mut reopened =
            GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        let again = reopened.connect().await;
        assert_eq!(again.state, GuiState::Ready);
        assert_eq!(
            again.host.as_ref().unwrap().host_instance_id,
            instance,
            "reopening must attach to the same host instance"
        );
        assert_eq!(launcher.calls(), 1);
        launcher.shutdown().await;
        // After the host is gone the window reports an absent host, not an invented state.
        let mut after =
            GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        assert_eq!(after.probe().await.state, GuiState::HostNotRunning);
    });
}

#[test]
fn t06d_gui_04_cli_and_gui_status_doctor_parity() {
    let store = Arc::new(MemoryStore::default());
    let (rt, roots, library) = prepared_roots("parity", &store);
    let launcher = Arc::new(InProcessLauncher::new(store.clone()));
    let deps = CliDeps {
        store: store.clone(),
        launcher: launcher.clone(),
    };
    let exe = std::env::current_exe().unwrap();
    let config = write_config(&roots, library, "test", &exe);
    rt.block_on(async {
        let mut session =
            GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        let gui_status = session.connect().await;
        let gui_doctor = session.doctor().await;

        // The CLI drives the same running host through the shared client.
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &[
                "--config".to_string(),
                config_arg(&config),
                "status".to_string(),
                "--json".to_string(),
            ],
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_OK);
        let cli_status = single_json(&out);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run(
            &[
                "--config".to_string(),
                config_arg(&config),
                "doctor".to_string(),
                "--json".to_string(),
            ],
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_OK);
        let cli_doctor = single_json(&out);

        // Normalized parity: the window carries exactly the CLI status DTO and the same
        // doctor checks; ids that legitimately differ (report id, durations) are excluded.
        let gui_status_value = serde_json::to_value(gui_status.host.as_ref().unwrap()).unwrap();
        assert_eq!(gui_status_value, cli_status);
        let gui_report = gui_doctor.doctor.as_ref().unwrap();
        assert_eq!(
            serde_json::to_value(&gui_report.host).unwrap(),
            cli_doctor["host"]
        );
        assert_eq!(
            normalize_checks(serde_json::to_value(&gui_report.checks).unwrap()),
            normalize_checks(cli_doctor["checks"].clone())
        );
        assert_eq!(
            gui_report.checks.len(),
            cli_doctor["checks"].as_array().unwrap().len()
        );
        // One host, one writer, one start and no runtime call from either surface.
        assert_eq!(launcher.calls(), 1);
        launcher.shutdown().await;
    });
}

#[test]
fn t06d_gui_05_protocol_and_identity_mismatch_are_reported_without_guessing() {
    let store = Arc::new(MemoryStore::default());
    let (rt, roots, library) = prepared_roots("mismatch", &store);
    let owner = CredentialKey::owner(CredentialEnvironment::Test, library);
    Authenticator::enroll_owner(store.as_ref(), owner).unwrap();
    let deps = CliDeps {
        store: store.clone(),
        launcher: Arc::new(RecordingLauncher::default()),
    };
    let exe = std::env::current_exe().unwrap();
    let config = write_config(&roots, library, "test", &exe);
    rt.block_on(async {
        // A host that speaks an incompatible major version is refused, not guessed.
        let forged =
            ForgedHost::start(&roots.path(), welcome((2, 0), library, all_capabilities())).await;
        let mut session =
            GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        let diagnostics = session.connect().await;
        assert_eq!(diagnostics.state, GuiState::ProtocolMismatch);
        assert_eq!(diagnostics.safe_code, Some(ErrorCode::ProtocolMismatch));
        assert!(
            diagnostics.host.is_none(),
            "no guessed status is ever shown"
        );
        forged.shutdown().await;

        // A host that omits required capabilities is treated the same way, and the
        // headless check reports the same refusal with the CLI host exit code.
        let forged = ForgedHost::start(
            &roots.path(),
            welcome((1, 0), library, vec!["system.status"]),
        )
        .await;
        let mut session =
            GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        let diagnostics = session.connect().await;
        assert_eq!(diagnostics.state, GuiState::ProtocolMismatch);
        assert_eq!(diagnostics.safe_code, Some(ErrorCode::ProtocolMismatch));
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run_self_check(&self_check_argv(&config, true), &deps, &mut out, &mut err).await;
        assert_eq!(exit, EXIT_HOST);
        let value = single_json(&out);
        assert_eq!(value["error"]["code"], "PROTOCOL_MISMATCH");
        assert_eq!(value["diagnostics"]["state"], "protocol_mismatch");
        assert_eq!(value["diagnostics"]["host"], serde_json::Value::Null);
        forged.shutdown().await;

        // A host that answers with another library identity is an identity refusal; the
        // window never adopts the other library silently.
        let other = LibraryId::new(Uuid::new_v4());
        let forged =
            ForgedHost::start(&roots.path(), welcome((1, 0), other, all_capabilities())).await;
        let mut session =
            GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        let diagnostics = session.connect().await;
        assert_eq!(diagnostics.state, GuiState::RootCredentialError);
        assert_eq!(diagnostics.safe_code, Some(ErrorCode::LibraryMismatch));
        assert!(diagnostics.host.is_none());

        // With that endpoint alive, a configuration whose library has no credential is a
        // root/credential refusal with the policy exit code of the shared client.
        let absent_library = LibraryId::new(Uuid::new_v4());
        let absent_config = write_config(&roots, absent_library, "test", &exe);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run_self_check(
            &self_check_argv(&absent_config, true),
            &deps,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(exit, EXIT_POLICY);
        let value = single_json(&out);
        assert_eq!(value["error"]["code"], "CREDENTIAL_STORE_UNAVAILABLE");
        assert_eq!(value["diagnostics"]["state"], "root_credential_error");
        assert_eq!(value["diagnostics"]["retryable"], false);
        forged.shutdown().await;
    });
}

#[test]
fn t06d_gui_06_open_report_reaches_only_the_validated_directory() {
    let store = Arc::new(MemoryStore::default());
    let (rt, roots, library) = prepared_roots("report", &store);
    let deps = CliDeps {
        store: store.clone(),
        launcher: Arc::new(RecordingLauncher::default()),
    };
    let exe = std::env::current_exe().unwrap();
    let config = write_config(&roots, library, "test", &exe);
    rt.block_on(async {
        let session = GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        let opener = RecordingOpener::default();
        let outcome = session.open_report(&opener);
        assert!(outcome.opened);
        assert_eq!(outcome.safe_code, None);
        let opened = opener.opened.lock().unwrap().clone();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0], roots.path().join("cache"));
        // A refusing shell is reported as a safe code, never as an opened report.
        let refusing = RecordingOpener {
            opened: Mutex::new(Vec::new()),
            refuse: true,
        };
        let refused = session.open_report(&refusing);
        assert!(!refused.opened);
        assert_eq!(refused.safe_code, Some(ErrorCode::RuntimeUnavailable));

        // A configuration whose cache/log leaf is absent is refused before any shell call.
        let broken = Roots::new("report-missing");
        let broken_config = write_config(&broken, library, "test", &exe);
        let broken_session =
            GuiSession::open(load_window_config(&broken_config).unwrap(), deps.clone()).unwrap();
        fs::remove_dir(broken.path().join("cache")).unwrap();
        let opener = RecordingOpener::default();
        let outcome = broken_session.open_report(&opener);
        assert!(!outcome.opened);
        assert_eq!(outcome.safe_code, Some(ErrorCode::PermissionDenied));
        assert!(!broken_session.report_available());
        assert!(
            opener.opened.lock().unwrap().is_empty(),
            "nothing is opened when the validated directory is unavailable"
        );
        assert!(broken_session.report_directory().is_err());
    });
}

#[test]
fn t06d_gui_07_state_mapping_is_total_and_shows_no_invented_counts() {
    // Every protocol code maps onto one documented state.
    let states: BTreeSet<GuiState> = [
        ErrorCode::MethodNotSupported,
        ErrorCode::InvalidRequest,
        ErrorCode::FrameTooLarge,
        ErrorCode::ProtocolTruncatedFrame,
        ErrorCode::PermissionDenied,
        ErrorCode::LibraryMismatch,
        ErrorCode::ProtocolMismatch,
        ErrorCode::HostAlreadyRunning,
        ErrorCode::SchemaIncompatible,
        ErrorCode::CredentialStoreUnavailable,
        ErrorCode::RuntimeUnavailable,
        ErrorCode::ModelNotInstalled,
        ErrorCode::AiDisabled,
        ErrorCode::ModelConfigUnsupported,
        ErrorCode::BridgeUnavailable,
        ErrorCode::WaitTimeout,
    ]
    .into_iter()
    .map(|code| {
        state_for_failure(&sm_cli::ClientFailure {
            exit: 4,
            code,
            message: "safe",
            retryable: false,
        })
    })
    .collect();
    let documented: BTreeSet<GuiState> = [
        GuiState::HostNotRunning,
        GuiState::Ready,
        GuiState::RuntimeUnavailable,
        GuiState::ProtocolMismatch,
        GuiState::RootCredentialError,
    ]
    .into_iter()
    .collect();
    assert!(states.is_subset(&documented));
    assert_eq!(states.len(), documented.len() - 1, "ready is a host state");

    // A disabled AI policy is a working foundation; an enabled policy without a ready
    // bridge is the runtime-unavailable state, and it never fails the whole window.
    let mut host = status_fixture();
    host.ai_enabled = false;
    host.bridge_state = BridgeState::Stopped;
    assert_eq!(state_for_status(&host), GuiState::Ready);
    assert!(!runtime_available(&host));
    host.ai_enabled = true;
    assert_eq!(state_for_status(&host), GuiState::RuntimeUnavailable);
    host.bridge_state = BridgeState::Ready;
    assert_eq!(state_for_status(&host), GuiState::Ready);
    assert!(runtime_available(&host));

    // The envelope invents nothing: there is no count field, and every unfinished
    // capability is listed as not implemented instead of showing an empty result.
    let diagnostics = sm_gui::GuiDiagnostics::from_status(host, false);
    let value = serde_json::to_value(&diagnostics).unwrap();
    assert!(!value
        .as_object()
        .unwrap()
        .keys()
        .any(|key| key.contains("count")));
    assert_eq!(diagnostics.state, GuiState::Ready);
    assert_eq!(diagnostics.schema_version, sm_gui::GUI_ENVELOPE_VERSION);
    assert_eq!(
        BTreeSet::from_iter(NOT_IMPLEMENTED.iter().map(|name| name.to_string())),
        BTreeSet::from_iter(diagnostics.not_implemented.iter().cloned())
    );
    assert!(!diagnostics
        .not_implemented
        .iter()
        .any(|name| name == "gallery"));
    for capability in ["search", "knowledge_pipeline"] {
        assert!(diagnostics
            .not_implemented
            .iter()
            .any(|name| name == capability));
    }
}

#[test]
fn t06d_gui_08_self_check_prints_exactly_one_json_with_cli_exits() {
    let store = Arc::new(MemoryStore::default());
    let (rt, roots, library) = prepared_roots("selfcheck", &store);
    let launcher = Arc::new(InProcessLauncher::new(store.clone()));
    let deps = CliDeps {
        store: store.clone(),
        launcher: launcher.clone(),
    };
    let exe = std::env::current_exe().unwrap();
    let config = write_config(&roots, library, "test", &exe);
    rt.block_on(async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit =
            run_self_check(&self_check_argv(&config, false), &deps, &mut out, &mut err).await;
        assert_eq!(exit, EXIT_OK);
        assert!(err.is_empty(), "a successful self-check keeps stderr clean");
        let value = single_json(&out);
        assert_eq!(value["mode"], "gui-self-check");
        assert_eq!(value["exit_code"], EXIT_OK);
        assert_eq!(value["error"], serde_json::Value::Null);
        assert_eq!(value["diagnostics"]["state"], "ready");
        assert_eq!(
            value["diagnostics"]["host"]["library_id"],
            library.as_uuid().to_string()
        );
        assert_eq!(value["diagnostics"]["host"]["protocol"]["major"], 1);
        assert_eq!(value["diagnostics"]["host"]["ai_enabled"], false);
        assert_eq!(value["diagnostics"]["report_available"], true);
        assert_eq!(value["diagnostics"]["safe_code"], serde_json::Value::Null);
        assert_eq!(launcher.calls(), 1);
        // A second self-check attaches to the running host without another start.
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            run_self_check(&self_check_argv(&config, false), &deps, &mut out, &mut err).await,
            EXIT_OK
        );
        assert_eq!(launcher.calls(), 1);
        assert_eq!(
            single_json(&out)["diagnostics"]["host"]["host_instance_id"],
            value["diagnostics"]["host"]["host_instance_id"]
        );
        launcher.shutdown().await;
    });
}

#[test]
fn t06d_gui_09_autostart_uses_the_configured_fixed_image_once() {
    let store = Arc::new(MemoryStore::default());
    let (rt, roots, library) = prepared_roots("autostart", &store);
    let launcher = Arc::new(InProcessLauncher::new(store.clone()));
    let deps = CliDeps {
        store: store.clone(),
        launcher: launcher.clone(),
    };
    let exe = std::env::current_exe().unwrap();
    let config = write_config(&roots, library, "test", &exe);
    rt.block_on(async {
        let mut session =
            GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        let diagnostics = session.connect().await;
        assert_eq!(diagnostics.state, GuiState::Ready);
        assert_eq!(launcher.calls(), 1);
        // The credential came from the explicit library-scoped owner key of the config.
        let key = CredentialKey::owner(CredentialEnvironment::Test, library);
        assert!(store.read(&key).is_ok());
        let calls = launcher.launched();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, exe);
        assert_eq!(calls[0].1, roots.path());
        // An immediately repeated action reuses the connection instead of starting again.
        let second = session.doctor().await;
        assert_eq!(second.state, GuiState::Ready);
        assert_eq!(launcher.calls(), 1);
        launcher.shutdown().await;
    });
}

#[test]
fn s04_processing_revision_conflict_keeps_host_ready() {
    assert_eq!(
        state_for_failure(&sm_cli::ClientFailure {
            exit: 5,
            code: ErrorCode::RevisionConflict,
            message: "conflict",
            retryable: false
        }),
        GuiState::Ready
    );
}

#[test]
fn s02_gui_09_capture_answer_carries_identities_only_and_a_closed_mode_vocabulary() {
    // A refusal reuses the shared CLI codes and never invents a record identity.
    let failure = sm_cli::ClientFailure {
        exit: sm_cli::EXIT_CAPTURE,
        code: ErrorCode::CaptureFocusUnavailable,
        message: "refused",
        retryable: false,
    };
    let refused = GuiCaptureAnswer::refused(&failure);
    assert_eq!(refused.state, GuiCaptureState::Refused);
    assert_eq!(refused.safe_code, Some(ErrorCode::CaptureFocusUnavailable));
    assert!(refused.record_id.is_none() && refused.asset_id.is_none());
    assert!(refused.width.is_none() && refused.byte_size.is_none());
    let value = serde_json::to_value(&refused).unwrap();
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    for forbidden in ["pixel", "data", "hwnd", "bytes", "absolute"] {
        assert!(
            !keys.iter().any(|key| key.contains(forbidden)),
            "the capture answer never carries {forbidden}"
        );
    }
    // A saved answer mirrors exactly the committed receipt.
    let receipt = sm_protocol::application::CaptureReceipt {
        operation_id: sm_domain::OperationId::new(Uuid::from_u128(0x51)),
        record_id: sm_domain::RecordId::new(Uuid::from_u128(0x52)),
        asset_id: sm_domain::AssetId::new(Uuid::from_u128(0x53)),
        canonical_mime: "image/webp".to_string(),
        canonical_sha256: "ab".repeat(32),
        width: 1000,
        height: 600,
        byte_size: "48271".to_string(),
        relative_path: "assets/2026-09/01900000-0000-7000-8000-000000000052/original.webp"
            .to_string(),
        warnings: vec!["geometry_fallback_getwindowrect".to_string()],
    };
    let answer = GuiCaptureAnswer::from_outcome(&sm_protocol::application::CaptureOutcome::Saved {
        receipt: receipt.clone(),
    });
    assert_eq!(answer.state, GuiCaptureState::Saved);
    assert_eq!(answer.operation_id, Some(receipt.operation_id));
    assert_eq!(answer.record_id, Some(receipt.record_id));
    assert_eq!(answer.asset_id, Some(receipt.asset_id));
    assert_eq!(answer.width, Some(1000));
    assert_eq!(answer.byte_size, Some("48271".to_string()));
    assert_eq!(answer.relative_path, Some(receipt.relative_path.clone()));
    assert!(answer.relative_path.unwrap().starts_with("assets/"));
    assert_eq!(answer.warnings, receipt.warnings);
    // A cancellation is its own state, never an empty saved answer.
    let cancelled =
        GuiCaptureAnswer::from_outcome(&sm_protocol::application::CaptureOutcome::Cancelled {
            operation_id: receipt.operation_id,
            reason: sm_protocol::application::CancelReason::Escape,
        });
    assert_eq!(cancelled.state, GuiCaptureState::Cancelled);
    assert_eq!(
        cancelled.reason,
        Some(sm_protocol::application::CancelReason::Escape)
    );
    assert!(cancelled.record_id.is_none());
    // The mode vocabulary the window may send is closed and has no free-form alias.
    assert_eq!(CAPTURE_MODES, ["region", "window", "monitor", "desktop"]);
}
