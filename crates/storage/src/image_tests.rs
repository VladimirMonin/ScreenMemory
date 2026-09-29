//! S02 image storage: migration, canonical publication, rollback and bounded reads.
//! Every fixture is synthetic and lives under the owned `.pi/temporary` root.
use super::*;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use sm_application::storage::{
    AssetChunk, CaptureContext, CaptureTarget, CommitImageCapture, FoundationRepository,
    LookupOutcome, PhysicalBounds, Provenance, RecordPageQuery, StorageError, ASSET_CHUNK_BYTES,
    MAX_CANONICAL_FILE_BYTES,
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
        operation_id: sm_domain::OperationId::new(Uuid::from_u128(
            0x0190_0000_0000_7000_8000_1000_0000_0000u128
                + u128::from(NEXT.fetch_add(1, Ordering::Relaxed)),
        )),
        principal_id: sm_domain::PrincipalId::new(Uuid::nil()),
        record_id: RecordId::new(Uuid::parse_str(record_id).unwrap()),
        asset_id: AssetId::new(Uuid::parse_str(asset_id).unwrap()),
        asset_type: sm_application::storage::AssetType::Original,
        mode: CaptureTarget::Region,
        width: 320,
        height: 200,
        capture_context: context(CaptureTarget::Region),
        provenances: manual_provenances(),
        warnings: Vec::new(),
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
    assert_eq!(snapshot.schema_version, 3);
    assert_eq!(
        snapshot.images.migration_version, 2,
        "0003 adds a global journal without changing the images schema contract"
    );
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
    assert_eq!(rows.len(), 3);
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
    assert_eq!(rows[2].0, 3);
    assert_eq!(rows[2].1, "0003_operations");
    assert_eq!(
        rows[2].2,
        format!(
            "{:x}",
            Sha256::digest(include_str!("../migrations/0003_operations.sql").as_bytes())
        ),
        "the checksum covers the exact shipped bytes, including the final newline"
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
            "ingest_operations",
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
    assert_eq!(snapshot.schema_version, 3);
    assert_eq!(snapshot.library_id, library_id);
    assert!(!f.rt(worker.read_ai_enabled()).unwrap());
    f.close(&worker);
    let db = direct(&f.db());
    assert_eq!(count(&db, "schema_migrations"), 3);
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
    assert_eq!(f.rt(worker.schema_snapshot()).unwrap().schema_version, 3);
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
    assert_eq!(count(&db, "schema_migrations"), 3);
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

#[test]
fn s02_images_11_bounded_read_returns_only_the_verified_committed_bytes() {
    let f = Fixture::new();
    let lease = f.lease();
    let worker = StorageWorker::start(lease).unwrap();
    let record_id = record_uuid(21);
    let asset_id = record_uuid(121);
    let bytes = canonical(640, 480, 0x51);
    let committed = f
        .rt(worker.commit_image_capture(draft(&record_id, &asset_id, bytes.clone())))
        .unwrap();
    assert_eq!(committed.size_bytes, bytes.len() as u64);
    let id = AssetId::new(Uuid::parse_str(&asset_id).unwrap());
    // The whole file arrives in bounded chunks that reassemble byte for byte.
    let mut collected: Vec<u8> = Vec::new();
    let mut offset = 0u64;
    loop {
        let chunk: AssetChunk = f
            .rt(worker.read_asset_chunk(id, offset, ASSET_CHUNK_BYTES))
            .unwrap();
        assert!(chunk.bytes.len() <= ASSET_CHUNK_BYTES as usize);
        assert_eq!(
            chunk.format,
            sm_application::storage::CANONICAL_FORMAT_WEBP_LOSSLESS
        );
        assert_eq!(chunk.sha256, committed.sha256);
        assert_eq!(chunk.total_size, bytes.len() as u64);
        assert_eq!(chunk.offset, offset);
        collected.extend_from_slice(&chunk.bytes);
        if chunk.eof {
            assert_eq!(chunk.next_offset, chunk.total_size);
            break;
        }
        offset = chunk.next_offset;
    }
    assert_eq!(collected, bytes);
    // An offset beyond the file is a refused query, not an empty success.
    assert_eq!(
        f.rt(worker.read_asset_chunk(id, bytes.len() as u64 + 1, 16)),
        Err(StorageError::InvalidQuery)
    );
    // An unknown asset is reported as missing.
    assert_eq!(
        f.rt(worker.read_asset_chunk(AssetId::new(Uuid::from_u128(0xbad)), 0, 16)),
        Err(StorageError::AssetNotFound)
    );
    f.close(&worker);
}

#[test]
fn s02_images_12_substituted_or_truncated_canonical_file_is_refused() {
    let f = Fixture::new();
    let lease = f.lease();
    let worker = StorageWorker::start(lease).unwrap();
    let record_id = record_uuid(22);
    let asset_id = record_uuid(122);
    let bytes = canonical(320, 200, 0x52);
    f.rt(worker.commit_image_capture(draft(&record_id, &asset_id, bytes.clone())))
        .unwrap();
    let id = AssetId::new(Uuid::parse_str(&asset_id).unwrap());
    assert!(f.rt(worker.read_asset_chunk(id, 0, 64)).is_ok());
    let published = f.published_files();
    assert_eq!(published.len(), 1);
    // Same length, different bytes: the committed digest must catch the substitution.
    let mut substituted = bytes.clone();
    let last = substituted.len() - 1;
    substituted[last] ^= 0xff;
    fs::write(&published[0], &substituted).unwrap();
    assert_eq!(
        f.rt(worker.read_asset_chunk(id, 0, 64)),
        Err(StorageError::AssetIntegrityFailed)
    );
    // Truncated file: refused by the committed size before any digest claim.
    fs::write(&published[0], &bytes[..bytes.len() - 4]).unwrap();
    assert_eq!(
        f.rt(worker.read_asset_chunk(id, 0, 64)),
        Err(StorageError::AssetIntegrityFailed)
    );
    // A deleted canonical file is reported as missing, never as an empty document.
    fs::remove_file(&published[0]).unwrap();
    assert_eq!(
        f.rt(worker.read_asset_chunk(id, 0, 64)),
        Err(StorageError::AssetNotFound)
    );
    f.close(&worker);
}

/// Real S02 images-version library: owner marker, shipped 0001+0002 objects, rows and
/// exactly two migration rows, so the next open is a genuine v2 -> v3 upgrade.
fn write_images_v2_library(fixture: &Fixture) -> Uuid {
    let foundation = include_str!("../migrations/0001_foundation.sql");
    let images = include_str!("../migrations/0002_images.sql");
    let db = Connection::open(fixture.db()).unwrap();
    db.execute_batch(&format!(
        "PRAGMA application_id={};",
        super::migration::OWNER_ID
    ))
    .unwrap();
    db.execute_batch(super::migration::BOOTSTRAP).unwrap();
    db.execute_batch(foundation).unwrap();
    db.execute_batch(images).unwrap();
    let library_id = Uuid::parse_str("01900000-0000-7000-8000-0000000000bb").unwrap();
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
    db.execute(
        "INSERT INTO schema_migrations VALUES (2, '0002_images', ?1, '2026-09-28T09:00:00.000Z')",
        [format!("{:x}", Sha256::digest(images.as_bytes()))],
    )
    .unwrap();
    drop(db);
    library_id
}

#[test]
fn s02_t09a_13_first_successful_commit_writes_a_complete_saved_journal_row() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let command = draft(&record_uuid(60), &record_uuid(160), canonical(8, 8, 80));
    let operation_id = command.operation_id;
    let principal_id = command.principal_id;
    let bytes = command.canonical.clone();
    let receipt = f
        .rt(worker.commit_image_capture(command))
        .expect("the first capture commits");
    f.close(&worker);

    // The saved row must agree with the images schema: a NOT NULL fingerprint, the
    // planned ids that were reserved before the file write, the committed record and the
    // replayable receipt metadata (mode, positive geometry, bounded warnings).
    let db = direct(&f.db());
    assert_eq!(count(&db, "ingest_operations"), 1);
    let (
        operation,
        principal,
        planned_record,
        planned_asset,
        fingerprint,
        saved_record,
        state,
        mode,
        width,
        height,
        warnings,
    ): (String, String, String, String, String, String, String, String, i64, i64, String) = db
        .query_row(
            "SELECT operation_id, principal_id, planned_record_id, planned_asset_id, content_sha256, record_id, state, mode, width, height, warnings FROM ingest_operations",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(operation, operation_id.as_uuid().to_string());
    assert_eq!(principal, principal_id.as_uuid().to_string());
    assert_eq!(planned_record, record_uuid(60));
    assert_eq!(planned_asset, record_uuid(160));
    assert_eq!(fingerprint, format!("{:x}", Sha256::digest(&bytes)));
    assert_eq!(fingerprint, receipt.sha256);
    assert_eq!(saved_record, record_uuid(60));
    assert_eq!(state, "saved");
    assert_eq!(mode, "region");
    assert_eq!((width, height), (320, 200));
    assert_eq!(warnings, "[]");
    let (assets, records): (i64, i64) = (count(&db, "assets"), count(&db, "records"));
    assert_eq!((records, assets), (1, 1));
    drop(db);

    // A replay of the same operation returns the identical committed receipt, including
    // the metadata recovered from the stored row rather than the retry's own values.
    let worker = StorageWorker::start(f.lease()).unwrap();
    let replay = replay_receipt(
        f.rt(worker.lookup_operation(operation_id, principal_id, CaptureTarget::Region))
            .unwrap(),
    );
    assert_eq!(replay, receipt);
    assert_eq!(replay.mode, CaptureTarget::Region);
    assert_eq!((replay.width, replay.height), (320, 200));
    assert_eq!(replay.canonical_mime, "image/webp");
    assert_eq!(replay.warnings, Vec::<String>::new());
    f.close(&worker);
}

#[test]
fn s02_t09a_14_v2_upgrade_to_v3_is_atomic_reopenable_and_keeps_images_contract() {
    let f = Fixture::new();
    let library_id = write_images_v2_library(&f);
    let worker = StorageWorker::start(f.lease()).unwrap();
    let snapshot = f.rt(worker.schema_snapshot()).unwrap();
    assert_eq!(snapshot.schema_version, 3);
    assert_eq!(snapshot.library_id, library_id);
    assert_eq!(
        snapshot.images.migration_version, 2,
        "0003 adds a global journal only; the images schema contract is unchanged"
    );
    f.close(&worker);

    let db = direct(&f.db());
    assert_eq!(count(&db, "schema_migrations"), 3);
    assert_eq!(count(&db, "ingest_operations"), 0);
    let checksum: String = db
        .query_row(
            "SELECT checksum FROM schema_migrations WHERE version = 3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        checksum,
        format!(
            "{:x}",
            Sha256::digest(include_str!("../migrations/0003_operations.sql").as_bytes())
        )
    );
    drop(db);

    // Reopening reports the identical snapshot: the upgrade is durable and idempotent.
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(f.rt(worker.schema_snapshot()).unwrap(), snapshot);
    f.close(&worker);

    // An interrupted upgrade must leave the readable v2 library and its rows intact.
    let broken = Fixture::new();
    let library_id = write_images_v2_library(&broken);
    assert!(matches!(
        StorageWorker::start_with_fault(broken.lease()),
        Err(StorageError::MigrationFailed)
    ));
    let db = direct(&broken.db());
    assert_eq!(count(&db, "schema_migrations"), 2);
    let journal_objects: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'ingest_operations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        journal_objects, 0,
        "a failed v2 -> v3 upgrade adds no schema"
    );
    let stored: String = db
        .query_row("SELECT library_id FROM library_meta", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, library_id.to_string());
    drop(db);
    // The retry completes the interrupted upgrade exactly once.
    let worker = StorageWorker::start(broken.lease()).unwrap();
    assert_eq!(
        broken.rt(worker.schema_snapshot()).unwrap().schema_version,
        3
    );
    broken.close(&worker);
    let db = direct(&broken.db());
    assert_eq!(count(&db, "schema_migrations"), 3);
    assert_eq!(count(&db, "ingest_operations"), 0);
}

/// A draft whose operation and principal are fixed by the caller, so a retry after a
/// lost reply can carry fresh random resource ids for the same accepted capture.
fn scoped_draft(
    operation: u128,
    principal: u128,
    record_id: &str,
    asset_id: &str,
    bytes: Vec<u8>,
) -> CommitImageCapture {
    let mut command = draft(record_id, asset_id, bytes);
    command.operation_id = sm_domain::OperationId::new(Uuid::from_u128(operation));
    command.principal_id = sm_domain::PrincipalId::new(Uuid::from_u128(principal));
    command
}

/// Unwrap the receipt of a successful replay lookup; a non-`Saved` outcome panics,
/// so a conflict or an unknown can never be mistaken for the expected receipt.
fn replay_receipt(outcome: LookupOutcome) -> sm_application::storage::CommittedImage {
    match outcome {
        LookupOutcome::Saved(receipt) => receipt,
        other => panic!("expected a saved replay receipt, got {other:?}"),
    }
}

const T09A_OPERATION: u128 = 0x0190_0000_0000_7000_8000_0000_0000_09a1;
const T09A_PRINCIPAL: u128 = 0x0190_0000_0000_7000_8000_0000_0000_09a2;
const T09A_INTRUDER: u128 = 0x0190_0000_0000_7000_8000_0000_0000_09a3;

#[test]
fn s02_t09a_16_replay_with_fresh_random_ids_returns_the_original_receipt() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let principal = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let bytes = canonical(64, 48, 0x61);
    let receipt = f
        .rt(worker.commit_image_capture(scoped_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(80),
            &record_uuid(180),
            bytes.clone(),
        )))
        .expect("first commit");

    // The retry repeats the semantic capture but, like the host, generates new ids.
    let retry = scoped_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(81),
        &record_uuid(181),
        bytes,
    );
    assert_ne!(retry.record_id, receipt.record_id);
    assert_ne!(retry.asset_id, receipt.asset_id);
    let replay = f
        .rt(worker.commit_image_capture(retry))
        .expect("replay returns the committed receipt");
    assert_eq!(replay, receipt);
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Saved(receipt.clone())
    );
    f.close(&worker);

    // Exactly one record, one asset, one journal row and one canonical file exist:
    // the replay neither duplicated the resource nor re-published a file.
    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 1);
    assert_eq!(count(&db, "assets"), 1);
    assert_eq!(count(&db, "ingest_operations"), 1);
    drop(db);
    assert_eq!(f.published_files().len(), 1);
}

#[test]
fn s02_t09a_17_semantic_mismatch_is_a_safe_conflict_without_new_rows() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let committed = canonical(64, 48, 0x62);
    f.rt(worker.commit_image_capture(scoped_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(82),
        &record_uuid(182),
        committed.clone(),
    )))
    .expect("first commit");

    // Same operation id, different pixels: conflict, not the old receipt. The seeds
    // differ by more than the low bit, so the synthetic pixels really differ after
    // the codec's own `seed | 1` normalisation.
    assert_eq!(
        f.rt(worker.commit_image_capture(scoped_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(83),
            &record_uuid(183),
            canonical(64, 48, 0x64),
        ))),
        Err(StorageError::IntegrityFailed),
        "changed pixels must not replay the old capture"
    );

    // Same pixels, different capture context: conflict.
    let mut changed_context = scoped_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(84),
        &record_uuid(184),
        committed.clone(),
    );
    changed_context.capture_context = context(CaptureTarget::Desktop);
    assert_eq!(
        f.rt(worker.commit_image_capture(changed_context)),
        Err(StorageError::IntegrityFailed)
    );

    // Same pixels and context, different provenance: conflict.
    let mut changed_provenance = scoped_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(85),
        &record_uuid(185),
        committed,
    );
    changed_provenance.provenances = vec![Provenance::Manual {
        when: "2026-09-28T09:16:00.000Z".to_owned(),
    }];
    assert_eq!(
        f.rt(worker.commit_image_capture(changed_provenance)),
        Err(StorageError::IntegrityFailed)
    );
    f.close(&worker);

    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 1);
    assert_eq!(count(&db, "assets"), 1);
    assert_eq!(count(&db, "ingest_operations"), 1);
    drop(db);
    assert_eq!(f.published_files().len(), 1);
}

#[test]
fn s02_t09a_18_cross_principal_replay_never_discloses_the_receipt() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let owner = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let intruder = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_INTRUDER));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let receipt = f
        .rt(worker.commit_image_capture(scoped_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(86),
            &record_uuid(186),
            canonical(64, 48, 0x64),
        )))
        .expect("owner commit");
    assert_eq!(
        f.rt(worker.lookup_operation(operation, owner, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Saved(receipt)
    );

    // The scope is the credential, not the operation id: a foreign principal sees
    // neither the receipt nor any metadata of the committed capture.
    assert_eq!(
        f.rt(worker.lookup_operation(operation, intruder, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Conflict,
        "a foreign principal must never receive another principal's receipt"
    );
    let mut foreign = scoped_draft(
        T09A_OPERATION,
        T09A_INTRUDER,
        &record_uuid(87),
        &record_uuid(187),
        canonical(64, 48, 0x64),
    );
    foreign.capture_context = context(CaptureTarget::Region);
    assert_eq!(
        f.rt(worker.commit_image_capture(foreign)),
        Err(StorageError::IntegrityFailed),
        "a foreign principal must not take over an owned operation id"
    );
    f.close(&worker);

    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 1);
    assert_eq!(count(&db, "assets"), 1);
    assert_eq!(count(&db, "ingest_operations"), 1);
    drop(db);
    assert_eq!(f.published_files().len(), 1);
}

#[test]
fn s02_t09a_19_absent_lookup_is_none_and_a_rolled_back_commit_leaves_no_journal() {
    let f = Fixture::new();
    let principal = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    // A genuinely unknown operation is Unknown: not an error and not a fabricated receipt.
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Unknown
    );
    f.close(&worker);

    // A commit that fails before its durable commit must persist no journal row, so a
    // later lookup still reports the operation as unknown.
    let worker = StorageWorker::start_with_image_fault(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.commit_image_capture(scoped_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(88),
            &record_uuid(188),
            canonical(64, 48, 0x65),
        ))),
        Err(StorageError::Unavailable)
    );
    f.close(&worker);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Unknown
    );
    f.close(&worker);

    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 0);
    assert_eq!(count(&db, "assets"), 0);
    assert_eq!(count(&db, "ingest_operations"), 0);
    drop(db);
    assert!(f.published_files().is_empty());
}

#[test]
fn s02_t09a_15_failed_commit_leaves_no_journal_row_and_no_orphan() {
    let f = Fixture::new();
    let worker = StorageWorker::start_with_image_fault(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.commit_image_capture(draft(
            &record_uuid(70),
            &record_uuid(170),
            canonical(8, 8, 90),
        ))),
        Err(StorageError::Unavailable)
    );
    f.close(&worker);
    assert!(f.published_files().is_empty(), "no orphan canonical file");
    let db = direct(&f.db());
    assert_eq!(count(&db, "records"), 0);
    assert_eq!(count(&db, "assets"), 0);
    assert_eq!(
        count(&db, "ingest_operations"),
        0,
        "the journal row rolls back with the record, asset and file"
    );
}

/// A fixed receipt-metadata draft for the roundtrip tests: a non-region mode, a positive
/// geometry and two stable diagnostic codes, all stored atomically with the saved row.
fn metadata_draft(
    operation: u128,
    principal: u128,
    record_id: &str,
    asset_id: &str,
    bytes: Vec<u8>,
) -> CommitImageCapture {
    let mut command = scoped_draft(operation, principal, record_id, asset_id, bytes);
    command.mode = CaptureTarget::Monitor;
    command.capture_context = context(CaptureTarget::Monitor);
    command.width = 1920;
    command.height = 1080;
    command.warnings = vec!["display_unknown".to_owned(), "title_unknown".to_owned()];
    command
}

#[test]
fn s02_t09a_20_saved_metadata_round_trips_after_reopen() {
    let f = Fixture::new();
    let principal = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    let receipt = f
        .rt(worker.commit_image_capture(metadata_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(90),
            &record_uuid(190),
            canonical(64, 48, 0x70),
        )))
        .expect("first commit");
    assert_eq!(receipt.mode, CaptureTarget::Monitor);
    assert_eq!((receipt.width, receipt.height), (1920, 1080));
    assert_eq!(receipt.canonical_mime, "image/webp");
    assert_eq!(
        receipt.warnings,
        vec!["display_unknown".to_owned(), "title_unknown".to_owned()]
    );
    f.close(&worker);

    // Reopening the library recovers exactly the stored metadata, not a fresh value.
    let worker = StorageWorker::start(f.lease()).unwrap();
    let found = f
        .rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor))
        .unwrap();
    assert_eq!(found, LookupOutcome::Saved(receipt.clone()));
    match found {
        LookupOutcome::Saved(saved) => {
            assert_eq!(saved.mode, CaptureTarget::Monitor);
            assert_eq!(saved.width, 1920);
            assert_eq!(saved.height, 1080);
            assert_eq!(saved.canonical_mime, "image/webp");
            assert_eq!(saved.warnings.len(), 2);
            assert_eq!(saved.relative_path, receipt.relative_path);
        }
        other => panic!("expected a saved receipt, got {other:?}"),
    }

    // A replay under the same mode still returns the recorded warnings, never the
    // retry's own (which are deliberately empty here).
    let mut retry = metadata_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(91),
        &record_uuid(191),
        canonical(64, 48, 0x70),
    );
    retry.warnings = Vec::new();
    assert_eq!(
        f.rt(worker.commit_image_capture(retry)).unwrap(),
        receipt,
        "a replay returns the stored warnings of the committed capture"
    );
    f.close(&worker);

    let db = direct(&f.db());
    assert_eq!(count(&db, "ingest_operations"), 1);
    assert_eq!(count(&db, "records"), 1);
}

#[test]
fn s02_t09a_21_changed_mode_is_a_safe_conflict() {
    let f = Fixture::new();
    let principal = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    let receipt = f
        .rt(worker.commit_image_capture(metadata_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(92),
            &record_uuid(192),
            canonical(64, 48, 0x71),
        )))
        .expect("first commit");

    // Same owner, same operation id, a different requested mode: same safe conflict with
    // no receipt metadata, not the stored receipt and not an unknown.
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Conflict,
        "a changed mode must not replay the stored receipt"
    );
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Desktop))
            .unwrap(),
        LookupOutcome::Conflict
    );
    assert_eq!(
        replay_receipt(
            f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor))
                .unwrap(),
        ),
        receipt,
        "the owner's own mode still replays the stored receipt"
    );

    // A commit retry under a changed mode is refused as a semantic mismatch and writes
    // no second row or file.
    assert_eq!(
        f.rt(worker.commit_image_capture(scoped_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(93),
            &record_uuid(193),
            canonical(64, 48, 0x71),
        ))),
        Err(StorageError::IntegrityFailed)
    );
    f.close(&worker);

    let db = direct(&f.db());
    assert_eq!(count(&db, "ingest_operations"), 1);
    assert_eq!(count(&db, "records"), 1);
    assert_eq!(count(&db, "assets"), 1);
    drop(db);
    assert_eq!(f.published_files().len(), 1);
}

#[test]
fn s02_t09a_22_foreign_principal_is_a_safe_conflict() {
    let f = Fixture::new();
    let owner = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let intruder = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_INTRUDER));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    let receipt = f
        .rt(worker.commit_image_capture(metadata_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(94),
            &record_uuid(194),
            canonical(64, 48, 0x72),
        )))
        .expect("owner commit");

    // The intruder and the owner use the same mode, so only the principal differs: the
    // result is the same safe conflict and the stored receipt is never disclosed.
    assert_eq!(
        f.rt(worker.lookup_operation(operation, intruder, CaptureTarget::Monitor))
            .unwrap(),
        LookupOutcome::Conflict,
        "a foreign principal must never receive another principal's receipt"
    );
    assert_eq!(
        f.rt(worker.lookup_operation(operation, owner, CaptureTarget::Monitor))
            .unwrap(),
        LookupOutcome::Saved(receipt)
    );
    f.close(&worker);
}

#[test]
fn s02_t09a_23_malformed_saved_metadata_is_refused_not_fabricated() {
    let f = Fixture::new();
    let principal = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    f.rt(worker.commit_image_capture(metadata_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(95),
        &record_uuid(195),
        canonical(64, 48, 0x73),
    )))
    .expect("first commit");
    f.close(&worker);

    // The stored warnings are a shape-valid JSON array but fail the bounded code
    // validation, so the reader must refuse the row rather than invent a receipt.
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "UPDATE ingest_operations SET warnings = '[\"Not A Code\"]'",
        [],
    )
    .expect("a shape-valid but invalid warning code is storable for the tamper probe");
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor)),
        Err(StorageError::IntegrityFailed),
        "a tampered saved row is refused, never repaired into a receipt"
    );
    f.close(&worker);

    // An out-of-vocabulary mode cannot even be written: the schema CHECK belongs to the
    // journal contract, so the tamper probe is rejected by the database itself.
    let db = Connection::open(f.db()).unwrap();
    assert!(
        db.execute("UPDATE ingest_operations SET mode = 'nonsense'", [])
            .is_err(),
        "the closed mode vocabulary is enforced by the journal schema"
    );
}

#[test]
fn s02_t09a_24_unknown_operation_is_unknown_and_not_a_conflict() {
    let f = Fixture::new();
    let principal = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    // Only a truly absent id is Unknown; it is never signalled as a conflict.
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Unknown
    );
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor))
            .unwrap(),
        LookupOutcome::Unknown
    );
    f.close(&worker);
}

/// Draft of an `active_window` capture: the typed mode and the context target use the
/// same closed vocabulary the journal `CHECK` enforces.
fn active_window_draft(
    operation: u128,
    principal: u128,
    record_id: &str,
    asset_id: &str,
    bytes: Vec<u8>,
) -> CommitImageCapture {
    let mut command = scoped_draft(operation, principal, record_id, asset_id, bytes);
    command.mode = CaptureTarget::ActiveWindow;
    command.capture_context = context(CaptureTarget::ActiveWindow);
    command
}

#[test]
fn s02_t09a_25_active_window_commit_reopen_replay_is_saved() {
    let f = Fixture::new();
    let principal = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    let receipt = f
        .rt(worker.commit_image_capture(active_window_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(96),
            &record_uuid(196),
            canonical(64, 48, 0x74),
        )))
        .expect("an active_window capture must satisfy the journal mode CHECK");
    assert_eq!(receipt.mode, CaptureTarget::ActiveWindow);
    f.close(&worker);

    // The stored row uses the typed vocabulary, not the wire alias.
    let db = direct(&f.db());
    let stored: String = db
        .query_row("SELECT mode FROM ingest_operations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, "active_window");
    drop(db);

    // Reopening replays the committed receipt under the same typed mode.
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::ActiveWindow))
            .unwrap(),
        LookupOutcome::Saved(receipt),
        "an ActiveWindow capture must survive reopen and replay as Saved"
    );
    f.close(&worker);
}

#[test]
fn s02_t09a_26_journal_mode_check_rejects_the_wire_alias_and_accepts_the_typed_name() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    f.rt(worker.commit_image_capture(active_window_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(97),
        &record_uuid(197),
        canonical(64, 48, 0x75),
    )))
    .expect("first commit");
    f.close(&worker);

    let db = Connection::open(f.db()).unwrap();
    // The wire alias `window` is not the stored vocabulary: the schema refuses it, so a
    // row can never carry a mode the typed reader cannot parse back.
    assert!(
        db.execute("UPDATE ingest_operations SET mode = 'window'", [])
            .is_err(),
        "the wire alias must not be a storable journal mode"
    );
    // An entirely unknown value is refused for the same reason.
    assert!(
        db.execute("UPDATE ingest_operations SET mode = 'nonsense'", [])
            .is_err(),
        "an out-of-vocabulary mode must be refused by the schema CHECK"
    );
    // The typed name is the accepted spelling.
    db.execute("UPDATE ingest_operations SET mode = 'active_window'", [])
        .expect("the typed CaptureTarget name is the stored vocabulary");
    let stored: String = db
        .query_row("SELECT mode FROM ingest_operations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, "active_window");
}

#[test]
fn s02_t09a_27_replay_of_missing_or_substituted_asset_is_an_integrity_failure() {
    let f = Fixture::new();
    let principal = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    let receipt = f
        .rt(worker.commit_image_capture(metadata_draft(
            T09A_OPERATION,
            T09A_PRINCIPAL,
            &record_uuid(98),
            &record_uuid(198),
            canonical(64, 48, 0x76),
        )))
        .expect("first commit");
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor))
            .unwrap(),
        LookupOutcome::Saved(receipt.clone())
    );
    f.close(&worker);

    let files = f.published_files();
    assert_eq!(files.len(), 1);
    let canonical_path = files[0].clone();
    let original = fs::read(&canonical_path).unwrap();

    // A deleted canonical file is a typed integrity failure on replay, never `Saved` and
    // never an invented `Unknown`.
    fs::remove_file(&canonical_path).unwrap();
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor)),
        Err(StorageError::IntegrityFailed),
        "a saved row whose canonical file is gone must fail closed"
    );
    // The bounded read reports the same absence with its own typed code.
    assert_eq!(
        f.rt(worker.read_asset_chunk(receipt.asset_id, 0, ASSET_CHUNK_BYTES)),
        Err(StorageError::AssetNotFound)
    );
    f.close(&worker);

    // A substituted file of a different size/digest is refused too, with the precise
    // typed integrity code for a committed asset that no longer matches its row.
    fs::write(&canonical_path, b"substituted").unwrap();
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor)),
        Err(StorageError::AssetIntegrityFailed),
        "a substituted canonical file must not replay as Saved"
    );
    assert_eq!(
        f.rt(worker.read_asset_chunk(receipt.asset_id, 0, ASSET_CHUNK_BYTES)),
        Err(StorageError::AssetIntegrityFailed)
    );
    f.close(&worker);

    // Restoring the exact committed bytes restores the honest replay.
    fs::write(&canonical_path, &original).unwrap();
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor))
            .unwrap(),
        LookupOutcome::Saved(receipt)
    );
    f.close(&worker);
}

#[test]
fn s02_t09a_28_foreign_and_mode_mismatch_stay_safe_even_if_the_asset_is_invalid() {
    let f = Fixture::new();
    let owner = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let intruder = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_INTRUDER));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    f.rt(worker.commit_image_capture(metadata_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(99),
        &record_uuid(199),
        canonical(64, 48, 0x77),
    )))
    .expect("owner commit");
    f.close(&worker);

    // Make the stored asset invalid before the foreign/mode probes: the checks that
    // decide conflict must run before any file IO, so neither probe may observe the
    // corruption or leak that the stored asset is unreadable.
    let files = f.published_files();
    assert_eq!(files.len(), 1);
    fs::remove_file(&files[0]).unwrap();

    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, intruder, CaptureTarget::Monitor))
            .unwrap(),
        LookupOutcome::Conflict,
        "a foreign principal must see the same safe conflict, never the integrity failure"
    );
    assert_eq!(
        f.rt(worker.lookup_operation(operation, owner, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Conflict,
        "a mode mismatch must be decided before the asset is touched"
    );
    // The owner's own matching request still fails closed on the missing asset.
    assert_eq!(
        f.rt(worker.lookup_operation(operation, owner, CaptureTarget::Monitor)),
        Err(StorageError::IntegrityFailed)
    );
    f.close(&worker);
}

#[test]
fn s02_t09a_29_commit_replay_of_missing_or_substituted_asset_is_an_integrity_failure() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let draft = metadata_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(100),
        &record_uuid(200),
        canonical(64, 48, 0x78),
    );
    let receipt = f
        .rt(worker.commit_image_capture(draft.clone()))
        .expect("first commit");
    // An honest retry of the same accepted capture still returns the committed receipt.
    assert_eq!(
        f.rt(worker.commit_image_capture(draft.clone())).unwrap(),
        receipt
    );
    f.close(&worker);

    let files = f.published_files();
    assert_eq!(files.len(), 1);
    let canonical_path = files[0].clone();
    let original = fs::read(&canonical_path).unwrap();

    // A deleted canonical file must not let the public commit retry report `Saved`.
    fs::remove_file(&canonical_path).unwrap();
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.commit_image_capture(draft.clone())),
        Err(StorageError::IntegrityFailed),
        "a replay whose canonical file is gone must fail closed, never fabricate Saved"
    );
    f.close(&worker);

    // A substituted file is refused with the precise typed integrity code, in both the
    // pre-transaction and the in-transaction replay branch.
    fs::write(&canonical_path, b"substituted").unwrap();
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.commit_image_capture(draft.clone())),
        Err(StorageError::AssetIntegrityFailed),
        "a substituted canonical file must not replay as Saved"
    );
    f.close(&worker);

    // Restoring the exact committed bytes restores the honest replay.
    fs::write(&canonical_path, &original).unwrap();
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(f.rt(worker.commit_image_capture(draft)).unwrap(), receipt);
    f.close(&worker);
}

#[test]
fn s02_t09a_30_wrong_mode_stays_conflict_even_if_saved_metadata_is_corrupt() {
    let f = Fixture::new();
    let owner = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let intruder = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_INTRUDER));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    f.rt(worker.commit_image_capture(metadata_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(101),
        &record_uuid(201),
        canonical(64, 48, 0x79),
    )))
    .expect("first commit");
    f.close(&worker);

    // Shape-valid JSON that fails the bounded warning-code validation: the receipt
    // deserialization would refuse it with an integrity failure.
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "UPDATE ingest_operations SET warnings = '[\"Not A Code\"]'",
        [],
    )
    .expect("a shape-valid but invalid warning code is storable for the tamper probe");
    drop(db);

    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, intruder, CaptureTarget::Monitor))
            .unwrap(),
        LookupOutcome::Conflict,
        "a foreign principal is decided by ownership, before any stored metadata"
    );
    assert_eq!(
        f.rt(worker.lookup_operation(operation, owner, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Conflict,
        "a mode mismatch must not leak the corrupt-receipt integrity failure"
    );
    // The owner's own matching mode still refuses the corrupt row.
    assert_eq!(
        f.rt(worker.lookup_operation(operation, owner, CaptureTarget::Monitor)),
        Err(StorageError::IntegrityFailed)
    );
    f.close(&worker);

    // Same probe for a saved row whose joined committed asset is gone entirely: the
    // wrong-mode request is still the plain safe conflict.
    let db = Connection::open(f.db()).unwrap();
    db.execute("UPDATE ingest_operations SET warnings = '[]'", [])
        .expect("the warning column is restored to a well-formed array");
    db.execute("DELETE FROM assets", [])
        .expect("the joined committed asset row is removed for the probe");
    drop(db);

    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, owner, CaptureTarget::Region))
            .unwrap(),
        LookupOutcome::Conflict,
        "a mode mismatch must not leak the missing joined asset"
    );
    assert_eq!(
        f.rt(worker.lookup_operation(operation, owner, CaptureTarget::Monitor)),
        Err(StorageError::IntegrityFailed),
        "the matching request still reports the corrupt saved row"
    );
    f.close(&worker);
}

#[test]
fn s02_t09a_31_oversized_stored_size_or_file_is_refused_before_any_read() {
    let f = Fixture::new();
    let principal = sm_domain::PrincipalId::new(Uuid::from_u128(T09A_PRINCIPAL));
    let operation = sm_domain::OperationId::new(Uuid::from_u128(T09A_OPERATION));
    let worker = StorageWorker::start(f.lease()).unwrap();
    let draft = metadata_draft(
        T09A_OPERATION,
        T09A_PRINCIPAL,
        &record_uuid(102),
        &record_uuid(202),
        canonical(64, 48, 0x7a),
    );
    let receipt = f
        .rt(worker.commit_image_capture(draft.clone()))
        .expect("first commit");
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor))
            .unwrap(),
        LookupOutcome::Saved(receipt.clone())
    );
    f.close(&worker);

    let files = f.published_files();
    assert_eq!(files.len(), 1);
    let canonical_path = files[0].clone();
    let original = fs::read(&canonical_path).unwrap();

    // A stored committed size above the bound is refused before any IO, so an oversized
    // row can never drive an unbounded read into memory on a replay
    // (`commit_image_capture` or `lookup_operation`).
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "UPDATE assets SET size_bytes = ?1",
        [i64::try_from(MAX_CANONICAL_FILE_BYTES + 1).unwrap()],
    )
    .expect("the oversized committed size is storable for the probe");
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor)),
        Err(StorageError::IntegrityFailed),
        "an oversized stored size must fail before the file is read"
    );
    assert_eq!(
        f.rt(worker.commit_image_capture(draft.clone())),
        Err(StorageError::IntegrityFailed),
        "the commit replay is bounded by the same stored-size check"
    );
    f.close(&worker);

    // A file that is obviously larger than the bound is refused from its metadata length,
    // before it is read at all, even when the stored size is honest.
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "UPDATE assets SET size_bytes = ?1",
        [i64::try_from(original.len()).unwrap()],
    )
    .expect("the honest committed size is restored");
    drop(db);
    let oversized = fs::File::create(&canonical_path).unwrap();
    oversized
        .set_len(MAX_CANONICAL_FILE_BYTES + 1)
        .expect("a sparse oversized file is created for the probe");
    drop(oversized);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor)),
        Err(StorageError::AssetIntegrityFailed),
        "an oversized file must be refused from its metadata length"
    );
    assert_eq!(
        f.rt(worker.commit_image_capture(draft.clone())),
        Err(StorageError::AssetIntegrityFailed),
        "the commit replay refuses the oversized file as well"
    );
    f.close(&worker);

    // Restoring the exact committed bytes keeps the honest replay unchanged.
    fs::write(&canonical_path, &original).unwrap();
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.lookup_operation(operation, principal, CaptureTarget::Monitor))
            .unwrap(),
        LookupOutcome::Saved(receipt.clone())
    );
    assert_eq!(f.rt(worker.commit_image_capture(draft)).unwrap(), receipt);
    f.close(&worker);
}
