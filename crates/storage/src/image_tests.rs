//! S02 image storage: migration, canonical publication, rollback and bounded reads.
//! Every fixture is synthetic and lives under the owned `.pi/temporary` root.
use super::*;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use sm_application::storage::{
    CaptureContext, CaptureTarget, CommitImageCapture, PhysicalBounds, Provenance, RecordPageQuery,
    StorageError,
};
use sm_codec::{encode_exact_lossless_verified, RgbaImage};
use sm_domain::{AssetId, RecordId};
use sm_isolation::{LibraryRootResolver, RootSource};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use uuid::Uuid;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// Captured-at observation used by the synthetic drafts; a real RFC3339 UTC instant.
const CAPTURED_AT: &str = "2026-09-28T09:15:00.000Z";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = base.join(format!(
            "s02-images-{}-{}-{}",
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
    fn root(&self) -> &Path {
        &self.0
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
    /// Every published canonical file below the owned assets directory.
    fn published_files(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![self.0.join("assets")];
        while let Some(directory) = stack.pop() {
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    found.push(path);
                }
            }
        }
        found.sort();
        found
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn synthetic(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        })
        .collect()
}

/// Real canonical container produced by the bundled codec from synthetic pixels.
fn canonical(width: u32, height: u32, seed: u64) -> Vec<u8> {
    let image = RgbaImage::from_rgba(
        width,
        height,
        synthetic((width * height * 4) as usize, seed),
    )
    .expect("synthetic image");
    encode_exact_lossless_verified(&image)
        .expect("encode+verify")
        .into_canonical()
        .bytes()
        .to_vec()
}

fn context(target: CaptureTarget) -> CaptureContext {
    CaptureContext {
        device: "synthetic-display".to_owned(),
        app: "synthetic-app".to_owned(),
        title: "Synthetic capture".to_owned(),
        target,
        sources: vec![target],
        bounds: PhysicalBounds {
            x: -120,
            y: 0,
            width: 320,
            height: 200,
        },
    }
}

fn manual_provenances() -> Vec<Provenance> {
    vec![Provenance::Manual {
        when: CAPTURED_AT.to_owned(),
    }]
}

fn draft(record_id: &str, asset_id: &str, bytes: Vec<u8>) -> CommitImageCapture {
    CommitImageCapture {
        record_id: RecordId::new(Uuid::parse_str(record_id).unwrap()),
        asset_id: AssetId::new(Uuid::parse_str(asset_id).unwrap()),
        asset_type: sm_application::storage::AssetType::Original,
        capture_context: context(CaptureTarget::Region),
        provenances: manual_provenances(),
        canonical: bytes,
    }
}

fn record_uuid(index: u64) -> String {
    Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000u128 + u128::from(index)).to_string()
}

fn direct(db: &Path) -> Connection {
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn s02_images_01_migration_applies_both_versions_and_stays_reopenable() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let snapshot = f.rt(worker.schema_snapshot()).unwrap();
    assert_eq!(snapshot.schema_version, 2);
    assert_eq!(snapshot.images.migration_version, 2);
    assert_eq!(
        snapshot.images.canonical_format,
        sm_application::storage::CANONICAL_FORMAT_WEBP_LOSSLESS
    );
    f.close(&worker);

    let db = direct(&f.db());
    let rows: Vec<(i64, String, String)> = db
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 1);
    assert_eq!(rows[0].1, "0001_foundation");
    assert_eq!(rows[1].0, 2);
    assert_eq!(rows[1].1, "0002_images");
    assert_eq!(
        rows[1].2,
        format!(
            "{:x}",
            Sha256::digest(include_str!("../migrations/0002_images.sql").as_bytes())
        )
    );
    assert_eq!(count(&db, "records"), 0);
    assert_eq!(count(&db, "assets"), 0);
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        tables,
        [
            "assets",
            "library_meta",
            "library_settings",
            "records",
            "schema_migrations",
            "sqlite_sequence"
        ],
        "sqlite_sequence belongs to the AUTOINCREMENT ordering key"
    );
    drop(db);

    // Reopening the same library reports the identical snapshot; no second writer.
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(f.rt(worker.schema_snapshot()).unwrap(), snapshot);
    f.close(&worker);
}

#[test]
fn s02_images_02_commit_writes_record_asset_and_file_together() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let bytes = canonical(64, 48, 5);
    let expected_sha = format!("{:x}", Sha256::digest(&bytes));
    let receipt = f
        .rt(worker.commit_image_capture(draft(&record_uuid(1), &record_uuid(101), bytes.clone())))
        .expect("commit");
    assert_eq!(receipt.size_bytes, bytes.len() as u64);
    assert_eq!(receipt.sha256, expected_sha);
    assert!(receipt.relative_path.starts_with("assets/202"));
    assert!(receipt.relative_path.ends_with("original.webp"));
    assert!(receipt.relative_path.contains(&record_uuid(1)));
    assert!(sm_application::storage::is_canonical_utc_timestamp(
        &receipt.created_at
    ));
    assert_eq!(
        receipt.relative_path,
        format!(
            "assets/{}/{}/original.webp",
            &receipt.created_at[..7],
            record_uuid(1)
        )
    );

    let published = f.root().join(&receipt.relative_path);
    assert_eq!(fs::read(&published).unwrap(), bytes);
    let page = f
        .rt(worker.page_records(RecordPageQuery::first(None).unwrap()))
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next_cursor, None);
    let record = &page.items[0];
    assert_eq!(record.id.as_uuid().to_string(), record_uuid(1));
    assert_eq!(record.title, "Synthetic capture");
    assert_eq!(record.target, CaptureTarget::Region);
    assert_eq!(record.bounds.width, 320);
    assert_eq!(record.assets.len(), 1);
    assert_eq!(record.assets[0].sha256, receipt.sha256);
    assert_eq!(record.assets[0].relative_path, receipt.relative_path);
    assert_eq!(
        record.assets[0].asset_type,
        sm_application::storage::AssetType::Original
    );
    assert_eq!(
        record.assets[0].format,
        sm_application::storage::CANONICAL_FORMAT_WEBP_LOSSLESS
    );
    f.close(&worker);

    // Committed rows and the canonical file survive the worker restart: the canon is
    // not a cache and no preview/close path can own it.
    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 1);
    assert_eq!(count(&db, "assets"), 1);
    let (context_json, provenances_json): (String, String) = db
        .query_row(
            "SELECT capture_context, provenances FROM records",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(context_json.contains("\"target\":\"region\""));
    assert!(context_json.contains("\"bounds\""));
    assert!(context_json.contains("\"sources\""));
    for member in ["device", "app", "title", "target", "sources", "bounds"] {
        assert!(
            context_json.contains(&format!("\"{member}\"")),
            "missing capture-context member {member}"
        );
    }
    assert!(
        !context_json.contains("\"geometry\""),
        "the captured geometry is the typed `bounds` member, not a second alias"
    );
    assert_eq!(
        provenances_json,
        "[{\"description\":\"PrintScreen\",\"when\":\"2026-09-28T09:15:00.000Z\"}]"
    );
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.page_records(RecordPageQuery::first(None).unwrap()))
            .unwrap()
            .items
            .len(),
        1
    );
    f.close(&worker);
    assert_eq!(fs::read(&published).unwrap(), bytes);
}

#[test]
fn s02_images_03_failed_commit_rolls_back_rows_and_removes_its_own_file() {
    let f = Fixture::new();
    let worker = StorageWorker::start_with_image_fault(f.lease()).unwrap();
    let bytes = canonical(32, 32, 7);
    assert_eq!(
        f.rt(worker.commit_image_capture(draft(&record_uuid(2), &record_uuid(102), bytes))),
        Err(StorageError::Unavailable)
    );
    assert_eq!(
        f.rt(worker.page_records(RecordPageQuery::first(None).unwrap()))
            .unwrap()
            .items
            .len(),
        0
    );
    f.close(&worker);
    assert!(f.published_files().is_empty(), "no orphan canonical file");
    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 0);
    assert_eq!(count(&db, "assets"), 0);
    drop(db);

    // A rollback must not touch an already committed canonical of another record.
    let worker = StorageWorker::start(f.lease()).unwrap();
    let first = f
        .rt(worker.commit_image_capture(draft(
            &record_uuid(3),
            &record_uuid(103),
            canonical(16, 16, 8),
        )))
        .unwrap();
    f.close(&worker);
    let worker = StorageWorker::start_with_image_fault(f.lease()).unwrap();
    assert!(f
        .rt(worker.commit_image_capture(draft(
            &record_uuid(4),
            &record_uuid(104),
            canonical(16, 16, 9),
        )))
        .is_err());
    f.close(&worker);
    let kept = f.root().join(&first.relative_path);
    assert_eq!(fs::read(&kept).unwrap(), canonical(16, 16, 8));
    assert_eq!(f.published_files(), vec![kept]);
    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 1);
    assert_eq!(count(&db, "assets"), 1);
}

#[test]
fn s02_images_04_rejected_payload_never_reaches_the_database_or_the_filesystem() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let mut not_a_container = draft(&record_uuid(5), &record_uuid(105), canonical(8, 8, 10));
    not_a_container.canonical = b"not-a-webp-container".to_vec();
    assert_eq!(
        f.rt(worker.commit_image_capture(not_a_container)),
        Err(StorageError::InvalidPayload)
    );

    let mut short_container = draft(&record_uuid(5), &record_uuid(105), canonical(8, 8, 10));
    short_container
        .canonical
        .truncate(short_container.canonical.len() - 1);
    assert_eq!(
        f.rt(worker.commit_image_capture(short_container)),
        Err(StorageError::InvalidPayload)
    );

    let mut empty_title = draft(&record_uuid(5), &record_uuid(105), canonical(8, 8, 10));
    empty_title.capture_context.title = String::new();
    assert_eq!(
        f.rt(worker.commit_image_capture(empty_title)),
        Err(StorageError::InvalidPayload)
    );

    let mut zero_geometry = draft(&record_uuid(5), &record_uuid(105), canonical(8, 8, 10));
    zero_geometry.capture_context.bounds.width = 0;
    assert_eq!(
        f.rt(worker.commit_image_capture(zero_geometry)),
        Err(StorageError::InvalidPayload)
    );

    let mut no_sources = draft(&record_uuid(5), &record_uuid(105), canonical(8, 8, 10));
    no_sources.capture_context.sources.clear();
    assert_eq!(
        f.rt(worker.commit_image_capture(no_sources)),
        Err(StorageError::InvalidPayload)
    );

    let mut bad_when = draft(&record_uuid(5), &record_uuid(105), canonical(8, 8, 10));
    bad_when.provenances = vec![Provenance::Manual {
        when: "28.09.2026 09:15".to_owned(),
    }];
    assert_eq!(
        f.rt(worker.commit_image_capture(bad_when)),
        Err(StorageError::InvalidPayload)
    );

    let mut no_provenance = draft(&record_uuid(5), &record_uuid(105), canonical(8, 8, 10));
    no_provenance.provenances.clear();
    assert_eq!(
        f.rt(worker.commit_image_capture(no_provenance)),
        Err(StorageError::InvalidPayload)
    );

    assert_eq!(
        f.rt(worker.page_records(RecordPageQuery::first(None).unwrap()))
            .unwrap()
            .items
            .len(),
        0
    );
    f.close(&worker);
    assert!(f.published_files().is_empty());
    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 0);
    assert_eq!(count(&db, "assets"), 0);
}

#[test]
fn s02_images_05_automatic_provenance_records_its_source() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let mut command = draft(&record_uuid(6), &record_uuid(106), canonical(12, 12, 11));
    command.capture_context = context(CaptureTarget::ActiveWindow);
    command.provenances = vec![
        Provenance::Manual {
            when: CAPTURED_AT.to_owned(),
        },
        Provenance::Automatic {
            when: CAPTURED_AT.to_owned(),
            source: CaptureTarget::ActiveWindow,
        },
    ];
    f.rt(worker.commit_image_capture(command)).expect("commit");
    f.close(&worker);
    let db = direct(&f.db());
    let provenances: String = db
        .query_row("SELECT provenances FROM records", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        provenances,
        "[{\"description\":\"PrintScreen\",\"when\":\"2026-09-28T09:15:00.000Z\"},\
         {\"description\":\"Automatic capture\",\"when\":\"2026-09-28T09:15:00.000Z\",\"source\":\"active_window\"}]"
    );
    let context_json: String = db
        .query_row("SELECT capture_context FROM records", [], |row| row.get(0))
        .unwrap();
    assert!(context_json.contains("\"target\":\"active_window\""));
    assert!(context_json.contains("\"sources\":[\"active_window\"]"));
}

#[test]
fn s02_images_06_pages_are_bounded_stable_and_cursor_validated() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    for index in 0..5u64 {
        f.rt(worker.commit_image_capture(draft(
            &record_uuid(10 + index),
            &record_uuid(110 + index),
            canonical(8, 8, 20 + index),
        )))
        .expect("commit");
    }
    let first = f
        .rt(worker.page_records(RecordPageQuery::first(Some(2)).unwrap()))
        .unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.items[0].id.as_uuid().to_string(), record_uuid(14));
    assert_eq!(first.items[1].id.as_uuid().to_string(), record_uuid(13));
    let cursor = first.next_cursor.clone().expect("cursor");

    // A record committed after the snapshot exists is not injected into that snapshot.
    f.rt(worker.commit_image_capture(draft(
        &record_uuid(20),
        &record_uuid(120),
        canonical(8, 8, 40),
    )))
    .expect("commit");
    let second = f
        .rt(worker.page_records(RecordPageQuery::new(Some(2), Some(cursor.clone())).unwrap()))
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|record| record.id.as_uuid().to_string())
            .collect::<Vec<_>>(),
        [record_uuid(12), record_uuid(11)]
    );
    let third = f
        .rt(worker.page_records(RecordPageQuery::new(Some(2), second.next_cursor.clone()).unwrap()))
        .unwrap();
    assert_eq!(
        third
            .items
            .iter()
            .map(|record| record.id.as_uuid().to_string())
            .collect::<Vec<_>>(),
        [record_uuid(10)]
    );
    assert_eq!(
        third.next_cursor, None,
        "an exhausted page carries no cursor"
    );
    let fresh = f
        .rt(worker.page_records(RecordPageQuery::first(Some(2)).unwrap()))
        .unwrap();
    assert_eq!(fresh.items[0].id.as_uuid().to_string(), record_uuid(20));

    // Bounds and cursor validation.
    assert_eq!(
        RecordPageQuery::first(Some(0)),
        Err(StorageError::InvalidQuery)
    );
    assert_eq!(
        RecordPageQuery::first(Some(201)),
        Err(StorageError::InvalidQuery)
    );
    assert_eq!(RecordPageQuery::first(None).unwrap().limit(), 30);
    for bad in [
        "1".to_owned(),
        "v2:01900000-0000-7000-8000-0000000000aa:1:1".to_owned(),
        format!("v1:{}:1", record_uuid(999)),
        format!(
            "v1:{}:01:1",
            f.rt(worker.schema_snapshot()).unwrap().library_id
        ),
        format!(
            "v1:{}:1:-1",
            f.rt(worker.schema_snapshot()).unwrap().library_id
        ),
        format!(
            "v1:{}:1:1:1",
            f.rt(worker.schema_snapshot()).unwrap().library_id
        ),
        format!(
            "v1:{}:99999999999999999999:1",
            f.rt(worker.schema_snapshot()).unwrap().library_id
        ),
    ] {
        assert_eq!(
            f.rt(worker.page_records(RecordPageQuery::new(Some(5), Some(bad.clone())).unwrap())),
            Err(StorageError::InvalidQuery),
            "cursor {bad} must be refused"
        );
    }
    f.close(&worker);
}

#[test]
fn s02_images_07_asset_vocabulary_is_closed_and_a_record_keeps_one_file_per_type() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let record = record_uuid(30);
    let mut first = draft(&record, &record_uuid(130), canonical(8, 8, 50));
    first.asset_type = sm_application::storage::AssetType::Original;
    f.rt(worker.commit_image_capture(first)).expect("original");
    // The three documented asset types are storable; each commit publishes one
    // record with its first immutable file, so the later types use their own record.
    let mut preview = draft(&record_uuid(31), &record_uuid(131), canonical(8, 8, 51));
    preview.asset_type = sm_application::storage::AssetType::Preview;
    f.rt(worker.commit_image_capture(preview)).expect("preview");
    let mut edited = draft(&record_uuid(32), &record_uuid(132), canonical(8, 8, 52));
    edited.asset_type = sm_application::storage::AssetType::Edited;
    f.rt(worker.commit_image_capture(edited)).expect("edited");
    let mut duplicate = draft(&record, &record_uuid(133), canonical(8, 8, 53));
    duplicate.asset_type = sm_application::storage::AssetType::Original;
    assert_eq!(
        f.rt(worker.commit_image_capture(duplicate)),
        Err(StorageError::IntegrityFailed),
        "the path layout keeps exactly one file per record and type"
    );
    let page = f
        .rt(worker.page_records(RecordPageQuery::first(None).unwrap()))
        .unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(
        page.items
            .iter()
            .map(|record| record.assets[0].asset_type.as_str())
            .collect::<Vec<_>>(),
        ["edited", "preview", "original"],
        "records stay ordered by insertion, not by clock"
    );
    assert!(page.items.iter().all(
        |record| record.assets.len() == 1 && record.assets[0].relative_path.ends_with(".webp")
    ));
    assert_eq!(f.published_files().len(), 3);
    f.close(&worker);

    // The DDL vocabulary itself rejects an AVIF row: the format is not silently accepted.
    let db = Connection::open(f.db()).unwrap();
    let rejected = db.execute(
        "INSERT INTO assets (id, record_id, asset_type, format, path, size_bytes, sha256, created_at) \
         VALUES ('01900000-0000-7000-8000-0000000000ff', ?1, 'original', 'avif', \
         'assets/2026-09/00000000-0000-0000-0000-000000000000/original.webp', 10, \
         'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', \
         '2026-09-28T09:15:00.000Z')",
        [&record],
    );
    assert!(rejected.is_err(), "avif must not be a canonical format");
    let escape = db.execute(
        "UPDATE assets SET path = 'assets/2026-09/../../outside.webp'",
        [],
    );
    assert!(escape.is_err(), "traversal must not be storable");
    let absolute = db.execute(
        "UPDATE assets SET path = 'C:/outside/assets/2026-09/x/original.webp'",
        [],
    );
    assert!(absolute.is_err(), "an absolute path must not be storable");
}

#[test]
fn s02_images_08_reader_refuses_tampered_stored_json() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    f.rt(worker.commit_image_capture(draft(
        &record_uuid(40),
        &record_uuid(140),
        canonical(8, 8, 60),
    )))
    .expect("commit");
    f.close(&worker);
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "UPDATE records SET capture_context = '{\"device\":\"x\"}'",
        [],
    )
    .expect("a shape-valid but incomplete context is storable, so the reader must reject it");
    let emptied = db.execute("UPDATE records SET provenances = '[]'", []);
    assert!(
        emptied.is_err(),
        "the array vocabulary requires at least one provenance"
    );
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.page_records(RecordPageQuery::first(None).unwrap())),
        Err(StorageError::IntegrityFailed),
        "a tampered record must be refused, not repaired or invented"
    );
    f.close(&worker);
}

#[test]
fn s02_images_09_upgrade_from_foundation_is_atomic_and_adds_no_rows() {
    let f = Fixture::new();
    let library_id = write_foundation_library(&f);
    let worker = StorageWorker::start(f.lease()).unwrap();
    let snapshot = f.rt(worker.schema_snapshot()).unwrap();
    assert_eq!(snapshot.schema_version, 2);
    assert_eq!(snapshot.library_id, library_id);
    assert!(!f.rt(worker.read_ai_enabled()).unwrap());
    f.close(&worker);
    let db = direct(&f.db());
    assert_eq!(count(&db, "schema_migrations"), 2);
    assert_eq!(count(&db, "records"), 0);
    assert_eq!(count(&db, "assets"), 0);
    let stored: String = db
        .query_row("SELECT library_id FROM library_meta", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, library_id.to_string());
    drop(db);

    // An interrupted upgrade leaves the foundation schema, its rows and its bytes.
    let broken = Fixture::new();
    let library_id = write_foundation_library(&broken);
    assert!(matches!(
        StorageWorker::start_with_fault(broken.lease()),
        Err(StorageError::MigrationFailed)
    ));
    let db = direct(&broken.db());
    assert_eq!(count(&db, "schema_migrations"), 1);
    let images_objects: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('records', 'assets')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(images_objects, 0, "a failed upgrade adds no schema");
    let stored: String = db
        .query_row("SELECT library_id FROM library_meta", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, library_id.to_string());
    drop(db);
    assert!(broken.published_files().is_empty());
    // The retry completes the interrupted upgrade exactly once.
    let worker = StorageWorker::start(broken.lease()).unwrap();
    assert_eq!(f.rt(worker.schema_snapshot()).unwrap().schema_version, 2);
    assert_eq!(
        broken
            .rt(worker.page_records(RecordPageQuery::first(None).unwrap()))
            .unwrap(),
        Page {
            items: Vec::new(),
            next_cursor: None,
        }
    );
    broken.close(&worker);
    let db = direct(&broken.db());
    assert_eq!(count(&db, "schema_migrations"), 2);
}

#[test]
fn s02_images_10_reparse_assets_root_is_refused_without_outside_write() {
    let f = Fixture::new();
    let outside = f.root().join("outside-target");
    fs::create_dir(&outside).unwrap();
    let assets = f.root().join("assets");
    let output = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&assets)
        .arg(&outside)
        .output()
        .expect("junction probe");
    assert!(
        output.status.success(),
        "synthetic junction fixture must be creatable in the owned test root"
    );
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.commit_image_capture(draft(
            &record_uuid(50),
            &record_uuid(150),
            canonical(8, 8, 70),
        ))),
        Err(StorageError::RootRejected),
        "a reparse point is never followed for a canonical write"
    );
    f.close(&worker);
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 0);
    drop(db);
    let _ = std::process::Command::new("cmd")
        .args(["/c", "rmdir"])
        .arg(&assets)
        .output();
}

/// Real foundation-version library: owner marker, shipped 0001 objects and rows.
fn write_foundation_library(fixture: &Fixture) -> Uuid {
    let foundation = include_str!("../migrations/0001_foundation.sql");
    let db = Connection::open(fixture.db()).unwrap();
    db.execute_batch(&format!(
        "PRAGMA application_id={};",
        super::migration::OWNER_ID
    ))
    .unwrap();
    db.execute_batch(super::migration::BOOTSTRAP).unwrap();
    db.execute_batch(foundation).unwrap();
    let library_id = Uuid::parse_str("01900000-0000-7000-8000-0000000000aa").unwrap();
    db.execute(
        "INSERT INTO library_meta VALUES (1, ?1, 1, '2026-09-28T09:00:00.000Z')",
        [library_id.to_string()],
    )
    .unwrap();
    for key in ["ai.enabled", "model.autostart"] {
        db.execute(
            "INSERT INTO library_settings VALUES (?1, 'false', 1)",
            [key],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO schema_migrations VALUES (1, '0001_foundation', ?1, '2026-09-28T09:00:00.000Z')",
        [format!("{:x}", Sha256::digest(foundation.as_bytes()))],
    )
    .unwrap();
    drop(db);
    library_id
}
