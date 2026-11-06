use super::*;
use sm_application::{
    access::{AgentAccessApi, AgentAuthority},
    credentials::Authenticator,
    knowledge::KnowledgeApi,
    search::SearchApi,
    storage::{FoundationRepository, *},
    FoundationApi, FoundationControlApi, Scope,
};
use sm_domain::{
    AssetId, CorrelationId, HostInstanceId, OperationId, PrincipalId, RecordId, RequestId,
};
use sm_framing::BufferBudget;
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use sm_pipe::{Endpoint, Listener, TrustedServerImage};
use sm_protocol::{application::*, knowledge::*, search::*};
use sm_session::{ApplicationClient, ClientError, CredentialSource, SessionHost};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
fn id() -> KnowledgeId {
    KnowledgeId(Uuid::new_v4())
}
fn context(p: PrincipalId) -> RequestContext {
    RequestContext::from_authenticated_peer(
        RequestId::new(Uuid::new_v4()),
        CorrelationId::new(Uuid::new_v4()),
        p,
        vec![
            Scope::ManageAccess,
            Scope::ReadMetadata,
            Scope::WriteMetadata,
            Scope::ManageCategories,
            Scope::ReadImages,
            Scope::ReadPaths,
            Scope::ReadSourceMetadata,
        ],
    )
}
#[derive(Default)]
struct Store {
    values: Mutex<HashMap<String, Secret>>,
    writes: AtomicUsize,
    fail_delete: AtomicBool,
}
impl SecretStore for Store {
    fn read(&self, key: &CredentialKey) -> Result<Secret, StoreError> {
        self.values
            .lock()
            .unwrap()
            .get(&key.target())
            .map(|s| Secret::from_bytes(s.as_bytes().to_vec()))
            .ok_or(StoreError::Missing)
    }
    fn create_new(&self, key: &CredentialKey, value: &Secret) -> Result<(), StoreError> {
        let mut values = self.values.lock().unwrap();
        if values.contains_key(&key.target()) {
            return Err(StoreError::Unavailable);
        }
        values.insert(key.target(), Secret::from_bytes(value.as_bytes().to_vec()));
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn delete_agent(&self, key: &CredentialKey) -> Result<(), StoreError> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable);
        }
        if !matches!(
            key.profile(),
            sm_application::credentials::Profile::Agent(_)
        ) {
            return Err(StoreError::Unavailable);
        }
        self.values.lock().unwrap().remove(&key.target());
        Ok(())
    }
}
struct Fixture {
    root: PathBuf,
    workers: Mutex<Vec<StorageWorker>>,
}
impl Fixture {
    fn new() -> Self {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.pi/temporary")
            .canonicalize()
            .unwrap();
        let root = PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\"))
            .join(Uuid::new_v4().to_string());
        std::fs::create_dir(&root).unwrap();
        for c in ["data", "config", "cache", "webview"] {
            std::fs::create_dir(root.join(c)).unwrap()
        }
        Self {
            root,
            workers: Mutex::new(vec![]),
        }
    }
    fn layout(&self) -> sm_isolation::NamespaceLayout {
        LibraryRootResolver::resolve(RootSource::Test {
            root: &self.root,
            forbidden_donor_roots: &[],
        })
        .unwrap()
    }
    fn start(&self) -> StorageWorker {
        let w = StorageWorker::start(HostLease::acquire(self.layout()).unwrap()).unwrap();
        self.workers.lock().unwrap().push(w.clone());
        w
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let workers = std::mem::take(self.workers.get_mut().unwrap_or_else(|e| e.into_inner()));
        let stopped = std::thread::spawn(move || {
            runtime().block_on(async {
                for w in workers {
                    if w.shutdown(Duration::from_secs(5)).await.is_err() {
                        return false;
                    }
                }
                true
            })
        })
        .join()
        .unwrap_or(false);
        if stopped {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => println!(
                    "OWN_ROOT_ABSENT {}",
                    self.root.file_name().unwrap().to_string_lossy()
                ),
                Err(_) => {
                    eprintln!(
                        "OWN_ROOT_RETAINED {}",
                        self.root.file_name().unwrap().to_string_lossy()
                    );
                    if !std::thread::panicking() {
                        panic!("root cleanup refused")
                    }
                }
            }
        } else {
            eprintln!(
                "OWN_WORKER_STOP_FAILED {}",
                self.root.file_name().unwrap().to_string_lossy()
            );
            if !std::thread::panicking() {
                panic!("worker stop refused")
            }
        }
    }
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn create() -> AccessRequest {
    AccessRequest::CreateProfile {
        operation_id: id(),
        name: CategoryName("Agent".into()),
        policy: AgentPolicy::default(),
    }
}
fn issue(p: &AgentProfileView) -> AccessRequest {
    AccessRequest::IssueConnection {
        operation_id: id(),
        profile_id: p.profile_id,
        expected_profile_revision: p.revision,
        display_name: CategoryName("Connection".into()),
    }
}
fn profile(r: AccessReply) -> AgentProfileView {
    match r {
        AccessReply::Profile(p) => *p,
        _ => panic!("profile"),
    }
}
fn connection(r: AccessReply) -> AgentConnectionView {
    match r {
        AccessReply::Connection(c) => *c,
        _ => panic!("connection"),
    }
}
async fn auth(
    w: &StorageWorker,
    store: &Store,
    library: LibraryId,
    c: &AgentConnectionView,
) -> Result<RequestContext, AppError> {
    let key = CredentialKey::agent(CredentialEnvironment::Test, library, c.connection_id);
    let secret = store
        .read(&key)
        .map_err(|_| error(ErrorCode::CredentialStoreUnavailable))?;
    w.authenticate_agent(
        library,
        c.connection_id,
        &secret,
        RequestId::new(Uuid::new_v4()),
        CorrelationId::new(Uuid::new_v4()),
    )
    .await
}
#[test]
fn s07_enrollment_faults_restart_replay_missing_collision_cleanup_failure() {
    runtime().block_on(async {
        let f = Fixture::new();
        let mut w = f.start();
        let library = LibraryId::new(w.schema_snapshot().await.unwrap().library_id);
        let store = Arc::new(Store::default());
        let ctx = context(PrincipalId::new(Uuid::new_v4()));
        let mut api = AccessServices::new(
            w.clone(),
            store.clone(),
            CredentialEnvironment::Test,
            library,
        );
        let p = profile(api.access(ctx.clone(), create()).await.unwrap());
        // Before write: committed digest/IDs, absent slot; an accidental retry does not mint.
        let req = issue(&p);
        api.fault.store(1, Ordering::SeqCst);
        assert!(api.access(ctx.clone(), req.clone()).await.is_err());
        api.fault.store(0, Ordering::SeqCst);
        let planned = match w
            .agent_command(ctx.clone(), Command::EnrollmentLookup(req.clone()))
            .await
            .unwrap()
        {
            Reply::Plan(Some(p)) => p,
            _ => panic!("planned"),
        };
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
        assert!(api.access(ctx.clone(), req.clone()).await.is_err());
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
        let collision_key = CredentialKey::agent(
            CredentialEnvironment::Test,
            library,
            planned.connection.connection_id,
        );
        let collision = Secret::generate_session().unwrap();
        store.create_new(&collision_key, &collision).unwrap();
        assert!(api.access(ctx.clone(), req).await.is_err());
        assert!(store
            .read(&collision_key)
            .unwrap()
            .matches_session_digest(&collision.session_digest().unwrap()));
        // After native-equivalent write, before activation: reopen completes SAME operation.
        let req2 = issue(&p);
        api.fault.store(2, Ordering::SeqCst);
        assert!(api.access(ctx.clone(), req2.clone()).await.is_err());
        let planned2 = match w
            .agent_command(ctx.clone(), Command::EnrollmentLookup(req2.clone()))
            .await
            .unwrap()
        {
            Reply::Plan(Some(p)) => p,
            _ => panic!("planned"),
        };
        assert!(auth(&w, &store, library, &planned2.connection)
            .await
            .is_err());
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(api);
        w = f.start();
        api = AccessServices::new(
            w.clone(),
            store.clone(),
            CredentialEnvironment::Test,
            library,
        );
        let active = connection(api.access(ctx.clone(), req2.clone()).await.unwrap());
        assert_eq!(active.connection_id, planned2.connection.connection_id);
        let bound = auth(&w, &store, library, &active).await.unwrap();
        // After activation/lost reply: durable receipt replays without a second native write.
        let req3 = issue(&p);
        api.fault.store(3, Ordering::SeqCst);
        assert!(api.access(ctx.clone(), req3.clone()).await.is_err());
        let writes = store.writes.load(Ordering::SeqCst);
        api.fault.store(0, Ordering::SeqCst);
        let lost = connection(api.access(ctx.clone(), req3.clone()).await.unwrap());
        let replay = connection(api.access(ctx.clone(), req3).await.unwrap());
        assert_eq!(lost.connection_id, replay.connection_id);
        assert_eq!(store.writes.load(Ordering::SeqCst), writes);
        store.fail_delete.store(true, Ordering::SeqCst);
        let revoke = AccessRequest::RevokeConnection {
            operation_id: id(),
            connection_id: active.connection_id,
            expected_revision: active.revision,
        };
        assert!(api.access(ctx.clone(), revoke.clone()).await.is_err());
        assert!(w.validate_agent(&bound).await.is_err());
        assert!(auth(&w, &store, library, &active).await.is_err());
        assert!(auth(&w, &store, library, &lost).await.is_ok());
        store.fail_delete.store(false, Ordering::SeqCst);
        api.access(ctx.clone(), revoke).await.unwrap();
        let key = CredentialKey::agent(CredentialEnvironment::Test, library, active.connection_id);
        assert!(matches!(store.read(&key), Err(StoreError::Missing)));
        // DB-only activation is not enough when its exact slot is absent; no owner lookup.
        let key = CredentialKey::agent(CredentialEnvironment::Test, library, lost.connection_id);
        store.delete_agent(&key).unwrap();
        assert!(auth(&w, &store, library, &lost).await.is_err());
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(api);
        w = f.start();
        assert!(auth(&w, &store, library, &lost).await.is_err());
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        store.values.lock().unwrap().clear();
    });
}

struct NoFoundation;
#[async_trait::async_trait]
impl FoundationApi for NoFoundation {
    async fn status(&self, _: RequestContext) -> Result<HostStatus, AppError> {
        Err(error(ErrorCode::MethodNotSupported))
    }
    async fn doctor(
        &self,
        _: RequestContext,
        _: DoctorRequest,
        _: CancellationToken,
    ) -> Result<DoctorReport, AppError> {
        Err(error(ErrorCode::MethodNotSupported))
    }
    async fn probe_runtime(
        &self,
        _: RequestContext,
        _: RuntimeProbeRequest,
        _: CancellationToken,
    ) -> Result<RuntimeProbeReport, AppError> {
        Err(error(ErrorCode::MethodNotSupported))
    }
    async fn list_records(
        &self,
        _: RequestContext,
        _: LibraryRecordsRequest,
    ) -> Result<LibraryRecordsPage, AppError> {
        Err(error(ErrorCode::MethodNotSupported))
    }
}
#[async_trait::async_trait]
impl FoundationControlApi for NoFoundation {
    async fn set_ai_policy(
        &self,
        _: RequestContext,
        _: AiPolicyPatch,
    ) -> Result<AiPolicyView, AppError> {
        Err(error(ErrorCode::MethodNotSupported))
    }
    async fn authorize_probe(
        &self,
        _: RequestContext,
        _: AuthorizeProbeRequest,
    ) -> Result<ProbeConsentReceipt, AppError> {
        Err(error(ErrorCode::MethodNotSupported))
    }
    async fn request_shutdown(&self, _: RequestContext) -> Result<ShutdownReceipt, AppError> {
        Err(error(ErrorCode::MethodNotSupported))
    }
}
struct Delayed {
    w: StorageWorker,
    delay_search: AtomicBool,
    delay_card: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Delayed {
    async fn wait(&self, ctx: &RequestContext, delay: &AtomicBool) {
        if ctx.agent_binding().is_some() && delay.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
    }
}
#[async_trait::async_trait]
impl SearchApi for Delayed {
    async fn search(
        &self,
        c: RequestContext,
        r: SearchRequest,
        cancel: CancellationToken,
    ) -> Result<SearchReply, AppError> {
        let result = self.w.search(c.clone(), r, cancel).await?;
        self.wait(&c, &self.delay_search).await;
        Ok(result)
    }
}
#[async_trait::async_trait]
impl KnowledgeApi for Delayed {
    async fn knowledge(
        &self,
        c: RequestContext,
        r: KnowledgeRequest,
    ) -> Result<KnowledgeReply, AppError> {
        let result = self.w.knowledge(c.clone(), r).await?;
        self.wait(&c, &self.delay_card).await;
        Ok(result)
    }
}
async fn saved(w: &StorageWorker, p: PrincipalId) -> KnowledgeId {
    let rid = id();
    let aid = id();
    let image = sm_codec::RgbaImage::from_rgba(2, 2, vec![80; 16]).unwrap();
    let canonical = sm_codec::encode_exact_lossless_verified(&image)
        .unwrap()
        .into_canonical()
        .bytes()
        .to_vec();
    w.commit_image_capture(CommitImageCapture {
        operation_id: OperationId::new(Uuid::new_v4()),
        principal_id: p,
        record_id: RecordId::new(rid.0),
        asset_id: AssetId::new(aid.0),
        asset_type: AssetType::Original,
        mode: CaptureTarget::Region,
        width: 2,
        height: 2,
        capture_context: CaptureContext {
            device: "synthetic".into(),
            app: "synthetic".into(),
            title: "synthetic".into(),
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
            when: "2026-10-04T12:00:00.000Z".into(),
        }],
        warnings: vec![],
        canonical,
    })
    .await
    .unwrap();
    rid
}
fn query() -> SearchRequest {
    SearchRequest::Start(SearchStart {
        search_id: id(),
        text: "".into(),
        filters: SearchFilters::default(),
        candidate_depth: 100,
        limit: 1,
    })
}
async fn connect(
    endpoint: &Endpoint,
    library: LibraryId,
    store: Arc<Store>,
    key: CredentialKey,
) -> Result<ApplicationClient, sm_session::SessionError> {
    ApplicationClient::connect(
        endpoint,
        &TrustedServerImage::from_configuration(&std::env::current_exe().unwrap()).unwrap(),
        library,
        None,
        &CredentialSource::Store { store, key },
        BufferBudget::new(),
    )
    .await
}
fn denied<T>(r: Result<T, ClientError>) {
    assert!(matches!(r,Err(ClientError::Application(e)) if e.code==ErrorCode::PermissionDenied));
}
#[test]
fn s07_native_restricted_read_delayed_revoke_disable_owner_and_connection_b() {
    runtime().block_on(async {
        let f = Fixture::new();
        let w = f.start();
        let library = LibraryId::new(w.schema_snapshot().await.unwrap().library_id);
        let store = Arc::new(Store::default());
        let owner_key = CredentialKey::owner(CredentialEnvironment::Test, library);
        let owner = Authenticator::enroll_owner(store.as_ref(), owner_key.clone()).unwrap();
        let owner_ctx = context(owner.principal());
        let rid = saved(&w, owner.principal()).await;
        let _rid2 = saved(&w, owner.principal()).await;
        let api = Arc::new(AccessServices::new(
            w.clone(),
            store.clone(),
            CredentialEnvironment::Test,
            library,
        ));
        let delayed = Arc::new(Delayed {
            w: w.clone(),
            delay_search: AtomicBool::new(false),
            delay_card: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        use sha2::Digest;
        let worker_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.pi/staging-target-t09b/debug/shot-image-worker.exe")
            .canonicalize()
            .unwrap();
        let worker_hash: [u8; 32] =
            sha2::Sha256::digest(std::fs::read(&worker_path).unwrap()).into();
        let image_worker = Arc::new(
            sm_image_worker::TrustedWorker::from_manifest(&worker_path, worker_hash).unwrap(),
        );
        let images = Arc::new(sm_storage::image_services::ImageServices::new(
            w.clone(),
            image_worker,
        ));
        let host = Arc::new(
            SessionHost::new(
                library,
                HostInstanceId::new(Uuid::new_v4()),
                owner,
                None,
                Arc::new(NoFoundation),
                Arc::new(NoFoundation),
                None,
                BufferBudget::new(),
            )
            .with_knowledge(delayed.clone())
            .with_search(delayed.clone())
            .with_images(images)
            .with_access(api.clone(), Arc::new(w.clone())),
        );
        let endpoint = Endpoint::for_layout(&f.layout()).unwrap();
        let mut listener = Listener::bind(Endpoint::for_layout(&f.layout()).unwrap()).unwrap();
        let stop = CancellationToken::new();
        let run_stop = stop.clone();
        let task =
            tokio::spawn(async move { sm_session::run(&mut listener, host, run_stop).await });
        let owner = connect(&endpoint, library, store.clone(), owner_key)
            .await
            .unwrap();
        let p = profile(owner.access(create()).await.unwrap());
        let c = connection(owner.access(issue(&p)).await.unwrap());
        let p2 = profile(owner.access(create()).await.unwrap());
        let b = connection(owner.access(issue(&p2)).await.unwrap());
        let agent = Arc::new(
            connect(
                &endpoint,
                library,
                store.clone(),
                CredentialKey::agent(CredentialEnvironment::Test, library, c.connection_id),
            )
            .await
            .unwrap(),
        );
        let client_b = connect(
            &endpoint,
            library,
            store.clone(),
            CredentialKey::agent(CredentialEnvironment::Test, library, b.connection_id),
        )
        .await
        .unwrap();
        let page = match agent.search(query()).await.unwrap() {
            SearchReply::Page(p) => p,
            _ => panic!("page"),
        };
        let cursor = page.next_cursor.unwrap();
        assert_eq!(page.items.len(), 1);
        agent
            .knowledge(KnowledgeRequest::Read { record_id: rid })
            .await
            .unwrap();
        denied(agent.knowledge(KnowledgeRequest::Categories {}).await);
        denied(agent.access(AccessRequest::List {}).await);
        denied(agent.status().await);
        denied(agent.request_shutdown().await);
        denied(
            agent
                .read_asset(LibraryAssetRequest {
                    asset_id: AssetId::new(page.items[0].asset_id.0),
                    offset: 0,
                })
                .await,
        );
        denied(
            agent
                .preview_asset(LibraryAssetRequest {
                    asset_id: AssetId::new(page.items[0].asset_id.0),
                    offset: 0,
                })
                .await,
        );
        denied(
            agent
                .record_metadata(sm_protocol::images::RecordMetadataRequest {
                    record_id: RecordId::new(rid.0),
                })
                .await,
        );
        denied(
            agent
                .capture_command(
                    CaptureCommandParams {
                        operation_id: OperationId::new(Uuid::new_v4()),
                        mode: sm_protocol::application::CaptureMode::Region,
                    },
                    3000,
                    None,
                )
                .await,
        );
        denied(
            agent
                .processing(sm_protocol::processing::ProcessingRequest::Inventory {})
                .await,
        );
        denied(
            agent
                .import_paths(sm_protocol::images::ImportRequest {
                    batch_id: sm_domain::ImportBatchId::new(Uuid::new_v4()),
                    source_paths: vec![f
                        .root
                        .join("absent-own-input.png")
                        .to_string_lossy()
                        .into_owned()],
                    profile: sm_protocol::images::CanonicalProfile::WebpExact,
                    duplicates: sm_protocol::images::DuplicatePolicy::KeepBoth,
                })
                .await,
        );
        let catalog = match owner
            .knowledge(KnowledgeRequest::Categories {})
            .await
            .unwrap()
        {
            KnowledgeReply::Categories(c) => c,
            _ => panic!("catalog"),
        };
        let card = match owner
            .knowledge(KnowledgeRequest::Read { record_id: rid })
            .await
            .unwrap()
        {
            KnowledgeReply::Card(c) => c,
            _ => panic!("card"),
        };
        let edit = CardEdit {
            operation_id: id(),
            record_id: rid,
            expected_record_revision: card.record_revision,
            expected_catalog_revision: catalog.revision,
            expected_category_revision: None,
            patch: CardPatch {
                title: FieldPatch::Set {
                    value: CardTitle("Owner write after revoke".into()),
                },
                ..Default::default()
            },
        };
        denied(agent.knowledge(KnowledgeRequest::Edit(edit.clone())).await);
        // Backend has already produced an eligible response, then durable revoke wins before send.
        delayed.delay_search.store(true, Ordering::SeqCst);
        let a = agent.clone();
        let pending = tokio::spawn(async move { a.search(query()).await });
        tokio::time::timeout(Duration::from_secs(3), delayed.entered.notified())
            .await
            .unwrap();
        owner
            .access(AccessRequest::RevokeConnection {
                operation_id: id(),
                connection_id: c.connection_id,
                expected_revision: c.revision,
            })
            .await
            .unwrap();
        delayed.release.notify_one();
        denied(pending.await.unwrap());
        delayed.delay_search.store(false, Ordering::SeqCst);
        denied(agent.search(SearchRequest::Page { cursor }).await);
        owner.knowledge(KnowledgeRequest::Edit(edit)).await.unwrap();
        let preview = owner
            .preview_asset(LibraryAssetRequest {
                asset_id: AssetId::new(card.source_asset_id.0),
                offset: 0,
            })
            .await
            .unwrap();
        assert!(preview.eof);
        assert!(!preview.data_hex.is_empty());
        assert!(connect(
            &endpoint,
            library,
            store.clone(),
            CredentialKey::agent(
                CredentialEnvironment::Test,
                LibraryId::new(Uuid::new_v4()),
                b.connection_id
            )
        )
        .await
        .is_err());
        client_b
            .knowledge(KnowledgeRequest::Read { record_id: rid })
            .await
            .unwrap();
        client_b.search(query()).await.unwrap();
        assert!(connect(
            &endpoint,
            library,
            store.clone(),
            CredentialKey::agent(CredentialEnvironment::Test, library, c.connection_id)
        )
        .await
        .is_err());
        assert!(connect(
            &endpoint,
            library,
            store.clone(),
            CredentialKey::agent(
                CredentialEnvironment::Test,
                library,
                AgentConnectionId::new(Uuid::new_v4()).unwrap()
            )
        )
        .await
        .is_err());
        let c2 = connection(owner.access(issue(&p)).await.unwrap());
        let a2 = Arc::new(
            connect(
                &endpoint,
                library,
                store.clone(),
                CredentialKey::agent(CredentialEnvironment::Test, library, c2.connection_id),
            )
            .await
            .unwrap(),
        );
        delayed.delay_card.store(true, Ordering::SeqCst);
        let a = a2.clone();
        let pending =
            tokio::spawn(
                async move { a.knowledge(KnowledgeRequest::Read { record_id: rid }).await },
            );
        tokio::time::timeout(Duration::from_secs(3), delayed.entered.notified())
            .await
            .unwrap();
        let disabled = profile(
            owner
                .access(AccessRequest::UpdateProfile {
                    operation_id: id(),
                    profile_id: p.profile_id,
                    expected_revision: p.revision,
                    name: p.name.clone(),
                    policy: AgentPolicy::default(),
                    state: AgentProfileState::Disabled,
                })
                .await
                .unwrap(),
        );
        delayed.release.notify_one();
        denied(pending.await.unwrap());
        owner
            .access(AccessRequest::UpdateProfile {
                operation_id: id(),
                profile_id: p.profile_id,
                expected_revision: disabled.revision,
                name: p.name.clone(),
                policy: AgentPolicy::default(),
                state: AgentProfileState::Active,
            })
            .await
            .unwrap();
        denied(
            a2.knowledge(KnowledgeRequest::Read { record_id: rid })
                .await,
        );
        client_b.search(query()).await.unwrap();
        // The shared publication read permit really orders delivery before mutation COMMIT.
        let bound = auth(&w, &store, library, &b).await.unwrap();
        let permit = w.delivery_permit(&bound).await.unwrap();
        let op = AccessRequest::RevokeConnection {
            operation_id: id(),
            connection_id: b.connection_id,
            expected_revision: b.revision,
        };
        let api2 = api.clone();
        let ctx = owner_ctx.clone();
        let mut revoke = tokio::spawn(async move { api2.access(ctx, op).await });
        assert!(tokio::time::timeout(Duration::from_millis(40), &mut revoke)
            .await
            .is_err());
        drop(permit);
        revoke.await.unwrap().unwrap();
        assert!(w.validate_agent(&bound).await.is_err());
        drop(agent);
        drop(a2);
        drop(client_b);
        drop(owner);
        stop.cancel();
        assert!(task.await.unwrap().is_ok());
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        store.values.lock().unwrap().clear();
    });
}
