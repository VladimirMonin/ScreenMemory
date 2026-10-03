#![cfg(windows)]
//! Public host/session/client oracles, published SDK against an owned synthetic WS endpoint.
//! No physical LMS/model, vault, desktop, clipboard or other process authority is accessed.
use async_trait::async_trait;
use sm_application::credentials::{
    CredentialEnvironment, CredentialKey, Secret, SecretStore, StoreError,
};
use sm_domain::{ImportBatchId, LibraryId, ModelProfileId, OperationId, ProcessingJobId, Revision};
use sm_framing::BufferBudget;
use sm_host::{
    bootstrap_with_processing, processing::TrustedProcessingRuntime, BootstrapMode,
    BootstrapOptions, CaptureOptions, DEFAULT_DRAIN,
};
use sm_isolation::{LibraryRootResolver, RootSource};
use sm_pipe::{Endpoint, TrustedServerImage};
use sm_protocol::{errors::ErrorCode, images::*, processing::*};
use sm_runtime::{private_node_command, RuntimeError, SecretAuthority};
use sm_session::{ApplicationClient, CredentialSource};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdout},
};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Default)]
struct MemoryStore(Mutex<HashMap<String, Vec<u8>>>);
impl SecretStore for MemoryStore {
    fn read(&self, k: &CredentialKey) -> Result<Secret, StoreError> {
        self.0
            .lock()
            .unwrap()
            .get(&k.target())
            .cloned()
            .map(Secret::from_bytes)
            .ok_or(StoreError::Missing)
    }
    fn create_new(&self, k: &CredentialKey, v: &Secret) -> Result<(), StoreError> {
        let mut m = self.0.lock().unwrap();
        if m.contains_key(&k.target()) {
            return Err(StoreError::Unavailable);
        }
        m.insert(k.target(), v.as_bytes().to_vec());
        Ok(())
    }
}
struct Secrets;
#[async_trait]
impl SecretAuthority for Secrets {
    async fn lm_studio_token(&self) -> Result<Zeroizing<String>, RuntimeError> {
        Ok(Zeroizing::new("sk-lm-syntest1:01234567890123456789".into()))
    }
}
fn repo() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\"))
}
fn own_root() -> PathBuf {
    let r = repo()
        .join(".pi")
        .join("temporary")
        .join(Uuid::new_v4().to_string());
    fs::create_dir(&r).unwrap();
    r
}
fn library() -> PathBuf {
    let r = own_root();
    for c in ["data", "config", "cache", "webview"] {
        fs::create_dir(r.join(c)).unwrap();
    }
    r
}
fn remove_own(root: PathBuf) {
    assert_eq!(root.parent().unwrap(), repo().join(".pi/temporary"));
    assert_eq!(
        Uuid::parse_str(root.file_name().unwrap().to_str().unwrap())
            .unwrap()
            .get_version_num(),
        4
    );
    fs::remove_dir_all(root).unwrap();
}
// The accepted SDK seam permits one supervisor per process, including independent test runtimes.
static SDK_OWNER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
struct Fixture {
    node: PathBuf,
    package_root: PathBuf,
    package: PathBuf,
    worker: Arc<sm_image_worker::TrustedWorker>,
}
impl Fixture {
    async fn new() -> Self {
        let repo = repo();
        let node = repo.join(".pi/toolchains/node-v24.13.0-win-x64/node.exe");
        let package_root = own_root();
        println!(
            "owned fixture package {}",
            package_root.file_name().unwrap().to_string_lossy()
        );
        let package = package_root.join("package");
        let output = private_node_command(&node)
            .unwrap()
            .arg(repo.join("packages/lmstudio-bridge/scripts/stage.mjs"))
            .arg(&package)
            .output()
            .await
            .unwrap();
        assert!(output.status.success(), "owned package staging failed");
        use sha2::Digest;
        let exe = repo.join(".pi/staging-target-t09b/debug/shot-image-worker.exe");
        let hash = sha2::Sha256::digest(fs::read(&exe).unwrap()).into();
        let worker = Arc::new(sm_image_worker::TrustedWorker::from_manifest(&exe, hash).unwrap());
        Self {
            node,
            package_root,
            package,
            worker,
        }
    }
    fn config(&self, server: &Server, approved: bool) -> TrustedProcessingRuntime {
        TrustedProcessingRuntime::open(
            &self.package,
            server.url.clone(),
            Arc::new(Secrets),
            self.worker.clone(),
            approved,
            1024,
        )
        .unwrap()
    }
    async fn start(
        &self,
        root: &Path,
        server: &Server,
        store: Arc<MemoryStore>,
    ) -> (
        ApplicationClient,
        tokio::task::JoinHandle<Result<sm_host::ShutdownOutcome, sm_host::HostError>>,
    ) {
        self.start_config(root, store, self.config(server, true))
            .await
    }
    async fn start_config(
        &self,
        root: &Path,
        store: Arc<MemoryStore>,
        config: TrustedProcessingRuntime,
    ) -> (
        ApplicationClient,
        tokio::task::JoinHandle<Result<sm_host::ShutdownOutcome, sm_host::HostError>>,
    ) {
        println!(
            "owned fixture library {}",
            root.file_name().unwrap().to_string_lossy()
        );
        let h = bootstrap_with_processing(
            BootstrapOptions {
                root: root.to_path_buf(),
                mode: BootstrapMode::Serve,
                store: store.clone(),
                capture: CaptureOptions::Disabled,
            },
            config,
        )
        .await
        .unwrap();
        let id = h.library_id();
        let instance = h.host_instance_id();
        let task = tokio::spawn(h.serve(DEFAULT_DRAIN));
        let c = connect(root, id, Some(instance), store).await;
        (c, task)
    }
    async fn save(&self, c: &ApplicationClient) -> ImportReceipt {
        let source = self.package_root.join(format!("{}.png", Uuid::new_v4()));
        fs::write(
            &source,
            include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
        )
        .unwrap();
        let batch = ImportBatchId::new(Uuid::new_v4());
        c.import_paths(ImportRequest {
            batch_id: batch,
            source_paths: vec![source.to_string_lossy().into_owned()],
            profile: CanonicalProfile::WebpExact,
            duplicates: DuplicatePolicy::KeepBoth,
        })
        .await
        .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let r = c
                    .import_status(ImportBatchRequest { batch_id: batch })
                    .await
                    .unwrap();
                if r.discovery_complete && r.in_flight == 0 && r.failed > 0 {
                    panic!(
                        "synthetic import failed: {:?}",
                        r.items
                            .iter()
                            .map(|i| (&i.state, &i.safe_code))
                            .collect::<Vec<_>>()
                    );
                }
                if r.discovery_complete && !r.active_work && r.in_flight == 0 {
                    assert_eq!(r.directory_issues, 0, "owned synthetic source refused");
                    assert_eq!(r.saved_new, 1, "synthetic source not Saved");
                    break r.items[0].receipt.clone().unwrap();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        fs::remove_file(source).unwrap();
        result
    }
}
async fn connect(
    root: &Path,
    id: LibraryId,
    instance: Option<sm_domain::HostInstanceId>,
    store: Arc<MemoryStore>,
) -> ApplicationClient {
    let layout = LibraryRootResolver::resolve(RootSource::Test {
        root,
        forbidden_donor_roots: &[],
    })
    .unwrap();
    let endpoint = Endpoint::for_layout(&layout).unwrap();
    ApplicationClient::connect(
        &endpoint,
        &TrustedServerImage::from_configuration(&std::env::current_exe().unwrap()).unwrap(),
        id,
        instance,
        &CredentialSource::Store {
            store,
            key: CredentialKey::owner(CredentialEnvironment::Test, id),
        },
        BufferBudget::new(),
    )
    .await
    .unwrap()
}
struct Server {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
    url: String,
}
impl Server {
    async fn new(f: &Fixture) -> Self {
        let mut child = private_node_command(&f.node)
            .unwrap()
            .arg(repo().join("packages/lmstudio-bridge/test/synthetic-endpoint.mjs"))
            .arg("--stdio-fixture")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        println!(
            "owned synthetic endpoint pid {} package {}",
            child.id().unwrap(),
            f.package_root.file_name().unwrap().to_string_lossy()
        );
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let url = lines.next_line().await.unwrap().unwrap();
        assert!(url.starts_with("ws://127.0.0.1:"));
        Self { child, lines, url }
    }
    async fn line(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(30), self.lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    }
    async fn wait(&mut self, want: &str) {
        loop {
            if self.line().await == want {
                break;
            }
        }
    }
    async fn write(&mut self, msg: &str) {
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
        stdin.flush().await.unwrap();
    }
    async fn mode(&mut self, m: &str) {
        self.write(m).await;
        self.wait("mode_set").await;
    }
    async fn finish(&mut self) {
        self.write("finish").await;
        self.wait("finished").await;
    }
    async fn stats(&mut self) -> serde_json::Value {
        self.write("stats").await;
        loop {
            let line = self.line().await;
            if let Some(s) = line.strip_prefix("stats ") {
                return serde_json::from_str(s).unwrap();
            }
        }
    }
    async fn close(mut self) {
        self.child.start_kill().unwrap();
        self.child.wait().await.unwrap();
    }
}
include!("support/processing_execution_failures.rs");
include!("support/processing_interfaces.rs");
fn profile() -> ModelProfile {
    ModelProfile {
        id: ModelProfileId::new(Uuid::new_v4()),
        model_key: "synthetic/vision.gguf".into(),
        context_length: 8192,
        eval_batch_size: 256,
        flash_attention: true,
        offload_kv_cache_to_gpu: true,
        k_cache: CacheType::F16,
        v_cache: CacheType::Q8,
        native_ttl_seconds: Some(900),
        temperature: 0.2,
        max_generation_tokens: 4000,
        reasoning: ReasoningMode::BackendDefault,
    }
}
async fn policy(c: &ApplicationClient, value: ProcessingPolicy) -> PolicyView {
    let ProcessingReply::Policy(old) = c.processing(ProcessingRequest::Policy {}).await.unwrap()
    else {
        panic!()
    };
    let ProcessingReply::Policy(p) = c
        .processing(ProcessingRequest::UpdatePolicy {
            operation_id: OperationId::new(Uuid::new_v4()),
            expected_revision: old.revision,
            value,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    p
}
async fn save_profile(
    c: &ApplicationClient,
    value: ModelProfile,
    revision: Option<Revision>,
) -> ProfileView {
    let ProcessingReply::Profile(p) = c
        .processing(ProcessingRequest::SaveProfile {
            operation_id: OperationId::new(Uuid::new_v4()),
            expected_revision: revision.unwrap_or(Revision::new(0)),
            value,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    p
}
async fn policy_revision(c: &ApplicationClient) -> Revision {
    let ProcessingReply::Policy(p) = c.processing(ProcessingRequest::Policy {}).await.unwrap()
    else {
        panic!()
    };
    p.revision
}
async fn plan(
    c: &ApplicationClient,
    r: &ImportReceipt,
    p: &ProfileView,
    kind: WorkKind,
) -> BatchView {
    let revision = policy_revision(c).await;
    let ProcessingReply::Batch(b) = c
        .processing(ProcessingRequest::Plan {
            operation_id: OperationId::new(Uuid::new_v4()),
            expected_policy_revision: revision,
            records: vec![r.record_id],
            kind,
            profile_id: Some(p.value.id),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    b
}
async fn batch(c: &ApplicationClient, b: &BatchView) -> BatchView {
    let ProcessingReply::Batch(v) = c
        .processing(ProcessingRequest::Batch { id: b.id })
        .await
        .unwrap()
    else {
        panic!()
    };
    v
}
async fn execution(c: &ApplicationClient, id: ProcessingJobId) -> ExecutionReport {
    let ProcessingReply::Execution(v) = c
        .processing(ProcessingRequest::Execution { id })
        .await
        .unwrap()
    else {
        panic!()
    };
    v
}
async fn terminal(c: &ApplicationClient, b: &BatchView) -> JobView {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let j = batch(c, b).await.jobs.remove(0);
            if !matches!(j.state, JobState::Pending | JobState::Running) {
                break j;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn stop(
    c: ApplicationClient,
    task: tokio::task::JoinHandle<Result<sm_host::ShutdownOutcome, sm_host::HostError>>,
) {
    c.request_shutdown().await.unwrap();
    drop(c);
    let o = task.await.unwrap().unwrap();
    assert_eq!(o.exit_code(), 0);
    assert!(o.processing_drain_in_budget);
}
fn code(error: sm_session::ClientError) -> ErrorCode {
    match error {
        sm_session::ClientError::Application(e) => e.code,
        other => panic!("expected application refusal, got {other:?}"),
    }
}
fn manual() -> ProcessingPolicy {
    ProcessingPolicy {
        mode: ProcessingMode::Manual,
        query_loading: QueryLoadPermission::Allow,
        ..ProcessingPolicy::default()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn s04_execution_public_client_sdk_success_permissions_singleflight_priority_cancel_reopen() {
    let _serial = SDK_OWNER.lock().await;
    let f = Fixture::new().await;
    // Off library saves are independent. Both loading policies refuse before mutating SDK dispatch.
    let root = library();
    let store = Arc::new(MemoryStore::default());
    let mut s = Server::new(&f).await;
    let (c, h) = f.start(&root, &s, store.clone()).await;
    let receipt = f.save(&c).await;
    assert!(matches!(
        c.status().await.unwrap().bridge_state,
        sm_protocol::application::BridgeState::Stopped
    ));
    let canonical = fs::read(root.join(&receipt.relative_path)).unwrap();
    let p = save_profile(&c, profile(), None).await;
    let err = c
        .processing(ProcessingRequest::Plan {
            operation_id: OperationId::new(Uuid::new_v4()),
            expected_policy_revision: policy_revision(&c).await,
            records: vec![receipt.record_id],
            kind: WorkKind::TechnicalOwnerSelfTest,
            profile_id: Some(p.value.id),
        })
        .await
        .unwrap_err();
    assert_eq!(code(err), ErrorCode::AiDisabled);
    assert_eq!(s.stats().await["loads"], 0);
    policy(
        &c,
        ProcessingPolicy {
            mode: ProcessingMode::Manual,
            query_loading: QueryLoadPermission::Forbid,
            ..ProcessingPolicy::default()
        },
    )
    .await;
    let refused = plan(&c, &receipt, &p, WorkKind::TechnicalOwnerSelfTest).await;
    assert_eq!(
        terminal(&c, &refused).await.reason,
        Some(BlockReason::RuntimeRefused)
    );
    assert_eq!(
        execution(&c, refused.jobs[0].id).await.attempts[0]
            .outcome_code
            .as_deref(),
        Some("MODEL_LOAD_FORBIDDEN")
    );
    assert_eq!(s.stats().await["loads"], 0);
    let background = plan(&c, &receipt, &p, WorkKind::TechnicalOwnerBackgroundSelfTest).await;
    assert_eq!(
        terminal(&c, &background).await.reason,
        Some(BlockReason::RuntimeRefused)
    );
    assert_eq!(s.stats().await["loads"], 0);
    policy(&c, manual()).await;
    s.mode("hang").await;
    let first = plan(&c, &receipt, &p, WorkKind::TechnicalOwnerSelfTest).await;
    s.wait("predict_received").await;
    assert!(matches!(
        c.status().await.unwrap().bridge_state,
        sm_protocol::application::BridgeState::Ready
    ));
    let before = execution(&c, first.jobs[0].id).await;
    assert_eq!(before.calls.len(), 2);
    assert_eq!(before.calls[0].state, RuntimeCallState::Settled);
    assert_eq!(before.calls[0].native_ttl_requested_seconds, Some(900));
    assert_eq!(
        before.calls[0].native_ttl_verification.as_deref(),
        Some("not_verified")
    );
    assert_eq!(before.calls[1].state, RuntimeCallState::Planned);
    assert_eq!(
        before.model_leases[0].state,
        ModelConsumerLeaseState::Active
    );
    assert_eq!(
        before.retained_derivatives[0].asset_id,
        receipt.original_asset_id
    );
    assert!(!before.reconciliation_required);
    // Admission is bounded at 32, independent of session waiter ownership. Foreground gets
    // priority but four foreground attempts cannot starve a queued background consumer.
    let mut pending = Vec::new();
    pending.push(plan(&c, &receipt, &p, WorkKind::TechnicalOwnerBackgroundSelfTest).await);
    for _ in 0..30 {
        pending.push(plan(&c, &receipt, &p, WorkKind::TechnicalOwnerSelfTest).await);
    }
    let overflow = c
        .processing(ProcessingRequest::Plan {
            operation_id: OperationId::new(Uuid::new_v4()),
            expected_policy_revision: policy_revision(&c).await,
            records: vec![receipt.record_id],
            kind: WorkKind::TechnicalOwnerSelfTest,
            profile_id: Some(p.value.id),
        })
        .await
        .unwrap_err();
    assert_eq!(code(overflow), ErrorCode::RuntimeUnavailable);
    // Queue pause does not cancel the already admitted consumer; no completion is fabricated.
    let paused = policy(
        &c,
        ProcessingPolicy {
            queue_paused: true,
            ..manual()
        },
    )
    .await;
    assert_eq!(
        paused.authorization_epoch,
        first.jobs[0].authorization_epoch
    );
    s.mode("complete").await;
    s.finish().await;
    assert_eq!(terminal(&c, &first).await.state, JobState::Succeeded);
    assert!(pending.iter().all(|b| b.jobs[0].state == JobState::Pending));
    assert_eq!(s.stats().await["predictions"], 1);
    assert_eq!(s.stats().await["cancelled"], 0);
    let library_id = c.status().await.unwrap().library_id;
    drop(c);
    let c = connect(&root, library_id, None, store.clone()).await;
    policy(&c, manual()).await;
    for b in &pending {
        let j = terminal(&c, b).await;
        assert_eq!(j.state, JobState::Succeeded);
        assert!(j.probe_result.as_ref().unwrap().validated);
    }
    let ProcessingReply::Execution(all) =
        c.processing(ProcessingRequest::Reconcile {}).await.unwrap()
    else {
        panic!()
    };
    let order: Vec<_> = all
        .attempts
        .iter()
        .rev()
        .filter(|a| a.state == AttemptState::Settled && a.outcome_code.is_none())
        .map(|a| a.job_id)
        .collect();
    assert_eq!(order.len(), 32);
    assert_eq!(order[0], first.jobs[0].id);
    assert_eq!(order[4], pending[0].jobs[0].id);
    assert!(all.retained_derivatives.is_empty());
    assert!(all
        .model_leases
        .iter()
        .all(|l| l.state == ModelConsumerLeaseState::Released));
    let stats = s.stats().await;
    assert_eq!(stats["loads"], 1);
    assert_eq!(stats["predictions"], 32);
    // Inference-only changes reuse compatible loaded weights, but their own frozen
    // profile parameters reach the published SDK rather than the first consumer's sampler.
    let mut sampler = p.value.clone();
    sampler.id = ModelProfileId::new(Uuid::new_v4());
    sampler.temperature = 0.4;
    let sampler = save_profile(&c, sampler, None).await;
    let b = plan(&c, &receipt, &sampler, WorkKind::TechnicalOwnerSelfTest).await;
    assert_eq!(terminal(&c, &b).await.state, JobState::Succeeded);
    let stats = s.stats().await;
    assert_eq!(stats["loads"], 1);
    assert_eq!(stats["temperature"], 0.4);
    assert_eq!(stats["maxTokens"]["checked"], true);
    assert_eq!(stats["maxTokens"]["value"], 4000);
    // Invalid/truncated output is not successful publication or S05 knowledge.
    for mode in ["invalid_json", "truncate"] {
        s.mode(mode).await;
        let b = plan(&c, &receipt, &p, WorkKind::TechnicalOwnerSelfTest).await;
        let j = terminal(&c, &b).await;
        assert_eq!(j.state, JobState::Blocked);
        assert!(j.probe_result.is_none());
    }
    // A changed profile fences the active frozen revision; no hidden reload/second instance.
    s.mode("hang").await;
    let racing = plan(&c, &receipt, &p, WorkKind::TechnicalOwnerSelfTest).await;
    s.wait("predict_received").await;
    let mut new = p.value.clone();
    new.context_length = 4096;
    let next = save_profile(&c, new, Some(p.revision)).await;
    let raced = terminal(&c, &racing).await;
    assert_eq!(raced.state, JobState::Superseded);
    assert_eq!(raced.reason, Some(BlockReason::ProfileRetired));
    assert_eq!(raced.profile.unwrap().revision, p.revision);
    let newer = plan(&c, &receipt, &next, WorkKind::TechnicalOwnerSelfTest).await;
    assert_eq!(
        terminal(&c, &newer).await.reason,
        Some(BlockReason::RuntimeRefused)
    );
    assert_eq!(s.stats().await["loads"], 1);
    // Fresh restart never adopts the same surviving synthetic instance reference/model key.
    assert_eq!(
        fs::read(root.join(&receipt.relative_path)).unwrap(),
        canonical
    );
    stop(c, h).await;
    let (c, h) = f.start(&root, &s, store).await;
    let b = plan(&c, &receipt, &next, WorkKind::TechnicalOwnerSelfTest).await;
    assert_eq!(
        terminal(&c, &b).await.reason,
        Some(BlockReason::RuntimeRefused)
    );
    assert_eq!(
        execution(&c, b.jobs[0].id).await.attempts[0]
            .outcome_code
            .as_deref(),
        Some("INSTANCE_BUDGET_UNKNOWN")
    );
    assert_eq!(s.stats().await["loads"], 1);
    stop(c, h).await;
    s.close().await;
    remove_own(root);
    // Off cancels a real prediction. Uncertain SDK cancellation retains the operation/asset
    // bindings across host reopen and a late producer result cannot unblock the lane.
    let root = library();
    let store = Arc::new(MemoryStore::default());
    let mut s = Server::new(&f).await;
    s.mode("ignore_cancel").await;
    let config = f.config(&s, true);
    #[cfg(feature = "processing-test-seams")]
    let config = config.with_expired_uncertain_derivative();
    let (c, h) = f.start_config(&root, store.clone(), config).await;
    let r = f.save(&c).await;
    let p = save_profile(&c, profile(), None).await;
    policy(&c, manual()).await;
    let b = plan(&c, &r, &p, WorkKind::TechnicalOwnerSelfTest).await;
    s.wait("predict_received").await;
    policy(&c, ProcessingPolicy::default()).await;
    let j = terminal(&c, &b).await;
    assert_eq!(j.reason, Some(BlockReason::BackendUncertain));
    assert!(j.probe_result.is_none());
    let e = execution(&c, j.id).await;
    assert!(e.reconciliation_required);
    assert_eq!(e.retained_derivatives.len(), 1);
    assert_eq!(e.model_leases[0].state, ModelConsumerLeaseState::Uncertain);
    let held = e.retained_derivatives[0].clone();
    let stats = s.stats().await;
    assert_eq!(stats["loads"], 1);
    assert_eq!(stats["predictions"], 1);
    stop(c, h).await;
    let (c, h) = f.start(&root, &s, store).await;
    let e = execution(&c, j.id).await;
    assert!(e.reconciliation_required);
    assert_eq!(e.retained_derivatives[0].operation_id, held.operation_id);
    assert_eq!(e.retained_derivatives[0].asset_id, r.original_asset_id);
    #[cfg(feature = "processing-test-seams")]
    {
        let path = held_cache(&root, held.operation_id);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if fs::OpenOptions::new().write(true).open(&path).is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("expired durable derivative must regain its real read lease on reopen");
    }
    policy(&c, manual()).await;
    let fresh = plan(&c, &r, &p, WorkKind::TechnicalOwnerSelfTest).await;
    assert_eq!(fresh.jobs[0].reason, Some(BlockReason::BackendUncertain));
    let error = c
        .processing(ProcessingRequest::ControlJob {
            id: j.id,
            expected_revision: j.revision,
            control: JobControl::Retry,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        code(error),
        ErrorCode::RuntimeUnavailable | ErrorCode::RevisionConflict
    ));
    s.mode("complete").await;
    s.finish().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(execution(&c, j.id).await.reconciliation_required);
    assert_eq!(s.stats().await["predictions"], 1);
    assert_eq!(s.stats().await["loads"], 1);
    let saved = f.save(&c).await;
    assert_ne!(saved.record_id, r.record_id);
    assert!(root.join(saved.relative_path).is_file());
    stop(c, h).await;
    s.close().await;
    remove_own(root);
    remove_own(f.package_root);
}
#[cfg(feature = "processing-test-seams")]
fn held_cache(root: &Path, id: OperationId) -> PathBuf {
    let mut dirs = vec![root.join("cache")];
    let mut found = Vec::new();
    while let Some(d) = dirs.pop() {
        for e in fs::read_dir(d).unwrap() {
            let e = e.unwrap();
            let t = e.file_type().unwrap();
            assert!(!t.is_symlink());
            if t.is_dir() {
                dirs.push(e.path());
            } else if e
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{}.", id.as_uuid()))
            {
                found.push(e.path());
            }
        }
    }
    assert_eq!(found.len(), 1);
    found.remove(0)
}
#[cfg(feature = "processing-test-seams")]
#[tokio::test(flavor = "current_thread")]
async fn s04_execution_real_publication_transaction_rollback_reopen_no_retry() {
    let _serial = SDK_OWNER.lock().await;
    let f = Fixture::new().await;
    let root = library();
    let store = Arc::new(MemoryStore::default());
    let mut s = Server::new(&f).await;
    let config = f.config(&s, true).with_publication_constraint_rollback();
    let (c, h) = f.start_config(&root, store.clone(), config).await;
    let r = f.save(&c).await;
    let original = fs::read(root.join(&r.relative_path)).unwrap();
    let p = save_profile(&c, profile(), None).await;
    policy(&c, manual()).await;
    let b = plan(&c, &r, &p, WorkKind::TechnicalOwnerSelfTest).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let e = execution(&c, b.jobs[0].id).await;
            if e.calls.len() == 2 && e.calls[1].state == RuntimeCallState::Settled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let j = batch(&c, &b).await.jobs.remove(0);
    assert_eq!(j.state, JobState::Blocked);
    assert_eq!(j.reason, Some(BlockReason::Interrupted));
    assert!(j.probe_result.is_none());
    let e = execution(&c, j.id).await;
    assert_eq!(e.attempts[0].state, AttemptState::Interrupted);
    assert_eq!(
        e.attempts[0].outcome_code.as_deref(),
        Some("PUBLICATION_NOT_COMMITTED")
    );
    assert!(e.retained_derivatives.is_empty());
    assert_eq!(s.stats().await["predictions"], 1);
    stop(c, h).await;
    let (c, h) = f.start(&root, &s, store).await;
    let j = batch(&c, &b).await.jobs.remove(0);
    assert_eq!(j.reason, Some(BlockReason::Interrupted));
    assert!(j.probe_result.is_none());
    let e = execution(&c, j.id).await;
    assert_eq!(e.attempts[0].state, AttemptState::Interrupted);
    assert!(!e.reconciliation_required);
    assert!(e.retained_derivatives.is_empty());
    assert_eq!(s.stats().await["predictions"], 1);
    assert_eq!(fs::read(root.join(&r.relative_path)).unwrap(), original);
    stop(c, h).await;
    s.close().await;
    remove_own(root);
    remove_own(f.package_root);
}

#[tokio::test(flavor = "current_thread")]
async fn s04_execution_real_elapsed_lease_sleep_gap_fences_late_success() {
    let _serial = SDK_OWNER.lock().await;
    let f = Fixture::new().await;
    let root = library();
    let mut s = Server::new(&f).await;
    s.mode("hang").await;
    let (c, h) = f.start(&root, &s, Arc::new(MemoryStore::default())).await;
    let r = f.save(&c).await;
    let p = save_profile(&c, profile(), None).await;
    policy(&c, manual()).await;
    let b = plan(&c, &r, &p, WorkKind::TechnicalOwnerSelfTest).await;
    s.wait("predict_received").await;
    s.write("finish_after_gap").await;
    s.wait("finish_scheduled").await;
    // Suspend this isolated current-thread host reactor, not Windows or another process.
    // The owned backend/SQLite thread continue; the 60s SQLite lease expires in real time.
    std::thread::sleep(Duration::from_secs(61));
    let j = terminal(&c, &b).await;
    assert_eq!(j.reason, Some(BlockReason::LeaseExpired));
    assert!(j.probe_result.is_none());
    let e = execution(&c, j.id).await;
    assert!(e.calls.iter().all(|c| c.state == RuntimeCallState::Settled));
    assert!(e.retained_derivatives.is_empty());
    assert_eq!(s.stats().await["predictions"], 1);
    stop(c, h).await;
    s.close().await;
    remove_own(root);
    remove_own(f.package_root);
}
