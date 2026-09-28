//! Bootstrap arguments: parsed before any user side effect, never from CWD/PATH/ENV.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapMode {
    /// Create the secure listener and serve authorized requests until an owner stops it.
    Serve,
    /// First-run library bootstrap and explicit owner enrollment of an already prepared
    /// owned root, then a clean shutdown without binding a listener.
    Initialize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapArgs {
    pub root: PathBuf,
    pub mode: BootstrapMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgError {
    MissingRoot,
    UnsupportedArgument,
    RootNotAbsolute,
}

/// Only `--root <absolute local path>` and the optional `--init` mode are accepted.
pub fn parse_bootstrap_args(argv: &[String]) -> Result<BootstrapArgs, ArgError> {
    let mut root: Option<PathBuf> = None;
    let mut mode = BootstrapMode::Serve;
    let mut index = 0;
    while index < argv.len() {
        match argv[index].as_str() {
            "--root" => {
                if root.is_some() {
                    return Err(ArgError::UnsupportedArgument);
                }
                let value = argv.get(index + 1).ok_or(ArgError::MissingRoot)?;
                root = Some(PathBuf::from(value));
                index += 1;
            }
            "--init" => {
                if mode == BootstrapMode::Initialize {
                    return Err(ArgError::UnsupportedArgument);
                }
                mode = BootstrapMode::Initialize;
            }
            _ => return Err(ArgError::UnsupportedArgument),
        }
        index += 1;
    }
    let root = root.ok_or(ArgError::MissingRoot)?;
    if !absolute_local(&root) {
        return Err(ArgError::RootNotAbsolute);
    }
    Ok(BootstrapArgs { root, mode })
}

/// Cheap shape check only; `sm-isolation` re-validates volume, reparse and overlap.
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
