//! Native-only scheduler ledger. No serde input for commands/fences and no SDK/FS work here.
use crate::processing::{self, ProcessingError};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sm_domain::{
    AssetId, HostInstanceId, OperationId, PrincipalId, ProcessingAttemptId, ProcessingBatchId,
    ProcessingJobId, Revision, RuntimeCallId,
};
use sm_protocol::processing::*;
use uuid::Uuid;

pub const MAX_ADMITTED: u32 = 32;
#[derive(Clone)]
pub struct ExecutionFence(StoredFence);
#[derive(Clone, Serialize, Deserialize)]
struct StoredFence {
    host: HostInstanceId,
    principal: PrincipalId,
    batch: ProcessingBatchId,
    attempt: ProcessingAttemptId,
    token: Uuid,
    job: JobView,
}
impl ExecutionFence {
    pub fn host(&self) -> HostInstanceId {
        self.0.host
    }
    pub fn principal(&self) -> PrincipalId {
        self.0.principal
    }
    pub fn batch(&self) -> ProcessingBatchId {
        self.0.batch
    }
    pub fn attempt(&self) -> ProcessingAttemptId {
        self.0.attempt
    }
    pub fn job(&self) -> &JobView {
        &self.0.job
    }
}
pub enum CallPlan {
    Load {
        id: RuntimeCallId,
        generation: Uuid,
        endpoint: String,
        plan: serde_json::Value,
    },
    Inference {
        id: RuntimeCallId,
        generation: Uuid,
        endpoint: String,
        image_operation: OperationId,
    },
}
pub enum CallOutcome {
    Settled(serde_json::Value),
    Uncertain { code: &'static str },
}
pub enum AttemptOutcome {
    Completed(TechnicalProbeResult),
    Cancelled,
    Refused { code: &'static str },
    Uncertain { code: &'static str },
}
pub enum Command {
    Recover {
        host: HostInstanceId,
        runtime_enabled: bool,
    },
    Activate {
        principal: PrincipalId,
        batch: ProcessingBatchId,
        explicit_resume: bool,
    },
    Claim {
        host: HostInstanceId,
        prefer_background: bool,
    },
    Heartbeat(ExecutionFence),
    Guard(ExecutionFence),
    Hold(ExecutionFence, OperationId),
    ModelLease {
        fence: ExecutionFence,
        generation: Uuid,
        instance_reference: String,
        profile_fingerprint: String,
    },
    PlanCall(ExecutionFence, CallPlan),
    CallOutcome(ExecutionFence, RuntimeCallId, CallOutcome),
    Settle(ExecutionFence, AttemptOutcome),
    InterruptKnown(ExecutionFence),
    #[cfg(feature = "processing-test-seams")]
    SettleWithConstraintRollback(ExecutionFence, AttemptOutcome),
    #[cfg(feature = "processing-test-seams")]
    PlanCallWithConstraintRollback(ExecutionFence, CallPlan),
    #[cfg(feature = "processing-test-seams")]
    ExpireHeldDerivative(ExecutionFence),
    Holds,
}
pub enum Reply {
    Unit,
    Claimed(Option<Box<ExecutionFence>>),
    Guarded(bool),
    Settled(Box<JobView>),
    Holds(Vec<Box<ExecutionFenceHold>>),
}
pub struct ExecutionFenceHold {
    pub principal: PrincipalId,
    pub record_id: sm_domain::RecordId,
    pub asset_id: AssetId,
    pub operation_id: OperationId,
}
fn sql(_: rusqlite::Error) -> ProcessingError {
    ProcessingError::Integrity
}
fn json<T: Serialize>(v: &T) -> Result<String, ProcessingError> {
    serde_json::to_string(v).map_err(|_| ProcessingError::Invalid)
}
fn parse<T: serde::de::DeserializeOwned>(v: &str) -> Result<T, ProcessingError> {
    serde_json::from_str(v).map_err(|_| ProcessingError::Integrity)
}
fn now(db: &Connection) -> Result<i64, ProcessingError> {
    db.query_row(
        "SELECT CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)",
        [],
        |r| r.get(0),
    )
    .map_err(sql)
}
fn increment(j: &mut JobView) -> Result<(), ProcessingError> {
    j.revision = Revision::new(
        j.revision
            .value()
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or(ProcessingError::Integrity)?,
    );
    Ok(())
}
pub(crate) fn technical(kind: WorkKind) -> bool {
    matches!(
        kind,
        WorkKind::TechnicalOwnerSelfTest | WorkKind::TechnicalOwnerBackgroundSelfTest
    )
}
pub(crate) fn capacity(db: &Connection) -> Result<(), ProcessingError> {
    let count:u32=db.query_row("SELECT count(*) FROM processing_jobs WHERE json_extract(value_json,'$.state') IN ('pending','running') OR json_extract(value_json,'$.reason')='backend_uncertain'",[],|r|r.get(0)).map_err(sql)?;
    if count >= MAX_ADMITTED {
        Err(ProcessingError::Unavailable)
    } else {
        Ok(())
    }
}
fn uncertain(db: &Connection) -> Result<bool, ProcessingError> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM processing_runtime_calls WHERE state='uncertain') OR EXISTS(SELECT 1 FROM processing_attempts WHERE state='uncertain')",[],|r|r.get(0)).map_err(sql)
}
fn unresolved(db: &Connection) -> Result<bool, ProcessingError> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM processing_runtime_calls WHERE state IN ('planned','uncertain')) OR EXISTS(SELECT 1 FROM processing_attempts WHERE state='uncertain')",[],|r|r.get(0)).map_err(sql)
}
fn guard(db: &Connection, f: &ExecutionFence) -> Result<bool, ProcessingError> {
    Ok(revocation(db, f)?.is_none())
}
fn revocation(db: &Connection, f: &ExecutionFence) -> Result<Option<BlockReason>, ProcessingError> {
    let owner: Option<String> = db
        .query_row(
            "SELECT host_id FROM processing_execution_host WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql)?;
    if owner.as_deref() != Some(&f.0.host.as_uuid().to_string()) {
        return Ok(Some(BlockReason::Interrupted));
    }
    let row:Option<(String,i64,String)>=db.query_row("SELECT state,lease_expires_ms,lease_token FROM processing_attempts WHERE id=?1 AND job_id=?2 AND host_id=?3",params![f.0.attempt.as_uuid().to_string(),f.0.job.id.as_uuid().to_string(),f.0.host.as_uuid().to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
    let Some((state, expires, token)) = row else {
        return Ok(Some(BlockReason::Interrupted));
    };
    if !matches!(state.as_str(), "claimed" | "dispatched")
        || token != f.0.token.to_string()
        || expires <= now(db)?
    {
        return Ok(Some(BlockReason::LeaseExpired));
    }
    let j = processing::job(db, f.0.principal, f.0.job.id)?;
    let p = processing::policy(db)?;
    let b = processing::batch(db, f.0.principal, f.0.batch)?;
    if p.value.mode == ProcessingMode::Off || p.authorization_epoch != f.0.job.authorization_epoch {
        return Ok(Some(BlockReason::AiDisabled));
    }
    if j.state != JobState::Running
        || j.reason == Some(BlockReason::CancelRequested)
        || b.control == BatchControl::Cancel
    {
        return Ok(Some(BlockReason::CancelRequested));
    }
    let Some(profile) = f.0.job.profile.as_ref() else {
        return Ok(Some(BlockReason::ProfileRetired));
    };
    if processing::profile(db, profile.value.id)?.revision != profile.revision {
        return Ok(Some(BlockReason::ProfileRetired));
    }
    let asset: Option<(String, String)> = db
        .query_row(
            "SELECT id,sha256 FROM assets WHERE record_id=?1 AND asset_type='original'",
            [f.0.job.record_id.as_uuid().to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(sql)?;
    Ok(
        if asset
            == Some((
                f.0.job.asset_id.as_uuid().to_string(),
                f.0.job.asset_sha256.clone(),
            ))
        {
            None
        } else {
            Some(BlockReason::ProfileRetired)
        },
    )
}
fn call_identity(plan: &CallPlan) -> (RuntimeCallId, Uuid, &str, &'static str) {
    match plan {
        CallPlan::Load {
            id,
            generation,
            endpoint,
            ..
        } => (*id, *generation, endpoint, "load"),
        CallPlan::Inference {
            id,
            generation,
            endpoint,
            ..
        } => (*id, *generation, endpoint, "inference"),
    }
}
fn local_endpoint(v: &str) -> bool {
    v.strip_prefix("ws://127.0.0.1:").is_some_and(|p| {
        !p.is_empty()
            && p.len() <= 5
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u16>().is_ok_and(|n| n > 0)
    })
}

pub(crate) fn activate(
    db: &Connection,
    p: PrincipalId,
    bid: ProcessingBatchId,
    explicit_resume: bool,
    selected: Option<ProcessingJobId>,
) -> Result<(), ProcessingError> {
    let enabled:bool=db.query_row("SELECT COALESCE((SELECT runtime_enabled FROM processing_execution_host WHERE singleton=1),0)",[],|r|r.get(0)).map_err(sql)?;
    if !enabled {
        return Ok(());
    }
    let policy = processing::policy(db)?;
    if policy.value.mode == ProcessingMode::Off {
        return Err(ProcessingError::Disabled);
    }
    let b = processing::batch(db, p, bid)?;
    if b.control != BatchControl::Resume {
        return Err(ProcessingError::Conflict);
    }
    for mut j in b.jobs {
        if selected.is_some_and(|id| id != j.id)
            || !technical(j.kind)
            || !matches!(
                j.reason,
                Some(
                    BlockReason::RuntimeUnconfigured
                        | BlockReason::Interrupted
                        | BlockReason::AiDisabled
                )
            )
            || j.state != JobState::Blocked
        {
            continue;
        }
        if !explicit_resume {
            let same_host:bool=db.query_row("SELECT planned_host=(SELECT host_id FROM processing_execution_host WHERE singleton=1) FROM processing_jobs WHERE id=?1",[j.id.as_uuid().to_string()],|r|r.get::<_,Option<bool>>(0)).map_err(sql)?.unwrap_or(false);
            if !same_host
                || j.reason != Some(BlockReason::RuntimeUnconfigured)
                || j.attempt_count != 0
            {
                continue;
            }
        }
        capacity(db)?;
        let Some(profile) = j.profile.as_ref() else {
            return Err(ProcessingError::Invalid);
        };
        if uncertain(db)? {
            j.reason = Some(BlockReason::BackendUncertain);
        } else if processing::profile(db, profile.value.id)?.revision != profile.revision {
            j.reason = Some(BlockReason::ProfileRetired);
        } else {
            j.state = JobState::Pending;
            j.reason = None;
            j.authorization_epoch = policy.authorization_epoch;
            j.policy_revision = policy.revision;
        }
        increment(&mut j)?;
        processing::save_job(db, &j)?;
    }
    Ok(())
}
pub(crate) fn handle(db: &mut Connection, command: Command) -> Result<Reply, ProcessingError> {
    #[cfg(feature = "processing-test-seams")]
    let (command, rollback_fault) = match command {
        Command::SettleWithConstraintRollback(f, o) => (Command::Settle(f, o), true),
        Command::PlanCallWithConstraintRollback(f, p) => (Command::PlanCall(f, p), true),
        other => (other, false),
    };
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql)?;
    let result = match command {
        Command::Recover {
            host,
            runtime_enabled,
        } => {
            if host.as_uuid().is_nil() {
                return Err(ProcessingError::Invalid);
            }
            tx.execute("INSERT INTO processing_execution_host VALUES (1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET host_id=excluded.host_id,runtime_enabled=excluded.runtime_enabled",params![host.as_uuid().to_string(),runtime_enabled]).map_err(sql)?;
            let mut statement=tx.prepare("SELECT id,fence_json FROM processing_attempts WHERE state IN ('claimed','dispatched') LIMIT 33").map_err(sql)?;
            let rows = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)?;
            drop(statement);
            if rows.len() > MAX_ADMITTED as usize {
                return Err(ProcessingError::Integrity);
            }
            for (attempt, frozen) in rows {
                let f: StoredFence = parse(&frozen)?;
                let uncertain:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM processing_runtime_calls WHERE attempt_id=?1 AND state IN ('planned','uncertain'))",[&attempt],|r|r.get(0)).map_err(sql)?;
                tx.execute("UPDATE processing_runtime_calls SET state='uncertain' WHERE attempt_id=?1 AND state='planned'",[&attempt]).map_err(sql)?;
                tx.execute(
                    "UPDATE processing_attempts SET state=?2 WHERE id=?1",
                    params![
                        attempt,
                        if uncertain {
                            "uncertain"
                        } else {
                            "interrupted"
                        }
                    ],
                )
                .map_err(sql)?;
                let mut j = processing::job(&tx, f.principal, f.job.id)?;
                increment(&mut j)?;
                j.state = JobState::Blocked;
                j.reason = Some(if uncertain {
                    BlockReason::BackendUncertain
                } else {
                    BlockReason::Interrupted
                });
                j.probe_result = None;
                processing::save_job(&tx, &j)?;
                tx.execute("UPDATE processing_model_leases SET state=?2 WHERE attempt_id=?1 AND state='active'",params![f.attempt.as_uuid().to_string(),if uncertain{"uncertain"}else{"abandoned"}]).map_err(sql)?;
                if !uncertain {
                    tx.execute(
                        "DELETE FROM processing_derivative_holds WHERE attempt_id=?1",
                        [f.attempt.as_uuid().to_string()],
                    )
                    .map_err(sql)?;
                }
            }
            tx.execute("UPDATE processing_jobs SET value_json=json_set(value_json,'$.state','blocked','$.reason','interrupted','$.revision',CAST(CAST(json_extract(value_json,'$.revision') AS INTEGER)+1 AS TEXT)) WHERE json_extract(value_json,'$.state')='pending'",[]).map_err(sql)?;
            tx.execute("UPDATE processing_batches SET control='pause',revision=revision+1 WHERE control='resume' AND EXISTS(SELECT 1 FROM processing_jobs j WHERE j.batch_id=processing_batches.id AND json_extract(j.value_json,'$.state')='blocked' AND json_extract(j.value_json,'$.reason') IN ('interrupted','backend_uncertain'))",[]).map_err(sql)?;
            Reply::Unit
        }
        Command::Activate {
            principal: p,
            batch: bid,
            explicit_resume,
        } => {
            activate(&tx, p, bid, explicit_resume, None)?;
            Reply::Unit
        }
        Command::Claim {
            host,
            prefer_background,
        } => {
            let p = processing::policy(&tx)?;
            let configured:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM processing_execution_host WHERE singleton=1 AND host_id=?1 AND runtime_enabled=1)",[host.as_uuid().to_string()],|r|r.get(0)).map_err(sql)?;
            if !configured
                || p.value.mode == ProcessingMode::Off
                || p.value.queue_paused
                || unresolved(&tx)?
            {
                Reply::Claimed(None)
            } else {
                let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM processing_attempts WHERE state IN ('claimed','dispatched','uncertain'))",[],|r|r.get(0)).map_err(sql)?;
                if active {
                    Reply::Claimed(None)
                } else {
                    let priority = if prefer_background {
                        "technical_owner_background_self_test"
                    } else {
                        "technical_owner_self_test"
                    };
                    let row:Option<(String,String,String)>=tx.query_row("SELECT j.value_json,b.id,b.principal_id FROM processing_jobs j JOIN processing_batches b ON b.id=j.batch_id WHERE b.control='resume' AND json_extract(j.value_json,'$.state')='pending' ORDER BY (json_extract(j.value_json,'$.kind')=?1) DESC,j.rowid LIMIT 1",[priority],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
                    if let Some((value, bid, principal)) = row {
                        let mut j: JobView = parse(&value)?;
                        if !technical(j.kind) {
                            return Err(ProcessingError::Invalid);
                        }
                        increment(&mut j)?;
                        j.state = JobState::Running;
                        j.reason = None;
                        j.attempt_count = j
                            .attempt_count
                            .checked_add(1)
                            .ok_or(ProcessingError::Integrity)?;
                        processing::save_job(&tx, &j)?;
                        let f = ExecutionFence(StoredFence {
                            host,
                            principal: PrincipalId::new(
                                Uuid::parse_str(&principal)
                                    .map_err(|_| ProcessingError::Integrity)?,
                            ),
                            batch: ProcessingBatchId::new(
                                Uuid::parse_str(&bid).map_err(|_| ProcessingError::Integrity)?,
                            ),
                            attempt: ProcessingAttemptId::new(Uuid::new_v4()),
                            token: Uuid::new_v4(),
                            job: j,
                        });
                        tx.execute("INSERT INTO processing_attempts(id,job_id,host_id,lease_token,lease_expires_ms,authorization_epoch,state,fence_json) VALUES (?1,?2,?3,?4,?5,?6,'claimed',?7)",params![f.0.attempt.as_uuid().to_string(),f.0.job.id.as_uuid().to_string(),host.as_uuid().to_string(),f.0.token.to_string(),now(&tx)?+60000,f.0.job.authorization_epoch.value(),json(&f.0)?]).map_err(sql)?;
                        Reply::Claimed(Some(Box::new(f)))
                    } else {
                        Reply::Claimed(None)
                    }
                }
            }
        }
        Command::Guard(f) => Reply::Guarded(guard(&tx, &f)?),
        Command::Heartbeat(f) => {
            let valid = guard(&tx, &f)?;
            if valid {
                tx.execute(
                    "UPDATE processing_attempts SET lease_expires_ms=?2 WHERE id=?1",
                    params![f.0.attempt.as_uuid().to_string(), now(&tx)? + 60000],
                )
                .map_err(sql)?;
            }
            Reply::Guarded(valid)
        }
        Command::ModelLease {
            fence: f,
            generation,
            instance_reference,
            profile_fingerprint,
        } => {
            if !guard(&tx, &f)? {
                return Err(ProcessingError::Conflict);
            }
            if generation.is_nil()
                || instance_reference.is_empty()
                || instance_reference.len() > 512
                || f.0
                    .job
                    .profile
                    .as_ref()
                    .is_none_or(|p| p.fingerprint != profile_fingerprint)
            {
                return Err(ProcessingError::Invalid);
            }
            tx.execute(
                "INSERT INTO processing_model_leases VALUES (?1,?2,?3,?4,?5,'active')",
                params![
                    Uuid::new_v4().to_string(),
                    f.0.attempt.as_uuid().to_string(),
                    generation.to_string(),
                    instance_reference,
                    profile_fingerprint
                ],
            )
            .map_err(sql)?;
            Reply::Unit
        }
        Command::Hold(f, op) => {
            if !guard(&tx, &f)? {
                return Err(ProcessingError::Conflict);
            }
            let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM export_operations WHERE id=?1 AND principal_id=?2 AND source_asset_id=?3 AND state='saved')",params![op.as_uuid().to_string(),f.0.principal.as_uuid().to_string(),f.0.job.asset_id.as_uuid().to_string()],|r|r.get(0)).map_err(sql)?;
            if !valid {
                return Err(ProcessingError::Integrity);
            }
            tx.execute(
                "INSERT INTO processing_derivative_holds VALUES (?1,?2,?3,?4,?5)",
                params![
                    f.0.attempt.as_uuid().to_string(),
                    op.as_uuid().to_string(),
                    f.0.job.record_id.as_uuid().to_string(),
                    f.0.job.asset_id.as_uuid().to_string(),
                    f.0.principal.as_uuid().to_string()
                ],
            )
            .map_err(sql)?;
            Reply::Unit
        }
        Command::PlanCall(f, plan) => {
            if !guard(&tx, &f)? {
                return Err(ProcessingError::Conflict);
            }
            if unresolved(&tx)? {
                return Err(ProcessingError::Uncertain);
            }
            let (id, generation, endpoint, kind) = call_identity(&plan);
            if id.as_uuid().is_nil() || generation.is_nil() || !local_endpoint(endpoint) {
                return Err(ProcessingError::Invalid);
            }
            let value = match &plan {
                CallPlan::Load { plan, .. } => {
                    if plan["intent_id"] != id.as_uuid().to_string()
                        || plan["identifier"] != format!("sm-{}", id.as_uuid())
                        || plan["model_key"]
                            != f.0
                                .job
                                .profile
                                .as_ref()
                                .ok_or(ProcessingError::Invalid)?
                                .value
                                .model_key
                    {
                        return Err(ProcessingError::Invalid);
                    }
                    plan.clone()
                }
                CallPlan::Inference {
                    image_operation, ..
                } => {
                    let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM processing_derivative_holds WHERE attempt_id=?1 AND operation_id=?2 AND asset_id=?3)",params![f.0.attempt.as_uuid().to_string(),image_operation.as_uuid().to_string(),f.0.job.asset_id.as_uuid().to_string()],|r|r.get(0)).map_err(sql)?;
                    if !valid {
                        return Err(ProcessingError::Integrity);
                    }
                    serde_json::json!({"image_operation_id":image_operation,"asset_id":f.0.job.asset_id,"asset_sha256":f.0.job.asset_sha256,"profile":f.0.job.profile,"authorization_epoch":f.0.job.authorization_epoch})
                }
            };
            let encoded = json(&value)?;
            if encoded.len() > 16384 {
                return Err(ProcessingError::Invalid);
            }
            tx.execute(
                "INSERT INTO processing_runtime_calls VALUES (?1,?2,?3,?4,?5,?6,'planned',NULL)",
                params![
                    id.as_uuid().to_string(),
                    f.0.attempt.as_uuid().to_string(),
                    kind,
                    generation.to_string(),
                    endpoint,
                    encoded
                ],
            )
            .map_err(sql)?;
            tx.execute(
                "UPDATE processing_attempts SET state='dispatched' WHERE id=?1",
                [f.0.attempt.as_uuid().to_string()],
            )
            .map_err(sql)?;
            Reply::Unit
        }
        Command::CallOutcome(f, id, outcome) => {
            // Technical settlement is accepted even after policy revocation; it cannot publish.
            let (state, value) = match outcome {
                CallOutcome::Settled(v) => ("settled", v),
                CallOutcome::Uncertain { code } => (
                    "uncertain",
                    serde_json::json!({"code":code,"settlement":"uncertain"}),
                ),
            };
            let encoded = json(&value)?;
            if encoded.len() > 32768 {
                return Err(ProcessingError::Invalid);
            }
            if tx.execute("UPDATE processing_runtime_calls SET state=?3,outcome_json=?4 WHERE id=?1 AND attempt_id=?2 AND state='planned'",params![id.as_uuid().to_string(),f.0.attempt.as_uuid().to_string(),state,encoded]).map_err(sql)?!=1{return Err(ProcessingError::Conflict);}
            Reply::Unit
        }
        Command::InterruptKnown(f) => {
            let safe:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM processing_attempts a JOIN processing_execution_host h ON h.host_id=a.host_id WHERE a.id=?1 AND a.lease_token=?2 AND a.state IN ('claimed','dispatched')) AND NOT EXISTS(SELECT 1 FROM processing_runtime_calls WHERE attempt_id=?1 AND state IN ('planned','uncertain'))",params![f.0.attempt.as_uuid().to_string(),f.0.token.to_string()],|r|r.get(0)).map_err(sql)?;
            if !safe {
                return Err(ProcessingError::Uncertain);
            }
            let mut j = processing::job(&tx, f.0.principal, f.0.job.id)?;
            increment(&mut j)?;
            j.state = JobState::Blocked;
            j.reason = Some(BlockReason::Interrupted);
            j.probe_result = None;
            processing::save_job(&tx, &j)?;
            tx.execute(
                "UPDATE processing_attempts SET state='interrupted',outcome_json=?2 WHERE id=?1",
                params![
                    f.0.attempt.as_uuid().to_string(),
                    json(&serde_json::json!({"code":"PUBLICATION_NOT_COMMITTED"}))?
                ],
            )
            .map_err(sql)?;
            tx.execute("UPDATE processing_model_leases SET state='abandoned' WHERE attempt_id=?1 AND state='active'",[f.0.attempt.as_uuid().to_string()]).map_err(sql)?;
            tx.execute(
                "DELETE FROM processing_derivative_holds WHERE attempt_id=?1",
                [f.0.attempt.as_uuid().to_string()],
            )
            .map_err(sql)?;
            tx.execute(
                "UPDATE processing_batches SET control='pause',revision=revision+1 WHERE id=?1",
                [f.0.batch.as_uuid().to_string()],
            )
            .map_err(sql)?;
            tx.execute(
                "UPDATE processing_execution_host SET runtime_enabled=0 WHERE host_id=?1",
                [f.0.host.as_uuid().to_string()],
            )
            .map_err(sql)?;
            Reply::Unit
        }
        Command::Settle(f, outcome) => {
            let row: Option<(String, String)> = tx
                .query_row(
                    "SELECT state,lease_token FROM processing_attempts WHERE id=?1 AND host_id=?2",
                    params![
                        f.0.attempt.as_uuid().to_string(),
                        f.0.host.as_uuid().to_string()
                    ],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(sql)?;
            let (state, token) = row.ok_or(ProcessingError::Conflict)?;
            if token != f.0.token.to_string() {
                return Err(ProcessingError::Conflict);
            }
            if state == "settled" {
                Reply::Settled(Box::new(processing::job(&tx, f.0.principal, f.0.job.id)?))
            } else {
                if !matches!(state.as_str(), "claimed" | "dispatched") {
                    return Err(ProcessingError::Conflict);
                }
                let mut j = processing::job(&tx, f.0.principal, f.0.job.id)?;
                let revoked = revocation(&tx, &f)?;
                let valid = revoked.is_none();
                let calls_uncertain:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM processing_runtime_calls WHERE attempt_id=?1 AND state IN ('planned','uncertain'))",[f.0.attempt.as_uuid().to_string()],|r|r.get(0)).map_err(sql)?;
                let outcome_code = match &outcome {
                    AttemptOutcome::Uncertain { code } | AttemptOutcome::Refused { code } => {
                        Some(*code)
                    }
                    _ => None,
                };
                let (attempt_state, receipt) = if calls_uncertain
                    || matches!(outcome, AttemptOutcome::Uncertain { .. })
                {
                    j.state = JobState::Blocked;
                    j.reason = Some(BlockReason::BackendUncertain);
                    j.probe_result = None;
                    tx.execute("UPDATE processing_runtime_calls SET state='uncertain' WHERE attempt_id=?1 AND state='planned'",[f.0.attempt.as_uuid().to_string()]).map_err(sql)?;
                    (
                        "uncertain",
                        serde_json::json!({"settlement":"uncertain","code":outcome_code}),
                    )
                } else {
                    match outcome {
                        AttemptOutcome::Completed(result) if valid => {
                            j.state = JobState::Succeeded;
                            j.reason = None;
                            j.probe_result = Some(result);
                        }
                        AttemptOutcome::Completed(_) | AttemptOutcome::Cancelled => {
                            j.state = match revoked {
                                Some(BlockReason::ProfileRetired) => JobState::Superseded,
                                Some(
                                    BlockReason::AiDisabled
                                    | BlockReason::LeaseExpired
                                    | BlockReason::Interrupted,
                                ) => JobState::Blocked,
                                _ => JobState::Cancelled,
                            };
                            j.reason = if j.state == JobState::Cancelled {
                                None
                            } else {
                                revoked
                            };
                            j.probe_result = None;
                        }
                        AttemptOutcome::Refused { .. } => {
                            j.state = JobState::Blocked;
                            j.reason = Some(revoked.unwrap_or(BlockReason::RuntimeRefused));
                            j.probe_result = None;
                        }
                        AttemptOutcome::Uncertain { .. } => unreachable!(),
                    }
                    tx.execute(
                        "DELETE FROM processing_derivative_holds WHERE attempt_id=?1",
                        [f.0.attempt.as_uuid().to_string()],
                    )
                    .map_err(sql)?;
                    (
                        "settled",
                        serde_json::json!({"state":j.state,"reason":j.reason,"code":outcome_code}),
                    )
                };
                tx.execute("UPDATE processing_model_leases SET state=?2 WHERE attempt_id=?1 AND state='active'",params![f.0.attempt.as_uuid().to_string(),if attempt_state=="uncertain"{"uncertain"}else{"released"}]).map_err(sql)?;
                increment(&mut j)?;
                processing::save_job(&tx, &j)?;
                tx.execute(
                    "UPDATE processing_attempts SET state=?2,outcome_json=?3 WHERE id=?1",
                    params![
                        f.0.attempt.as_uuid().to_string(),
                        attempt_state,
                        json(&receipt)?
                    ],
                )
                .map_err(sql)?;
                Reply::Settled(Box::new(j))
            }
        }
        #[cfg(feature = "processing-test-seams")]
        Command::SettleWithConstraintRollback(..) | Command::PlanCallWithConstraintRollback(..) => {
            unreachable!("normalized native test seam")
        }
        #[cfg(feature = "processing-test-seams")]
        Command::ExpireHeldDerivative(f) => {
            tx.execute("UPDATE export_operations SET receipt_json=json_set(receipt_json,'$.expires_at_epoch_seconds',CAST(unixepoch()-1 AS TEXT)) WHERE id IN (SELECT operation_id FROM processing_derivative_holds WHERE attempt_id=?1)",[f.0.attempt.as_uuid().to_string()]).map_err(sql)?;
            Reply::Unit
        }
        Command::Holds => {
            let mut stmt=tx.prepare("SELECT principal_id,record_id,asset_id,operation_id FROM processing_derivative_holds LIMIT 33").map_err(sql)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)?;
            if rows.len() > MAX_ADMITTED as usize {
                return Err(ProcessingError::Integrity);
            }
            let mut holds = Vec::new();
            for (p, r, a, o) in rows {
                holds.push(Box::new(ExecutionFenceHold {
                    principal: PrincipalId::new(
                        Uuid::parse_str(&p).map_err(|_| ProcessingError::Integrity)?,
                    ),
                    record_id: sm_domain::RecordId::new(
                        Uuid::parse_str(&r).map_err(|_| ProcessingError::Integrity)?,
                    ),
                    asset_id: AssetId::new(
                        Uuid::parse_str(&a).map_err(|_| ProcessingError::Integrity)?,
                    ),
                    operation_id: OperationId::new(
                        Uuid::parse_str(&o).map_err(|_| ProcessingError::Integrity)?,
                    ),
                }));
            }
            Reply::Holds(holds)
        }
    };
    #[cfg(feature = "processing-test-seams")]
    if rollback_fault {
        // A real SQLite CHECK failure after guarded publication writes, before commit.
        // The command/fence cannot be deserialized or admitted by application RPC.
        tx.execute(
            "INSERT INTO processing_execution_host VALUES (2,'invalid',0)",
            [],
        )
        .map_err(sql)?;
    }
    tx.commit().map_err(sql)?;
    Ok(result)
}

fn load_config_view(v: &serde_json::Value) -> Option<LoadConfigurationView> {
    v.as_object()?;
    let number = |key: &str| v[key].as_u64().and_then(|n| u32::try_from(n).ok());
    let cache = |key: &str| {
        v[key]
            .as_str()
            .filter(|s| {
                matches!(
                    *s,
                    "f16" | "f32" | "q8_0" | "q4_0" | "q4_1" | "q5_0" | "q5_1" | "iq4_nl"
                )
            })
            .map(str::to_owned)
    };
    Some(LoadConfigurationView {
        context_length: number("contextLength"),
        eval_batch_size: number("evalBatchSize"),
        flash_attention: v["flashAttention"].as_bool(),
        offload_kv_cache_to_gpu: v["offloadKVCacheToGpu"].as_bool(),
        k_cache_type: cache("llamaKCacheQuantizationType"),
        v_cache_type: cache("llamaVCacheQuantizationType"),
        max_parallel_predictions: number("maxParallelPredictions"),
    })
}
pub(crate) fn report(
    db: &Connection,
    p: PrincipalId,
    id: Option<ProcessingJobId>,
) -> Result<ExecutionReport, ProcessingError> {
    if let Some(id) = id {
        processing::job(db, p, id)?;
    }
    let mut stmt=db.prepare("SELECT a.id,a.job_id,a.state,a.host_id,a.authorization_epoch,a.outcome_json FROM processing_attempts a JOIN processing_jobs j ON j.id=a.job_id JOIN processing_batches b ON b.id=j.batch_id WHERE b.principal_id=?1 AND (?2 IS NULL OR a.job_id=?2) ORDER BY a.rowid DESC LIMIT 128").map_err(sql)?;
    let rows = stmt
        .query_map(
            params![p.as_uuid().to_string(), id.map(|v| v.as_uuid().to_string())],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, u64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .map_err(sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql)?;
    let mut attempts = Vec::new();
    let mut calls = Vec::new();
    let mut held = Vec::new();
    let mut leases = Vec::new();
    for (a, j, s, h, e, o) in rows {
        let outcome: serde_json::Value = o
            .map(|s| parse(&s))
            .transpose()?
            .unwrap_or(serde_json::Value::Null);
        let aid =
            ProcessingAttemptId::new(Uuid::parse_str(&a).map_err(|_| ProcessingError::Integrity)?);
        attempts.push(AttemptView {
            id: aid,
            job_id: ProcessingJobId::new(
                Uuid::parse_str(&j).map_err(|_| ProcessingError::Integrity)?,
            ),
            state: parse(&format!("\"{s}\""))?,
            host_id: HostInstanceId::new(
                Uuid::parse_str(&h).map_err(|_| ProcessingError::Integrity)?,
            ),
            authorization_epoch: Revision::new(e),
            outcome_code: outcome["code"].as_str().map(str::to_owned),
        });
        let mut st=db.prepare("SELECT id,state,kind,generation,outcome_json,plan_json FROM processing_runtime_calls WHERE attempt_id=?1 ORDER BY rowid LIMIT 2").map_err(sql)?;
        let rs = st
            .query_map([&a], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        for (c, s, k, g, o, plan) in rs {
            let requested: serde_json::Value = parse(&plan)?;
            let v: serde_json::Value = o
                .map(|s| parse(&s))
                .transpose()?
                .unwrap_or(serde_json::Value::Null);
            calls.push(RuntimeCallView {
                load_requested: if k == "load" {
                    load_config_view(&v["requested"])
                } else {
                    None
                },
                load_reported_applied: if k == "load" {
                    load_config_view(&v["applied"])
                } else {
                    None
                },
                load_configuration: (k == "load").then(|| match v["configuration"].as_str() {
                    Some("reported_applied") => LoadConfigurationStatus::ReportedApplied,
                    Some("mismatch") => LoadConfigurationStatus::Mismatch,
                    _ => LoadConfigurationStatus::Unknown,
                }),
                id: RuntimeCallId::new(
                    Uuid::parse_str(&c).map_err(|_| ProcessingError::Integrity)?,
                ),
                attempt_id: aid,
                state: parse(&format!("\"{s}\""))?,
                kind: parse(&format!("\"{k}\""))?,
                generation: g,
                outcome_code: v["code"].as_str().map(str::to_owned),
                native_ttl_requested_seconds: v["native_ttl"]["requested_seconds"]
                    .as_u64()
                    .or_else(|| requested["ttl_seconds"].as_u64())
                    .and_then(|v| v.try_into().ok()),
                native_ttl_verification: v["native_ttl"]["verification"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| (k == "load").then(|| "not_verified".to_owned())),
            });
        }
        let mut lease=db.prepare("SELECT id,state,generation,profile_fingerprint FROM processing_model_leases WHERE attempt_id=?1").map_err(sql)?;
        let ls = lease
            .query_map([&a], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        for (id, state, generation, profile_fingerprint) in ls {
            leases.push(ModelConsumerLeaseView {
                id: sm_domain::ModelConsumerLeaseId::new(
                    Uuid::parse_str(&id).map_err(|_| ProcessingError::Integrity)?,
                ),
                attempt_id: aid,
                state: parse(&format!("\"{state}\""))?,
                generation,
                profile_fingerprint,
            });
        }
        let hold: Option<(String, String)> = db
            .query_row(
                "SELECT operation_id,asset_id FROM processing_derivative_holds WHERE attempt_id=?1",
                [&a],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(sql)?;
        if let Some((o, asset)) = hold {
            held.push(DerivativeHoldView {
                attempt_id: aid,
                operation_id: OperationId::new(
                    Uuid::parse_str(&o).map_err(|_| ProcessingError::Integrity)?,
                ),
                asset_id: AssetId::new(
                    Uuid::parse_str(&asset).map_err(|_| ProcessingError::Integrity)?,
                ),
            });
        }
    }
    Ok(ExecutionReport {
        reconciliation_required: uncertain(db)?,
        attempts,
        calls,
        retained_derivatives: held,
        model_leases: leases,
    })
}
