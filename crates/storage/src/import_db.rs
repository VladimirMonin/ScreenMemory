//! Only the existing SQLite owner executes these private typed commands.
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sm_application::{images::*, storage::StorageError};
use sm_domain::{ImportBatchId, ImportItemId, OperationId, PrincipalId, Revision};
use sm_isolation::CanonicalSeal;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct Item {
    pub id: Uuid,
    pub ordinal: u32,
    pub snapshot: Uuid,
    pub operation: Uuid,
    pub record: Uuid,
    pub asset: Uuid,
    pub path: String,
    pub hash: Option<String>,
    pub length: Option<u64>,
    pub durable: bool,
}
#[derive(Clone)]
pub(crate) struct AssetSnapshot {
    pub asset: Uuid,
    pub record: Uuid,
    pub created: String,
    pub format: String,
    pub path: String,
    pub hash: String,
    pub length: u64,
}
pub(crate) enum Command {
    Metadata(Uuid),
    Create(PrincipalId, ImportRequest),
    Report(PrincipalId, ImportBatchId),
    ReportPage(PrincipalId, ImportReportPageRequest),
    Control(PrincipalId, ImportControlRequest),
    Plan(PrincipalId, ImportBatchId),
    Register(ImportBatchId, String, u32),
    Next(ImportBatchId, u32),
    Accepted(Uuid, String, u64, String),
    State(Uuid, &'static str, &'static str),
    DiscoveryDone(ImportBatchId, bool, u32),
    DiscoveryLimit(ImportBatchId),
    Duplicate(String, u64),
    Asset(Uuid),
    CurrentAsset(Uuid),
    Commit {
        batch: ImportBatchId,
        item: Item,
        target: Box<AssetSnapshot>,
        receipt: Box<ImportReceipt>,
        report: String,
        linked: bool,
        _seal: CanonicalSeal,
    },
    ResolveExport(PrincipalId, ExportRequest, String),
    ExportIntent(OperationId, ExportReceipt),
    SaveExport(OperationId, ExportReceipt),
    CheckDestination(String),
    ExportRead(PrincipalId, OperationId),
    ExportRetained(PrincipalId, OperationId),
    ExpiredExports(PrincipalId),
}
pub(crate) enum Reply {
    Metadata(RecordMetadata),
    Unit,
    Report(ImportReport),
    Plan(ImportRequest, String, String, Uuid),
    Item(Option<Item>),
    Asset(Option<AssetSnapshot>),
    Export(AssetSnapshot, Option<(ExportReceipt, bool)>, String),
    ExportReceipt(ExportReceipt),
    Retained(bool),
    Exports(Vec<ExportReceipt>),
}
fn err(_: rusqlite::Error) -> StorageError {
    StorageError::IntegrityFailed
}
fn json<T: serde::Serialize>(v: &T) -> Result<String, StorageError> {
    serde_json::to_string(v).map_err(|_| StorageError::InvalidPayload)
}
fn uuid(s: String) -> Result<Uuid, StorageError> {
    Uuid::parse_str(&s).map_err(|_| StorageError::IntegrityFailed)
}
fn owner(
    db: &Connection,
    batch: ImportBatchId,
    principal: PrincipalId,
) -> Result<(), StorageError> {
    let stored: Option<String> = db
        .query_row(
            "SELECT principal_id FROM import_batches WHERE id=?1",
            [batch.as_uuid().to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    if stored.as_deref() != Some(&principal.as_uuid().to_string()) {
        return Err(StorageError::PermissionDenied);
    }
    Ok(())
}
fn asset_row(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, String, String, u64)> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
    ))
}
fn asset(
    v: (String, String, String, String, String, String, u64),
) -> Result<AssetSnapshot, StorageError> {
    Ok(AssetSnapshot {
        asset: uuid(v.0)?,
        record: uuid(v.1)?,
        created: v.2,
        format: v.3,
        path: v.4,
        hash: v.5,
        length: v.6,
    })
}
pub(crate) fn execute(db: &mut Connection, command: Command) -> Result<Reply, StorageError> {
    match command {
        Command::Metadata(id) => {
            let kind: String = db
                .query_row(
                    "SELECT source_kind FROM records WHERE id=?1",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .map_err(|_| StorageError::AssetNotFound)?;
            let mut query = db.prepare("SELECT metadata_json FROM origin_occurrences WHERE record_id=?1 ORDER BY rowid LIMIT 16").map_err(err)?;
            let sources = query
                .query_map([id.to_string()], |r| r.get::<_, String>(0))
                .map_err(err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(err)?;
            if sources.iter().map(String::len).sum::<usize>() > 256 * 1024 {
                return Err(StorageError::InvalidPayload);
            }
            Ok(Reply::Metadata(RecordMetadata {
                record_id: sm_domain::RecordId::new(id),
                source_kind: kind,
                source_reports: sources,
            }))
        }
        Command::Create(principal, request) => {
            let id = request.batch_id.as_uuid().to_string();
            let text = json(&request)?;
            let existing: Option<(String, String)> = db
                .query_row(
                    "SELECT principal_id,request_json FROM import_batches WHERE id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(err)?;
            if let Some((p, j)) = existing {
                if p != principal.as_uuid().to_string() || j != text {
                    return Err(StorageError::PermissionDenied);
                }
            } else {
                db.execute("INSERT INTO import_batches(id,principal_id,request_json,control,created_at) VALUES (?1,?2,?3,'running',strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![id,principal.as_uuid().to_string(),text]).map_err(err)?;
            }
            report(db, principal, request.batch_id, 0).map(Reply::Report)
        }
        Command::Report(p, b) => report(db, p, b, 0).map(Reply::Report),
        Command::ReportPage(p, r) => report(db, p, r.batch_id, r.after_ordinal).map(Reply::Report),
        Command::Control(p, request) => {
            owner(db, request.batch_id, p)?;
            let control = match request.control {
                ImportControl::Pause => "paused",
                ImportControl::CancelProcessing => "cancelled",
                _ => "running",
            };
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(err)?;
            tx.execute(
                "UPDATE import_batches SET control=?2,revision=revision+1 WHERE id=?1",
                params![request.batch_id.as_uuid().to_string(), control],
            )
            .map_err(err)?;
            if request.control == ImportControl::CancelProcessing {
                tx.execute("UPDATE import_batches SET discovery_complete=1,discovery_abandoned=1 WHERE id=?1 AND discovery_complete=0",[request.batch_id.as_uuid().to_string()]).map_err(err)?;
                tx.execute("UPDATE import_items SET state='cancelled_unaccepted',error_code='CANCELLED' WHERE batch_id=?1 AND durable_input=0 AND state='discovered'",[request.batch_id.as_uuid().to_string()]).map_err(err)?;
            }
            if matches!(
                request.control,
                ImportControl::Pause | ImportControl::CancelProcessing
            ) {
                tx.execute("UPDATE import_items SET state='held' WHERE batch_id=?1 AND durable_input=1 AND state='processing'",[request.batch_id.as_uuid().to_string()]).map_err(err)?;
            }
            if matches!(
                request.control,
                ImportControl::Resume | ImportControl::Retry
            ) {
                tx.execute("UPDATE import_batches SET discovery_limit=CASE WHEN discovery_limit_hit=1 THEN MIN(discovery_limit+100000,10000000) ELSE discovery_limit END,discovery_limit_hit=0 WHERE id=?1",[request.batch_id.as_uuid().to_string()]).map_err(err)?;
                tx.execute("UPDATE import_items SET state='held',error_code=NULL WHERE batch_id=?1 AND durable_input=1 AND state IN ('held','recoverable')",[request.batch_id.as_uuid().to_string()]).map_err(err)?;
            }
            tx.commit().map_err(err)?;
            report(db, p, request.batch_id, 0).map(Reply::Report)
        }
        Command::Plan(p, b) => {
            owner(db, b, p)?;
            let (request, control, time): (String, String, String) = db
                .query_row(
                    "SELECT request_json,control,created_at FROM import_batches WHERE id=?1",
                    [b.as_uuid().to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(err)?;
            let library: String = db
                .query_row("SELECT library_id FROM library_meta", [], |r| r.get(0))
                .map_err(err)?;
            Ok(Reply::Plan(
                serde_json::from_str(&request).map_err(|_| StorageError::IntegrityFailed)?,
                control,
                time,
                uuid(library)?,
            ))
        }
        Command::Register(b, path, root) => {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(err)?;
            let inserted=tx.execute("INSERT OR IGNORE INTO import_items(id,batch_id,ordinal,source_path,state,snapshot_id,operation_id,planned_record_id,planned_asset_id) SELECT ?1,?2,(SELECT COALESCE(MAX(ordinal)+1,0) FROM import_items WHERE batch_id=?2),?3,'discovered',?4,?5,?6,?7 WHERE EXISTS(SELECT 1 FROM import_batches WHERE id=?2 AND control='running' AND discovery_abandoned=0)",params![Uuid::new_v4().to_string(),b.as_uuid().to_string(),&path,Uuid::new_v4().to_string(),Uuid::new_v4().to_string(),Uuid::new_v4().to_string(),Uuid::new_v4().to_string()]).map_err(err)?;
            let linked=tx.execute("INSERT OR IGNORE INTO import_item_roots SELECT id,?3 FROM import_items WHERE batch_id=?1 AND source_path=?2 COLLATE NOCASE",params![b.as_uuid().to_string(),path,root]).map_err(err)?;
            if inserted + linked > 0 {
                tx.execute(
                    "UPDATE import_batches SET revision=revision+1 WHERE id=?1",
                    [b.as_uuid().to_string()],
                )
                .map_err(err)?;
            }
            tx.commit().map_err(err)?;
            Ok(Reply::Unit)
        }
        Command::Next(b, after) => {
            let value = db.query_row("SELECT id,snapshot_id,operation_id,planned_record_id,planned_asset_id,source_path,source_sha256,source_length,durable_input,ordinal FROM import_items WHERE batch_id=?1 AND ordinal>=?2 AND state IN ('discovered','processing','held','recoverable') ORDER BY ordinal LIMIT 1",params![b.as_uuid().to_string(),after],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<u64>>(7)?,r.get::<_,bool>(8)?,r.get::<_,u32>(9)?))).optional().map_err(err)?;
            Ok(Reply::Item(
                value
                    .map(|v| {
                        Ok(Item {
                            id: uuid(v.0)?,
                            snapshot: uuid(v.1)?,
                            operation: uuid(v.2)?,
                            record: uuid(v.3)?,
                            asset: uuid(v.4)?,
                            path: v.5,
                            hash: v.6,
                            length: v.7,
                            durable: v.8,
                            ordinal: v.9,
                        })
                    })
                    .transpose()?,
            ))
        }
        Command::Accepted(id, hash, len, resolved) => {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(err)?;
            tx.execute("UPDATE import_items SET durable_input=1,source_sha256=?2,source_length=?3,resolved_source_path=?4,state=CASE WHEN (SELECT control FROM import_batches WHERE id=import_items.batch_id)='running' THEN 'processing' ELSE 'held' END,error_code=NULL WHERE id=?1 AND state NOT IN ('saved_new','linked_existing')",params![id.to_string(),hash,len,resolved]).map_err(err)?;
            tx.execute("UPDATE import_batches SET revision=revision+1 WHERE id=(SELECT batch_id FROM import_items WHERE id=?1)",[id.to_string()]).map_err(err)?;
            tx.commit().map_err(err)?;
            Ok(Reply::Unit)
        }
        Command::State(id, state, code) => {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(err)?;
            tx.execute("UPDATE import_batches SET revision=revision+1 WHERE id=(SELECT batch_id FROM import_items WHERE id=?1)",[id.to_string()]).map_err(err)?;
            tx.execute("UPDATE import_items SET state=?2,error_code=COALESCE(error_code,?3) WHERE id=?1 AND state NOT IN ('saved_new','linked_existing')",params![id.to_string(),state,code]).map_err(err)?;
            tx.commit().map_err(err)?;
            Ok(Reply::Unit)
        }
        Command::DiscoveryLimit(b) => {
            db.execute("UPDATE import_batches SET control='paused',discovery_limit_hit=1,revision=revision+1 WHERE id=?1",[b.as_uuid().to_string()]).map_err(err)?;
            Ok(Reply::Unit)
        }
        Command::DiscoveryDone(b, complete, issues) => {
            db.execute("UPDATE import_batches SET discovery_complete=CASE WHEN discovery_abandoned=1 OR control='cancelled' THEN 1 ELSE ?2 END,directory_issues=directory_issues+?3,revision=revision+1 WHERE id=?1",params![b.as_uuid().to_string(),complete,issues]).map_err(err)?;
            Ok(Reply::Unit)
        }
        Command::CurrentAsset(id) => {
            let a=db.query_row("SELECT id,record_id,created_at,format,path,sha256,size_bytes FROM assets WHERE record_id=?1 AND asset_type IN ('edited','original') ORDER BY CASE asset_type WHEN 'edited' THEN 0 ELSE 1 END LIMIT 1",[id.to_string()],asset_row).optional().map_err(err)?;
            Ok(Reply::Asset(a.map(asset).transpose()?))
        }
        Command::Asset(id) => {
            let a=db.query_row("SELECT id,record_id,created_at,format,path,sha256,size_bytes FROM assets WHERE id=?1",[id.to_string()],asset_row).optional().map_err(err)?;
            Ok(Reply::Asset(a.map(asset).transpose()?))
        }
        Command::Duplicate(hash, len) => {
            let v=db.query_row("SELECT a.id,a.record_id,a.created_at,a.format,a.path,a.sha256,a.size_bytes FROM origin_occurrences o JOIN assets a ON a.id=o.original_asset_id JOIN records r ON r.id=o.record_id WHERE o.source_sha256=?1 AND o.source_length=?2 AND r.source_kind='imported_image' ORDER BY r.record_pk,o.rowid LIMIT 1",params![hash,len],asset_row).optional().map_err(err)?;
            Ok(Reply::Asset(v.map(asset).transpose()?))
        }
        Command::Commit {
            batch,
            item,
            target,
            receipt,
            report,
            linked,
            _seal,
        } => {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(err)?;
            let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM import_items i JOIN import_batches b ON b.id=i.batch_id WHERE i.id=?1 AND i.batch_id=?2 AND i.operation_id=?3 AND i.snapshot_id=?4 AND i.durable_input=1 AND i.source_sha256=?5 AND i.source_length=?6 AND i.state IN ('processing','held','recoverable') AND b.control='running')",params![item.id.to_string(),batch.as_uuid().to_string(),item.operation.to_string(),item.snapshot.to_string(),item.hash,item.length],|r|r.get(0)).map_err(err)?;
            if !valid {
                return Err(StorageError::IntegrityFailed);
            }
            if linked {
                let unchanged:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1 AND record_id=?2 AND sha256=?3 AND path=?4 AND size_bytes=?5)",params![target.asset.to_string(),target.record.to_string(),target.hash,target.path,target.length],|r|r.get(0)).map_err(err)?;
                if !unchanged {
                    return Err(StorageError::IntegrityFailed);
                }
            } else {
                tx.execute("INSERT INTO records(id,library_id,created_at,capture_context,provenances,source_kind) VALUES (?1,(SELECT library_id FROM library_meta),?2,'{}','[{\"kind\":\"imported_image\"}]','imported_image')",params![target.record.to_string(),target.created]).map_err(err)?;
                tx.execute(
                    "INSERT INTO assets VALUES (?1,?2,'original',?3,?4,?5,?6,?7)",
                    params![
                        target.asset.to_string(),
                        target.record.to_string(),
                        target.format,
                        target.path,
                        target.length,
                        target.hash,
                        target.created
                    ],
                )
                .map_err(err)?;
                tx.execute(
                    "INSERT INTO asset_processing VALUES (?1,?2)",
                    params![target.asset.to_string(), report],
                )
                .map_err(err)?;
            }
            tx.execute("INSERT INTO origin_occurrences VALUES (?1,?2,?3,?4,?5,?6,?7,(SELECT resolved_source_path FROM import_items WHERE id=?2),?8,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![receipt.origin_id.as_uuid().to_string(),item.id.to_string(),target.record.to_string(),target.asset.to_string(),item.hash,item.length,item.path,report]).map_err(err)?;
            tx.execute(
                "UPDATE import_items SET state=?2,outcome_json=?3,error_code=NULL WHERE id=?1",
                params![
                    item.id.to_string(),
                    if linked {
                        "linked_existing"
                    } else {
                        "saved_new"
                    },
                    json(&receipt)?
                ],
            )
            .map_err(err)?;
            tx.execute(
                "UPDATE import_batches SET revision=revision+1 WHERE id=?1",
                [batch.as_uuid().to_string()],
            )
            .map_err(err)?;
            tx.commit().map_err(err)?;
            drop(_seal);
            Ok(Reply::Unit)
        }
        Command::ResolveExport(principal, request, resolved) => {
            let text = json(&request)?;
            let old:Option<(String,String,String,Option<String>,String,String)>=db.query_row("SELECT principal_id,request_json,source_asset_id,receipt_json,source_metadata_json,state FROM export_operations WHERE id=?1",[request.operation_id.as_uuid().to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(err)?;
            let (id, receipt, metadata) = if let Some((p, j, a, r, metadata, state)) = old {
                if p != principal.as_uuid().to_string() || j != text {
                    return Err(StorageError::PermissionDenied);
                }
                (
                    a,
                    r.map(|r| {
                        serde_json::from_str(&r)
                            .map(|receipt| (receipt, state == "saved"))
                            .map_err(|_| StorageError::IntegrityFailed)
                    })
                    .transpose()?,
                    metadata,
                )
            } else {
                let a:String=match request.asset_id {
                    Some(a)=>db.query_row("SELECT id FROM assets WHERE id=?1 AND record_id=?2",params![a.as_uuid().to_string(),request.record_id.as_uuid().to_string()],|r|r.get(0)).map_err(err)?,
                    None=>db.query_row("SELECT id FROM assets WHERE record_id=?1 ORDER BY CASE asset_type WHEN 'edited' THEN 0 WHEN 'original' THEN 1 ELSE 2 END LIMIT 1",[request.record_id.as_uuid().to_string()],|r|r.get(0)).map_err(err)?,
                };
                // Protect every historical source spelling, even when source no longer exists.
                let source:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM origin_occurrences WHERE source_path=?1 COLLATE NOCASE OR resolved_source_path=?2 COLLATE NOCASE)",params![&request.destination_path,&resolved],|r|r.get(0)).map_err(err)?;
                if source {
                    return Err(StorageError::RootRejected);
                }
                let origin:Option<(String,String)>=db.query_row("SELECT source_path,metadata_json FROM origin_occurrences WHERE record_id=?1 ORDER BY rowid LIMIT 1",[request.record_id.as_uuid().to_string()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
                let metadata = match origin {
                    Some((path, metadata)) => json(
                        &serde_json::json!({"source_path":path,"source_report":serde_json::from_str::<serde_json::Value>(&metadata).map_err(|_|StorageError::IntegrityFailed)?}),
                    )?,
                    None => "{}".into(),
                };
                db.execute(
                    "INSERT INTO export_operations VALUES (?1,?2,?3,?4,'pending',NULL,?5)",
                    params![
                        request.operation_id.as_uuid().to_string(),
                        principal.as_uuid().to_string(),
                        text,
                        a,
                        metadata
                    ],
                )
                .map_err(err)?;
                (a, None, metadata)
            };
            let a=asset(db.query_row("SELECT id,record_id,created_at,format,path,sha256,size_bytes FROM assets WHERE id=?1",[id],asset_row).map_err(err)?)?;
            Ok(Reply::Export(a, receipt, metadata))
        }
        Command::CheckDestination(path) => {
            let protected:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM origin_occurrences WHERE source_path=?1 COLLATE NOCASE OR resolved_source_path=?1 COLLATE NOCASE)",[path],|r|r.get(0)).map_err(err)?;
            if protected {
                return Err(StorageError::RootRejected);
            }
            Ok(Reply::Unit)
        }
        Command::ExportRetained(p, id) => {
            let retained:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM processing_derivative_holds h JOIN export_operations e ON e.id=h.operation_id WHERE h.principal_id=?1 AND h.operation_id=?2 AND e.principal_id=h.principal_id AND e.source_asset_id=h.asset_id)",params![p.as_uuid().to_string(),id.as_uuid().to_string()],|r|r.get(0)).map_err(err)?;
            Ok(Reply::Retained(retained))
        }
        Command::ExportRead(p, id) => {
            let json:String=db.query_row("SELECT receipt_json FROM export_operations WHERE id=?1 AND principal_id=?2 AND state='saved'",params![id.as_uuid().to_string(),p.as_uuid().to_string()],|r|r.get(0)).map_err(err)?;
            Ok(Reply::ExportReceipt(
                serde_json::from_str(&json).map_err(|_| StorageError::IntegrityFailed)?,
            ))
        }
        Command::ExpiredExports(p) => {
            let mut stmt=db.prepare("SELECT receipt_json FROM export_operations WHERE principal_id=?1 AND state='saved' AND NOT EXISTS(SELECT 1 FROM processing_derivative_holds h WHERE h.operation_id=export_operations.id) AND CAST(json_extract(receipt_json,'$.expires_at_epoch_seconds') AS INTEGER) <= CAST(strftime('%s','now') AS INTEGER) LIMIT 100").map_err(err)?;
            let rows = stmt
                .query_map([p.as_uuid().to_string()], |r| r.get::<_, String>(0))
                .map_err(err)?;
            let receipts = rows
                .map(|r| {
                    serde_json::from_str(&r.map_err(err)?)
                        .map_err(|_| StorageError::IntegrityFailed)
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            Ok(Reply::Exports(receipts))
        }
        Command::ExportIntent(id, receipt) => {
            db.execute("UPDATE export_operations SET state='pending',receipt_json=?2 WHERE id=?1 AND state!='saved'",params![id.as_uuid().to_string(),json(&receipt)?]).map_err(err)?;
            Ok(Reply::Unit)
        }
        Command::SaveExport(id, receipt) => {
            db.execute(
                "UPDATE export_operations SET state='saved',receipt_json=?2 WHERE id=?1",
                params![id.as_uuid().to_string(), json(&receipt)?],
            )
            .map_err(err)?;
            Ok(Reply::Unit)
        }
    }
}
pub(crate) fn reconcile_startup(db: &mut Connection) -> Result<(), StorageError> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    tx.execute(
        "UPDATE import_items SET state='held',error_code='INTERRUPTED' WHERE state='processing'",
        [],
    )
    .map_err(err)?;
    tx.execute("UPDATE import_batches SET control='paused',revision=revision+1 WHERE control='running' AND (discovery_complete=0 OR EXISTS(SELECT 1 FROM import_items WHERE batch_id=import_batches.id AND state IN ('discovered','held','recoverable')))",[]).map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(())
}
fn report(
    db: &Connection,
    p: PrincipalId,
    b: ImportBatchId,
    after: u32,
) -> Result<ImportReport, StorageError> {
    owner(db, b, p)?;
    let (control,complete,issues,rev,abandoned,limit,limit_hit):(String,bool,u32,u64,bool,u32,bool)=db.query_row("SELECT control,discovery_complete,directory_issues,revision,discovery_abandoned,discovery_limit,discovery_limit_hit FROM import_batches WHERE id=?1",[b.as_uuid().to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(err)?;
    let mut report = ImportReport {
        batch_id: b,
        revision: Revision::new(rev),
        control,
        discovery_complete: complete,
        discovery_abandoned: abandoned,
        discovery_limit: limit,
        discovery_limit_hit: limit_hit,
        directory_issues: issues,
        discovered: 0,
        saved_new: 0,
        linked_existing: 0,
        skipped: 0,
        failed: 0,
        held: 0,
        cancelled_unaccepted: 0,
        in_flight: 0,
        retained_input: 0,
        items: vec![],
        next_ordinal: None,
        state: ImportBatchState::Running,
        active_work: false,
    };
    let mut stmt=db.prepare("SELECT state,COUNT(*),SUM(durable_input) FROM import_items WHERE batch_id=?1 GROUP BY state").map_err(err)?;
    let groups = stmt
        .query_map([b.as_uuid().to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, u32>(2)?,
            ))
        })
        .map_err(err)?;
    for g in groups {
        let (s, n, retained) = g.map_err(err)?;
        report.discovered += n;
        match s.as_str() {
            "saved_new" => report.saved_new = n,
            "linked_existing" => report.linked_existing = n,
            "skipped" => report.skipped = n,
            "recoverable" => report.failed = n,
            "held" => report.held = n,
            "cancelled_unaccepted" => report.cancelled_unaccepted = n,
            _ => report.in_flight += n,
        };
        if s != "saved_new" && s != "linked_existing" {
            report.retained_input += retained;
        }
    }
    let mut stmt=db.prepare("SELECT id,ordinal,state,durable_input,outcome_json,error_code FROM import_items WHERE batch_id=?1 AND ordinal>=?2 ORDER BY ordinal LIMIT 101").map_err(err)?;
    let rows = stmt
        .query_map(params![b.as_uuid().to_string(), after], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(err)?;
    for r in rows {
        let (id, ordinal, state, durable, outcome, code) = r.map_err(err)?;
        if report.items.len() == 100 {
            report.next_ordinal = Some(ordinal);
            break;
        }
        let state = serde_json::from_value(serde_json::Value::String(state))
            .map_err(|_| StorageError::IntegrityFailed)?;
        report.items.push(ImportItemReport {
            item_id: ImportItemId::new(uuid(id)?),
            ordinal,
            state,
            durable_input: durable,
            receipt: outcome
                .map(|j| serde_json::from_str(&j).map_err(|_| StorageError::IntegrityFailed))
                .transpose()?,
            safe_code: code,
        });
    }
    report.state = match report.control.as_str() {
        "paused" => ImportBatchState::Paused,
        "cancelled" => ImportBatchState::Cancelled,
        _ => {
            if !report.discovery_complete || report.in_flight > 0 {
                ImportBatchState::Running
            } else if report.held > 0 || report.failed > 0 {
                ImportBatchState::NeedsAttention
            } else if report.directory_issues > 0 || report.skipped > 0 {
                ImportBatchState::CompletedWithErrors
            } else {
                ImportBatchState::Completed
            }
        }
    };
    Ok(report)
}
