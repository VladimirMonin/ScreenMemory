//! Authenticated application RPC over the OS-verified sm-pipe endpoint.
//! Host bootstrap owns lease, library metadata, enrollment, services and readiness.
mod client;
pub use client::{ApplicationClient, ClientError, CredentialSource};
use serde::{Deserialize, Serialize};
use sm_application::{
    capture::CaptureApi,
    credentials::{Authenticator, CredentialKey, Secret, SecretStore},
    images::ImageLibraryApi,
    FoundationApi, FoundationControlApi, RequestContext, Scope,
};
use sm_domain::{CorrelationId, HostInstanceId, LibraryId, RequestId};
use sm_framing::{BufferBudget, Frame, Phase, Reader, Writer};
use sm_pipe::{Connection, Endpoint, Listener, TrustedServerImage};
use sm_protocol::{
    application::{
        ApplicationResult, CaptureCommandParams, CaptureOutcome, DoctorReport, HostStatus,
        KnownParams, LibraryAssetChunk, LibraryAssetRequest, ProtocolVersion, RequestEnvelope,
        Response, ShutdownReceipt, APPLICATION_VERSION,
    },
    errors::{AppError, ErrorCode},
    values::{MethodName, SafeMessage},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{ReadHalf, WriteHalf},
    sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::{timeout_at, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Capability list negotiated by exactly one build of this workspace. S02 adds the two
/// capture handlers; a peer that does not speak the same list is refused instead of being
/// served a method it cannot decode.
pub const CAPABILITIES: [&str; 14] = [
    "system.status",
    "system.doctor",
    "system.request_shutdown",
    "capture.command",
    "library.asset.read",
    "library.import",
    "library.import.status",
    "library.import.report",
    "library.import.control",
    "library.export",
    "library.asset.preview",
    "library.record.metadata",
    "processing.request",
    "knowledge.request",
];
const HANDSHAKE_TIME: Duration = Duration::from_secs(3);
const MAX_ACTIVE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionError {
    Transport,
    InvalidRequest,
    PermissionDenied,
    LibraryMismatch,
    ProtocolMismatch,
    CapabilityMismatch,
    CredentialUnavailable,
    Closed,
    Capacity,
    Timeout,
    UnexpectedResponse,
}
fn safe_error(code: ErrorCode) -> AppError {
    AppError {
        code,
        safe_message: SafeMessage::parse("Request rejected.").expect("constant"),
        retryable: false,
    }
}
fn response_error(id: RequestId, code: ErrorCode) -> Response {
    Response::Error {
        request_id: id,
        error: safe_error(code),
    }
}

// Handshake is private to the native Rust session. It is NOT an application or bridge DTO.
// Numeric bytes avoid a printable credential String; no Debug or logging of secret fields.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    version: ProtocolVersion,
    library_id: LibraryId,
    capabilities: Vec<String>,
    profile: WireProfile,
    credential: Vec<u8>,
}
impl Drop for Hello {
    fn drop(&mut self) {
        self.credential.fill(0);
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireProfile {
    Owner,
    TestReadOnly,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Welcome {
    version: ProtocolVersion,
    library_id: LibraryId,
    host_instance_id: HostInstanceId,
    capabilities: Vec<String>,
}
fn capability_set(values: &[String]) -> bool {
    values.len() == CAPABILITIES.len() && values.iter().zip(CAPABILITIES).all(|(a, b)| a == b)
}

/// Instantiate only after HostLease, DB, library identity and explicit credential enrollment.
/// The two typed ports are real host services; no synthetic report is supplied by this crate.
pub struct SessionHost {
    library: LibraryId,
    host: HostInstanceId,
    owner: Authenticator,
    read_only: Option<Authenticator>,
    foundation: Arc<dyn FoundationApi>,
    control: Arc<dyn FoundationControlApi>,
    capture: Option<Arc<dyn CaptureApi>>,
    images: Option<Arc<dyn ImageLibraryApi>>,
    processing: Option<Arc<dyn sm_application::processing::ProcessingApi>>,
    knowledge: Option<Arc<dyn sm_application::knowledge::KnowledgeApi>>,
    budget: BufferBudget,
}
impl SessionHost {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        library: LibraryId,
        host: HostInstanceId,
        owner: Authenticator,
        read_only: Option<Authenticator>,
        foundation: Arc<dyn FoundationApi>,
        control: Arc<dyn FoundationControlApi>,
        capture: Option<Arc<dyn CaptureApi>>,
        budget: BufferBudget,
    ) -> Self {
        Self {
            library,
            host,
            owner,
            read_only,
            foundation,
            control,
            capture,
            images: None,
            processing: None,
            knowledge: None,
            budget,
        }
    }
    pub fn with_knowledge(
        mut self,
        knowledge: Arc<dyn sm_application::knowledge::KnowledgeApi>,
    ) -> Self {
        self.knowledge = Some(knowledge);
        self
    }
    pub fn with_processing(
        mut self,
        processing: Arc<dyn sm_application::processing::ProcessingApi>,
    ) -> Self {
        self.processing = Some(processing);
        self
    }
    pub fn with_images(mut self, images: Arc<dyn ImageLibraryApi>) -> Self {
        self.images = Some(images);
        self
    }
    /// Listener.accept has already checked OS peer identity before this privileged flow.
    /// The entire handshake, including writer/admission waits, shares one absolute deadline.
    pub async fn serve(&self, connection: Connection) -> Result<(), SessionError> {
        let (r, w) = tokio::io::split(connection);
        let mut reader = Reader::new(r, self.budget.clone());
        let writer = Writer::new(w, self.budget.clone());
        let deadline = Instant::now() + HANDSHAKE_TIME;
        let context = timeout_at(deadline, async {
            let frame = reader
                .receive(Phase::Handshake)
                .await
                .map_err(|_| SessionError::Transport)?
                .ok_or(SessionError::Closed)?;
            let hello: Hello = serde_json::from_slice(frame.as_bytes())
                .map_err(|_| SessionError::InvalidRequest)?;
            drop(frame);
            if !APPLICATION_VERSION.compatible(hello.version) {
                return Err(SessionError::ProtocolMismatch);
            }
            if hello.library_id != self.library {
                return Err(SessionError::LibraryMismatch);
            }
            if !capability_set(&hello.capabilities) {
                return Err(SessionError::CapabilityMismatch);
            }
            let authenticator = match hello.profile {
                WireProfile::Owner => &self.owner,
                WireProfile::TestReadOnly => self
                    .read_only
                    .as_ref()
                    .ok_or(SessionError::PermissionDenied)?,
            };
            let supplied = Secret::from_bytes(hello.credential.to_vec());
            let context = authenticator
                .authenticate(
                    self.library,
                    &supplied,
                    RequestId::new(Uuid::new_v4()),
                    CorrelationId::new(Uuid::new_v4()),
                )
                .map_err(|_| SessionError::PermissionDenied)?
                .bind_connection(Uuid::new_v4());
            drop(supplied);
            drop(hello);
            writer
                .send(
                    Phase::Handshake,
                    &Welcome {
                        version: APPLICATION_VERSION,
                        library_id: self.library,
                        host_instance_id: self.host,
                        capabilities: CAPABILITIES.iter().map(|s| (*s).into()).collect(),
                    },
                )
                .await
                .map_err(|_| SessionError::Transport)?;
            Ok(context)
        })
        .await
        .map_err(|_| SessionError::Timeout)??;
        self.requests(reader, &writer, context).await
    }
    async fn requests(
        &self,
        mut reader: Reader<ReadHalf<Connection>>,
        writer: &Writer<WriteHalf<Connection>>,
        context: RequestContext,
    ) -> Result<(), SessionError> {
        // Reader is cancellation-unsafe by design. A dedicated, bounded pump keeps
        // receive polled across handler completions; no select branch drops mid-frame.
        let (tx, mut rx) = mpsc::channel::<Result<Option<Frame>, SessionError>>(1);
        let read_task = tokio::spawn(async move {
            loop {
                let received = reader
                    .receive(Phase::Application)
                    .await
                    .map_err(|_| SessionError::InvalidRequest);
                let end = !matches!(received, Ok(Some(_)));
                if tx.send(received).await.is_err() || end {
                    break;
                }
            }
        });
        let mut active: HashMap<RequestId, CancellationToken> = HashMap::new();
        let mut tasks = JoinSet::new();
        let outcome = loop {
            tokio::select! {
                biased;
                finished = tasks.join_next(), if !tasks.is_empty() => {
                    match finished {
                        Some(Ok(Ok(id))) => { active.remove(&id); }
                        _ => break Err(SessionError::Transport),
                    }
                }
                received = rx.recv() => {
                    let frame = match received {
                        Some(Ok(Some(f))) => f,
                        Some(Ok(None)) => break Ok(()),
                        _ => break Err(SessionError::InvalidRequest),
                    };
                    let message: Incoming = match serde_json::from_slice(frame.as_bytes()) {
                        Ok(v) => v,
                        Err(_) => break Err(SessionError::InvalidRequest),
                    };
                    drop(frame);
                    match message {
                        Incoming::Cancel(Cancel { request_id, .. }) => {
                            if let Some(token) = active.get(&request_id) { token.cancel(); }
                            else if active.len() < MAX_ACTIVE {
                                if writer.send(Phase::Application, &response_error(request_id, ErrorCode::PermissionDenied)).await.is_err() {
                                    break Err(SessionError::Transport);
                                }
                            } else { break Err(SessionError::Capacity); }
                        }
                        Incoming::Request(request) => {
                            let id = request.request_id;
                            if active.len() >= MAX_ACTIVE { break Err(SessionError::Capacity); }
                            if active.contains_key(&id) {
                                if writer.send(Phase::Application, &response_error(id, ErrorCode::InvalidRequest)).await.is_err() {
                                    break Err(SessionError::Transport);
                                }
                                continue;
                            }
                            let token = CancellationToken::new();
                            active.insert(id, token.clone());
                            let writer = writer.clone();
                            let foundation = self.foundation.clone();
                            let control = self.control.clone();
                            let capture = self.capture.clone();
                            let images = self.images.clone();
                            let processing = self.processing.clone();
                            let knowledge = self.knowledge.clone();
                            let context = context.for_request(id, CorrelationId::new(Uuid::new_v4()));
                            let library = self.library;
                            tasks.spawn(async move {
                                let response = tokio::select! {
                                    biased;
                                    _ = token.cancelled() => response_error(id, ErrorCode::WaitTimeout),
                                    result = dispatch(request, library, context, foundation, control, capture.clone(), images, processing, knowledge, token.clone()) => result,
                                };
                                // Keep the active ID/slot until a terminal response is actually sent.
                                writer.send(Phase::Application, &response).await.map_err(|_| ())?;
                                Ok::<_, ()>(id)
                            });
                        }
                    }
                }
            }
        };
        for token in active.values() {
            token.cancel();
        }
        tasks.abort_all();
        read_task.abort();
        while tasks.join_next().await.is_some() {}
        let _ = read_task.await;
        outcome
    }
}
/// A bounded accept loop: at most the sealed pool's 16 checked-out native handles.
/// CLIENT_LIMIT is transient while all owned slots are checked out, never an extra instance.
pub async fn run(
    listener: &mut Listener,
    host: Arc<SessionHost>,
    stop: CancellationToken,
) -> Result<(), SessionError> {
    run_draining(listener, host, stop, Duration::ZERO).await
}
/// Integration hook of the same accept loop: `drain` is a bounded window for already
/// admitted connections to finish an in-flight authorized response (for example a
/// shutdown receipt) before their tasks are aborted. A zero drain is the plain loop.
/// The window is a bound, never a claim that owned work actually finished.
pub async fn run_draining(
    listener: &mut Listener,
    host: Arc<SessionHost>,
    stop: CancellationToken,
    drain: Duration,
) -> Result<(), SessionError> {
    let mut clients = JoinSet::new();
    let mut outcome = Ok(());
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            joined = clients.join_next(), if !clients.is_empty() => { let _ = joined; },
            accepted = listener.accept(), if clients.len() < 16 => {
                match accepted {
                    Ok(connection) => { let host = host.clone(); clients.spawn(async move { let _ = host.serve(connection).await; }); }
                    Err(error) if error.code == "CLIENT_LIMIT" => tokio::time::sleep(Duration::from_millis(10)).await,
                    Err(error) if matches!(error.code, "CLIENT_SESSION" | "PEER_SESSION" | "PEER_TOKEN" | "PEER_PID" | "CLIENT_PID_CHANGED") => continue,
                    Err(_) => { outcome = Err(SessionError::Transport); break; }
                }
            }
        }
    }
    if drain > Duration::ZERO && !clients.is_empty() {
        let deadline = Instant::now() + drain;
        while !clients.is_empty() {
            match timeout_at(deadline, clients.join_next()).await {
                Ok(Some(_)) => {}
                _ => break,
            }
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    outcome
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel {
    kind: CancelKind,
    request_id: RequestId,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CancelKind {
    Cancel,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Incoming {
    Cancel(Cancel),
    Request(RequestEnvelope),
}
// A cancel is never privileged: only this connection's active IDs can be signalled.
#[allow(clippy::too_many_arguments)]
async fn dispatch(
    request: RequestEnvelope,
    library: LibraryId,
    ctx: RequestContext,
    foundation: Arc<dyn FoundationApi>,
    control: Arc<dyn FoundationControlApi>,
    capture: Option<Arc<dyn CaptureApi>>,
    images: Option<Arc<dyn ImageLibraryApi>>,
    processing: Option<Arc<dyn sm_application::processing::ProcessingApi>>,
    knowledge: Option<Arc<dyn sm_application::knowledge::KnowledgeApi>>,
    cancel: CancellationToken,
) -> Response {
    let id = request.request_id;
    let fail = |code| response_error(id, code);
    if !APPLICATION_VERSION.compatible(request.protocol) {
        return fail(ErrorCode::ProtocolMismatch);
    }
    if request.library_id != library {
        return fail(ErrorCode::LibraryMismatch);
    }
    let name = request.method.as_str();
    if !matches!(
        name,
        "system.status"
            | "system.doctor"
            | "system.request_shutdown"
            | "runtime.probe"
            | "system.set_ai_policy"
            | "system.authorize_probe"
            | "capture.command"
            | "library.asset.read"
            | "library.asset.preview"
            | "library.record.metadata"
            | "library.list_records"
            | "library.import"
            | "library.import.status"
            | "library.import.report"
            | "library.import.control"
            | "library.export"
            | "processing.request"
            | "knowledge.request"
    ) {
        return Response::Error {
            request_id: id,
            error: AppError::method_not_supported(),
        };
    }
    let params = match serde_json::from_value::<KnownParams>(
        serde_json::json!({"method":name,"params":request.params}),
    ) {
        Ok(p) => p,
        Err(_) => return fail(ErrorCode::InvalidRequest),
    };
    let scope = match &params {
        KnownParams::Status(_) => Scope::ReadStatus,
        KnownParams::Doctor(d) if !d.include_runtime_inventory => Scope::RunDoctor,
        KnownParams::Doctor(_) => Scope::ReadRuntimeInventory,
        KnownParams::RequestShutdown(_)
        | KnownParams::SetAiPolicy(_)
        | KnownParams::AuthorizeProbe(_) => Scope::ManageHost,
        KnownParams::ProbeRuntime(_) => Scope::RunAiProbe,
        // S02: a new capture and a bounded read of an owned canonical asset.
        KnownParams::CaptureCommand(_) => Scope::CaptureScreen,
        KnownParams::ReadAsset(_) | KnownParams::PreviewAsset(_) => Scope::ReadImages,
        KnownParams::RecordMetadata(_) => Scope::ReadSourceMetadata,
        // S02-T11: list committed records with their assets.
        KnownParams::ListRecords(_) => Scope::ListRecords,
        KnownParams::Import(_)
        | KnownParams::ImportStatus(_)
        | KnownParams::ImportReportPage(_)
        | KnownParams::ImportControl(_) => Scope::ImportFiles,
        KnownParams::Export(_) => Scope::ExportImages,
        KnownParams::Processing(
            sm_protocol::processing::ProcessingRequest::Policy {}
            | sm_protocol::processing::ProcessingRequest::Profile { .. }
            | sm_protocol::processing::ProcessingRequest::Batch { .. }
            | sm_protocol::processing::ProcessingRequest::Execution { .. }
            | sm_protocol::processing::ProcessingRequest::Reconcile {}
            | sm_protocol::processing::ProcessingRequest::Overview { .. }
            | sm_protocol::processing::ProcessingRequest::Inventory {},
        ) => Scope::ReadProcessing,
        KnownParams::Processing(_) => Scope::ManageProcessing,
        KnownParams::Knowledge(
            sm_protocol::knowledge::KnowledgeRequest::Read { .. }
            | sm_protocol::knowledge::KnowledgeRequest::Categories {},
        ) => Scope::ReadMetadata,
        KnownParams::Knowledge(sm_protocol::knowledge::KnowledgeRequest::Edit(_)) => {
            Scope::WriteMetadata
        }
        KnownParams::Knowledge(_) => Scope::ManageCategories,
    };
    if !ctx.allows(scope) {
        return fail(ErrorCode::PermissionDenied);
    }
    if request.timeout_ms == 0 || request.timeout_ms > 30_000 {
        return fail(ErrorCode::InvalidRequest);
    }
    let operation = async {
        match params {
            KnownParams::Knowledge(request) => match knowledge {
                Some(api) => api
                    .knowledge(ctx, request)
                    .await
                    .map(|reply| ApplicationResult::Knowledge(Box::new(reply))),
                None => Err(AppError::method_not_supported()),
            },
            KnownParams::Processing(request) => match processing {
                Some(api) => api
                    .processing(ctx, request)
                    .await
                    .map(|reply| ApplicationResult::Processing(Box::new(reply))),
                None => Err(AppError::method_not_supported()),
            },
            KnownParams::Status(_) => foundation.status(ctx).await.map(ApplicationResult::Status),
            KnownParams::Doctor(d) if !d.include_runtime_inventory => foundation
                .doctor(ctx, d, cancel.clone())
                .await
                .map(ApplicationResult::Doctor),
            KnownParams::RequestShutdown(_) => control
                .request_shutdown(ctx)
                .await
                .map(ApplicationResult::Shutdown),
            KnownParams::CaptureCommand(params) => {
                capture_command(capture, ctx, params, cancel.clone()).await
            }
            KnownParams::ReadAsset(request) => read_asset(capture, ctx, request).await,
            KnownParams::PreviewAsset(request) => match images {
                Some(api) => api
                    .preview_asset(ctx, request)
                    .await
                    .map(ApplicationResult::AssetChunk),
                None => Err(AppError::method_not_supported()),
            },
            KnownParams::RecordMetadata(request) => match images {
                Some(api) => api
                    .record_metadata(ctx, request)
                    .await
                    .map(ApplicationResult::RecordMetadata),
                None => Err(AppError::method_not_supported()),
            },
            KnownParams::ListRecords(request) => foundation
                .list_records(ctx, request)
                .await
                .map(ApplicationResult::RecordsPage),
            KnownParams::Import(request) => match images {
                Some(api) => api
                    .import_paths(ctx, request)
                    .await
                    .map(ApplicationResult::ImportReport),
                None => Err(AppError::method_not_supported()),
            },
            KnownParams::ImportStatus(request) => match images {
                Some(api) => api
                    .import_status(ctx, request)
                    .await
                    .map(ApplicationResult::ImportReport),
                None => Err(AppError::method_not_supported()),
            },
            KnownParams::ImportReportPage(request) => match images {
                Some(api) => api
                    .import_report_page(ctx, request)
                    .await
                    .map(ApplicationResult::ImportReport),
                None => Err(AppError::method_not_supported()),
            },
            KnownParams::ImportControl(request) => match images {
                Some(api) => api
                    .import_control(ctx, request)
                    .await
                    .map(ApplicationResult::ImportReport),
                None => Err(AppError::method_not_supported()),
            },
            KnownParams::Export(request) => match images {
                Some(api) => api
                    .export_image(ctx, request)
                    .await
                    .map(|receipt| ApplicationResult::ExportReceipt(Box::new(receipt))),
                None => Err(AppError::method_not_supported()),
            },
            _ => Err(AppError::method_not_supported()),
        }
    };
    match tokio::time::timeout(Duration::from_millis(request.timeout_ms as u64), operation).await {
        Ok(Ok(result)) => Response::Result {
            request_id: id,
            result,
        },
        Ok(Err(error)) => Response::Error {
            request_id: id,
            error,
        },
        Err(_) => {
            cancel.cancel();
            fail(ErrorCode::WaitTimeout)
        }
    }
}

/// Capture handlers exist only when this host owns a capture adapter; otherwise the closed
/// `METHOD_NOT_SUPPORTED` answer is returned instead of an invented result.
async fn capture_command(
    capture: Option<Arc<dyn CaptureApi>>,
    ctx: RequestContext,
    params: CaptureCommandParams,
    cancel: CancellationToken,
) -> Result<ApplicationResult, AppError> {
    match capture {
        Some(capture) => capture
            .capture_command(ctx, params, cancel)
            .await
            .map(ApplicationResult::Capture),
        None => Err(AppError::method_not_supported()),
    }
}

async fn read_asset(
    capture: Option<Arc<dyn CaptureApi>>,
    ctx: RequestContext,
    request: LibraryAssetRequest,
) -> Result<ApplicationResult, AppError> {
    match capture {
        Some(capture) => capture
            .read_asset(ctx, request)
            .await
            .map(ApplicationResult::AssetChunk),
        None => Err(AppError::method_not_supported()),
    }
}
