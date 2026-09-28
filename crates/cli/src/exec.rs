//! Command execution: bounded discovery, one explicit autostart, typed results only.
use crate::{
    args::{self, Command},
    config::{self, ConfigError, PreviewConfig, ResolvedTarget, TargetError},
    launcher::HostLauncher,
    output, RETRY_INTERVAL, START_DEADLINE,
};
use sm_application::credentials::{CredentialEnvironment, CredentialKey, SecretStore};
use sm_framing::BufferBudget;
use sm_protocol::{
    application::{CheckOutcome, DoctorReport, HostState, APPLICATION_VERSION},
    errors::{AppError, ErrorCode},
};
use sm_session::{ApplicationClient, ClientError, CredentialSource, SessionError};
use std::{io::Write, sync::Arc, time::Instant};

/// Explicit dependencies of one CLI invocation; production wires the native vault and
/// the fixed process launcher, tests inject the same seams without touching live data.
pub struct CliDeps {
    pub store: Arc<dyn SecretStore + Send + Sync>,
    pub launcher: Arc<dyn HostLauncher>,
}

struct Failure {
    exit: u8,
    code: ErrorCode,
    message: &'static str,
    retryable: bool,
}

/// Stable exit code per protocol error code; never a free-form string.
pub fn exit_for_code(code: ErrorCode) -> u8 {
    match code {
        ErrorCode::PermissionDenied
        | ErrorCode::CredentialStoreUnavailable
        | ErrorCode::AiDisabled => crate::EXIT_POLICY,
        ErrorCode::WaitTimeout => crate::EXIT_WAIT,
        ErrorCode::SchemaIncompatible
        | ErrorCode::MethodNotSupported
        | ErrorCode::InvalidRequest
        | ErrorCode::FrameTooLarge
        | ErrorCode::ProtocolTruncatedFrame => crate::EXIT_STORAGE,
        ErrorCode::LibraryMismatch
        | ErrorCode::ProtocolMismatch
        | ErrorCode::HostAlreadyRunning
        | ErrorCode::RuntimeUnavailable
        | ErrorCode::ModelNotInstalled
        | ErrorCode::ModelConfigUnsupported
        | ErrorCode::BridgeUnavailable => crate::EXIT_HOST,
    }
}

/// A doctor report carrying a failed mandatory local check exits non-zero; a report whose
/// only deviations are `NOT_RUN`/`UNSUPPORTED` optional capabilities stays successful.
pub fn doctor_exit_code(report: &DoctorReport) -> u8 {
    let mut exit = crate::EXIT_OK;
    for check in &report.checks {
        if matches!(check.outcome, CheckOutcome::Failed) {
            let code = check.safe_code.unwrap_or(ErrorCode::RuntimeUnavailable);
            let mapped = exit_for_code(code);
            exit = exit.max(if mapped == crate::EXIT_HOST {
                crate::EXIT_STORAGE
            } else {
                mapped
            });
        }
    }
    exit
}

fn session_failure(error: SessionError) -> Failure {
    match error {
        SessionError::CredentialUnavailable => Failure {
            exit: crate::EXIT_POLICY,
            code: ErrorCode::CredentialStoreUnavailable,
            message: "The owner credential of this library is not available.",
            retryable: false,
        },
        SessionError::PermissionDenied => Failure {
            exit: crate::EXIT_POLICY,
            code: ErrorCode::PermissionDenied,
            message: "The host refused the supplied credential.",
            retryable: false,
        },
        SessionError::LibraryMismatch => Failure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::LibraryMismatch,
            message: "The configured library identity does not match the host.",
            retryable: false,
        },
        SessionError::ProtocolMismatch | SessionError::CapabilityMismatch => Failure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::ProtocolMismatch,
            message: "The host does not speak the expected application protocol.",
            retryable: false,
        },
        _ => Failure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::RuntimeUnavailable,
            message: "Host is not available.",
            retryable: true,
        },
    }
}

fn client_failure(error: ClientError) -> Failure {
    match error {
        ClientError::Session(error) => session_failure(error),
        ClientError::Application(error) => application_failure(error),
    }
}

fn application_failure(error: AppError) -> Failure {
    Failure {
        exit: exit_for_code(error.code),
        code: error.code,
        message: "The host rejected the request.",
        retryable: false,
    }
}

async fn open_client(
    config: &PreviewConfig,
    target: &ResolvedTarget,
    store: &Arc<dyn SecretStore + Send + Sync>,
) -> Result<ApplicationClient, SessionError> {
    let source = CredentialSource::Store {
        store: store.clone(),
        key: CredentialKey::owner(CredentialEnvironment::Test, config.library_id),
    };
    ApplicationClient::connect(
        &target.endpoint,
        &target.image,
        config.library_id,
        None,
        &source,
        BufferBudget::new(),
    )
    .await
}

/// Returns the process exit code. stdout receives at most one JSON value.
pub async fn run(argv: &[String], deps: &CliDeps, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let args = match args::parse_args(argv) {
        Ok(args) => args,
        Err(_) => return usage_failure(wants_json(argv), out, err),
    };
    if args.command == Command::Version {
        let line = format!(
            "shot {} (protocol {}.{})",
            env!("CARGO_PKG_VERSION"),
            APPLICATION_VERSION.major,
            APPLICATION_VERSION.minor
        );
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
        return crate::EXIT_OK;
    }
    let config_path = args
        .config
        .as_ref()
        .expect("machine commands require --config");
    let config = match config::load_config(config_path) {
        Ok(config) => config,
        Err(error) => return config_failure(error, args.json, out, err),
    };
    let target = match config::resolve_target(&config) {
        Ok(target) => target,
        Err(error) => return target_failure(error, args.json, out, err),
    };
    // Only the two stop-style commands are forbidden from starting a host.
    let no_start = matches!(args.command, Command::HostStop)
        || matches!(args.command, Command::Status { no_start: true });
    let client = match discover_client(&config, &target, deps, no_start).await {
        Ok(client) => client,
        Err(failure) => return emit_failure(failure, args.json, out, err),
    };
    let exit = match args.command {
        Command::Status { .. } => match client.status().await {
            Ok(status) => {
                if args.json {
                    output::write_json(out, &status);
                } else {
                    output::write_status_human(out, &status);
                }
                crate::EXIT_OK
            }
            Err(error) => return emit_failure(client_failure(error), args.json, out, err),
        },
        Command::Doctor => match client.doctor().await {
            Ok(report) => {
                let exit = doctor_exit_code(&report);
                if args.json {
                    output::write_json(out, &report);
                } else {
                    output::write_doctor_human(out, &report);
                }
                exit
            }
            Err(error) => return emit_failure(client_failure(error), args.json, out, err),
        },
        Command::HostStop => match client.request_shutdown().await {
            // The receipt means the stop request was accepted, not that handles closed.
            Ok(receipt) if matches!(receipt.state, HostState::Stopping) => {
                if args.json {
                    output::write_json(out, &receipt);
                } else {
                    output::write_stop_human(out, &receipt);
                }
                crate::EXIT_OK
            }
            Ok(_) => {
                return emit_failure(
                    Failure {
                        exit: crate::EXIT_HOST,
                        code: ErrorCode::RuntimeUnavailable,
                        message: "The host did not accept the stop request.",
                        retryable: false,
                    },
                    args.json,
                    out,
                    err,
                )
            }
            Err(error) => return emit_failure(client_failure(error), args.json, out, err),
        },
        Command::Version => crate::EXIT_OK,
    };
    // Dropping the client closes only this connection; the host keeps running.
    drop(client);
    exit
}

/// No-start commands perform exactly one attempt; autostart commands may start the fixed
/// trusted host image once and then retry within the bounded discovery deadline.
async fn discover_client(
    config: &PreviewConfig,
    target: &ResolvedTarget,
    deps: &CliDeps,
    no_start: bool,
) -> Result<ApplicationClient, Failure> {
    let store = deps.store.clone();
    match open_client(config, target, &store).await {
        Ok(client) => return Ok(client),
        Err(error) => {
            let failure = session_failure(error);
            if no_start || !failure.retryable {
                return Err(failure);
            }
        }
    }
    let deadline = Instant::now() + START_DEADLINE;
    let mut child = match deps.launcher.launch(&config.host_executable, &config.root) {
        Ok(child) => child,
        Err(_) => {
            return Err(Failure {
                exit: crate::EXIT_HOST,
                code: ErrorCode::RuntimeUnavailable,
                message: "The fixed host executable could not be started.",
                retryable: false,
            })
        }
    };
    loop {
        match open_client(config, target, &store).await {
            Ok(client) => {
                child.mark_established();
                return Ok(client);
            }
            Err(error) => {
                let failure = session_failure(error);
                if !failure.retryable {
                    return Err(failure);
                }
            }
        }
        if Instant::now() >= deadline {
            let exited = child.has_exited();
            // The guard terminates only this owned, never-reached child handle.
            drop(child);
            return Err(if exited {
                Failure {
                    exit: crate::EXIT_HOST,
                    code: ErrorCode::RuntimeUnavailable,
                    message: "The host did not become available.",
                    retryable: false,
                }
            } else {
                Failure {
                    exit: crate::EXIT_WAIT,
                    code: ErrorCode::WaitTimeout,
                    message: "The host did not become ready in time.",
                    retryable: false,
                }
            });
        }
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}

fn wants_json(argv: &[String]) -> bool {
    argv.iter().any(|token| token == "--json")
}

fn usage_failure(json: bool, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let failure = Failure {
        exit: crate::EXIT_USAGE,
        code: ErrorCode::InvalidRequest,
        message: "Invalid arguments. Expected --config <path> status|doctor|host stop [--json].",
        retryable: false,
    };
    emit_failure(failure, json, out, err)
}

fn config_failure(error: ConfigError, json: bool, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let message = match error {
        ConfigError::Missing => "The configuration file does not exist.",
        ConfigError::NotAbsolute => "Configuration paths must be absolute local paths.",
        ConfigError::NotRegularFile => "The configuration path is not a regular file.",
        ConfigError::Invalid => "The configuration file is not valid trusted configuration.",
        ConfigError::UnsupportedVersion => "The configuration schema version is not supported.",
        ConfigError::UnsupportedEnvironment => {
            "Only an explicit test namespace configuration is supported."
        }
    };
    emit_failure(
        Failure {
            exit: crate::EXIT_USAGE,
            code: ErrorCode::InvalidRequest,
            message,
            retryable: false,
        },
        json,
        out,
        err,
    )
}

fn target_failure(error: TargetError, json: bool, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let failure = match error {
        TargetError::Root => Failure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::RuntimeUnavailable,
            message: "The configured library root is not available.",
            retryable: false,
        },
        TargetError::Endpoint => Failure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::RuntimeUnavailable,
            message: "The configured library endpoint could not be derived.",
            retryable: false,
        },
        TargetError::Image => Failure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::RuntimeUnavailable,
            message: "The configured host executable is not a trusted local image.",
            retryable: false,
        },
    };
    emit_failure(failure, json, out, err)
}

fn emit_failure(failure: Failure, json: bool, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    if json {
        let error = output::safe_error(failure.code, failure.message);
        output::write_error_json(out, &error);
    } else {
        output::write_error_human(err, failure.code, failure.message);
    }
    failure.exit
}
