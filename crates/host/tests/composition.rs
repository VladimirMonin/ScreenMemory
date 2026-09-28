#![cfg(windows)]
//! In-process host composition: real root/lease/SQLite/pipe/session with synthetic
//! in-memory credentials. No native vault, no model, no network, no foreign process.
use sm_application::credentials::{
    CredentialEnvironment, CredentialKey, Secret, SecretStore, StoreError,
};
use sm_domain::LibraryId;
use sm_framing::BufferBudget;
use sm_host::{bootstrap, BootstrapMode, BootstrapOptions, HostError, DEFAULT_DRAIN};
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use sm_pipe::{Endpoint, Listener, TrustedServerImage};
use sm_protocol::application::{BridgeState, CheckOutcome, HostState};
use sm_session::{ApplicationClient, CredentialSource};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use uuid::Uuid;

struct Roots(PathBuf);
impl Roots {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = parent.join(format!("sm-host-{}", Uuid::new_v4()));
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
    fn endpoint(&self) -> Endpoint {
        Endpoint::for_layout(&self.layout()).unwrap()
    }
    fn data(&self) -> PathBuf {
        self.0.join("data")
    }
    fn cache(&self) -> PathBuf {
        self.0.join("cache")
    }
}
impl Drop for Roots {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
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

fn options(
    root: &Path,
    mode: BootstrapMode,
    store: Arc<dyn SecretStore + Send + Sync>,
) -> BootstrapOptions {
    BootstrapOptions {
        root: root.to_path_buf(),
        mode,
        store,
    }
}

fn image() -> TrustedServerImage {
    // The in-process host serves from this test process, so its image is the expectation.
    TrustedServerImage::from_configuration(&std::env::current_exe().unwrap()).unwrap()
}

async fn connect(
    roots: &Roots,
    library: LibraryId,
    host: Option<sm_domain::HostInstanceId>,
    store: &Arc<MemoryStore>,
) -> ApplicationClient {
    let key = CredentialKey::owner(CredentialEnvironment::Test, library);
    ApplicationClient::connect(
        &roots.endpoint(),
        &image(),
        library,
        host,
        &CredentialSource::Store {
            store: store.clone(),
            key,
        },
        BufferBudget::new(),
    )
    .await
    .unwrap()
}

#[test]
fn s01_host_cli_01_status_doctor_stop_and_lease_release() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let host = bootstrap(options(&roots.path(), BootstrapMode::Serve, store.clone()))
            .await
            .unwrap();
        let library = host.library_id();
        let host_instance = host.host_instance_id();
        assert_eq!(host.bootstrap_report().library_id, library);
        // The images migration (0002) is applied by the same worker, so the host
        // reports the fully migrated library schema.
        assert_eq!(host.bootstrap_report().schema_version, 2);
        assert!(fs::metadata(roots.data().join("library.sqlite3")).is_ok());
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let client = connect(&roots, library, Some(host_instance), &store).await;
        let status = client.status().await.unwrap();
        assert_eq!(status.library_id, library);
        assert_eq!(status.host_instance_id, host_instance);
        assert_eq!(status.schema_version, 2);
        assert!(!status.ai_enabled);
        assert!(matches!(status.bridge_state, BridgeState::Stopped));
        assert!(matches!(status.state, HostState::Ready));
        assert_eq!(status.available_methods.len(), 3);
        let doctor = client.doctor().await.unwrap();
        assert_eq!(doctor.host.library_id, library);
        assert_eq!(doctor.host.host_instance_id, host_instance);
        let check = |id: &str| {
            doctor
                .checks
                .iter()
                .find(|check| check.check_id.as_str() == id)
                .unwrap_or_else(|| panic!("missing check {id}"))
        };
        for id in [
            "foundation.root_identity",
            "foundation.host_lease",
            "foundation.credential_store",
            "foundation.storage_schema",
            "foundation.sqlite_runtime",
            "foundation.log_sink",
        ] {
            assert!(
                matches!(check(id).outcome, CheckOutcome::Passed),
                "mandatory check {id}"
            );
            assert!(check(id).duration_ms.is_some());
        }
        // Unfinished capabilities are honest, never a fabricated pass or a foundation failure.
        assert!(matches!(
            check("runtime.lmstudio").outcome,
            CheckOutcome::NotRun
        ));
        assert!(matches!(
            check("bridge.supervisor").outcome,
            CheckOutcome::Unsupported
        ));
        assert!(matches!(
            check("codec.webp_avif").outcome,
            CheckOutcome::NotRun
        ));
        assert!(!doctor
            .checks
            .iter()
            .any(|check| matches!(check.outcome, CheckOutcome::Failed)));
        // The receipt means the request was accepted, not that handles are closed.
        let receipt = client.request_shutdown().await.unwrap();
        assert_eq!(receipt.host_instance_id, host_instance);
        assert!(matches!(receipt.state, HostState::Stopping));
        drop(client);
        let outcome = tokio::time::timeout(Duration::from_secs(8), handle)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(outcome.storage_closed, "SQLite closure was not confirmed");
        assert!(outcome.serve_ok);
        assert!(outcome.log_flushed);
        assert_eq!(outcome.exit_code(), sm_host::EXIT_OK);
        // The exclusive lease is only released after the SQLite handles closed.
        assert!(HostLease::acquire(roots.layout()).is_ok());
        // The secure endpoint is fully released as well: the same pipe name can be
        // bound again, so no connection instance outlives the stopped host.
        assert!(Listener::bind(roots.endpoint()).is_ok());
        // The doctor probe file was removed; only owned operational logs remain.
        for entry in fs::read_dir(roots.cache()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                name.starts_with("operational-") && name.ends_with(".jsonl"),
                "unexpected cache entry {name}"
            );
        }
        // Reopen keeps the durable library identity and does not re-enroll.
        let reopened = bootstrap(options(&roots.path(), BootstrapMode::Serve, store.clone()))
            .await
            .unwrap();
        assert_eq!(reopened.library_id(), library);
        let outcome = reopened.shutdown().await;
        assert!(outcome.storage_closed);
        assert!(HostLease::acquire(roots.layout()).is_ok());
    });
}

#[test]
fn s01_host_cli_02_second_writer_is_refused_for_one_root() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let host = bootstrap(options(&roots.path(), BootstrapMode::Serve, store.clone()))
            .await
            .unwrap();
        let library = host.library_id();
        let host_instance = host.host_instance_id();
        let handle = tokio::spawn(async move { host.serve(DEFAULT_DRAIN).await });
        let second = bootstrap(options(&roots.path(), BootstrapMode::Serve, store.clone())).await;
        assert!(matches!(second, Err(HostError::LeaseBusy)));
        // The owner keeps serving the same identity after the refused second start.
        let client = connect(&roots, library, Some(host_instance), &store).await;
        assert_eq!(
            client.status().await.unwrap().host_instance_id,
            host_instance
        );
        drop(client);
        let stopper = connect(&roots, library, Some(host_instance), &store).await;
        stopper.request_shutdown().await.unwrap();
        drop(stopper);
        tokio::time::timeout(Duration::from_secs(8), handle)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        // A new owner may start only after the previous one released the lease.
        let restarted = bootstrap(options(&roots.path(), BootstrapMode::Serve, store.clone()))
            .await
            .unwrap();
        assert_eq!(restarted.library_id(), library);
        assert_ne!(restarted.host_instance_id(), host_instance);
        let outcome = restarted.shutdown().await;
        assert!(outcome.storage_closed);
    });
}

#[test]
fn s01_host_cli_03_initialize_enrolls_owner_without_a_listener() {
    let rt = runtime();
    rt.block_on(async {
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let host = bootstrap(options(
            &roots.path(),
            BootstrapMode::Initialize,
            store.clone(),
        ))
        .await
        .unwrap();
        let library = host.library_id();
        // Explicit first-run enrollment happened through the injected store.
        let key = CredentialKey::owner(CredentialEnvironment::Test, library);
        let secret = store.read(&key).unwrap();
        assert_eq!(secret.as_bytes().len(), 33);
        assert!(!secret.as_bytes()[1..].iter().all(|byte| *byte == 0));
        let outcome = host.shutdown().await;
        assert!(outcome.storage_closed);
        assert!(outcome.log_flushed);
        // Initialize never binds the secure endpoint.
        assert!(sm_pipe::open(&roots.endpoint(), &image()).is_err());
        assert!(HostLease::acquire(roots.layout()).is_ok());
    });
}

/// Refuses every owner-credential write: the composition must fail closed at the credential
/// step, before the bounded logger of S01 §6.3 step 5b is created.
struct RefusingStore;
impl SecretStore for RefusingStore {
    fn read(&self, _key: &CredentialKey) -> Result<Secret, StoreError> {
        Err(StoreError::Missing)
    }
    fn create_new(&self, _key: &CredentialKey, _value: &Secret) -> Result<(), StoreError> {
        Err(StoreError::Unavailable)
    }
}

/// Witness of the fixed step order: at the moment the owner credential is written, no bounded
/// logger file may exist yet. It can only ever observe a violation of the order.
struct ProbeStore {
    inner: Arc<MemoryStore>,
    cache: PathBuf,
    logged_before_enrollment: Arc<AtomicBool>,
}
impl SecretStore for ProbeStore {
    fn read(&self, key: &CredentialKey) -> Result<Secret, StoreError> {
        self.inner.read(key)
    }
    fn create_new(&self, key: &CredentialKey, value: &Secret) -> Result<(), StoreError> {
        if count_operational_logs(&self.cache) > 0 {
            self.logged_before_enrollment.store(true, Ordering::SeqCst);
        }
        self.inner.create_new(key, value)
    }
}

fn count_operational_logs(cache: &Path) -> usize {
    fs::read_dir(cache)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("operational-")
        })
        .count()
}

/// Bounded wait for the root to become free again. The storage worker closes SQLite and the
/// pinned entries before it drops the lease, so a re-acquirable lease also means the owned
/// files are closed.
fn await_root_release(roots: &Roots) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if let Ok(lease) = HostLease::acquire(roots.layout()) {
            drop(lease);
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("a refused composition did not release the owned root");
}

#[test]
fn s01_host_cli_05_owner_enrollment_precedes_the_logger() {
    let rt = runtime();
    rt.block_on(async {
        // 1) A refused owner enrollment fails closed at the credential step (exit 6) and no
        // file logger was ever created, even though storage/migration already ran.
        let refused = Roots::new();
        let result = bootstrap(options(
            &refused.path(),
            BootstrapMode::Serve,
            Arc::new(RefusingStore),
        ))
        .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("a refused owner enrollment must fail the composition"),
        };
        assert!(matches!(error, HostError::CredentialUnavailable));
        assert_eq!(error.exit_code(), sm_host::EXIT_POLICY);
        assert!(fs::metadata(refused.data().join("library.sqlite3")).is_ok());
        assert_eq!(
            count_operational_logs(&refused.cache()),
            0,
            "the bounded logger must not start before a successful owner enrollment"
        );
        await_root_release(&refused);

        // 2) The order witness and its counterpart: no logger file existed when the owner was
        // written, and after a successful enrollment the logger does exist, so the absence in
        // case 1 is a real ordering fact and not an unused logger.
        let roots = Roots::new();
        let store = Arc::new(MemoryStore::default());
        let logged_before_enrollment = Arc::new(AtomicBool::new(false));
        let probe = Arc::new(ProbeStore {
            inner: store.clone(),
            cache: roots.cache(),
            logged_before_enrollment: logged_before_enrollment.clone(),
        });
        let host = bootstrap(options(&roots.path(), BootstrapMode::Serve, probe))
            .await
            .unwrap();
        assert!(
            !logged_before_enrollment.load(Ordering::SeqCst),
            "the bounded logger started before the owner enrollment completed"
        );
        let outcome = host.shutdown().await;
        assert!(outcome.storage_closed);
        assert!(
            count_operational_logs(&roots.cache()) >= 1,
            "the bounded logger must exist after a successful owner enrollment"
        );
    });
}

#[test]
fn s01_host_cli_04_unprepared_root_is_refused_without_creation() {
    let rt = runtime();
    rt.block_on(async {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let missing = parent.join(format!("sm-host-missing-{}", Uuid::new_v4()));
        let store = Arc::new(MemoryStore::default());
        let refused = bootstrap(BootstrapOptions {
            root: missing.clone(),
            mode: BootstrapMode::Serve,
            store,
        })
        .await;
        assert!(matches!(refused, Err(HostError::Root(_))));
        let error = match refused {
            Err(error) => error,
            Ok(_) => panic!("an unprepared root must be refused"),
        };
        // No directory, database or lock file is created for a refused root.
        assert!(!missing.exists());
        assert_eq!(error.exit_code(), sm_host::EXIT_HOST);
    });
}
