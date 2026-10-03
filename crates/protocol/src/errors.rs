use crate::values::SafeMessage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sm_domain::RequestId;

/// Fixed public registry. Never expose provider exception strings in `safe_message`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    MethodNotSupported,
    InvalidRequest,
    FrameTooLarge,
    ProtocolTruncatedFrame,
    PermissionDenied,
    LibraryMismatch,
    ProtocolMismatch,
    HostAlreadyRunning,
    SchemaIncompatible,
    CredentialStoreUnavailable,
    RuntimeUnavailable,
    ModelNotInstalled,
    AiDisabled,
    ModelConfigUnsupported,
    BridgeUnavailable,
    WaitTimeout,
    RevisionConflict,
    IdempotencyConflict,
    CategoryNotAvailable,
    CategoryNameConflict,
    CategoryCatalogTooLarge,
    /// S02 §5.6: capture vocabulary. Every value is produced by the real capture/library
    /// path; none of them is a placeholder for an unimplemented capability.
    CaptureBusy,
    CaptureBackpressure,
    CaptureTargetLost,
    CaptureTopologyChanged,
    CaptureFocusUnavailable,
    CaptureUnavailable,
    CaptureGdiFailed,
    ImageLimitExceeded,
    CanonicalSizeUnsupported,
    AssetIntegrityFailed,
    AssetNotFound,
    StorageFull,
    /// S02: the bundled codec refused to encode or decode the accepted image.
    CodecFailed,
    /// S02: the pixel buffer handed to the codec was not a usable image.
    ImageInputInvalid,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppError {
    pub code: ErrorCode,
    pub safe_message: SafeMessage,
    pub retryable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ErrorWithRequest {
    pub request_id: RequestId,
    #[serde(flatten)]
    pub error: AppError,
}
impl AppError {
    pub fn method_not_supported() -> Self {
        Self {
            code: ErrorCode::MethodNotSupported,
            safe_message: SafeMessage::parse("Method is not supported.")
                .expect("static safe message"),
            retryable: false,
        }
    }
}
