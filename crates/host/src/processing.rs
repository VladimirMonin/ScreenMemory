//! Host authorization for durable processing. Waiter cancellation never owns the execution task.
pub use crate::processing_execution::TrustedProcessingRuntime;
use crate::processing_execution::{ExecutionOwner, RuntimeStatus};
use async_trait::async_trait;
use sm_application::{processing::ProcessingApi, RequestContext, Scope};
use sm_domain::HostInstanceId;
use sm_protocol::{
    errors::{AppError, ErrorCode},
    processing::{ProcessingReply, ProcessingRequest, WorkKind},
    values::SafeMessage,
};
use sm_storage::{
    image_services::ImageServices, processing::ProcessingError, processing_execution::Command,
    StorageWorker,
};
use std::sync::Arc;

pub struct ProcessingServices {
    storage: StorageWorker,
    execution: Option<ExecutionOwner>,
    runtime_status: Arc<RuntimeStatus>,
}
impl ProcessingServices {
    pub async fn compose(
        storage: StorageWorker,
        host: HostInstanceId,
        config: Option<TrustedProcessingRuntime>,
        images: Option<Arc<ImageServices>>,
    ) -> Result<Self, ProcessingError> {
        if config.is_some() && images.is_none() {
            return Err(ProcessingError::Unavailable);
        }
        storage
            .execution(Command::Recover {
                host,
                runtime_enabled: config.is_some(),
            })
            .await?;
        let runtime_status = Arc::new(RuntimeStatus::new(config.is_some()));
        let execution = config.map(|config| {
            ExecutionOwner::start(
                storage.clone(),
                host,
                config,
                images.expect("checked image owner"),
                runtime_status.clone(),
            )
        });
        Ok(Self {
            storage,
            execution,
            runtime_status,
        })
    }
    pub(crate) fn runtime_status(&self) -> Arc<RuntimeStatus> {
        self.runtime_status.clone()
    }
    pub(crate) async fn shutdown(&self) -> bool {
        match &self.execution {
            Some(owner) => owner.shutdown().await,
            None => true,
        }
    }
}
fn error(code: ErrorCode) -> AppError {
    AppError {
        code,
        safe_message: SafeMessage::parse("Processing request rejected.").expect("constant"),
        retryable: false,
    }
}
#[async_trait]
impl ProcessingApi for ProcessingServices {
    async fn processing(
        &self,
        ctx: RequestContext,
        request: ProcessingRequest,
    ) -> Result<ProcessingReply, AppError> {
        let read = matches!(
            request,
            ProcessingRequest::Policy {}
                | ProcessingRequest::Overview { .. }
                | ProcessingRequest::Inventory {}
                | ProcessingRequest::Profile { .. }
                | ProcessingRequest::Batch { .. }
                | ProcessingRequest::Execution { .. }
                | ProcessingRequest::Reconcile {}
        );
        if !ctx.allows(if read {
            Scope::ReadProcessing
        } else {
            Scope::ManageProcessing
        }) {
            return Err(error(ErrorCode::PermissionDenied));
        }
        if matches!(
            request,
            ProcessingRequest::Plan {
                kind: WorkKind::TechnicalOwnerSelfTest | WorkKind::TechnicalOwnerBackgroundSelfTest,
                ..
            }
        ) && [
            Scope::RunAiProbe,
            Scope::ReadImages,
            Scope::LoadModelForProbe,
            Scope::ReadRuntimeInventory,
            Scope::ExportImages,
        ]
        .iter()
        .any(|s| !ctx.allows(*s))
        {
            return Err(error(ErrorCode::PermissionDenied));
        }
        if matches!(request, ProcessingRequest::Inventory {}) {
            if !ctx.allows(Scope::ReadRuntimeInventory) {
                return Err(error(ErrorCode::PermissionDenied));
            }
            let owner = self
                .execution
                .as_ref()
                .ok_or_else(|| error(ErrorCode::RuntimeUnavailable))?;
            return owner
                .inventory(ctx)
                .await
                .map(ProcessingReply::Inventory)
                .map_err(|_| error(ErrorCode::RuntimeUnavailable));
        }
        // Activation, policy and replay are transactional in the writer. The writer wakes
        // the independent host lane even if the session drops this waiter before delivery.
        let mut reply = self
            .storage
            .processing(ctx.principal_id(), request)
            .await
            .map_err(|e| {
                error(match e {
                    ProcessingError::Invalid => ErrorCode::InvalidRequest,
                    ProcessingError::Conflict => ErrorCode::RevisionConflict,
                    ProcessingError::Disabled => ErrorCode::AiDisabled,
                    ProcessingError::Unsupported => ErrorCode::ModelConfigUnsupported,
                    ProcessingError::NotFound => ErrorCode::AssetNotFound,
                    ProcessingError::Uncertain | ProcessingError::Unavailable => {
                        ErrorCode::RuntimeUnavailable
                    }
                    ProcessingError::Integrity => ErrorCode::SchemaIncompatible,
                })
            })?;
        if let ProcessingReply::Overview(view) = &mut reply {
            view.bridge_state = self.runtime_status.bridge();
        }
        Ok(reply)
    }
}
