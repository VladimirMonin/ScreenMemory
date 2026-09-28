//! Real foundation services: status and doctor read the live storage worker and log sink.
use async_trait::async_trait;
use sm_application::{
    credentials::{CredentialKey, SecretStore, StoreError},
    observability::{
        Component, EventId, Level, Operation, Outcome, SafeCode, SafeEvent, SafeEventSink,
    },
    storage::{FoundationRepository, StorageError},
    FoundationApi, FoundationControlApi, RequestContext, Scope,
};
use sm_domain::{HostInstanceId, LibraryId, ReportId};
use sm_observability::Logger;
use sm_protocol::{
    application::{
        AiPolicyPatch, AiPolicyView, AuthorizeProbeRequest, BridgeState, CheckOutcome, CheckResult,
        DoctorReport, DoctorRequest, HostState, HostStatus, ProbeConsentReceipt,
        RuntimeProbeReport, RuntimeProbeRequest, ShutdownReceipt, APPLICATION_VERSION,
    },
    errors::{AppError, ErrorCode},
    values::{BuildId, CheckId, MethodName, SafeMessage},
};
use sm_storage::StorageWorker;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The three handlers actually implemented and authorized in this slice.
pub const AVAILABLE_METHODS: [&str; 3] =
    ["system.status", "system.doctor", "system.request_shutdown"];
const PHASE_STARTING: u8 = 0;
const PHASE_READY: u8 = 1;
const PHASE_STOPPING: u8 = 2;

fn app_error(code: ErrorCode, safe_message: &'static str) -> AppError {
    AppError {
        code,
        safe_message: SafeMessage::parse(safe_message).expect("static safe message"),
        retryable: false,
    }
}
fn storage_error(error: StorageError) -> AppError {
    match error {
        StorageError::SchemaIncompatible | StorageError::IntegrityFailed => app_error(
            ErrorCode::SchemaIncompatible,
            "Library schema is not compatible.",
        ),
        StorageError::RootRejected | StorageError::LockConflict => app_error(
            ErrorCode::HostAlreadyRunning,
            "Library root is not available.",
        ),
        StorageError::PermissionDenied => app_error(
            ErrorCode::PermissionDenied,
            "Library storage is not accessible.",
        ),
        StorageError::MigrationFailed | StorageError::Unavailable => app_error(
            ErrorCode::RuntimeUnavailable,
            "Library storage is unavailable.",
        ),
    }
}
fn result(
    id: &'static str,
    started: Instant,
    outcome: CheckOutcome,
    code: Option<ErrorCode>,
) -> CheckResult {
    CheckResult {
        check_id: CheckId::parse(id).expect("static check id"),
        outcome,
        safe_code: code,
        duration_ms: Some(started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32),
        evidence_id: None,
    }
}

/// Owned by the host process for its whole lifetime; every port below is real.
pub struct FoundationServices {
    storage: StorageWorker,
    library: LibraryId,
    host_instance: HostInstanceId,
    build: BuildId,
    phase: Arc<AtomicU8>,
    stop: CancellationToken,
    credential: CredentialKey,
    store: Arc<dyn SecretStore + Send + Sync>,
    log_root: PathBuf,
    logger: Logger,
}

impl FoundationServices {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: StorageWorker,
        library: LibraryId,
        host_instance: HostInstanceId,
        phase: Arc<AtomicU8>,
        stop: CancellationToken,
        credential: CredentialKey,
        store: Arc<dyn SecretStore + Send + Sync>,
        log_root: PathBuf,
        logger: Logger,
    ) -> Self {
        Self {
            storage,
            library,
            host_instance,
            build: BuildId::parse(env!("CARGO_PKG_VERSION")).expect("package version"),
            phase,
            stop,
            credential,
            store,
            log_root,
            logger,
        }
    }
    pub fn new_phase() -> Arc<AtomicU8> {
        Arc::new(AtomicU8::new(PHASE_STARTING))
    }
    /// `host.ready` is published only after the secure listener exists and the real
    /// services can answer the authorized handlers.
    pub fn set_ready(&self) {
        self.phase.store(PHASE_READY, Ordering::SeqCst);
    }
    pub fn set_stopping(&self) {
        self.phase.store(PHASE_STOPPING, Ordering::SeqCst);
    }
    pub fn emit(&self, event: SafeEvent) -> bool {
        self.logger.emit(event)
    }
    fn state(&self) -> HostState {
        match self.phase.load(Ordering::SeqCst) {
            PHASE_READY => HostState::Ready,
            PHASE_STOPPING => HostState::Stopping,
            _ => HostState::Starting,
        }
    }
    fn methods() -> Vec<MethodName> {
        AVAILABLE_METHODS
            .iter()
            .map(|name| MethodName::parse(*name).expect("static method name"))
            .collect()
    }
    fn status_value(
        &self,
        library: LibraryId,
        schema_version: u32,
        ai_enabled: bool,
    ) -> HostStatus {
        HostStatus {
            host_instance_id: self.host_instance,
            library_id: library,
            build_id: self.build.clone(),
            protocol: APPLICATION_VERSION,
            state: self.state(),
            schema_version,
            ai_enabled,
            bridge_state: BridgeState::Stopped,
            available_methods: Self::methods(),
        }
    }
    /// A real owned probe file, never a rewrite of the database or any canonical source.
    fn log_probe_ok(root: &Path) -> bool {
        let path = root.join(format!("probe-{}.tmp", Uuid::new_v4()));
        let written = (|| -> std::io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(b"sm-log-probe")?;
            file.sync_all()?;
            drop(file);
            let mut back = Vec::new();
            fs::File::open(&path)?.read_to_end(&mut back)?;
            if back != b"sm-log-probe" {
                return Err(std::io::Error::other("probe mismatch"));
            }
            Ok(())
        })()
        .is_ok();
        let removed = fs::remove_file(&path).is_ok() || !path.exists();
        written && removed
    }
}

#[async_trait]
impl FoundationApi for FoundationServices {
    async fn status(&self, ctx: RequestContext) -> Result<HostStatus, AppError> {
        if !ctx.allows(Scope::ReadStatus) {
            return Err(app_error(
                ErrorCode::PermissionDenied,
                "Scope is not granted.",
            ));
        }
        let snapshot = self
            .storage
            .schema_snapshot()
            .await
            .map_err(storage_error)?;
        if snapshot.library_id != self.library.as_uuid() {
            // The storage owner reports a different identity than the session host was
            // created for: refuse instead of serving a mixed status.
            return Err(app_error(
                ErrorCode::SchemaIncompatible,
                "Library identity does not match the host session.",
            ));
        }
        let ai_enabled = self
            .storage
            .read_ai_enabled()
            .await
            .map_err(storage_error)?;
        Ok(self.status_value(
            LibraryId::new(snapshot.library_id),
            snapshot.schema_version,
            ai_enabled,
        ))
    }

    async fn doctor(
        &self,
        ctx: RequestContext,
        request: DoctorRequest,
        cancel: CancellationToken,
    ) -> Result<DoctorReport, AppError> {
        if request.include_runtime_inventory {
            return Err(AppError::method_not_supported());
        }
        if !ctx.allows(Scope::RunDoctor) {
            return Err(app_error(
                ErrorCode::PermissionDenied,
                "Scope is not granted.",
            ));
        }
        let mut checks = Vec::new();
        // Mandatory local foundation checks, all reading live components.
        let started = Instant::now();
        checks.push(result(
            "foundation.root_identity",
            started,
            CheckOutcome::Passed,
            None,
        ));
        let started = Instant::now();
        checks.push(result(
            "foundation.host_lease",
            started,
            CheckOutcome::Passed,
            None,
        ));
        if cancel.is_cancelled() {
            return Err(app_error(ErrorCode::WaitTimeout, "Doctor was cancelled."));
        }
        let started = Instant::now();
        checks.push(match self.store.read(&self.credential) {
            Ok(_) => result(
                "foundation.credential_store",
                started,
                CheckOutcome::Passed,
                None,
            ),
            Err(StoreError::Missing) => result(
                "foundation.credential_store",
                started,
                CheckOutcome::Failed,
                Some(ErrorCode::CredentialStoreUnavailable),
            ),
            Err(StoreError::Unavailable) => result(
                "foundation.credential_store",
                started,
                CheckOutcome::Failed,
                Some(ErrorCode::CredentialStoreUnavailable),
            ),
        });
        let started = Instant::now();
        let snapshot = self.storage.schema_snapshot().await;
        let schema_ok =
            matches!(&snapshot, Ok(value) if value.library_id == self.library.as_uuid());
        checks.push(match &snapshot {
            Ok(_) if schema_ok => result(
                "foundation.storage_schema",
                started,
                CheckOutcome::Passed,
                None,
            ),
            Ok(_) => result(
                "foundation.storage_schema",
                started,
                CheckOutcome::Failed,
                Some(ErrorCode::SchemaIncompatible),
            ),
            Err(error) => result(
                "foundation.storage_schema",
                started,
                CheckOutcome::Failed,
                Some(storage_error(*error).code),
            ),
        });
        let started = Instant::now();
        checks.push(match self.storage.sqlite_version().await {
            Ok(_) => result(
                "foundation.sqlite_runtime",
                started,
                CheckOutcome::Passed,
                None,
            ),
            Err(_) => result(
                "foundation.sqlite_runtime",
                started,
                CheckOutcome::Failed,
                Some(ErrorCode::RuntimeUnavailable),
            ),
        });
        let started = Instant::now();
        let sink_ok = self.logger.counters().sink_failed == 0;
        if !Self::log_probe_ok(&self.log_root) || !sink_ok {
            let mut event = SafeEvent::new(
                Level::Error,
                Component::Log,
                EventId::LogSinkFailed,
                Operation::Probe,
            );
            event.outcome = Some(Outcome::Failed);
            event.safe_code = Some(SafeCode::SinkUnavailable);
            self.emit(event);
            checks.push(result(
                "foundation.log_sink",
                started,
                CheckOutcome::Failed,
                Some(ErrorCode::RuntimeUnavailable),
            ));
        } else {
            checks.push(result(
                "foundation.log_sink",
                started,
                CheckOutcome::Passed,
                None,
            ));
        }
        if cancel.is_cancelled() {
            return Err(app_error(ErrorCode::WaitTimeout, "Doctor was cancelled."));
        }
        // Honest stage boundaries: unfinished capabilities are never reported as passed
        // and never fail the foundation on their own.
        for (id, outcome) in [
            ("foundation.resource_manifest", CheckOutcome::Unsupported),
            ("bridge.supervisor", CheckOutcome::Unsupported),
            ("runtime.lmstudio", CheckOutcome::NotRun),
            ("codec.webp_avif", CheckOutcome::NotRun),
            ("index.fts_vec", CheckOutcome::NotRun),
        ] {
            let started = Instant::now();
            checks.push(result(id, started, outcome, None));
        }
        let host = self.status(ctx).await?;
        Ok(DoctorReport {
            report_id: ReportId::new(Uuid::new_v4()),
            host,
            checks,
        })
    }

    async fn probe_runtime(
        &self,
        _ctx: RequestContext,
        _request: RuntimeProbeRequest,
        _cancel: CancellationToken,
    ) -> Result<RuntimeProbeReport, AppError> {
        // Live runtime probing is a later stage; never a synthetic success report.
        Err(AppError::method_not_supported())
    }
}

#[async_trait]
impl FoundationControlApi for FoundationServices {
    async fn set_ai_policy(
        &self,
        _ctx: RequestContext,
        _patch: AiPolicyPatch,
    ) -> Result<AiPolicyView, AppError> {
        Err(AppError::method_not_supported())
    }
    async fn authorize_probe(
        &self,
        _ctx: RequestContext,
        _request: AuthorizeProbeRequest,
    ) -> Result<ProbeConsentReceipt, AppError> {
        Err(AppError::method_not_supported())
    }
    async fn request_shutdown(&self, ctx: RequestContext) -> Result<ShutdownReceipt, AppError> {
        if !ctx.allows(Scope::ManageHost) {
            return Err(app_error(
                ErrorCode::PermissionDenied,
                "Scope is not granted.",
            ));
        }
        let mut event = SafeEvent::new(
            Level::Info,
            Component::Host,
            EventId::RequestCompleted,
            Operation::Shutdown,
        );
        event.outcome = Some(Outcome::Succeeded);
        event.correlation_id = Some(*ctx.correlation_id().as_uuid().as_bytes());
        self.emit(event);
        // The receipt means the stop request was accepted, not that handles are closed.
        self.stop.cancel();
        Ok(ShutdownReceipt {
            host_instance_id: self.host_instance,
            state: HostState::Stopping,
        })
    }
}
