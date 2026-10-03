//! S03 shared typed service contract. Paths are explicit source/destination roles.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sm_domain::{AssetId, ImportBatchId, ImportItemId, OperationId, OriginId, RecordId, Revision};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordMetadataRequest {
    pub record_id: RecordId,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordMetadata {
    pub record_id: RecordId,
    pub source_kind: String,
    pub source_reports: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalProfile {
    WebpExact,
    AvifSdr90,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DuplicatePolicy {
    ReuseRecord,
    KeepBoth,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub batch_id: ImportBatchId,
    pub source_paths: Vec<String>,
    pub profile: CanonicalProfile,
    pub duplicates: DuplicatePolicy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportControl {
    Pause,
    CancelProcessing,
    Resume,
    Retry,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportBatchRequest {
    pub batch_id: ImportBatchId,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportReportPageRequest {
    pub batch_id: ImportBatchId,
    pub after_ordinal: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportControlRequest {
    pub batch_id: ImportBatchId,
    pub control: ImportControl,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportState {
    Discovered,
    Processing,
    Held,
    Recoverable,
    SavedNew,
    LinkedExisting,
    Skipped,
    CancelledUnaccepted,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportReceipt {
    pub operation_id: OperationId,
    pub record_id: RecordId,
    pub original_asset_id: AssetId,
    pub current_asset_id: AssetId,
    pub origin_id: OriginId,
    pub canonical_mime: String,
    pub canonical_sha256: String,
    pub relative_path: String,
    pub width: u32,
    pub height: u32,
    pub requested_profile_used: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportItemReport {
    pub item_id: ImportItemId,
    pub ordinal: u32,
    pub state: ImportState,
    pub durable_input: bool,
    pub receipt: Option<ImportReceipt>,
    pub safe_code: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportBatchState {
    Running,
    Pausing,
    Paused,
    Cancelling,
    Cancelled,
    NeedsAttention,
    Completed,
    CompletedWithErrors,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportReport {
    pub batch_id: ImportBatchId,
    pub revision: Revision,
    pub control: String,
    pub state: ImportBatchState,
    pub active_work: bool,
    pub discovery_complete: bool,
    pub discovery_abandoned: bool,
    pub discovery_limit: u32,
    pub discovery_limit_hit: bool,
    pub directory_issues: u32,
    pub discovered: u32,
    pub saved_new: u32,
    pub linked_existing: u32,
    pub skipped: u32,
    pub failed: u32,
    pub held: u32,
    pub cancelled_unaccepted: u32,
    pub in_flight: u32,
    pub retained_input: u32,
    pub items: Vec<ImportItemReport>,
    pub next_ordinal: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Png,
    Jpeg,
    Webp,
    Avif,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportMetadataPolicy {
    StripPrivate,
    IncludeSourceSidecar,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub operation_id: OperationId,
    pub record_id: RecordId,
    /// None resolves Current exactly once. Replay never resolves again.
    pub asset_id: Option<AssetId>,
    pub format: ExportFormat,
    pub max_side: Option<u32>,
    pub jpeg_background: Option<[u8; 3]>,
    /// None selects the host-managed export registry (TTL 24h), never a wire cache path.
    pub destination_path: Option<String>,
    pub metadata_policy: ExportMetadataPolicy,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportReceipt {
    pub operation_id: OperationId,
    pub source_asset_id: AssetId,
    pub export_path: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub byte_size: String,
    pub sha256: String,
    pub expires_at_epoch_seconds: Option<String>,
    pub sidecar: Option<ExportSidecar>,
    pub partial: bool,
    pub existence: ExportExistence,
    pub encoding_profile_fingerprint: String,
    pub applied_parameters: ExportAppliedParameters,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportAppliedParameters {
    pub format: ExportFormat,
    pub max_side: Option<u32>,
    pub jpeg_background: Option<[u8; 3]>,
    pub quality: Option<u8>,
    pub resize_applied: bool,
    pub private_metadata_stripped: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportExistence {
    Verified,
    NotChecked,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportSidecar {
    pub export_path: String,
    pub sha256: String,
    pub byte_size: String,
}
