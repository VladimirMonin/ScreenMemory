//! One diagnostic window: at most one shared `sm-session` application client.
//!
//! The window never speaks the wire protocol itself and never starts a second host: it
//! reuses the *same* client-side integration primitives as `shot` (explicit trusted
//! configuration, fixed absolute host image, bounded discovery, typed DTOs). Closing the
//! window drops only this connection; the host keeps running and the next window connects
//! to the same instance.
use crate::{
    model::{
        state_for_failure, GuiAssetPreview, GuiCaptureAnswer, GuiDiagnostics, GuiLibraryPage,
        OpenReportOutcome, PreviewAssembler,
    },
    open::{safe_code, validated_report_directory, OpenReportError, ReportOpener},
};
use sm_cli::{
    client_failure, connect_session, resolve_target, target_failure, CliDeps, ClientFailure,
    PreviewConfig, ResolvedTarget,
};
use sm_domain::{LibraryId, OperationId};
use sm_protocol::application::{
    CaptureCommandParams, CaptureMode, DoctorReport, HostStatus, LibraryRecordsRequest,
};
use sm_session::{ApplicationClient, ClientError};
use std::path::PathBuf;

/// Longest wait of one interactive capture command; it is independent of the IPC handshake.
pub const CAPTURE_WAIT_MS: u32 = 30_000;
fn gui_code(failure: ClientFailure) -> String {
    serde_json::to_value(failure.code)
        .expect("code")
        .as_str()
        .expect("string")
        .to_owned()
}

pub struct GuiSession {
    config: PreviewConfig,
    target: ResolvedTarget,
    deps: CliDeps,
    client: Option<ApplicationClient>,
}

impl GuiSession {
    /// Validate the resolved client target of the explicit configuration before any
    /// window is created; a refusal is reported with the shared safe codes.
    pub fn open(config: PreviewConfig, deps: CliDeps) -> Result<Self, ClientFailure> {
        let target = resolve_target(&config).map_err(target_failure)?;
        Ok(Self {
            config,
            target,
            deps,
            client: None,
        })
    }

    pub async fn knowledge(
        &mut self,
        request: sm_protocol::knowledge::KnowledgeRequest,
    ) -> Result<sm_protocol::knowledge::KnowledgeReply, String> {
        self.ensure(true).await.map_err(gui_code)?;
        let result = self
            .client
            .as_ref()
            .expect("connected")
            .knowledge(request)
            .await;
        self.finish(result).map_err(gui_code)
    }
    pub async fn processing(
        &mut self,
        request: sm_protocol::processing::ProcessingRequest,
    ) -> Result<sm_protocol::processing::ProcessingReply, String> {
        self.ensure(true).await.map_err(gui_code)?;
        let result = self
            .client
            .as_ref()
            .expect("connected")
            .processing(request)
            .await;
        self.finish(result).map_err(gui_code)
    }
    pub fn library_id(&self) -> LibraryId {
        self.config.library_id
    }

    /// True when the owned cache/log directory of the configuration is usable.
    pub fn report_available(&self) -> bool {
        validated_report_directory(&self.config).is_ok()
    }

    pub fn report_directory(&self) -> Result<PathBuf, OpenReportError> {
        validated_report_directory(&self.config)
    }

    /// One bounded attempt without starting a host: exactly what the window shows first.
    pub async fn probe(&mut self) -> GuiDiagnostics {
        self.run(false, false).await
    }

    /// The «Подключиться» action: at most one start of the fixed trusted image.
    pub async fn connect(&mut self) -> GuiDiagnostics {
        self.run(true, false).await
    }

    /// The «Проверить основу» action: connect (bounded, may start the fixed image) then
    /// the ordinary doctor through the same client. No model or runtime inventory call.
    pub async fn doctor(&mut self) -> GuiDiagnostics {
        self.run(true, true).await
    }

    /// The sequence every window command and the headless self-check share:
    /// connect (bounded), status, and optionally the ordinary doctor.
    pub async fn run(&mut self, allow_start: bool, with_doctor: bool) -> GuiDiagnostics {
        let report_available = self.report_available();
        let status = match self.status(allow_start).await {
            Ok(status) => status,
            Err(failure) => {
                return GuiDiagnostics::refused(
                    state_for_failure(&failure),
                    &failure,
                    report_available,
                )
            }
        };
        if !with_doctor {
            return GuiDiagnostics::from_status(status, report_available);
        }
        match self.doctor_report(allow_start).await {
            Ok(report) => GuiDiagnostics::with_doctor(status, report, report_available),
            Err(failure) => {
                GuiDiagnostics::refused(state_for_failure(&failure), &failure, report_available)
            }
        }
    }

    async fn ensure(&mut self, allow_start: bool) -> Result<(), ClientFailure> {
        if self.client.is_some() {
            return Ok(());
        }
        let client = connect_session(&self.config, &self.target, &self.deps, !allow_start).await?;
        self.client = Some(client);
        Ok(())
    }

    pub async fn status(&mut self, allow_start: bool) -> Result<HostStatus, ClientFailure> {
        self.ensure(allow_start).await?;
        let result = self
            .client
            .as_ref()
            .expect("client is connected")
            .status()
            .await;
        self.finish(result)
    }

    pub async fn doctor_report(
        &mut self,
        allow_start: bool,
    ) -> Result<DoctorReport, ClientFailure> {
        self.ensure(allow_start).await?;
        let result = self
            .client
            .as_ref()
            .expect("client is connected")
            .doctor()
            .await;
        self.finish(result)
    }

    /// A failed call drops this connection so the next command reconnects instead of
    /// reusing a session whose reader may already be closed. The host is untouched.
    fn finish<T>(&mut self, result: Result<T, ClientError>) -> Result<T, ClientFailure> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.client = None;
                Err(client_failure(error))
            }
        }
    }

    /// The «Открыть диагностический отчёт» action: validate the directory derived from
    /// this configuration and hand exactly that path to the opener. No caller-supplied
    /// path exists on this surface.
    pub fn open_report(&self, opener: &dyn ReportOpener) -> OpenReportOutcome {
        match self
            .report_directory()
            .and_then(|directory| opener.open(&directory))
        {
            Ok(()) => OpenReportOutcome {
                opened: true,
                safe_code: None,
            },
            Err(error) => OpenReportOutcome {
                opened: false,
                safe_code: Some(safe_code(error)),
            },
        }
    }

    /// The capture action: the *same* host command the CLI uses. The native session runs
    /// inside the host; this window only sends an authorized request and shows the answer.
    pub async fn capture(&mut self, mode: CaptureMode) -> GuiCaptureAnswer {
        if let Err(failure) = self.ensure(true).await {
            return GuiCaptureAnswer::refused(&failure);
        }
        let params = CaptureCommandParams {
            operation_id: OperationId::new(uuid::Uuid::new_v4()),
            mode,
        };
        let result = self
            .client
            .as_ref()
            .expect("client is connected")
            .capture_command(params, CAPTURE_WAIT_MS, None)
            .await;
        match self.finish(result) {
            Ok(outcome) => GuiCaptureAnswer::from_outcome(&outcome),
            Err(failure) => GuiCaptureAnswer::refused(&failure),
        }
    }

    /// S02-T11: list committed records from the host library.
    pub async fn list(&mut self, limit: u16, cursor: Option<String>) -> GuiLibraryPage {
        if let Err(failure) = self.ensure(true).await {
            return GuiLibraryPage::refused(&failure);
        }
        let request = LibraryRecordsRequest { limit, cursor };
        let result = self
            .client
            .as_ref()
            .expect("client is connected")
            .list_records(request)
            .await;
        match self.finish(result) {
            Ok(page) => GuiLibraryPage::from_page(&page),
            Err(failure) => GuiLibraryPage::refused(&failure),
        }
    }

    /// S02-T11: read a complete canonical asset (preview image) from the host.
    pub async fn read_asset_full(&mut self, asset_id: sm_domain::AssetId) -> GuiAssetPreview {
        if let Err(failure) = self.ensure(true).await {
            return GuiAssetPreview::refused(&failure);
        }
        let mut preview = PreviewAssembler::new(asset_id);
        let mut offset: u32 = 0;
        loop {
            let request = sm_protocol::application::LibraryAssetRequest { asset_id, offset };
            match self
                .client
                .as_ref()
                .expect("client is connected")
                .preview_asset(request)
                .await
            {
                Ok(chunk) => {
                    let next = chunk.next_offset;
                    match preview.push(chunk) {
                        Ok(Some(image)) => return image,
                        Ok(None) => offset = next,
                        Err(code) => return GuiAssetPreview::unavailable(code),
                    }
                }
                Err(error) => {
                    self.client = None;
                    return GuiAssetPreview::refused(&client_failure(error));
                }
            }
        }
    }

    pub async fn import_paths(
        &mut self,
        request: sm_protocol::images::ImportRequest,
    ) -> Result<sm_protocol::images::ImportReport, String> {
        self.ensure(true).await.map_err(gui_code)?;
        let result = self
            .client
            .as_ref()
            .expect("connected")
            .import_paths(request)
            .await;
        self.finish(result).map_err(gui_code)
    }
    pub async fn import_report(
        &mut self,
        request: sm_protocol::images::ImportReportPageRequest,
    ) -> Result<sm_protocol::images::ImportReport, String> {
        self.ensure(true).await.map_err(gui_code)?;
        let result = self
            .client
            .as_ref()
            .expect("connected")
            .import_report_page(request)
            .await;
        self.finish(result).map_err(gui_code)
    }
    pub async fn import_control(
        &mut self,
        request: sm_protocol::images::ImportControlRequest,
    ) -> Result<sm_protocol::images::ImportReport, String> {
        self.ensure(true).await.map_err(gui_code)?;
        let result = self
            .client
            .as_ref()
            .expect("connected")
            .import_control(request)
            .await;
        self.finish(result).map_err(gui_code)
    }
    pub async fn export_image(
        &mut self,
        request: sm_protocol::images::ExportRequest,
    ) -> Result<sm_protocol::images::ExportReceipt, String> {
        self.ensure(true).await.map_err(gui_code)?;
        let result = self
            .client
            .as_ref()
            .expect("connected")
            .export_image(request, None)
            .await;
        self.finish(result).map_err(gui_code)
    }
    pub async fn metadata(
        &mut self,
        request: sm_protocol::images::RecordMetadataRequest,
    ) -> Result<sm_protocol::images::RecordMetadata, String> {
        self.ensure(true).await.map_err(gui_code)?;
        let result = self
            .client
            .as_ref()
            .expect("connected")
            .record_metadata(request)
            .await;
        self.finish(result).map_err(gui_code)
    }
    /// Close this window's connection only; the host is never stopped from here.
    pub fn close(&mut self) {
        self.client = None;
    }
}
