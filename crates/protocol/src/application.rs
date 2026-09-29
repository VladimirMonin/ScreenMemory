use crate::{
    errors::{AppError, ErrorCode},
    values::{BuildId, CheckId, ConsentLifetime, MethodName, ProfileId, UtcTime},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sm_domain::{
    AssetId, ConsentId, EvidenceId, HostInstanceId, LibraryId, OperationId, RecordId, ReportId,
    RequestId, Revision,
};

pub const APPLICATION_VERSION: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}
impl ProtocolVersion {
    pub fn compatible(self, peer: Self) -> bool {
        self.major == peer.major && self.minor == peer.minor
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostState {
    Starting,
    Ready,
    Degraded,
    Stopping,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckOutcome {
    Passed,
    Failed,
    Blocked,
    NotRun,
    Unsupported,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BridgeState {
    Stopped,
    Starting,
    Ready,
    Degraded,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProbeFixture {
    SyntheticMenuV1,
    EmbeddingTextV1,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    pub check_id: CheckId,
    pub outcome: CheckOutcome,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "present"
    )]
    #[schemars(with = "ErrorCode")]
    pub safe_code: Option<ErrorCode>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "present"
    )]
    #[schemars(with = "u32")]
    pub duration_ms: Option<u32>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "present"
    )]
    #[schemars(with = "EvidenceId")]
    pub evidence_id: Option<EvidenceId>,
}
fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostStatus {
    pub host_instance_id: HostInstanceId,
    pub library_id: LibraryId,
    pub build_id: BuildId,
    pub protocol: ProtocolVersion,
    pub state: HostState,
    pub schema_version: u32,
    pub ai_enabled: bool,
    pub bridge_state: BridgeState,
    pub available_methods: Vec<MethodName>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorRequest {
    pub include_runtime_inventory: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorReport {
    pub report_id: ReportId,
    pub host: HostStatus,
    pub checks: Vec<CheckResult>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProbeRequest {
    pub fixture: ProbeFixture,
    pub profile_id: ProfileId,
    pub allow_model_load: bool,
    pub consent_id: ConsentId,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProbeReport {
    pub probe_id: ReportId,
    pub checks: Vec<CheckResult>,
    pub compatibility_evidence_id: EvidenceId,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AiPolicyPatch {
    pub expected_revision: Revision,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AiPolicyView {
    pub revision: Revision,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorizeProbeRequest {
    pub fixture: ProbeFixture,
    pub profile_id: ProfileId,
    pub allow_load: bool,
    pub lifetime_seconds: ConsentLifetime,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProbeConsentReceipt {
    pub consent_id: ConsentId,
    pub expires_at: UtcTime,
    pub policy_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShutdownReceipt {
    pub host_instance_id: HostInstanceId,
    pub state: HostState,
}

/// S02 §3.1: closed vocabulary of one capture target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Region,
    Window,
    Monitor,
    Desktop,
}

impl CaptureMode {
    /// Closed name used in diagnostics; never a client-supplied free-form value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Region => "region",
            Self::Window => "window",
            Self::Monitor => "monitor",
            Self::Desktop => "desktop",
        }
    }
}

/// S02 §5.2: parameters of `capture.command`.
///
/// The initiator is deliberately absent: a client cannot declare itself a native hotkey.
/// The trusted host adapter records the trigger out of band.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureCommandParams {
    pub operation_id: OperationId,
    pub mode: CaptureMode,
}

/// S02 §5.2: why a capture ended without an accepted frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    Escape,
    EmptySelection,
    ExplicitRequest,
    ClientDisconnected,
}

/// S02 §5.2/§11.5: receipt of one committed capture. No pixels and no absolute path.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureReceipt {
    pub operation_id: OperationId,
    pub record_id: RecordId,
    pub asset_id: AssetId,
    pub canonical_mime: String,
    pub canonical_sha256: String,
    pub width: u32,
    pub height: u32,
    /// Decimal string: a byte count must not round through a JavaScript number.
    pub byte_size: String,
    /// Library-relative canonical path (`assets/...`), never an absolute path.
    pub relative_path: String,
    pub warnings: Vec<String>,
}

/// S02 §5.2: terminal outcome of one `capture.command`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CaptureOutcome {
    Saved {
        receipt: CaptureReceipt,
    },
    Cancelled {
        operation_id: OperationId,
        reason: CancelReason,
    },
}

/// S02 §10.1: bounded read of one committed canonical asset.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryAssetRequest {
    pub asset_id: AssetId,
    pub offset: u32,
}

/// S02 §10.1: one bounded chunk of a canonical asset. `data_hex` keeps the frame a
/// printable JSON document without a base64 dependency, and `sha256` is the whole-file
/// digest so a truncated or substituted file is refused instead of exported.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryAssetChunk {
    pub asset_id: AssetId,
    pub offset: u32,
    pub next_offset: u32,
    pub eof: bool,
    /// Decimal string of the whole canonical file size.
    pub total_size: String,
    pub sha256: String,
    pub mime: String,
    #[schemars(regex(pattern = "^([0-9a-f]{2})*$"))]
    pub data_hex: String,
}

/// S02 §10.3: one bounded page of committed library records.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryRecordsRequest {
    pub limit: u16,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "present"
    )]
    pub cursor: Option<String>,
}

/// S02 §10.3: one entry in a library records page; asset paths are library-relative.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryRecordEntry {
    pub id: RecordId,
    pub created_at: String,
    pub title: String,
    pub target: String,
    pub width: u32,
    pub height: u32,
    pub byte_size: String,
    pub preview_asset_id: Option<AssetId>,
}

/// S02 §10.3: one bounded page of committed records; `next_cursor` is opaque.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryRecordsPage {
    pub items: Vec<LibraryRecordEntry>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "present"
    )]
    pub next_cursor: Option<String>,
}

/// Decoded after the authorized handshake; `params` never carries a trusted context.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    pub protocol: ProtocolVersion,
    pub request_id: RequestId,
    pub library_id: LibraryId,
    pub method: MethodName,
    pub timeout_ms: u32,
    pub params: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum KnownParams {
    #[serde(rename = "system.status")]
    Status(EmptyParams),
    #[serde(rename = "system.doctor")]
    Doctor(DoctorRequest),
    #[serde(rename = "runtime.probe")]
    ProbeRuntime(RuntimeProbeRequest),
    #[serde(rename = "system.set_ai_policy")]
    SetAiPolicy(AiPolicyPatch),
    #[serde(rename = "system.authorize_probe")]
    AuthorizeProbe(AuthorizeProbeRequest),
    #[serde(rename = "system.request_shutdown")]
    RequestShutdown(EmptyParams),
    #[serde(rename = "capture.command")]
    CaptureCommand(CaptureCommandParams),
    #[serde(rename = "library.asset.read")]
    ReadAsset(LibraryAssetRequest),
    #[serde(rename = "library.list_records")]
    ListRecords(LibraryRecordsRequest),
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyParams {}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ApplicationResult {
    Status(HostStatus),
    Doctor(DoctorReport),
    ProbeRuntime(RuntimeProbeReport),
    AiPolicy(AiPolicyView),
    ProbeConsent(ProbeConsentReceipt),
    Shutdown(ShutdownReceipt),
    Capture(CaptureOutcome),
    AssetChunk(LibraryAssetChunk),
    RecordsPage(LibraryRecordsPage),
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Result {
        request_id: RequestId,
        result: ApplicationResult,
    },
    Error {
        request_id: RequestId,
        error: AppError,
    },
}

/// A strict parameter decoder: unknown methods are rejected before any handler is invoked.
/// Authentication/scopes/consent and replay detection belong to the future host boundary.
pub fn dispatch_parsed<F>(request: RequestEnvelope, library: LibraryId, handler: F) -> Response
where
    F: FnOnce(KnownParams) -> ApplicationResult,
{
    let fail = |code: ErrorCode| Response::Error {
        request_id: request.request_id,
        error: AppError {
            code,
            safe_message: crate::values::SafeMessage::parse("Request rejected.")
                .expect("static template"),
            retryable: false,
        },
    };
    if !APPLICATION_VERSION.compatible(request.protocol) {
        return fail(ErrorCode::ProtocolMismatch);
    }
    if request.library_id != library {
        return fail(ErrorCode::LibraryMismatch);
    }
    let method = request.method.as_str();
    if !matches!(
        method,
        "system.status"
            | "system.doctor"
            | "runtime.probe"
            | "system.set_ai_policy"
            | "system.authorize_probe"
            | "system.request_shutdown"
            | "capture.command"
            | "library.asset.read"
            | "library.list_records"
    ) {
        return Response::Error {
            request_id: request.request_id,
            error: AppError::method_not_supported(),
        };
    }
    let pair = serde_json::json!({"method": method, "params": request.params});
    match serde_json::from_value::<KnownParams>(pair) {
        Ok(params) => Response::Result {
            request_id: request.request_id,
            result: handler(params),
        },
        Err(_) => fail(ErrorCode::InvalidRequest),
    }
}
