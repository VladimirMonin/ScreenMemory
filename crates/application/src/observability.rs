//! Operational diagnostics: closed vocabulary only. No text from requests or exceptions.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Host,
    Protocol,
    Storage,
    Bridge,
    Probe,
    Codec,
    Sqlite,
    Policy,
    Log,
    /// S02: capture sessions and the canonical ingest of an accepted frame.
    Capture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Startup,
    Shutdown,
    Request,
    Migration,
    BridgeStartup,
    Probe,
    Rotation,
    Progress,
    /// S02: one capture session from the authorized intent to the committed receipt.
    Capture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeCode {
    PermissionDenied,
    ProtocolMismatch,
    LockConflict,
    SinkUnavailable,
    QueueFull,
    ProbeBlocked,
    InternalFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventId {
    HostStarted,
    HostReady,
    HostFailed,
    HostStopped,
    HostLockConflict,
    ProtocolDenied,
    ProtocolMismatch,
    RequestCompleted,
    MigrationChecked,
    MigrationApplied,
    MigrationFailed,
    BridgeStartRequested,
    BridgeReady,
    BridgeExited,
    BridgeRestartBlocked,
    ProbeStarted,
    ProbeCompleted,
    ProbeBlocked,
    CodecProbeCompleted,
    SqliteProbeCompleted,
    PermissionDenied,
    LogEventsDropped,
    LogRotationCompleted,
    LogSinkFailed,
    ProbeProgress,
    /// S02: a capture session was authorized and started.
    CaptureStarted,
    /// S02: an accepted frame was committed as a canonical record.
    CaptureCompleted,
    /// S02: the session or the canonical commit ended in a typed failure.
    CaptureFailed,
    /// S02: a typed refusal before the freeze (busy, backpressure, lost target).
    CaptureRefused,
}

impl EventId {
    pub fn is_progress(self) -> bool {
        matches!(self, Self::ProbeProgress)
    }
}

/// No free-form text, serde flatten, arbitrary values, or Debug implementation for payloads.
/// UUID bytes are opaque correlation identities; never derive these from source paths.
#[derive(Clone, Copy, Serialize)]
pub struct SafeEvent {
    pub schema_version: u8,
    pub timestamp_unix_ms: u64,
    pub level: Level,
    pub component: Component,
    pub event: EventId,
    pub operation: Operation,
    pub correlation_id: Option<[u8; 16]>,
    pub duration_ms: Option<u64>,
    pub outcome: Option<Outcome>,
    pub safe_code: Option<SafeCode>,
    pub count: Option<u32>,
}

impl SafeEvent {
    pub fn new(level: Level, component: Component, event: EventId, operation: Operation) -> Self {
        Self {
            schema_version: 1,
            timestamp_unix_ms: 0,
            level,
            component,
            event,
            operation,
            correlation_id: None,
            duration_ms: None,
            outcome: None,
            safe_code: None,
            count: None,
        }
    }
}

/// Only accepted events are queued. false means dropped, not an exception payload.
pub trait SafeEventSink: Send + Sync {
    fn emit(&self, event: SafeEvent) -> bool;
}
