//! The "open the diagnostic report" action.
//!
//! The command takes **no path**: the directory is derived from the already validated
//! configuration, re-proven through the same namespace resolver the host uses, and only
//! then handed to the shell. A foreign, linked or absent directory is refused with a
//! stable safe code instead of opening anything, so no arbitrary path can be reached
//! from the window.
use sm_cli::PreviewConfig;
use sm_isolation::{LibraryRootResolver, RootSource};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Leaf the namespace resolver opens for the cache/log tree of the owned root.
const CACHE_LEAF: &str = "cache";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenReportError {
    /// The configured root is not a usable owned namespace.
    Root,
    /// The cache/log directory of the root is absent.
    Missing,
    /// The directory (or its marker chain) is a link/reparse point.
    Linked,
    /// The leaf exists but is not a directory.
    NotADirectory,
    /// No trusted local shell could be started for the validated directory.
    Shell,
}

/// Opens exactly one validated directory. Production uses the Windows shell; tests
/// inject a recorder, so no window or Explorer process is ever started by a test.
pub trait ReportOpener: Send + Sync {
    fn open(&self, directory: &Path) -> Result<(), OpenReportError>;
}

/// The validated cache/log directory of the configured owned root: this is where the
/// host writes its bounded operational log and its pre-root stderr diagnostics.
pub fn validated_report_directory(config: &PreviewConfig) -> Result<PathBuf, OpenReportError> {
    let donor: [&Path; 0] = [];
    // Resolving the namespace proves the root exists, rejects traversal/link/overlap
    // spellings and fails closed when the root is absent; the environment must match the
    // explicit configuration instead of being remapped.
    let layout = LibraryRootResolver::resolve(RootSource::Test {
        root: &config.root,
        forbidden_donor_roots: &donor,
    })
    .map_err(|_| OpenReportError::Root)?;
    if layout.environment != config.environment {
        return Err(OpenReportError::Root);
    }
    let directory = config.root.join(CACHE_LEAF);
    let metadata = fs::symlink_metadata(&directory).map_err(|_| OpenReportError::Missing)?;
    if metadata.file_type().is_symlink() || is_reparse(&metadata) {
        return Err(OpenReportError::Linked);
    }
    if !metadata.is_dir() {
        return Err(OpenReportError::NotADirectory);
    }
    Ok(directory)
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

/// Stable protocol code of one refusal; never a path or OS string.
pub fn safe_code(error: OpenReportError) -> sm_protocol::errors::ErrorCode {
    use sm_protocol::errors::ErrorCode;
    match error {
        OpenReportError::Root | OpenReportError::Linked => ErrorCode::PermissionDenied,
        OpenReportError::Missing | OpenReportError::NotADirectory | OpenReportError::Shell => {
            ErrorCode::RuntimeUnavailable
        }
    }
}

/// The Windows shell, located from the Windows directory of this machine, never from
/// PATH, CWD or an environment variable, and never a second copy next to this binary.
pub struct SystemReportOpener;

impl ReportOpener for SystemReportOpener {
    fn open(&self, directory: &Path) -> Result<(), OpenReportError> {
        if !directory.is_absolute() {
            return Err(OpenReportError::Root);
        }
        let shell = system_shell()?;
        let child = Command::new(shell)
            .arg(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| OpenReportError::Shell)?;
        drop(child);
        Ok(())
    }
}

fn system_shell() -> Result<PathBuf, OpenReportError> {
    let mut buffer = [0u16; 260];
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    };
    if length == 0 || length as usize >= buffer.len() {
        return Err(OpenReportError::Shell);
    }
    let text =
        String::from_utf16(&buffer[..length as usize]).map_err(|_| OpenReportError::Shell)?;
    let candidate = Path::new(&text).join("explorer.exe");
    let metadata = fs::symlink_metadata(&candidate).map_err(|_| OpenReportError::Shell)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || is_reparse(&metadata) {
        return Err(OpenReportError::Shell);
    }
    Ok(candidate)
}
