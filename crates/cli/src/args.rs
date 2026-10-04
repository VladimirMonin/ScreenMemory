//! Argument parsing only: no IO, no side effect, no implicit default configuration.
use sm_protocol::application::CaptureMode;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Version,
    Image(crate::image_args::ImageCommand),
    Processing(crate::processing::ProcessingCommand),
    Knowledge(crate::knowledge::KnowledgeCommand),
    Search(crate::search::SearchCommand),
    Status {
        no_start: bool,
    },
    Doctor,
    HostStop,
    /// S02 §11.4: one capture command.
    Capture {
        mode: CaptureMode,
        /// Explicit operation identity; the trusted CLI mints one when it is omitted.
        operation_id: Option<String>,
        /// Optional absolute local destination of a copy of the canonical file.
        output: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliArgs {
    pub config: Option<PathBuf>,
    pub json: bool,
    pub command: Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgError {
    UnknownArgument,
    MissingCommand,
    MissingConfig,
    ConfigNotAbsolute,
    UnexpectedArgument,
    UnsupportedCombination,
    MissingCaptureMode,
    UnknownCaptureMode,
    OutputNotAbsolute,
}

/// Closed mode vocabulary of `shot capture`; `all` is the documented alias of `desktop`.
pub fn parse_capture_mode(raw: &str) -> Result<CaptureMode, ArgError> {
    match raw {
        "region" => Ok(CaptureMode::Region),
        "window" => Ok(CaptureMode::Window),
        "monitor" => Ok(CaptureMode::Monitor),
        "desktop" | "all" => Ok(CaptureMode::Desktop),
        _ => Err(ArgError::UnknownCaptureMode),
    }
}

/// Accepted forms: `--version`; `status [--no-start] [--json]`; `doctor [--json]`;
/// `host stop [--json]`; `capture --mode region|window|monitor|all [--operation-id <uuid>]
/// [--output <absolute local path>] [--json]`; optional `--config <absolute local path>`.
pub fn parse_args(argv: &[String]) -> Result<CliArgs, ArgError> {
    // Select the first command token, never an option's value. The selected
    // parser still owns validation of flags, duplicates and missing values.
    let mut tokens = argv.iter();
    while let Some(token) = tokens.next() {
        match token.as_str() {
            "--json" | "--no-start" => continue,
            flag if flag.starts_with("--") && flag != "--version" => {
                tokens.next();
                continue;
            }
            "search" => return crate::search::parse(argv),
            "knowledge" => return crate::knowledge::parse(argv),
            "processing" => return crate::processing::parse(argv),
            "import" | "export" | "metadata" => return crate::image_args::parse(argv),
            _ => break,
        }
    }
    let mut config: Option<PathBuf> = None;
    let mut json = false;
    let mut no_start = false;
    let mut command: Option<Command> = None;
    let mut capture_mode: Option<CaptureMode> = None;
    let mut operation_id: Option<String> = None;
    let mut output: Option<PathBuf> = None;
    let mut index = 0;
    while index < argv.len() {
        let token = argv[index].as_str();
        match token {
            "--config" => {
                if config.is_some() {
                    return Err(ArgError::UnknownArgument);
                }
                let value = argv.get(index + 1).ok_or(ArgError::MissingConfig)?;
                let path = PathBuf::from(value);
                if !absolute_local(&path) {
                    return Err(ArgError::ConfigNotAbsolute);
                }
                config = Some(path);
                index += 1;
            }
            "--json" => json = true,
            "--no-start" => no_start = true,
            "--version" => {
                command = match command {
                    Some(Command::Version) | None => Some(Command::Version),
                    Some(_) => return Err(ArgError::UnexpectedArgument),
                };
            }
            "status" => {
                if command.is_some() {
                    return Err(ArgError::UnexpectedArgument);
                }
                command = Some(Command::Status { no_start: false });
            }
            "doctor" => {
                if command.is_some() {
                    return Err(ArgError::UnexpectedArgument);
                }
                command = Some(Command::Doctor);
            }
            "capture" => {
                if command.is_some() {
                    return Err(ArgError::UnexpectedArgument);
                }
                command = Some(Command::Capture {
                    // Filled in after the whole command line was read.
                    mode: CaptureMode::Region,
                    operation_id: None,
                    output: None,
                });
            }
            "--mode" => {
                if capture_mode.is_some() {
                    return Err(ArgError::UnexpectedArgument);
                }
                let value = argv.get(index + 1).ok_or(ArgError::MissingCaptureMode)?;
                capture_mode = Some(parse_capture_mode(value)?);
                index += 1;
            }
            "--operation-id" => {
                if operation_id.is_some() {
                    return Err(ArgError::UnexpectedArgument);
                }
                let value = argv.get(index + 1).ok_or(ArgError::MissingCaptureMode)?;
                operation_id = Some(value.clone());
                index += 1;
            }
            "--output" => {
                if output.is_some() {
                    return Err(ArgError::UnexpectedArgument);
                }
                let value = argv.get(index + 1).ok_or(ArgError::OutputNotAbsolute)?;
                let path = PathBuf::from(value);
                if !absolute_local(&path) {
                    return Err(ArgError::OutputNotAbsolute);
                }
                output = Some(path);
                index += 1;
            }
            "host" => {
                if command.is_some() || argv.get(index + 1).map(String::as_str) != Some("stop") {
                    return Err(ArgError::UnknownArgument);
                }
                command = Some(Command::HostStop);
                index += 1;
            }
            _ => return Err(ArgError::UnknownArgument),
        }
        index += 1;
    }
    let command = command.ok_or(ArgError::MissingCommand)?;
    let command = match command {
        Command::Status { .. } => {
            if config.is_none() {
                return Err(ArgError::MissingConfig);
            }
            Command::Status { no_start }
        }
        Command::Search(_) | Command::Knowledge(_) | Command::Processing(_) | Command::Image(_) => {
            unreachable!("image parser owns these forms")
        }
        Command::Version => {
            if json || no_start || config.is_some() {
                return Err(ArgError::UnsupportedCombination);
            }
            Command::Version
        }
        Command::Doctor | Command::HostStop => {
            if no_start {
                return Err(ArgError::UnsupportedCombination);
            }
            if config.is_none() {
                return Err(ArgError::MissingConfig);
            }
            command
        }
        Command::Capture { .. } => {
            if no_start {
                return Err(ArgError::UnsupportedCombination);
            }
            if config.is_none() {
                return Err(ArgError::MissingConfig);
            }
            let mode = capture_mode.ok_or(ArgError::MissingCaptureMode)?;
            if operation_id
                .as_deref()
                .is_some_and(|value| uuid::Uuid::parse_str(value).is_err())
            {
                return Err(ArgError::UnknownArgument);
            }
            Command::Capture {
                mode,
                operation_id,
                output,
            }
        }
    };
    Ok(CliArgs {
        config,
        json,
        command,
    })
}

/// Absolute `X:\...` form only; the resolver re-validates volume, reparse and layout.
pub(crate) fn absolute_local(path: &std::path::Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with(r"\\") || text.starts_with("//") {
        return false;
    }
    if !text.as_bytes().get(1).is_some_and(|c| *c == b':') {
        return false;
    }
    !text.chars().skip(2).any(|c| c == ':')
}
