//! Storage-owned bounded read workers and fixed-window SearchService. No SDK dependency.
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension};
use sm_application::{search::SearchApi, RequestContext, Scope};
use sm_protocol::{
    errors::{AppError, ErrorCode},
    knowledge::{KnowledgeId, SqlRevision},
    search::*,
    values::SafeMessage,
};
use std::{
    collections::HashMap,
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn error(code: ErrorCode) -> AppError {
    AppError {
        code,
        safe_message: SafeMessage::parse("Search request rejected.").expect("constant"),
        retryable: false,
    }
}
fn sql(_: rusqlite::Error) -> AppError {
    error(ErrorCode::AssetIntegrityFailed)
}
fn granted(ctx: &RequestContext) -> u8 {
    1 | if ctx.allows(Scope::ReadSourceMetadata) {
        2
    } else {
        0
    } | if ctx.allows(Scope::ReadPaths) { 4 } else { 0 }
}
#[derive(Clone)]
struct Candidate {
    id: String,
    revision: u64,
    exact: bool,
    lexical: bool,
}
struct Window {
    principal: Uuid,
    mask: u8,
    params: SearchStart,
    epoch: u64,
    created: Instant,
    ids: Vec<Candidate>,
    truncated: bool,
    cursors: HashMap<Uuid, usize>,
}
#[derive(Default)]
pub(crate) struct Cache {
    windows: HashMap<Uuid, Window>,
    seen: HashMap<Uuid, (Uuid, [u8; 32])>,
}
pub(crate) enum ReadCommand {
    Query(
        RequestContext,
        Box<SearchRequest>,
        CancellationToken,
        Instant,
        oneshot::Sender<Result<SearchReply, AppError>>,
    ),
    #[cfg(test)]
    Stall(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>),
    Stop,
}
#[derive(Clone)]
pub(crate) struct Endpoint {
    pub(crate) sender: SyncSender<ReadCommand>,
}
type ReaderQueue = Arc<Mutex<Receiver<ReadCommand>>>;
type SessionCache = Arc<Mutex<Cache>>;
pub(crate) fn channel() -> (Endpoint, ReaderQueue, SessionCache) {
    let (sender, receiver) = mpsc::sync_channel(32);
    (
        Endpoint { sender },
        Arc::new(Mutex::new(receiver)),
        Arc::new(Mutex::new(Cache::default())),
    )
}
pub(crate) fn reader(
    db: Connection,
    receiver: Arc<Mutex<Receiver<ReadCommand>>>,
    cache: Arc<Mutex<Cache>>,
) {
    loop {
        let command = match receiver.lock() {
            Ok(r) => r.recv(),
            Err(_) => return,
        };
        let Ok(command) = command else { return };
        match command {
            ReadCommand::Stop => return,
            #[cfg(test)]
            ReadCommand::Stall(entered, release) => {
                let _ = entered.send(());
                let _ = release.recv_timeout(Duration::from_secs(10));
            }
            ReadCommand::Query(ctx, req, cancel, deadline, tx) => {
                if tx.is_closed() {
                    continue;
                }
                let token = cancel.clone();
                db.progress_handler(
                    1000,
                    Some(move || token.is_cancelled() || Instant::now() >= deadline),
                );
                let result = if cancel.is_cancelled() {
                    Err(error(ErrorCode::SearchCancelled))
                } else if Instant::now() >= deadline {
                    Err(error(ErrorCode::WaitTimeout))
                } else {
                    cache
                        .lock()
                        .map_err(|_| error(ErrorCode::SearchBusy))
                        .and_then(|mut cache| handle(&db, &mut cache, ctx, *req))
                };
                db.progress_handler(0, None::<fn() -> bool>);
                let result = if cancel.is_cancelled() {
                    Err(error(ErrorCode::SearchCancelled))
                } else if Instant::now() >= deadline {
                    Err(error(ErrorCode::WaitTimeout))
                } else {
                    result
                };
                let _ = tx.send(result);
            }
        }
    }
}
pub(crate) fn open_reader(path: &std::path::Path) -> Result<Connection, crate::StorageError> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| crate::StorageError::Unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(|_| crate::StorageError::Unavailable)?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON;")
        .map_err(|_| crate::StorageError::Unavailable)?;
    Ok(db)
}
#[async_trait::async_trait]
impl SearchApi for crate::StorageWorker {
    async fn search(
        &self,
        ctx: RequestContext,
        req: SearchRequest,
        cancel: CancellationToken,
    ) -> Result<SearchReply, AppError> {
        if !ctx.allows(Scope::ReadMetadata) {
            return Err(error(ErrorCode::PermissionDenied));
        }
        let req: SearchRequest = serde_json::from_value(
            serde_json::to_value(req).map_err(|_| error(ErrorCode::InvalidRequest))?,
        )
        .map_err(|_| error(ErrorCode::InvalidRequest))?;
        if let SearchRequest::Start(r) = &req {
            sm_protocol::search::validate(r).map_err(error)?;
        }
        let child = cancel.child_token();
        let _cancel_on_drop = child.clone().drop_guard();
        let (tx, rx) = oneshot::channel();
        {
            let gate = self.gate.lock().map_err(|_| error(ErrorCode::SearchBusy))?;
            if gate.closing {
                return Err(error(ErrorCode::SearchBusy));
            }
            self.search_endpoint
                .sender
                .try_send(ReadCommand::Query(
                    ctx,
                    Box::new(req),
                    child,
                    Instant::now() + Duration::from_secs(2),
                    tx,
                ))
                .map_err(|_| error(ErrorCode::SearchBusy))?;
        }
        tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(error(ErrorCode::SearchCancelled)),
            result=tokio::time::timeout(Duration::from_secs(3),rx)=>result.map_err(|_|error(ErrorCode::WaitTimeout))?.map_err(|_|error(ErrorCode::SearchBusy))?,
        }
    }
}
fn epoch(db: &Connection) -> Result<u64, AppError> {
    db.query_row("SELECT projection_epoch FROM search_projection_meta WHERE singleton=1 AND lexical_fingerprint=?1",[crate::search_projection::FINGERPRINT],|r|r.get(0)).map_err(sql)
}
fn handle(
    db: &Connection,
    cache: &mut Cache,
    ctx: RequestContext,
    req: SearchRequest,
) -> Result<SearchReply, AppError> {
    if !ctx.allows(Scope::ReadMetadata) {
        return Err(error(ErrorCode::PermissionDenied));
    }
    let principal = ctx.principal_id().as_uuid();
    let mask = granted(&ctx);
    cache
        .windows
        .retain(|_, w| w.created.elapsed() < Duration::from_secs(300));
    match req {
        SearchRequest::Cancel { search_id } => {
            if !cache
                .windows
                .get(&search_id.0)
                .is_some_and(|w| w.principal == principal)
            {
                return Err(error(ErrorCode::SearchExpired));
            }
            cache.windows.remove(&search_id.0);
            Ok(SearchReply::Cancelled { search_id })
        }
        SearchRequest::Page { cursor } => {
            let w = cache
                .windows
                .values_mut()
                .find(|w| w.principal == principal && w.cursors.contains_key(&cursor.0))
                .ok_or_else(|| error(ErrorCode::SearchExpired))?;
            let offset = w.cursors[&cursor.0];
            page(db, w, mask, offset).map(SearchReply::Page)
        }
        SearchRequest::Start(r) => {
            sm_protocol::search::validate(&r).map_err(error)?;
            use sha2::Digest;
            let fingerprint: [u8; 32] = sha2::Sha256::digest(
                serde_json::to_vec(&r).map_err(|_| error(ErrorCode::InvalidRequest))?,
            )
            .into();
            if let Some((owner, prior)) = cache.seen.get(&r.search_id.0) {
                if *owner != principal {
                    return Err(error(ErrorCode::SearchExpired));
                }
                if *prior != fingerprint {
                    return Err(error(ErrorCode::IdempotencyConflict));
                }
                if !cache.windows.contains_key(&r.search_id.0) {
                    return Err(error(ErrorCode::SearchExpired));
                }
            } else if cache.seen.len() >= 8192 {
                return Err(error(ErrorCode::SearchBusy));
            }
            if let Some(w) = cache.windows.get_mut(&r.search_id.0) {
                if w.principal != principal {
                    return Err(error(ErrorCode::SearchExpired));
                }
                if w.params != r {
                    return Err(error(ErrorCode::IdempotencyConflict));
                }
                return page(db, w, mask, 0).map(SearchReply::Page);
            }
            // Fixed bounded LRU-style eviction; evicted cursors explicitly expire.
            while cache.windows.len() >= 32
                || cache
                    .windows
                    .values()
                    .filter(|w| w.principal == principal)
                    .count()
                    >= 4
            {
                let per = cache
                    .windows
                    .values()
                    .filter(|w| w.principal == principal)
                    .count()
                    >= 4;
                let id = cache
                    .windows
                    .iter()
                    .filter(|(_, w)| !per || w.principal == principal)
                    .min_by_key(|(_, w)| w.created)
                    .map(|(id, _)| *id)
                    .ok_or_else(|| error(ErrorCode::SearchBusy))?;
                cache.windows.remove(&id);
            }
            let tx = db.unchecked_transaction().map_err(sql)?;
            let e = epoch(&tx)?;
            let mut ids = retrieve(&tx, &r, mask)?;
            let truncated = ids.len() > r.candidate_depth as usize;
            ids.truncate(r.candidate_depth as usize);
            let mut w = Window {
                principal,
                mask,
                params: r,
                epoch: e,
                created: Instant::now(),
                ids,
                truncated,
                cursors: HashMap::new(),
            };
            let p = page_snapshot(&tx, &mut w, mask, 0)?;
            tx.commit().map_err(sql)?;
            cache
                .seen
                .insert(w.params.search_id.0, (principal, fingerprint));
            cache.windows.insert(w.params.search_id.0, w);
            Ok(SearchReply::Page(p))
        }
    }
}
fn page(db: &Connection, w: &mut Window, mask: u8, offset: usize) -> Result<SearchPage, AppError> {
    let tx = db.unchecked_transaction().map_err(sql)?;
    let p = page_snapshot(&tx, w, mask, offset)?;
    tx.commit().map_err(sql)?;
    Ok(p)
}
fn plain(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    for c in s.chars() {
        let text = if c.is_control() || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
        {
            format!("\\u{{{:x}}}", c as u32)
        } else {
            c.to_string()
        };
        let len = text.chars().count();
        if n + len > max {
            break;
        }
        out.push_str(&text);
        n += len;
    }
    out
}
fn page_snapshot(
    db: &Connection,
    w: &mut Window,
    mask: u8,
    offset: usize,
) -> Result<SearchPage, AppError> {
    if mask != w.mask {
        return Err(error(ErrorCode::PermissionDenied));
    }
    if epoch(db)? != w.epoch {
        return Err(error(ErrorCode::SearchSnapshotChanged));
    }
    let mut items = Vec::new();
    let end = (offset + w.params.limit as usize).min(w.ids.len());
    for (i, c) in w.ids[offset..end].iter().enumerate() {
        let hit:Option<SearchHit>=db.query_row("SELECT d.source_asset_id,d.title,d.description,d.category_id,d.category_text,d.exact_terms_truncated FROM search_documents d JOIN records r ON r.id=d.record_id JOIN assets a ON a.id=d.source_asset_id AND a.record_id=r.id AND a.asset_type='original' JOIN knowledge_source_documents s ON s.record_id=d.record_id AND s.content_revision=d.content_revision AND s.source_sha256=d.source_sha256 WHERE d.record_id=?1 AND d.content_revision=?2 AND (d.required_scope_mask & ?3)=d.required_scope_mask",params![c.id,c.revision,mask],|r| {
            let asset:String=r.get(0)?; let category:Option<String>=r.get(3)?;let label:String=r.get(4)?;
            Ok(SearchHit{record_id:KnowledgeId(Uuid::parse_str(&c.id).map_err(|_|rusqlite::Error::InvalidQuery)?),asset_id:KnowledgeId(Uuid::parse_str(&asset).map_err(|_|rusqlite::Error::InvalidQuery)?),content_revision:SqlRevision(c.revision),title:plain(&r.get::<_,String>(1)?,320),excerpt:plain(&r.get::<_,String>(2)?,320),category_id:category.map(|s|Uuid::parse_str(&s).map(KnowledgeId)).transpose().map_err(|_|rusqlite::Error::InvalidQuery)?,category_label:(!label.is_empty()).then(||plain(&label,80)),exact_match:c.exact,lexical_match:c.lexical,exact_terms_truncated:r.get(5)?,position:(offset+i+1) as u32})
        }).optional().map_err(sql)?;
        items.push(hit.ok_or_else(|| error(ErrorCode::SearchSnapshotChanged))?);
    }
    let next_cursor = if end < w.ids.len() {
        let id = w
            .cursors
            .iter()
            .find(|(_, n)| **n == end)
            .map(|(id, _)| *id)
            .unwrap_or_else(Uuid::new_v4);
        w.cursors.insert(id, end);
        Some(KnowledgeId(id))
    } else {
        None
    };
    Ok(SearchPage {
        search_id: w.params.search_id,
        projection_epoch: SqlRevision(w.epoch),
        candidate_depth: w.params.candidate_depth,
        window_truncated: w.truncated,
        next_cursor,
        items,
    })
}
fn retrieve(db: &Connection, r: &SearchStart, mask: u8) -> Result<Vec<Candidate>, AppError> {
    let terms = crate::search_projection::atoms(&r.text);
    if !r.text.trim().is_empty() && terms.is_empty() {
        return Err(error(ErrorCode::NoLexicalTerms));
    }
    if terms.len() > 64 {
        return Err(error(ErrorCode::InvalidRequest));
    }
    let (keys, _) = crate::search_projection::exact_keys(&[&r.text])
        .map_err(|_| error(ErrorCode::InvalidRequest))?;
    let mut binds: Vec<rusqlite::types::Value> = vec![(mask as i64).into()];
    let mut filters = "(d.required_scope_mask & ?1)=d.required_scope_mask".to_owned();
    let mut bind = |value: rusqlite::types::Value| {
        binds.push(value);
        format!("?{}", binds.len())
    };
    match &r.filters.category {
        CategoryFilter::Any {} => {}
        CategoryFilter::Uncategorized {} => filters.push_str(" AND d.category_id IS NULL"),
        CategoryFilter::AnyOf { ids } => {
            let ps: Vec<_> = ids.iter().map(|id| bind(id.0.to_string().into())).collect();
            filters.push_str(&format!(" AND d.category_id IN ({})", ps.join(",")));
        }
    }
    if let Some(kind) = r.filters.source_kind {
        filters.push_str(&format!(
            " AND d.source_kind={}",
            bind(
                match kind {
                    SourceKind::Capture => "capture",
                    SourceKind::ImportedImage => "imported_image",
                }
                .to_string()
                .into()
            )
        ))
    }
    if let DateFilter::UtcRange {
        from_inclusive,
        to_exclusive,
        ..
    } = &r.filters.date
    {
        let utc = |s: &str| {
            let ns = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                .expect("validated")
                .to_offset(time::UtcOffset::UTC)
                .unix_timestamp_nanos();
            // Ingested times are discrete milliseconds: ceil both range bounds,
            // including pre-epoch dates, rather than truncating a sub-ms boundary.
            ns.div_euclid(1_000_000) + i128::from(ns.rem_euclid(1_000_000) != 0)
        };
        let from = bind((utc(from_inclusive) as i64).into());
        let to = bind((utc(to_exclusive) as i64).into());
        filters.push_str(&format!(" AND CAST(round((julianday(d.ingested_at_utc)-2440587.5)*86400000) AS INTEGER)>={from} AND CAST(round((julianday(d.ingested_at_utc)-2440587.5)*86400000) AS INTEGER)<{to}"));
    }
    let eligible=format!("SELECT d.record_pk,d.record_id,d.content_revision,d.ingested_at_utc FROM search_documents d JOIN records r ON r.id=d.record_id AND r.rowid=d.record_pk JOIN assets a ON a.id=d.source_asset_id AND a.record_id=r.id AND a.asset_type='original' JOIN knowledge_source_documents s ON s.record_id=d.record_id AND s.content_revision=d.content_revision AND s.source_sha256=d.source_sha256 WHERE {filters}");
    let expression = terms
        .iter()
        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let fts = if terms.is_empty() {
        "SELECT record_pk,NULL AS cost FROM eligible".into()
    } else {
        let p = bind(expression.into());
        format!("SELECT image_fts.rowid AS record_pk,rank AS cost FROM image_fts WHERE image_fts MATCH {p} AND rank MATCH 'bm25(6.0,3.0,1.0,2.0,2.0,2.0,1.0)'")
    };
    let exact = if keys.is_empty() {
        "SELECT record_pk,0 AS matched FROM eligible WHERE 0".into()
    } else {
        let ps: Vec<_> = keys.iter().map(|k| bind(k.clone().into())).collect();
        format!("SELECT x.record_pk,count(DISTINCT x.term_key) AS matched FROM search_exact_terms x JOIN eligible e ON e.record_pk=x.record_pk WHERE x.term_key IN ({}) GROUP BY x.record_pk",ps.join(","))
    };
    let statement=format!("WITH eligible AS MATERIALIZED ({eligible}), lexical AS MATERIALIZED ({fts}), exact AS ({exact}), candidates AS (SELECT record_pk FROM lexical UNION SELECT record_pk FROM exact) SELECT e.record_id,e.content_revision,coalesce(x.matched,0),l.cost,l.record_pk IS NOT NULL FROM candidates c JOIN eligible e ON e.record_pk=c.record_pk LEFT JOIN lexical l ON l.record_pk=c.record_pk LEFT JOIN exact x ON x.record_pk=c.record_pk ORDER BY (coalesce(x.matched,0)={n} AND {n}>0) DESC,coalesce(x.matched,0) DESC,(l.cost IS NOT NULL) DESC,l.cost ASC,e.ingested_at_utc DESC,e.record_id ASC LIMIT {depth}",n=keys.len(),depth=r.candidate_depth+1);
    let mut stmt = db.prepare(&statement).map_err(sql)?;
    let rows = stmt
        .query_map(params_from_iter(binds), |row| {
            Ok(Candidate {
                id: row.get(0)?,
                revision: row.get(1)?,
                exact: row.get::<_, u32>(2)? > 0,
                lexical: row.get(4)?,
            })
        })
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}
#[cfg(test)]
mod tests;
