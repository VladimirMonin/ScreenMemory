#![cfg(windows)]
//! Raw native-wire negative fixtures: synthetic owned endpoint and in-memory verifier only.
use async_trait::async_trait;
use sm_application::{
    credentials::{
        Authenticator, CredentialEnvironment, CredentialKey, Secret, SecretStore, StoreError,
    },
    FoundationApi, FoundationControlApi, RequestContext,
};
use sm_domain::{HostInstanceId, LibraryId, RequestId};
use sm_framing::{BufferBudget, Phase, Reader, Writer};
use sm_isolation::{LibraryRootResolver, RootSource};
use sm_pipe::{Endpoint, Listener, TrustedServerImage};
use sm_protocol::{
    application::{
        AiPolicyPatch, AiPolicyView, AuthorizeProbeRequest, BridgeState, DoctorReport,
        DoctorRequest, HostState, HostStatus, LibraryRecordsPage, LibraryRecordsRequest,
        ProbeConsentReceipt, Response, RuntimeProbeReport, RuntimeProbeRequest, ShutdownReceipt,
        APPLICATION_VERSION,
    },
    errors::{AppError, ErrorCode},
    values::BuildId,
};
use sm_session::{run, SessionHost};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{io::AsyncWriteExt, net::windows::named_pipe::NamedPipeClient};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Roots(PathBuf);
impl Roots {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = parent.join(format!("t06c-wire-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(child)).unwrap();
        }
        Self(root)
    }
    fn endpoint(&self) -> Endpoint {
        let layout = LibraryRootResolver::resolve(RootSource::Test {
            root: &self.0,
            forbidden_donor_roots: &[],
        })
        .unwrap();
        Endpoint::for_layout(&layout).unwrap()
    }
}
impl Drop for Roots {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Store([u8; 33]);
impl SecretStore for Store {
    fn read(&self, _: &CredentialKey) -> Result<Secret, StoreError> {
        Ok(Secret::from_bytes(self.0.to_vec()))
    }
    fn create_new(&self, _: &CredentialKey, _: &Secret) -> Result<(), StoreError> {
        panic!("auth must not enroll")
    }
}
struct Services {
    library: LibraryId,
    host: HostInstanceId,
    calls: AtomicUsize,
    delayed: AtomicUsize,
}
#[async_trait]
impl FoundationApi for Services {
    async fn status(&self, _: RequestContext) -> Result<HostStatus, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HostStatus {
            host_instance_id: self.host,
            library_id: self.library,
            build_id: BuildId::parse("synthetic").unwrap(),
            protocol: APPLICATION_VERSION,
            state: HostState::Ready,
            schema_version: 1,
            ai_enabled: false,
            bridge_state: BridgeState::Stopped,
            available_methods: vec![],
        })
    }
    async fn doctor(
        &self,
        _: RequestContext,
        _: DoctorRequest,
        _: CancellationToken,
    ) -> Result<DoctorReport, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.delayed.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
    async fn probe_runtime(
        &self,
        _: RequestContext,
        _: RuntimeProbeRequest,
        _: CancellationToken,
    ) -> Result<RuntimeProbeReport, AppError> {
        panic!("unsupported")
    }
    async fn list_records(
        &self,
        _: RequestContext,
        _: LibraryRecordsRequest,
    ) -> Result<LibraryRecordsPage, AppError> {
        Ok(LibraryRecordsPage {
            items: Vec::new(),
            next_cursor: None,
        })
    }
}
#[async_trait]
impl FoundationControlApi for Services {
    async fn set_ai_policy(
        &self,
        _: RequestContext,
        _: AiPolicyPatch,
    ) -> Result<AiPolicyView, AppError> {
        panic!("unsupported")
    }
    async fn authorize_probe(
        &self,
        _: RequestContext,
        _: AuthorizeProbeRequest,
    ) -> Result<ProbeConsentReceipt, AppError> {
        panic!("unsupported")
    }
    async fn request_shutdown(&self, _: RequestContext) -> Result<ShutdownReceipt, AppError> {
        panic!("not part of raw fixtures")
    }
}
struct Fixture {
    _roots: Roots,
    endpoint: Endpoint,
    image: TrustedServerImage,
    library: LibraryId,
    key: [u8; 33],
    calls: Arc<Services>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), sm_session::SessionError>>,
}
impl Fixture {
    fn new() -> Self {
        let roots = Roots::new();
        let endpoint = roots.endpoint();
        let listener = Listener::bind(roots.endpoint()).unwrap();
        let image =
            TrustedServerImage::from_configuration(&std::env::current_exe().unwrap()).unwrap();
        let library = LibraryId::new(Uuid::new_v4());
        let host = HostInstanceId::new(Uuid::new_v4());
        let mut key = [42; 33];
        key[0] = 1;
        let auth = Authenticator::open(
            &Store(key),
            CredentialKey::owner(CredentialEnvironment::Test, library),
        )
        .unwrap();
        let calls = Arc::new(Services {
            library,
            host,
            calls: AtomicUsize::new(0),
            delayed: AtomicUsize::new(0),
        });
        let session = Arc::new(SessionHost::new(
            library,
            host,
            auth,
            None,
            calls.clone(),
            calls.clone(),
            None,
            BufferBudget::new(),
        ));
        let stop = CancellationToken::new();
        let token = stop.clone();
        let task = tokio::spawn(async move {
            let mut listener = listener;
            run(&mut listener, session, token).await
        });
        Self {
            _roots: roots,
            endpoint,
            image,
            library,
            key,
            calls,
            stop,
            task,
        }
    }
    async fn raw(&self) -> NamedPipeClient {
        sm_pipe::open(&self.endpoint, &self.image).unwrap()
    }
    async fn hello(
        &self,
        pipe: &mut NamedPipeClient,
        library: LibraryId,
        version: u16,
        caps: &[&str],
        secret: &[u8],
    ) -> bool {
        let payload = serde_json::json!({"version":{"major":version,"minor":0}, "library_id": library,
            "capabilities": caps, "profile":"owner", "credential": secret});
        Writer::new(&mut *pipe, BufferBudget::new())
            .send(Phase::Handshake, &payload)
            .await
            .unwrap();
        let reply = Reader::new(&mut *pipe, BufferBudget::new())
            .receive(Phase::Handshake)
            .await;
        matches!(reply, Ok(Some(_)))
    }
    async fn close(self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
/// The negotiated list of this build: the three foundation handlers plus the two S02
/// capture handlers. A peer that does not speak exactly this list is refused.
const CAPS: [&str; 14] = [
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
fn request(
    f: &Fixture,
    id: RequestId,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({"protocol": APPLICATION_VERSION, "request_id":id, "library_id":f.library,
        "method": method, "timeout_ms":3000, "params":params})
}
async fn send(pipe: &mut NamedPipeClient, value: serde_json::Value) {
    Writer::new(&mut *pipe, BufferBudget::new())
        .send(Phase::Application, &value)
        .await
        .unwrap();
}
async fn reply(pipe: &mut NamedPipeClient) -> Response {
    let frame = tokio::time::timeout(
        Duration::from_secs(3),
        Reader::new(&mut *pipe, BufferBudget::new()).receive(Phase::Application),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    serde_json::from_slice(frame.as_bytes()).unwrap()
}
#[tokio::test]
async fn s01_ipc_03_native_rejects_wrong_library_major_capabilities_credential_and_malformed() {
    let f = Fixture::new();
    for (library, version, caps, secret) in [
        (LibraryId::new(Uuid::new_v4()), 1, &CAPS[..], &f.key[..]),
        (f.library, 2, &CAPS[..], &f.key[..]),
        (f.library, 1, &["system.status"][..], &f.key[..]),
        (f.library, 1, &CAPS[..], &[1; 33][..]),
    ] {
        let mut pipe = f.raw().await;
        assert!(!f.hello(&mut pipe, library, version, caps, secret).await);
    }
    let mut claimed_gui = f.raw().await;
    Writer::new(&mut claimed_gui, BufferBudget::new())
        .send(
            Phase::Handshake,
            &serde_json::json!({"version":APPLICATION_VERSION,"library_id":f.library,
            "capabilities":CAPS,"profile":"owner","credential":f.key.to_vec(),"client_kind":"gui"}),
        )
        .await
        .unwrap();
    assert!(!matches!(
        Reader::new(&mut claimed_gui, BufferBudget::new())
            .receive(Phase::Handshake)
            .await,
        Ok(Some(_))
    ));
    drop(claimed_gui);
    let mut pipe = f.raw().await;
    assert!(f.hello(&mut pipe, f.library, 1, &CAPS, &f.key).await);
    let id = RequestId::new(Uuid::new_v4());
    let mut malformed = request(&f, id, "system.status", serde_json::json!({"unexpected":1}));
    assert!(matches!(
        reply_after(&mut pipe, &mut malformed).await,
        Response::Error {
            error: AppError {
                code: ErrorCode::InvalidRequest,
                ..
            },
            ..
        }
    ));
    assert_eq!(f.calls.calls.load(Ordering::SeqCst), 0);
    drop(pipe);
    f.close().await;
}
async fn reply_after(pipe: &mut NamedPipeClient, request: &mut serde_json::Value) -> Response {
    send(pipe, std::mem::take(request)).await;
    reply(pipe).await
}
#[tokio::test]
async fn s01_ipc_03_duplicate_cancel_connection_scope_and_reconnect() {
    let f = Fixture::new();
    let mut a = f.raw().await;
    let mut b = f.raw().await;
    assert!(f.hello(&mut a, f.library, 1, &CAPS, &f.key).await);
    assert!(f.hello(&mut b, f.library, 1, &CAPS, &f.key).await);
    let id = RequestId::new(Uuid::new_v4());
    send(
        &mut a,
        request(
            &f,
            id,
            "system.doctor",
            serde_json::json!({"include_runtime_inventory":false}),
        ),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.calls.delayed.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    send(
        &mut a,
        request(
            &f,
            id,
            "system.doctor",
            serde_json::json!({"include_runtime_inventory":false}),
        ),
    )
    .await;
    assert!(
        matches!(reply(&mut a).await, Response::Error {request_id, error: AppError { code: ErrorCode::InvalidRequest, .. }} if request_id == id)
    );
    send(&mut b, serde_json::json!({"kind":"cancel","request_id":id})).await;
    assert!(matches!(
        reply(&mut b).await,
        Response::Error {
            error: AppError {
                code: ErrorCode::PermissionDenied,
                ..
            },
            ..
        }
    ));
    send(&mut a, serde_json::json!({"kind":"cancel","request_id":id})).await;
    assert!(
        matches!(reply(&mut a).await, Response::Error {request_id, error: AppError { code: ErrorCode::WaitTimeout, .. }} if request_id == id)
    );
    drop(a);
    drop(b);
    let mut again = f.raw().await;
    assert!(f.hello(&mut again, f.library, 1, &CAPS, &f.key).await);
    let next = RequestId::new(Uuid::new_v4());
    send(
        &mut again,
        request(&f, next, "system.status", serde_json::json!({})),
    )
    .await;
    assert!(
        matches!(reply(&mut again).await, Response::Result {request_id, ..} if request_id == next)
    );
    assert_eq!(f.calls.calls.load(Ordering::SeqCst), 2);
    drop(again);
    f.close().await;
}
#[tokio::test]
async fn s01_ipc_03_ninth_request_fails_closed_and_disconnect_preserves_host() {
    let f = Fixture::new();
    let mut pipe = f.raw().await;
    assert!(f.hello(&mut pipe, f.library, 1, &CAPS, &f.key).await);
    for _ in 0..8 {
        send(
            &mut pipe,
            request(
                &f,
                RequestId::new(Uuid::new_v4()),
                "system.doctor",
                serde_json::json!({"include_runtime_inventory":false}),
            ),
        )
        .await;
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.calls.delayed.load(Ordering::SeqCst) != 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    send(
        &mut pipe,
        request(
            &f,
            RequestId::new(Uuid::new_v4()),
            "system.status",
            serde_json::json!({}),
        ),
    )
    .await;
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        Reader::new(&mut pipe, BufferBudget::new()).receive(Phase::Application),
    )
    .await
    .unwrap();
    assert!(!matches!(result, Ok(Some(_))));
    drop(pipe);
    let mut reconnect = f.raw().await;
    assert!(f.hello(&mut reconnect, f.library, 1, &CAPS, &f.key).await);
    let next = RequestId::new(Uuid::new_v4());
    send(
        &mut reconnect,
        request(&f, next, "system.status", serde_json::json!({})),
    )
    .await;
    assert!(
        matches!(reply(&mut reconnect).await, Response::Result {request_id, ..} if request_id == next)
    );
    assert_eq!(f.calls.calls.load(Ordering::SeqCst), 9); // eight delayed + one reconnect, never ninth handler
    drop(reconnect);
    f.close().await;
}
#[tokio::test]
async fn s01_ipc_03_absolute_handshake_deadline_closes_partial_native_frame() {
    let f = Fixture::new();
    let mut pipe = f.raw().await;
    pipe.write_all(&(100u32).to_le_bytes()).await.unwrap();
    let start = tokio::time::Instant::now();
    let mut byte = [0u8];
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::io::AsyncReadExt::read(&mut pipe, &mut byte),
    )
    .await;
    assert!(outcome.is_ok());
    assert!(start.elapsed() >= Duration::from_millis(2700));
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(f.calls.calls.load(Ordering::SeqCst), 0);
    drop(pipe);
    f.close().await;
}
