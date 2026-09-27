#![cfg(windows)]
use sm_isolation::{HostLease, LeaseError, LibraryRootResolver, RootSource};
use std::{
    ffi::c_void,
    fs, io,
    os::windows::io::{AsHandle, AsRawHandle, OwnedHandle},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier,
    },
    thread,
    time::{Duration, Instant},
};

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = base.join(format!(
            "t05a-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn root(&self, name: &str) -> PathBuf {
        let root = self.0.join(name);
        fs::create_dir(&root).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(child)).unwrap();
        }
        root
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn acquire(root: &Path) -> Result<HostLease, LeaseError> {
    HostLease::acquire(
        LibraryRootResolver::resolve(RootSource::Test {
            root,
            forbidden_donor_roots: &[],
        })
        .unwrap(),
    )
}
fn until(limit: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "bounded synthetic worker deadline exceeded"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
struct Workers(Vec<Child>);
impl Drop for Workers {
    fn drop(&mut self) {
        for child in &mut self.0 {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
            }
        }
        for child in &mut self.0 {
            let _ = child.wait();
        }
    }
}
fn spawn_with(
    root: &Path,
    coordination: &Path,
    n: usize,
    mut launch: impl FnMut(&mut Command, usize) -> io::Result<Child>,
    mut observe: impl FnMut(&Child),
) -> io::Result<Workers> {
    let exe = std::env::current_exe()?;
    let mut workers = Workers(Vec::new());
    for i in 0..n {
        let mut command = Command::new(&exe);
        command
            .args(["--exact", "t05a_worker", "--nocapture", "--test-threads=1"])
            .env("SM_T05A_WORKER", "1")
            .env("SM_T05A_ROOT", root)
            .env("SM_T05A_SYNC", coordination)
            .env("SM_T05A_INDEX", i.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        workers.0.push(launch(&mut command, i)?);
        observe(workers.0.last().unwrap()); // panic here still drops the owning guard
    }
    Ok(workers)
}
fn spawn(root: &Path, coordination: &Path, n: usize) -> Workers {
    spawn_with(root, coordination, n, |command, _| command.spawn(), |_| {}).unwrap()
}

// Private test-only OS process handles remain valid even after Workers reaps its Children.
// On an assertion panic, the observer terminates only processes it duplicated from
// harness-owned Children; it never looks up a PID or a foreign process.
#[link(name = "kernel32")]
extern "system" {
    fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    fn TerminateProcess(handle: *mut c_void, exit_code: u32) -> i32;
}
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
struct ObservedChildren(Vec<OwnedHandle>);
impl Drop for ObservedChildren {
    fn drop(&mut self) {
        for handle in &self.0 {
            let raw = handle.as_raw_handle();
            if unsafe { WaitForSingleObject(raw, 0) } == WAIT_TIMEOUT {
                unsafe { TerminateProcess(raw, 1) };
                // Never block indefinitely during panic cleanup.
                unsafe { WaitForSingleObject(raw, 5_000) };
            }
        }
    }
}
#[test]
fn t05a_04_partial_spawn_failure_reaps_owned_children() {
    let f = Fixture::new();
    let root = f.root("partial-spawn-root");
    let sync = f.root("partial-spawn-coordination");
    let mut observed = ObservedChildren(Vec::new());
    let failure = match spawn_with(
        &root,
        &sync,
        4,
        |command, index| {
            if index == 3 {
                // Real CreateProcess failure, not an exhausted-resource simulation.
                Command::new(sync.join("missing-worker-executable.exe")).spawn()
            } else {
                command.spawn()
            }
        },
        |child| {
            observed
                .0
                .push(child.as_handle().try_clone_to_owned().unwrap())
        },
    ) {
        Ok(_) => panic!("missing executable unexpectedly spawned"),
        Err(error) => error,
    };
    assert_eq!(observed.0.len(), 3); // all three are real successfully spawned workers
    assert_eq!(failure.kind(), io::ErrorKind::NotFound);
    assert_eq!(failure.raw_os_error(), Some(2)); // original Windows CreateProcess cause
    for handle in &observed.0 {
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 5_000) },
            WAIT_OBJECT_0,
            "owned child not terminated and reaped before spawn error returned"
        );
    }
}
#[test]
fn t05a_05_observer_panic_reaps_owned_children() {
    let f = Fixture::new();
    let root = f.root("panic-spawn-root");
    let sync = f.root("panic-spawn-coordination");
    let mut observed = ObservedChildren(Vec::new());
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = spawn_with(
            &root,
            &sync,
            3,
            |command, _| command.spawn(),
            |child| {
                observed
                    .0
                    .push(child.as_handle().try_clone_to_owned().unwrap());
                if observed.0.len() == 3 {
                    panic!("synthetic observer assertion");
                }
            },
        );
    }))
    .expect_err("injected assertion must unwind");
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"synthetic observer assertion")
    );
    assert_eq!(observed.0.len(), 3);
    for handle in &observed.0 {
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 5_000) },
            WAIT_OBJECT_0,
            "owned child not terminated and reaped during observer panic"
        );
    }
}
#[test]
fn t05a_worker() {
    if std::env::var_os("SM_T05A_WORKER").is_none() {
        return;
    }
    let root = PathBuf::from(std::env::var_os("SM_T05A_ROOT").unwrap());
    let sync = PathBuf::from(std::env::var_os("SM_T05A_SYNC").unwrap());
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
    let base = fs::canonicalize(base).unwrap();
    assert!(fs::canonicalize(&root).unwrap().starts_with(&base));
    assert!(fs::canonicalize(&sync).unwrap().starts_with(&base));
    let index: usize = std::env::var("SM_T05A_INDEX").unwrap().parse().unwrap();
    assert!(index < 50);
    fs::write(sync.join(format!("ready-{index}")), b"ready").unwrap();
    until(Duration::from_secs(20), || sync.join("start").exists());
    let result = acquire(&root);
    let state = match &result {
        Ok(_) => "owner",
        Err(LeaseError::Busy) => "busy",
        Err(_) => "unsafe",
    };
    fs::write(sync.join(format!("result-{index}")), state).unwrap();
    if result.is_ok() {
        until(Duration::from_secs(60), || sync.join("release").exists());
    }
}
#[test]
fn t05a_01_fifty_simultaneous_processes_and_crash() {
    let f = Fixture::new();
    let root = f.root("same-root");
    let sync = f.root("coordination");
    let mut workers = spawn(&root, &sync, 50);
    until(Duration::from_secs(15), || {
        (0..50).all(|i| sync.join(format!("ready-{i}")).exists())
    });
    fs::write(sync.join("start"), b"start").unwrap();
    until(Duration::from_secs(15), || {
        (0..50).all(|i| sync.join(format!("result-{i}")).exists())
    });
    let outcomes: Vec<_> = (0..50)
        .map(|i| fs::read_to_string(sync.join(format!("result-{i}"))).unwrap())
        .collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|value| value.as_str() == "owner")
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|value| value.as_str() == "busy")
            .count(),
        49
    );
    assert!(matches!(acquire(&root), Err(LeaseError::Busy)));
    let independent = acquire(&f.root("other-root")).unwrap(); // process B may own a distinct identity
    drop(independent);
    let winner = outcomes.iter().position(|value| value == "owner").unwrap();
    assert!(workers.0[winner].try_wait().unwrap().is_none());
    workers.0[winner].kill().unwrap(); // only Child spawned by this harness
    assert!(!workers.0[winner].wait().unwrap().success()); // expected crash, not test success
    until(Duration::from_secs(5), || acquire(&root).is_ok());
    let lease = acquire(&root).unwrap();
    fs::write(sync.join("release"), b"release").unwrap();
    drop(lease);
    until(Duration::from_secs(10), || {
        workers
            .0
            .iter_mut()
            .all(|child| child.try_wait().unwrap().is_some())
    });
    for (i, child) in workers.0.iter_mut().enumerate() {
        if i != winner {
            assert_eq!(child.wait().unwrap().code(), Some(0));
        }
    }
    drop(workers);
    assert!(acquire(&root).is_ok());
}
#[test]
fn t05a_02_alias_independent_roots_and_stale_contents() {
    let f = Fixture::new();
    let a = f.root("Mixed Case");
    let b = f.root("another root");
    fs::write(
        a.join("data/host.lock"),
        b"stale PID 987654321\nCANARY-SECRET",
    )
    .unwrap();
    let first = acquire(&a).unwrap();
    // A held lock inode cannot be unlinked or renamed into a second inode/name.
    assert!(fs::remove_file(a.join("data/host.lock")).is_err());
    assert!(fs::rename(a.join("data/host.lock"), a.join("data/moved.lock")).is_err());
    let alias = f.0.join("mixed case").join(".");
    assert!(matches!(acquire(&alias), Err(LeaseError::Busy)));
    let independent = acquire(&b).unwrap();
    drop(first);
    assert_eq!(
        fs::read(a.join("data/host.lock")).unwrap(),
        b"stale PID 987654321\nCANARY-SECRET"
    );
    let again = acquire(&a).unwrap();
    drop(again);
    drop(independent);
    assert!(a.join("data/host.lock").is_file());
}
#[test]
fn t05a_03_unsafe_entries_and_pinned_rename_junction_attempts() {
    let f = Fixture::new();
    let root = f.root("owned");
    let outside = f.root("external-sentinel");
    fs::write(outside.join("sentinel"), b"SYNTHETIC-UNCHANGED").unwrap();
    // Positive controls on the identical nonempty layout before any lock exists:
    // without handles both root and data CAN move, and a real junction can occupy
    // the vacated leaf. Rejection with pins is therefore meaningful.
    let moved_root = f.0.join("control-moved-root");
    fs::rename(&root, &moved_root).unwrap();
    fs::rename(&moved_root, &root).unwrap();
    let moved_data = root.join("control-moved-data");
    fs::rename(root.join("data"), &moved_data).unwrap();
    let mut link = Workers(vec![Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(root.join("data"))
        .arg(&outside)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()]);
    until(Duration::from_secs(5), || {
        link.0[0].try_wait().unwrap().is_some()
    });
    assert_eq!(link.0[0].wait().unwrap().code(), Some(0));
    assert!(matches!(
        LibraryRootResolver::resolve(RootSource::Test {
            root: &root,
            forbidden_donor_roots: &[]
        }),
        Err(sm_isolation::RootError::Reparse)
    ));
    let mut unlink = Workers(vec![Command::new("cmd.exe")
        .args(["/d", "/c", "rmdir"])
        .arg(root.join("data"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()]);
    until(Duration::from_secs(5), || {
        unlink.0[0].try_wait().unwrap().is_some()
    });
    assert_eq!(unlink.0[0].wait().unwrap().code(), Some(0));
    assert!(!root.join("data").exists());
    fs::rename(&moved_data, root.join("data")).unwrap();
    assert!(!outside.join("host.lock").exists());
    let lock = root.join("data/host.lock");
    fs::create_dir(&lock).unwrap();
    assert!(matches!(acquire(&root), Err(LeaseError::UnsafeEntry)));
    fs::remove_dir(&lock).unwrap();
    std::os::windows::fs::symlink_file(outside.join("sentinel"), &lock).unwrap();
    assert!(matches!(acquire(&root), Err(LeaseError::UnsafeEntry)));
    fs::remove_file(&lock).unwrap();
    fs::hard_link(outside.join("sentinel"), &lock).unwrap();
    assert!(matches!(acquire(&root), Err(LeaseError::UnsafeEntry)));
    fs::remove_file(&lock).unwrap();
    let pinned = LibraryRootResolver::resolve(RootSource::Test {
        root: &root,
        forbidden_donor_roots: &[],
    })
    .unwrap();
    let target = f.0.join("renamed-root");
    let barrier = Arc::new(Barrier::new(2));
    let attempts = thread::spawn({
        let root = root.clone();
        let target = target.clone();
        let barrier = barrier.clone();
        move || {
            barrier.wait();
            for _ in 0..1000 {
                assert!(
                    fs::rename(&root, &target).is_err(),
                    "pinned root unexpectedly moved"
                );
                assert!(
                    fs::rename(root.join("data"), root.join("moved-data")).is_err(),
                    "pinned data unexpectedly moved"
                );
            }
        }
    });
    // Concurrent with rename attempts, the same mklink used in the unpinned
    // positive control must fail on the retained data leaf before lock creation.
    let junction = Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(root.join("data"))
        .arg(&outside)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut junction = Workers(vec![junction]);
    barrier.wait();
    until(Duration::from_secs(5), || {
        junction.0[0].try_wait().unwrap().is_some()
    });
    assert!(!junction.0[0].wait().unwrap().success());
    attempts.join().unwrap();
    assert!(!outside.join("host.lock").exists()); // before first lock creation
                                                  // Retained pins survive the resolver-to-lease gap; no path-only handoff.
    let owner = HostLease::acquire(pinned).unwrap();
    assert!(matches!(acquire(&root), Err(LeaseError::Busy)));
    assert_eq!(
        fs::read(outside.join("sentinel")).unwrap(),
        b"SYNTHETIC-UNCHANGED"
    );
    assert!(!outside.join("host.lock").exists());
    drop(owner);
    assert!(acquire(&root).is_ok());
}
