//! Typed diagnostic payloads of the S01 window (S01 §10.1/§10.3).
//!
//! The window never receives a free-form string: every handler answers with this
//! envelope. `host`/`doctor` are the *same* `sm-protocol` DTOs the CLI prints, so the
//! normalized fields of `shot status --json` and of the window are literally equal.
//! Nothing here invents data: a capability that does not exist in this slice is listed
//! in `not_implemented` instead of showing an empty ("0 images") result.
use serde::{Deserialize, Serialize};
use sm_cli::ClientFailure;
use sm_domain::{AssetId, OperationId, RecordId};
use sm_protocol::{
    application::{BridgeState, CancelReason, CaptureOutcome, DoctorReport, HostStatus},
    errors::ErrorCode,
};

/// Version of this envelope only; the application protocol version stays in `host`.
pub const GUI_ENVELOPE_VERSION: u32 = 1;

/// Capabilities still absent; the window marks them as not implemented. `capture` is no
/// longer listed: S02 wires a real button through the same host command as `shot`.
pub const NOT_IMPLEMENTED: [&str; 6] = [
    "library_counts",
    "gallery",
    "search",
    "drop_zone",
    "model_settings",
    "bridge_runtime",
];

/// Capture modes the window may request; the vocabulary is the wire one.
pub const CAPTURE_MODES: [&str; 4] = ["region", "window", "monitor", "desktop"];

/// Terminal state of one capture answer, as the window shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiCaptureState {
    Saved,
    Cancelled,
    Refused,
}

/// One capture answer. It carries identities and geometry only: never pixels, never a
/// window handle and never an absolute path.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiCaptureAnswer {
    pub schema_version: u32,
    pub state: GuiCaptureState,
    pub operation_id: Option<OperationId>,
    pub record_id: Option<RecordId>,
    pub asset_id: Option<AssetId>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub byte_size: Option<String>,
    pub relative_path: Option<String>,
    pub reason: Option<CancelReason>,
    pub safe_code: Option<ErrorCode>,
    pub retryable: bool,
    pub warnings: Vec<String>,
}

impl GuiCaptureAnswer {
    fn envelope(state: GuiCaptureState) -> Self {
        Self {
            schema_version: GUI_ENVELOPE_VERSION,
            state,
            operation_id: None,
            record_id: None,
            asset_id: None,
            width: None,
            height: None,
            byte_size: None,
            relative_path: None,
            reason: None,
            safe_code: None,
            retryable: false,
            warnings: Vec::new(),
        }
    }

    /// A committed capture of this window session.
    pub fn from_outcome(outcome: &CaptureOutcome) -> Self {
        match outcome {
            CaptureOutcome::Saved { receipt } => Self {
                state: GuiCaptureState::Saved,
                operation_id: Some(receipt.operation_id),
                record_id: Some(receipt.record_id),
                asset_id: Some(receipt.asset_id),
                width: Some(receipt.width),
                height: Some(receipt.height),
                byte_size: Some(receipt.byte_size.clone()),
                relative_path: Some(receipt.relative_path.clone()),
                warnings: receipt.warnings.clone(),
                ..Self::envelope(GuiCaptureState::Saved)
            },
            CaptureOutcome::Cancelled {
                operation_id,
                reason,
            } => Self {
                operation_id: Some(*operation_id),
                reason: Some(*reason),
                ..Self::envelope(GuiCaptureState::Cancelled)
            },
        }
    }

    /// A typed refusal: the same stable codes the CLI reports.
    pub fn refused(failure: &ClientFailure) -> Self {
        Self {
            safe_code: Some(failure.code),
            retryable: failure.retryable,
            ..Self::envelope(GuiCaptureState::Refused)
        }
    }
}

/// One of the six states of S01 §10.1 plus `connecting`, which bounds one attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiState {
    /// No host answered and starting one is either not allowed or was not requested.
    HostNotRunning,
    /// One bounded attempt is in flight; no model call happens here.
    Connecting,
    /// The shared client answered with real IDs/versions of a running host.
    Ready,
    /// Foundation is up, but the configured AI runtime is not usable.
    RuntimeUnavailable,
    /// The host does not speak the expected application protocol.
    ProtocolMismatch,
    /// Root/identity/credential refusal with a stable safe code.
    RootCredentialError,
}

/// Bounded windows the window may show; sourced from the shared client constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiLimits {
    pub connect_deadline_ms: u32,
    pub retry_interval_ms: u32,
}
impl Default for GuiLimits {
    fn default() -> Self {
        Self {
            connect_deadline_ms: sm_cli::START_DEADLINE.as_millis() as u32,
            retry_interval_ms: sm_cli::RETRY_INTERVAL.as_millis() as u32,
        }
    }
}

/// Answer of one diagnostic command. Exactly one JSON value; no path, frame or secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiDiagnostics {
    pub schema_version: u32,
    pub state: GuiState,
    pub host: Option<HostStatus>,
    pub doctor: Option<DoctorReport>,
    pub runtime_available: bool,
    pub safe_code: Option<ErrorCode>,
    pub retryable: bool,
    pub bounded_ms: u32,
    pub report_available: bool,
    pub not_implemented: Vec<String>,
}

impl GuiDiagnostics {
    fn envelope(state: GuiState) -> Self {
        Self {
            schema_version: GUI_ENVELOPE_VERSION,
            state,
            host: None,
            doctor: None,
            runtime_available: false,
            safe_code: None,
            retryable: false,
            bounded_ms: GuiLimits::default().connect_deadline_ms,
            report_available: false,
            not_implemented: NOT_IMPLEMENTED
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        }
    }

    /// The bounded attempt the window is waiting for. A caller that never receives an
    /// answer still has to leave this state within `bounded_ms`.
    pub fn connecting() -> Self {
        Self::envelope(GuiState::Connecting)
    }

    /// A refused diagnostic command: a stable safe code plus the typed state, never a
    /// message from the wire, a path or an OS string.
    pub fn refused(state: GuiState, failure: &ClientFailure, report_available: bool) -> Self {
        let mut value = Self::envelope(state);
        value.safe_code = Some(failure.code);
        value.retryable = failure.retryable;
        value.report_available = report_available;
        value
    }

    /// A real status answer of the running host.
    pub fn from_status(host: HostStatus, report_available: bool) -> Self {
        let runtime_available = runtime_available(&host);
        let state = state_for_status(&host);
        let mut value = Self::envelope(state);
        value.runtime_available = runtime_available;
        value.report_available = report_available;
        value.host = Some(host);
        value
    }

    /// A real doctor report of the running host; the status DTO is carried by it.
    pub fn with_doctor(host: HostStatus, report: DoctorReport, report_available: bool) -> Self {
        let runtime_available = runtime_available(&host);
        let state = state_for_status(&host);
        let mut value = Self::envelope(state);
        value.runtime_available = runtime_available;
        value.report_available = report_available;
        value.host = Some(host);
        value.doctor = Some(report);
        value
    }
}

/// The AI runtime is usable only when the library policy enables it *and* the bridge
/// reports ready. An intentionally disabled policy is not a runtime failure.
pub fn runtime_available(host: &HostStatus) -> bool {
    host.ai_enabled && matches!(host.bridge_state, BridgeState::Ready)
}

/// `RuntimeUnavailable` is entered only when the policy asks for AI but the runtime is
/// not usable; a disabled policy stays a working foundation (`Ready`).
pub fn state_for_status(host: &HostStatus) -> GuiState {
    if host.ai_enabled && !runtime_available(host) {
        GuiState::RuntimeUnavailable
    } else {
        GuiState::Ready
    }
}

/// Total mapping of every client/protocol code onto one of the documented states.
pub fn state_for_failure(failure: &ClientFailure) -> GuiState {
    match failure.code {
        ErrorCode::ProtocolMismatch
        | ErrorCode::SchemaIncompatible
        | ErrorCode::MethodNotSupported
        | ErrorCode::InvalidRequest
        | ErrorCode::FrameTooLarge
        | ErrorCode::ProtocolTruncatedFrame => GuiState::ProtocolMismatch,
        ErrorCode::PermissionDenied
        | ErrorCode::CredentialStoreUnavailable
        | ErrorCode::LibraryMismatch => GuiState::RootCredentialError,
        ErrorCode::AiDisabled
        | ErrorCode::ModelNotInstalled
        | ErrorCode::ModelConfigUnsupported
        | ErrorCode::BridgeUnavailable => GuiState::RuntimeUnavailable,
        ErrorCode::RuntimeUnavailable | ErrorCode::WaitTimeout | ErrorCode::HostAlreadyRunning => {
            GuiState::HostNotRunning
        }
        // The S02 capture and asset vocabulary is reported by the capture answer itself;
        // it never means that the host, its root or its credential became unusable.
        ErrorCode::CaptureBusy
        | ErrorCode::CaptureBackpressure
        | ErrorCode::CaptureTargetLost
        | ErrorCode::CaptureTopologyChanged
        | ErrorCode::CaptureFocusUnavailable
        | ErrorCode::CaptureUnavailable
        | ErrorCode::CaptureGdiFailed
        | ErrorCode::ImageLimitExceeded
        | ErrorCode::CanonicalSizeUnsupported
        | ErrorCode::ImageInputInvalid
        | ErrorCode::CodecFailed
        | ErrorCode::AssetIntegrityFailed
        | ErrorCode::AssetNotFound
        | ErrorCode::StorageFull => GuiState::Ready,
    }
}

/// Answer of the "open the diagnostic report" action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenReportOutcome {
    pub opened: bool,
    pub safe_code: Option<ErrorCode>,
}
