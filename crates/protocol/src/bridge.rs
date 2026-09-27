//! Private bridge wire contract; not the application named-pipe protocol.
use crate::{
    application::{CheckResult, ProbeFixture, ProtocolVersion},
    values::ProfileId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sm_domain::RequestId;

pub const BRIDGE_VERSION: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BridgeProbeRequest {
    pub protocol: ProtocolVersion,
    pub request_id: RequestId,
    pub fixture: ProbeFixture,
    pub profile_id: ProfileId,
    pub allow_model_load: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BridgeErrorCode {
    Unavailable,
    Unsupported,
    ModelNotInstalled,
    ModelConfigUnsupported,
    WaitTimeout,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BridgeError {
    pub code: BridgeErrorCode,
    pub retryable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeResponse {
    Result {
        request_id: RequestId,
        checks: Vec<CheckResult>,
    },
    Error {
        request_id: RequestId,
        error: BridgeError,
    },
}
