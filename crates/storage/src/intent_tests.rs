//! Synthetic NTFS roots only; no native capture or canonical file publication.
use super::*;
use rusqlite::params;
use sha2::{Digest, Sha256};
use sm_application::{
    capture::{CaptureRequest, CaptureTrigger},
    storage::{CreateCaptureIntent, CreateIntentOutcome, PhysicalBounds, ReadIntentOutcome},
};
use sm_isolation::{LibraryRootResolver, RootSource};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = base.join(format!("s02-intent-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        for part in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(part)).unwrap();
        }
        Self(root)
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
    fn db(&self) -> PathBuf {
        self.0.join("data/library.sqlite3")
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
    fn assert_no_output(&self) {
        assert!(!self.0.join("assets").exists());
        assert!(!self.0.join("staging").exists());
        let db = Connection::open(self.db()).unwrap();
        for table in ["records", "assets", "ingest_operations"] {
            let count: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn input(library_id: LibraryId) -> CreateCaptureIntent {
    let principal = Sha256::digest(b"synthetic scoped principal non-rfc");
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&principal[..16]);
    // CredentialKey::principal uses the unchanged SHA-256 prefix, including variant bits.
    let principal_id = PrincipalId::new(Uuid::from_bytes(bytes));
    assert_eq!(
        principal_id.as_uuid().to_string(),
        "09c2f399-89bd-9184-4710-b40d1c5adb53"
    );
    let operation_id = Uuid::from_u128(0x0190_0000_0000_7000_0000_0000_0000_2026);
    let target = CaptureTarget::Region;
    CreateCaptureIntent {
        library_id,
        principal_id,
        request: CaptureRequest::new(
            operation_id,
            sm_application::capture::CaptureMode::Region,
            CaptureTrigger::ClientCommand,
        ),
        record_id: RecordId::new(Uuid::new_v4()),
        asset_id: AssetId::new(Uuid::new_v4()),
        frame_id: Uuid::new_v4(),
        width: 3,
        height: 2,
        pixel_sha256: format!("{:x}", Sha256::digest([0u8; 24])),
        context: CaptureContext {
            device: "synthetic-device".into(),
            app: "synthetic-app".into(),
            title: "synthetic".into(),
            target,
            sources: vec![target],
            bounds: PhysicalBounds {
                x: -5,
                y: 0,
                width: 3,
                height: 2,
            },
        },
        warnings: vec!["display_unknown".into(), "window_unknown".into()],
        encoder_fingerprint: "bundled-webp-v1".into(),
        captured_at_utc: "2026-09-28T09:15:00.000Z".into(),
    }
}
#[test]
fn s02_t09m2_01_create_reopen_scoped_read_and_exact_duplicate() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let value = input(LibraryId::new(
        f.rt(worker.schema_snapshot()).unwrap().library_id,
    ));
    let created = f.rt(worker.create_capture_intent(value.clone())).unwrap();
    let CreateIntentOutcome::Created(pending) = created else {
        panic!("new intent")
    };
    assert_eq!(pending.input, value);
    assert!(sm_application::storage::is_canonical_utc_timestamp(
        &pending.planned_created_at_utc
    ));
    assert_ne!(pending.planned_created_at_utc, value.captured_at_utc);
    assert_eq!(
        f.rt(worker.create_capture_intent(value.clone())).unwrap(),
        CreateIntentOutcome::Existing(pending.clone())
    );
    assert_eq!(
        f.rt(worker.read_capture_intent(
            value.library_id,
            value.principal_id,
            value.request.clone()
        ))
        .unwrap(),
        ReadIntentOutcome::Pending(pending.clone())
    );
    assert_eq!(
        f.rt(worker.lookup_operation(
            OperationId::new(value.request.operation_id()),
            value.principal_id,
            CaptureTarget::Region
        ))
        .unwrap(),
        sm_application::storage::LookupOutcome::Unknown
    );
    let foreign = PrincipalId::new(Uuid::new_v4());
    assert_eq!(
        f.rt(worker.read_capture_intent(value.library_id, foreign, value.request.clone()))
            .unwrap(),
        ReadIntentOutcome::Conflict
    );
    let mut changed = value.clone();
    changed.principal_id = foreign;
    assert_eq!(
        f.rt(worker.create_capture_intent(changed)).unwrap(),
        CreateIntentOutcome::Conflict
    );
    let mut changed = value.clone();
    changed.request = CaptureRequest::new(
        value.request.operation_id(),
        sm_application::capture::CaptureMode::Desktop,
        CaptureTrigger::ClientCommand,
    );
    assert_eq!(
        f.rt(worker.create_capture_intent(changed.clone())).unwrap(),
        CreateIntentOutcome::Conflict
    );
    assert_eq!(
        f.rt(worker.read_capture_intent(value.library_id, value.principal_id, changed.request))
            .unwrap(),
        ReadIntentOutcome::Conflict
    );
    let mut changed = value.clone();
    changed.request = CaptureRequest::new(
        value.request.operation_id(),
        value.request.mode(),
        CaptureTrigger::Hotkey,
    );
    assert_eq!(
        f.rt(worker.create_capture_intent(changed.clone())).unwrap(),
        CreateIntentOutcome::Conflict
    );
    assert_eq!(
        f.rt(worker.read_capture_intent(value.library_id, value.principal_id, changed.request))
            .unwrap(),
        ReadIntentOutcome::Conflict
    );
    let mut changed = value.clone();
    changed.pixel_sha256 = "a".repeat(64);
    assert_eq!(
        f.rt(worker.create_capture_intent(changed)).unwrap(),
        CreateIntentOutcome::Conflict
    );
    let mut changed = value.clone();
    changed.frame_id = Uuid::new_v4();
    assert_eq!(
        f.rt(worker.create_capture_intent(changed)).unwrap(),
        CreateIntentOutcome::Conflict
    );
    let absent = CaptureRequest::new(
        Uuid::new_v4(),
        value.request.mode(),
        CaptureTrigger::ClientCommand,
    );
    assert_eq!(
        f.rt(worker.read_capture_intent(value.library_id, value.principal_id, absent))
            .unwrap(),
        ReadIntentOutcome::Unknown
    );
    f.close(&worker);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.read_capture_intent(
            value.library_id,
            value.principal_id,
            value.request.clone()
        ))
        .unwrap(),
        ReadIntentOutcome::Pending(pending.clone())
    );
    assert_eq!(
        f.rt(worker.create_capture_intent(value)).unwrap(),
        CreateIntentOutcome::Existing(pending)
    );
    f.close(&worker);
    f.assert_no_output();
}
#[test]
fn s02_t09m2_02_invalid_inputs_do_not_insert() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let value = input(LibraryId::new(
        f.rt(worker.schema_snapshot()).unwrap().library_id,
    ));
    let mut cases = Vec::new();
    let mut bad = value.clone();
    bad.width = 0;
    cases.push(bad);
    let mut bad = value.clone();
    bad.height = 16_384;
    cases.push(bad);
    let mut bad = value.clone();
    bad.context.bounds.width = 2;
    cases.push(bad);
    let mut bad = value.clone();
    bad.context.target = CaptureTarget::Desktop;
    cases.push(bad);
    let mut bad = value.clone();
    bad.pixel_sha256 = "A".repeat(64);
    cases.push(bad);
    let mut bad = value.clone();
    bad.pixel_sha256 = "a".repeat(63);
    cases.push(bad);
    let mut bad = value.clone();
    bad.warnings = vec!["private/path".into()];
    cases.push(bad);
    let mut bad = value.clone();
    bad.warnings = vec!["ok".into(); 17];
    cases.push(bad);
    let mut bad = value.clone();
    bad.encoder_fingerprint.clear();
    cases.push(bad);
    let mut bad = value.clone();
    bad.encoder_fingerprint = "x".repeat(513);
    cases.push(bad);
    let mut bad = value.clone();
    bad.context.title.clear();
    cases.push(bad);
    let mut bad = value.clone();
    bad.context.title = "x".repeat(262_145);
    cases.push(bad);
    let mut bad = value.clone();
    bad.record_id = RecordId::new(Uuid::nil());
    cases.push(bad);
    let mut bad = value.clone();
    bad.asset_id = AssetId::new(value.record_id.as_uuid());
    cases.push(bad);
    let mut bad = value.clone();
    let mut non_rfc_v4 = *bad.record_id.as_uuid().as_bytes();
    non_rfc_v4[8] &= 0x3f;
    bad.record_id = RecordId::new(Uuid::from_bytes(non_rfc_v4));
    cases.push(bad);
    let mut bad = value.clone();
    bad.principal_id = PrincipalId::new(Uuid::nil());
    cases.push(bad);
    let mut bad = value.clone();
    bad.request = CaptureRequest::new(
        Uuid::nil(),
        value.request.mode(),
        CaptureTrigger::ClientCommand,
    );
    cases.push(bad);
    let mut bad = value.clone();
    bad.library_id = LibraryId::new(Uuid::new_v4());
    assert_eq!(
        f.rt(worker.create_capture_intent(bad)),
        Err(StorageError::IntegrityFailed)
    );
    for bad in cases {
        assert_eq!(
            f.rt(worker.create_capture_intent(bad)),
            Err(StorageError::InvalidPayload)
        );
    }
    f.close(&worker);
    f.assert_no_output();
    let db = Connection::open(f.db()).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM capture_intents", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn s02_t09m2_03_saved_precedes_pending_and_corrupt_owned_row_fails_closed() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let value = input(LibraryId::new(
        f.rt(worker.schema_snapshot()).unwrap().library_id,
    ));
    let created = f.rt(worker.create_capture_intent(value.clone())).unwrap();
    assert!(matches!(created, CreateIntentOutcome::Created(_)));
    f.close(&worker);
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "UPDATE capture_intents SET context_sha256=?1 WHERE operation_id=?2",
        params!["a".repeat(64), value.request.operation_id().to_string()],
    )
    .unwrap();
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.read_capture_intent(
            value.library_id,
            value.principal_id,
            value.request.clone()
        )),
        Err(StorageError::IntegrityFailed)
    );
    assert_eq!(
        f.rt(worker.create_capture_intent(value.clone())),
        Err(StorageError::IntegrityFailed)
    );
    let mut changed_caller = value.clone();
    changed_caller.frame_id = Uuid::new_v4();
    assert_eq!(
        f.rt(worker.create_capture_intent(changed_caller)),
        Err(StorageError::IntegrityFailed)
    );
    assert_eq!(
        f.rt(worker.read_capture_intent(
            value.library_id,
            PrincipalId::new(Uuid::new_v4()),
            value.request.clone()
        ))
        .unwrap(),
        ReadIntentOutcome::Conflict
    );
    f.close(&worker);
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "DELETE FROM capture_intents WHERE operation_id=?1",
        [value.request.operation_id().to_string()],
    )
    .unwrap();
    db.execute("INSERT INTO records (id,library_id,created_at,capture_context,provenances) VALUES (?1,?2,?3,'{}','[{}]')", params![value.record_id.as_uuid().to_string(),value.library_id.as_uuid().to_string(),value.captured_at_utc]).unwrap();
    db.execute("INSERT INTO ingest_operations (operation_id,principal_id,kind,planned_record_id,planned_asset_id,content_sha256,semantic_fingerprint,mode,width,height,warnings,record_id,state,created_at_utc,updated_at_utc) VALUES (?1,?2,'capture',?3,?4,?5,?5,'region',3,2,'[]',?3,'saved',?6,?6)",params![value.request.operation_id().to_string(),value.principal_id.as_uuid().to_string(),value.record_id.as_uuid().to_string(),value.asset_id.as_uuid().to_string(),value.pixel_sha256,value.captured_at_utc]).unwrap();
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.create_capture_intent(value.clone())).unwrap(),
        CreateIntentOutcome::Conflict
    );
    assert_eq!(
        f.rt(worker.read_capture_intent(
            value.library_id,
            value.principal_id,
            value.request.clone()
        ))
        .unwrap(),
        ReadIntentOutcome::Conflict
    );
    assert_eq!(
        f.rt(worker.read_capture_intent(
            value.library_id,
            PrincipalId::new(Uuid::new_v4()),
            value.request
        ))
        .unwrap(),
        ReadIntentOutcome::Conflict
    );
    f.close(&worker);
    assert!(!f.0.join("assets").exists());
    assert!(!f.0.join("staging").exists());
}

#[test]
fn s02_t09m2_04_fingerprint_tamper_and_real_alternate_trigger() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.lease()).unwrap();
    let value = input(LibraryId::new(
        f.rt(worker.schema_snapshot()).unwrap().library_id,
    ));
    let CreateIntentOutcome::Created(pending) =
        f.rt(worker.create_capture_intent(value.clone())).unwrap()
    else {
        panic!("created")
    };
    f.close(&worker);
    let alternate = CaptureRequest::new(
        value.request.operation_id(),
        value.request.mode(),
        CaptureTrigger::Hotkey,
    );
    // Independently derive the v1 digest using the closed trigger and the persisted mode.
    let mut sha = Sha256::new();
    sha.update(b"ScreenMemory.capture-intent.request.v1");
    for part in [
        alternate.operation_id().to_string(),
        alternate.target().as_str().to_owned(),
        alternate.trigger().as_str().to_owned(),
    ] {
        sha.update([0]);
        sha.update(part.as_bytes());
    }
    let alternate_fingerprint = format!("{:x}", sha.finalize());
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "UPDATE capture_intents SET request_fingerprint=?1 WHERE operation_id=?2",
        params!["a".repeat(64), value.request.operation_id().to_string()],
    )
    .unwrap();
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.read_capture_intent(
            value.library_id,
            value.principal_id,
            value.request.clone()
        )),
        Err(StorageError::IntegrityFailed)
    );
    assert_eq!(
        f.rt(worker.create_capture_intent(value.clone())),
        Err(StorageError::IntegrityFailed)
    );
    assert_eq!(
        f.rt(worker.read_capture_intent(
            value.library_id,
            PrincipalId::new(Uuid::new_v4()),
            value.request.clone()
        ))
        .unwrap(),
        ReadIntentOutcome::Conflict
    );
    let other_mode = CaptureRequest::new(
        value.request.operation_id(),
        sm_application::capture::CaptureMode::Desktop,
        CaptureTrigger::ClientCommand,
    );
    assert_eq!(
        f.rt(worker.read_capture_intent(value.library_id, value.principal_id, other_mode))
            .unwrap(),
        ReadIntentOutcome::Conflict
    );
    f.close(&worker);
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "UPDATE capture_intents SET request_fingerprint=?1 WHERE operation_id=?2",
        params![
            alternate_fingerprint,
            value.request.operation_id().to_string()
        ],
    )
    .unwrap();
    drop(db);
    let worker = StorageWorker::start(f.lease()).unwrap();
    assert_eq!(
        f.rt(worker.read_capture_intent(
            value.library_id,
            value.principal_id,
            value.request.clone()
        ))
        .unwrap(),
        ReadIntentOutcome::Conflict
    );
    assert_eq!(
        f.rt(worker.create_capture_intent(value.clone())).unwrap(),
        CreateIntentOutcome::Conflict
    );
    let mut alternate_input = value.clone();
    alternate_input.request = alternate.clone();
    let alternate_pending = sm_application::storage::PendingCaptureIntent {
        input: alternate_input.clone(),
        planned_created_at_utc: pending.planned_created_at_utc.clone(),
    };
    assert_eq!(
        f.rt(worker.read_capture_intent(value.library_id, value.principal_id, alternate))
            .unwrap(),
        ReadIntentOutcome::Pending(Box::new(alternate_pending.clone()))
    );
    assert_eq!(
        f.rt(worker.create_capture_intent(alternate_input)).unwrap(),
        CreateIntentOutcome::Existing(Box::new(alternate_pending))
    );
    f.close(&worker);
    f.assert_no_output();
}
