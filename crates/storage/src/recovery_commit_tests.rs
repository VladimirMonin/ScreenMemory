use super::*;
use crate::match_recovery::ValidatedIntentRead;
use sha2::{Digest, Sha256};
use sm_application::{
    capture::{CaptureMode, CaptureRequest, CaptureTrigger},
    storage::{
        CaptureContext, CaptureTarget, CreateCaptureIntent, CreateIntentOutcome,
        FoundationRepository, LookupOutcome, PhysicalBounds, Provenance,
    },
};
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId, RecordId};
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = base.join(Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(child)).unwrap();
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
    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.0.join("data/library.sqlite3")).unwrap()
    }
    fn counts(&self, expected: (i64, i64, i64, i64)) {
        let db = self.db();
        let counts = ["capture_intents", "records", "assets", "ingest_operations"].map(|table| {
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
        });
        assert_eq!(counts, [expected.0, expected.1, expected.2, expected.3]);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn intent(library: LibraryId, pixels: &[u8]) -> CreateCaptureIntent {
    let mode = CaptureTarget::Region;
    CreateCaptureIntent {
        library_id: library,
        principal_id: PrincipalId::new(Uuid::new_v4()),
        request: CaptureRequest::new(
            Uuid::new_v4(),
            CaptureMode::Region,
            CaptureTrigger::ClientCommand,
        ),
        record_id: RecordId::new(Uuid::new_v4()),
        asset_id: AssetId::new(Uuid::new_v4()),
        frame_id: Uuid::new_v4(),
        width: 1,
        height: 1,
        pixel_sha256: format!("{:x}", Sha256::digest(pixels)),
        context: CaptureContext {
            device: "synthetic".into(),
            app: "synthetic".into(),
            title: "synthetic".into(),
            target: mode,
            sources: vec![mode],
            bounds: PhysicalBounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        },
        warnings: vec!["display_unknown".into(), "window_unknown".into()],
        encoder_fingerprint: sm_codec::encoder_fingerprint(),
        captured_at_utc: "2026-09-28T09:15:00.000Z".into(),
    }
}
fn reopened(i: &CreateCaptureIntent) -> OwnedReopenInputV2 {
    OwnedReopenInputV2 {
        library_id: i.library_id,
        principal_id: i.principal_id,
        operation_id: OperationId::new(i.request.operation_id()),
        mode: i.request.target(),
        encoder_fingerprint: i.encoder_fingerprint.clone(),
    }
}
async fn published(worker: &StorageWorker, i: &CreateCaptureIntent) -> VerifiedPublished {
    let ValidatedIntentRead::Pending(witness) = worker
        .read_validated_capture_intent(i.library_id, i.principal_id, i.request.clone())
        .await
        .unwrap()
    else {
        panic!("pending")
    };
    let stage = worker.reopen_stage_owned_v2(reopened(i)).await.unwrap();
    let candidate = worker
        .prepare_recovery_candidate(witness, stage)
        .await
        .unwrap();
    assert_eq!(
        sm_codec::decode_rgba(candidate.encoded_bytes())
            .unwrap()
            .pixels(),
        [11, 22, 33, 0]
    );
    worker.publish_recovery_candidate(candidate).await.unwrap()
}
async fn setup(f: &Fixture) -> (CreateCaptureIntent, String) {
    let worker = StorageWorker::start(f.lease()).unwrap();
    let pixels = [11, 22, 33, 0];
    let i = intent(
        LibraryId::new(worker.schema_snapshot().await.unwrap().library_id),
        &pixels,
    );
    let CreateIntentOutcome::Created(pending) =
        worker.create_capture_intent(i.clone()).await.unwrap()
    else {
        panic!("created")
    };
    worker
        .stage_capture_owned_v2(OwnedStageInputV2 {
            capture: OwnedStageInput {
                library_id: i.library_id,
                principal_id: i.principal_id,
                operation_id: OperationId::new(i.request.operation_id()),
                record_id: i.record_id,
                asset_id: i.asset_id,
                frame_id: i.frame_id,
                mode: i.request.target(),
                context: i.context.clone(),
                captured_at: i.captured_at_utc.clone(),
                encoder_fingerprint: i.encoder_fingerprint.clone(),
                pixels: pixels.to_vec(),
            },
            warnings: i.warnings.clone(),
        })
        .await
        .unwrap();
    worker.shutdown(Duration::from_secs(5)).await.unwrap();
    (i, pending.planned_created_at_utc)
}
fn paths(f: &Fixture, i: &CreateCaptureIntent, planned: &str) -> (PathBuf, PathBuf) {
    (
        f.0.join("assets")
            .join(&planned[..7])
            .join(i.record_id.as_uuid().to_string())
            .join("original.webp"),
        f.0.join("staging")
            .join(i.request.operation_id().to_string())
            .join("manifest.json"),
    )
}

#[test]
fn s02_t09n3b2_01_f06_f07_restarts_exact_saved_and_replay() {
    for after_inserts in [false, true] {
        let f = Fixture::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let (i, planned) = setup(&f).await;
            let (final_path, stage_path) = paths(&f, &i, &planned);
            let failing = StorageWorker::start_with_recovery_fault(f.lease(), after_inserts).unwrap();
            let sealed = failing.seal_recovery_published(published(&failing, &i).await).await.unwrap();
            let original = fs::read(&final_path).unwrap();
            let manifest = fs::read(&stage_path).unwrap();
            assert_eq!(failing.commit_sealed_recovery(sealed).await, Err(StorageError::Unavailable));
            assert_eq!(fs::read(&stage_path).unwrap(), manifest);
            assert_eq!(fs::read(&final_path).unwrap(), original);
            failing.shutdown(Duration::from_secs(5)).await.unwrap();
            f.counts((1,0,0,0));
            let resumed = StorageWorker::start(f.lease()).unwrap();
            let published_file = published(&resumed, &i).await;
            assert!(!published_file.newly_published());
            let sealed = resumed.seal_recovery_published(published_file).await.unwrap();
            // Two independent pre-commit witnesses: the second reaches the Saved-first
            // transaction branch after the first has deleted pending.
            let replay_seal = resumed
                .seal_recovery_published(published(&resumed, &i).await)
                .await
                .unwrap();
            let foreign_seal = resumed
                .seal_recovery_published(published(&resumed, &i).await)
                .await
                .unwrap();
            let mode_seal = resumed
                .seal_recovery_published(published(&resumed, &i).await)
                .await
                .unwrap();
            let saved = resumed.commit_sealed_recovery(sealed).await.unwrap();
            assert_eq!(resumed.commit_sealed_recovery(replay_seal).await.unwrap(), saved);
            let db = f.db();
            db.execute("UPDATE ingest_operations SET principal_id=?1 WHERE operation_id=?2", [Uuid::new_v4().to_string(), i.request.operation_id().to_string()]).unwrap();
            assert_eq!(resumed.commit_sealed_recovery(foreign_seal).await, Err(StorageError::IntegrityFailed));
            db.execute("UPDATE ingest_operations SET principal_id=?1, mode='desktop' WHERE operation_id=?2", [i.principal_id.as_uuid().to_string(), i.request.operation_id().to_string()]).unwrap();
            assert_eq!(resumed.commit_sealed_recovery(mode_seal).await, Err(StorageError::IntegrityFailed));
            db.execute("UPDATE ingest_operations SET mode='region' WHERE operation_id=?1", [i.request.operation_id().to_string()]).unwrap();
            drop(db);
            assert_eq!(saved.created_at, planned);
            assert_eq!(saved.relative_path, format!("assets/{}/{}/original.webp", &planned[..7], i.record_id.as_uuid()));
            assert_eq!(saved.record_id, i.record_id);
            assert_eq!(saved.asset_id, i.asset_id);
            assert_eq!(saved.warnings, i.warnings);
            assert_eq!(saved.sha256, format!("{:x}", Sha256::digest(&original)));
            assert_eq!(saved.size_bytes, original.len() as u64);
            assert_eq!(resumed.lookup_operation(OperationId::new(i.request.operation_id()), i.principal_id, i.request.target()).await.unwrap(), LookupOutcome::Saved(saved.clone()));
            resumed.shutdown(Duration::from_secs(5)).await.unwrap();
            f.counts((0,1,1,1));
            let db = f.db();
            let stored: (String,String,String,String,String,String) = db.query_row(
                "SELECT r.id,a.id,io.planned_record_id,io.planned_asset_id,r.provenances,io.warnings FROM records r JOIN assets a ON a.record_id=r.id JOIN ingest_operations io ON io.record_id=r.id", [],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
            assert_eq!((&stored.0,&stored.1), (&stored.2,&stored.3));
            assert_eq!(stored.0, i.record_id.as_uuid().to_string());
            assert_eq!(stored.1, i.asset_id.as_uuid().to_string());
            assert_eq!(stored.5, sm_application::storage::warnings_to_json(&i.warnings).unwrap());
            assert_eq!(sm_application::storage::provenances_from_json(&stored.4).unwrap(), vec![Provenance::Automatic { when: i.captured_at_utc.clone(), source: i.context.target }]);
            let updated: String = db.query_row("SELECT updated_at_utc FROM ingest_operations WHERE operation_id=?1", [i.request.operation_id().to_string()], |r| r.get(0)).unwrap();
            assert!(sm_application::storage::is_canonical_utc_timestamp(&updated));
            drop(db);
            assert!(stage_path.exists());
            assert_eq!(fs::read(&final_path).unwrap(), original);
            let again = StorageWorker::start(f.lease()).unwrap();
            assert_eq!(again.lookup_operation(OperationId::new(i.request.operation_id()), i.principal_id, i.request.target()).await.unwrap(), LookupOutcome::Saved(saved));
            again.shutdown(Duration::from_secs(5)).await.unwrap();
        });
    }
}

#[test]
fn s02_t09n3b2_03_saved_replay_rejects_corrupt_asset_type_record_time_and_library() {
    for mutation in ["asset_type", "created_at", "library_id"] {
        let f = Fixture::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let (i, planned) = setup(&f).await;
            let (final_path, stage_path) = paths(&f, &i, &planned);
            let worker = StorageWorker::start(f.lease()).unwrap();
            // Both proofs precede the first Saved; replay cannot create a new proof
            // from a deleted pending intent.
            let first = worker
                .seal_recovery_published(published(&worker, &i).await)
                .await
                .unwrap();
            let replay = worker
                .seal_recovery_published(published(&worker, &i).await)
                .await
                .unwrap();
            worker.commit_sealed_recovery(first).await.unwrap();
            f.counts((0, 1, 1, 1));
            let canonical = fs::read(&final_path).unwrap();
            let manifest = fs::read(&stage_path).unwrap();
            let mut db = f.db();
            match mutation {
                "asset_type" => {
                    db.execute(
                        "UPDATE assets SET asset_type='preview' WHERE id=?1",
                        [i.asset_id.as_uuid().to_string()],
                    )
                    .unwrap();
                }
                "created_at" => {
                    db.execute(
                        "UPDATE records SET created_at='2037-12-01T00:00:00.000Z' WHERE id=?1",
                        [i.record_id.as_uuid().to_string()],
                    )
                    .unwrap();
                }
                "library_id" => {
                    // The foundation schema has a singleton library_meta, not a second
                    // library row. Change its ID and the record together under deferred
                    // FK enforcement: the resulting synthetic DB still passes FK checks,
                    // but no longer belongs to the sealed trusted library.
                    let other = Uuid::new_v4().to_string();
                    let tx = db.transaction().unwrap();
                    tx.execute_batch("PRAGMA defer_foreign_keys=ON").unwrap();
                    tx.execute(
                        "UPDATE library_meta SET library_id=?1 WHERE singleton=1",
                        [&other],
                    )
                    .unwrap();
                    tx.execute(
                        "UPDATE records SET library_id=?1 WHERE id=?2",
                        [&other, &i.record_id.as_uuid().to_string()],
                    )
                    .unwrap();
                    tx.commit().unwrap();
                    let violations: i64 = db
                        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                            r.get(0)
                        })
                        .unwrap();
                    assert_eq!(violations, 0);
                }
                _ => unreachable!(),
            }
            drop(db);
            let result = worker.commit_sealed_recovery(replay).await;
            worker.shutdown(Duration::from_secs(5)).await.unwrap();
            assert_eq!(result, Err(StorageError::IntegrityFailed), "{mutation}");
            f.counts((0, 1, 1, 1));
            assert_eq!(fs::read(&final_path).unwrap(), canonical);
            assert_eq!(fs::read(&stage_path).unwrap(), manifest);
        });
    }
}

#[test]
fn s02_t09n3b2_04_saved_replay_rejects_changed_journal_created_at() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let (i, planned) = setup(&f).await;
        let (final_path, stage_path) = paths(&f, &i, &planned);
        let worker = StorageWorker::start(f.lease()).unwrap();
        let first = worker
            .seal_recovery_published(published(&worker, &i).await)
            .await
            .unwrap();
        let replay = worker
            .seal_recovery_published(published(&worker, &i).await)
            .await
            .unwrap();
        worker.commit_sealed_recovery(first).await.unwrap();
        let canonical = fs::read(&final_path).unwrap();
        let manifest = fs::read(&stage_path).unwrap();
        let db = f.db();
        assert_eq!(db.execute(
            "UPDATE ingest_operations SET created_at_utc='2037-12-01T00:00:00.000Z' WHERE operation_id=?1",
            [i.request.operation_id().to_string()],
        ).unwrap(), 1);
        drop(db);
        let result = worker.commit_sealed_recovery(replay).await;
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        assert_eq!(result, Err(StorageError::IntegrityFailed));
        f.counts((0, 1, 1, 1));
        assert_eq!(fs::read(&final_path).unwrap(), canonical);
        assert_eq!(fs::read(&stage_path).unwrap(), manifest);
    });
}

#[test]
fn s02_t09n3b2_05_saved_replay_rejects_changed_singleton_library() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let (i, planned) = setup(&f).await;
        let (final_path, stage_path) = paths(&f, &i, &planned);
        let worker = StorageWorker::start(f.lease()).unwrap();
        let first = worker
            .seal_recovery_published(published(&worker, &i).await)
            .await
            .unwrap();
        let replay = worker
            .seal_recovery_published(published(&worker, &i).await)
            .await
            .unwrap();
        worker.commit_sealed_recovery(first).await.unwrap();
        let canonical = fs::read(&final_path).unwrap();
        let manifest = fs::read(&stage_path).unwrap();
        let db = f.db();
        // Deliberate synthetic FK corruption, not a production write: changing
        // singleton library_meta alone leaves the record's library ID untouched.
        db.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        let other = Uuid::new_v4().to_string();
        assert_eq!(
            db.execute(
                "UPDATE library_meta SET library_id=?1 WHERE singleton=1",
                [&other],
            )
            .unwrap(),
            1
        );
        let record_library: String = db
            .query_row(
                "SELECT library_id FROM records WHERE id=?1",
                [i.record_id.as_uuid().to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(record_library, i.library_id.as_uuid().to_string());
        assert_ne!(record_library, other);
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        drop(db);
        let result = worker.commit_sealed_recovery(replay).await;
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        assert_eq!(result, Err(StorageError::IntegrityFailed));
        f.counts((0, 1, 1, 1));
        assert_eq!(fs::read(&final_path).unwrap(), canonical);
        assert_eq!(fs::read(&stage_path).unwrap(), manifest);
    });
}

#[test]
fn s02_t09n3b2_02_stale_foreign_mode_tamper_and_v4_trigger_fail_closed() {
    for mutation in [
        "warnings",
        "planned",
        "principal",
        "mode",
        "context",
        "missing",
    ] {
        let f = Fixture::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let (i, planned) = setup(&f).await;
            let (final_path, stage_path) = paths(&f, &i, &planned);
            let worker = StorageWorker::start(f.lease()).unwrap();
            let sealed = worker.seal_recovery_published(published(&worker, &i).await).await.unwrap();
            let original = fs::read(&final_path).unwrap();
            let db = f.db();
            let sql = match mutation {
                "warnings" => "UPDATE capture_intents SET warnings='[\"different\"]' WHERE operation_id=?1",
                "planned" => "UPDATE capture_intents SET planned_created_at_utc='2037-12-01T00:00:00.000Z' WHERE operation_id=?1",
                "principal" => "UPDATE capture_intents SET principal_id='123e4567-e89b-42d3-a456-426614174000' WHERE operation_id=?1",
                "mode" => "UPDATE capture_intents SET mode='desktop' WHERE operation_id=?1",
                "missing" => "DELETE FROM capture_intents WHERE operation_id=?1",
                _ => "UPDATE capture_intents SET context_json='{}' WHERE operation_id=?1",
            };
            db.execute(sql, [i.request.operation_id().to_string()]).unwrap();
            assert_eq!(worker.commit_sealed_recovery(sealed).await, Err(StorageError::IntegrityFailed));
            drop(db);
            worker.shutdown(Duration::from_secs(5)).await.unwrap();
            f.counts((if mutation == "missing" { 0 } else { 1 },0,0,0));
            assert!(stage_path.exists());
            assert_eq!(fs::read(&final_path).unwrap(), original);
        });
    }
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let (i, planned) = setup(&f).await;
        let (final_path, stage_path) = paths(&f, &i, &planned);
        let worker = StorageWorker::start(f.lease()).unwrap();
        let published_file = published(&worker, &i).await;
        let original = fs::read(&final_path).unwrap();
        drop(published_file);
        let mut tampered = original.clone();
        tampered[12] ^= 1;
        fs::write(&final_path, &tampered).unwrap();
        assert!(worker
            .publish_recovery_candidate({
                let ValidatedIntentRead::Pending(w) = worker
                    .read_validated_capture_intent(i.library_id, i.principal_id, i.request.clone())
                    .await
                    .unwrap()
                else {
                    panic!("pending")
                };
                let reopened = worker.reopen_stage_owned_v2(reopened(&i)).await.unwrap();
                worker
                    .prepare_recovery_candidate(w, reopened)
                    .await
                    .unwrap()
            })
            .await
            .is_err());
        fs::write(&final_path, &original).unwrap();
        let sealed = worker
            .seal_recovery_published(published(&worker, &i).await)
            .await
            .unwrap();
        let db = f.db();
        // v4's journal trigger rejects the pending ID even before other missing columns.
        let error = db
            .execute(
                "INSERT INTO ingest_operations (operation_id) VALUES (?1)",
                [i.request.operation_id().to_string()],
            )
            .unwrap_err();
        assert!(error.to_string().contains("operation still pending"));
        drop(db);
        assert_eq!(
            worker
                .commit_sealed_recovery(sealed)
                .await
                .unwrap()
                .created_at,
            planned
        );
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        f.counts((0, 1, 1, 1));
        assert!(stage_path.exists());
    });
}
