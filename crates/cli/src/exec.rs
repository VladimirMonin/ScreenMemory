//! Command execution: bounded discovery, one explicit autostart, typed results only.
use crate::{
    args::{self, Command},
    config::{self, ConfigError, PreviewConfig, ResolvedTarget, TargetError},
    launcher::HostLauncher,
    output, RETRY_INTERVAL, START_DEADLINE,
};
use sm_application::credentials::{CredentialEnvironment, CredentialKey, SecretStore};
use sm_domain::OperationId;
use sm_framing::BufferBudget;
use sm_protocol::{
    application::{
        CaptureMode, CaptureOutcome, CheckOutcome, DoctorReport, HostState, LibraryAssetRequest,
        APPLICATION_VERSION,
    },
    errors::{AppError, ErrorCode},
};
use sm_session::{ApplicationClient, ClientError, CredentialSource, SessionError};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use uuid::Uuid;

/// Explicit dependencies of one client invocation; production wires the native vault and
/// the fixed process launcher, tests inject the same seams without touching live data.
/// Shared by `shot` and the GUI shell, so both integrations autostart the same fixed
/// trusted image through exactly one implementation.
#[derive(Clone)]
pub struct CliDeps {
    pub store: Arc<dyn SecretStore + Send + Sync>,
    pub launcher: Arc<dyn HostLauncher>,
}

/// Typed outcome of one client operation: a stable command exit code, the protocol
/// error code and a bounded safe message. Shared by `shot` and the private GUI shell so
/// both integrations report the same codes for the same failure; never a free-form string.
#[derive(Debug)]
pub struct ClientFailure {
    pub exit: u8,
    pub code: ErrorCode,
    pub message: &'static str,
    pub retryable: bool,
}

/// Stable exit code per protocol error code; never a free-form string.
pub fn exit_for_code(code: ErrorCode) -> u8 {
    match code {
        ErrorCode::PermissionDenied
        | ErrorCode::CredentialStoreUnavailable
        | ErrorCode::AiDisabled => crate::EXIT_POLICY,
        ErrorCode::WaitTimeout => crate::EXIT_WAIT,
        ErrorCode::RevisionConflict => crate::EXIT_CONFLICT,
        // S02 §11.5: a known-state lookup failure is code 3, a limit that the caller asked
        // for is code 2, and a capture/GDI/save/integrity failure is code 7.
        ErrorCode::AssetNotFound => crate::EXIT_NOT_FOUND,
        ErrorCode::ImageLimitExceeded
        | ErrorCode::CanonicalSizeUnsupported
        | ErrorCode::ImageInputInvalid => crate::EXIT_USAGE,
        ErrorCode::CodecFailed
        | ErrorCode::AssetIntegrityFailed
        | ErrorCode::StorageFull
        | ErrorCode::CaptureBusy
        | ErrorCode::CaptureBackpressure
        | ErrorCode::CaptureTargetLost
        | ErrorCode::CaptureTopologyChanged
        | ErrorCode::CaptureFocusUnavailable
        | ErrorCode::CaptureUnavailable
        | ErrorCode::CaptureGdiFailed => crate::EXIT_CAPTURE,
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

pub fn session_failure(error: SessionError) -> ClientFailure {
    match error {
        SessionError::CredentialUnavailable => ClientFailure {
            exit: crate::EXIT_POLICY,
            code: ErrorCode::CredentialStoreUnavailable,
            message: "The owner credential of this library is not available.",
            retryable: false,
        },
        SessionError::PermissionDenied => ClientFailure {
            exit: crate::EXIT_POLICY,
            code: ErrorCode::PermissionDenied,
            message: "The host refused the supplied credential.",
            retryable: false,
        },
        SessionError::LibraryMismatch => ClientFailure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::LibraryMismatch,
            message: "The configured library identity does not match the host.",
            retryable: false,
        },
        SessionError::ProtocolMismatch | SessionError::CapabilityMismatch => ClientFailure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::ProtocolMismatch,
            message: "The host does not speak the expected application protocol.",
            retryable: false,
        },
        _ => ClientFailure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::RuntimeUnavailable,
            message: "Host is not available.",
            retryable: true,
        },
    }
}

pub fn client_failure(error: ClientError) -> ClientFailure {
    match error {
        ClientError::Session(error) => session_failure(error),
        ClientError::Application(error) => application_failure(error),
    }
}

fn application_failure(error: AppError) -> ClientFailure {
    ClientFailure {
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
        Err(error) => return emit_failure(target_failure(error), args.json, out, err),
    };
    // Only the two stop-style commands are forbidden from starting a host.
    let no_start = matches!(args.command, Command::HostStop)
        || matches!(args.command, Command::Status { no_start: true });
    let client = match connect_session(&config, &target, deps, no_start).await {
        Ok(client) => client,
        Err(failure) => return emit_failure(failure, args.json, out, err),
    };
    let exit = match args.command {
        Command::Processing(command) => match crate::processing::run(&client, command).await {
            Ok(reply) => {
                output::write_json(out, &reply);
                crate::EXIT_OK
            }
            Err(failure) => return emit_failure(failure, true, out, err),
        },
        Command::Image(command) => {
            let (exit, value) = crate::images::run(&client, command).await;
            output::write_json(out, &value);
            if exit != 0 {
                let _ = writeln!(err, "Image operation incomplete or rejected.");
            }
            exit
        }
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
                    ClientFailure {
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
        Command::Capture {
            mode,
            operation_id,
            output,
        } => {
            return run_capture(
                &config,
                &client,
                mode,
                operation_id.as_deref(),
                output.as_deref(),
                args.json,
                out,
                err,
            )
            .await;
        }
        Command::Version => crate::EXIT_OK,
    };
    // Dropping the client closes only this connection; the host keeps running.
    drop(client);
    exit
}

/// No-start commands perform exactly one attempt; autostart commands may start the fixed
/// trusted host image once and then retry within the bounded discovery deadline.
pub async fn connect_session(
    config: &PreviewConfig,
    target: &ResolvedTarget,
    deps: &CliDeps,
    no_start: bool,
) -> Result<ApplicationClient, ClientFailure> {
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
            return Err(ClientFailure {
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
                ClientFailure {
                    exit: crate::EXIT_HOST,
                    code: ErrorCode::RuntimeUnavailable,
                    message: "The host did not become available.",
                    retryable: false,
                }
            } else {
                ClientFailure {
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
    let failure = ClientFailure {
        exit: crate::EXIT_USAGE,
        code: ErrorCode::InvalidRequest,
        message: "Invalid arguments. Expected --config <path> status|doctor|host stop [--json].",
        retryable: false,
    };
    emit_failure(failure, json, out, err)
}

/// Closed safe message of one configuration refusal; shared so the GUI shell refuses an
/// invalid explicit configuration with exactly the same wording as `shot`.
pub fn config_message(error: ConfigError) -> &'static str {
    match error {
        ConfigError::Missing => "The configuration file does not exist.",
        ConfigError::NotAbsolute => "Configuration paths must be absolute local paths.",
        ConfigError::NotRegularFile => "The configuration path is not a regular file.",
        ConfigError::Invalid => "The configuration file is not valid trusted configuration.",
        ConfigError::UnsupportedVersion => "The configuration schema version is not supported.",
        ConfigError::UnsupportedEnvironment => {
            "Only an explicit test namespace configuration is supported."
        }
    }
}

fn config_failure(error: ConfigError, json: bool, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    emit_failure(
        ClientFailure {
            exit: crate::EXIT_USAGE,
            code: ErrorCode::InvalidRequest,
            message: config_message(error),
            retryable: false,
        },
        json,
        out,
        err,
    )
}

/// Typed refusal of one resolved client target; the same codes the GUI shell reports.
pub fn target_failure(error: TargetError) -> ClientFailure {
    match error {
        TargetError::Root => ClientFailure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::RuntimeUnavailable,
            message: "The configured library root is not available.",
            retryable: false,
        },
        TargetError::Endpoint => ClientFailure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::RuntimeUnavailable,
            message: "The configured library endpoint could not be derived.",
            retryable: false,
        },
        TargetError::Image => ClientFailure {
            exit: crate::EXIT_HOST,
            code: ErrorCode::RuntimeUnavailable,
            message: "The configured host executable is not a trusted local image.",
            retryable: false,
        },
    }
}

fn emit_failure(
    failure: ClientFailure,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    if json {
        let error = output::safe_error(failure.code, failure.message);
        output::write_error_json(out, &error);
    } else {
        output::write_error_human(err, failure.code, failure.message);
    }
    failure.exit
}

/// Longest client wait of one interactive capture command. The host bounds a single
/// application request to the same value (§11.5: the wait limit is separate from the
/// three-second IPC handshake).
pub const CAPTURE_WAIT_MS: u32 = 30_000;
/// Bounded copy buffer: one canonical asset chunk at a time.
const OUTPUT_CHUNK_LIMIT: usize = 512 * 1024;

/// Refusal of an output destination that would land inside the owned library root.
///
/// A lexicographic prefix test of the raw argument is not enough: a spelling such as
/// `<sibling>/../<root>/copy.webp` does not start with the root textually yet resolves
/// inside it, and a differently spelled or linked spelling of the root resolves inside it
/// as well. The destination is therefore judged twice: first lexically after normalizing
/// `.`/`..` components, then physically by comparing the canonical path of the directory
/// the copy would be created in against the canonical root.
fn destination_refusal(root: &Path, destination: &Path) -> Option<ClientFailure> {
    if destination_inside_root(root, destination) {
        Some(ClientFailure {
            exit: crate::EXIT_USAGE,
            code: ErrorCode::InvalidRequest,
            message: "The output destination must be outside the library root.",
            retryable: false,
        })
    } else {
        None
    }
}

/// Lexically normalized form of one path: `.` disappears and `..` pops a known component.
/// A `..` that cannot be resolved (a relative path already at its head) stays a real
/// component, so it can neither climb above the path nor pretend to be outside the root.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Physical path of the destination that `create_new` would really use: an existing
/// destination is resolved through its own (possibly reparse) path, a new file through the
/// parent directory it must be created in.
fn resolved_destination(destination: &Path) -> Option<PathBuf> {
    let probe = if destination.exists() {
        destination
    } else {
        destination.parent()?
    };
    probe.canonicalize().ok()
}

fn destination_inside_root(root: &Path, destination: &Path) -> bool {
    if normalize_lexically(destination).starts_with(normalize_lexically(root)) {
        return true;
    }
    // Both sides are canonicalized, so two spellings of one directory (case, reparse,
    // short name) compare equal on every Windows filesystem.
    let (Ok(root_real), Some(destination_real)) =
        (root.canonicalize(), resolved_destination(destination))
    else {
        // Without a resolvable root/parent the later `create_new` fails on its own and can
        // publish nothing inside the library.
        return false;
    };
    // A prefix match is compared component-wise, so `<root>-other` is not inside `<root>`.
    destination_real.starts_with(root_real)
}

/// One `capture` command: request, optional canonical copy, then one JSON document.
#[allow(clippy::too_many_arguments)]
async fn run_capture(
    config: &PreviewConfig,
    client: &ApplicationClient,
    mode: CaptureMode,
    operation_id: Option<&str>,
    output: Option<&std::path::Path>,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let operation = match operation_id {
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(id) => OperationId::new(id),
            Err(_) => {
                return emit_failure(
                    ClientFailure {
                        exit: crate::EXIT_USAGE,
                        code: ErrorCode::InvalidRequest,
                        message: "The operation identity is not a canonical UUID.",
                        retryable: false,
                    },
                    json,
                    out,
                    err,
                )
            }
        },
        // The trusted CLI mints a real operation identity; no ordinal is invented.
        None => OperationId::new(Uuid::new_v4()),
    };
    // A destination inside the owned library root is refused: the CLI never writes into the
    // library and never turns an output copy into a second canonical file.
    if let Some(path) = output {
        if let Some(failure) = destination_refusal(&config.root, path) {
            return emit_failure(failure, json, out, err);
        }
    }
    let params = sm_protocol::application::CaptureCommandParams {
        operation_id: operation,
        mode,
    };
    let outcome = match client.capture_command(params, CAPTURE_WAIT_MS, None).await {
        Ok(outcome) => outcome,
        Err(error) => return emit_failure(client_failure(error), json, out, err),
    };
    if let (Some(path), CaptureOutcome::Saved { receipt }) = (output, &outcome) {
        if let Err(failure) = write_canonical_copy(client, receipt, path, &config.root).await {
            return emit_failure(failure, json, out, err);
        }
    }
    if json {
        output::write_json(out, &outcome);
    } else {
        output::write_capture_human(out, &outcome);
    }
    // A normal user cancellation is a successful command (S02 §11.5).
    crate::EXIT_OK
}

/// Copy the committed canonical file to an explicit destination through bounded reads.
async fn write_canonical_copy(
    client: &ApplicationClient,
    receipt: &sm_protocol::application::CaptureReceipt,
    path: &std::path::Path,
    root: &std::path::Path,
) -> Result<(), ClientFailure> {
    // Judged again at the write itself, with the same physical rules: the destination must
    // not resolve into the pinned library root, whatever its spelling.
    if let Some(failure) = destination_refusal(root, path) {
        return Err(failure);
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => {
            return Err(ClientFailure {
                exit: crate::EXIT_USAGE,
                code: ErrorCode::InvalidRequest,
                message: match error.kind() {
                    // An existing destination is never overwritten by a capture copy.
                    std::io::ErrorKind::AlreadyExists => {
                        "The output destination already exists and is never overwritten."
                    }
                    std::io::ErrorKind::PermissionDenied => {
                        "The output destination is not writable."
                    }
                    _ => "The output destination could not be created.",
                },
                retryable: false,
            });
        }
    };
    let mut offset = 0u32;
    loop {
        let request = LibraryAssetRequest {
            asset_id: receipt.asset_id,
            offset,
        };
        let chunk = match client.read_asset(request).await {
            Ok(chunk) => chunk,
            Err(error) => return Err(client_failure(error)),
        };
        let bytes = match decode_hex(&chunk.data_hex) {
            Some(bytes) => bytes,
            None => {
                return Err(ClientFailure {
                    exit: crate::EXIT_CAPTURE,
                    code: ErrorCode::AssetIntegrityFailed,
                    message: "The host returned an unreadable asset chunk.",
                    retryable: false,
                })
            }
        };
        if bytes.len() > OUTPUT_CHUNK_LIMIT {
            return Err(ClientFailure {
                exit: crate::EXIT_CAPTURE,
                code: ErrorCode::AssetIntegrityFailed,
                message: "The host returned an oversized asset chunk.",
                retryable: false,
            });
        }
        if file.write_all(&bytes).is_err() {
            return Err(ClientFailure {
                exit: crate::EXIT_CAPTURE,
                code: ErrorCode::StorageFull,
                message: "The output destination could not be written completely.",
                retryable: false,
            });
        }
        if chunk.eof {
            break;
        }
        if chunk.next_offset <= offset || chunk.next_offset == 0 {
            // A non-advancing transfer would loop forever; it is refused instead.
            return Err(ClientFailure {
                exit: crate::EXIT_CAPTURE,
                code: ErrorCode::AssetIntegrityFailed,
                message: "The asset transfer did not advance.",
                retryable: false,
            });
        }
        offset = chunk.next_offset;
    }
    if file.flush().is_err() {
        return Err(ClientFailure {
            exit: crate::EXIT_CAPTURE,
            code: ErrorCode::StorageFull,
            message: "The output destination could not be flushed.",
            retryable: false,
        });
    }
    Ok(())
}

fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    if !raw.len().is_multiple_of(2) || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = Vec::with_capacity(raw.len() / 2);
    let mut index = 0;
    while index < raw.len() {
        let pair = u8::from_str_radix(&raw[index..index + 2], 16).ok()?;
        bytes.push(pair);
        index += 2;
    }
    Some(bytes)
}
