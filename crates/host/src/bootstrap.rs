//! S01 §6.3 bootstrap order and §6.5 shutdown order for the single host owner.
use crate::args::BootstrapMode;
use crate::capture::{drain_hotkeys, CaptureIngest, CommandTasks, HostCaptureApi};
use crate::services::FoundationServices;
use serde::Serialize;
use sm_application::{
    capture::{CaptureApi, CaptureError, NativeCaptureAdapter},
    credentials::{
        Authenticator, CredentialEnvironment, CredentialError, CredentialKey, Secret, SecretStore,
        StoreError,
    },
    observability::{
        Component, EventId, Level, Operation, Outcome, SafeCode, SafeEvent, SafeEventSink,
    },
    storage::{FoundationRepository, StorageError},
};
use sm_capture::{CaptureHostConfig, HotkeySink, WindowsCaptureAdapter};
use sm_domain::{HostInstanceId, LibraryId, PrincipalId};
use sm_framing::BufferBudget;
use sm_isolation::{HostLease, LeaseError, LibraryRootResolver, RootError, RootSource};
use sm_observability::{
    BoundedLogger, ConsoleSink, FileSink, FlushOutcome, LogSink, Logger, RotationLimits,
    SystemClock, DEFAULT_CAPACITY,
};
use sm_pipe::{Endpoint, Listener};
use sm_session::SessionHost;
use sm_storage::StorageWorker;
use std::{
    path::Path,
    path::PathBuf,
    sync::{atomic::AtomicU8, Arc, OnceLock},
    time::Duration,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Control grace budget from S01 §6.5 for the whole foundation shutdown.
pub const SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);
/// Bounded window for already admitted connections to deliver an in-flight authorized
/// receipt before their session tasks are aborted.
pub const DEFAULT_DRAIN: Duration = Duration::from_millis(750);
const LOG_FLUSH_BUDGET: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostError {
    Root(RootError),
    LeaseBusy,
    LeaseUnsafe,
    Storage(StorageError),
    CredentialUnavailable,
    CredentialDenied,
    Endpoint,
    Logging,
    Compose,
    Session,
    /// S02: the trusted native capture owner could not be started.
    Capture(CaptureError),
}
impl HostError {
    /// Fixed host exit code; never a path, OS string or secret in the process status.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Root(_) | Self::LeaseBusy | Self::LeaseUnsafe => crate::EXIT_HOST,
            Self::Storage(StorageError::LockConflict) => crate::EXIT_HOST,
            Self::Capture(
                CaptureError::Busy | CaptureError::Backpressure | CaptureError::Unavailable,
            ) => crate::EXIT_STORAGE,
            Self::Capture(_) | Self::Storage(_) | Self::Logging => crate::EXIT_STORAGE,
            Self::CredentialUnavailable | Self::CredentialDenied => crate::EXIT_POLICY,
            Self::Endpoint | Self::Compose | Self::Session => crate::EXIT_HOST,
        }
    }
    /// Closed safe code for the stderr/startup event; no free-form text.
    pub fn safe_code(self) -> SafeCode {
        match self {
            Self::Root(RootError::Inaccessible) => SafeCode::PermissionDenied,
            Self::Root(_) | Self::LeaseUnsafe | Self::Endpoint | Self::Compose => {
                SafeCode::ProbeBlocked
            }
            Self::LeaseBusy => SafeCode::LockConflict,
            Self::CredentialUnavailable | Self::CredentialDenied => SafeCode::PermissionDenied,
            Self::Logging => SafeCode::SinkUnavailable,
            Self::Capture(_) | Self::Storage(_) | Self::Session => SafeCode::InternalFailure,
        }
    }
    pub fn event_id(self) -> EventId {
        match self {
            Self::LeaseBusy => EventId::HostLockConflict,
            _ => EventId::HostFailed,
        }
    }
}

/// S02 capture wiring of one composition. The native owner is supplied by the caller so
/// that a composition test can inject a deterministic adapter without a live desktop.
#[derive(Default)]
pub enum CaptureOptions {
    /// No capture handler exists; the capture methods are then not advertised either.
    #[default]
    Disabled,
    /// Start the real Windows owner. The host installs its own bounded hotkey sink.
    Native(Box<CaptureHostConfig>),
    /// Adapter supplied by the caller (composition/integration tests).
    Injected(Arc<dyn NativeCaptureAdapter>),
    /// Trusted composition-only seam for synthetic hotkey delivery. Not a wire option;
    /// the production binary constructs only `Native` or `Disabled`.
    InjectedHotkey {
        adapter: Arc<dyn NativeCaptureAdapter>,
        sink: Arc<OnceLock<Arc<dyn HotkeySink>>>,
    },
}

/// Explicit configuration of one owned root plus the host credential store of this process.
pub struct BootstrapOptions {
    pub root: PathBuf,
    pub mode: BootstrapMode,
    pub store: Arc<dyn SecretStore + Send + Sync>,
    /// Capture wiring of this composition.
    pub capture: CaptureOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapReport {
    pub library_id: LibraryId,
    pub schema_version: u32,
    pub format_major: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownOutcome {
    /// SQLite handles closed, pinned entries released and the lease dropped.
    pub storage_closed: bool,
    pub log_flushed: bool,
    pub serve_ok: bool,
    /// False when a registered command or hotkey save outlived shutdown grace.
    pub command_drain_in_budget: bool,
    /// False if the hotkey consumer outlived grace or unexpectedly stopped.
    pub hotkey_drain_in_budget: bool,
}
impl ShutdownOutcome {
    pub fn exit_code(self) -> u8 {
        if self.storage_closed
            && self.serve_ok
            && self.command_drain_in_budget
            && self.hotkey_drain_in_budget
        {
            crate::EXIT_OK
        } else {
            crate::EXIT_STORAGE
        }
    }
}

/// Adapter so a type-erased host credential store can drive the typed enrollment API.
struct DynStore<'a>(&'a (dyn SecretStore + Send + Sync));
impl SecretStore for DynStore<'_> {
    fn read(&self, key: &CredentialKey) -> Result<Secret, StoreError> {
        self.0.read(key)
    }
    fn create_new(&self, key: &CredentialKey, value: &Secret) -> Result<(), StoreError> {
        self.0.create_new(key, value)
    }
}

/// Resolved and locked resources of an owned root. Blocking work only: no async here.
struct Prepared {
    storage: StorageWorker,
    endpoint: Endpoint,
    cache_root: PathBuf,
}
fn prepare(root: &Path) -> Result<Prepared, HostError> {
    // No donor roots are declared for an explicit owned test/development root.
    let donor: [&Path; 0] = [];
    let layout = LibraryRootResolver::resolve(RootSource::Test {
        root,
        forbidden_donor_roots: &donor,
    })
    .map_err(HostError::Root)?;
    let cache_root = layout.cache.canonical_path().to_path_buf();
    // This slice serves only the explicit test/development namespace. A product or dev
    // root would use a different credential namespace and is refused, never silently
    // mapped onto the synthetic Test policy.
    if layout.environment != sm_isolation::Environment::Test {
        return Err(HostError::Root(RootError::Unsupported));
    }
    // Endpoint identity is derived from the pinned open root before the lease consumes it.
    let endpoint = Endpoint::for_layout(&layout).map_err(|_| HostError::Endpoint)?;
    let lease = HostLease::acquire(layout).map_err(|error| match error {
        LeaseError::Busy => HostError::LeaseBusy,
        LeaseError::UnsafeEntry => HostError::LeaseUnsafe,
    })?;
    // The lease is consumed here and stays owned by the SQLite worker until it closes.
    let storage = StorageWorker::start(lease).map_err(HostError::Storage)?;
    Ok(Prepared {
        storage,
        endpoint,
        cache_root,
    })
}

fn emit_startup_failure(mut logger: BoundedLogger, error: HostError) -> HostError {
    let mut event = SafeEvent::new(
        Level::Error,
        Component::Host,
        error.event_id(),
        Operation::Startup,
    );
    event.outcome = Some(Outcome::Failed);
    event.safe_code = Some(error.safe_code());
    logger.producer().emit(event);
    let _ = logger.shutdown(LOG_FLUSH_BUDGET);
    error
}

/// Compose the foundation in the fixed §6.3 order. The blocking root/lease/SQLite-worker
/// handshake runs on a dedicated blocking thread; the runtime thread only awaits typed
/// storage responses.
pub async fn bootstrap(options: BootstrapOptions) -> Result<BootstrappedHost, HostError> {
    let mode = options.mode;
    let root = options.root.clone();
    let store = options.store.clone();
    let capture = options.capture;
    let prepared = tokio::task::spawn_blocking(move || prepare(&root))
        .await
        .map_err(|_| HostError::Compose)??;
    let snapshot = prepared
        .storage
        .schema_snapshot()
        .await
        .map_err(HostError::Storage)?;
    if snapshot.library_id.is_nil() {
        return Err(HostError::Storage(StorageError::IntegrityFailed));
    }
    let library = LibraryId::new(snapshot.library_id);
    // Step 5a: credential policy first. The owner credential is enrolled/reopened explicitly
    // (Test namespace only) while the lease is held and *before* the bounded logger starts:
    // S01 §6.3 step 5 orders credential/session policy, then structured logs and minimal
    // services. A refused enrollment therefore never creates a file logger; the process-level
    // caller reports the bounded typed pre-root event on stderr with the fixed exit code.
    let key = CredentialKey::owner(CredentialEnvironment::Test, library);
    let owner = match Authenticator::enroll_owner(&DynStore(store.as_ref()), key.clone()) {
        Ok(authenticator) => authenticator,
        Err(error) => {
            return Err(match error {
                CredentialError::PermissionDenied => HostError::CredentialDenied,
                _ => HostError::CredentialUnavailable,
            });
        }
    };
    // Step 5b: structured logs and minimal services.
    let mut logger = BoundedLogger::start(
        FileSink::new(prepared.cache_root.clone(), RotationLimits::default())
            .map_err(|_| HostError::Logging)?,
        Arc::new(SystemClock::default()),
        DEFAULT_CAPACITY,
    )
    .map_err(|_| HostError::Logging)?;
    let mut migration = SafeEvent::new(
        Level::Info,
        Component::Storage,
        EventId::MigrationChecked,
        Operation::Migration,
    );
    migration.outcome = Some(Outcome::Succeeded);
    logger.producer().emit(migration);
    let host_instance = HostInstanceId::new(Uuid::new_v4());
    let stop = CancellationToken::new();
    let phase = FoundationServices::new_phase();
    // S02 step: the capture owner exists before the listener, so a readiness claim is
    // never made by a host whose advertised capture handler could not be constructed.
    // Only the explicitly enrolled owner credential identifies the trusted native hotkey
    // path; client requests keep their authenticated session principal.
    let owner_principal = owner.principal();
    let command_tasks = CommandTasks::new();
    let (capture, hotkey_task) = match start_capture(
        capture,
        prepared.storage.clone(),
        logger.producer(),
        owner_principal,
        library,
        command_tasks.clone(),
    )
    .await
    {
        Ok(pair) => pair,
        Err(error) => {
            // A capture owner that cannot start is a startup failure, never a host that
            // advertises a handler it does not own.
            let mut failed = SafeEvent::new(
                Level::Error,
                Component::Host,
                EventId::HostFailed,
                Operation::Startup,
            );
            failed.outcome = Some(Outcome::Failed);
            failed.safe_code = Some(SafeCode::InternalFailure);
            logger.producer().emit(failed);
            let _ = logger.shutdown(LOG_FLUSH_BUDGET);
            return Err(error);
        }
    };
    let services = Arc::new(FoundationServices::new(
        prepared.storage.clone(),
        library,
        host_instance,
        phase.clone(),
        stop.clone(),
        key,
        store,
        prepared.cache_root.clone(),
        logger.producer(),
        capture.clone(),
    ));
    let hotkey_task = hotkey_task;
    let session = Arc::new(SessionHost::new(
        library,
        host_instance,
        owner,
        None,
        services.clone(),
        services.clone(),
        capture.clone(),
        BufferBudget::new(),
    ));
    // Step 6: the sealed listener exists before any readiness claim; connections are
    // only accepted once `serve` runs the bounded loop.
    let listener = match mode {
        BootstrapMode::Initialize => None,
        BootstrapMode::Serve => match Listener::bind(prepared.endpoint) {
            Ok(listener) => Some(listener),
            Err(_) => {
                services.set_stopping();
                command_tasks.close();
                let _ = close_hotkeys(hotkey_task).await;
                command_tasks.drain().await;
                // No detached consumer or lease may survive a failed bind.
                let _ = prepared.storage.shutdown(SHUTDOWN_BUDGET).await;
                return Err(emit_startup_failure(logger, HostError::Endpoint));
            }
        },
    };
    let mut started = SafeEvent::new(
        Level::Info,
        Component::Host,
        EventId::HostStarted,
        Operation::Startup,
    );
    started.outcome = Some(Outcome::Succeeded);
    logger.producer().emit(started);
    if mode == BootstrapMode::Serve {
        // §6.3 step 6: the listener exists, so readiness is published and connections may
        // be served. Delivery of this typed event is bounded/best-effort; readiness itself
        // is a functional state, never a log-delivery claim.
        services.set_ready();
        let mut ready = SafeEvent::new(
            Level::Info,
            Component::Host,
            EventId::HostReady,
            Operation::Startup,
        );
        ready.outcome = Some(Outcome::Succeeded);
        logger.producer().emit(ready);
    }
    Ok(BootstrappedHost {
        library_id: library,
        host_instance_id: host_instance,
        schema_version: snapshot.schema_version,
        format_major: snapshot.format_major,
        stop,
        session,
        listener,
        storage: prepared.storage,
        logger,
        services,
        hotkey_task,
        command_tasks,
    })
}

/// Explicit close signal is required because the native owner keeps a sender alive.
struct HotkeyConsumer {
    close: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

async fn close_hotkeys(task: Option<HotkeyConsumer>) -> bool {
    match task {
        Some(task) => {
            let _ = task.close.send(());
            task.join.await.is_ok()
        }
        None => true,
    }
}

/// Bring up the S02 capture owner and its bounded hotkey consumer.
async fn start_capture(
    options: CaptureOptions,
    storage: StorageWorker,
    logger: Logger,
    owner_principal: PrincipalId,
    library_id: LibraryId,
    command_tasks: Arc<CommandTasks>,
) -> Result<(Option<Arc<dyn CaptureApi>>, Option<HotkeyConsumer>), HostError> {
    let slots = Arc::new(AtomicU8::new(0));
    let (adapter, hotkeys) = match options {
        CaptureOptions::Disabled => return Ok((None, None)),
        CaptureOptions::Injected(adapter) => (adapter, None),
        CaptureOptions::InjectedHotkey { adapter, sink } => {
            let (hotkey_sink, receiver) =
                crate::capture::HotkeyPersistence::channel(slots.clone(), command_tasks.clone());
            sink.set(hotkey_sink).map_err(|_| HostError::Compose)?;
            (adapter, Some(receiver))
        }
        CaptureOptions::Native(mut config) => {
            // The host owns the bounded sink of the hotkey path; the native thread only
            // enqueues a finished session.
            let (sink, receiver) =
                crate::capture::HotkeyPersistence::channel(slots.clone(), command_tasks.clone());
            config.hotkey_sink = Some(sink);
            let started =
                tokio::task::spawn_blocking(move || WindowsCaptureAdapter::start(*config))
                    .await
                    .map_err(|_| HostError::Compose)?
                    .map_err(HostError::Capture)?;
            (
                Arc::new(started) as Arc<dyn NativeCaptureAdapter>,
                Some(receiver),
            )
        }
    };
    let ingest = CaptureIngest::new(
        adapter,
        storage,
        logger,
        owner_principal,
        library_id,
        command_tasks,
        slots,
    );
    let api: Arc<dyn CaptureApi> = HostCaptureApi::new(ingest.clone());
    let task = hotkeys.map(|receiver| {
        let (close, closed) = oneshot::channel();
        HotkeyConsumer {
            close,
            join: tokio::spawn(drain_hotkeys(ingest, receiver, closed)),
        }
    });
    Ok((Some(api), task))
}

/// One live owner of an owned root: exclusive lease, one SQLite writer, one listener.
pub struct BootstrappedHost {
    library_id: LibraryId,
    host_instance_id: HostInstanceId,
    schema_version: u32,
    format_major: u32,
    stop: CancellationToken,
    session: Arc<SessionHost>,
    listener: Option<Listener>,
    storage: StorageWorker,
    logger: BoundedLogger,
    services: Arc<FoundationServices>,
    hotkey_task: Option<HotkeyConsumer>,
    command_tasks: Arc<CommandTasks>,
}
impl BootstrappedHost {
    pub fn library_id(&self) -> LibraryId {
        self.library_id
    }
    pub fn host_instance_id(&self) -> HostInstanceId {
        self.host_instance_id
    }
    pub fn bootstrap_report(&self) -> BootstrapReport {
        BootstrapReport {
            library_id: self.library_id,
            schema_version: self.schema_version,
            format_major: self.format_major,
        }
    }
    pub fn stop_token(&self) -> CancellationToken {
        self.stop.clone()
    }
    /// Publish `host.ready` and serve authorized requests until an owner stops the host.
    pub async fn serve(mut self, drain: Duration) -> Result<ShutdownOutcome, HostError> {
        let listener = self.listener.as_mut().ok_or(HostError::Compose)?;
        // Readiness was published at the end of bootstrap; no request can reach a handler
        // before this loop runs.
        let served =
            sm_session::run_draining(listener, self.session.clone(), self.stop.clone(), drain)
                .await;
        Ok(self.finish(served.is_ok()).await)
    }
    /// Clean shutdown of an initialize-only composition; no listener was bound.
    pub async fn shutdown(self) -> ShutdownOutcome {
        self.finish(true).await
    }
    /// Stop admission first, then close the writer/listener chain, confirm SQLite closure
    /// (which releases the lease), and flush the bounded logger last.
    async fn finish(self, serve_ok: bool) -> ShutdownOutcome {
        let BootstrappedHost {
            listener,
            storage,
            mut logger,
            services,
            hotkey_task,
            command_tasks,
            ..
        } = self;
        services.set_stopping();
        command_tasks.close();
        drop(listener);
        // Closing the receiver is explicit: the native adapter retains its sink sender.
        // Its in-flight owned permits remain deliverable after close.
        let hotkey_join = hotkey_task.map(|consumer| {
            let _ = consumer.close.send(());
            consumer.join
        });
        let deadline = std::time::Instant::now() + SHUTDOWN_BUDGET;
        // A timeout is not permission to close the DB beneath an accepted save.
        // Keep this finish future (and the lease) alive until all admitted command tasks
        // resolve; the process cannot report a successful stop after the grace elapsed.
        let drained_in_budget = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            command_tasks.drain(),
        )
        .await
        .is_ok();
        if !drained_in_budget {
            command_tasks.drain().await;
        }
        let hotkey_drain_in_budget = if let Some(mut join) = hotkey_join {
            match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), &mut join).await
            {
                Ok(result) => result.is_ok(),
                Err(_) => {
                    // Never abort or release the lease before a queued crop is persisted.
                    let _ = join.await;
                    false
                }
            }
        } else {
            true
        };
        let storage_closed = storage.shutdown(SHUTDOWN_BUDGET).await.is_ok();
        let sink_failed = logger.counters().sink_failed;
        let mut stopped = SafeEvent::new(
            Level::Info,
            Component::Host,
            EventId::HostStopped,
            Operation::Shutdown,
        );
        stopped.outcome = Some(
            if storage_closed
                && drained_in_budget
                && hotkey_drain_in_budget
                && serve_ok
                && sink_failed == 0
            {
                Outcome::Succeeded
            } else {
                Outcome::Unknown
            },
        );
        if !storage_closed {
            stopped.safe_code = Some(SafeCode::InternalFailure);
        }
        logger.producer().emit(stopped);
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let log_flushed = matches!(
            logger.shutdown(remaining.min(LOG_FLUSH_BUDGET)),
            FlushOutcome::Flushed
        );
        ShutdownOutcome {
            storage_closed,
            log_flushed,
            serve_ok,
            command_drain_in_budget: drained_in_budget,
            hotkey_drain_in_budget,
        }
    }
}

/// Startup failure diagnostics before any owned root exists: one bounded safe event.
pub fn emit_pre_root_failure(error: HostError) -> bool {
    let mut sink = ConsoleSink::new(std::io::stderr());
    let mut event = SafeEvent::new(
        Level::Error,
        Component::Host,
        error.event_id(),
        Operation::Startup,
    );
    event.outcome = Some(Outcome::Failed);
    event.safe_code = Some(error.safe_code());
    event.timestamp_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    sink.write_event(&event).is_ok() && sink.flush().is_ok()
}
