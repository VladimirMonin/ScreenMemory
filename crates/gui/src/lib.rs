//! S01-T06d — minimal *working* diagnostic window (Tauri v2 shell + typed client commands).
//!
//! Scope: one window that speaks to the *same* Rust application client as `shot`
//! (`sm-session` through the shared CLI integration primitives), with an explicit trusted
//! configuration, bounded states and a headless `--self-check`. Search, gallery, capture,
//! drop-zone, model settings and the bridge are marked as not implemented instead of
//! showing invented results.
//!
//! Boundaries: the frontend never imports an SDK, SQLite or a native capture binding — it
//! only calls these commands; the handlers never accept a path, executable or frame.
mod config;
mod model;
mod open;
mod selfcheck;
mod session;
mod shell;

pub use config::{load_window_config, parse_gui_args, GuiArgError, GuiArgs, CONFIG_SCHEMA_VERSION};
pub use model::{
    runtime_available, state_for_failure, state_for_status, GuiDiagnostics, GuiLimits, GuiState,
    OpenReportOutcome, GUI_ENVELOPE_VERSION, NOT_IMPLEMENTED,
};
pub use open::{validated_report_directory, OpenReportError, ReportOpener, SystemReportOpener};
pub use selfcheck::{run_self_check, SELF_CHECK_MODE};
pub use session::GuiSession;
pub use shell::{
    gui_connect, gui_doctor, gui_limits, gui_open_report, gui_probe, run_window, ShellState,
};
