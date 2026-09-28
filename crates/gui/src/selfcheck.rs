//! Headless `--self-check`: real connect + status + doctor against the configured owned
//! host, exactly one JSON value on stdout and the CLI exit codes. This is the executable
//! proof of the window's client path without driving the webview.
use crate::{
    config::{load_window_config, parse_gui_args},
    model::{state_for_failure, GuiDiagnostics},
    session::GuiSession,
};
use serde::Serialize;
use sm_cli::{config_message, doctor_exit_code, CliDeps, ClientFailure, EXIT_USAGE};
use sm_protocol::{
    errors::{AppError, ErrorCode},
    values::SafeMessage,
};
use std::io::Write;

/// Marker of the machine-readable self-check report.
pub const SELF_CHECK_MODE: &str = "gui-self-check";

const USAGE_MESSAGE: &str =
    "Invalid arguments. Expected --self-check --config <absolute local path> [--no-start].";

#[derive(Serialize)]
struct SelfCheckReport<'a> {
    mode: &'static str,
    exit_code: u8,
    error: Option<AppError>,
    diagnostics: Option<&'a GuiDiagnostics>,
}

/// Run one headless self-check and return the process exit code. stdout receives exactly
/// one JSON value; diagnostics never carry a path, frame, credential or OS string.
pub async fn run_self_check(
    argv: &[String],
    deps: &CliDeps,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let args = match parse_gui_args(argv) {
        Ok(args) if args.self_check => args,
        _ => return emit(usage_failure(), EXIT_USAGE, None, out, err),
    };
    let config = match load_window_config(&args.config) {
        Ok(config) => config,
        Err(error) => {
            return emit(
                ClientFailure {
                    exit: EXIT_USAGE,
                    code: ErrorCode::InvalidRequest,
                    message: config_message(error),
                    retryable: false,
                },
                EXIT_USAGE,
                None,
                out,
                err,
            )
        }
    };
    let mut session = match GuiSession::open(config, deps.clone()) {
        Ok(session) => session,
        Err(failure) => {
            let exit = failure.exit;
            return emit(failure, exit, None, out, err);
        }
    };
    let report_available = session.report_available();
    // The window commands use `GuiSession::run`, which performs this same sequence; the
    // native tests assert both orchestrations produce the same diagnostics.
    let status = match session.status(args.allow_start).await {
        Ok(status) => status,
        Err(failure) => {
            let diagnostics =
                GuiDiagnostics::refused(state_for_failure(&failure), &failure, report_available);
            let exit = failure.exit;
            return emit(failure, exit, Some(diagnostics), out, err);
        }
    };
    match session.doctor_report(args.allow_start).await {
        Ok(report) => {
            // A report whose only deviations are optional NOT_RUN/UNSUPPORTED checks
            // stays successful; a failed mandatory check makes the command non-zero.
            let exit = doctor_exit_code(&report);
            let diagnostics = GuiDiagnostics::with_doctor(status, report, report_available);
            write_report(out, exit, None, Some(&diagnostics));
            exit
        }
        Err(failure) => {
            let diagnostics =
                GuiDiagnostics::refused(state_for_failure(&failure), &failure, report_available);
            let exit = failure.exit;
            emit(failure, exit, Some(diagnostics), out, err)
        }
    }
}

fn usage_failure() -> ClientFailure {
    ClientFailure {
        exit: EXIT_USAGE,
        code: ErrorCode::InvalidRequest,
        message: USAGE_MESSAGE,
        retryable: false,
    }
}

/// A refusal is reported with the stable exit code of the same failure in `shot` and the
/// shared safe message; the protocol code travels as the `AppError` shape of the CLI.
fn emit(
    failure: ClientFailure,
    exit_code: u8,
    diagnostics: Option<GuiDiagnostics>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let error = AppError {
        code: failure.code,
        safe_message: SafeMessage::parse(failure.message).expect("static safe message"),
        retryable: failure.retryable,
    };
    // The stable protocol token only; never the wire message, a path or an OS string.
    if let Ok(serde_json::Value::String(token)) = serde_json::to_value(failure.code) {
        let _ = writeln!(
            err,
            "screenmemory-gui --self-check: {token}: {}",
            failure.message
        );
    }
    write_report(out, exit_code, Some(error), diagnostics.as_ref());
    exit_code
}

fn write_report(
    out: &mut dyn Write,
    exit_code: u8,
    error: Option<AppError>,
    diagnostics: Option<&GuiDiagnostics>,
) {
    let report = SelfCheckReport {
        mode: SELF_CHECK_MODE,
        exit_code,
        error,
        diagnostics,
    };
    if let Ok(line) = serde_json::to_string(&report) {
        let _ = out.write_all(line.as_bytes());
    }
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}
