#![cfg(windows)]
use sm_application::observability::{SafeEvent, SafeEventSink};
use sm_isolation::{
    Environment, InstalledRoots, LibraryRootResolver as Resolver, RootError, RootSource,
};
use sm_observability::{ConsoleSink, LogSink};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        // Cryptographic UUID naming avoids PID reuse and coarse wall-clock granularity.
        // create_dir remains atomic: collision is an error, never adoption or cleanup.
        Self::create_exact(parent.join(format!("t02b-{}", Uuid::new_v4()))).unwrap()
    }
    fn create_exact(root: PathBuf) -> io::Result<Self> {
        fs::create_dir(&root)?;
        Ok(Self(root))
    }
    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn installed<'a>(
    env: Environment,
    paths: &'a [PathBuf; 4],
    donor: &'a [&'a Path],
) -> RootSource<'a> {
    RootSource::Installed(InstalledRoots {
        environment: env,
        data: &paths[0],
        config: &paths[1],
        cache: &paths[2],
        webview: &paths[3],
        forbidden_donor_roots: donor,
    })
}
fn dirs(f: &Fixture, env: Environment) -> [PathBuf; 4] {
    let base = f.dir(env.installed_directory());
    ["data", "config", "cache", "webview"].map(|name| {
        let path = base.join(name);
        fs::create_dir(&path).unwrap();
        path
    })
}

#[test]
fn t02b_09_uuid_fixture_concurrency_and_collision_refusal() {
    let fixtures = std::thread::scope(|scope| {
        let tasks = (0..24)
            .map(|_| scope.spawn(Fixture::new))
            .collect::<Vec<_>>();
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .collect::<Vec<_>>()
    });
    let names = fixtures
        .iter()
        .map(|f| f.0.clone())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(names.len(), 24);
    let owned = &fixtures[0];
    let sentinel = owned.0.join("owned-sentinel.txt");
    fs::write(&sentinel, b"SYNTHETIC-OWNED").unwrap();
    assert_eq!(
        Fixture::create_exact(owned.0.clone()).err().unwrap().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&sentinel).unwrap(), b"SYNTHETIC-OWNED");
    assert!(owned.0.exists());
    drop(fixtures); // only successfully created own guards remove their exact roots
}

#[test]
fn t02b_01_distinct_namespaces_and_sentinel() {
    let f = Fixture::new();
    let mut paths = dirs(&f, Environment::Product);
    fs::write(paths[0].join("sentinel.txt"), b"SYNTHETIC-UNCHANGED").unwrap();
    let donor = f.dir("fake donor");
    let mut identities = Vec::new();
    for env in [Environment::Product, Environment::Dev, Environment::Test] {
        let per_env = if env == Environment::Product {
            paths.clone()
        } else {
            dirs(&f, env)
        };
        let found = Resolver::resolve(installed(env, &per_env, &[&donor])).unwrap();
        identities.push(found.data.file_identity());
        assert_ne!(found.data.file_identity(), found.config.file_identity());
        assert_ne!(found.cache.file_identity(), found.webview.file_identity());
        assert_eq!(found.data.volume_serial(), found.config.volume_serial());
        assert!(found.app_id().starts_with("io.screenmemory."));
        assert!(found.secret_namespace().starts_with("ScreenMemory."));
    }
    assert!(matches!(
        Resolver::resolve(installed(Environment::Dev, &paths, &[])),
        Err(RootError::Invalid)
    ));
    assert_eq!(identities.len(), 3);
    assert_ne!(identities[0], identities[1]);
    assert_ne!(identities[1], identities[2]);
    assert_ne!(Environment::Product.app_id(), Environment::Dev.app_id());
    assert_ne!(
        Environment::Dev.secret_namespace(),
        Environment::Test.secret_namespace()
    );
    assert_ne!(
        Environment::Product.window_class(),
        Environment::Test.window_class()
    );
    assert_ne!(
        Environment::Product.endpoint_prefix(),
        Environment::Dev.endpoint_prefix()
    );
    assert_eq!(
        fs::read(paths[0].join("sentinel.txt")).unwrap(),
        b"SYNTHETIC-UNCHANGED"
    );
    paths[0] = paths[0].parent().unwrap().join("data");
    fs::remove_file(paths[0].join("sentinel.txt")).unwrap();
    fs::remove_dir(&paths[0]).unwrap();
    assert!(matches!(
        Resolver::resolve(installed(Environment::Product, &paths, &[])),
        Err(RootError::Missing)
    ));
    assert!(!paths[0].exists());
}

#[test]
fn t02b_02_unicode_space_and_identity_alias() {
    let f = Fixture::new();
    let unicode_parent = f.dir("Dätā space");
    let env_root = unicode_parent.join(Environment::Dev.installed_directory());
    fs::create_dir(&env_root).unwrap();
    let paths = ["data", "config", "cache", "webview"].map(|name| {
        let path = env_root.join(name);
        fs::create_dir(&path).unwrap();
        path
    });
    let first = Resolver::resolve(installed(Environment::Dev, &paths, &[])).unwrap();
    let mut aliased = paths.clone();
    aliased[0] =
        f.0.join("dÄTĀ SPACE")
            .join("screenmemory.dev")
            .join("DATA")
            .join(".");
    let second = Resolver::resolve(installed(Environment::Dev, &aliased, &[])).unwrap();
    assert_eq!(
        (first.data.volume_serial(), first.data.file_identity()),
        (second.data.volume_serial(), second.data.file_identity())
    );
    assert!(matches!(
        Resolver::resolve(installed(Environment::Dev, &aliased, &[&paths[0]])),
        Err(RootError::Overlap)
    ));
}

#[test]
fn t02b_03_overlap_and_invalid_fail_closed() {
    let f = Fixture::new();
    let mut paths = dirs(&f, Environment::Product);
    let donor_parent = f.0.clone();
    assert!(matches!(
        Resolver::resolve(installed(Environment::Product, &paths, &[&donor_parent])),
        Err(RootError::Overlap)
    ));
    let descendant = paths[0].join("descendant");
    fs::create_dir(&descendant).unwrap();
    assert!(matches!(
        Resolver::resolve(installed(Environment::Product, &paths, &[&descendant])),
        Err(RootError::Overlap)
    ));
    paths[1] = paths[0].clone();
    assert!(matches!(
        Resolver::resolve(installed(Environment::Product, &paths, &[])),
        Err(RootError::Invalid)
    ));
    paths[1] = PathBuf::from("relative");
    assert!(matches!(
        Resolver::resolve(installed(Environment::Product, &paths, &[])),
        Err(RootError::Invalid)
    ));
    for bad in [
        r"\\server\share\data",
        r"\\?\C:\device",
        r"C:\data:stream",
        r"C:\temp\..\data",
        "C:\\temp\\data\u{0000}hidden",
    ] {
        paths[1] = PathBuf::from(bad);
        assert!(
            matches!(
                Resolver::resolve(installed(Environment::Product, &paths, &[])),
                Err(RootError::Invalid)
            ),
            "syntax must reject before IO"
        );
    }
}

#[test]
fn t02b_04_native_junction_is_rejected_without_admin() {
    let f = Fixture::new();
    let mut paths = dirs(&f, Environment::Test);
    let target = f.dir("junction target");
    fs::write(target.join("sentinel.txt"), b"JUNCTION-SYNTHETIC").unwrap();
    let link = paths[0].clone();
    fs::remove_dir(&link).unwrap();
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "junction proof blocked; raw exit={:?}",
        output.status.code()
    );
    paths[0] = link.clone();
    assert!(matches!(
        Resolver::resolve(installed(Environment::Test, &paths, &[])),
        Err(RootError::Reparse)
    ));
    let alias = f.0.join("ancestor junction");
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&alias)
        .arg(link.parent().unwrap())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(matches!(
        Resolver::resolve(RootSource::Test {
            root: &alias,
            forbidden_donor_roots: &[]
        }),
        Err(RootError::Reparse)
    ));
    let alias_status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "rmdir"])
        .arg(&alias)
        .status()
        .unwrap();
    assert_eq!(alias_status.code(), Some(0));
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "rmdir"])
        .arg(&link)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        fs::read(target.join("sentinel.txt")).unwrap(),
        b"JUNCTION-SYNTHETIC"
    );
}

#[test]
fn t02b_08_symlink_probe_distinct_from_junction() {
    let f = Fixture::new();
    let paths = dirs(&f, Environment::Test);
    let target = f.dir("symlink target");
    let link = paths[0].clone();
    fs::remove_dir(&link).unwrap();
    match std::os::windows::fs::symlink_dir(&target, &link) {
        Ok(()) => {
            assert!(matches!(
                Resolver::resolve(installed(Environment::Test, &paths, &[])),
                Err(RootError::Reparse)
            ));
            fs::remove_dir(&link).unwrap();
        }
        Err(err) => {
            panic!(
                "T02B-SYMLINK BLOCKED: raw OS code {:?}; junction proof is separate",
                err.raw_os_error()
            );
        }
    }
}

struct SharedBytes(Arc<Mutex<Vec<u8>>>);
impl Write for SharedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Events(Mutex<Vec<SafeEvent>>);
impl SafeEventSink for Events {
    fn emit(&self, event: SafeEvent) -> bool {
        self.0.lock().unwrap().push(event);
        true
    }
}
#[test]
fn t02b_05_logged_errors_do_not_contain_canary_paths() {
    let f = Fixture::new();
    let paths = dirs(&f, Environment::Test);
    let canary = f.0.join("CANARY-PRIVATE-ROOT-missing");
    let sink = Events(Mutex::new(Vec::new()));
    assert!(matches!(
        Resolver::resolve_logged(installed(Environment::Test, &paths, &[&canary]), &sink),
        Err(RootError::Missing)
    ));
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut logger = ConsoleSink::new(SharedBytes(bytes.clone()));
    logger.write_event(&sink.0.lock().unwrap()[0]).unwrap();
    logger.flush().unwrap();
    let bytes = bytes.lock().unwrap();
    assert!(!bytes.windows(6).any(|window| window == b"CANARY"));
    assert!(!bytes.windows(4).any(|window| window == b"t02b"));
}

#[test]
fn t02b_09_file_leaf_and_unknown_donor_rejected() {
    let f = Fixture::new();
    let paths = dirs(&f, Environment::Product);
    let missing_donor = f.0.join("fake donor absent");
    assert!(matches!(
        Resolver::resolve(installed(Environment::Product, &paths, &[&missing_donor])),
        Err(RootError::Missing)
    ));
    fs::remove_dir(&paths[0]).unwrap();
    fs::write(&paths[0], b"synthetic file, never adopted").unwrap();
    assert!(matches!(
        Resolver::resolve(installed(Environment::Product, &paths, &[])),
        Err(RootError::Invalid)
    ));
    assert_eq!(
        fs::read(&paths[0]).unwrap(),
        b"synthetic file, never adopted"
    );
}

#[test]
fn t02b_06_explicit_test_root_only() {
    let f = Fixture::new();
    let root = f.dir("test root");
    for name in ["data", "config", "cache", "webview"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    let layout = Resolver::resolve(RootSource::Test {
        root: &root,
        forbidden_donor_roots: &[],
    })
    .unwrap();
    assert_eq!(layout.environment, Environment::Test);
    for bad in [
        Path::new("."),
        Path::new(r"\\server\share\test"),
        Path::new(r"\\?\C:\device"),
        Path::new(r"C:\test:stream"),
        Path::new(r"C:\temporary\..\test"),
        Path::new("C:\\temporary\\test\u{0000}hidden"),
    ] {
        assert!(matches!(
            Resolver::resolve(RootSource::Test {
                root: bad,
                forbidden_donor_roots: &[]
            }),
            Err(RootError::Invalid)
        ));
    }
    let missing = f.0.join("missing-test-root");
    assert!(matches!(
        Resolver::resolve(RootSource::Test {
            root: &missing,
            forbidden_donor_roots: &[]
        }),
        Err(RootError::Missing)
    ));
    let linked = f.dir("test-link-target");
    let link = f.0.join("test-link");
    std::os::windows::fs::symlink_dir(&linked, &link).unwrap();
    assert!(matches!(
        Resolver::resolve(RootSource::Test {
            root: &link,
            forbidden_donor_roots: &[]
        }),
        Err(RootError::Reparse)
    ));
    fs::remove_dir(&link).unwrap();
}
