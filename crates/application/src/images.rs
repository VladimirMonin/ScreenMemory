//! The image service is independent of an RPC waiter's cancellation or connection.
use crate::RequestContext;
use async_trait::async_trait;
use sm_protocol::errors::AppError;
pub use sm_protocol::images::*;
#[async_trait]
pub trait ImageLibraryApi: Send + Sync {
    async fn preview_asset(
        &self,
        _ctx: RequestContext,
        _request: sm_protocol::application::LibraryAssetRequest,
    ) -> Result<sm_protocol::application::LibraryAssetChunk, AppError> {
        Err(AppError::method_not_supported())
    }
    async fn record_metadata(
        &self,
        _ctx: RequestContext,
        _request: RecordMetadataRequest,
    ) -> Result<RecordMetadata, AppError> {
        Err(AppError::method_not_supported())
    }
    async fn import_paths(
        &self,
        ctx: RequestContext,
        request: ImportRequest,
    ) -> Result<ImportReport, AppError>;
    async fn import_status(
        &self,
        ctx: RequestContext,
        request: ImportBatchRequest,
    ) -> Result<ImportReport, AppError>;
    async fn import_report_page(
        &self,
        ctx: RequestContext,
        request: ImportReportPageRequest,
    ) -> Result<ImportReport, AppError>;
    async fn import_control(
        &self,
        ctx: RequestContext,
        request: ImportControlRequest,
    ) -> Result<ImportReport, AppError>;
    async fn export_image(
        &self,
        ctx: RequestContext,
        request: ExportRequest,
    ) -> Result<ExportReceipt, AppError>;
}
