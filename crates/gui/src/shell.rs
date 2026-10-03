//! Tauri v2 window and its command handlers (S01 §10.1).
//!
//! Every handler is a thin typed adapter over one `GuiSession`, i.e. over the same
//! shared `sm-session` client the CLI uses. No handler accepts a path, a host image or a
//! protocol frame from the frontend, and no handler can stop the host: closing the window
//! only closes this connection.
use crate::{
    model::{
        GuiAssetPreview, GuiCaptureAnswer, GuiDiagnostics, GuiLibraryPage, GuiLimits,
        OpenReportOutcome,
    },
    open::ReportOpener,
    session::GuiSession,
};
use sm_protocol::application::CaptureMode;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, WebviewWindowBuilder};
use tokio::sync::Mutex;

/// One window owns one session; the mutex serializes the bounded attempts so two clicks
/// can never run two overlapping connects or start two hosts.
pub struct ShellState {
    session: Arc<Mutex<GuiSession>>,
    opener: Arc<dyn ReportOpener>,
    allow_start: bool,
}

impl ShellState {
    pub fn new(session: GuiSession, opener: Arc<dyn ReportOpener>, allow_start: bool) -> Self {
        Self {
            session: Arc::new(Mutex::new(session)),
            opener,
            allow_start,
        }
    }

    fn session(&self) -> Arc<Mutex<GuiSession>> {
        self.session.clone()
    }
}

/// Bounded windows of this build, so the window can show a bounded progress state.
#[tauri::command]
pub async fn gui_limits() -> GuiLimits {
    GuiLimits::default()
}

/// One no-start attempt: the state the window shows right after it opens.
#[tauri::command]
pub async fn gui_probe(app: AppHandle) -> GuiDiagnostics {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    let diagnostics = guard.probe().await;
    drop(guard);
    diagnostics
}

/// The «Подключиться» action: bounded connect, at most one start of the fixed image.
#[tauri::command]
pub async fn gui_connect(app: AppHandle) -> GuiDiagnostics {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    let diagnostics = guard
        .run(app.state::<ShellState>().allow_start, false)
        .await;
    drop(guard);
    diagnostics
}

/// The «Проверить основу» action: bounded connect plus the ordinary doctor.
#[tauri::command]
pub async fn gui_doctor(app: AppHandle) -> GuiDiagnostics {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    let diagnostics = guard.run(app.state::<ShellState>().allow_start, true).await;
    drop(guard);
    diagnostics
}

/// A fixed first page; no renderer-supplied cursor, filter or path.
#[tauri::command]
pub async fn gui_library_recent(app: AppHandle) -> GuiLibraryPage {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.list(24, None).await
}

#[tauri::command]
pub async fn gui_library_page(app: AppHandle, cursor: Option<String>) -> GuiLibraryPage {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.list(24, cursor).await
}
#[tauri::command]
pub async fn gui_import(
    app: AppHandle,
    request: sm_protocol::images::ImportRequest,
) -> Result<sm_protocol::images::ImportReport, String> {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.import_paths(request).await
}
#[tauri::command]
pub async fn gui_import_report(
    app: AppHandle,
    request: sm_protocol::images::ImportReportPageRequest,
) -> Result<sm_protocol::images::ImportReport, String> {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.import_report(request).await
}
#[tauri::command]
pub async fn gui_import_control(
    app: AppHandle,
    request: sm_protocol::images::ImportControlRequest,
) -> Result<sm_protocol::images::ImportReport, String> {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.import_control(request).await
}
#[tauri::command]
pub async fn gui_export(
    app: AppHandle,
    request: sm_protocol::images::ExportRequest,
) -> Result<sm_protocol::images::ExportReceipt, String> {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.export_image(request).await
}
#[tauri::command]
pub async fn gui_metadata(
    app: AppHandle,
    request: sm_protocol::images::RecordMetadataRequest,
) -> Result<sm_protocol::images::RecordMetadata, String> {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.metadata(request).await
}

/// Typed processing only: no paths/secrets/endpoint/approval/load flags accepted.
#[tauri::command]
pub async fn gui_processing(
    app: AppHandle,
    request: sm_protocol::processing::ProcessingRequest,
) -> Result<sm_protocol::processing::ProcessingReply, String> {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.processing(request).await
}

/// Only a canonical UUIDv4 asset identity crosses the WebView boundary. The host
/// still checks whether that asset exists and is readable in the authenticated library.
pub fn parse_preview_asset_id(text: &str) -> Option<sm_domain::AssetId> {
    let id = uuid::Uuid::parse_str(text).ok()?;
    if text != id.hyphenated().to_string()
        || id.get_version() != Some(uuid::Version::Random)
        || id.get_variant() != uuid::Variant::RFC4122
    {
        return None;
    }
    Some(sm_domain::AssetId::new(id))
}

#[tauri::command]
pub async fn gui_asset_preview(app: AppHandle, asset_id: String) -> GuiAssetPreview {
    let Some(id) = parse_preview_asset_id(&asset_id) else {
        return GuiAssetPreview::unavailable("PREVIEW_UNAVAILABLE");
    };
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.read_asset_full(id).await
}

/// The «Открыть диагностический отчёт» action. The handler takes no path: only the
/// validated cache/log directory of the explicit configuration can ever be opened.
/// One capture command per mode. Every handler stays **argument free**, so the WebView
/// still cannot name a mode string, a window, a monitor, a path or a pixel buffer; the
/// closed set of registered handlers *is* the accepted vocabulary (S02 section 11).
async fn capture_with(app: &AppHandle, mode: CaptureMode) -> GuiCaptureAnswer {
    let session = app.state::<ShellState>().session();
    let mut guard = session.lock().await;
    guard.capture(mode).await
}

/// Capture the region selected on the frozen desktop.
#[tauri::command]
pub async fn gui_capture_region(app: AppHandle) -> GuiCaptureAnswer {
    capture_with(&app, CaptureMode::Region).await
}

/// Capture the active external window.
#[tauri::command]
pub async fn gui_capture_window(app: AppHandle) -> GuiCaptureAnswer {
    capture_with(&app, CaptureMode::Window).await
}

/// Capture the monitor under the cursor.
#[tauri::command]
pub async fn gui_capture_monitor(app: AppHandle) -> GuiCaptureAnswer {
    capture_with(&app, CaptureMode::Monitor).await
}

/// Capture every connected monitor.
#[tauri::command]
pub async fn gui_capture_desktop(app: AppHandle) -> GuiCaptureAnswer {
    capture_with(&app, CaptureMode::Desktop).await
}

#[tauri::command]
pub async fn gui_open_report(app: AppHandle) -> OpenReportOutcome {
    let state = app.state::<ShellState>();
    let session = state.session();
    let opener = state.opener.clone();
    let guard = session.lock().await;
    guard.open_report(opener.as_ref())
}

/// Build the window only after root validation. WebView data stays in the explicit
/// synthetic root, never Tauri's default per-user application directory.
pub fn run_window(state: ShellState, webview_directory: std::path::PathBuf) -> tauri::Result<()> {
    tauri::Builder::default()
        .manage(state)
        .setup(move |app| {
            let config = &app.config().app.windows[0];
            let window = WebviewWindowBuilder::from_config(app, config)?
                .data_directory(webview_directory)
                .build()?;
            let event_window = window.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) =
                    event
                {
                    let paths: Option<Vec<String>> = paths
                        .iter()
                        .map(|p| p.to_str().map(str::to_owned))
                        .collect();
                    if let Some(paths) = paths.filter(|p| !p.is_empty() && p.len() <= 128) {
                        let _ = event_window.emit("gui-import-paths", paths);
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            gui_limits,
            gui_processing,
            gui_probe,
            gui_connect,
            gui_doctor,
            gui_capture_region,
            gui_capture_window,
            gui_capture_monitor,
            gui_capture_desktop,
            gui_library_recent,
            gui_library_page,
            gui_import,
            gui_import_report,
            gui_import_control,
            gui_export,
            gui_metadata,
            gui_asset_preview,
            gui_open_report
        ])
        .run(tauri::generate_context!())
}
