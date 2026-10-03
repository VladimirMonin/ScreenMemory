//! Durable typed processing repository on the existing SQLite thread. No SDK/FS calls.
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use sm_domain::{
    AssetId, ModelProfileId, OperationId, PrincipalId, ProcessingBatchId, ProcessingJobId, Revision,
};
use sm_protocol::processing::*;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessingError {
    Invalid,
    Conflict,
    Disabled,
    Unsupported,
    NotFound,
    Uncertain,
    Integrity,
    Unavailable,
}
fn sql(_: rusqlite::Error) -> ProcessingError {
    ProcessingError::Integrity
}
fn encode<T: Serialize>(v: &T) -> Result<String, ProcessingError> {
    serde_json::to_string(v).map_err(|_| ProcessingError::Invalid)
}
fn decode<T: DeserializeOwned>(v: &str) -> Result<T, ProcessingError> {
    serde_json::from_str(v).map_err(|_| ProcessingError::Integrity)
}
fn fingerprint<T: Serialize>(v: &T) -> Result<String, ProcessingError> {
    Ok(format!("{:x}", Sha256::digest(encode(v)?.as_bytes())))
}
fn next(r: Revision) -> Result<Revision, ProcessingError> {
    r.value()
        .checked_add(1)
        .filter(|v| *v <= i64::MAX as u64)
        .map(Revision::new)
        .ok_or(ProcessingError::Integrity)
}
fn id(v: Uuid) -> Result<String, ProcessingError> {
    if v.is_nil() {
        Err(ProcessingError::Invalid)
    } else {
        Ok(v.to_string())
    }
}

pub(crate) fn policy(db: &Connection) -> Result<PolicyView, ProcessingError> {
    let (r,e,j): (u64,u64,String) = db.query_row("SELECT revision,authorization_epoch,value_json FROM processing_policy WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(sql)?;
    Ok(PolicyView {
        revision: Revision::new(r),
        authorization_epoch: Revision::new(e),
        value: decode(&j)?,
    })
}
pub(crate) fn profile(
    db: &Connection,
    key: ModelProfileId,
) -> Result<ProfileView, ProcessingError> {
    let row: Option<(u64,String,String)> = db.query_row("SELECT revision,fingerprint,value_json FROM processing_profiles WHERE id=?1 ORDER BY revision DESC LIMIT 1", [id(key.as_uuid())?], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
    let (r, f, j) = row.ok_or(ProcessingError::NotFound)?;
    let value: ModelProfile = decode(&j)?;
    if value.id != key || fingerprint(&value)? != f {
        return Err(ProcessingError::Integrity);
    }
    Ok(ProfileView {
        revision: Revision::new(r),
        fingerprint: f,
        value,
    })
}
pub(crate) fn job(
    db: &Connection,
    p: PrincipalId,
    key: ProcessingJobId,
) -> Result<JobView, ProcessingError> {
    let j: Option<String> = db.query_row("SELECT j.value_json FROM processing_jobs j JOIN processing_batches b ON b.id=j.batch_id WHERE j.id=?1 AND b.principal_id=?2", params![id(key.as_uuid())?,id(p.as_uuid())?], |r| r.get(0)).optional().map_err(sql)?;
    let value: JobView = decode(&j.ok_or(ProcessingError::NotFound)?)?;
    if value.id != key {
        return Err(ProcessingError::Integrity);
    }
    Ok(value)
}
pub(crate) fn batch(
    db: &Connection,
    p: PrincipalId,
    key: ProcessingBatchId,
) -> Result<BatchView, ProcessingError> {
    let row: Option<(u64, String)> = db
        .query_row(
            "SELECT revision,control FROM processing_batches WHERE id=?1 AND principal_id=?2",
            params![id(key.as_uuid())?, id(p.as_uuid())?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(sql)?;
    let (r, c) = row.ok_or(ProcessingError::NotFound)?;
    let mut stmt = db
        .prepare(
            "SELECT value_json FROM processing_jobs WHERE batch_id=?1 ORDER BY ordinal LIMIT 513",
        )
        .map_err(sql)?;
    let rows = stmt
        .query_map([key.as_uuid().to_string()], |r| r.get::<_, String>(0))
        .map_err(sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql)?;
    if rows.is_empty() || rows.len() > 512 {
        return Err(ProcessingError::Integrity);
    }
    Ok(BatchView {
        id: key,
        revision: Revision::new(r),
        control: decode(&format!("\"{c}\""))?,
        jobs: rows.iter().map(|j| decode(j)).collect::<Result<_, _>>()?,
    })
}
fn overview(
    db: &Connection,
    p: PrincipalId,
    before: Option<ProcessingJobId>,
) -> Result<ProcessingOverview, ProcessingError> {
    let mut stmt = db.prepare("SELECT id FROM processing_profiles p WHERE revision=(SELECT MAX(revision) FROM processing_profiles WHERE id=p.id) ORDER BY id LIMIT 32").map_err(sql)?;
    let keys = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql)?;
    let profiles = keys
        .iter()
        .map(|s| {
            profile(
                db,
                ModelProfileId::new(Uuid::parse_str(s).map_err(|_| ProcessingError::Integrity)?),
            )
        })
        .collect::<Result<_, _>>()?;
    let cursor = before.map(|v| id(v.as_uuid())).transpose()?;
    let mut stmt = db.prepare("SELECT b.id,b.revision,j.value_json FROM processing_jobs j JOIN processing_batches b ON b.id=j.batch_id WHERE b.principal_id=?1 AND (?2 IS NULL OR j.id<?2) ORDER BY j.id DESC LIMIT 33").map_err(sql)?;
    let rows = stmt
        .query_map(params![p.as_uuid().to_string(), cursor], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql)?;
    let mut jobs = rows
        .iter()
        .take(32)
        .map(|(b, r, j)| {
            Ok(QueuedJob {
                batch_id: ProcessingBatchId::new(
                    Uuid::parse_str(b).map_err(|_| ProcessingError::Integrity)?,
                ),
                batch_revision: Revision::new(*r),
                job: decode(j)?,
            })
        })
        .collect::<Result<Vec<_>, ProcessingError>>()?;
    let next_before = if rows.len() > 32 {
        jobs.last().map(|j| j.job.id)
    } else {
        None
    };
    let configured: bool = db
        .query_row(
            "SELECT runtime_enabled FROM processing_execution_host WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql)?
        .unwrap_or(false);
    Ok(ProcessingOverview {
        policy: policy(db)?,
        profiles,
        jobs: std::mem::take(&mut jobs),
        next_before,
        runtime_configured: configured,
        bridge_state: sm_protocol::application::BridgeState::Stopped,
        model_autostart: false,
        analyze_handler_available: false,
        physical_unload_supported: false,
        supported_reasoning: vec![ReasoningMode::BackendDefault],
        supported_retention: vec![HostRetention::NoHostEviction],
        native_ttl_verification: "not_verified".into(),
    })
}
pub(crate) fn save_job(db: &Connection, j: &JobView) -> Result<(), ProcessingError> {
    if db
        .execute(
            "UPDATE processing_jobs SET value_json=?2 WHERE id=?1",
            params![j.id.as_uuid().to_string(), encode(j)?],
        )
        .map_err(sql)?
        != 1
    {
        return Err(ProcessingError::Integrity);
    }
    Ok(())
}
fn validate_profile(v: &ModelProfile) -> Result<(), ProcessingError> {
    id(v.id.as_uuid())?;
    if v.model_key.is_empty()
        || v.model_key.len() > 512
        || v.model_key.chars().any(char::is_control)
        || !(1024..=262144).contains(&v.context_length)
        || !(1..=2048).contains(&v.eval_batch_size)
        || !(1..=4000).contains(&v.max_generation_tokens)
        || !v.temperature.is_finite()
        || !(0.0..=1.0).contains(&v.temperature)
        || v.native_ttl_seconds
            .is_some_and(|s| !(1..=86400).contains(&s))
    {
        return Err(ProcessingError::Invalid);
    }
    if v.reasoning != ReasoningMode::BackendDefault
        || (!v.flash_attention && v.v_cache != CacheType::F16 && v.v_cache != CacheType::F32)
    {
        return Err(ProcessingError::Unsupported);
    }
    Ok(())
}
fn operation(req: &ProcessingRequest) -> Option<OperationId> {
    match req {
        ProcessingRequest::UpdatePolicy { operation_id, .. }
        | ProcessingRequest::SaveProfile { operation_id, .. }
        | ProcessingRequest::Plan { operation_id, .. } => Some(*operation_id),
        _ => None,
    }
}

pub(crate) fn handle(
    db: &mut Connection,
    p: PrincipalId,
    request: ProcessingRequest,
) -> Result<ProcessingReply, ProcessingError> {
    id(p.as_uuid())?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql)?;
    let f = fingerprint(&request)?;
    if let Some(op) = operation(&request) {
        let prior: Option<(String,String,String)> = tx.query_row("SELECT principal_id,fingerprint,reply_json FROM processing_control_receipts WHERE operation_id=?1", [id(op.as_uuid())?], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
        if let Some((owner, fp, json)) = prior {
            return if owner == p.as_uuid().to_string() && fp == f {
                decode(&json)
            } else {
                Err(ProcessingError::Conflict)
            };
        }
    }
    let result = match &request {
        ProcessingRequest::Policy {} => ProcessingReply::Policy(policy(&tx)?),
        ProcessingRequest::Overview { before } => {
            ProcessingReply::Overview(Box::new(overview(&tx, p, *before)?))
        }
        ProcessingRequest::Inventory {} => return Err(ProcessingError::Unavailable),
        ProcessingRequest::UpdatePolicy {
            expected_revision,
            value,
            ..
        } => {
            let old = policy(&tx)?;
            if old.revision != *expected_revision {
                return Err(ProcessingError::Conflict);
            }
            // All requested physical host lifecycle modes are unsupported, including KeepLoaded.
            if value.retention != HostRetention::NoHostEviction {
                return Err(ProcessingError::Unsupported);
            }
            let r = next(old.revision)?;
            // Queue pause controls admission, not already admitted consumers. Other
            // authority changes revoke the frozen epoch and fence in-flight publication.
            let e = if old.value.mode != value.mode
                || old.value.background_loading != value.background_loading
                || old.value.query_loading != value.query_loading
                || old.value.retention != value.retention
            {
                next(old.authorization_epoch)?
            } else {
                old.authorization_epoch
            };
            tx.execute("UPDATE processing_policy SET revision=?1,authorization_epoch=?2,value_json=?3 WHERE singleton=1", params![r.value(),e.value(),encode(value)?]).map_err(sql)?;
            tx.execute("UPDATE library_settings SET value_json=?1,revision=revision+1 WHERE key='ai.enabled'", [if value.mode == ProcessingMode::Off { "false" } else { "true" }]).map_err(sql)?;
            if value.mode == ProcessingMode::Off {
                // No unbounded Rust allocation of historical jobs on Off. SQLite revokes
                // the epoch and updates nonterminal states in this same transaction.
                tx.execute("UPDATE processing_jobs SET value_json=json_set(value_json,'$.revision',CAST(CAST(json_extract(value_json,'$.revision') AS INTEGER)+1 AS TEXT),'$.reason',CASE WHEN json_extract(value_json,'$.state')='running' THEN 'cancel_requested' ELSE 'ai_disabled' END,'$.state',CASE WHEN json_extract(value_json,'$.state')='running' THEN 'running' ELSE 'blocked' END) WHERE json_extract(value_json,'$.state') NOT IN ('succeeded','cancelled','superseded')",[]).map_err(sql)?;
            }
            ProcessingReply::Policy(PolicyView {
                revision: r,
                authorization_epoch: e,
                value: value.clone(),
            })
        }
        ProcessingRequest::SaveProfile {
            expected_revision,
            value,
            ..
        } => {
            validate_profile(value)?;
            let prior: Option<u64> = tx
                .query_row(
                    "SELECT MAX(revision) FROM processing_profiles WHERE id=?1",
                    [value.id.as_uuid().to_string()],
                    |r| r.get(0),
                )
                .map_err(sql)?;
            if expected_revision.value() != prior.unwrap_or(0) {
                return Err(ProcessingError::Conflict);
            }
            let r = next(*expected_revision)?;
            let fp = fingerprint(value)?;
            tx.execute(
                "INSERT INTO processing_profiles VALUES (?1,?2,?3,?4)",
                params![
                    value.id.as_uuid().to_string(),
                    r.value(),
                    fp,
                    encode(value)?
                ],
            )
            .map_err(sql)?;
            ProcessingReply::Profile(ProfileView {
                revision: r,
                fingerprint: fp,
                value: value.clone(),
            })
        }
        ProcessingRequest::Profile { id } => ProcessingReply::Profile(profile(&tx, *id)?),
        ProcessingRequest::Plan {
            operation_id,
            expected_policy_revision,
            records,
            kind,
            profile_id,
        } => {
            let policy = policy(&tx)?;
            if policy.revision != *expected_policy_revision {
                return Err(ProcessingError::Conflict);
            }
            if policy.value.mode == ProcessingMode::Off {
                return Err(ProcessingError::Disabled);
            }
            if records.is_empty()
                || records.len() > 512
                || records
                    .iter()
                    .map(|r| r.as_uuid())
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != records.len()
            {
                return Err(ProcessingError::Invalid);
            }
            let frozen = profile_id.map(|id| profile(&tx, id)).transpose()?;
            if crate::processing_execution::technical(*kind)
                && (records.len() != 1 || frozen.is_none())
            {
                return Err(ProcessingError::Invalid);
            }
            if crate::processing_execution::technical(*kind) {
                crate::processing_execution::capacity(&tx)?;
            }
            let bid = ProcessingBatchId::new(operation_id.as_uuid());
            tx.execute("INSERT INTO processing_batches(id,principal_id,revision,control) VALUES (?1,?2,1,'resume')", params![id(bid.as_uuid())?,p.as_uuid().to_string()]).map_err(sql)?;
            for (ordinal, record) in records.iter().enumerate() {
                let asset: Option<(String, String)> = tx
                    .query_row(
                        "SELECT id,sha256 FROM assets WHERE record_id=?1 AND asset_type='original'",
                        [id(record.as_uuid())?],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(sql)?;
                let (aid, sha) = asset.ok_or(ProcessingError::NotFound)?;
                let asset_id =
                    AssetId::new(Uuid::parse_str(&aid).map_err(|_| ProcessingError::Integrity)?);
                let j = JobView {
                    id: ProcessingJobId::new(Uuid::new_v4()),
                    record_id: *record,
                    asset_id,
                    asset_sha256: sha,
                    kind: *kind,
                    revision: Revision::new(1),
                    state: JobState::Blocked,
                    reason: Some(if *kind == WorkKind::Analyze {
                        BlockReason::HandlerUnavailable
                    } else {
                        BlockReason::RuntimeUnconfigured
                    }),
                    policy_revision: policy.revision,
                    authorization_epoch: policy.authorization_epoch,
                    profile: frozen.clone(),
                    attempt_count: 0,
                    probe_result: None,
                };
                tx.execute(
                    "INSERT INTO processing_jobs(id,batch_id,ordinal,record_id,asset_id,value_json,planned_host) VALUES (?1,?2,?3,?4,?5,?6,(SELECT host_id FROM processing_execution_host WHERE singleton=1))",
                    params![
                        j.id.as_uuid().to_string(),
                        bid.as_uuid().to_string(),
                        ordinal as u32,
                        record.as_uuid().to_string(),
                        aid,
                        encode(&j)?
                    ],
                )
                .map_err(sql)?;
            }
            crate::processing_execution::activate(&tx, p, bid, false, None)?;
            ProcessingReply::Batch(batch(&tx, p, bid)?)
        }
        ProcessingRequest::Batch { id } => ProcessingReply::Batch(batch(&tx, p, *id)?),
        ProcessingRequest::Execution { id } => {
            ProcessingReply::Execution(crate::processing_execution::report(&tx, p, Some(*id))?)
        }
        ProcessingRequest::Reconcile {} => {
            ProcessingReply::Execution(crate::processing_execution::report(&tx, p, None)?)
        }
        ProcessingRequest::ControlBatch {
            id,
            expected_revision,
            control,
        } => {
            let mut b = batch(&tx, p, *id)?;
            if b.revision != *expected_revision {
                return Err(ProcessingError::Conflict);
            }
            if b.control == BatchControl::Cancel {
                return Err(ProcessingError::Conflict);
            }
            if *control == BatchControl::Resume && policy(&tx)?.value.mode == ProcessingMode::Off {
                return Err(ProcessingError::Disabled);
            }
            b.revision = next(b.revision)?;
            tx.execute(
                "UPDATE processing_batches SET revision=?2,control=?3 WHERE id=?1",
                params![
                    id.as_uuid().to_string(),
                    b.revision.value(),
                    match control {
                        BatchControl::Pause => "pause",
                        BatchControl::Resume => "resume",
                        BatchControl::Cancel => "cancel",
                    }
                ],
            )
            .map_err(sql)?;
            if *control == BatchControl::Cancel {
                for j in &mut b.jobs {
                    if !matches!(
                        j.state,
                        JobState::Succeeded | JobState::Cancelled | JobState::Superseded
                    ) {
                        cancel(&tx, j)?;
                    }
                }
            }
            if *control == BatchControl::Resume {
                crate::processing_execution::activate(&tx, p, *id, true, None)?;
            }
            ProcessingReply::Batch(batch(&tx, p, *id)?)
        }
        ProcessingRequest::ControlJob {
            id,
            expected_revision,
            control,
        } => {
            let mut j = job(&tx, p, *id)?;
            if j.revision != *expected_revision {
                return Err(ProcessingError::Conflict);
            }
            match control {
                JobControl::Cancel => cancel(&tx, &mut j)?,
                JobControl::Retry => {
                    if j.reason == Some(BlockReason::BackendUncertain) {
                        return Err(ProcessingError::Uncertain);
                    }
                    if j.state == JobState::Running || j.state == JobState::Succeeded {
                        return Err(ProcessingError::Conflict);
                    }
                    let pol = policy(&tx)?;
                    if pol.value.mode == ProcessingMode::Off {
                        return Err(ProcessingError::Disabled);
                    }
                    j.revision = next(j.revision)?;
                    j.state = JobState::Blocked;
                    j.reason = Some(if j.kind == WorkKind::Analyze {
                        BlockReason::HandlerUnavailable
                    } else {
                        BlockReason::RuntimeUnconfigured
                    });
                    j.policy_revision = pol.revision;
                    j.authorization_epoch = pol.authorization_epoch;
                    save_job(&tx, &j)?;
                    let bid: String = tx
                        .query_row(
                            "SELECT batch_id FROM processing_jobs WHERE id=?1",
                            [j.id.as_uuid().to_string()],
                            |r| r.get(0),
                        )
                        .map_err(sql)?;
                    crate::processing_execution::activate(
                        &tx,
                        p,
                        ProcessingBatchId::new(
                            Uuid::parse_str(&bid).map_err(|_| ProcessingError::Integrity)?,
                        ),
                        true,
                        Some(*id),
                    )?;
                    j = job(&tx, p, *id)?;
                }
            }
            ProcessingReply::Job(j)
        }
    };
    if let Some(op) = operation(&request) {
        tx.execute(
            "INSERT INTO processing_control_receipts VALUES (?1,?2,?3,?4)",
            params![
                op.as_uuid().to_string(),
                p.as_uuid().to_string(),
                f,
                encode(&result)?
            ],
        )
        .map_err(sql)?;
    }
    tx.commit().map_err(sql)?;
    Ok(result)
}
fn cancel(db: &Connection, j: &mut JobView) -> Result<(), ProcessingError> {
    if matches!(
        j.state,
        JobState::Succeeded | JobState::Cancelled | JobState::Superseded
    ) {
        return Err(ProcessingError::Conflict);
    }
    j.revision = next(j.revision)?;
    if j.state == JobState::Running {
        j.reason = Some(BlockReason::CancelRequested);
    } else {
        j.state = JobState::Cancelled;
        j.reason = None;
    }
    save_job(db, j)
}
