//! Storage-only process interruption: the parent may terminate ONLY its own Child handle.
//! No shotd, clipboard, host bootstrap or live library participates.
use super::*;
use crate::recovery_commit_tests::{paths, published, reopened, setup, Fixture};
use sha2::{Digest, Sha256};
use sm_application::{
    capture::{CaptureMode, CaptureTrigger},
    storage::{FoundationRepository, LookupOutcome, ReadIntentOutcome},
};
use sm_domain::OperationId;
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const CHILD_TEST: &str = "recovery_process_tests::s02_t09n4_child";
const TOKEN_ENV: &str = "SM_T09N4_CHILD_TOKEN";
const ROOT_ENV: &str = "SM_T09N4_CHILD_ROOT";
const PHASE_ENV: &str = "SM_T09N4_CHILD_PHASE";
const OP_ENV: &str = "SM_T09N4_CHILD_OPERATION";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CrashPhase {
    F06,
    F07,
}
impl CrashPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::F06 => "f06",
            Self::F07 => "f07",
        }
    }
}

/// Constructor is called exclusively by the ignored child test. Merely setting an
/// environment variable in a parent or another test cannot activate a checkpoint.
pub(super) struct CrashGate {
    root: PathBuf,
    token: Uuid,
    phase: CrashPhase,
}
impl CrashGate {
    fn child(root: &Path, phase: CrashPhase) -> Option<Self> {
        let args: Vec<_> = std::env::args().collect();
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        let base = fs::canonicalize(base).ok()?;
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        if root.parent() != Some(base.as_path())
            || !args
                .windows(2)
                .any(|v| v[0] == "--exact" && v[1] == CHILD_TEST)
            || !args.iter().any(|v| v == "--ignored")
            || std::env::var(PHASE_ENV).ok().as_deref() != Some(phase.as_str())
            || std::env::var(ROOT_ENV).ok().as_deref() != root.to_str()
            || root
                .file_name()
                .and_then(|v| v.to_str())
                .and_then(|v| Uuid::parse_str(v).ok())
                .filter(|v| v.get_version_num() == 4)
                .is_none()
        {
            return None;
        }
        let token = Uuid::parse_str(&std::env::var(TOKEN_ENV).ok()?).ok()?;
        (token.get_version_num() == 4 && token.to_string() == std::env::var(TOKEN_ENV).ok()?)
            .then_some(Self {
                root: root.to_owned(),
                token,
                phase,
            })
    }
    fn marker(&self) -> PathBuf {
        self.root.join(format!("t09n4-{}.ready", self.token))
    }
    pub(super) fn checkpoint(&self, phase: CrashPhase) {
        if self.phase != phase {
            return;
        }
        // If setup or marker creation fails, do NOT permit commit to advance.
        let temporary = self
            .root
            .join(format!("t09n4-{}.handshake.tmp", self.token));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .expect("child-only handshake creation");
        file.write_all(phase.as_str().as_bytes())
            .expect("handshake write");
        file.sync_all().expect("handshake sync");
        fs::rename(temporary, self.marker()).expect("publish complete handshake");
        loop {
            thread::park(); // Only the parent holding Child may terminate this child.
        }
    }
}

struct OwnedChild(Child);
impl OwnedChild {
    fn terminate_after_handshake(&mut self) -> std::process::ExitStatus {
        self.0.kill().expect("kill only this spawned Child handle");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.0.try_wait().expect("reap owned child") {
                assert!(
                    !status.success(),
                    "process kill must not be a normal return"
                );
                return status;
            }
            assert!(Instant::now() < deadline, "owned child kill/reap timeout");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            // Cleanup is bounded too: never wait indefinitely or fall back to PID/name kill.
            let _ = self.0.kill();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn spawn(f: &Fixture, phase: CrashPhase, token: &str, operation: Uuid) -> OwnedChild {
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            CHILD_TEST,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(TOKEN_ENV, token)
        .env(ROOT_ENV, &f.0)
        .env(PHASE_ENV, phase.as_str())
        .env(OP_ENV, operation.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn own test executable");
    OwnedChild(child)
}

fn await_marker(child: &mut OwnedChild, marker: &Path, phase: CrashPhase) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        if marker.exists() {
            assert_eq!(fs::read(marker).unwrap(), phase.as_str().as_bytes());
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "child exited before kill"
            );
            return;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child setup/checkpoint failed"
        );
        assert!(
            Instant::now() < deadline,
            "child handshake timeout; owned child killed by guard"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "launched only by the owning parent with exact argv and UUIDv4 child marker"]
fn s02_t09n4_child() {
    let root = PathBuf::from(std::env::var(ROOT_ENV).expect("child root"));
    let phase = match std::env::var(PHASE_ENV).as_deref() {
        Ok("f06") => CrashPhase::F06,
        Ok("f07") => CrashPhase::F07,
        _ => panic!("invalid child phase"),
    };
    let gate = Arc::new(CrashGate::child(&root, phase).expect("fail-closed child-only marker"));
    let operation = Uuid::parse_str(&std::env::var(OP_ENV).unwrap()).unwrap();
    assert_eq!(operation.get_version_num(), 4);
    // The root is project-owned by the parent; this wrapper must not remove it.
    let lease = HostLease::acquire(
        LibraryRootResolver::resolve(RootSource::Test {
            root: &root,
            forbidden_donor_roots: &[],
        })
        .unwrap(),
    )
    .unwrap();
    let worker = StorageWorker::start_with_process_crash_gate(lease, gate).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let library = LibraryId::new(worker.schema_snapshot().await.unwrap().library_id);
        let db = rusqlite::Connection::open(root.join("data/library.sqlite3")).unwrap();
        let (principal, mode): (String, String) = db
            .query_row(
                "SELECT principal_id,mode FROM capture_intents WHERE operation_id=?1",
                [operation.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(mode, "region");
        let principal = PrincipalId::new(Uuid::parse_str(&principal).unwrap());
        let request = CaptureRequest::new(
            operation,
            CaptureMode::Region,
            CaptureTrigger::ClientCommand,
        );
        let ReadIntentOutcome::Pending(pending) = worker
            .read_capture_intent(library, principal, request.clone())
            .await
            .unwrap()
        else {
            panic!("pending required")
        };
        let i = pending.input;
        let sealed = worker
            .seal_recovery_published(published(&worker, &i).await)
            .await
            .unwrap();
        // F06 blocks on the file lane AFTER its successful sealed canonical recheck.
        // F07 blocks on the SQL lane with DELETE + all three INSERTs still uncommitted.
        let _ = worker.commit_sealed_recovery(sealed).await;
        panic!("checkpoint did not park child");
    });
}

#[test]
fn s02_t09n4_f06_f07_owned_process_kill_and_two_restarts() {
    for phase in [CrashPhase::F06, CrashPhase::F07] {
        let f = Fixture::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let (i, planned) = setup(&f).await;
            let (final_path, stage_path) = paths(&f, &i, &planned);
            let manifest = fs::read(&stage_path).unwrap();
            let operation_dir = stage_path.parent().unwrap();
            let pixels_path = operation_dir.join("pixels.rgba");
            let context_path = operation_dir.join("context.json");
            let pixels = fs::read(&pixels_path).unwrap();
            let context = fs::read(&context_path).unwrap();
            let stage_hashes = [manifest.as_slice(), pixels.as_slice(), context.as_slice()]
                .map(Sha256::digest);
            let unknown = f.0.join("staging").join(i.request.operation_id().to_string()).join("unknown.tmp");
            fs::write(&unknown, b"keep-unknown").unwrap();
            // Bad child setup MUST exit without a handshake or canonical/DB write.
            let bad_token = Uuid::new_v4().to_string().to_uppercase();
            let mut bad = spawn(&f, phase, &bad_token, i.request.operation_id());
            let until = Instant::now() + Duration::from_secs(25);
            let bad_status = loop {
                if let Some(status) = bad.0.try_wait().unwrap() { break status; }
                assert!(Instant::now() < until, "bad child timeout; guard kills it");
                thread::sleep(Duration::from_millis(20));
            };
            assert_eq!(bad_status.code(), Some(101), "invalid setup must fail before handshake");
            assert!(!f.0.join(format!("t09n4-{}.ready", bad_token.to_lowercase())).exists());
            assert!(!final_path.exists());
            f.counts((1, 0, 0, 0));
            let token = Uuid::new_v4();
            let marker = f.0.join(format!("t09n4-{token}.ready"));
            let mut child = spawn(&f, phase, &token.to_string(), i.request.operation_id());
            await_marker(&mut child, &marker, phase);
            assert!(final_path.exists());
            let canonical = fs::read(&final_path).unwrap();
            let digest = format!("{:x}", Sha256::digest(&canonical));
            let status = child.terminate_after_handshake();
            assert_eq!(status.code(), Some(1), "raw Windows TerminateProcess exit from owned Child::kill");
            assert_eq!(fs::read(&stage_path).unwrap(), manifest);
            assert_eq!(fs::read(&pixels_path).unwrap(), pixels);
            assert_eq!(fs::read(&context_path).unwrap(), context);
            assert_eq!([manifest.as_slice(), pixels.as_slice(), context.as_slice()]
                .map(Sha256::digest), stage_hashes);
            assert_eq!(fs::read(&final_path).unwrap(), canonical);
            assert_eq!(format!("{:x}", Sha256::digest(fs::read(&final_path).unwrap())), digest);
            assert_eq!(fs::read(&unknown).unwrap(), b"keep-unknown");
            f.counts((1, 0, 0, 0));
            let first = StorageWorker::start(f.lease()).unwrap();
            let read = first.read_capture_intent(i.library_id, i.principal_id, i.request.clone()).await.unwrap();
            assert!(matches!(read, ReadIntentOutcome::Pending(_)));
            let stage = first.reopen_stage_owned_v2(reopened(&i)).await.unwrap();
            assert_eq!(stage.envelope().warnings(), Some(i.warnings.as_slice()));
            let published_file = published(&first, &i).await;
            assert!(!published_file.newly_published());
            let saved = first.commit_sealed_recovery(first.seal_recovery_published(published_file).await.unwrap()).await.unwrap();
            assert_eq!(saved.operation_id, OperationId::new(i.request.operation_id()));
            assert_eq!(saved.record_id, i.record_id);
            assert_eq!(saved.asset_id, i.asset_id);
            assert_eq!(saved.warnings, i.warnings);
            assert_eq!(saved.created_at, planned);
            assert_eq!(saved.relative_path, format!("assets/{}/{}/original.webp", &planned[..7], i.record_id.as_uuid()));
            assert_eq!(saved.sha256, digest);
            assert_eq!(saved.size_bytes, canonical.len() as u64);
            first.shutdown(Duration::from_secs(5)).await.unwrap();
            f.counts((0, 1, 1, 1));
            let db = f.db();
            let row: (String,String,String,String,String) = db.query_row(
                "SELECT r.id,a.id,io.operation_id,io.warnings,a.path FROM records r JOIN assets a ON a.record_id=r.id JOIN ingest_operations io ON io.record_id=r.id", [],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
            ).unwrap();
            assert_eq!(row.0, i.record_id.as_uuid().to_string());
            assert_eq!(row.1, i.asset_id.as_uuid().to_string());
            assert_eq!(row.2, i.request.operation_id().to_string());
            assert_eq!(row.3, sm_application::storage::warnings_to_json(&i.warnings).unwrap());
            assert_eq!(row.4, saved.relative_path);
            drop(db);
            let second = StorageWorker::start(f.lease()).unwrap();
            assert_eq!(second.lookup_operation(saved.operation_id, i.principal_id, i.request.target()).await.unwrap(), LookupOutcome::Saved(saved));
            second.shutdown(Duration::from_secs(5)).await.unwrap();
            f.counts((0, 1, 1, 1));
            assert_eq!(fs::read(&stage_path).unwrap(), manifest);
            assert_eq!(fs::read(&pixels_path).unwrap(), pixels);
            assert_eq!(fs::read(&context_path).unwrap(), context);
            assert_eq!([manifest.as_slice(), pixels.as_slice(), context.as_slice()]
                .map(Sha256::digest), stage_hashes);
            assert_eq!(fs::read(&final_path).unwrap(), canonical);
            assert_eq!(fs::read(&unknown).unwrap(), b"keep-unknown");
        });
    }
}
