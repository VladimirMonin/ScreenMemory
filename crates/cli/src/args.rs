//! Argument parsing only: no IO, no side effect, no implicit default configuration.
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Version,
    Status { no_start: bool },
    Doctor,
    HostStop,
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
}

/// Accepted forms: `--version`; `status [--no-start] [--json]`; `doctor [--json]`;
/// `host stop [--json]`; optional `--config <absolute local path>` before the command.
pub fn parse_args(argv: &[String]) -> Result<CliArgs, ArgError> {
    let mut config: Option<PathBuf> = None;
    let mut json = false;
    let mut no_start = false;
    let mut command: Option<Command> = None;
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
    };
    Ok(CliArgs {
        config,
        json,
        command,
    })
}

/// Absolute `X:\...` form only; the resolver re-validates volume, reparse and layout.
fn absolute_local(path: &std::path::Path) -> bool {
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
