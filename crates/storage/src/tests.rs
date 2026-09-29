use super::*;
use sha2::{Digest, Sha256};
use sm_isolation::{LibraryRootResolver, RootSource};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = base.join(format!(
            "s01-db-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(child)).unwrap();
        }
        Self(root)
    }
    fn db(&self) -> PathBuf {
        self.0.join("data/library.sqlite3")
    }
    fn lease(&self) -> HostLease {
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &self.0,
                forbidden_donor_roots: &[],
            })
            .unwrap(),
        )
        .unwrap()
    }
    fn rt<T>(&self, f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(f)
    }
    fn close(&self, worker: &StorageWorker) {
        self.rt(worker.shutdown(Duration::from_secs(5))).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn s01_db_01_fresh_reopen_uuid_seeds_pragmas_and_bundled_sqlite() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let first = f.rt(worker.schema_snapshot()).unwrap();
    assert_eq!(first.schema_version, 3);
    assert_eq!(first.images.migration_version, 2);
    assert_eq!(
        first.images.canonical_format,
        sm_application::storage::CANONICAL_FORMAT_WEBP_LOSSLESS
    );
    assert_eq!(first.format_major, 1);
    assert!(!f.rt(worker.read_ai_enabled()).unwrap());
    let version = f.rt(worker.sqlite_version()).unwrap();
    println!(
        "bundled sqlite_version={version}; worker pragmas={:?}",
        f.rt(worker.read_pragmas()).unwrap()
    );
    assert!(
        !version.is_empty(),
        "bundled runtime sqlite_version must be returned"
    );
    assert_eq!(
        f.rt(worker.read_pragmas()).unwrap(),
        ("wal".to_owned(), 1, 2, 5000)
    );
    f.close(&worker);
    let db = Connection::open_with_flags(f.db(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let value = |sql| db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!(value("SELECT count(*) FROM schema_migrations"), 3);
    assert_eq!(value("SELECT count(*) FROM library_settings"), 2);
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(first, f.rt(worker.schema_snapshot()).unwrap());
    f.close(&worker);
}

#[test]
fn s01_db_02_preflight_corrupt_metadata_settings_checksum_future_and_foreign() {
    for statement in [
        "UPDATE schema_migrations SET checksum='corrupt'",
        "UPDATE schema_migrations SET version=9 WHERE version=2",
        "INSERT INTO schema_migrations VALUES (4,'unknown','x','now')",
        "UPDATE library_meta SET library_id='not-a-uuid'",
        "UPDATE library_meta SET format_major=3",
        "UPDATE library_settings SET value_json='not-json' WHERE key='ai.enabled'",
        "INSERT INTO library_settings VALUES ('endpoint', '123', 1)",
        "CREATE TABLE unexpected (value TEXT)",
        "CREATE INDEX unexpected_idx ON library_meta(format_major)",
        "DELETE FROM library_meta",
    ] {
        let f = Fixture::new();
        let worker = StorageWorker::start(f.lease()).unwrap();
        f.close(&worker);
        let db = Connection::open(f.db()).unwrap();
        db.execute_batch(statement).unwrap();
        drop(db);
        let original = fs::read(f.db()).unwrap();
        let result = StorageWorker::start(f.lease());
        assert!(
            matches!(
                result,
                Err(StorageError::IntegrityFailed | StorageError::SchemaIncompatible)
            ),
            "{statement}"
        );
        assert_eq!(
            fs::read(f.db()).unwrap(),
            original,
            "failed preflight must preserve data"
        );
    }
    let f = Fixture::new();
    let db = Connection::open(f.db()).unwrap();
    db.execute_batch(
        "CREATE TABLE foreign_notes (sentinel TEXT); INSERT INTO foreign_notes VALUES ('keep');",
    )
    .unwrap();
    drop(db);
    let original = fs::read(f.db()).unwrap();
    assert!(matches!(
        StorageWorker::start(f.lease()),
        Err(StorageError::SchemaIncompatible)
    ));
    assert_eq!(fs::read(f.db()).unwrap(), original);
    assert!(!f.db().with_file_name("library.sqlite3-wal").exists());
}

#[test]
fn s01_db_02_real_uncheckpointed_wal_preflight() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let original = f.rt(worker.schema_snapshot()).unwrap();
    f.close(&worker);
    let reader = Connection::open(f.db()).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT revision FROM library_settings LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let writer = Connection::open(f.db()).unwrap();
    writer.execute_batch("PRAGMA wal_autocheckpoint=0; UPDATE library_settings SET revision=2 WHERE key='ai.enabled'").unwrap();
    drop(writer);
    assert!(f.0.join("data/library.sqlite3-wal").exists());
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(f.rt(worker.schema_snapshot()).unwrap(), original);
    reader.execute_batch("ROLLBACK").unwrap();
    drop(reader);
    f.close(&worker);
    let db = Connection::open(f.db()).unwrap();
    let revision: i64 = db
        .query_row(
            "SELECT revision FROM library_settings WHERE key='ai.enabled'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        revision, 2,
        "preflight must see live WAL, not immutable main db"
    );
}

#[test]
fn s01_db_02_fault_rollback_and_interrupted_empty_table() {
    let f = Fixture::new();
    assert!(matches!(
        StorageWorker::start_with_fault(f.lease()),
        Err(StorageError::MigrationFailed)
    ));
    let db = Connection::open(f.db()).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "DDL and migration row rolled back together");
    let marker: u32 = db
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap();
    assert_eq!(marker, super::migration::OWNER_ID);
    db.execute_batch(super::migration::BOOTSTRAP).unwrap();
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    // Both shipped migrations are applied by one transaction on a fresh file; the
    // interrupted attempt left an owned, schema-empty database with durable intent.
    assert_eq!(f.rt(worker.schema_snapshot()).unwrap().schema_version, 3);
    f.close(&worker);
    let retry = Fixture::new();
    assert!(matches!(
        StorageWorker::start_with_fault(retry.lease()),
        Err(StorageError::MigrationFailed)
    ));
    // Crash/rollback left an owned, schema-empty DB with durable intent.
    let worker = StorageWorker::start(retry.lease()).unwrap();
    assert_eq!(
        retry.rt(worker.schema_snapshot()).unwrap().schema_version,
        3
    );
    retry.close(&worker);
}

#[test]
fn s01_db_02_foreign_empty_schema_and_unclaimed_files_never_adopted() {
    // Exact bytes and canary remain unchanged; no WAL/SHM/journal is created
    // by the rejected worker. Every scenario is a real SQLite/file fixture.
    for case in [
        "retained_freelist_foreign",
        "retained_freelist_zero",
        "vacuum_foreign",
        "vacuum_zero",
        "spoofed_marker_foreign_schema",
        "preexisting_zero_file",
    ] {
        let f = Fixture::new();
        let sentinel = f.0.join("synthetic-sentinel");
        fs::write(&sentinel, b"canary-keep").unwrap();
        if case == "preexisting_zero_file" {
            fs::write(f.db(), []).unwrap();
        } else {
            let db = Connection::open(f.db()).unwrap();
            let id = match case {
                "retained_freelist_foreign" | "vacuum_foreign" => 123456,
                "spoofed_marker_foreign_schema" => super::migration::OWNER_ID,
                _ => 0,
            };
            db.execute_batch(&format!("PRAGMA application_id={id}; CREATE TABLE private_data (secret BLOB); INSERT INTO private_data VALUES ('synthetic-canary'); DROP TABLE private_data;"))
                .unwrap();
            if case.starts_with("vacuum") {
                db.execute_batch("VACUUM").unwrap();
            }
            if case == "spoofed_marker_foreign_schema" {
                db.execute_batch("CREATE TABLE foreign_notes (sentinel TEXT); INSERT INTO foreign_notes VALUES ('keep')").unwrap();
            }
            drop(db);
        }
        let original = fs::read(f.db()).unwrap();
        let original_hash = Sha256::digest(&original);
        let prior_sidecars: Vec<_> = ["-wal", "-shm", "-journal"]
            .iter()
            .map(|suffix| {
                f.db()
                    .with_file_name(format!("library.sqlite3{suffix}"))
                    .exists()
            })
            .collect();
        let result = StorageWorker::start(f.lease());
        assert!(
            matches!(result, Err(StorageError::SchemaIncompatible)),
            "{case}: unexpected open outcome"
        );
        assert_eq!(
            Sha256::digest(fs::read(f.db()).unwrap()),
            original_hash,
            "{case}"
        );
        assert_eq!(fs::read(f.db()).unwrap(), original, "{case}");
        assert_eq!(fs::read(&sentinel).unwrap(), b"canary-keep", "{case}");
        let after_sidecars: Vec<_> = ["-wal", "-shm", "-journal"]
            .iter()
            .map(|suffix| {
                f.db()
                    .with_file_name(format!("library.sqlite3{suffix}"))
                    .exists()
            })
            .collect();
        assert_eq!(after_sidecars, prior_sidecars, "{case}: sidecar created");
        if case.starts_with("retained_freelist") {
            let db = Connection::open_with_flags(f.db(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            let freelist: i64 = db
                .query_row("PRAGMA freelist_count", [], |r| r.get(0))
                .unwrap();
            let page_count: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
            assert_eq!((page_count, freelist), (2, 1), "{case}: reviewer shape");
        }
    }
}

#[test]
fn s01_db_02_missing_main_with_foreign_sidecar_is_not_fresh() {
    for suffix in ["-wal", "-journal"] {
        let f = Fixture::new();
        let sidecar = f.db().with_file_name(format!("library.sqlite3{suffix}"));
        fs::write(&sidecar, b"synthetic-nonempty-foreign-sidecar").unwrap();
        assert!(matches!(
            StorageWorker::start(f.lease()),
            Err(StorageError::RootRejected)
        ));
        assert!(!f.db().exists());
        assert_eq!(
            fs::read(sidecar).unwrap(),
            b"synthetic-nonempty-foreign-sidecar"
        );
    }
}

#[test]
fn s01_db_02_lock_before_open_and_worker_close_reacquire() {
    let f = Fixture::new();
    let lease = f.lease();
    assert!(matches!(
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &f.0,
                forbidden_donor_roots: &[]
            })
            .unwrap()
        ),
        Err(LeaseError::Busy)
    ));
    assert!(!f.db().exists());
    let worker = StorageWorker::start(lease).unwrap();
    assert!(matches!(
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &f.0,
                forbidden_donor_roots: &[]
            })
            .unwrap()
        ),
        Err(LeaseError::Busy)
    ));
    let second = worker.clone();
    f.close(&worker);
    assert!(matches!(
        f.rt(second.schema_snapshot()),
        Err(StorageError::Unavailable)
    ));
    let worker = StorageWorker::start(f.lease()).unwrap();
    f.close(&worker);
}

#[test]
fn s01_db_02_timeout_preserves_owner_until_connection_close() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let paused = worker.pause_for_test(Duration::from_millis(150));
    assert!(matches!(
        f.rt(worker.shutdown(Duration::ZERO)),
        Err(StorageError::Unavailable)
    ));
    assert!(matches!(
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &f.0,
                forbidden_donor_roots: &[]
            })
            .unwrap()
        ),
        Err(LeaseError::Busy)
    ));
    f.rt(paused).unwrap().unwrap();
    // Timed-out Stop remains queued; no second owner until worker closes SQLite.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(lease) = HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &f.0,
                forbidden_donor_roots: &[],
            })
            .unwrap(),
        ) {
            let worker = StorageWorker::start(lease).unwrap();
            f.close(&worker);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker must eventually close"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn s01_db_02_unsafe_entry_and_outside_sentinel() {
    let f = Fixture::new();
    let outside = f.0.join("outside-sentinel");
    fs::write(&outside, b"synthetic-canary").unwrap();
    fs::create_dir(f.db()).unwrap();
    assert!(matches!(
        StorageWorker::start(f.lease()),
        Err(StorageError::RootRejected)
    ));
    assert_eq!(fs::read(&outside).unwrap(), b"synthetic-canary");
    fs::remove_dir(f.db()).unwrap();
    fs::write(f.0.join("outside-hardlink"), b"synthetic-hardlink").unwrap();
    fs::hard_link(f.0.join("outside-hardlink"), f.db()).unwrap();
    assert!(matches!(
        StorageWorker::start(f.lease()),
        Err(StorageError::RootRejected)
    ));
    assert_eq!(
        fs::read(f.0.join("outside-hardlink")).unwrap(),
        b"synthetic-hardlink"
    );
    fs::remove_file(f.db()).unwrap();
    fs::create_dir(f.0.join("data/library.sqlite3-wal")).unwrap();
    assert!(matches!(
        StorageWorker::start(f.lease()),
        Err(StorageError::RootRejected)
    ));
    assert!(!f.db().exists());
    assert_eq!(fs::read(&outside).unwrap(), b"synthetic-canary");
}

#[test]
fn s01_db_02_owned_file_sharing_denial_and_retry() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    f.close(&worker);
    // Owned fixture only. Sharing-denial IO probe, NOT an ACL probe.
    let original = fs::read(f.db()).unwrap();
    let hold = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(f.db())
        .unwrap();
    let denied = StorageWorker::start(f.lease()).map(|_| ());
    assert!(
        matches!(
            denied,
            Err(StorageError::PermissionDenied | StorageError::RootRejected)
        ),
        "{denied:?}"
    );
    drop(hold);
    assert_eq!(fs::read(f.db()).unwrap(), original);
    let worker = StorageWorker::start(f.lease()).unwrap();
    f.close(&worker);
}

#[test]
fn s01_db_02_database_reparse_symlink_rejected_without_outside_write() {
    let f = Fixture::new();
    let outside = f.0.join("outside-sentinel");
    fs::write(&outside, b"synthetic-canary").unwrap();
    std::os::windows::fs::symlink_file(&outside, f.db()).unwrap();
    assert!(matches!(
        StorageWorker::start(f.lease()),
        Err(StorageError::RootRejected)
    ));
    assert_eq!(fs::read(&outside).unwrap(), b"synthetic-canary");
    fs::remove_file(f.db()).unwrap();
}

#[test]
fn s01_db_02_unicode_root_and_case_alias() {
    let f = Fixture::new();
    let root = f.0.join("Unicode-данные");
    fs::create_dir(&root).unwrap();
    for child in ["data", "config", "cache", "webview"] {
        fs::create_dir(root.join(child)).unwrap();
    }
    let acquire = |path: &Path| {
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: path,
                forbidden_donor_roots: &[],
            })
            .unwrap(),
        )
        .unwrap()
    };
    let worker = StorageWorker::start(acquire(&root)).unwrap();
    let first = f.rt(worker.schema_snapshot()).unwrap();
    f.close(&worker);
    let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
    let worker = StorageWorker::start(acquire(&upper)).unwrap();
    assert_eq!(first, f.rt(worker.schema_snapshot()).unwrap());
    f.close(&worker);
}
