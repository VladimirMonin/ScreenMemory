//! Typed diagnostic payloads of the S01 window (S01 §10.1/§10.3).
//!
//! The window never receives a free-form string: every handler answers with this
//! envelope. `host`/`doctor` are the *same* `sm-protocol` DTOs the CLI prints, so the
//! normalized fields of `shot status --json` and of the window are literally equal.
//! Nothing here invents data: a capability that does not exist in this slice is listed
//! in `not_implemented` instead of showing an empty ("0 images") result.
use serde::{Deserialize, Serialize};
use sm_cli::ClientFailure;
use sm_protocol::{
    application::{BridgeState, DoctorReport, HostStatus},
    errors::ErrorCode,
};

/// Version of this envelope only; the application protocol version stays in `host`.
pub const GUI_ENVELOPE_VERSION: u32 = 1;

/// Capabilities deliberately absent from S01; the window marks them as not implemented.
pub const NOT_IMPLEMENTED: [&str; 7] = [
    "capture",
    "library_counts",
    "gallery",
    "search",
    "drop_zone",
    "model_settings",
    "bridge_runtime",
];

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
    }
}

/// Answer of the "open the diagnostic report" action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenReportOutcome {
    pub opened: bool,
    pub safe_code: Option<ErrorCode>,
}
