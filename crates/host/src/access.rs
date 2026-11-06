//! Native enrollment orchestration. SQL and SecretStore intentionally have no common transaction.
use sm_application::{
    access::AgentAccessApi,
    credentials::{CredentialEnvironment, CredentialKey, Secret, SecretStore, StoreError},
    RequestContext,
};
use sm_domain::LibraryId;
use sm_protocol::{
    access::*,
    errors::{AppError, ErrorCode},
};
use sm_storage::{
    access::{error, Command, Reply},
    StorageWorker,
};
use std::sync::Arc;

pub struct AccessServices {
    storage: StorageWorker,
    store: Arc<dyn SecretStore + Send + Sync>,
    environment: CredentialEnvironment,
    library: LibraryId,
    enrollment: tokio::sync::Mutex<()>,
    #[cfg(test)]
    fault: std::sync::atomic::AtomicU8,
}
impl AccessServices {
    pub fn new(
        storage: StorageWorker,
        store: Arc<dyn SecretStore + Send + Sync>,
        environment: CredentialEnvironment,
        library: LibraryId,
    ) -> Self {
        Self {
            storage,
            store,
            environment,
            library,
            enrollment: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            fault: std::sync::atomic::AtomicU8::new(0),
        }
    }
    fn key(&self, id: AgentConnectionId) -> CredentialKey {
        CredentialKey::agent(self.environment, self.library, id)
    }
    async fn issue(
        &self,
        ctx: RequestContext,
        req: AccessRequest,
    ) -> Result<AccessReply, AppError> {
        let _serial = self.enrollment.lock().await;
        let prior = match self
            .storage
            .agent_command(ctx.clone(), Command::EnrollmentLookup(req.clone()))
            .await?
        {
            Reply::Plan(p) => p,
            _ => return Err(error(ErrorCode::MethodNotSupported)),
        };
        let (plan, generated) = match prior {
            Some(plan) => (plan, None),
            None => {
                let secret = Secret::generate_session()
                    .map_err(|_| error(ErrorCode::CredentialStoreUnavailable))?;
                let digest = secret
                    .session_digest()
                    .map_err(|_| error(ErrorCode::CredentialStoreUnavailable))?;
                match self
                    .storage
                    .agent_command(ctx.clone(), Command::Plan(req.clone(), digest))
                    .await?
                {
                    Reply::Plan(Some(p)) => (p, Some(secret)),
                    _ => return Err(error(ErrorCode::MethodNotSupported)),
                }
            }
        };
        #[cfg(test)]
        if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 1 {
            return Err(error(ErrorCode::WaitTimeout));
        }
        if plan.connection.state == AgentConnectionState::Revoked {
            return Err(error(ErrorCode::PermissionDenied));
        }
        let key = self.key(plan.connection.connection_id);
        // An accidental retry with an absent slot never invents a replacement secret.
        match self.store.read(&key) {
            Ok(saved) if saved.matches_session_digest(&plan.expected_digest) => {}
            Err(StoreError::Missing) => {
                let generated =
                    generated.ok_or_else(|| error(ErrorCode::CredentialStoreUnavailable))?;
                if !generated.matches_session_digest(&plan.expected_digest) {
                    return Err(error(ErrorCode::CredentialStoreUnavailable));
                }
                self.store
                    .create_new(&key, &generated)
                    .map_err(|_| error(ErrorCode::CredentialStoreUnavailable))?;
            }
            _ => return Err(error(ErrorCode::CredentialStoreUnavailable)),
        }
        let readback = self
            .store
            .read(&key)
            .map_err(|_| error(ErrorCode::CredentialStoreUnavailable))?;
        if !readback.matches_session_digest(&plan.expected_digest) {
            return Err(error(ErrorCode::CredentialStoreUnavailable));
        }
        #[cfg(test)]
        if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 2 {
            return Err(error(ErrorCode::WaitTimeout));
        }
        let d = readback
            .session_digest()
            .map_err(|_| error(ErrorCode::CredentialStoreUnavailable))?;
        let reply = match self
            .storage
            .agent_command(ctx, Command::Activate(req, d))
            .await?
        {
            Reply::Public(r) => r,
            _ => return Err(error(ErrorCode::MethodNotSupported)),
        };
        #[cfg(test)]
        if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 3 {
            return Err(error(ErrorCode::WaitTimeout));
        }
        Ok(reply)
    }
}
#[async_trait::async_trait]
impl AgentAccessApi for AccessServices {
    async fn access(
        &self,
        ctx: RequestContext,
        req: AccessRequest,
    ) -> Result<AccessReply, AppError> {
        if !ctx.allows(sm_application::Scope::ManageAccess) || ctx.agent_binding().is_some() {
            return Err(error(ErrorCode::PermissionDenied));
        }
        if matches!(req, AccessRequest::IssueConnection { .. }) {
            return self.issue(ctx, req).await;
        }
        let cleanup = match &req {
            AccessRequest::RevokeConnection { connection_id, .. } => Some(*connection_id),
            _ => None,
        };
        let disabled = matches!(
            req,
            AccessRequest::UpdateProfile {
                state: AgentProfileState::Disabled | AgentProfileState::Revoked,
                ..
            }
        );
        let reply = match self
            .storage
            .agent_command(ctx.clone(), Command::Manage(req.clone()))
            .await?
        {
            Reply::Public(r) => r,
            _ => return Err(error(ErrorCode::MethodNotSupported)),
        };
        // Authority is already committed/published before the fallible exact delete.
        if let Some(id) = cleanup {
            self.store
                .delete_agent(&self.key(id))
                .map_err(|_| error(ErrorCode::CredentialStoreUnavailable))?;
        }
        if disabled {
            // Fixed durable targets from the original operation, not a replay-time
            // scan that could delete a newly issued unrelated connection's credential.
            if let Reply::Cleanup(ids) = self
                .storage
                .agent_command(ctx, Command::CleanupTargets(req))
                .await?
            {
                for id in ids {
                    self.store
                        .delete_agent(&self.key(id))
                        .map_err(|_| error(ErrorCode::CredentialStoreUnavailable))?;
                }
            } else {
                return Err(error(ErrorCode::MethodNotSupported));
            }
        }
        Ok(reply)
    }
}
#[cfg(test)]
mod tests;
