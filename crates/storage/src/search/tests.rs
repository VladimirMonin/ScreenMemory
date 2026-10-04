use super::*;
use crate::{
    knowledge::{self, SearchProjectionTx},
    search_projection, StorageWorker,
};
use sm_application::knowledge::KnowledgeApi;
use sm_domain::{CorrelationId, PrincipalId, RequestId};
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use sm_protocol::knowledge::*;
use std::{
    fs,
    path::{Path, PathBuf},
};
struct Fixture(PathBuf, Mutex<Vec<StorageWorker>>);
impl Fixture {
    fn new() -> Self {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.pi/temporary")
            .canonicalize()
            .unwrap();
        let root = PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\"))
            .join(Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        for n in ["data", "cache", "config", "webview"] {
            fs::create_dir(root.join(n)).unwrap()
        }
        Self(root, Mutex::new(Vec::new()))
    }
    fn start(&self) -> StorageWorker {
        let w = StorageWorker::start(self.lease()).unwrap();
        self.1.lock().unwrap().push(w.clone());
        w
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
    fn db(&self) -> Connection {
        let db = Connection::open(self.0.join("data/library.sqlite3")).unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        db
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let workers = std::mem::take(self.1.get_mut().unwrap_or_else(|e| e.into_inner()));
        let stopped = std::thread::spawn(move || {
            rt().block_on(async {
                for w in workers {
                    if w.shutdown(Duration::from_secs(5)).await.is_err() {
                        return false;
                    }
                }
                true
            })
        })
        .join()
        .unwrap_or(false);
        if stopped {
            if fs::remove_dir_all(&self.0).is_err() {
                eprintln!(
                    "OWN_FIXTURE_CLEANUP_FAILED {}",
                    self.0.file_name().unwrap().to_string_lossy()
                );
            }
        } else {
            eprintln!(
                "OWN_FIXTURE_STOP_FAILED {}",
                self.0.file_name().unwrap().to_string_lossy()
            );
        }
    }
}
fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn ctx(p: Uuid, mask: u8) -> RequestContext {
    let mut s = vec![
        Scope::ReadMetadata,
        Scope::WriteMetadata,
        Scope::ManageCategories,
    ];
    if mask & 2 != 0 {
        s.push(Scope::ReadSourceMetadata)
    }
    if mask & 4 != 0 {
        s.push(Scope::ReadPaths)
    }
    RequestContext::from_authenticated_peer(
        RequestId::new(Uuid::new_v4()),
        CorrelationId::new(Uuid::new_v4()),
        PrincipalId::new(p),
        s,
    )
}
fn request(text: &str) -> SearchStart {
    SearchStart {
        search_id: KnowledgeId(Uuid::new_v4()),
        text: text.into(),
        filters: SearchFilters::default(),
        limit: 30,
        candidate_depth: 100,
    }
}
async fn page(w: &StorageWorker, c: RequestContext, r: SearchStart) -> SearchPage {
    match w
        .search(c, SearchRequest::Start(r), CancellationToken::new())
        .await
        .unwrap()
    {
        SearchReply::Page(p) => p,
        _ => panic!("page"),
    }
}
async fn fresh() -> (Fixture, StorageWorker) {
    let f = Fixture::new();
    let w = f.start();
    (f, w)
}
/// Fixture setup ONLY in storage owner on its stopped own DB. No image/Saved claim:
/// actual import/canonical lifecycle is independently exercised by native GUI/CLI test.
fn seed(
    db: &mut Connection,
    id: KnowledgeId,
    title: &str,
    description: &str,
    category: Option<KnowledgeId>,
    mask: u8,
) {
    let tx = db.transaction().unwrap();
    let aid = Uuid::new_v4();
    tx.execute("INSERT INTO records(id,library_id,created_at,capture_context,provenances,source_kind) VALUES(?1,(SELECT library_id FROM library_meta),'2026-10-03T12:00:00.000Z','{}','[{}]','imported_image')",[id.0.to_string()]).unwrap();
    tx.execute("INSERT INTO assets VALUES(?1,?2,'original','webp_lossless',?3,1,?4,'2026-10-03T12:00:00.000Z')",params![aid.to_string(),id.0.to_string(),format!("assets/2026-10/{}/original.webp",id.0),"0".repeat(64)]).unwrap();
    knowledge::initial(&tx, id).unwrap();
    tx.commit().unwrap();
    let catalog: u64 = db
        .query_row("SELECT revision FROM category_catalog_meta", [], |r| {
            r.get(0)
        })
        .unwrap();
    knowledge::handle(
        db,
        PrincipalId::new(Uuid::new_v4()),
        KnowledgeRequest::Edit(CardEdit {
            operation_id: KnowledgeId(Uuid::new_v4()),
            record_id: id,
            expected_record_revision: SqlRevision(1),
            expected_catalog_revision: SqlRevision(catalog),
            expected_category_revision: category.map(|_| SqlRevision(1)),
            patch: CardPatch {
                title: FieldPatch::Set {
                    value: CardTitle(title.into()),
                },
                description: FieldPatch::Set {
                    value: CardDescription(description.into()),
                },
                category_id: FieldPatch::Set { value: category },
            },
        }),
    )
    .unwrap();
    if mask != 1 {
        let mut card = knowledge::read(db, id).unwrap();
        card.source.required_read_scopes = vec!["read_metadata".into()];
        if mask & 2 != 0 {
            card.source
                .required_read_scopes
                .push("read_source_metadata".into())
        }
        if mask & 4 != 0 {
            card.source.required_read_scopes.push("read_paths".into())
        }
        let j = serde_json::to_string(&card.source).unwrap();
        use sha2::Digest;
        card.source_sha256 = format!("{:x}", sha2::Sha256::digest(j.as_bytes()));
        let tx = db.transaction().unwrap();
        tx.execute("UPDATE knowledge_source_documents SET source_json=?1,source_sha256=?2 WHERE record_id=?3",params![j,card.source_sha256,id.0.to_string()]).unwrap();
        search_projection::Projection
            .on_source_changed(&tx, None, &card)
            .unwrap();
        tx.commit().unwrap();
    }
}
fn category(db: &mut Connection, name: &str) -> KnowledgeId {
    match knowledge::handle(
        db,
        PrincipalId::new(Uuid::new_v4()),
        KnowledgeRequest::Category(CategoryMutation {
            operation_id: KnowledgeId(Uuid::new_v4()),
            expected_catalog_revision: SqlRevision(
                db.query_row("SELECT revision FROM category_catalog_meta", [], |r| {
                    r.get(0)
                })
                .unwrap(),
            ),
            change: CategoryChange::Create {
                name: CategoryName(name.into()),
                description: CategoryDescription(String::new()),
            },
        }),
    )
    .unwrap()
    {
        KnowledgeReply::Category(c) => c.category.id,
        _ => panic!("category"),
    }
}
#[test]
fn s06_exact_literal_normalizer_limits_and_no_grammar() {
    for atom in [
        "Q8_0",
        "0xc0000005",
        "django.db",
        "v1.2.3",
        "C++",
        "C#",
        "settings.json",
        "128K",
    ] {
        assert_eq!(search_projection::atoms(atom), vec![atom]);
        assert_eq!(search_projection::exact_keys(&[atom]).unwrap().0.len(), 1);
    }
    assert_ne!(
        search_projection::normalize("ё").unwrap(),
        search_projection::normalize("е").unwrap()
    );
    assert_ne!(
        search_projection::normalize("Q8_0").unwrap(),
        search_projection::normalize("Q4_0").unwrap()
    );
    assert_eq!(
        search_projection::normalize("E\u{301}").unwrap(),
        search_projection::normalize("É").unwrap()
    );
    let text = (0..4100)
        .map(|n| format!("k_{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    let (keys, truncated) = search_projection::exact_keys(&[&text]).unwrap();
    assert_eq!(keys.len(), 4096);
    assert!(truncated);
    assert!(
        search_projection::exact_keys(&[&format!("k_{}", "x".repeat(128))])
            .unwrap()
            .0
            .is_empty()
    );
}
#[test]
fn s06_projection_rollback_reopen_scope_pages_filters_epoch_cancel_next_mutation() {
    rt().block_on(async{
 let(f,w)=fresh().await;w.shutdown(Duration::from_secs(5)).await.unwrap();let mut db=f.db();let p=Uuid::new_v4();let c=ctx(p,1);let rare=category(&mut db,"RareClass");
 let visible=KnowledgeId(Uuid::new_v4());seed(&mut db,visible,"Q8_0 manual","Visible description\u{1b}<b>plain</b>",Some(rare),1);
 let restricted=KnowledgeId(Uuid::new_v4());seed(&mut db,restricted,"Secret Q8_0","Hidden scopes",None,7);
 for _ in 0..125 {seed(&mut db,KnowledgeId(Uuid::new_v4()),"Q8_0 generic","distractor",None,1)}
 // Mandatory hook error rolls back overrides/source/revisions/FTS/receipt/epoch together.
 let before=knowledge::read(&db,visible).unwrap();let e=epoch(&db).unwrap();
 db.execute_batch("CREATE TRIGGER reject_projection BEFORE UPDATE ON search_documents BEGIN SELECT RAISE(ABORT,'test projection failure'); END;").unwrap();
 let edit=CardEdit{operation_id:KnowledgeId(Uuid::new_v4()),record_id:visible,expected_record_revision:before.record_revision,expected_catalog_revision:SqlRevision(2),expected_category_revision:None,patch:CardPatch{title:FieldPatch::Set{value:CardTitle("Changed77".into())},..Default::default()}};
 assert_eq!(knowledge::handle(&mut db,PrincipalId::new(p),KnowledgeRequest::Edit(edit.clone())).unwrap_err(),knowledge::KnowledgeError::Integrity);
 assert_eq!(knowledge::read(&db,visible).unwrap().source_sha256,before.source_sha256);assert_eq!(epoch(&db).unwrap(),e);
 db.execute_batch("DROP TRIGGER reject_projection;").unwrap();drop(db);
 let w=f.start();let mut r=request("Q8_0");r.limit=1;r.filters.category=CategoryFilter::AnyOf{ids:vec![rare]};
 assert_eq!(page(&w,c.clone(),r.clone()).await.items[0].record_id,visible);
 let mut conflict=r.clone();conflict.text="different".into();assert_eq!(w.search(c.clone(),SearchRequest::Start(conflict),CancellationToken::new()).await.unwrap_err().code,ErrorCode::IdempotencyConflict);
 let mut browse=request("");browse.limit=1;let first=page(&w,c.clone(),browse.clone()).await;assert!(first.window_truncated);let cursor=first.next_cursor.unwrap();
 assert_eq!(w.search(ctx(Uuid::new_v4(),1),SearchRequest::Page{cursor},CancellationToken::new()).await.unwrap_err().code,ErrorCode::SearchExpired);
 assert_eq!(w.search(ctx(Uuid::new_v4(),1),SearchRequest::Cancel{search_id:first.search_id},CancellationToken::new()).await.unwrap_err().code,ErrorCode::SearchExpired);
 assert_eq!(w.search(ctx(p,7),SearchRequest::Page{cursor},CancellationToken::new()).await.unwrap_err().code,ErrorCode::PermissionDenied);
 let all=page(&w,c.clone(),request("Secret")).await;assert!(all.items.is_empty());assert_eq!(page(&w,ctx(p,7),request("Secret")).await.items[0].record_id,restricted);
 let mut unsupported=request("Q8_0");unsupported.filters.application=ApplicationFilter::Unattributed{};assert_eq!(w.search(c.clone(),SearchRequest::Start(unsupported),CancellationToken::new()).await.unwrap_err().code,ErrorCode::SearchFilterUnsupported);
 assert_eq!(w.search(c.clone(),SearchRequest::Start(request("***")),CancellationToken::new()).await.unwrap_err().code,ErrorCode::NoLexicalTerms);
 assert!(page(&w,c.clone(),request("OR NEAR NOT")).await.items.is_empty());
 let token=CancellationToken::new();token.cancel();assert_eq!(w.search(c.clone(),SearchRequest::Start(request("Q8_0")),token).await.unwrap_err().code,ErrorCode::SearchCancelled);
 w.knowledge(c.clone(),KnowledgeRequest::Edit(edit)).await.unwrap();
 // Search cache LRU may expire an earlier window; create a fresh current snapshot then mutate.
 let mut refreshed=request("");refreshed.limit=1;
 let snap=page(&w,c.clone(),refreshed).await;let cursor=snap.next_cursor.unwrap();let current=match w.knowledge(c.clone(),KnowledgeRequest::Read{record_id:visible}).await.unwrap(){KnowledgeReply::Card(v)=>v,_=>panic!("card")};
 let req=CardEdit{operation_id:KnowledgeId(Uuid::new_v4()),record_id:visible,expected_record_revision:current.record_revision,expected_catalog_revision:SqlRevision(2),expected_category_revision:None,patch:CardPatch{description:FieldPatch::Set{value:CardDescription("Fresh88".into())},..Default::default()}};
 w.knowledge(c.clone(),KnowledgeRequest::Edit(req)).await.unwrap();assert_eq!(w.search(c.clone(),SearchRequest::Page{cursor},CancellationToken::new()).await.unwrap_err().code,ErrorCode::SearchSnapshotChanged);
 assert!(page(&w,c.clone(),request("manual")).await.items.is_empty());assert_eq!(page(&w,c.clone(),request("Fresh88")).await.items[0].record_id,visible);
 w.shutdown(Duration::from_secs(5)).await.unwrap();let w=f.start();assert_eq!(w.search(c.clone(),SearchRequest::Page{cursor},CancellationToken::new()).await.unwrap_err().code,ErrorCode::SearchExpired);assert_eq!(page(&w,c,request("Fresh88")).await.items[0].record_id,visible);w.shutdown(Duration::from_secs(5)).await.unwrap();
});
}
#[test]
fn s06_declared_production_gold_v2_shared_service() {
    rt().block_on(async{
 let gold:serde_json::Value=serde_json::from_str(include_str!("../../../../tests/fixtures/search/gold-v2.json")).unwrap();
 let(f,w)=fresh().await;w.shutdown(Duration::from_secs(5)).await.unwrap();let mut db=f.db();let mut categories=HashMap::new();
 for card in gold["cards"].as_array().unwrap(){if let Some(cat)=card["category_id"].as_str(){if !categories.contains_key(cat){categories.insert(cat.to_owned(),category(&mut db,card["category_label"].as_str().unwrap()));}}}
 for card in gold["cards"].as_array().unwrap(){let id=KnowledgeId(Uuid::parse_str(card["record_id"].as_str().unwrap()).unwrap());seed(&mut db,id,card["title"].as_str().unwrap(),card["description"].as_str().unwrap(),card["category_id"].as_str().map(|s|categories[s]),1);}
 drop(db);let w=f.start();let c=ctx(Uuid::new_v4(),1);let mut rows=Vec::new();let mut recall=0.;let mut count=0.;let mut fp=0;
 for q in gold["queries"].as_array().unwrap(){let mut r=request(q["text"].as_str().unwrap());r.limit=10;if let Some(cat)=q["category_id"].as_str(){r.filters.category=CategoryFilter::AnyOf{ids:vec![categories[cat]]};}
 let result=page(&w,c.clone(),r).await;let ids:Vec<_>=result.items.iter().map(|h|h.record_id.0.to_string()).collect();let expected=q["relevance"].as_object().unwrap();
 if expected.is_empty(){fp+=ids.len();assert!(ids.is_empty(),"negative {}: {:?}",q["id"],ids)}else{let found=ids.iter().take(5).filter(|id|expected.contains_key(*id)).count();let r=found as f64/expected.len() as f64;recall+=r;count+=1.;assert!(r>=0.85,"gold {}: {:?}",q["id"],ids);}
 if let Some(top)=q["expected_top1"].as_str(){assert_eq!(ids.first().map(String::as_str),Some(top),"exact {}",q["id"])}
 for id in q["forbidden_ids"].as_array().unwrap(){assert!(!ids.contains(&id.as_str().unwrap().to_owned()),"forbidden {}",q["id"]);}
 rows.push(serde_json::json!({"query_id":q["id"],"split":q["split"],"group":q["group"],"actual_ids":ids}));
 }
 assert_eq!(rows.len(),120);assert_eq!(fp,0);assert!(recall/count>=0.85);
 let out=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary").join(Uuid::new_v4().to_string());fs::create_dir(&out).unwrap();fs::write(out.join("gold-results.json"),serde_json::to_vec_pretty(&serde_json::json!({"configuration":search_projection::FINGERPRINT,"macroRecallAt5":recall/count,"negative_false_positives":fp,"queries":rows})).unwrap()).unwrap();println!("gold evidence={}",out.file_name().unwrap().to_string_lossy());w.shutdown(Duration::from_secs(5)).await.unwrap();
});
}
#[test]
fn s06_all_scope_masks_ttl_eviction_and_reopen_repair() {
    rt().block_on(async {
        let (f, w) = fresh().await;
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        let mut db = f.db();
        let mut expected = Vec::new();
        for mask in [1, 3, 5, 7] {
            let id = KnowledgeId(Uuid::new_v4());
            seed(
                &mut db,
                id,
                "ScopeMatrix77",
                "sensitive description",
                None,
                mask,
            );
            expected.push((id, mask));
        }
        // Reopen repairs a removed contentful FTS row, not source or historical data.
        db.execute(
            "DELETE FROM image_fts WHERE rowid=(SELECT record_pk FROM search_documents LIMIT 1)",
            [],
        )
        .unwrap();
        drop(db);
        let w = f.start();
        for mask in [1, 3, 5, 7] {
            let c = ctx(Uuid::new_v4(), mask);
            let result = page(&w, c, request("ScopeMatrix77")).await;
            let mut ids = result
                .items
                .iter()
                .map(|h| h.record_id.0)
                .collect::<Vec<_>>();
            ids.sort();
            let mut permitted = expected
                .iter()
                .filter(|(_, required)| mask & required == *required)
                .map(|(id, _)| id.0)
                .collect::<Vec<_>>();
            permitted.sort();
            assert_eq!(ids, permitted);
        }
        let c = ctx(Uuid::new_v4(), 7);
        for (from, to, n) in [
            ("2026-10-03T13:00:00+01:00", "2026-10-03T12:00:00.001Z", 4),
            ("2026-10-03T12:00:00.000001Z", "2026-10-03T12:00:01Z", 0),
            ("2026-10-03T11:59:59Z", "2026-10-03T12:00:00Z", 0),
        ] {
            let mut r = request("ScopeMatrix77");
            r.filters.date = DateFilter::UtcRange {
                axis: DateAxis::Ingested,
                from_inclusive: from.into(),
                to_exclusive: to.into(),
            };
            assert_eq!(page(&w, c.clone(), r).await.items.len(), n);
        }
        let mut r = request("ScopeMatrix77");
        r.filters.source_kind = Some(SourceKind::Capture);
        assert!(page(&w, c.clone(), r).await.items.is_empty());
        let mut r = request("ScopeMatrix77");
        r.filters.source_kind = Some(SourceKind::ImportedImage);
        assert_eq!(page(&w, c, r).await.items.len(), 4);
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        let db = f.db();
        let c = ctx(Uuid::new_v4(), 7);
        let mut cache = Cache::default();
        let mut r = request("");
        r.limit = 1;
        let SearchReply::Page(p) =
            handle(&db, &mut cache, c.clone(), SearchRequest::Start(r.clone())).unwrap()
        else {
            panic!("page")
        };
        let cursor = p.next_cursor.unwrap();
        cache.windows.get_mut(&r.search_id.0).unwrap().created =
            Instant::now() - Duration::from_secs(301);
        assert_eq!(
            handle(&db, &mut cache, c.clone(), SearchRequest::Page { cursor })
                .unwrap_err()
                .code,
            ErrorCode::SearchExpired
        );
        r.text = "changed".into();
        assert_eq!(
            handle(&db, &mut cache, c, SearchRequest::Start(r))
                .unwrap_err()
                .code,
            ErrorCode::IdempotencyConflict
        );
        drop(db);
    });
}
#[test]
fn s06_queued_timeout_keeps_writer_and_next_reader_alive() {
    rt().block_on(async {
        let (f, w) = fresh().await;
        w.shutdown(Duration::from_secs(5)).await.unwrap();
        let mut db = f.db();
        let id = KnowledgeId(Uuid::new_v4());
        seed(&mut db, id, "BeforeTimeout77", "synthetic", None, 1);
        drop(db);
        let w = f.start();
        let (entered, rx) = mpsc::channel();
        let (release1, wait1) = mpsc::channel();
        let (release2, wait2) = mpsc::channel();
        w.search_endpoint
            .sender
            .try_send(ReadCommand::Stall(entered.clone(), wait1))
            .unwrap();
        w.search_endpoint
            .sender
            .try_send(ReadCommand::Stall(entered, wait2))
            .unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let c = ctx(Uuid::new_v4(), 1);
        assert_eq!(
            w.search(
                c.clone(),
                SearchRequest::Start(request("BeforeTimeout77")),
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .code,
            ErrorCode::WaitTimeout
        );
        w.knowledge(
            c.clone(),
            KnowledgeRequest::Edit(CardEdit {
                operation_id: KnowledgeId(Uuid::new_v4()),
                record_id: id,
                expected_record_revision: SqlRevision(2),
                expected_catalog_revision: SqlRevision(1),
                expected_category_revision: None,
                patch: CardPatch {
                    title: FieldPatch::Set {
                        value: CardTitle("AfterTimeout88".into()),
                    },
                    ..Default::default()
                },
            }),
        )
        .await
        .unwrap();
        release1.send(()).unwrap();
        release2.send(()).unwrap();
        assert_eq!(
            page(&w, c, request("AfterTimeout88")).await.items[0].record_id,
            id
        );
        w.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}
#[path = "benchmark_tests.rs"]
mod benchmark_tests;
#[path = "graded_tests.rs"]
mod graded_tests;
