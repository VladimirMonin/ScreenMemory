//! Path-free shared search port. Authority comes only from the authenticated context.
use crate::RequestContext;
use async_trait::async_trait;
use sm_protocol::{
    errors::AppError,
    search::{SearchReply, SearchRequest},
};
use tokio_util::sync::CancellationToken;
#[async_trait]
pub trait SearchApi: Send + Sync {
    async fn search(
        &self,
        ctx: RequestContext,
        request: SearchRequest,
        cancel: CancellationToken,
    ) -> Result<SearchReply, AppError>;
}
