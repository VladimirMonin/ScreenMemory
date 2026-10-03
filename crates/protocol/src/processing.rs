//! Durable processing controls. No endpoints, secrets, SQL or caller-supplied authority.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sm_domain::{
    AssetId, ModelProfileId, OperationId, ProcessingBatchId, ProcessingJobId, RecordId, Revision,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingMode {
    Automatic,
    Manual,
    Off,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoadPermission {
    Allow,
    Forbid,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryLoadPermission {
    Ask,
    Allow,
    Forbid,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostRetention {
    NoHostEviction,
    Idle { seconds: u32 },
    AfterBatch,
    KeepLoaded,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessingPolicy {
    pub mode: ProcessingMode,
    pub background_loading: LoadPermission,
    pub query_loading: QueryLoadPermission,
    pub queue_paused: bool,
    pub retention: HostRetention,
}
impl Default for ProcessingPolicy {
    fn default() -> Self {
        Self {
            mode: ProcessingMode::Off,
            background_loading: LoadPermission::Forbid,
            query_loading: QueryLoadPermission::Ask,
            queue_paused: false,
            retention: HostRetention::NoHostEviction,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyView {
    pub revision: Revision,
    pub authorization_epoch: Revision,
    pub value: ProcessingPolicy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CacheType {
    F16,
    F32,
    Q8,
    Q4,
    Q5,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    BackendDefault,
    Off,
    Budget100,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    pub id: ModelProfileId,
    #[schemars(length(min = 1, max = 512))]
    pub model_key: String,
    #[schemars(range(min = 1024, max = 262144))]
    pub context_length: u32,
    #[schemars(range(min = 1, max = 2048))]
    pub eval_batch_size: u32,
    pub flash_attention: bool,
    pub offload_kv_cache_to_gpu: bool,
    pub k_cache: CacheType,
    pub v_cache: CacheType,
    pub native_ttl_seconds: Option<u32>,
    #[schemars(range(min = 1, max = 4000))]
    pub max_generation_tokens: u32,
    #[schemars(range(min = 0, max = 1))]
    pub temperature: f64,
    pub reasoning: ReasoningMode,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileView {
    pub revision: Revision,
    pub fingerprint: String,
    pub value: ModelProfile,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    Analyze,
    TechnicalOwnerSelfTest,
    TechnicalOwnerBackgroundSelfTest,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Pending,
    Running,
    Blocked,
    Failed,
    Cancelled,
    Superseded,
    Succeeded,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    HandlerUnavailable,
    RuntimeUnconfigured,
    AiDisabled,
    Interrupted,
    BackendUncertain,
    CancelRequested,
    ProfileRetired,
    RuntimeRefused,
    LeaseExpired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BatchControl {
    Pause,
    Resume,
    Cancel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobControl {
    Cancel,
    Retry,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobView {
    pub id: ProcessingJobId,
    pub record_id: RecordId,
    pub asset_id: AssetId,
    pub asset_sha256: String,
    pub kind: WorkKind,
    pub revision: Revision,
    pub state: JobState,
    pub reason: Option<BlockReason>,
    pub policy_revision: Revision,
    pub authorization_epoch: Revision,
    pub profile: Option<ProfileView>,
    pub attempt_count: u32,
    #[serde(default)]
    pub probe_result: Option<TechnicalProbeResult>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchView {
    pub id: ProcessingBatchId,
    pub revision: Revision,
    pub control: BatchControl,
    pub jobs: Vec<JobView>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessingRequest {
    Policy {},
    Overview {
        before: Option<ProcessingJobId>,
    },
    /// Explicit owner inventory only; may start the private bridge, never loads a model.
    Inventory {},
    UpdatePolicy {
        operation_id: OperationId,
        expected_revision: Revision,
        value: ProcessingPolicy,
    },
    SaveProfile {
        operation_id: OperationId,
        expected_revision: Revision,
        value: ModelProfile,
    },
    Profile {
        id: ModelProfileId,
    },
    Plan {
        operation_id: OperationId,
        expected_policy_revision: Revision,
        #[schemars(length(min = 1, max = 512))]
        records: Vec<RecordId>,
        kind: WorkKind,
        profile_id: Option<ModelProfileId>,
    },
    Batch {
        id: ProcessingBatchId,
    },
    ControlBatch {
        id: ProcessingBatchId,
        expected_revision: Revision,
        control: BatchControl,
    },
    ControlJob {
        id: ProcessingJobId,
        expected_revision: Revision,
        control: JobControl,
    },
    Execution {
        id: ProcessingJobId,
    },
    /// Read-only durable reconciliation; never authorizes replay/retry or adopts an instance.
    Reconcile {},
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ProcessingReply {
    Policy(PolicyView),
    Overview(Box<ProcessingOverview>),
    Inventory(InventoryView),
    Profile(ProfileView),
    Batch(BatchView),
    Job(JobView),
    Execution(ExecutionReport),
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessingOverview {
    pub policy: PolicyView,
    #[schemars(length(max = 32))]
    pub profiles: Vec<ProfileView>,
    #[schemars(length(max = 32))]
    pub jobs: Vec<QueuedJob>,
    pub next_before: Option<ProcessingJobId>,
    pub runtime_configured: bool,
    pub bridge_state: crate::application::BridgeState,
    pub model_autostart: bool,
    pub analyze_handler_available: bool,
    pub physical_unload_supported: bool,
    pub supported_reasoning: Vec<ReasoningMode>,
    pub supported_retention: Vec<HostRetention>,
    pub native_ttl_verification: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueuedJob {
    pub batch_id: ProcessingBatchId,
    pub batch_revision: Revision,
    pub job: JobView,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InventoryView {
    #[schemars(length(max = 512))]
    pub downloaded: Vec<InventoryModel>,
    #[schemars(length(max = 512))]
    pub loaded: Vec<InventoryInstance>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InventoryModel {
    pub model_key: String,
    pub vision: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InventoryInstance {
    pub model_key: String,
    pub identifier: String,
    pub instance_reference: String,
    pub owned: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TechnicalProbeResult {
    pub validated: bool,
    pub stop_reason: String,
    pub generated_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub image_operation_id: OperationId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Claimed,
    Dispatched,
    Settled,
    Uncertain,
    Interrupted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCallState {
    Planned,
    Settled,
    Uncertain,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttemptView {
    pub id: sm_domain::ProcessingAttemptId,
    pub job_id: ProcessingJobId,
    pub state: AttemptState,
    pub host_id: sm_domain::HostInstanceId,
    pub authorization_epoch: Revision,
    pub outcome_code: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCallKind {
    Load,
    Inference,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCallView {
    #[serde(default)]
    pub load_requested: Option<LoadConfigurationView>,
    #[serde(default)]
    pub load_reported_applied: Option<LoadConfigurationView>,
    #[serde(default)]
    pub load_configuration: Option<LoadConfigurationStatus>,
    pub id: sm_domain::RuntimeCallId,
    pub attempt_id: sm_domain::ProcessingAttemptId,
    pub state: RuntimeCallState,
    pub kind: RuntimeCallKind,
    pub generation: String,
    pub native_ttl_requested_seconds: Option<u32>,
    pub native_ttl_verification: Option<String>,
    pub outcome_code: Option<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoadConfigurationStatus {
    ReportedApplied,
    Mismatch,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadConfigurationView {
    pub context_length: Option<u32>,
    pub eval_batch_size: Option<u32>,
    pub flash_attention: Option<bool>,
    pub offload_kv_cache_to_gpu: Option<bool>,
    pub k_cache_type: Option<String>,
    pub v_cache_type: Option<String>,
    pub max_parallel_predictions: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivativeHoldView {
    pub attempt_id: sm_domain::ProcessingAttemptId,
    pub operation_id: OperationId,
    pub asset_id: AssetId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelConsumerLeaseState {
    Active,
    Released,
    Abandoned,
    Uncertain,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelConsumerLeaseView {
    pub id: sm_domain::ModelConsumerLeaseId,
    pub attempt_id: sm_domain::ProcessingAttemptId,
    pub state: ModelConsumerLeaseState,
    pub generation: String,
    pub profile_fingerprint: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReport {
    pub attempts: Vec<AttemptView>,
    pub calls: Vec<RuntimeCallView>,
    pub retained_derivatives: Vec<DerivativeHoldView>,
    pub model_leases: Vec<ModelConsumerLeaseView>,
    pub reconciliation_required: bool,
}
