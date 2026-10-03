use super::*;
use processing::ProcessingError;
use rusqlite::params;
use sha2::{Digest, Sha256};
use sm_domain::{ModelProfileId, ProcessingBatchId, Revision};
use sm_isolation::{LibraryRootResolver, RootSource};
use sm_protocol::processing::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let root = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"))
            .join(Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        for name in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(name)).unwrap();
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
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn op() -> OperationId {
    OperationId::new(Uuid::new_v4())
}
fn profile() -> ModelProfile {
    ModelProfile {
        id: ModelProfileId::new(Uuid::new_v4()),
        model_key: "owned-synthetic/model".into(),
        context_length: 8192,
        eval_batch_size: 256,
        flash_attention: true,
        offload_kv_cache_to_gpu: true,
        k_cache: CacheType::F16,
        v_cache: CacheType::F16,
        native_ttl_seconds: Some(900),
        max_generation_tokens: 4000,
        temperature: 0.2,
        reasoning: ReasoningMode::BackendDefault,
    }
}
async fn saved(w: &StorageWorker, p: PrincipalId) -> (RecordId, AssetId) {
    use sm_application::storage::*;
    let rid = RecordId::new(Uuid::new_v4());
    let aid = AssetId::new(Uuid::new_v4());
    let image = sm_codec::RgbaImage::from_rgba(2, 2, vec![80; 16]).unwrap();
    let bytes = sm_codec::encode_exact_lossless_verified(&image)
        .unwrap()
        .into_canonical()
        .bytes()
        .to_vec();
    w.commit_image_capture(CommitImageCapture {
        operation_id: op(),
        principal_id: p,
        record_id: rid,
        asset_id: aid,
        asset_type: AssetType::Original,
        mode: CaptureTarget::Region,
        width: 2,
        height: 2,
        capture_context: CaptureContext {
            device: "synthetic".into(),
            app: "synthetic".into(),
            title: "synthetic".into(),
            target: CaptureTarget::Region,
            sources: vec![CaptureTarget::Region],
            bounds: PhysicalBounds {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
        },
        provenances: vec![Provenance::Manual {
            when: "2026-10-03T12:00:00.000Z".into(),
        }],
        warnings: vec![],
        canonical: bytes,
    })
    .await
    .unwrap();
    (rid, aid)
}
fn policy_reply(v: ProcessingReply) -> PolicyView {
    match v {
        ProcessingReply::Policy(p) => p,
        _ => panic!("policy"),
    }
}
fn batch_reply(v: ProcessingReply) -> BatchView {
    match v {
        ProcessingReply::Batch(b) => b,
        _ => panic!("batch"),
    }
}

#[test]
fn s04_processing_durable_public_controls_replay_freeze_off_reopen_and_preserve_saved() {
    rt().block_on(async {
        let f = Fixture::new();
        let w = StorageWorker::start(f.lease()).unwrap();
        let p = PrincipalId::new(Uuid::new_v4());
        let (rid, aid) = saved(&w, p).await;
        let before = w
            .read_asset_chunk(aid, 0, sm_application::storage::ASSET_CHUNK_BYTES)
            .await
            .unwrap();
        let initial = policy_reply(w.processing(p, ProcessingRequest::Policy {}).await.unwrap());
        assert_eq!(initial.value, ProcessingPolicy::default());
        assert!(!w.read_ai_enabled().await.unwrap());
        let pid = profile();
        let save = ProcessingRequest::SaveProfile {
            operation_id: op(),
            expected_revision: Revision::new(0),
            value: pid.clone(),
        };
        w.processing(p, save.clone()).await.unwrap();
        w.processing(p, save).await.unwrap();
        let value = ProcessingPolicy {
            mode: ProcessingMode::Manual,
            ..initial.value
        };
        let update = ProcessingRequest::UpdatePolicy {
            operation_id: op(),
            expected_revision: initial.revision,
            value: value.clone(),
        };
        let enabled = policy_reply(w.processing(p, update.clone()).await.unwrap());
        let replay = policy_reply(w.processing(p, update.clone()).await.unwrap());
        assert_eq!(replay.revision, enabled.revision);
        assert!(w.read_ai_enabled().await.unwrap());
        assert_eq!(
            w.processing(PrincipalId::new(Uuid::new_v4()), update)
                .await
                .unwrap_err(),
            ProcessingError::Conflict
        );
        let plan = ProcessingRequest::Plan {
            operation_id: op(),
            expected_policy_revision: enabled.revision,
            records: vec![rid],
            kind: WorkKind::Analyze,
            profile_id: Some(pid.id),
        };
        let batch = batch_reply(w.processing(p, plan.clone()).await.unwrap());
        assert_eq!(batch.jobs[0].state, JobState::Blocked);
        assert_eq!(batch.jobs[0].reason, Some(BlockReason::HandlerUnavailable));
        assert_eq!(batch.jobs[0].attempt_count, 0);
        assert_eq!(batch.jobs[0].asset_id, aid);
        let replay = batch_reply(w.processing(p, plan.clone()).await.unwrap());
        assert_eq!(batch.jobs[0].id, replay.jobs[0].id);
        let mut changed = pid.clone();
        changed.temperature = 0.8;
        w.processing(
            p,
            ProcessingRequest::SaveProfile {
                operation_id: op(),
                expected_revision: Revision::new(1),
                value: changed,
            },
        )
        .await
        .unwrap();
        let read = batch_reply(
            w.processing(p, ProcessingRequest::Batch { id: batch.id })
                .await
                .unwrap(),
        );
        assert_eq!(read.jobs[0].profile.as_ref().unwrap().value, pid);
        assert_eq!(
            w.processing(
                PrincipalId::new(Uuid::new_v4()),
                ProcessingRequest::Batch { id: batch.id }
            )
            .await
            .unwrap_err(),
            ProcessingError::NotFound
        );
        let pause = ProcessingRequest::ControlBatch {
            id: batch.id,
            expected_revision: batch.revision,
            control: BatchControl::Pause,
        };
        let paused = batch_reply(w.processing(p, pause.clone()).await.unwrap());
        assert_eq!(paused.control, BatchControl::Pause);
        assert_eq!(
            w.processing(p, pause).await.unwrap_err(),
            ProcessingError::Conflict
        );
        let off = policy_reply(
            w.processing(
                p,
                ProcessingRequest::UpdatePolicy {
                    operation_id: op(),
                    expected_revision: enabled.revision,
                    value: ProcessingPolicy::default(),
                },
            )
            .await
            .unwrap(),
        );
        assert!(off.authorization_epoch.value() > enabled.authorization_epoch.value());
        assert!(!w.read_ai_enabled().await.unwrap());
        let bad = ProcessingPolicy {
            retention: HostRetention::KeepLoaded,
            ..ProcessingPolicy::default()
        };
        assert_eq!(
            w.processing(
                p,
                ProcessingRequest::UpdatePolicy {
                    operation_id: op(),
                    expected_revision: off.revision,
                    value: bad
                }
            )
            .await
            .unwrap_err(),
            ProcessingError::Unsupported
        );
        assert_eq!(
            w.processing(
                p,
                ProcessingRequest::ControlJob {
                    id: batch.jobs[0].id,
                    expected_revision: Revision::new(2),
                    control: JobControl::Retry
                }
            )
            .await
            .unwrap_err(),
            ProcessingError::Disabled
        );
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        let w = StorageWorker::start(f.lease()).unwrap();
        let reopened = batch_reply(
            w.processing(p, ProcessingRequest::Batch { id: batch.id })
                .await
                .unwrap(),
        );
        assert_eq!(reopened.control, BatchControl::Pause);
        assert_eq!(reopened.jobs[0].reason, Some(BlockReason::AiDisabled));
        // Replay returns the original admission receipt, not a new selection or work dispatch.
        let replay = batch_reply(w.processing(p, plan).await.unwrap());
        assert_eq!(replay.jobs[0].id, batch.jobs[0].id);
        let after = w
            .read_asset_chunk(aid, 0, sm_application::storage::ASSET_CHUNK_BYTES)
            .await
            .unwrap();
        assert_eq!(before.bytes, after.bytes);
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        assert_eq!(
            w.processing(p, ProcessingRequest::Policy {})
                .await
                .unwrap_err(),
            ProcessingError::Unavailable
        );
    });
}

#[test]
fn s04_processing_admission_rolls_back_missing_duplicate_and_unsupported_profiles() {
    rt().block_on(async {
        let f = Fixture::new();
        let w = StorageWorker::start(f.lease()).unwrap();
        let p = PrincipalId::new(Uuid::new_v4());
        let (rid, _) = saved(&w, p).await;
        w.processing(
            p,
            ProcessingRequest::UpdatePolicy {
                operation_id: op(),
                expected_revision: Revision::new(1),
                value: ProcessingPolicy {
                    mode: ProcessingMode::Automatic,
                    ..ProcessingPolicy::default()
                },
            },
        )
        .await
        .unwrap();
        for records in [
            vec![],
            vec![rid, rid],
            vec![rid, RecordId::new(Uuid::new_v4())],
        ] {
            let operation = op();
            assert!(w
                .processing(
                    p,
                    ProcessingRequest::Plan {
                        operation_id: operation,
                        expected_policy_revision: Revision::new(2),
                        records,
                        kind: WorkKind::Analyze,
                        profile_id: None
                    }
                )
                .await
                .is_err());
            assert_eq!(
                w.processing(
                    p,
                    ProcessingRequest::Batch {
                        id: ProcessingBatchId::new(operation.as_uuid())
                    }
                )
                .await
                .unwrap_err(),
                ProcessingError::NotFound
            );
        }
        for reason in [ReasoningMode::Off, ReasoningMode::Budget100] {
            let mut v = profile();
            v.reasoning = reason;
            assert_eq!(
                w.processing(
                    p,
                    ProcessingRequest::SaveProfile {
                        operation_id: op(),
                        expected_revision: Revision::new(0),
                        value: v
                    }
                )
                .await
                .unwrap_err(),
                ProcessingError::Unsupported
            );
        }
        let mut v = profile();
        v.max_generation_tokens = 4001;
        assert_eq!(
            w.processing(
                p,
                ProcessingRequest::SaveProfile {
                    operation_id: op(),
                    expected_revision: Revision::new(0),
                    value: v
                }
            )
            .await
            .unwrap_err(),
            ProcessingError::Invalid
        );
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        let db = Connection::open_with_flags(f.db(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM processing_jobs", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM processing_runtime_calls", [], |r| r
                .get::<_, u32>(
                0
            ))
            .unwrap(),
            0
        );
        drop(db);
    });
}

#[test]
fn s04_processing_migration_populated_v5_preflight_rollback_upgrade_checksum_reopen() {
    // Historical metadata-only fixture retained; real Saved/asset/origin migration is below.

    let f = Fixture::new();
    let db = Connection::open(f.db()).unwrap();
    migration::mark_new_file(&db).unwrap();
    db.execute_batch(migration::BOOTSTRAP).unwrap();
    for (n, name, text) in [
        (
            1,
            "0001_foundation",
            include_str!("../migrations/0001_foundation.sql"),
        ),
        (
            2,
            "0002_images",
            include_str!("../migrations/0002_images.sql"),
        ),
        (
            3,
            "0003_operations",
            include_str!("../migrations/0003_operations.sql"),
        ),
        (
            4,
            "0004_capture_intents",
            include_str!("../migrations/0004_capture_intents.sql"),
        ),
        (
            5,
            "0005_import_export",
            include_str!("../migrations/0005_import_export.sql"),
        ),
    ] {
        db.execute_batch(text).unwrap();
        db.execute(
            "INSERT INTO schema_migrations VALUES (?1,?2,?3,'now')",
            params![n, name, format!("{:x}", Sha256::digest(text.as_bytes()))],
        )
        .unwrap();
    }
    let library = Uuid::new_v4().to_string();
    db.execute("INSERT INTO library_meta VALUES (1,?1,1,'now')", [&library])
        .unwrap();
    for k in ["ai.enabled", "model.autostart"] {
        db.execute("INSERT INTO library_settings VALUES (?1,'false',1)", [k])
            .unwrap();
    }
    db.execute("INSERT INTO records(id,library_id,created_at,capture_context,provenances) VALUES (?1,?2,'2026-10-03T12:00:00.000Z','{}','[{}]')",params![Uuid::new_v4().to_string(),library]).unwrap();
    drop(db);
    let before = fs::read(f.db()).unwrap();
    assert!(!migration::preflight(&f.db(), true).unwrap());
    assert_eq!(fs::read(f.db()).unwrap(), before);
    let mut db = Connection::open(f.db()).unwrap();
    assert_eq!(
        migration::initialize(&mut db, false, true).unwrap_err(),
        StorageError::MigrationFailed
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='processing_jobs'",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    let s = migration::initialize(&mut db, false, false).unwrap();
    assert_eq!(s.schema_version, 7);
    assert_eq!(s.library_id.to_string(), library);
    assert_eq!(
        db.query_row("SELECT count(*) FROM records", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
    drop(db);
    assert!(!migration::preflight(&f.db(), true).unwrap());
    let db = Connection::open(f.db()).unwrap();
    db.execute(
        "UPDATE schema_migrations SET checksum='bad' WHERE version=6",
        [],
    )
    .unwrap();
    drop(db);
    let before = fs::read(f.db()).unwrap();
    assert_eq!(
        migration::preflight(&f.db(), true).unwrap_err(),
        StorageError::SchemaIncompatible
    );
    assert_eq!(fs::read(f.db()).unwrap(), before);
}

#[test]
fn s04_execution_migration_actual_saved_canonical_origin_v5_rollback_upgrade_reopen() {
    use sm_application::{images::ImageLibraryApi, RequestContext, Scope};
    use sm_domain::{CorrelationId, ImportBatchId, RequestId};
    use sm_protocol::images::*;
    let f = Fixture::new();
    let source_root = Fixture::new();
    let runtime = rt();
    let (record, asset, canonical_path, canonical, origin_before) = runtime.block_on(async {
        let w = StorageWorker::start(f.lease()).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let repo = PathBuf::from(repo.to_string_lossy().trim_start_matches(r"\\?\"));
        let executable = repo
            .join(".pi")
            .join("staging-target-t09b")
            .join("debug")
            .join("shot-image-worker.exe");
        let hash = Sha256::digest(fs::read(&executable).unwrap()).into();
        let worker =
            Arc::new(sm_image_worker::TrustedWorker::from_manifest(&executable, hash).unwrap());
        let service = image_services::ImageServices::new(w.clone(), worker);
        let context = RequestContext::from_authenticated_peer(
            RequestId::new(Uuid::new_v4()),
            CorrelationId::new(Uuid::new_v4()),
            PrincipalId::new(Uuid::new_v4()),
            vec![Scope::ImportFiles, Scope::ReadImages],
        );
        let source = source_root.0.join("input.png");
        fs::write(
            &source,
            include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
        )
        .unwrap();
        let batch = ImportBatchId::new(Uuid::new_v4());
        service
            .import_paths(
                context.clone(),
                ImportRequest {
                    batch_id: batch,
                    source_paths: vec![source.to_string_lossy().into_owned()],
                    profile: CanonicalProfile::WebpExact,
                    duplicates: DuplicatePolicy::KeepBoth,
                },
            )
            .await
            .unwrap();
        let report = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let r = service
                    .import_status(context.clone(), ImportBatchRequest { batch_id: batch })
                    .await
                    .unwrap();
                if r.discovery_complete && !r.active_work {
                    assert_eq!(r.saved_new, 1);
                    assert_eq!(r.failed, 0);
                    break r;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let receipt = report.items[0].receipt.as_ref().unwrap();
        let path = f.0.join(&receipt.relative_path);
        let bytes = fs::read(&path).unwrap();
        assert!(sm_codec::decode_rgba(&bytes).is_ok());
        fs::remove_file(source).unwrap();
        w.shutdown(Duration::from_secs(20)).await.unwrap();
        drop(service);
        drop(w);
        let db = Connection::open(f.db()).unwrap();
        let origin: (String, String, String) = db
            .query_row(
                "SELECT source_sha256,metadata_json,created_at FROM origin_occurrences",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        drop(db);
        (
            receipt.record_id,
            receipt.original_asset_id,
            path,
            bytes,
            origin,
        )
    });
    // Own newly generated root only, all owners closed. Restore the unaccepted additive
    // v6 layer to a v5 fixture; the real S03 Saved row, canonical and origin are unchanged.
    assert_eq!(
        Uuid::parse_str(f.0.file_name().unwrap().to_str().unwrap())
            .unwrap()
            .get_version_num(),
        4
    );
    let mut db = Connection::open(f.db()).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE; DROP TABLE knowledge_projection_debts; DROP TABLE knowledge_source_documents; DROP TABLE knowledge_cards; DROP TABLE knowledge_write_receipts; DROP TABLE knowledge_category_versions; DROP TABLE knowledge_categories; DROP TABLE category_catalog_meta; DELETE FROM schema_migrations WHERE version=7; DROP TABLE processing_derivative_holds; DROP TABLE processing_model_leases; DROP TABLE processing_runtime_calls; DROP TABLE processing_attempts; DROP TABLE processing_jobs; DROP TABLE processing_batches; DROP TABLE processing_control_receipts; DROP TABLE processing_profiles; DROP TABLE processing_execution_host; DROP TABLE processing_policy; DELETE FROM schema_migrations WHERE version=6; COMMIT;").unwrap();
    drop(db);
    let before = fs::read(f.db()).unwrap();
    assert!(!migration::preflight(&f.db(), true).unwrap());
    assert_eq!(fs::read(f.db()).unwrap(), before);
    db = Connection::open(f.db()).unwrap();
    assert_eq!(
        migration::initialize(&mut db, false, true).unwrap_err(),
        StorageError::MigrationFailed
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='processing_jobs'",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    let assert_saved = |db: &Connection| {
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM records WHERE id=?1",
                [record.as_uuid().to_string()],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(db.query_row("SELECT count(*) FROM assets WHERE id=?1 AND record_id=?2 AND asset_type='original'",params![asset.as_uuid().to_string(),record.as_uuid().to_string()],|r|r.get::<_,u32>(0)).unwrap(),1);
        let origin: (String, String, String) = db
            .query_row(
                "SELECT source_sha256,metadata_json,created_at FROM origin_occurrences",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(origin, origin_before);
        assert_eq!(fs::read(&canonical_path).unwrap(), canonical);
    };
    assert_saved(&db);
    assert_eq!(
        migration::initialize(&mut db, false, false)
            .unwrap()
            .schema_version,
        7
    );
    assert_saved(&db);
    drop(db);
    runtime.block_on(async {
        let w = StorageWorker::start(f.lease()).unwrap();
        assert_eq!(w.schema_snapshot().await.unwrap().schema_version, 7);
        w.shutdown(Duration::from_secs(20)).await.unwrap();
    });
    let db = Connection::open(f.db()).unwrap();
    assert_saved(&db);
    assert_eq!(
        db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, u32>(
            0
        ))
        .unwrap(),
        0
    );
    drop(db);
}
