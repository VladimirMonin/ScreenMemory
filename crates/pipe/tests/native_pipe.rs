#![cfg(windows)]
use sm_framing::{BufferBudget, Phase, Reader, Writer};
use sm_isolation::{LibraryRootResolver, RootSource};
use sm_pipe::{open, Endpoint, Listener, TrustedServerImage};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let id = format!(
            "t06b-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let root = parent.join(id);
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
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn own_image() -> TrustedServerImage {
    TrustedServerImage::from_configuration(&std::env::current_exe().unwrap()).unwrap()
}

#[tokio::test]
async fn s01_pipe_01_sealed_sixteen_concurrent_frames_and_reconnect() {
    let fixture = Fixture::new();
    let endpoint = fixture.endpoint();
    let image = own_image();
    let mut listener = Listener::bind(endpoint).unwrap();
    let endpoint = fixture.endpoint();
    let mut clients = Vec::new();
    for _ in 0..16 {
        clients.push(open(&endpoint, &image).unwrap());
    }
    assert_eq!(open(&endpoint, &image).err().unwrap().win32, Some(231));
    let mut servers = Vec::new();
    for _ in 0..16 {
        servers.push(
            tokio::time::timeout(std::time::Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(listener.accept().await.err().unwrap().code, "CLIENT_LIMIT");
    let budget = BufferBudget::default();
    for (i, client) in clients.iter_mut().enumerate() {
        Writer::new(&mut *client, budget.clone())
            .send(Phase::Application, &serde_json::json!({"synthetic":i}))
            .await
            .unwrap();
    }
    let mut mapping = Vec::new();
    for server in &mut servers {
        let frame = Reader::new(server.stream(), budget.clone())
            .receive(Phase::Application)
            .await
            .unwrap()
            .unwrap();
        let id = serde_json::from_slice::<serde_json::Value>(frame.as_bytes()).unwrap()
            ["synthetic"]
            .as_u64()
            .unwrap();
        assert!(id < 16);
        mapping.push(id);
        drop(frame);
        Writer::new(server.stream(), budget.clone())
            .send(Phase::Application, &serde_json::json!({"ack":id}))
            .await
            .unwrap();
    }
    for (i, client) in clients.iter_mut().enumerate() {
        let frame = Reader::new(client, budget.clone())
            .receive(Phase::Application)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(frame.as_bytes()).unwrap()["ack"],
            i
        );
    }
    let paired_slot = mapping.iter().position(|id| *id == 0).unwrap();
    drop(clients.remove(0));
    drop(servers.remove(paired_slot));
    // Handle returned by owning connection, never recreated; other 15 still hold the name.
    let (server, reopened) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(listener.accept(), async {
            loop {
                match open(&endpoint, &image) {
                    Ok(client) => break client,
                    Err(error) if error.win32 == Some(231) => tokio::task::yield_now().await,
                    Err(error) => panic!("unexpected code {} {:?}", error.code, error.win32),
                }
            }
        })
    })
    .await
    .unwrap();
    let mut reopened = reopened;
    let mut server = server.unwrap();
    Writer::new(&mut reopened, budget.clone())
        .send(Phase::Application, &serde_json::json!({"reconnected":true}))
        .await
        .unwrap();
    let frame = Reader::new(server.stream(), budget)
        .receive(Phase::Application)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(frame.as_bytes()).unwrap()["reconnected"],
        true
    );
}

#[tokio::test]
async fn s01_pipe_02_owned_collision_alias_and_image_mismatch() {
    let fixture = Fixture::new();
    let endpoint = fixture.endpoint();
    let mut listener = Listener::bind(endpoint).unwrap();
    let other = Fixture::new();
    let separate_listener = Listener::bind(other.endpoint()).unwrap();
    assert!(open(&other.endpoint(), &own_image()).is_ok());
    assert_ne!(
        sm_isolation::Environment::Test.endpoint_prefix(),
        sm_isolation::Environment::Dev.endpoint_prefix()
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1), listener.accept())
            .await
            .is_err()
    );
    let alias = PathBuf::from(fixture.0.to_string_lossy().to_uppercase());
    let layout = LibraryRootResolver::resolve(RootSource::Test {
        root: &alias,
        forbidden_donor_roots: &[],
    })
    .unwrap();
    let endpoint = Endpoint::for_layout(&layout).unwrap();
    let collision = Listener::bind(fixture.endpoint());
    assert_eq!(collision.err().unwrap().win32, Some(5));
    let fake = fixture.0.join("synthetic-other.exe");
    fs::write(&fake, b"SYNTHETIC-NON-EXECUTABLE").unwrap();
    let foreign = TrustedServerImage::from_configuration(&fake).unwrap();
    assert_eq!(
        open(&endpoint, &foreign).err().unwrap().code,
        "SERVER_IMAGE_MISMATCH"
    );
    assert!(open(&endpoint, &own_image()).is_ok());
    drop(listener);
    drop(separate_listener);
}
