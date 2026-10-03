use crate::RequestContext;
use async_trait::async_trait;
use sm_protocol::{
    errors::AppError,
    processing::{ProcessingReply, ProcessingRequest},
};

/// Session cancellation drops only the waiter. Once admitted, durable controls commit
/// on the single SQLite writer; work cancellation requires an explicit domain control.
#[async_trait]
pub trait ProcessingApi: Send + Sync {
    async fn processing(
        &self,
        ctx: RequestContext,
        request: ProcessingRequest,
    ) -> Result<ProcessingReply, AppError>;
}
