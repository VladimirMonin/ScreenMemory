use super::*;
use sm_application::credentials::{Profile, StoreError};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::windows::named_pipe::NamedPipeClient;

/// Store lookup is explicit, read-only and bound to the caller-selected library/profile.
/// Provided bytes are permitted only for the synthetic TestReadOnly principal.
pub enum CredentialSource {
    Store {
        store: Arc<dyn SecretStore + Send + Sync>,
        key: CredentialKey,
    },
    TestReadOnly {
        key: CredentialKey,
        secret: Secret,
    },
}
impl CredentialSource {
    fn load(&self, library: LibraryId) -> Result<(WireProfile, Secret), SessionError> {
        match self {
            Self::Store { store, key } => {
                if key.library() != library {
                    return Err(SessionError::LibraryMismatch);
                }
                let profile = match key.profile() {
                    Profile::Owner => WireProfile::Owner,
                    Profile::TestReadOnly => WireProfile::TestReadOnly,
                };
                store
                    .read(key)
                    .map(|secret| (profile, secret))
                    .map_err(|_: StoreError| SessionError::CredentialUnavailable)
            }
            Self::TestReadOnly { key, secret } => {
                if key.library() != library
                    || key.profile() != Profile::TestReadOnly
                    || key.environment() != sm_application::credentials::CredentialEnvironment::Test
                {
                    return Err(SessionError::PermissionDenied);
                }
                Ok((
                    WireProfile::TestReadOnly,
                    Secret::from_bytes(secret.as_bytes().to_vec()),
                ))
            }
        }
    }
}
#[derive(Debug)]
pub enum ClientError {
    Session(SessionError),
    Application(AppError),
}
impl From<SessionError> for ClientError {
    fn from(value: SessionError) -> Self {
        Self::Session(value)
    }
}

struct ClientState {
    pending: Mutex<HashMap<RequestId, oneshot::Sender<Result<Response, SessionError>>>>,
    capacity: Arc<Semaphore>,
    closed: AtomicBool,
}
struct Pending<'a> {
    state: &'a ClientState,
    id: RequestId,
    _permit: OwnedSemaphorePermit,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.state
            .pending
            .lock()
            .expect("pending lock")
            .remove(&self.id);
    }
}
/// Single shared typed client for CLI and private Tauri handlers; no host autostart or retry.
/// Caller must supply the trusted absolute server image and expected library identity.
pub struct ApplicationClient {
    writer: Writer<WriteHalf<NamedPipeClient>>,
    state: Arc<ClientState>,
    receive_task: tokio::task::JoinHandle<()>,
    library: LibraryId,
    host: HostInstanceId,
}
impl ApplicationClient {
    pub async fn connect(
        endpoint: &Endpoint,
        image: &TrustedServerImage,
        expected_library: LibraryId,
        expected_host: Option<HostInstanceId>,
        credential: &CredentialSource,
        budget: BufferBudget,
    ) -> Result<Self, SessionError> {
        let deadline = Instant::now() + HANDSHAKE_TIME;
        let pipe = sm_pipe::open(endpoint, image).map_err(|_| SessionError::Transport)?;
        let (profile, secret) = credential.load(expected_library)?;
        if secret.as_bytes().len() != 33 || secret.as_bytes()[0] != 1 {
            return Err(SessionError::CredentialUnavailable);
        }
        let (r, w) = tokio::io::split(pipe);
        let mut reader = Reader::new(r, budget.clone());
        let writer = Writer::new(w, budget);
        let welcome = timeout_at(deadline, async {
            writer
                .send(
                    Phase::Handshake,
                    &Hello {
                        version: APPLICATION_VERSION,
                        library_id: expected_library,
                        capabilities: CAPABILITIES.iter().map(|s| (*s).into()).collect(),
                        profile,
                        credential: secret.as_bytes().to_vec(),
                    },
                )
                .await
                .map_err(|_| SessionError::Transport)?;
            let frame = reader
                .receive(Phase::Handshake)
                .await
                .map_err(|_| SessionError::Transport)?
                .ok_or(SessionError::Closed)?;
            let result = serde_json::from_slice::<Welcome>(frame.as_bytes())
                .map_err(|_| SessionError::InvalidRequest)?;
            drop(frame);
            Ok::<_, SessionError>(result)
        })
        .await
        .map_err(|_| SessionError::Timeout)??;
        drop(secret);
        if !APPLICATION_VERSION.compatible(welcome.version) {
            return Err(SessionError::ProtocolMismatch);
        }
        if welcome.library_id != expected_library
            || expected_host.is_some_and(|id| id != welcome.host_instance_id)
        {
            return Err(SessionError::LibraryMismatch);
        }
        if !capability_set(&welcome.capabilities) {
            return Err(SessionError::CapabilityMismatch);
        }
        let state = Arc::new(ClientState {
            pending: Mutex::new(HashMap::new()),
            capacity: Arc::new(Semaphore::new(MAX_ACTIVE)),
            closed: AtomicBool::new(false),
        });
        let receive_state = state.clone();
        let receive_task = tokio::spawn(async move {
            while let Ok(Some(frame)) = reader.receive(Phase::Application).await {
                let response: Response = match serde_json::from_slice(frame.as_bytes()) {
                    Ok(v) => v,
                    Err(_) => break,
                };
                drop(frame);
                let id = match &response {
                    Response::Result { request_id, .. } | Response::Error { request_id, .. } => {
                        *request_id
                    }
                };
                let sender = receive_state
                    .pending
                    .lock()
                    .expect("pending lock")
                    .remove(&id);
                match sender {
                    Some(tx) => {
                        let _ = tx.send(Ok(response));
                    }
                    None => break,
                }
            }
            // Publish closure before draining, so future calls cannot wait on an unpolled reader.
            receive_state.closed.store(true, Ordering::Release);
            receive_state.pending.lock().expect("pending lock").clear();
        });
        Ok(Self {
            writer,
            state,
            receive_task,
            library: expected_library,
            host: welcome.host_instance_id,
        })
    }
    pub fn host_instance_id(&self) -> HostInstanceId {
        self.host
    }
    pub fn library_id(&self) -> LibraryId {
        self.library
    }
    async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u32,
        cancellation: Option<CancellationToken>,
    ) -> Result<ApplicationResult, ClientError> {
        if self.state.closed.load(Ordering::Acquire) {
            return Err(SessionError::Closed.into());
        }
        let permit = self
            .state
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| SessionError::Capacity)?;
        let id = RequestId::new(Uuid::new_v4());
        let (sender, receiver) = oneshot::channel();
        self.state
            .pending
            .lock()
            .expect("pending lock")
            .insert(id, sender);
        let pending = Pending {
            state: &self.state,
            id,
            _permit: permit,
        };
        if self.state.closed.load(Ordering::Acquire) {
            return Err(SessionError::Closed.into());
        }
        let request = RequestEnvelope {
            protocol: APPLICATION_VERSION,
            request_id: id,
            library_id: self.library,
            method: MethodName::parse(method).expect("constant method"),
            timeout_ms,
            params,
        };
        self.writer
            .send(Phase::Application, &request)
            .await
            .map_err(|_| SessionError::Transport)?;
        let response = if let Some(cancellation) = cancellation {
            tokio::pin!(receiver);
            tokio::select! {
                response = &mut receiver => response.map_err(|_| SessionError::Closed)??,
                _ = cancellation.cancelled() => {
                    // This only signals the current connection's active request.
                    // Acknowledgement is the terminal response, NOT backend termination.
                    self.writer.send(Phase::Application, &Cancel { kind: CancelKind::Cancel, request_id: id })
                        .await.map_err(|_| SessionError::Transport)?;
                    receiver.await.map_err(|_| SessionError::Closed)??
                }
            }
        } else {
            receiver.await.map_err(|_| SessionError::Closed)??
        };
        drop(pending);
        match response {
            Response::Result { result, .. } => Ok(result),
            Response::Error { error, .. } => Err(ClientError::Application(error)),
        }
    }
    pub async fn status(&self) -> Result<HostStatus, ClientError> {
        match self
            .call("system.status", serde_json::json!({}), 3000, None)
            .await?
        {
            ApplicationResult::Status(v)
                if v.library_id == self.library
                    && v.host_instance_id == self.host
                    && APPLICATION_VERSION.compatible(v.protocol) =>
            {
                Ok(v)
            }
            _ => Err(SessionError::UnexpectedResponse.into()),
        }
    }
    pub async fn doctor(&self) -> Result<DoctorReport, ClientError> {
        match self
            .call(
                "system.doctor",
                serde_json::json!({"include_runtime_inventory":false}),
                30000,
                None,
            )
            .await?
        {
            ApplicationResult::Doctor(v)
                if v.host.library_id == self.library && v.host.host_instance_id == self.host =>
            {
                Ok(v)
            }
            _ => Err(SessionError::UnexpectedResponse.into()),
        }
    }
    pub async fn doctor_cancellable(
        &self,
        cancellation: CancellationToken,
    ) -> Result<DoctorReport, ClientError> {
        match self
            .call(
                "system.doctor",
                serde_json::json!({"include_runtime_inventory":false}),
                30000,
                Some(cancellation),
            )
            .await?
        {
            ApplicationResult::Doctor(v)
                if v.host.library_id == self.library && v.host.host_instance_id == self.host =>
            {
                Ok(v)
            }
            _ => Err(SessionError::UnexpectedResponse.into()),
        }
    }
    pub async fn request_shutdown(&self) -> Result<ShutdownReceipt, ClientError> {
        match self
            .call("system.request_shutdown", serde_json::json!({}), 3000, None)
            .await?
        {
            ApplicationResult::Shutdown(v) if v.host_instance_id == self.host => Ok(v),
            _ => Err(SessionError::UnexpectedResponse.into()),
        }
    }
    /// S02: run one capture command on the host. A region capture waits for the user, so
    /// the client wait limit is explicit and independent of the IPC handshake.
    pub async fn capture_command(
        &self,
        params: CaptureCommandParams,
        timeout_ms: u32,
        cancellation: Option<CancellationToken>,
    ) -> Result<CaptureOutcome, ClientError> {
        match self
            .call(
                "capture.command",
                serde_json::to_value(params).expect("capture params are serializable"),
                timeout_ms,
                cancellation,
            )
            .await?
        {
            ApplicationResult::Capture(outcome) => Ok(outcome),
            _ => Err(SessionError::UnexpectedResponse.into()),
        }
    }

    /// S02: one bounded chunk of a committed canonical asset.
    pub async fn read_asset(
        &self,
        request: LibraryAssetRequest,
    ) -> Result<LibraryAssetChunk, ClientError> {
        match self
            .call(
                "library.asset.read",
                serde_json::to_value(request).expect("asset request is serializable"),
                10_000,
                None,
            )
            .await?
        {
            ApplicationResult::AssetChunk(chunk) => Ok(chunk),
            _ => Err(SessionError::UnexpectedResponse.into()),
        }
    }

    /// S02-T11: bounded page of committed library records.
    pub async fn list_records(
        &self,
        request: sm_protocol::application::LibraryRecordsRequest,
    ) -> Result<sm_protocol::application::LibraryRecordsPage, ClientError> {
        match self
            .call(
                "library.list_records",
                serde_json::to_value(request).expect("list records request is serializable"),
                10_000,
                None,
            )
            .await?
        {
            ApplicationResult::RecordsPage(page) => Ok(page),
            _ => Err(SessionError::UnexpectedResponse.into()),
        }
   }

    /// Explicit best-effort cancellation, only for a request still pending on this client.
    pub async fn cancel(&self, id: RequestId) -> Result<(), SessionError> {
        if !self
            .state
            .pending
            .lock()
            .expect("pending lock")
            .contains_key(&id)
        {
            return Err(SessionError::PermissionDenied);
        }
        self.writer
            .send(
                Phase::Application,
                &Cancel {
                    kind: CancelKind::Cancel,
                    request_id: id,
                },
            )
            .await
            .map_err(|_| SessionError::Transport)
    }
}
impl Drop for ApplicationClient {
    fn drop(&mut self) {
        self.receive_task.abort();
    }
}
