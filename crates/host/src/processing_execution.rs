//! One host-owned execution lane: singleflight models, short SQLite admissions, no UI ownership.
use async_trait::async_trait;
use sm_application::{RequestContext, Scope};
use sm_domain::{CorrelationId, HostInstanceId, OperationId, RequestId, RuntimeCallId};
use sm_protocol::processing::*;
use sm_runtime::{
    BridgePackage, LoadIntentStore, LoadObservation, LoadPlan, OwnedInstance,
    PreparedInferenceImage, Reasoning, RuntimeError, RuntimeSupervisor, SecretAuthority,
    Settlement, VisionRequest,
};
use sm_storage::{
    image_services::ImageServices,
    processing_execution::{AttemptOutcome, CallOutcome, CallPlan, Command, ExecutionFence, Reply},
    StorageWorker,
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicU32, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Composition-only trusted configuration. Not Deserialize or an application RPC parameter.
pub struct TrustedProcessingRuntime {
    runtime: RuntimeSupervisor,
    endpoint: String,
    authority: Arc<dyn SecretAuthority>,
    image_worker: Arc<sm_image_worker::TrustedWorker>,
    ask_load_approved: bool,
    image_side: u32,
    #[cfg(feature = "processing-test-seams")]
    publication_rollback_once: bool,
    #[cfg(feature = "processing-test-seams")]
    load_intent_rollback_once: bool,
    #[cfg(feature = "processing-test-seams")]
    expire_uncertain_derivative: bool,
}
impl TrustedProcessingRuntime {
    pub fn open(
        package_directory: &Path,
        selected_numeric_loopback_ws: String,
        authority: Arc<dyn SecretAuthority>,
        image_worker: Arc<sm_image_worker::TrustedWorker>,
        ask_load_approved: bool,
        image_side: u32,
    ) -> Result<Self, RuntimeError> {
        if ![1024, 2048].contains(&image_side) {
            return Err(runtime_error("IMAGE_SIZE_INVALID", Settlement::Settled));
        }
        let package = BridgePackage::open(package_directory)?;
        let runtime = RuntimeSupervisor::new(package, selected_numeric_loopback_ws.clone())?;
        Ok(Self {
            runtime,
            endpoint: selected_numeric_loopback_ws,
            authority,
            image_worker,
            ask_load_approved,
            image_side,
            #[cfg(feature = "processing-test-seams")]
            publication_rollback_once: false,
            #[cfg(feature = "processing-test-seams")]
            load_intent_rollback_once: false,
            #[cfg(feature = "processing-test-seams")]
            expire_uncertain_derivative: false,
        })
    }
    #[cfg(feature = "processing-test-seams")]
    pub fn with_publication_constraint_rollback(mut self) -> Self {
        self.publication_rollback_once = true;
        self
    }
    #[cfg(feature = "processing-test-seams")]
    pub fn with_load_intent_constraint_rollback(mut self) -> Self {
        self.load_intent_rollback_once = true;
        self
    }
    #[cfg(feature = "processing-test-seams")]
    pub fn with_expired_uncertain_derivative(mut self) -> Self {
        self.expire_uncertain_derivative = true;
        self
    }
    pub(crate) fn image_worker(&self) -> Arc<sm_image_worker::TrustedWorker> {
        self.image_worker.clone()
    }
}
pub(crate) struct RuntimeStatus {
    phase: AtomicU8,
    configured: bool,
}
impl RuntimeStatus {
    pub(crate) fn new(configured: bool) -> Self {
        Self {
            phase: AtomicU8::new(0),
            configured,
        }
    }
    pub(crate) fn configured(&self) -> bool {
        self.configured
    }
    pub(crate) fn bridge(&self) -> sm_protocol::application::BridgeState {
        use sm_protocol::application::BridgeState::*;
        match self.phase.load(Ordering::Acquire) {
            0 => Stopped,
            1 => Starting,
            2 => Ready,
            3 => Degraded,
            _ => Unavailable,
        }
    }
    fn set(&self, value: u8) {
        self.phase.store(value, Ordering::Release);
    }
}
fn runtime_error(code: &'static str, settlement: Settlement) -> RuntimeError {
    RuntimeError { code, settlement }
}
fn ledger_error(_: sm_storage::processing::ProcessingError) -> RuntimeError {
    runtime_error("DURABLE_ADMISSION_REFUSED", Settlement::Settled)
}
fn context(f: &ExecutionFence) -> RequestContext {
    RequestContext::from_authenticated_peer(
        RequestId::new(f.attempt().as_uuid()),
        CorrelationId::new(Uuid::new_v4()),
        f.principal(),
        vec![
            Scope::RunAiProbe,
            Scope::ReadRuntimeInventory,
            Scope::LoadModelForProbe,
            Scope::ReadImages,
            Scope::ExportImages,
            Scope::ReadProcessing,
            Scope::ManageProcessing,
        ],
    )
}

struct SqlLoadIntents {
    storage: StorageWorker,
    fence: ExecutionFence,
    endpoint: String,
    ask_load_approved: bool,
    generation: Mutex<Option<Uuid>>,
    #[cfg(feature = "processing-test-seams")]
    load_intent_rollback: bool,
}
#[async_trait]
impl LoadIntentStore for SqlLoadIntents {
    async fn commit_before_dispatch(
        &self,
        endpoint: &str,
        generation: Uuid,
        plan: &LoadPlan,
    ) -> Result<(), RuntimeError> {
        if endpoint != self.endpoint {
            return Err(runtime_error("ENDPOINT_MISMATCH", Settlement::Settled));
        }
        let p = self
            .storage
            .processing(self.fence.principal(), ProcessingRequest::Policy {})
            .await
            .map_err(ledger_error)?;
        let ProcessingReply::Policy(policy) = p else {
            return Err(runtime_error("POLICY_INVALID", Settlement::Settled));
        };
        let permitted = if self.fence.job().kind == WorkKind::TechnicalOwnerBackgroundSelfTest {
            policy.value.background_loading == LoadPermission::Allow
        } else {
            match policy.value.query_loading {
                QueryLoadPermission::Allow => true,
                QueryLoadPermission::Ask => self.ask_load_approved,
                QueryLoadPermission::Forbid => false,
            }
        };
        if !permitted {
            return Err(runtime_error("MODEL_LOAD_FORBIDDEN", Settlement::Settled));
        }
        let plan = CallPlan::Load {
            id: RuntimeCallId::new(plan.intent_id),
            generation,
            endpoint: endpoint.to_owned(),
            plan: serde_json::to_value(plan)
                .map_err(|_| runtime_error("LOAD_PLAN_INVALID", Settlement::Settled))?,
        };
        #[cfg(feature = "processing-test-seams")]
        let command = if self.load_intent_rollback {
            Command::PlanCallWithConstraintRollback(self.fence.clone(), plan)
        } else {
            Command::PlanCall(self.fence.clone(), plan)
        };
        #[cfg(not(feature = "processing-test-seams"))]
        let command = Command::PlanCall(self.fence.clone(), plan);
        self.storage
            .execution(command)
            .await
            .map_err(ledger_error)?;
        *self
            .generation
            .lock()
            .map_err(|_| runtime_error("JOURNAL_UNAVAILABLE", Settlement::Uncertain))? =
            Some(generation);
        Ok(())
    }
    async fn record_outcome(
        &self,
        intent: Uuid,
        outcome: &Result<LoadObservation, RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let observed = match outcome {
            Ok(v) => CallOutcome::Settled(
                serde_json::to_value(v)
                    .map_err(|_| runtime_error("LOAD_OUTCOME_INVALID", Settlement::Uncertain))?,
            ),
            Err(e) if e.settlement == Settlement::Uncertain => {
                CallOutcome::Uncertain { code: e.code }
            }
            Err(e) => {
                CallOutcome::Settled(serde_json::json!({"code":e.code,"settlement":"settled"}))
            }
        };
        self.storage
            .execution(Command::CallOutcome(
                self.fence.clone(),
                RuntimeCallId::new(intent),
                observed,
            ))
            .await
            .map_err(|_| runtime_error("LOAD_OUTCOME_NOT_PERSISTED", Settlement::Uncertain))?;
        Ok(())
    }
}
struct ReadyModel {
    instance: OwnedInstance,
    profile: ProfileView,
    generation: Uuid,
}
struct ModelLease {
    model: Arc<ReadyModel>,
    profile: ProfileView,
    consumers: Arc<AtomicU32>,
}
fn load_compatible(a: &ModelProfile, b: &ModelProfile) -> bool {
    a.model_key == b.model_key
        && a.context_length == b.context_length
        && a.eval_batch_size == b.eval_batch_size
        && a.flash_attention == b.flash_attention
        && a.offload_kv_cache_to_gpu == b.offload_kv_cache_to_gpu
        && a.k_cache == b.k_cache
        && a.v_cache == b.v_cache
        && a.native_ttl_seconds == b.native_ttl_seconds
}
impl Drop for ModelLease {
    fn drop(&mut self) {
        self.consumers.fetch_sub(1, Ordering::AcqRel);
    }
}
struct ModelManager {
    runtime: RuntimeSupervisor,
    endpoint: String,
    authority: Arc<dyn SecretAuthority>,
    ask_load_approved: bool,
    status: Arc<RuntimeStatus>,
    start_attempted: bool,
    started: bool,
    ready: Option<Arc<ReadyModel>>,
    consumers: Arc<AtomicU32>,
    fenced: bool,
    #[cfg(feature = "processing-test-seams")]
    load_intent_rollback_once: bool,
}
impl ModelManager {
    async fn acquire(
        &mut self,
        storage: &StorageWorker,
        f: &ExecutionFence,
        cancel: CancellationToken,
    ) -> Result<ModelLease, RuntimeError> {
        if self.fenced {
            return Err(runtime_error(
                "RECONCILIATION_REQUIRED",
                Settlement::Settled,
            ));
        }
        let profile = f
            .job()
            .profile
            .as_ref()
            .ok_or_else(|| runtime_error("PROFILE_REQUIRED", Settlement::Settled))?;
        if let Some(model) = &self.ready {
            if !load_compatible(&model.profile.value, &profile.value) {
                return Err(runtime_error(
                    "HOST_RELOAD_UNSUPPORTED",
                    Settlement::Settled,
                ));
            }
            self.runtime
                .observe(&model.instance, cancel.clone())
                .await?;
        } else {
            if !self.started {
                if self.start_attempted {
                    return Err(runtime_error(
                        "EXPLICIT_RESTART_REQUIRED",
                        Settlement::Settled,
                    ));
                }
                self.start_attempted = true;
                self.status.set(1);
                if self.runtime.start(self.authority.as_ref()).await.is_err() {
                    self.status.set(4);
                    self.fenced = true;
                    return Err(runtime_error("RUNTIME_START_REFUSED", Settlement::Settled));
                }
                self.started = true;
                self.status.set(2);
            }
            let inventory = self.runtime.inventory(&context(f), cancel.clone()).await?;
            // Inventory never creates an OwnedInstance. Unknown/foreign memory is not an eviction budget.
            if inventory["loaded"].as_array().is_none_or(|m| !m.is_empty()) {
                return Err(runtime_error(
                    "INSTANCE_BUDGET_UNKNOWN",
                    Settlement::Settled,
                ));
            }
            let exact = inventory["downloaded"].as_array().is_some_and(|m| {
                m.iter()
                    .filter(|v| v["model_key"] == profile.value.model_key && v["vision"] == true)
                    .count()
                    == 1
            });
            if !exact {
                return Err(runtime_error(
                    "EXACT_LOCAL_MODEL_REQUIRED",
                    Settlement::Settled,
                ));
            }
            let id = Uuid::new_v4();
            let cache = |t| match t {
                CacheType::F16 => sm_runtime::CacheType::F16,
                CacheType::F32 => sm_runtime::CacheType::F32,
                CacheType::Q8 => sm_runtime::CacheType::Q8_0,
                CacheType::Q4 => sm_runtime::CacheType::Q4_0,
                CacheType::Q5 => sm_runtime::CacheType::Q5_0,
            };
            let p = &profile.value;
            let plan = LoadPlan {
                intent_id: id,
                identifier: format!("sm-{id}"),
                model_key: p.model_key.clone(),
                context_length: p.context_length,
                eval_batch_size: p.eval_batch_size,
                flash_attention: p.flash_attention,
                offload_kv_cache_to_gpu: p.offload_kv_cache_to_gpu,
                k_cache_type: cache(p.k_cache),
                v_cache_type: cache(p.v_cache),
                ttl_seconds: p.native_ttl_seconds,
            };
            let store = SqlLoadIntents {
                storage: storage.clone(),
                fence: f.clone(),
                endpoint: self.endpoint.clone(),
                ask_load_approved: self.ask_load_approved,
                generation: Mutex::new(None),
                #[cfg(feature = "processing-test-seams")]
                load_intent_rollback: self.load_intent_rollback_once,
            };
            #[cfg(feature = "processing-test-seams")]
            {
                self.load_intent_rollback_once = false;
            }
            let instance = self
                .runtime
                .load_owned(&context(f), true, plan, &store, cancel)
                .await?;
            let generation = store
                .generation
                .lock()
                .map_err(|_| runtime_error("JOURNAL_UNAVAILABLE", Settlement::Uncertain))?
                .ok_or_else(|| runtime_error("LOAD_INTENT_MISSING", Settlement::Uncertain))?;
            self.ready = Some(Arc::new(ReadyModel {
                instance,
                profile: profile.clone(),
                generation,
            }));
        }
        let model = self
            .ready
            .as_ref()
            .ok_or_else(|| runtime_error("INSTANCE_MISSING", Settlement::Uncertain))?
            .clone();
        storage
            .execution(Command::ModelLease {
                fence: f.clone(),
                generation: model.generation,
                instance_reference: model.instance.observation().instance_reference.clone(),
                profile_fingerprint: profile.fingerprint.clone(),
            })
            .await
            .map_err(ledger_error)?;
        self.consumers.fetch_add(1, Ordering::AcqRel);
        Ok(ModelLease {
            model,
            profile: profile.clone(),
            consumers: self.consumers.clone(),
        })
    }
}

type InventoryRequest = (
    RequestContext,
    tokio::sync::oneshot::Sender<Result<InventoryView, RuntimeError>>,
);
pub(crate) struct ExecutionOwner {
    inventory: tokio::sync::mpsc::Sender<InventoryRequest>,
    stop: CancellationToken,
    join: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}
impl ExecutionOwner {
    pub(crate) fn start(
        storage: StorageWorker,
        host: HostInstanceId,
        config: TrustedProcessingRuntime,
        images: Arc<ImageServices>,
        status: Arc<RuntimeStatus>,
    ) -> Self {
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let wake = storage.execution_wakeup();
        let (inventory_tx, mut inventory_rx) = tokio::sync::mpsc::channel::<InventoryRequest>(8);
        let join = tokio::spawn(async move {
            let TrustedProcessingRuntime {
                runtime,
                endpoint,
                authority,
                ask_load_approved,
                image_side,
                ..
            } = config;
            let mut manager = ModelManager {
                runtime,
                endpoint,
                authority,
                ask_load_approved,
                status,
                start_attempted: false,
                started: false,
                ready: None,
                consumers: Arc::new(AtomicU32::new(0)),
                fenced: false,
                #[cfg(feature = "processing-test-seams")]
                load_intent_rollback_once: config.load_intent_rollback_once,
            };
            let mut retained = Vec::new();
            // Reopen retained canonical-derived operations without adopting any model/old publication fence.
            if let Ok(Reply::Holds(holds)) = storage.execution(Command::Holds).await {
                for hold in holds {
                    let ctx = RequestContext::from_authenticated_peer(
                        RequestId::new(Uuid::new_v4()),
                        CorrelationId::new(Uuid::new_v4()),
                        hold.principal,
                        vec![Scope::ReadImages, Scope::ExportImages],
                    );
                    match images.begin_export_read(ctx, hold.operation_id).await {
                        Ok(mut lease) => {
                            if lease.read_all().is_err() {
                                manager.fenced = true;
                            }
                            retained.push(lease);
                        }
                        Err(_) => manager.fenced = true,
                    };
                }
            }
            let mut uncertain_images = Vec::new();
            #[cfg(feature = "processing-test-seams")]
            let mut publication_rollback_once = config.publication_rollback_once;
            let mut interactive_streak = 0u32;
            loop {
                if stopping.is_cancelled() {
                    break;
                }
                if manager.fenced {
                    tokio::select! {
                        _ = stopping.cancelled() => break,
                        Some((_, answer)) = inventory_rx.recv() => { let _ = answer.send(Err(runtime_error("RECONCILIATION_REQUIRED", Settlement::Settled))); }
                    }
                    continue;
                }
                let claimed = storage
                    .execution(Command::Claim {
                        host,
                        prefer_background: interactive_streak >= 4,
                    })
                    .await;
                let f = match claimed {
                    Ok(Reply::Claimed(Some(f))) => *f,
                    Ok(Reply::Claimed(None)) => {
                        tokio::select! {
                            _ = stopping.cancelled()=>break,
                            _ = wake.notified()=>{},
                            Some((ctx, answer)) = inventory_rx.recv()=> {
                                if answer.is_closed() { continue; }
                                let result = explicit_inventory(&mut manager, &ctx).await;
                                let _ = answer.send(result);
                            }
                        }
                        continue;
                    }
                    _ => {
                        manager.fenced = true;
                        manager.status.set(3);
                        continue;
                    }
                };
                if f.job().kind == WorkKind::TechnicalOwnerBackgroundSelfTest {
                    interactive_streak = 0;
                } else {
                    interactive_streak = interactive_streak.saturating_add(1);
                }
                let cancellation = CancellationToken::new();
                let result = {
                    let execution = execute(
                        &storage,
                        &images,
                        &mut manager,
                        &f,
                        image_side,
                        cancellation.clone(),
                    );
                    tokio::pin!(execution);
                    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
                    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    let mut last = tokio::time::Instant::now();
                    loop {
                        tokio::select! { biased;
                            _=stopping.cancelled(),if !cancellation.is_cancelled()=>{cancellation.cancel();},
                            _=wake.notified()=>{if !matches!(storage.execution(Command::Guard(f.clone())).await,Ok(Reply::Guarded(true))){cancellation.cancel();}},
                            _=heartbeat.tick()=>{
                                let elapsed=last.elapsed();last=tokio::time::Instant::now();
                                if elapsed>Duration::from_secs(45)||!matches!(storage.execution(Command::Heartbeat(f.clone())).await,Ok(Reply::Guarded(true))){cancellation.cancel();}
                            },
                            result=&mut execution=>break result,
                        }
                    }
                };
                let (outcome, image, uncertain) = result;
                #[cfg(feature = "processing-test-seams")]
                let command = if publication_rollback_once
                    && matches!(outcome, AttemptOutcome::Completed(_))
                {
                    publication_rollback_once = false;
                    Command::SettleWithConstraintRollback(f.clone(), outcome)
                } else {
                    Command::Settle(f.clone(), outcome)
                };
                #[cfg(not(feature = "processing-test-seams"))]
                let command = Command::Settle(f.clone(), outcome);
                let settled = storage.execution(command).await;
                #[cfg(feature = "processing-test-seams")]
                if uncertain && config.expire_uncertain_derivative {
                    let _ = storage
                        .execution(Command::ExpireHeldDerivative(f.clone()))
                        .await;
                }
                if uncertain || settled.is_err() {
                    // Backend settlement and domain publication are distinct. A known
                    // completed call is not mislabeled as GPU uncertainty after rollback.
                    if !uncertain && settled.is_err() {
                        let _ = storage.execution(Command::InterruptKnown(f.clone())).await;
                    }
                    if let Some(image) = image {
                        uncertain_images.push(image);
                    }
                    manager.fenced = true;
                    manager.status.set(3);
                }
            }
            let _ = manager.runtime.shutdown().await;
            manager.status.set(0);
            // SQL holds persist across shutdown/Node kill. Local lease drop is not backend settlement.
            drop(uncertain_images);
            drop(retained);
        });
        Self {
            inventory: inventory_tx,
            stop,
            join: tokio::sync::Mutex::new(Some(join)),
        }
    }
    pub(crate) async fn inventory(
        &self,
        ctx: RequestContext,
    ) -> Result<InventoryView, RuntimeError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inventory
            .try_send((ctx, tx))
            .map_err(|_| runtime_error("INVENTORY_BUSY", Settlement::Settled))?;
        tokio::time::timeout(Duration::from_secs(4), rx)
            .await
            .map_err(|_| runtime_error("INVENTORY_BUSY", Settlement::Settled))?
            .map_err(|_| runtime_error("RUNTIME_UNAVAILABLE", Settlement::Settled))?
    }
    pub(crate) async fn shutdown(&self) -> bool {
        self.stop.cancel();
        let mut slot = self.join.lock().await;
        if let Some(mut task) = slot.take() {
            let in_budget = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
            match in_budget {
                Ok(v) => v.is_ok(),
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    false
                }
            }
        } else {
            true
        }
    }
}
impl Drop for ExecutionOwner {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn explicit_inventory(
    manager: &mut ModelManager,
    ctx: &RequestContext,
) -> Result<InventoryView, RuntimeError> {
    if !manager.started {
        if manager.start_attempted {
            return Err(runtime_error(
                "EXPLICIT_RESTART_REQUIRED",
                Settlement::Settled,
            ));
        }
        manager.start_attempted = true;
        manager.status.set(1);
        if manager
            .runtime
            .start(manager.authority.as_ref())
            .await
            .is_err()
        {
            manager.status.set(4);
            manager.fenced = true;
            return Err(runtime_error("RUNTIME_START_REFUSED", Settlement::Settled));
        }
        manager.started = true;
        manager.status.set(2);
    }
    let v = manager
        .runtime
        .inventory(ctx, CancellationToken::new())
        .await?;
    let view: InventoryView = serde_json::from_value(v)
        .map_err(|_| runtime_error("INVENTORY_INVALID", Settlement::Settled))?;
    if view.downloaded.len() > 512 || view.loaded.len() > 512 {
        return Err(runtime_error(
            "INVENTORY_LIMIT_EXCEEDED",
            Settlement::Settled,
        ));
    }
    Ok(view)
}
async fn execute(
    storage: &StorageWorker,
    images: &ImageServices,
    manager: &mut ModelManager,
    f: &ExecutionFence,
    image_side: u32,
    cancel: CancellationToken,
) -> (AttemptOutcome, Option<PreparedInferenceImage>, bool) {
    if !matches!(
        storage.execution(Command::Guard(f.clone())).await,
        Ok(Reply::Guarded(true))
    ) {
        return (AttemptOutcome::Cancelled, None, false);
    }
    let operation = OperationId::new(Uuid::new_v4());
    let prepared = PreparedInferenceImage::prepare(
        images,
        context(f),
        operation,
        f.job().record_id,
        f.job().asset_id,
        image_side,
    )
    .await;
    let image = match prepared {
        Ok(i) => i,
        Err(_) => {
            return (
                AttemptOutcome::Refused {
                    code: "IMAGE_PREPARATION_FAILED",
                },
                None,
                false,
            )
        }
    };
    if storage
        .execution(Command::Hold(f.clone(), operation))
        .await
        .is_err()
    {
        return (AttemptOutcome::Cancelled, None, false);
    }
    let lease = match manager.acquire(storage, f, cancel.clone()).await {
        Ok(l) => l,
        Err(e) => {
            return if e.settlement == Settlement::Uncertain {
                (
                    AttemptOutcome::Uncertain { code: e.code },
                    Some(image),
                    true,
                )
            } else {
                (AttemptOutcome::Refused { code: e.code }, Some(image), false)
            }
        }
    };
    let call = RuntimeCallId::new(Uuid::new_v4());
    if storage
        .execution(Command::PlanCall(
            f.clone(),
            CallPlan::Inference {
                id: call,
                generation: lease.model.generation,
                endpoint: manager.endpoint.clone(),
                image_operation: operation,
            },
        ))
        .await
        .is_err()
    {
        return (AttemptOutcome::Cancelled, Some(image), false);
    }
    let profile = &lease.profile.value;
    let request=VisionRequest{prompt:"Technical owner self-test only. Return a JSON object with a short title for this synthetic fixture. Do not produce library knowledge.".into(),schema:serde_json::json!({"type":"object","properties":{"title":{"type":"string","minLength":1,"maxLength":128}},"required":["title"],"additionalProperties":false}),max_tokens:profile.max_generation_tokens,temperature:profile.temperature,reasoning:Reasoning::BackendDefault};
    let response = manager
        .runtime
        .predict_structured(
            &context(f),
            true,
            &lease.model.instance,
            &image,
            request,
            cancel.clone(),
        )
        .await;
    let (outcome, call_outcome, uncertain) = match response {
        Ok(prediction) => {
            let valid = prediction.complete
                && prediction.settlement == Settlement::Settled
                && prediction.stop_reason == "eosFound"
                && prediction.value.as_ref().is_some_and(|v| {
                    v.as_object().is_some_and(|o| o.len() == 1)
                        && v["title"]
                            .as_str()
                            .is_some_and(|t| !t.is_empty() && t.chars().count() <= 128)
                });
            let result = TechnicalProbeResult {
                validated: valid,
                stop_reason: prediction.stop_reason.clone(),
                generated_tokens: prediction.usage.generated_tokens,
                reasoning_tokens: prediction.usage.reasoning_tokens,
                image_operation_id: operation,
            };
            let outcome = if cancel.is_cancelled() {
                AttemptOutcome::Cancelled
            } else if valid {
                AttemptOutcome::Completed(result.clone())
            } else {
                AttemptOutcome::Refused {
                    code: "PROBE_RESPONSE_INVALID",
                }
            };
            (
                outcome,
                CallOutcome::Settled(serde_json::to_value(result).unwrap()),
                false,
            )
        }
        Err(e) if e.settlement == Settlement::Uncertain => (
            AttemptOutcome::Uncertain { code: e.code },
            CallOutcome::Uncertain { code: e.code },
            true,
        ),
        Err(e) => (
            if cancel.is_cancelled() {
                AttemptOutcome::Cancelled
            } else {
                AttemptOutcome::Refused { code: e.code }
            },
            CallOutcome::Settled(serde_json::json!({"code":e.code,"settlement":"settled"})),
            false,
        ),
    };
    if storage
        .execution(Command::CallOutcome(f.clone(), call, call_outcome))
        .await
        .is_err()
    {
        return (
            AttemptOutcome::Uncertain {
                code: "INFERENCE_OUTCOME_NOT_PERSISTED",
            },
            Some(image),
            true,
        );
    }
    (outcome, Some(image), uncertain)
}
