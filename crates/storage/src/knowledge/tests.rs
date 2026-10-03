use super::*;
use crate::StorageWorker;
use sm_application::storage::*;
use sm_domain::{AssetId, CorrelationId, OperationId, RecordId, RequestId};
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.pi/temporary")
            .canonicalize()
            .unwrap();
        let path = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"))
            .join(Uuid::new_v4().to_string());
        fs::create_dir(&path).unwrap();
        for c in ["data", "config", "cache", "webview"] {
            fs::create_dir(path.join(c)).unwrap();
        }
        Self(path)
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
fn id() -> KnowledgeId {
    KnowledgeId(Uuid::new_v4())
}
fn ctx(p: PrincipalId, scopes: Vec<Scope>) -> RequestContext {
    RequestContext::from_authenticated_peer(
        RequestId::new(Uuid::new_v4()),
        CorrelationId::new(Uuid::new_v4()),
        p,
        scopes,
    )
}
async fn saved(w: &StorageWorker, p: PrincipalId) -> KnowledgeId {
    let rid = id();
    let aid = id();
    let image = sm_codec::RgbaImage::from_rgba(2, 2, vec![80; 16]).unwrap();
    let canonical = sm_codec::encode_exact_lossless_verified(&image)
        .unwrap()
        .into_canonical()
        .bytes()
        .to_vec();
    w.commit_image_capture(CommitImageCapture {
        operation_id: OperationId::new(Uuid::new_v4()),
        principal_id: p,
        record_id: RecordId::new(rid.0),
        asset_id: AssetId::new(aid.0),
        asset_type: AssetType::Original,
        mode: CaptureTarget::Region,
        width: 2,
        height: 2,
        capture_context: CaptureContext {
            device: "private path C:/secret".into(),
            app: "serial GPS".into(),
            title: "private window".into(),
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
        canonical,
    })
    .await
    .unwrap();
    rid
}
async fn card(w: &StorageWorker, p: PrincipalId, r: KnowledgeId) -> CardView {
    match w
        .knowledge_request(p, KnowledgeRequest::Read { record_id: r })
        .await
        .unwrap()
    {
        KnowledgeReply::Card(c) => *c,
        _ => panic!("card"),
    }
}
async fn cat(w: &StorageWorker, p: PrincipalId, r: CategoryMutation) -> CategoryReceipt {
    match w
        .knowledge_request(p, KnowledgeRequest::Category(r))
        .await
        .unwrap()
    {
        KnowledgeReply::Category(c) => c,
        _ => panic!("category"),
    }
}
fn edit_req(r: KnowledgeId, rev: u64, catalog: u64) -> CardEdit {
    CardEdit {
        operation_id: id(),
        record_id: r,
        expected_record_revision: SqlRevision(rev),
        expected_catalog_revision: SqlRevision(catalog),
        expected_category_revision: None,
        patch: CardPatch::default(),
    }
}
#[test]
fn manual_saved_replay_conflict_keep_inherit_empty_unicode_reopen_and_canonical_preservation() {
    rt().block_on(async {
        let f = Fixture::new();
        let w = StorageWorker::start(f.lease()).unwrap();
        let p = PrincipalId::new(Uuid::new_v4());
        let r = saved(&w, p).await;
        let before = card(&w, p, r).await;
        assert!(!encode(&before).unwrap().contains("private"));
        assert_eq!(before.projection_state, ProjectionState::HandlerUnavailable);
        let context = ctx(p, vec![Scope::ReadMetadata, Scope::WriteMetadata]);
        let mut req = edit_req(r, 1, 1);
        req.patch.title = FieldPatch::Set {
            value: CardTitle("😀".repeat(160)),
        };
        req.patch.description = FieldPatch::Set {
            value: CardDescription(String::new()),
        };
        req.patch.category_id = FieldPatch::Set { value: None };
        let result = w
            .knowledge(context.clone(), KnowledgeRequest::Edit(req.clone()))
            .await
            .unwrap();
        let replay = w
            .knowledge(context.clone(), KnowledgeRequest::Edit(req.clone()))
            .await
            .unwrap();
        assert_eq!(encode(&result).unwrap(), encode(&replay).unwrap());
        let after = card(&w, p, r).await;
        assert_eq!(after.record_revision, SqlRevision(2));
        assert_eq!(after.content_revision, SqlRevision(2));
        assert_eq!(after.effective.description_source, FieldSource::User);
        assert_eq!(after.effective.category_source, FieldSource::User);
        let stale = edit_req(r, 1, 1);
        assert_eq!(
            w.knowledge(context.clone(), KnowledgeRequest::Edit(stale))
                .await
                .unwrap_err()
                .code,
            ErrorCode::RevisionConflict
        );
        req.patch.title = FieldPatch::Set {
            value: CardTitle("different".into()),
        };
        assert_eq!(
            w.knowledge(context.clone(), KnowledgeRequest::Edit(req))
                .await
                .unwrap_err()
                .code,
            ErrorCode::IdempotencyConflict
        );
        let req = edit_req(r, 2, 1);
        w.knowledge(context.clone(), KnowledgeRequest::Edit(req))
            .await
            .unwrap();
        assert_eq!(card(&w, p, r).await.record_revision, SqlRevision(2));
        let mut req = edit_req(r, 2, 1);
        req.patch.description = FieldPatch::Inherit;
        w.knowledge(context.clone(), KnowledgeRequest::Edit(req))
            .await
            .unwrap();
        let inherited = card(&w, p, r).await;
        assert_eq!(inherited.record_revision, SqlRevision(3));
        assert_eq!(inherited.content_revision, SqlRevision(2));
        assert_eq!(inherited.effective.description_source, FieldSource::None);
        let mut bad = edit_req(r, 3, 1);
        bad.patch.title = FieldPatch::Set {
            value: CardTitle("😀".repeat(161)),
        };
        assert_eq!(
            w.knowledge(context.clone(), KnowledgeRequest::Edit(bad))
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            w.knowledge(
                ctx(p, vec![Scope::ReadSourceMetadata]),
                KnowledgeRequest::Read { record_id: r }
            )
            .await
            .unwrap_err()
            .code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            w.knowledge(
                ctx(p, vec![Scope::ReadMetadata]),
                KnowledgeRequest::Edit(edit_req(r, 3, 1))
            )
            .await
            .unwrap_err()
            .code,
            ErrorCode::PermissionDenied
        );
        assert!(!w.read_ai_enabled().await.unwrap());
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        let db = Connection::open(f.0.join("data/library.sqlite3")).unwrap();
        let assets: Vec<(String, String, u64)> = db
            .prepare("SELECT path,sha256,size_bytes FROM assets")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (path, digest, size) in assets {
            let bytes = fs::read(f.0.join(path)).unwrap();
            assert_eq!(bytes.len() as u64, size);
            assert_eq!(hash_bytes(&bytes), digest);
        }
        drop(db);
        let w = StorageWorker::start(f.lease()).unwrap();
        let reopened = card(&w, p, r).await;
        assert_eq!(encode(&inherited).unwrap(), encode(&reopened).unwrap());
        w.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}
fn hash_bytes(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
#[test]
fn categories_nfc_casefold_atomic_rename_delete_archive_restore_and_revision_checks() {
    rt().block_on(async {
        let f = Fixture::new();
        let w = StorageWorker::start(f.lease()).unwrap();
        let p = PrincipalId::new(Uuid::new_v4());
        let r = saved(&w, p).await;
        let create = CategoryMutation {
            operation_id: id(),
            expected_catalog_revision: SqlRevision(1),
            change: CategoryChange::Create {
                name: CategoryName("  Café Straße  ".into()),
                description: CategoryDescription("purpose".into()),
            },
        };
        let c = cat(&w, p, create.clone()).await;
        assert_eq!(c.category.name.0, "Café Straße");
        assert_eq!(
            encode(&KnowledgeReply::Category(c.clone())).unwrap(),
            encode(
                &w.knowledge_request(p, KnowledgeRequest::Category(create))
                    .await
                    .unwrap()
            )
            .unwrap()
        );
        let duplicate = CategoryMutation {
            operation_id: id(),
            expected_catalog_revision: SqlRevision(2),
            change: CategoryChange::Create {
                name: CategoryName("Cafe\u{301} STRASSE".into()),
                description: CategoryDescription(String::new()),
            },
        };
        assert_eq!(
            w.knowledge_request(p, KnowledgeRequest::Category(duplicate))
                .await
                .unwrap_err(),
            KnowledgeError::NameConflict
        );
        let mut edit = edit_req(r, 1, 2);
        edit.expected_category_revision = Some(SqlRevision(1));
        edit.patch.category_id = FieldPatch::Set {
            value: Some(c.category.id),
        };
        w.knowledge_request(p, KnowledgeRequest::Edit(edit))
            .await
            .unwrap();
        let rename = CategoryMutation {
            operation_id: id(),
            expected_catalog_revision: SqlRevision(2),
            change: CategoryChange::Rename {
                category_id: c.category.id,
                expected_revision: SqlRevision(1),
                name: CategoryName("Локальные модели".into()),
                description: CategoryDescription(String::new()),
            },
        };
        let renamed = cat(&w, p, rename.clone()).await;
        assert_eq!(renamed.affected_records, 1);
        let after = card(&w, p, r).await;
        assert_eq!(
            after.effective.category_label.unwrap().0,
            "Локальные модели"
        );
        assert_eq!(after.content_revision, SqlRevision(3));
        let stale = CategoryMutation {
            operation_id: id(),
            ..rename
        };
        assert_eq!(
            w.knowledge_request(p, KnowledgeRequest::Category(stale))
                .await
                .unwrap_err(),
            KnowledgeError::Revision
        );
        let state = |catalog, revision, s, count| CategoryMutation {
            operation_id: id(),
            expected_catalog_revision: SqlRevision(catalog),
            change: CategoryChange::State {
                category_id: c.category.id,
                expected_revision: SqlRevision(revision),
                state: s,
                expected_assigned_records: count,
            },
        };
        let archived = cat(&w, p, state(3, 2, CategoryState::Archived, 1)).await;
        assert_eq!(archived.affected_records, 0);
        assert_eq!(
            card(&w, p, r).await.effective.category_id,
            Some(c.category.id)
        );
        let mut assign = edit_req(r, 3, 4);
        assign.expected_category_revision = Some(SqlRevision(3));
        assign.patch.category_id = FieldPatch::Set {
            value: Some(c.category.id),
        };
        assert_eq!(
            w.knowledge_request(p, KnowledgeRequest::Edit(assign))
                .await
                .unwrap_err(),
            KnowledgeError::CategoryUnavailable
        );
        cat(&w, p, state(4, 3, CategoryState::Active, 1)).await;
        assert_eq!(
            w.knowledge_request(
                p,
                KnowledgeRequest::Category(state(5, 4, CategoryState::Deleted, 0))
            )
            .await
            .unwrap_err(),
            KnowledgeError::Revision
        );
        let deleted = cat(&w, p, state(5, 4, CategoryState::Deleted, 1)).await;
        assert_eq!(deleted.affected_records, 1);
        let card = card(&w, p, r).await;
        assert_eq!(card.overrides.category_id, FieldPatch::Set { value: None });
        assert_eq!(card.effective.category_source, FieldSource::User);
        assert_eq!(card.content_revision, SqlRevision(4));
        assert_eq!(
            w.knowledge_request(
                p,
                KnowledgeRequest::Category(state(6, 5, CategoryState::Active, 0))
            )
            .await
            .unwrap_err(),
            KnowledgeError::CategoryUnavailable
        );
        w.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}
#[test]
fn additive_v6_saved_upgrade_rollback_reopen_preserves_metadata_and_canonical() {
    rt().block_on(async {
        let f = Fixture::new(); let w = StorageWorker::start(f.lease()).unwrap(); let p = PrincipalId::new(Uuid::new_v4());
        let r = saved(&w, p).await; w.shutdown(Duration::from_secs(5)).await.unwrap();
        let mut db = Connection::open(f.0.join("data/library.sqlite3")).unwrap();
        let preserved: (String,String,String) = db.query_row("SELECT r.capture_context,r.provenances,a.sha256 FROM records r JOIN assets a ON a.record_id=r.id WHERE r.id=?1", [r.0.to_string()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        // Stopped newly created OWN synthetic fixture, remove only unaccepted empty v7 layer.
        db.execute_batch("BEGIN IMMEDIATE; DROP TABLE knowledge_projection_debts; DROP TABLE knowledge_source_documents; DROP TABLE knowledge_cards; DROP TABLE knowledge_write_receipts; DROP TABLE knowledge_category_versions; DROP TABLE knowledge_categories; DROP TABLE category_catalog_meta; DELETE FROM schema_migrations WHERE version=7; COMMIT;").unwrap();
        assert_eq!(crate::migration::initialize(&mut db,false,true).unwrap_err(),StorageError::MigrationFailed);
        assert_eq!(db.query_row("SELECT MAX(version) FROM schema_migrations",[],|r|r.get::<_,u32>(0)).unwrap(),6);
        assert_eq!(db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_cards'",[],|r|r.get::<_,u32>(0)).unwrap(),0);
        let after: (String,String,String)=db.query_row("SELECT r.capture_context,r.provenances,a.sha256 FROM records r JOIN assets a ON a.record_id=r.id WHERE r.id=?1",[r.0.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();assert_eq!(after,preserved);drop(db);
        let w=StorageWorker::start(f.lease()).unwrap();assert_eq!(w.schema_snapshot().await.unwrap().schema_version,7);let c=card(&w,p,r).await;assert_eq!(c.record_revision,SqlRevision(1));w.shutdown(Duration::from_secs(5)).await.unwrap();
        let w=StorageWorker::start(f.lease()).unwrap();assert_eq!(card(&w,p,r).await.source_sha256,c.source_sha256);w.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}
#[test]
fn known_sensitive_source_scopes_survive_manual_edits_and_are_not_listed_or_replayed_to_denied_reader(
) {
    rt().block_on(async {
        let f=Fixture::new();let w=StorageWorker::start(f.lease()).unwrap();let p=PrincipalId::new(Uuid::new_v4());let r=saved(&w,p).await;
        let mut req=edit_req(r,1,1);req.patch.title=FieldPatch::Set {value:CardTitle("restricted synthetic text".into())};w.knowledge_request(p,KnowledgeRequest::Edit(req)).await.unwrap();w.shutdown(Duration::from_secs(5)).await.unwrap();
        // Private known-sensitivity fixture only, no fabricated AI result/run/publication.
        let db=Connection::open(f.0.join("data/library.sqlite3")).unwrap();let mut s=read(&db,r).unwrap().source;s.required_read_scopes.push("read_source_metadata".into());s.required_read_scopes.push("read_paths".into());let j=encode(&s).unwrap();let h=hash(&j);
        db.execute("UPDATE knowledge_source_documents SET source_json=?1,source_sha256=?2 WHERE record_id=?3",params![j,h,r.0.to_string()]).unwrap();db.execute("UPDATE knowledge_projection_debts SET source_sha256=?1 WHERE record_id=?2",params![h,r.0.to_string()]).unwrap();drop(db);
        let w=StorageWorker::start(f.lease()).unwrap();let denied=ctx(p,vec![Scope::ReadMetadata,Scope::WriteMetadata]);
        assert_eq!(w.knowledge(denied.clone(),KnowledgeRequest::Read {record_id:r}).await.unwrap_err().code,ErrorCode::PermissionDenied);
        let mut edit=edit_req(r,2,1);edit.patch.title=FieldPatch::Set {value:CardTitle("edited but still restricted".into())};
        assert_eq!(w.knowledge(denied.clone(),KnowledgeRequest::Edit(edit.clone())).await.unwrap_err().code,ErrorCode::PermissionDenied);
        w.knowledge_request(p,KnowledgeRequest::Edit(edit.clone())).await.unwrap();assert_eq!(card(&w,p,r).await.source.required_read_scopes,s.required_read_scopes);
        assert_eq!(w.knowledge(denied,KnowledgeRequest::Edit(edit)).await.unwrap_err().code,ErrorCode::PermissionDenied);
        let list=w.library_page(RecordPageQuery::new(Some(24),None).unwrap()).await.unwrap();assert!(!list.items[0].title.contains("restricted"));w.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}
#[test]
fn concurrent_clients_expected_revision_has_exactly_one_winner_without_last_write_wins() {
    rt().block_on(async {
        let f = Fixture::new();
        let w = StorageWorker::start(f.lease()).unwrap();
        let p = PrincipalId::new(Uuid::new_v4());
        let r = saved(&w, p).await;
        let mut a = edit_req(r, 1, 1);
        a.patch.description = FieldPatch::Set {
            value: CardDescription("client A".into()),
        };
        let mut b = edit_req(r, 1, 1);
        b.patch.description = FieldPatch::Set {
            value: CardDescription("client B".into()),
        };
        let barrier = std::sync::Barrier::new(2);
        let (left, right) = std::thread::scope(|scope| {
            let left = scope.spawn(|| {
                barrier.wait();
                rt().block_on(w.knowledge_request(p, KnowledgeRequest::Edit(a)))
            });
            let right = scope.spawn(|| {
                barrier.wait();
                rt().block_on(w.knowledge_request(p, KnowledgeRequest::Edit(b)))
            });
            (left.join().unwrap(), right.join().unwrap())
        });
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        assert!(matches!(
            (left, right),
            (Ok(_), Err(KnowledgeError::Revision)) | (Err(KnowledgeError::Revision), Ok(_))
        ));
        let c = card(&w, p, r).await;
        assert_eq!(c.record_revision, SqlRevision(2));
        assert_eq!(c.content_revision, SqlRevision(2));
        assert!(["client A", "client B"].contains(&c.effective.description.0.as_str()));
        w.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}
#[test]
fn active_category_ceiling_refuses_create_and_restore_without_partial_catalog_or_receipt() {
    rt().block_on(async {
        let f = Fixture::new();
        let w = StorageWorker::start(f.lease()).unwrap();
        let p = PrincipalId::new(Uuid::new_v4());
        let mut first = None;
        for n in 0..128 {
            let c = cat(
                &w,
                p,
                CategoryMutation {
                    operation_id: id(),
                    expected_catalog_revision: SqlRevision(n + 1),
                    change: CategoryChange::Create {
                        name: CategoryName(format!("category {n}")),
                        description: CategoryDescription(String::new()),
                    },
                },
            )
            .await;
            if n == 0 {
                first = Some(c.category.id);
            }
        }
        let extra = CategoryMutation {
            operation_id: id(),
            expected_catalog_revision: SqlRevision(129),
            change: CategoryChange::Create {
                name: CategoryName("extra".into()),
                description: CategoryDescription(String::new()),
            },
        };
        assert_eq!(
            w.knowledge_request(p, KnowledgeRequest::Category(extra))
                .await
                .unwrap_err(),
            KnowledgeError::Limit
        );
        let first = first.unwrap();
        cat(
            &w,
            p,
            CategoryMutation {
                operation_id: id(),
                expected_catalog_revision: SqlRevision(129),
                change: CategoryChange::State {
                    category_id: first,
                    expected_revision: SqlRevision(1),
                    state: CategoryState::Archived,
                    expected_assigned_records: 0,
                },
            },
        )
        .await;
        cat(
            &w,
            p,
            CategoryMutation {
                operation_id: id(),
                expected_catalog_revision: SqlRevision(130),
                change: CategoryChange::Create {
                    name: CategoryName("extra".into()),
                    description: CategoryDescription(String::new()),
                },
            },
        )
        .await;
        let restore = CategoryMutation {
            operation_id: id(),
            expected_catalog_revision: SqlRevision(131),
            change: CategoryChange::State {
                category_id: first,
                expected_revision: SqlRevision(2),
                state: CategoryState::Active,
                expected_assigned_records: 0,
            },
        };
        assert_eq!(
            w.knowledge_request(p, KnowledgeRequest::Category(restore))
                .await
                .unwrap_err(),
            KnowledgeError::Limit
        );
        let c = match w
            .knowledge_request(p, KnowledgeRequest::Categories {})
            .await
            .unwrap()
        {
            KnowledgeReply::Categories(c) => c,
            _ => panic!(),
        };
        assert_eq!(c.revision, SqlRevision(131));
        assert_eq!(
            c.items
                .iter()
                .filter(|c| c.state == CategoryState::Active)
                .count(),
            128
        );
        assert_eq!(
            c.items.iter().find(|c| c.id == first).unwrap().state,
            CategoryState::Archived
        );
        w.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}
struct FailHook;
impl SearchProjectionTx for FailHook {
    fn on_source_changed(
        &self,
        _: &Transaction<'_>,
        _: Option<SqlRevision>,
        _: &CardView,
    ) -> Result<(), KnowledgeError> {
        Err(KnowledgeError::Integrity)
    }
}
#[test]
fn projection_failure_rolls_back_edit_receipt_category_and_all_affected_sources() {
    rt().block_on(async {
        let f = Fixture::new();
        let w = StorageWorker::start(f.lease()).unwrap();
        let p = PrincipalId::new(Uuid::new_v4());
        let r = saved(&w, p).await;
        let r2 = saved(&w, p).await;
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        // Private transaction-hook regression on stopped OWN database, not live runtime publication.
        let mut db = Connection::open(f.0.join("data/library.sqlite3")).unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        let before = read(&db, r).unwrap();
        let mut edit = edit_req(r, 1, 1);
        edit.patch.title = FieldPatch::Set {
            value: CardTitle("should rollback".into()),
        };
        assert_eq!(
            handle_with_hook(&mut db, p, KnowledgeRequest::Edit(edit.clone()), &FailHook)
                .unwrap_err(),
            KnowledgeError::Integrity
        );
        assert_eq!(
            encode(&read(&db, r).unwrap()).unwrap(),
            encode(&before).unwrap()
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM knowledge_write_receipts", [], |r| r
                .get::<_, u32>(
                0
            ))
            .unwrap(),
            0
        );
        handle(&mut db, p, KnowledgeRequest::Edit(edit)).unwrap();
        let create = CategoryMutation {
            operation_id: id(),
            expected_catalog_revision: SqlRevision(1),
            change: CategoryChange::Create {
                name: CategoryName("Old".into()),
                description: CategoryDescription(String::new()),
            },
        };
        let c = match handle(&mut db, p, KnowledgeRequest::Category(create)).unwrap() {
            KnowledgeReply::Category(c) => c,
            _ => panic!(),
        };
        let mut edit = edit_req(r, 2, 2);
        edit.expected_category_revision = Some(SqlRevision(1));
        edit.patch.category_id = FieldPatch::Set {
            value: Some(c.category.id),
        };
        handle(&mut db, p, KnowledgeRequest::Edit(edit)).unwrap();
        let mut edit2 = edit_req(r2, 1, 2);
        edit2.expected_category_revision = Some(SqlRevision(1));
        edit2.patch.category_id = FieldPatch::Set {
            value: Some(c.category.id),
        };
        handle(&mut db, p, KnowledgeRequest::Edit(edit2)).unwrap();
        let before2 = read(&db, r2).unwrap();
        let before = read(&db, r).unwrap();
        let rename = CategoryMutation {
            operation_id: id(),
            expected_catalog_revision: SqlRevision(2),
            change: CategoryChange::Rename {
                category_id: c.category.id,
                expected_revision: SqlRevision(1),
                name: CategoryName("New".into()),
                description: CategoryDescription(String::new()),
            },
        };
        assert_eq!(
            handle_with_hook(&mut db, p, KnowledgeRequest::Category(rename), &FailHook)
                .unwrap_err(),
            KnowledgeError::Integrity
        );
        assert_eq!(category(&db, c.category.id).unwrap().name.0, "Old");
        assert_eq!(catalog_revision(&db).unwrap(), SqlRevision(2));
        assert_eq!(
            encode(&read(&db, r2).unwrap()).unwrap(),
            encode(&before2).unwrap()
        );
        let delete = CategoryMutation {
            operation_id: id(),
            expected_catalog_revision: SqlRevision(2),
            change: CategoryChange::State {
                category_id: c.category.id,
                expected_revision: SqlRevision(1),
                state: CategoryState::Deleted,
                expected_assigned_records: 2,
            },
        };
        assert_eq!(
            handle_with_hook(&mut db, p, KnowledgeRequest::Category(delete), &FailHook)
                .unwrap_err(),
            KnowledgeError::Integrity
        );
        assert_eq!(
            category(&db, c.category.id).unwrap().state,
            CategoryState::Active
        );
        assert_eq!(category(&db, c.category.id).unwrap().assigned_records, 2);
        assert_eq!(
            encode(&read(&db, r2).unwrap()).unwrap(),
            encode(&before2).unwrap()
        );
        assert_eq!(
            encode(&read(&db, r).unwrap()).unwrap(),
            encode(&before).unwrap()
        );
    });
}
