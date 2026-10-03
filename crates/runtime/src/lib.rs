//! Host-only SDK owning seam. No application RPC, model discovery, downloads or automatic retries.
mod supervisor;
#[cfg(windows)]
mod windows;
pub use supervisor::{private_node_command, BridgePackage, RuntimeSupervisor};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sm_application::{images::ImageLibraryApi, RequestContext, Scope};
use sm_domain::{AssetId, OperationId, RecordId};
use sm_protocol::images::{ExportFormat, ExportMetadataPolicy, ExportRequest};
use sm_storage::image_services::{ExportReadLease, ImageServices};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Settlement {
    Settled,
    Uncertain,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: &'static str,
    pub settlement: Settlement,
}
impl RuntimeError {
    pub(crate) fn settled(code: &'static str) -> Self {
        Self {
            code,
            settlement: Settlement::Settled,
        }
    }
    pub(crate) fn uncertain(code: &'static str) -> Self {
        Self {
            code,
            settlement: Settlement::Uncertain,
        }
    }
}
#[async_trait]
pub trait SecretAuthority: Send + Sync {
    /// Empty token is explicit unauthenticated LM Studio policy, never environment inheritance.
    async fn lm_studio_token(&self) -> Result<Zeroizing<String>, RuntimeError>;
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadPlan {
    pub intent_id: uuid::Uuid,
    pub identifier: String,
    pub model_key: String,
    pub context_length: u32,
    pub eval_batch_size: u32,
    pub flash_attention: bool,
    pub offload_kv_cache_to_gpu: bool,
    pub k_cache_type: CacheType,
    pub v_cache_type: CacheType,
    pub ttl_seconds: Option<u32>,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum CacheType {
    F16,
    F32,
    Q8_0,
    Q4_0,
    Q4_1,
    Q5_0,
    Q5_1,
    #[serde(rename = "IQ4_NL")]
    Iq4Nl,
}
#[async_trait]
pub trait LoadIntentStore: Send + Sync {
    /// MUST commit exact plan/endpoint/generation before returning. Refuse unresolved earlier calls;
    /// this is the durable queue owner seam, NOT a flag accepted from application wire.
    async fn commit_before_dispatch(
        &self,
        endpoint: &str,
        generation: uuid::Uuid,
        plan: &LoadPlan,
    ) -> Result<(), RuntimeError>;
    /// Persist observed identity/config or uncertainty. Failure fences further dispatch.
    async fn record_outcome(
        &self,
        intent: uuid::Uuid,
        outcome: &Result<LoadObservation, RuntimeError>,
    ) -> Result<(), RuntimeError>;
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInventory {
    pub downloaded: Vec<DownloadedModel>,
    pub loaded: Vec<LoadedModel>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadedModel {
    pub model_key: String,
    pub vision: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadedModel {
    pub model_key: String,
    pub identifier: String,
    pub instance_reference: String,
    pub owned: bool,
}

#[async_trait]
pub trait ModelLifecycle: Send {
    async fn inventory(
        &mut self,
        ctx: &RequestContext,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<RuntimeInventory, RuntimeError>;
    async fn load_owned(
        &mut self,
        ctx: &RequestContext,
        ai_enabled: bool,
        plan: LoadPlan,
        store: &dyn LoadIntentStore,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<OwnedInstance, RuntimeError>;
    fn unload_owned(&mut self, instance: &OwnedInstance) -> Result<(), RuntimeError>;
}
#[async_trait]
pub trait VisionInference: Send {
    async fn predict_structured(
        &mut self,
        ctx: &RequestContext,
        ai_enabled: bool,
        instance: &OwnedInstance,
        image: &PreparedInferenceImage,
        request: VisionRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Prediction, RuntimeError>;
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadObservation {
    pub instance_reference: String,
    pub identifier: String,
    pub model_key: String,
    pub intent_id: uuid::Uuid,
    pub requested: serde_json::Value,
    pub applied: serde_json::Value,
    pub configuration: String,
    /// Explicit requested server policy, not verified physical eviction or host lifecycle.
    pub native_ttl: NativeTtlObservation,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTtlObservation {
    pub requested_seconds: Option<u32>,
    pub verification: String,
}
/// Not Deserialize. Only a successful explicit owned load creates this generation-bound capability.
pub struct OwnedInstance {
    pub(crate) observation: LoadObservation,
    pub(crate) generation: uuid::Uuid,
}
impl OwnedInstance {
    pub fn observation(&self) -> &LoadObservation {
        &self.observation
    }
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reasoning {
    BackendDefault,
    Off,
    Budget100,
}
pub struct VisionRequest {
    pub prompt: String,
    pub schema: serde_json::Value,
    pub max_tokens: u32,
    pub temperature: f64,
    pub reasoning: Reasoning,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prediction {
    pub instance_reference: String,
    pub settlement: Settlement,
    pub complete: bool,
    pub value: Option<serde_json::Value>,
    pub stop_reason: String,
    pub usage: Usage,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub generated_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}
/// Native preparation is the ONLY constructor: a committed specific canonical asset via S03 export.
/// Keep this lease after Uncertain until durable reconciliation. No caller-supplied paths or bytes.
pub struct PreparedInferenceImage {
    pub(crate) bytes: Zeroizing<Vec<u8>>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) max_side: u32,
    pub(crate) hash: String,
    _lease: ExportReadLease,
    operation_id: OperationId,
    asset_id: AssetId,
}
impl PreparedInferenceImage {
    pub fn operation_id(&self) -> OperationId {
        self.operation_id
    }
    pub fn asset_id(&self) -> AssetId {
        self.asset_id
    }
    pub async fn prepare(
        images: &ImageServices,
        ctx: RequestContext,
        operation_id: OperationId,
        record_id: RecordId,
        asset_id: AssetId,
        max_side: u32,
    ) -> Result<Self, RuntimeError> {
        if ![1024, 2048].contains(&max_side) {
            return Err(RuntimeError::settled("IMAGE_SIZE_INVALID"));
        }
        let receipt = images
            .export_image(
                ctx.clone(),
                ExportRequest {
                    operation_id,
                    record_id,
                    asset_id: Some(asset_id),
                    format: ExportFormat::Png,
                    max_side: Some(max_side),
                    jpeg_background: None,
                    destination_path: None,
                    metadata_policy: ExportMetadataPolicy::StripPrivate,
                },
            )
            .await
            .map_err(|_| RuntimeError::settled("IMAGE_PREPARATION_FAILED"))?;
        let mut lease = images
            .begin_export_read(ctx, operation_id)
            .await
            .map_err(|_| RuntimeError::settled("IMAGE_LEASE_FAILED"))?;
        let bytes = Zeroizing::new(
            lease
                .read_all()
                .map_err(|_| RuntimeError::settled("IMAGE_INTEGRITY_FAILED"))?,
        );
        if receipt.source_asset_id != asset_id
            || bytes.len() > 20 * 1024 * 1024
            || bytes.len() < 24
            || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            return Err(RuntimeError::settled("IMAGE_INVALID"));
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        if width == 0 || height == 0 || width.max(height) > max_side {
            return Err(RuntimeError::settled("IMAGE_INVALID"));
        }
        Ok(Self {
            bytes,
            width,
            height,
            max_side,
            hash: receipt.sha256,
            _lease: lease,
            operation_id,
            asset_id,
        })
    }
}
pub(crate) fn require(ctx: &RequestContext, scope: Scope) -> Result<(), RuntimeError> {
    if ctx.allows(scope) {
        Ok(())
    } else {
        Err(RuntimeError::settled("ACCESS_DENIED"))
    }
}
