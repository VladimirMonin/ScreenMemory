//! Agent authority is owned by the existing SQLite worker, never a second database.
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use sm_application::{
    access::{AgentAuthority, AgentBinding, DeliveryPermit},
    credentials::Secret,
    RequestContext, Scope,
};
use sm_domain::{CorrelationId, LibraryId, PrincipalId, RequestId};
use sm_protocol::{
    access::*,
    errors::{AppError, ErrorCode},
    knowledge::{KnowledgeId, SqlRevision},
    values::SafeMessage,
};
use std::time::Duration;
use uuid::Uuid;

pub fn error(code: ErrorCode) -> AppError {
    AppError {
        code,
        safe_message: SafeMessage::parse("Agent access rejected.").expect("constant"),
        retryable: false,
    }
}
fn sql(_: rusqlite::Error) -> AppError {
    error(ErrorCode::AssetIntegrityFailed)
}
fn encode<T: Serialize>(v: &T) -> Result<String, AppError> {
    serde_json::to_string(v).map_err(|_| error(ErrorCode::InvalidRequest))
}
fn decode<T: DeserializeOwned>(v: &str) -> Result<T, AppError> {
    serde_json::from_str(v).map_err(|_| error(ErrorCode::AssetIntegrityFailed))
}
fn next(n: u64) -> Result<u64, AppError> {
    n.checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| sql(rusqlite::Error::InvalidQuery))
}
fn state<T: Serialize>(s: &T) -> Result<String, AppError> {
    Ok(encode(s)?.trim_matches('"').to_owned())
}
#[derive(Clone)]
pub struct EnrollmentPlan {
    pub connection: AgentConnectionView,
    pub expected_digest: [u8; 32],
}
/// Native-only enrollment phases. Wire cannot supply digests or activate a connection.
pub enum Command {
    Manage(AccessRequest),
    EnrollmentLookup(AccessRequest),
    CleanupTargets(AccessRequest),
    Plan(AccessRequest, [u8; 32]),
    Activate(AccessRequest, [u8; 32]),
    Authenticate {
        library: LibraryId,
        connection: AgentConnectionId,
        secret: Secret,
        request: RequestId,
        correlation: CorrelationId,
    },
    Validate(RequestContext),
}
pub enum Reply {
    Public(AccessReply),
    Plan(Option<EnrollmentPlan>),
    Cleanup(Vec<AgentConnectionId>),
    Context(RequestContext),
    Validated,
}
impl Command {
    pub(crate) fn mutates(&self) -> bool {
        matches!(self, Self::Manage(r) if !matches!(r, AccessRequest::List {}))
            || matches!(self, Self::Plan(..) | Self::Activate(..))
    }
}
fn profile(db: &Connection, id: KnowledgeId) -> Result<AgentProfileView, AppError> {
    db.query_row("SELECT p.principal_id,p.revision,p.security_revision,v.name,p.state FROM agent_profiles p JOIN agent_profile_versions v ON v.profile_id=p.id AND v.revision=p.revision WHERE p.id=?1",[id.0.to_string()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?,r.get::<_,u64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).optional().map_err(sql)?.ok_or_else(||error(ErrorCode::PermissionDenied)).and_then(|(principal,revision,security,name,s)|Ok(AgentProfileView { profile_id:id, principal_id:decode(&encode(&principal)?)?, revision:SqlRevision(revision), security_revision:SqlRevision(security),name:decode(&encode(&name)?)?,state:decode(&encode(&s)?)?,policy:AgentPolicy::default() }))
}
fn connection(db: &Connection, id: AgentConnectionId) -> Result<AgentConnectionView, AppError> {
    db.query_row("SELECT profile_id,revision,display_name,state,credential_generation FROM agent_connections WHERE id=?1",[id.0.0.to_string()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,u64>(4)?))).optional().map_err(sql)?.ok_or_else(||error(ErrorCode::PermissionDenied)).and_then(|(p,revision,name,s,generation)|{
        let p=profile(db,decode(&encode(&p)?)?)?;
        let s:AgentConnectionState=decode(&encode(&s)?)?;
        Ok(AgentConnectionView { connection_id:id, profile_id:p.profile_id,principal_id:p.principal_id,revision:SqlRevision(revision),display_name:decode(&encode(&name)?)?,state:s,credential_generation:SqlRevision(generation),credential_ready:s==AgentConnectionState::Active })
    })
}
fn digest(db: &Connection, id: AgentConnectionId) -> Result<[u8; 32], AppError> {
    let bytes: Vec<u8> = db
        .query_row(
            "SELECT expected_digest FROM agent_connections WHERE id=?1",
            [id.0 .0.to_string()],
            |r| r.get(0),
        )
        .map_err(sql)?;
    bytes
        .try_into()
        .map_err(|_| error(ErrorCode::AssetIntegrityFailed))
}
fn fingerprint(req: &AccessRequest) -> Result<String, AppError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("agent-access-v1:{}", encode(req)?))
    ))
}
fn receipt(
    db: &Connection,
    ctx: &RequestContext,
    req: &AccessRequest,
) -> Result<Option<AccessReply>, AppError> {
    let Some(op) = req.operation_id() else {
        return Ok(None);
    };
    let row:Option<(String,String,String)>=db.query_row("SELECT owner_principal_id,request_fingerprint,result_json FROM access_receipts WHERE operation_id=?1",[op.0.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
    match row {
        Some((owner, f, j))
            if owner == ctx.principal_id().as_uuid().to_string() && f == fingerprint(req)? =>
        {
            decode(&j).map(Some)
        }
        Some(_) => Err(error(ErrorCode::IdempotencyConflict)),
        None => Ok(None),
    }
}
fn save(
    db: &Connection,
    ctx: &RequestContext,
    req: &AccessRequest,
    reply: &AccessReply,
) -> Result<(), AppError> {
    db.execute(
        "INSERT INTO access_receipts VALUES(?1,?2,?3,?4,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        params![
            req.operation_id()
                .ok_or_else(|| error(ErrorCode::InvalidRequest))?
                .0
                .to_string(),
            ctx.principal_id().as_uuid().to_string(),
            fingerprint(req)?,
            encode(reply)?
        ],
    )
    .map_err(sql)?;
    Ok(())
}
fn event(
    db: &Connection,
    ctx: &RequestContext,
    req: &AccessRequest,
    p: &AgentProfileView,
    c: Option<AgentConnectionId>,
    kind: &str,
) -> Result<(), AppError> {
    db.execute("INSERT INTO access_events(operation_id,owner_principal_id,profile_id,connection_id,event_kind,security_revision,occurred_at_utc) VALUES(?1,?2,?3,?4,?5,?6,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![req.operation_id().ok_or_else(||error(ErrorCode::InvalidRequest))?.0.to_string(),ctx.principal_id().as_uuid().to_string(),p.profile_id.0.to_string(),c.map(|c|c.0.0.to_string()),kind,p.security_revision.0]).map_err(sql)?;
    Ok(())
}
fn enrollment(
    db: &Connection,
    ctx: &RequestContext,
    req: &AccessRequest,
) -> Result<Option<EnrollmentPlan>, AppError> {
    let AccessRequest::IssueConnection { operation_id, .. } = req else {
        return Err(error(ErrorCode::InvalidRequest));
    };
    let row:Option<(String,String,String)>=db.query_row("SELECT owner_principal_id,request_fingerprint,connection_id FROM agent_enrollments WHERE operation_id=?1",[operation_id.0.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
    if let Some((owner, f, id)) = row {
        if owner != ctx.principal_id().as_uuid().to_string() || f != fingerprint(req)? {
            return Err(error(ErrorCode::IdempotencyConflict));
        }
        let id: AgentConnectionId = decode(&encode(&id)?)?;
        return Ok(Some(EnrollmentPlan {
            connection: connection(db, id)?,
            expected_digest: digest(db, id)?,
        }));
    }
    if receipt(db, ctx, req)?.is_some() {
        return Err(error(ErrorCode::IdempotencyConflict));
    }
    Ok(None)
}
pub(crate) fn validate(db: &Connection, ctx: &RequestContext) -> Result<(), AppError> {
    let Some(b) = ctx.agent_binding() else {
        return Ok(());
    };
    let c = connection(db, b.connection_id)?;
    let p = profile(db, c.profile_id)?;
    if c.state != AgentConnectionState::Active
        || p.state != AgentProfileState::Active
        || ctx.principal_id().as_uuid() != p.principal_id.0
        || b.security_revision != p.security_revision
        || b.credential_generation != c.credential_generation
    {
        return Err(error(ErrorCode::PermissionDenied));
    }
    Ok(())
}
pub(crate) fn handle(
    db: &mut Connection,
    ctx: RequestContext,
    command: Command,
) -> Result<Reply, AppError> {
    match command {
        Command::Validate(c) => {
            validate(db, &c)?;
            return Ok(Reply::Validated);
        }
        Command::Authenticate {
            library,
            connection: id,
            secret,
            request,
            correlation,
        } => {
            let current: String = db
                .query_row(
                    "SELECT library_id FROM library_meta WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )
                .map_err(sql)?;
            if current != library.as_uuid().to_string() {
                return Err(error(ErrorCode::LibraryMismatch));
            }
            let c = connection(db, id)?;
            let p = profile(db, c.profile_id)?;
            if c.state != AgentConnectionState::Active
                || p.state != AgentProfileState::Active
                || !secret.matches_session_digest(&digest(db, id)?)
            {
                return Err(error(ErrorCode::PermissionDenied));
            }
            return Ok(Reply::Context(
                RequestContext::from_authenticated_peer(
                    request,
                    correlation,
                    PrincipalId::new(p.principal_id.0),
                    vec![Scope::ReadMetadata],
                )
                .bind_agent(AgentBinding {
                    connection_id: id,
                    security_revision: p.security_revision,
                    credential_generation: c.credential_generation,
                }),
            ));
        }
        _ => {}
    }
    if !ctx.allows(Scope::ManageAccess) || ctx.agent_binding().is_some() {
        return Err(error(ErrorCode::PermissionDenied));
    }
    let (req, phase, expected) = match command {
        Command::Manage(r) => (r, 0, None),
        Command::EnrollmentLookup(r) => (r, 1, None),
        Command::CleanupTargets(r) => (r, 4, None),
        Command::Plan(r, d) => (r, 2, Some(d)),
        Command::Activate(r, d) => (r, 3, Some(d)),
        _ => unreachable!(),
    };
    // Revalidate even native callers constructing public DTO fields.
    let req: AccessRequest =
        serde_json::from_str(&encode(&req)?).map_err(|_| error(ErrorCode::InvalidRequest))?;
    if phase == 1 {
        return enrollment(db, &ctx, &req).map(Reply::Plan);
    }
    if phase == 4 {
        if receipt(db, &ctx, &req)?.is_none() {
            return Err(error(ErrorCode::PermissionDenied));
        }
        let op = req
            .operation_id()
            .ok_or_else(|| error(ErrorCode::InvalidRequest))?;
        let mut stmt=db.prepare("SELECT connection_id FROM access_cleanup_targets WHERE operation_id=?1 ORDER BY connection_id LIMIT 129").map_err(sql)?;
        let ids = stmt
            .query_map([op.0.to_string()], |r| r.get::<_, String>(0))
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        if ids.len() > 128 {
            return Err(error(ErrorCode::AssetIntegrityFailed));
        }
        return Ok(Reply::Cleanup(
            ids.iter()
                .map(|s| decode(&encode(s)?))
                .collect::<Result<_, _>>()?,
        ));
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql)?;
    if let Some(reply) = receipt(&tx, &ctx, &req)? {
        return Ok(Reply::Public(reply));
    }
    // Globally unique operation IDs also reserve enrollment before its success receipt.
    let existing = enrollment_collision(&tx, &ctx, &req)?;
    let reply = match &req {
        AccessRequest::List {} => {
            let mut stmt = tx
                .prepare("SELECT id FROM agent_profiles ORDER BY id LIMIT 65")
                .map_err(sql)?;
            let ids = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)?;
            let profiles = ids
                .iter()
                .map(|s| profile(&tx, decode(&encode(s)?)?))
                .collect::<Result<Vec<_>, _>>()?;
            let mut stmt = tx
                .prepare("SELECT id FROM agent_connections ORDER BY id LIMIT 129")
                .map_err(sql)?;
            let ids = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)?;
            let connections = ids
                .iter()
                .map(|s| connection(&tx, decode(&encode(s)?)?))
                .collect::<Result<Vec<_>, _>>()?;
            if profiles.len() > 64 || connections.len() > 128 {
                return Err(error(ErrorCode::AssetIntegrityFailed));
            }
            return Ok(Reply::Public(AccessReply::Catalog {
                profiles,
                connections,
            }));
        }
        AccessRequest::CreateProfile { name, .. } => {
            cap(&tx, "agent_profiles", 64)?;
            let id = KnowledgeId(Uuid::new_v4());
            let principal = KnowledgeId(Uuid::new_v4());
            tx.execute("INSERT INTO agent_profiles VALUES(?1,?2,1,1,'active',strftime('%Y-%m-%dT%H:%M:%fZ','now'),strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![id.0.to_string(),principal.0.to_string()]).map_err(sql)?;
            tx.execute("INSERT INTO agent_profile_versions VALUES(?1,1,?2,'metadata_only_v1','active',1,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![id.0.to_string(),name.0]).map_err(sql)?;
            let p = profile(&tx, id)?;
            event(&tx, &ctx, &req, &p, None, "create_profile")?;
            AccessReply::Profile(Box::new(p))
        }
        AccessRequest::UpdateProfile {
            profile_id,
            expected_revision,
            name,
            state: s,
            ..
        } => {
            let p = profile(&tx, *profile_id)?;
            if p.revision != *expected_revision {
                return Err(error(ErrorCode::RevisionConflict));
            }
            if p.state == AgentProfileState::Revoked {
                return Err(error(ErrorCode::PermissionDenied));
            }
            let revision = next(p.revision.0)?;
            let security = if p.state != *s {
                next(p.security_revision.0)?
            } else {
                p.security_revision.0
            };
            tx.execute("INSERT INTO agent_profile_versions VALUES(?1,?2,?3,'metadata_only_v1',?4,?5,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![profile_id.0.to_string(),revision,name.0,state(s)?,security]).map_err(sql)?;
            tx.execute("UPDATE agent_profiles SET revision=?2,security_revision=?3,state=?4,updated_at_utc=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",params![profile_id.0.to_string(),revision,security,state(s)?]).map_err(sql)?;
            if *s != AgentProfileState::Active {
                tx.execute("INSERT INTO access_cleanup_targets SELECT ?1,id FROM agent_connections WHERE profile_id=?2",params![req.operation_id().expect("mutation").0.to_string(),profile_id.0.to_string()]).map_err(sql)?;
            }
            let p = profile(&tx, *profile_id)?;
            event(&tx, &ctx, &req, &p, None, "update_profile")?;
            AccessReply::Profile(Box::new(p))
        }
        AccessRequest::IssueConnection {
            operation_id,
            profile_id,
            expected_profile_revision,
            display_name,
        } => {
            let prior = if existing {
                enrollment(&tx, &ctx, &req)?
            } else {
                None
            };
            if phase == 2 {
                if let Some(prior) = prior {
                    return Ok(Reply::Plan(Some(prior)));
                }
                let p = profile(&tx, *profile_id)?;
                if p.revision != *expected_profile_revision {
                    return Err(error(ErrorCode::RevisionConflict));
                }
                if p.state != AgentProfileState::Active {
                    return Err(error(ErrorCode::PermissionDenied));
                }
                cap(&tx, "agent_connections", 128)?;
                let id = AgentConnectionId(KnowledgeId(Uuid::new_v4()));
                let d = expected.ok_or_else(|| error(ErrorCode::InvalidRequest))?;
                tx.execute("INSERT INTO agent_connections VALUES(?1,?2,1,?3,'enrolling',1,?4,strftime('%Y-%m-%dT%H:%M:%fZ','now'),strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![id.0.0.to_string(),profile_id.0.to_string(),display_name.0,d.as_slice()]).map_err(sql)?;
                tx.execute("INSERT INTO agent_enrollments VALUES(?1,?2,?3,?4,'planned',strftime('%Y-%m-%dT%H:%M:%fZ','now'))",params![operation_id.0.to_string(),ctx.principal_id().as_uuid().to_string(),id.0.0.to_string(),fingerprint(&req)?]).map_err(sql)?;
                event(&tx, &ctx, &req, &p, Some(id), "plan_connection")?;
                let result = EnrollmentPlan {
                    connection: connection(&tx, id)?,
                    expected_digest: d,
                };
                tx.commit().map_err(sql)?;
                return Ok(Reply::Plan(Some(result)));
            }
            if phase != 3 {
                return Err(error(ErrorCode::InvalidRequest));
            }
            let plan = prior.ok_or_else(|| error(ErrorCode::PermissionDenied))?;
            let p = profile(&tx, *profile_id)?;
            if p.state != AgentProfileState::Active
                || plan.connection.state != AgentConnectionState::Enrolling
                || Some(plan.expected_digest) != expected
            {
                return Err(error(ErrorCode::PermissionDenied));
            }
            let id = plan.connection.connection_id;
            tx.execute("UPDATE agent_connections SET state='active',revision=revision+1,updated_at_utc=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",[id.0.0.to_string()]).map_err(sql)?;
            tx.execute(
                "UPDATE agent_enrollments SET state='activated' WHERE operation_id=?1",
                [operation_id.0.to_string()],
            )
            .map_err(sql)?;
            event(&tx, &ctx, &req, &p, Some(id), "activate_connection")?;
            AccessReply::Connection(Box::new(connection(&tx, id)?))
        }
        AccessRequest::RevokeConnection {
            connection_id,
            expected_revision,
            ..
        } => {
            let c = connection(&tx, *connection_id)?;
            if c.revision != *expected_revision {
                return Err(error(ErrorCode::RevisionConflict));
            }
            if c.state == AgentConnectionState::Revoked {
                return Err(error(ErrorCode::PermissionDenied));
            }
            let revision = next(c.revision.0)?;
            tx.execute("UPDATE agent_connections SET state='revoked',revision=?2,updated_at_utc=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",params![connection_id.0.0.to_string(),revision]).map_err(sql)?;
            let p = profile(&tx, c.profile_id)?;
            event(
                &tx,
                &ctx,
                &req,
                &p,
                Some(*connection_id),
                "revoke_connection",
            )?;
            AccessReply::Connection(Box::new(connection(&tx, *connection_id)?))
        }
    };
    save(&tx, &ctx, &req, &reply)?;
    tx.commit().map_err(sql)?;
    Ok(Reply::Public(reply))
}
fn cap(db: &Connection, table: &str, max: u64) -> Result<(), AppError> {
    let n: u64 = db
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .map_err(sql)?;
    if n >= max {
        Err(error(ErrorCode::InvalidRequest))
    } else {
        Ok(())
    }
}
fn enrollment_collision(
    db: &Connection,
    ctx: &RequestContext,
    req: &AccessRequest,
) -> Result<bool, AppError> {
    let Some(id) = req.operation_id() else {
        return Ok(false);
    };
    let row:Option<(String,String)>=db.query_row("SELECT owner_principal_id,request_fingerprint FROM agent_enrollments WHERE operation_id=?1",[id.0.to_string()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql)?;
    if let Some((p, f)) = row {
        if p != ctx.principal_id().as_uuid().to_string()
            || f != fingerprint(req)?
            || !matches!(req, AccessRequest::IssueConnection { .. })
        {
            return Err(error(ErrorCode::IdempotencyConflict));
        }
        return Ok(true);
    }
    Ok(false)
}
impl crate::StorageWorker {
    pub async fn agent_command(
        &self,
        ctx: RequestContext,
        command: Command,
    ) -> Result<Reply, AppError> {
        // The owned write permit travels with the admitted writer request. Dropping a
        // caller never releases publication ahead of an already-admitted COMMIT.
        let publication = if command.mutates() {
            Some(self.access_publication.clone().write_owned().await)
        } else {
            None
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let gate = self
                .gate
                .lock()
                .map_err(|_| error(ErrorCode::WaitTimeout))?;
            if gate.closing {
                return Err(error(ErrorCode::WaitTimeout));
            }
            self.sender
                .try_send(crate::Request::Access(tx, ctx, command, publication))
                .map_err(|_| error(ErrorCode::WaitTimeout))?;
        }
        tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| error(ErrorCode::WaitTimeout))?
            .map_err(|_| error(ErrorCode::WaitTimeout))?
    }
}
#[async_trait::async_trait]
impl AgentAuthority for crate::StorageWorker {
    async fn authenticate_agent(
        &self,
        library: LibraryId,
        connection: AgentConnectionId,
        supplied: &Secret,
        request: RequestId,
        correlation: CorrelationId,
    ) -> Result<RequestContext, AppError> {
        let _permit = self.access_publication.clone().read_owned().await;
        let ctx = RequestContext::from_authenticated_peer(
            request,
            correlation,
            PrincipalId::new(Uuid::nil()),
            vec![],
        );
        match self
            .agent_command(
                ctx,
                Command::Authenticate {
                    library,
                    connection,
                    secret: Secret::from_bytes(supplied.as_bytes().to_vec()),
                    request,
                    correlation,
                },
            )
            .await?
        {
            Reply::Context(c) => Ok(c),
            _ => Err(error(ErrorCode::WaitTimeout)),
        }
    }
    async fn validate_agent(&self, ctx: &RequestContext) -> Result<(), AppError> {
        if ctx.agent_binding().is_none() {
            return Ok(());
        }
        match self
            .agent_command(ctx.clone(), Command::Validate(ctx.clone()))
            .await?
        {
            Reply::Validated => Ok(()),
            _ => Err(error(ErrorCode::WaitTimeout)),
        }
    }
    async fn delivery_permit(&self, ctx: &RequestContext) -> Result<DeliveryPermit, AppError> {
        let permit = self.access_publication.clone().read_owned().await;
        self.validate_agent(ctx).await?;
        Ok(DeliveryPermit::hold(permit))
    }
}
#[cfg(test)]
mod tests;
