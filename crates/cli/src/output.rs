//! Output boundary: at most one JSON value on stdout, safe diagnostics on stderr.
use serde::Serialize;
use sm_protocol::{
    application::{DoctorReport, HostStatus, ShutdownReceipt},
    errors::{AppError, ErrorCode},
    values::SafeMessage,
};
use std::io::Write;

pub fn safe_error(code: ErrorCode, message: &'static str) -> AppError {
    AppError {
        code,
        safe_message: SafeMessage::parse(message).expect("static safe message"),
        retryable: false,
    }
}

/// Exactly one JSON value plus a trailing newline: no banner, progress or colour.
pub fn write_json<T: Serialize>(out: &mut dyn Write, value: &T) {
    if let Ok(line) = serde_json::to_string(value) {
        let _ = out.write_all(line.as_bytes());
    }
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

pub fn write_error_json(out: &mut dyn Write, error: &AppError) {
    write_json(out, error);
}

pub fn write_error_human(err: &mut dyn Write, code: ErrorCode, message: &str) {
    let _ = writeln!(err, "shot: {}: {message}", token(&code));
}

pub fn write_status_human(out: &mut dyn Write, status: &HostStatus) {
    let _ = writeln!(out, "state: {}", token(&status.state));
    let _ = writeln!(out, "build: {}", status.build_id.as_str());
    let _ = writeln!(
        out,
        "protocol: {}.{}",
        status.protocol.major, status.protocol.minor
    );
    let _ = writeln!(out, "schema: {}", status.schema_version);
    let _ = writeln!(out, "ai_enabled: {}", status.ai_enabled);
    let _ = writeln!(out, "bridge: {}", token(&status.bridge_state));
    let _ = writeln!(
        out,
        "host_instance_id: {}",
        status.host_instance_id.as_uuid()
    );
    let _ = writeln!(out, "library_id: {}", status.library_id.as_uuid());
    let methods = status
        .available_methods
        .iter()
        .map(|name| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(out, "methods: {methods}");
    let _ = out.flush();
}

pub fn write_doctor_human(out: &mut dyn Write, report: &DoctorReport) {
    let _ = writeln!(
        out,
        "host_instance_id: {}",
        report.host.host_instance_id.as_uuid()
    );
    let _ = writeln!(out, "library_id: {}", report.host.library_id.as_uuid());
    for check in &report.checks {
        let _ = writeln!(
            out,
            "check {}: {}",
            check.check_id.as_str(),
            token(&check.outcome)
        );
    }
    let _ = out.flush();
}

pub fn write_stop_human(out: &mut dyn Write, receipt: &ShutdownReceipt) {
    let _ = writeln!(
        out,
        "host_instance_id: {}",
        receipt.host_instance_id.as_uuid()
    );
    let _ = writeln!(out, "state: {}", token(&receipt.state));
    let _ = out.flush();
}

fn token<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        _ => "unknown".to_string(),
    }
}
