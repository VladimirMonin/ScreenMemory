//! Native agent authority ports. Durable bindings are not deserializable client claims.
use crate::{credentials::Secret, RequestContext};
use async_trait::async_trait;
use sm_domain::{CorrelationId, LibraryId, RequestId};
use sm_protocol::{
    access::{AccessReply, AccessRequest, AgentConnectionId},
    errors::AppError,
    knowledge::SqlRevision,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentBinding {
    pub connection_id: AgentConnectionId,
    pub security_revision: SqlRevision,
    pub credential_generation: SqlRevision,
}
/// Keeps the publication boundary alive through actual framed delivery. No wire constructor.
pub struct DeliveryPermit {
    _guard: Box<dyn Send + Sync>,
}
impl DeliveryPermit {
    pub fn hold(guard: impl Send + Sync + 'static) -> Self {
        Self {
            _guard: Box::new(guard),
        }
    }
}
#[async_trait]
pub trait AgentAuthority: Send + Sync {
    async fn authenticate_agent(
        &self,
        library: LibraryId,
        connection: AgentConnectionId,
        supplied: &Secret,
        request: RequestId,
        correlation: CorrelationId,
    ) -> Result<RequestContext, AppError>;
    async fn validate_agent(&self, ctx: &RequestContext) -> Result<(), AppError>;
    async fn delivery_permit(&self, ctx: &RequestContext) -> Result<DeliveryPermit, AppError>;
}
#[async_trait]
pub trait AgentAccessApi: Send + Sync {
    async fn access(
        &self,
        ctx: RequestContext,
        request: AccessRequest,
    ) -> Result<AccessReply, AppError>;
}
