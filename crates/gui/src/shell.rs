//! Tauri v2 window and its command handlers (S01 §10.1).
//!
//! Every handler is a thin typed adapter over one `GuiSession`, i.e. over the same
//! shared `sm-session` client the CLI uses. No handler accepts a path, a host image or a
//! protocol frame from the frontend, and no handler can stop the host: closing the window
//! only closes this connection.
use crate::{
    model::{GuiDiagnostics, GuiLimits, OpenReportOutcome},
    open::ReportOpener,
    session::GuiSession,
};
use std::sync::Arc;
use tauri::{AppHandle, Manager, WebviewWindowBuilder};
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

/// The «Открыть диагностический отчёт» action. The handler takes no path: only the
/// validated cache/log directory of the explicit configuration can ever be opened.
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
            WebviewWindowBuilder::from_config(app, config)?
                .data_directory(webview_directory)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            gui_limits,
            gui_probe,
            gui_connect,
            gui_doctor,
            gui_open_report
        ])
        .run(tauri::generate_context!())
}
