//! Application seams only: host-created authority and ports; no concrete adapters.
pub mod credentials;
pub mod observability;
pub mod storage;

use async_trait::async_trait;
use sm_domain::{CorrelationId, PrincipalId, RequestId};
use sm_protocol::{
    application::{
        AiPolicyPatch, AiPolicyView, AuthorizeProbeRequest, DoctorReport, DoctorRequest,
        HostStatus, ProbeConsentReceipt, RuntimeProbeReport, RuntimeProbeRequest, ShutdownReceipt,
    },
    errors::AppError,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    ReadStatus,
    RunDoctor,
    ReadRuntimeInventory,
    RunAiProbe,
    LoadModelForProbe,
    ManageHost,
}
/// Constructed only by the authenticated host boundary; never decoded from request.params.
#[derive(Clone, Debug)]
pub struct RequestContext {
    request_id: RequestId,
    correlation_id: CorrelationId,
    principal_id: PrincipalId,
    granted_scopes: Vec<Scope>,
}
impl RequestContext {
    /// Host-only call site must first establish peer identity and authenticated scopes.
    pub fn from_authenticated_peer(
        request_id: RequestId,
        correlation_id: CorrelationId,
        principal_id: PrincipalId,
        granted_scopes: Vec<Scope>,
    ) -> Self {
        Self {
            request_id,
            correlation_id,
            principal_id,
            granted_scopes,
        }
    }
    pub fn request_id(&self) -> RequestId {
        self.request_id
    }
    pub fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }
    pub fn principal_id(&self) -> PrincipalId {
        self.principal_id
    }
    pub fn allows(&self, scope: Scope) -> bool {
        self.granted_scopes.contains(&scope)
    }
}

#[async_trait]
pub trait FoundationApi: Send + Sync {
    async fn status(&self, ctx: RequestContext) -> Result<HostStatus, AppError>;
    async fn doctor(
        &self,
        ctx: RequestContext,
        request: DoctorRequest,
        cancel: CancellationToken,
    ) -> Result<DoctorReport, AppError>;
    async fn probe_runtime(
        &self,
        ctx: RequestContext,
        request: RuntimeProbeRequest,
        cancel: CancellationToken,
    ) -> Result<RuntimeProbeReport, AppError>;
}
#[async_trait]
pub trait FoundationControlApi: Send + Sync {
    async fn set_ai_policy(
        &self,
        ctx: RequestContext,
        patch: AiPolicyPatch,
    ) -> Result<AiPolicyView, AppError>;
    async fn authorize_probe(
        &self,
        ctx: RequestContext,
        request: AuthorizeProbeRequest,
    ) -> Result<ProbeConsentReceipt, AppError>;
    async fn request_shutdown(&self, ctx: RequestContext) -> Result<ShutdownReceipt, AppError>;
}

/// Outbound ports expose capabilities without implementing storage, SDK, or policy here.
#[async_trait]
pub trait RuntimeProbePort: Send + Sync {
    async fn probe(
        &self,
        request: RuntimeProbeRequest,
        cancel: CancellationToken,
    ) -> Result<RuntimeProbeReport, AppError>;
}
