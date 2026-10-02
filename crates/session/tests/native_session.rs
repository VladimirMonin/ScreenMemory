#![cfg(windows)]
use async_trait::async_trait;
use sm_application::{
    credentials::{
        Authenticator, CredentialEnvironment, CredentialKey, Secret, SecretStore, StoreError,
    },
    FoundationApi, FoundationControlApi, RequestContext,
};
use sm_domain::{HostInstanceId, LibraryId, ReportId};
use sm_framing::BufferBudget;
use sm_isolation::{LibraryRootResolver, RootSource};
use sm_pipe::{Endpoint, Listener, TrustedServerImage};
use sm_protocol::{
    application::{
        AiPolicyPatch, AiPolicyView, AuthorizeProbeRequest, BridgeState, CheckResult, DoctorReport,
        DoctorRequest, HostState, HostStatus, LibraryRecordsPage, LibraryRecordsRequest,
        ProbeConsentReceipt, RuntimeProbeReport, RuntimeProbeRequest, ShutdownReceipt,
        APPLICATION_VERSION,
    },
    errors::{AppError, ErrorCode},
    values::{BuildId, MethodName, SafeMessage},
};
use sm_session::{run, ApplicationClient, ClientError, CredentialSource, SessionHost};
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
use tokio::{sync::Notify, task::JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Roots(PathBuf);
impl Roots {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = parent.join(format!("t06c-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(child)).unwrap();
        }
        Self(root)
    }
    fn endpoint(&self) -> Endpoint {
        let layout = LibraryRootResolver::resolve(RootSource::Test {
            root: &self.0,
            forbidden_donor_roots: &[],
        })
        .unwrap();
        Endpoint::for_layout(&layout).unwrap()
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
struct Services {
    library: LibraryId,
    host: HostInstanceId,
    status_calls: AtomicUsize,
    doctor_calls: AtomicUsize,
    shutdown_calls: AtomicUsize,
    doctor_started: AtomicUsize,
    release: Notify,
    delay: AtomicUsize,
    fail_status: AtomicUsize,
}
impl Services {
    fn status_value(&self) -> HostStatus {
        HostStatus {
            host_instance_id: self.host,
            library_id: self.library,
            build_id: BuildId::parse("synthetic-t06c").unwrap(),
            protocol: APPLICATION_VERSION,
            state: HostState::Ready,
            schema_version: 1,
            ai_enabled: false,
            bridge_state: BridgeState::Stopped,
            available_methods: ["system.status", "system.doctor", "system.request_shutdown"]
                .iter()
                .map(|s| MethodName::parse(*s).unwrap())
                .collect(),
        }
    }
}
#[async_trait]
impl FoundationApi for Services {
    async fn status(&self, ctx: RequestContext) -> Result<HostStatus, AppError> {
        assert!(ctx.connection_id().is_some());
        self.status_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_status.load(Ordering::SeqCst) != 0 {
            return Err(AppError {
                code: ErrorCode::RuntimeUnavailable,
                safe_message: SafeMessage::parse("Synthetic status failure.").unwrap(),
                retryable: false,
            });
        }
        Ok(self.status_value())
    }
    async fn doctor(
        &self,
        ctx: RequestContext,
        request: DoctorRequest,
        _: CancellationToken,
    ) -> Result<DoctorReport, AppError> {
        assert!(ctx.connection_id().is_some());
        assert!(!request.include_runtime_inventory);
        self.doctor_calls.fetch_add(1, Ordering::SeqCst);
        if self.delay.load(Ordering::SeqCst) > 0 {
            self.doctor_started.fetch_add(1, Ordering::SeqCst);
            self.release.notified().await;
        }
        Ok(DoctorReport {
            report_id: ReportId::new(Uuid::new_v4()),
            host: self.status_value(),
            checks: Vec::<CheckResult>::new(),
        })
    }
    async fn probe_runtime(
        &self,
        _: RequestContext,
        _: RuntimeProbeRequest,
        _: CancellationToken,
    ) -> Result<RuntimeProbeReport, AppError> {
        panic!("unsupported handler invoked")
    }
    async fn list_records(
        &self,
        _: RequestContext,
        _: LibraryRecordsRequest,
    ) -> Result<LibraryRecordsPage, AppError> {
        Ok(LibraryRecordsPage {
            items: Vec::new(),
            next_cursor: None,
        })
    }
}
#[async_trait]
impl FoundationControlApi for Services {
    async fn set_ai_policy(
        &self,
        _: RequestContext,
        _: AiPolicyPatch,
    ) -> Result<AiPolicyView, AppError> {
        panic!("unsupported handler invoked")
    }
    async fn authorize_probe(
        &self,
        _: RequestContext,
        _: AuthorizeProbeRequest,
    ) -> Result<ProbeConsentReceipt, AppError> {
        panic!("unsupported handler invoked")
    }
    async fn request_shutdown(&self, ctx: RequestContext) -> Result<ShutdownReceipt, AppError> {
        assert!(ctx.connection_id().is_some());
        self.shutdown_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ShutdownReceipt {
            host_instance_id: self.host,
            state: HostState::Stopping,
        })
    }
}
struct Fixture {
    _roots: Roots,
    endpoint: Endpoint,
    image: TrustedServerImage,
    store: Arc<MemoryStore>,
    library: LibraryId,
    host: HostInstanceId,
    services: Arc<Services>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), sm_session::SessionError>>,
}
impl Fixture {
    fn new() -> Self {
        Self::new_with_drain(None)
    }
    /// `Some(drain)` exercises the integration hook: on stop the accept loop gives already
    /// admitted connections a bounded window to deliver an in-flight response.
    fn new_with_drain(drain: Option<Duration>) -> Self {
        let roots = Roots::new();
        let endpoint = roots.endpoint();
        let listener = Listener::bind(roots.endpoint()).unwrap();
        let image =
            TrustedServerImage::from_configuration(&std::env::current_exe().unwrap()).unwrap();
        let library = LibraryId::new(Uuid::new_v4());
        let host = HostInstanceId::new(Uuid::new_v4());
        let store = Arc::new(MemoryStore::default());
        let owner = CredentialKey::owner(CredentialEnvironment::Test, library);
        let reader = CredentialKey::test_read_only(library);
        let owner_auth = Authenticator::enroll_owner(store.as_ref(), owner).unwrap();
        let reader_auth = Authenticator::enroll_test_read_only(store.as_ref(), reader).unwrap();
        let services = Arc::new(Services {
            library,
            host,
            status_calls: AtomicUsize::new(0),
            doctor_calls: AtomicUsize::new(0),
            shutdown_calls: AtomicUsize::new(0),
            doctor_started: AtomicUsize::new(0),
            release: Notify::new(),
            delay: AtomicUsize::new(0),
            fail_status: AtomicUsize::new(0),
        });
        let budget = BufferBudget::new();
        let session = Arc::new(SessionHost::new(
            library,
            host,
            owner_auth,
            Some(reader_auth),
            services.clone(),
            services.clone(),
            // The S01 session fixtures own no capture handler; S02 composition tests cover
            // the advertised capture methods with a real injected adapter.
            None,
            budget,
        ));
        let stop = CancellationToken::new();
        let token = stop.clone();
        let task = tokio::spawn(async move {
            let mut listener = listener;
            match drain {
                Some(value) => sm_session::run_draining(&mut listener, session, token, value).await,
                None => run(&mut listener, session, token).await,
            }
        });
        Self {
            _roots: roots,
            endpoint,
            image,
            store,
            library,
            host,
            services,
            stop,
            task,
        }
    }
    async fn client(&self, key: CredentialKey) -> ApplicationClient {
        ApplicationClient::connect(
            &self.endpoint,
            &self.image,
            self.library,
            Some(self.host),
            &CredentialSource::Store {
                store: self.store.clone(),
                key,
            },
            BufferBudget::new(),
        )
        .await
        .unwrap()
    }
    async fn close(self) {
        self.stop.cancel();
        self.services.release.notify_waiters();
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
#[tokio::test]
async fn s01_ipc_03_authenticated_native_owner_readonly_and_reconnect() {
    let f = Fixture::new();
    let owner = f
        .client(CredentialKey::owner(CredentialEnvironment::Test, f.library))
        .await;
    assert_eq!(owner.status().await.unwrap().host_instance_id, f.host);
    assert_eq!(owner.doctor().await.unwrap().host.library_id, f.library);
    assert!(matches!(
        owner.request_shutdown().await.unwrap().state,
        HostState::Stopping
    ));
    let ro = f.client(CredentialKey::test_read_only(f.library)).await;
    assert!(matches!(
        ro.request_shutdown().await,
        Err(ClientError::Application(AppError {
            code: ErrorCode::PermissionDenied,
            ..
        }))
    ));
    assert_eq!(f.services.shutdown_calls.load(Ordering::SeqCst), 1);
    drop(owner);
    drop(ro);
    let again = f
        .client(CredentialKey::owner(CredentialEnvironment::Test, f.library))
        .await;
    assert_eq!(again.status().await.unwrap().library_id, f.library);
    drop(again);
    f.close().await;
}
#[tokio::test]
async fn s01_ipc_03_rejects_bad_identity_and_secret_before_handlers() {
    let f = Fixture::new();
    let source = CredentialSource::Store {
        store: f.store.clone(),
        key: CredentialKey::owner(CredentialEnvironment::Test, f.library),
    };
    let other = LibraryId::new(Uuid::new_v4());
    assert!(ApplicationClient::connect(
        &f.endpoint,
        &f.image,
        other,
        None,
        &source,
        BufferBudget::new()
    )
    .await
    .is_err());
    let mut bad = vec![1; 33];
    bad[1] = 77;
    let wrong = CredentialSource::TestReadOnly {
        key: CredentialKey::test_read_only(f.library),
        secret: Secret::from_bytes(bad),
    };
    assert!(ApplicationClient::connect(
        &f.endpoint,
        &f.image,
        f.library,
        None,
        &wrong,
        BufferBudget::new()
    )
    .await
    .is_err());
    assert_eq!(f.services.status_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.services.doctor_calls.load(Ordering::SeqCst), 0);
    f.close().await;
}
#[tokio::test]
async fn s01_ipc_03_typed_error_cancel_and_host_binding() {
    let f = Fixture::new();
    let source = CredentialSource::Store {
        store: f.store.clone(),
        key: CredentialKey::owner(CredentialEnvironment::Test, f.library),
    };
    assert!(ApplicationClient::connect(
        &f.endpoint,
        &f.image,
        f.library,
        Some(HostInstanceId::new(Uuid::new_v4())),
        &source,
        BufferBudget::new()
    )
    .await
    .is_err());
    assert_eq!(f.services.status_calls.load(Ordering::SeqCst), 0);
    let client = Arc::new(
        f.client(CredentialKey::owner(CredentialEnvironment::Test, f.library))
            .await,
    );
    f.services.fail_status.store(1, Ordering::SeqCst);
    assert!(matches!(
        client.status().await,
        Err(ClientError::Application(AppError {
            code: ErrorCode::RuntimeUnavailable,
            ..
        }))
    ));
    f.services.fail_status.store(0, Ordering::SeqCst);
    f.services.delay.store(1, Ordering::SeqCst);
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let client = client.clone();
        let cancellation = cancellation.clone();
        async move { client.doctor_cancellable(cancellation).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.services.doctor_started.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap(),
        Err(ClientError::Application(AppError {
            code: ErrorCode::WaitTimeout,
            ..
        }))
    ));
    assert_eq!(client.status().await.unwrap().library_id, f.library);
    drop(client);
    f.close().await;
}
#[tokio::test]
async fn s01_ipc_03_parallel_delay_keeps_fast_status_and_bounds_active() {
    let f = Fixture::new();
    f.services.delay.store(1, Ordering::SeqCst);
    let client = Arc::new(
        f.client(CredentialKey::owner(CredentialEnvironment::Test, f.library))
            .await,
    );
    let mut tasks = JoinSet::new();
    for _ in 0..7 {
        let c = client.clone();
        tasks.spawn(async move { c.doctor().await });
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.services.doctor_started.load(Ordering::SeqCst) < 7 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(client.status().await.unwrap().library_id, f.library);
    let c = client.clone();
    tasks.spawn(async move { c.doctor().await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.services.doctor_started.load(Ordering::SeqCst) < 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        client.status().await,
        Err(ClientError::Session(sm_session::SessionError::Capacity))
    ));
    f.services.release.notify_waiters();
    while let Some(result) = tasks.join_next().await {
        assert!(result.unwrap().is_ok());
    }
    drop(client);
    f.close().await;
}

/// The host-side integration hook: after stop is signalled, an already admitted
/// connection must still be able to receive its in-flight authorized response.
#[test]
fn s01_host_cli_stop_drain_preserves_in_flight_response() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut f = Fixture::new_with_drain(Some(Duration::from_millis(1500)));
        f.services.delay.store(1, Ordering::SeqCst);
        let client = Arc::new(
            f.client(CredentialKey::owner(CredentialEnvironment::Test, f.library))
                .await,
        );
        let doctor = tokio::spawn({
            let client = client.clone();
            async move { client.doctor().await }
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            while f.services.doctor_started.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        f.stop.cancel();
        // While the delayed handler is in flight the accept loop stays in its bounded
        // drain instead of aborting the connection task immediately.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut f.task)
                .await
                .is_err(),
            "stop drain returned before the in-flight handler finished"
        );
        f.services.release.notify_one();
        let report = tokio::time::timeout(Duration::from_secs(3), doctor)
            .await
            .expect("in-flight response was not delivered inside the drain")
            .unwrap()
            .unwrap();
        assert_eq!(report.host.library_id, f.library);
        let served = tokio::time::timeout(Duration::from_secs(4), &mut f.task)
            .await
            .unwrap()
            .unwrap();
        assert!(served.is_ok());
        drop(client);
    });
}
