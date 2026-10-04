//! Sole SQLite-worker manual knowledge owner. No filesystem/SDK/model calls.
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use sm_application::{knowledge::KnowledgeApi, RequestContext, Scope};
use sm_domain::PrincipalId;
use sm_protocol::{
    errors::{AppError, ErrorCode},
    knowledge::*,
    values::SafeMessage,
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnowledgeError {
    Invalid,
    Permission,
    Revision,
    Replay,
    NotFound,
    CategoryUnavailable,
    NameConflict,
    Limit,
    Integrity,
    Unavailable,
}
fn sql(_: rusqlite::Error) -> KnowledgeError {
    KnowledgeError::Integrity
}
fn encode<T: Serialize>(v: &T) -> Result<String, KnowledgeError> {
    serde_json::to_string(v).map_err(|_| KnowledgeError::Invalid)
}
fn decode<T: DeserializeOwned>(s: &str) -> Result<T, KnowledgeError> {
    serde_json::from_str(s).map_err(|_| KnowledgeError::Integrity)
}
fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
fn next(r: SqlRevision) -> Result<SqlRevision, KnowledgeError> {
    r.0.checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .map(SqlRevision)
        .ok_or(KnowledgeError::Integrity)
}
fn fallback(id: KnowledgeId) -> CardTitle {
    CardTitle(format!("Изображение {}", &id.0.to_string()[..8]))
}
fn catalog_revision(db: &Connection) -> Result<SqlRevision, KnowledgeError> {
    db.query_row(
        "SELECT revision FROM category_catalog_meta WHERE singleton=1",
        [],
        |r| r.get::<_, u64>(0),
    )
    .map(SqlRevision)
    .map_err(sql)
}
fn assigned(db: &Connection, id: KnowledgeId) -> Result<u32, KnowledgeError> {
    db.query_row(
        "SELECT COUNT(*) FROM knowledge_cards WHERE category_id=?1",
        [id.0.to_string()],
        |r| r.get(0),
    )
    .map_err(sql)
}
fn category(db: &Connection, id: KnowledgeId) -> Result<CategoryView, KnowledgeError> {
    let row: Option<(u64, String, String, String)> = db
        .query_row(
            "SELECT revision,name,description,state FROM knowledge_categories WHERE id=?1",
            [id.0.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(sql)?;
    let (rev, n, d, s) = row.ok_or(KnowledgeError::NotFound)?;
    Ok(CategoryView {
        id,
        revision: SqlRevision(rev),
        name: decode(&encode(&n)?)?,
        description: decode(&encode(&d)?)?,
        state: decode(&encode(&s)?)?,
        assigned_records: assigned(db, id)?,
    })
}
fn catalog(db: &Connection) -> Result<CategoryCatalog, KnowledgeError> {
    let mut stmt = db
        .prepare("SELECT id FROM knowledge_categories ORDER BY name_key,id LIMIT 513")
        .map_err(sql)?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql)?;
    if ids.len() > 512 {
        return Err(KnowledgeError::Limit);
    }
    Ok(CategoryCatalog {
        revision: catalog_revision(db)?,
        items: ids
            .iter()
            .map(|s| {
                let id: KnowledgeId = decode(&encode(s)?)?;
                category(db, id)
            })
            .collect::<Result<_, _>>()?,
    })
}
fn source(id: KnowledgeId, asset: KnowledgeId, effective: &EffectiveCard) -> KnowledgeSource {
    KnowledgeSource {
        source_version: "manual_effective_v1".into(),
        record_id: id,
        source_asset_id: asset,
        title: effective.title.clone(),
        description: effective.description.clone(),
        category_id: effective.category_id,
        category_label: effective.category_label.clone(),
        required_read_scopes: vec!["read_metadata".into()],
    }
}
pub(crate) fn read(db: &Connection, id: KnowledgeId) -> Result<CardView, KnowledgeError> {
    read_inner(db, id, true)
}
fn read_inner(
    db: &Connection,
    id: KnowledgeId,
    check_projection: bool,
) -> Result<CardView, KnowledgeError> {
    let asset: Option<String> = db.query_row("SELECT a.id FROM records r JOIN assets a ON a.record_id=r.id AND a.asset_type='original' WHERE r.id=?1", [id.0.to_string()], |r| r.get(0)).optional().map_err(sql)?;
    let asset: KnowledgeId = decode(&encode(&asset.ok_or(KnowledgeError::NotFound)?)?)?;
    let row: Option<(u64,u64,String,String,String)> = db.query_row("SELECT record_revision,content_revision,overrides_json,effective_json,source_asset_id FROM knowledge_cards WHERE record_id=?1", [id.0.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(sql)?;
    let persisted = row.is_some();
    let (record_revision, content_revision, overrides, effective) = if let Some((r, c, o, e, a)) =
        row
    {
        if a != asset.0.to_string() {
            return Err(KnowledgeError::Integrity);
        }
        let overrides: CardPatch = decode(&o)?;
        let effective: EffectiveCard = decode(&e)?;
        let label = effective
            .category_id
            .map(|id| category(db, id))
            .transpose()?
            .map(|c| c.name);
        // Current source and effective rows must agree; no silent stale category resolver.
        if effective != sm_application::knowledge::resolve(&overrides, None, fallback(id), label) {
            return Err(KnowledgeError::Integrity);
        }
        (SqlRevision(r), SqlRevision(c), overrides, effective)
    } else {
        let o = CardPatch {
            title: FieldPatch::Inherit,
            description: FieldPatch::Inherit,
            category_id: FieldPatch::Inherit,
        };
        let e = sm_application::knowledge::resolve(&o, None, fallback(id), None);
        (SqlRevision(1), SqlRevision(1), o, e)
    };
    let mut source = source(id, asset, &effective);
    let stored: Option<(u64,String,String)> = db.query_row("SELECT content_revision,source_json,source_sha256 FROM knowledge_source_documents WHERE record_id=?1", [id.0.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
    if persisted != stored.is_some() {
        return Err(KnowledgeError::Integrity);
    }
    let mut projection_state = ProjectionState::HandlerUnavailable;
    if let Some((c, s, h)) = stored {
        let prior: KnowledgeSource = decode(&s)?;
        if !prior.required_read_scopes.contains(&"read_metadata".into())
            || prior.required_read_scopes.iter().any(|s| {
                !matches!(
                    s.as_str(),
                    "read_metadata" | "read_source_metadata" | "read_paths"
                )
            })
        {
            return Err(KnowledgeError::Integrity);
        }
        source.required_read_scopes = prior.required_read_scopes;
        if c != content_revision.0 || s != encode(&source)? || h != hash(&s) {
            return Err(KnowledgeError::Integrity);
        }
        if check_projection {
            let debt: Option<(u64,String)> = db.query_row("SELECT target_content_revision,source_sha256 FROM knowledge_projection_debts WHERE record_id=?1", [id.0.to_string()], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql)?;
            let projected: Option<(u64,String,String,String,u8)> = db.query_row("SELECT content_revision,source_sha256,title,description,required_scope_mask FROM search_documents WHERE record_id=?1 AND source_asset_id=?2", params![id.0.to_string(),asset.0.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(sql)?;
            if debt.is_none()
                && projected
                    == Some((
                        c,
                        h.clone(),
                        source.title.0.clone(),
                        source.description.0.clone(),
                        crate::search_projection::scope_mask(&source.required_read_scopes)?,
                    ))
            {
                projection_state = ProjectionState::Current;
            } else if debt != Some((c, h)) {
                return Err(KnowledgeError::Integrity);
            }
        }
    }
    let digest = hash(&encode(&source)?);
    Ok(CardView {
        record_id: id,
        source_asset_id: asset,
        record_revision,
        content_revision,
        overrides,
        effective,
        source,
        source_sha256: digest,
        projection_state,
    })
}
/// Worker-only S06 adapter seam; it MUST run synchronously in the existing transaction.
/// No async wait, model, image decode or nested SQLite connection is permitted.
pub trait SearchProjectionTx {
    fn on_source_changed(
        &self,
        tx: &Transaction<'_>,
        previous: Option<SqlRevision>,
        current: &CardView,
    ) -> Result<(), KnowledgeError>;
}
fn persist(
    tx: &Transaction<'_>,
    previous: &CardView,
    mut card: CardView,
    hook: &dyn SearchProjectionTx,
) -> Result<CardView, KnowledgeError> {
    card.source = source(card.record_id, card.source_asset_id, &card.effective);
    // Manual patch/category changes NEVER declassify inherited known sensitive input.
    card.source.required_read_scopes = previous.source.required_read_scopes.clone();
    card.source_sha256 = hash(&encode(&card.source)?);
    let changed = card.source_sha256 != previous.source_sha256;
    card.content_revision = if changed {
        next(previous.content_revision)?
    } else {
        previous.content_revision
    };
    let existed: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM knowledge_cards WHERE record_id=?1)",
            [card.record_id.0.to_string()],
            |r| r.get(0),
        )
        .map_err(sql)?;
    tx.execute("INSERT INTO knowledge_cards VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(record_id) DO UPDATE SET record_revision=excluded.record_revision,content_revision=excluded.content_revision,overrides_json=excluded.overrides_json,effective_json=excluded.effective_json,source_asset_id=excluded.source_asset_id,category_id=excluded.category_id", params![card.record_id.0.to_string(),card.record_revision.0,card.content_revision.0,encode(&card.overrides)?,encode(&card.effective)?,card.source_asset_id.0.to_string(),card.effective.category_id.map(|v| v.0.to_string())]).map_err(sql)?;
    if changed || !existed {
        tx.execute("INSERT INTO knowledge_source_documents VALUES(?1,?2,'manual_effective_v1',?3,?4,strftime('%Y-%m-%dT%H:%M:%fZ','now')) ON CONFLICT(record_id) DO UPDATE SET content_revision=excluded.content_revision,source_json=excluded.source_json,source_sha256=excluded.source_sha256,updated_at_utc=excluded.updated_at_utc", params![card.record_id.0.to_string(),card.content_revision.0,encode(&card.source)?,card.source_sha256]).map_err(sql)?;
        hook.on_source_changed(tx, existed.then_some(previous.content_revision), &card)?;
    }
    card.projection_state = ProjectionState::Current;
    Ok(card)
}
/// Materialize new Saved fallback source and projection inside the caller's transaction.
pub(crate) fn initial(tx: &Transaction<'_>, id: KnowledgeId) -> Result<(), KnowledgeError> {
    let card = read_inner(tx, id, false)?;
    persist(
        tx,
        &card,
        card.clone(),
        &crate::search_projection::Projection,
    )?;
    Ok(())
}
/// Bounded per-transaction backfill/repair before reader/listener publication, no model/file IO.
pub(crate) fn backfill(db: &mut Connection) -> Result<(), KnowledgeError> {
    let mut after = 0i64;
    loop {
        let ids: Vec<(i64, String)> = {
            let mut s=db.prepare("SELECT r.rowid,r.id FROM records r JOIN assets a ON a.record_id=r.id AND a.asset_type='original' WHERE r.rowid>?1 ORDER BY r.rowid LIMIT 128").map_err(sql)?;
            let rows = s
                .query_map([after], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        if ids.is_empty() {
            break;
        }
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        for (pk, id) in &ids {
            let id: KnowledgeId = decode(&encode(id)?)?;
            let c = read_inner(&tx, id, false)?;
            let existed: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM knowledge_cards WHERE record_id=?1)",
                    [id.0.to_string()],
                    |r| r.get(0),
                )
                .map_err(sql)?;
            if !existed {
                initial(&tx, id)?;
            } else {
                crate::search_projection::Projection.on_source_changed(&tx, None, &c)?;
            }
            after = *pk;
        }
        tx.commit().map_err(sql)?;
    }
    Ok(())
}
fn apply<T: Clone>(current: &mut FieldPatch<T>, patch: &FieldPatch<T>) {
    if !matches!(patch, FieldPatch::Keep) {
        *current = patch.clone();
    }
}
fn edit(
    tx: &Transaction<'_>,
    req: &CardEdit,
    hook: &dyn SearchProjectionTx,
) -> Result<KnowledgeReply, KnowledgeError> {
    let before = read(tx, req.record_id)?;
    if before.record_revision != req.expected_record_revision
        || catalog_revision(tx)? != req.expected_catalog_revision
    {
        return Err(KnowledgeError::Revision);
    }
    match req.patch.category_id {
        FieldPatch::Set { value: Some(id) } => {
            let cat = category(tx, id)?;
            if cat.state != CategoryState::Active {
                return Err(KnowledgeError::CategoryUnavailable);
            }
            if req.expected_category_revision != Some(cat.revision) {
                return Err(KnowledgeError::Revision);
            }
        }
        _ if req.expected_category_revision.is_some() => return Err(KnowledgeError::Invalid),
        _ => {}
    }
    let mut card = before.clone();
    apply(&mut card.overrides.title, &req.patch.title);
    apply(&mut card.overrides.description, &req.patch.description);
    apply(&mut card.overrides.category_id, &req.patch.category_id);
    if card.overrides != before.overrides {
        card.record_revision = next(before.record_revision)?;
    }
    let category_id = match card.overrides.category_id {
        FieldPatch::Set { value } => value,
        _ => None,
    };
    let label = category_id
        .map(|id| category(tx, id))
        .transpose()?
        .map(|c| c.name);
    card.effective =
        sm_application::knowledge::resolve(&card.overrides, None, fallback(req.record_id), label);
    let card = persist(tx, &before, card, hook)?;
    let changed = before.source_sha256 != card.source_sha256;
    Ok(KnowledgeReply::Edited(Box::new(CardReceipt {
        operation_id: req.operation_id,
        card,
        effective_changed: changed,
    })))
}
#[link(name = "Normaliz")]
unsafe extern "system" {
    fn NormalizeString(
        form: i32,
        source: *const u16,
        source_len: i32,
        destination: *mut u16,
        destination_len: i32,
    ) -> i32;
}
/// Versioned NFC + full Unicode casefold (unicase pinned2.9). Windows memory-only NFC.
fn name_key(name: &CategoryName) -> Result<(CategoryName, String), KnowledgeError> {
    let s = name.0.trim();
    let input: Vec<u16> = s.encode_utf16().collect();
    let mut output = vec![0u16; 1024];
    let n = unsafe {
        NormalizeString(
            1,
            input.as_ptr(),
            input.len() as i32,
            output.as_mut_ptr(),
            output.len() as i32,
        )
    };
    if n <= 0 {
        return Err(KnowledgeError::Invalid);
    }
    output.truncate(n as usize);
    let normalized = String::from_utf16(&output).map_err(|_| KnowledgeError::Invalid)?;
    let name: CategoryName = decode(&encode(&normalized)?)?;
    let key = unicase::UniCase::unicode(normalized).to_folded_case();
    Ok((name, key))
}
fn mutate_category(
    tx: &Transaction<'_>,
    req: &CategoryMutation,
    hook: &dyn SearchProjectionTx,
) -> Result<KnowledgeReply, KnowledgeError> {
    let old_revision = catalog_revision(tx)?;
    if old_revision != req.expected_catalog_revision {
        return Err(KnowledgeError::Revision);
    }
    let (mut cat, existing) = match &req.change {
        CategoryChange::Create { name, description } => (
            CategoryView {
                id: KnowledgeId(Uuid::new_v4()),
                revision: SqlRevision(1),
                name: name.clone(),
                description: description.clone(),
                state: CategoryState::Active,
                assigned_records: 0,
            },
            false,
        ),
        CategoryChange::Rename {
            category_id,
            expected_revision,
            name,
            description,
        } => {
            let mut c = category(tx, *category_id)?;
            if c.revision != *expected_revision {
                return Err(KnowledgeError::Revision);
            }
            if c.state == CategoryState::Deleted {
                return Err(KnowledgeError::CategoryUnavailable);
            }
            c.name = name.clone();
            c.description = description.clone();
            c.revision = next(c.revision)?;
            (c, true)
        }
        CategoryChange::State {
            category_id,
            expected_revision,
            state,
            expected_assigned_records,
        } => {
            let mut c = category(tx, *category_id)?;
            if c.revision != *expected_revision || c.assigned_records != *expected_assigned_records
            {
                return Err(KnowledgeError::Revision);
            }
            // Restore only archived, not a deleted UUID/history.
            if c.state == CategoryState::Deleted {
                return Err(KnowledgeError::CategoryUnavailable);
            }
            c.state = *state;
            c.revision = next(c.revision)?;
            (c, true)
        }
    };
    let (name, key) = name_key(&cat.name)?;
    cat.name = name;
    if cat.state != CategoryState::Deleted {
        let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM knowledge_categories WHERE name_key=?1 AND id!=?2 AND state!='deleted')", params![key,cat.id.0.to_string()], |r|r.get(0)).map_err(sql)?;
        if duplicate {
            return Err(KnowledgeError::NameConflict);
        }
    }
    let active: u32 = tx
        .query_row(
            "SELECT COUNT(*) FROM knowledge_categories WHERE state='active' AND id!=?1",
            [cat.id.0.to_string()],
            |r| r.get(0),
        )
        .map_err(sql)?;
    let total: u32 = tx
        .query_row("SELECT COUNT(*) FROM knowledge_categories", [], |r| {
            r.get(0)
        })
        .map_err(sql)?;
    if (cat.state == CategoryState::Active && active >= 128) || (!existing && total >= 512) {
        return Err(KnowledgeError::Limit);
    }
    // Bounded refusal, never truncate an atomic rename/delete maintenance operation.
    if cat.assigned_records > 4096 {
        return Err(KnowledgeError::Limit);
    }
    let mut stmt=tx.prepare("SELECT record_id FROM knowledge_cards WHERE category_id=?1 ORDER BY record_id LIMIT 4097").map_err(sql)?;
    let ids = stmt
        .query_map([cat.id.0.to_string()], |r| r.get::<_, String>(0))
        .map_err(sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql)?;
    drop(stmt);
    let previous = ids
        .iter()
        .map(|s| {
            let id = decode(&encode(s)?)?;
            read(tx, id)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let statement = if existing {
        "UPDATE knowledge_categories SET revision=?2,name=?3,description=?4,name_key=?5,state=?6 WHERE id=?1"
    } else {
        // Even an accidental UUID collision can never reuse a deleted identity/history.
        "INSERT INTO knowledge_categories VALUES(?1,?2,?3,?4,'win_nfc_unicase2.9_v1',?5,?6)"
    };
    tx.execute(
        statement,
        params![
            cat.id.0.to_string(),
            cat.revision.0,
            cat.name.0,
            cat.description.0,
            key,
            match cat.state {
                CategoryState::Active => "active",
                CategoryState::Archived => "archived",
                CategoryState::Deleted => "deleted",
            }
        ],
    )
    .map_err(sql)?;
    let mut affected = 0;
    for before in previous {
        let mut card = before.clone();
        if cat.state == CategoryState::Deleted {
            card.overrides.category_id = FieldPatch::Set { value: None };
        }
        let label = if cat.state == CategoryState::Deleted {
            None
        } else {
            Some(cat.name.clone())
        };
        card.effective = sm_application::knowledge::resolve(
            &card.overrides,
            None,
            fallback(card.record_id),
            label,
        );
        if card.effective != before.effective || card.overrides != before.overrides {
            card.record_revision = next(card.record_revision)?;
            persist(tx, &before, card, hook)?;
            affected += 1;
        }
    }
    cat.assigned_records = assigned(tx, cat.id)?;
    tx.execute(
        "INSERT INTO knowledge_category_versions VALUES(?1,?2,?3)",
        params![cat.id.0.to_string(), cat.revision.0, encode(&cat)?],
    )
    .map_err(sql)?;
    let catalog_revision = next(old_revision)?;
    tx.execute(
        "UPDATE category_catalog_meta SET revision=?1 WHERE singleton=1",
        [catalog_revision.0],
    )
    .map_err(sql)?;
    Ok(KnowledgeReply::Category(CategoryReceipt {
        operation_id: req.operation_id,
        category: cat,
        catalog_revision,
        affected_records: affected,
        projection_state: ProjectionState::Current,
    }))
}
pub(crate) fn handle_authorized(
    db: &mut Connection,
    ctx: RequestContext,
    req: KnowledgeRequest,
) -> Result<KnowledgeReply, KnowledgeError> {
    if !ctx.allows(required_scope(&req)) {
        return Err(KnowledgeError::Permission);
    }
    // Serialized on the writer, including replay, so a late sensitive publisher cannot
    // race a read/mutation-scope check. Never issue restricted receipt payload to caller.
    let record = match &req {
        KnowledgeRequest::Read { record_id } => Some(*record_id),
        KnowledgeRequest::Edit(r) => Some(r.record_id),
        _ => None,
    };
    if let Some(record) = record {
        let view = read(db, record)?;
        for scope in &view.source.required_read_scopes {
            let required = match scope.as_str() {
                "read_metadata" => Scope::ReadMetadata,
                "read_source_metadata" => Scope::ReadSourceMetadata,
                "read_paths" => Scope::ReadPaths,
                _ => return Err(KnowledgeError::Integrity),
            };
            if !ctx.allows(required) {
                return Err(KnowledgeError::Permission);
            }
        }
    }
    handle_with_hook(
        db,
        ctx.principal_id(),
        req,
        &crate::search_projection::Projection,
    )
}
#[cfg(test)]
pub(crate) fn handle(
    db: &mut Connection,
    principal: PrincipalId,
    req: KnowledgeRequest,
) -> Result<KnowledgeReply, KnowledgeError> {
    handle_with_hook(db, principal, req, &crate::search_projection::Projection)
}
fn handle_with_hook(
    db: &mut Connection,
    principal: PrincipalId,
    req: KnowledgeRequest,
    hook: &dyn SearchProjectionTx,
) -> Result<KnowledgeReply, KnowledgeError> {
    // Validate native constructors too; domain wrappers deliberately have no unchecked wire path.
    let request: KnowledgeRequest =
        serde_json::from_str(&encode(&req)?).map_err(|_| KnowledgeError::Invalid)?;
    match request {
        KnowledgeRequest::Read { record_id } => {
            return read(db, record_id).map(|c| KnowledgeReply::Card(Box::new(c)))
        }
        KnowledgeRequest::Categories {} => return catalog(db).map(KnowledgeReply::Categories),
        _ => {}
    }
    let operation = match &request {
        KnowledgeRequest::Edit(r) => r.operation_id,
        KnowledgeRequest::Category(r) => r.operation_id,
        _ => unreachable!(),
    };
    let fingerprint = hash(&format!("manual-knowledge-v1:{}", encode(&request)?));
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql)?;
    let replay:Option<(String,String,String)>=tx.query_row("SELECT principal_id,request_fingerprint,result_json FROM knowledge_write_receipts WHERE operation_id=?1", [operation.0.to_string()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
    if let Some((p, f, j)) = replay {
        if p != principal.as_uuid().to_string() || f != fingerprint {
            return Err(KnowledgeError::Replay);
        }
        return decode(&j);
    }
    let reply = match &request {
        KnowledgeRequest::Edit(r) => edit(&tx, r, hook)?,
        KnowledgeRequest::Category(r) => mutate_category(&tx, r, hook)?,
        _ => unreachable!(),
    };
    tx.execute("INSERT INTO knowledge_write_receipts VALUES(?1,?2,?3,?4,strftime('%Y-%m-%dT%H:%M:%fZ','now'))", params![operation.0.to_string(),principal.as_uuid().to_string(),fingerprint,encode(&reply)?]).map_err(sql)?;
    tx.commit().map_err(sql)?;
    Ok(reply)
}
pub fn required_scope(req: &KnowledgeRequest) -> Scope {
    match req {
        KnowledgeRequest::Read { .. } | KnowledgeRequest::Categories {} => Scope::ReadMetadata,
        KnowledgeRequest::Edit(_) => Scope::WriteMetadata,
        KnowledgeRequest::Category(_) => Scope::ManageCategories,
    }
}
fn app_error(error: KnowledgeError) -> AppError {
    let code = match error {
        KnowledgeError::Invalid => ErrorCode::InvalidRequest,
        KnowledgeError::Permission => ErrorCode::PermissionDenied,
        KnowledgeError::Revision => ErrorCode::RevisionConflict,
        KnowledgeError::Replay => ErrorCode::IdempotencyConflict,
        KnowledgeError::NotFound => ErrorCode::AssetNotFound,
        KnowledgeError::CategoryUnavailable => ErrorCode::CategoryNotAvailable,
        KnowledgeError::NameConflict => ErrorCode::CategoryNameConflict,
        KnowledgeError::Limit => ErrorCode::CategoryCatalogTooLarge,
        KnowledgeError::Integrity => ErrorCode::AssetIntegrityFailed,
        KnowledgeError::Unavailable => ErrorCode::WaitTimeout,
    };
    AppError {
        code,
        safe_message: SafeMessage::parse("Metadata request rejected.").expect("constant"),
        retryable: false,
    }
}
#[async_trait::async_trait]
impl KnowledgeApi for crate::StorageWorker {
    async fn knowledge(
        &self,
        ctx: RequestContext,
        req: KnowledgeRequest,
    ) -> Result<KnowledgeReply, AppError> {
        if !ctx.allows(required_scope(&req)) {
            return Err(AppError {
                code: ErrorCode::PermissionDenied,
                safe_message: SafeMessage::parse("Metadata request rejected.").expect("constant"),
                retryable: false,
            });
        }
        self.knowledge_authorized(ctx, req).await.map_err(app_error)
    }
}
#[cfg(test)]
mod tests;
