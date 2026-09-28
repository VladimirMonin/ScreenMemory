//! Explicit arguments and trusted configuration of the diagnostic window.
//!
//! The window accepts exactly one absolute local `--config` file with the same
//! dev-preview schema the CLI uses (`schema_version`, `environment = test`, absolute
//! `root`, canonical `library_id`, absolute `host_executable`). There is no default
//! path and no CWD/HOME/PATH/ENV fallback: without a valid explicit configuration the
//! shell refuses with a stable exit code and never guesses a root.
use sm_cli::{load_config, ConfigError, PreviewConfig};
use std::path::{Path, PathBuf};

/// Schema of the explicit trusted configuration; shared with the CLI dev-preview flow.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuiArgs {
    pub config: PathBuf,
    pub allow_start: bool,
    pub self_check: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiArgError {
    UnknownArgument,
    MissingConfig,
    ConfigNotAbsolute,
    UnexpectedArgument,
    UnsupportedCombination,
}

/// Accepted forms: `--config <absolute local path> [--no-start]` for the window and
/// `--self-check --config <absolute local path> [--no-start] [--json]` for the headless
/// check. `--json` only selects machine output, which `--self-check` already produces.
pub fn parse_gui_args(argv: &[String]) -> Result<GuiArgs, GuiArgError> {
    let mut config: Option<PathBuf> = None;
    let mut self_check = false;
    let mut allow_start = true;
    let mut json = false;
    let mut index = 0;
    while index < argv.len() {
        match argv[index].as_str() {
            "--config" => {
                if config.is_some() {
                    return Err(GuiArgError::UnexpectedArgument);
                }
                let value = argv.get(index + 1).ok_or(GuiArgError::MissingConfig)?;
                let path = PathBuf::from(value);
                if !absolute_local(&path) {
                    return Err(GuiArgError::ConfigNotAbsolute);
                }
                config = Some(path);
                index += 1;
            }
            "--self-check" => {
                if self_check {
                    return Err(GuiArgError::UnexpectedArgument);
                }
                self_check = true;
            }
            "--no-start" => {
                if !allow_start {
                    return Err(GuiArgError::UnexpectedArgument);
                }
                allow_start = false;
            }
            "--json" => {
                if json {
                    return Err(GuiArgError::UnexpectedArgument);
                }
                json = true;
            }
            _ => return Err(GuiArgError::UnknownArgument),
        }
        index += 1;
    }
    let config = config.ok_or(GuiArgError::MissingConfig)?;
    if json && !self_check {
        return Err(GuiArgError::UnsupportedCombination);
    }
    Ok(GuiArgs {
        config,
        allow_start,
        self_check,
    })
}

/// Load and validate the explicit trusted configuration; no implicit default.
pub fn load_window_config(path: &Path) -> Result<PreviewConfig, ConfigError> {
    load_config(path)
}

/// Absolute `X:\...` form only; UNC, device and ADS spellings are refused before any
/// file system access, and `load_config` re-validates the whole document.
fn absolute_local(path: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_string()).collect()
    }

    #[test]
    fn t06d_gui_01_arguments_require_one_explicit_absolute_config() {
        let parsed = parse_gui_args(&argv(&["--config", r"C:\owned\preview.json"])).unwrap();
        assert_eq!(parsed.config, PathBuf::from(r"C:\owned\preview.json"));
        assert!(parsed.allow_start);
        assert!(!parsed.self_check);
        assert_eq!(
            parse_gui_args(&argv(&["--config", r"C:\owned\preview.json", "--no-start"])).unwrap(),
            GuiArgs {
                config: PathBuf::from(r"C:\owned\preview.json"),
                allow_start: false,
                self_check: false,
            }
        );
        let check = parse_gui_args(&argv(&[
            "--self-check",
            "--config",
            r"C:\owned\preview.json",
            "--json",
        ]))
        .unwrap();
        assert!(check.self_check);
    }

    #[test]
    fn t06d_gui_02_arguments_refuse_every_implicit_or_foreign_form() {
        for tokens in [
            vec![],
            vec!["--config"],
            vec!["--config", "relative.json"],
            vec!["--config", r"\\server\share\preview.json"],
            vec!["--config", r"\\?\C:\owned\preview.json"],
            vec!["--config", r"C:\owned\preview.json:stream"],
            vec!["--config", r"C:\owned\preview.json", "--bogus"],
            vec![
                "--config",
                r"C:\owned\preview.json",
                "--config",
                r"C:\owned\preview.json",
            ],
            vec!["--config", r"C:\owned\preview.json", "--json"],
        ] {
            assert!(
                parse_gui_args(&argv(&tokens)).is_err(),
                "accepted {tokens:?}"
            );
        }
    }
}
