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
        let root = parent.join(Uuid::new_v4().to_string());
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
        if std::thread::panicking() {
            return;
        }
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
    search_started: AtomicUsize,
    search_dropped: AtomicUsize,
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
        Self::configured(drain, None)
    }
    fn configured(
        drain: Option<Duration>,
        images: Option<(
            Roots,
            LibraryId,
            Arc<dyn sm_application::images::ImageLibraryApi>,
        )>,
    ) -> Self {
        Self::configured_search(drain, images, false)
    }
    fn configured_search(
        drain: Option<Duration>,
        images: Option<(
            Roots,
            LibraryId,
            Arc<dyn sm_application::images::ImageLibraryApi>,
        )>,
        search: bool,
    ) -> Self {
        let (roots, library, images) = match images {
            Some((roots, library, images)) => (roots, library, Some(images)),
            None => (Roots::new(), LibraryId::new(Uuid::new_v4()), None),
        };
        let endpoint = roots.endpoint();
        let listener = Listener::bind(roots.endpoint()).unwrap();
        let image =
            TrustedServerImage::from_configuration(&std::env::current_exe().unwrap()).unwrap();
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
            search_started: AtomicUsize::new(0),
            search_dropped: AtomicUsize::new(0),
        });
        let budget = BufferBudget::new();
        let session = SessionHost::new(
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
        );
        let session = if search {
            session.with_search(services.clone())
        } else {
            session
        };
        let session = Arc::new(match images {
            Some(images) => session.with_images(images),
            None => session,
        });
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
#[async_trait]
impl sm_application::search::SearchApi for Services {
    async fn search(
        &self,
        _ctx: RequestContext,
        request: sm_protocol::search::SearchRequest,
        _cancel: CancellationToken,
    ) -> Result<sm_protocol::search::SearchReply, AppError> {
        struct Finished<'a>(&'a AtomicUsize);
        impl Drop for Finished<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let _finished = Finished(&self.search_dropped);
        self.search_started.fetch_add(1, Ordering::SeqCst);
        if self.delay.load(Ordering::SeqCst) > 0 {
            self.release.notified().await;
        }
        let sm_protocol::search::SearchRequest::Start(r) = request else {
            panic!("test start")
        };
        Ok(sm_protocol::search::SearchReply::Page(
            sm_protocol::search::SearchPage {
                search_id: r.search_id,
                projection_epoch: sm_protocol::knowledge::SqlRevision(1),
                candidate_depth: r.candidate_depth,
                window_truncated: false,
                next_cursor: None,
                items: Vec::new(),
            },
        ))
    }
}
#[tokio::test]
async fn s06_native_search_cancel_terminal_drain_scoped_and_next_rpc() {
    let f = Fixture::configured_search(None, None, true);
    f.services.delay.store(1, Ordering::SeqCst);
    let owner = Arc::new(
        f.client(CredentialKey::owner(CredentialEnvironment::Test, f.library))
            .await,
    );
    let other = Arc::new(
        f.client(CredentialKey::owner(CredentialEnvironment::Test, f.library))
            .await,
    );
    let request = || {
        sm_protocol::search::SearchRequest::Start(sm_protocol::search::SearchStart {
            search_id: sm_protocol::knowledge::KnowledgeId(Uuid::new_v4()),
            text: "synthetic".into(),
            filters: Default::default(),
            limit: 30,
            candidate_depth: 100,
        })
    };
    let token = CancellationToken::new();
    let signal = token.clone();
    let c = owner.clone();
    let r = request();
    let a = tokio::spawn(async move { c.search_cancellable(r, token).await });
    let c = other.clone();
    let r = request();
    let b = tokio::spawn(async move { c.search(r).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.services.search_started.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    signal.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), a)
            .await
            .unwrap()
            .unwrap(),
        Err(ClientError::Application(AppError {
            code: ErrorCode::WaitTimeout,
            ..
        }))
    ));
    assert!(!b.is_finished());
    assert_eq!(f.services.search_dropped.load(Ordering::SeqCst), 1);
    owner.status().await.unwrap();
    f.services.delay.store(0, Ordering::SeqCst);
    f.services.release.notify_waiters();
    b.await.unwrap().unwrap();
    owner.search(request()).await.unwrap();
    drop(owner);
    drop(other);
    f.close().await;
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
async fn s03_native_rpc_real_import_survives_client_close_and_exports() {
    use sm_application::storage::FoundationRepository;
    use sm_domain::{ImportBatchId, OperationId};
    use sm_protocol::images::*;
    let roots = Roots::new();
    let external = Roots::new();
    let storage = sm_storage::StorageWorker::start(
        sm_isolation::HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &roots.0,
                forbidden_donor_roots: &[],
            })
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let library = LibraryId::new(storage.schema_snapshot().await.unwrap().library_id);
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.pi/staging-target-t09b/debug/shot-image-worker.exe");
    let worker = Arc::new(
        sm_image_worker::TrustedWorker::from_manifest(
            &path,
            sm_image::sha256(&fs::read(&path).unwrap()),
        )
        .unwrap(),
    );
    let images = Arc::new(sm_storage::image_services::ImageServices::new(
        storage.clone(),
        worker,
    ));
    let source = external.0.join("source.png");
    fs::write(
        &source,
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    let req = ImportRequest {
        batch_id: ImportBatchId::new(Uuid::new_v4()),
        source_paths: vec![source.to_string_lossy().into_owned()],
        profile: CanonicalProfile::WebpExact,
        duplicates: DuplicatePolicy::KeepBoth,
    };
    let batch = req.batch_id;
    let f = Fixture::configured(None, Some((roots, library, images)));
    let owner = f
        .client(CredentialKey::owner(CredentialEnvironment::Test, library))
        .await;
    let readonly = f.client(CredentialKey::test_read_only(library)).await;
    assert!(matches!(
        readonly.import_paths(req.clone()).await,
        Err(ClientError::Application(AppError {
            code: ErrorCode::PermissionDenied,
            ..
        }))
    ));
    owner.import_paths(req).await.unwrap();
    drop(owner);
    let owner = f
        .client(CredentialKey::owner(CredentialEnvironment::Test, library))
        .await;
    let end = tokio::time::Instant::now() + Duration::from_secs(30);
    let report = loop {
        let report = owner
            .import_status(ImportBatchRequest { batch_id: batch })
            .await
            .unwrap();
        if report.saved_new == 1 && !report.active_work {
            break report;
        }
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    fs::remove_file(source).unwrap();
    let imported = report.items[0].receipt.as_ref().unwrap();
    let preview = owner
        .preview_asset(sm_protocol::application::LibraryAssetRequest {
            asset_id: imported.original_asset_id,
            offset: 0,
        })
        .await
        .unwrap();
    assert_eq!(preview.mime, "image/webp");
    assert!(preview.eof);
    assert_eq!(preview.sha256, imported.canonical_sha256);
    assert!(matches!(
        readonly
            .record_metadata(RecordMetadataRequest {
                record_id: imported.record_id
            })
            .await,
        Err(ClientError::Application(AppError {
            code: ErrorCode::PermissionDenied,
            ..
        }))
    ));
    let metadata = owner
        .record_metadata(RecordMetadataRequest {
            record_id: imported.record_id,
        })
        .await
        .unwrap();
    assert_eq!(metadata.source_kind, "imported_image");
    assert_eq!(metadata.source_reports.len(), 1);
    let export = ExportRequest {
        operation_id: OperationId::new(Uuid::new_v4()),
        record_id: imported.record_id,
        asset_id: None,
        format: ExportFormat::Png,
        max_side: Some(2),
        jpeg_background: None,
        destination_path: None,
        metadata_policy: ExportMetadataPolicy::StripPrivate,
    };
    assert!(matches!(
        readonly.export_image(export.clone(), None).await,
        Err(ClientError::Application(AppError {
            code: ErrorCode::PermissionDenied,
            ..
        }))
    ));
    let receipt = owner.export_image(export, None).await.unwrap();
    assert!(fs::read(receipt.export_path)
        .unwrap()
        .starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(receipt.source_asset_id, imported.original_asset_id);
    drop(owner);
    drop(readonly);
    storage.shutdown(Duration::from_secs(20)).await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn s03_native_import_export_owner_scope_and_readonly_denial() {
    use sm_domain::{ImportBatchId, OperationId, RecordId};
    use sm_protocol::images::*;
    let f = Fixture::new();
    let owner = f
        .client(CredentialKey::owner(CredentialEnvironment::Test, f.library))
        .await;
    let ro = f.client(CredentialKey::test_read_only(f.library)).await;
    let import = ImportRequest {
        batch_id: ImportBatchId::new(Uuid::new_v4()),
        source_paths: vec![r"C:\synthetic\image.png".into()],
        profile: CanonicalProfile::WebpExact,
        duplicates: DuplicatePolicy::ReuseRecord,
    };
    assert!(matches!(
        ro.import_paths(import.clone()).await,
        Err(ClientError::Application(AppError {
            code: ErrorCode::PermissionDenied,
            ..
        }))
    ));
    // The optional service has not been composed in this existing session fixture.
    // Owner reaches the typed capability boundary; no mock Saved result is supplied.
    assert!(matches!(
        owner.import_paths(import).await,
        Err(ClientError::Application(AppError {
            code: ErrorCode::MethodNotSupported,
            ..
        }))
    ));
    let export = ExportRequest {
        operation_id: OperationId::new(Uuid::new_v4()),
        record_id: RecordId::new(Uuid::new_v4()),
        asset_id: None,
        format: ExportFormat::Png,
        max_side: None,
        jpeg_background: None,
        destination_path: None,
        metadata_policy: ExportMetadataPolicy::StripPrivate,
    };
    assert!(matches!(
        ro.export_image(export.clone(), None).await,
        Err(ClientError::Application(AppError {
            code: ErrorCode::PermissionDenied,
            ..
        }))
    ));
    assert!(matches!(
        owner.export_image(export, None).await,
        Err(ClientError::Application(AppError {
            code: ErrorCode::MethodNotSupported,
            ..
        }))
    ));
    drop(owner);
    drop(ro);
    f.close().await;
}

#[tokio::test]
async fn s05_native_knowledge_closed_scope_and_truthful_uncomposed_port() {
    use sm_protocol::knowledge::*;
    let f = Fixture::new();
    let owner = f
        .client(CredentialKey::owner(CredentialEnvironment::Test, f.library))
        .await;
    let ro = f.client(CredentialKey::test_read_only(f.library)).await;
    let id = KnowledgeId(Uuid::new_v4());
    for req in [
        KnowledgeRequest::Read { record_id: id },
        KnowledgeRequest::Categories {},
        KnowledgeRequest::Edit(CardEdit {
            operation_id: KnowledgeId(Uuid::new_v4()),
            record_id: id,
            expected_record_revision: SqlRevision(1),
            expected_catalog_revision: SqlRevision(1),
            expected_category_revision: None,
            patch: CardPatch::default(),
        }),
        KnowledgeRequest::Category(CategoryMutation {
            operation_id: KnowledgeId(Uuid::new_v4()),
            expected_catalog_revision: SqlRevision(1),
            change: CategoryChange::Create {
                name: CategoryName("synthetic".into()),
                description: CategoryDescription(String::new()),
            },
        }),
    ] {
        assert!(matches!(
            ro.knowledge(req.clone()).await,
            Err(ClientError::Application(AppError {
                code: ErrorCode::PermissionDenied,
                ..
            }))
        ));
        assert!(matches!(
            owner.knowledge(req).await,
            Err(ClientError::Application(AppError {
                code: ErrorCode::MethodNotSupported,
                ..
            }))
        ));
    }
    for req in [
        sm_protocol::search::SearchRequest::Start(sm_protocol::search::SearchStart {
            search_id: id,
            text: "literal".into(),
            filters: Default::default(),
            limit: 30,
            candidate_depth: 100,
        }),
        sm_protocol::search::SearchRequest::Page { cursor: id },
        sm_protocol::search::SearchRequest::Cancel { search_id: id },
    ] {
        assert!(matches!(
            ro.search(req.clone()).await,
            Err(ClientError::Application(AppError {
                code: ErrorCode::PermissionDenied,
                ..
            }))
        ));
        assert!(matches!(
            owner.search(req).await,
            Err(ClientError::Application(AppError {
                code: ErrorCode::MethodNotSupported,
                ..
            }))
        ));
    }
    drop(owner);
    drop(ro);
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
