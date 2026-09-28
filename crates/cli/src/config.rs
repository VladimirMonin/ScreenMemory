//! Explicit trusted preview configuration. No default path, no environment fallback.
use serde::Deserialize;
use sm_domain::LibraryId;
use sm_isolation::{Environment, LibraryRootResolver, RootSource};
use sm_pipe::{Endpoint, TrustedServerImage};
use std::path::{Path, PathBuf};

const SUPPORTED_SCHEMA: u32 = 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    schema_version: u32,
    environment: String,
    root: PathBuf,
    library_id: LibraryId,
    host_executable: PathBuf,
}

/// Validated configuration of one owned preview root. `environment` is fixed to the
/// explicit test namespace in this slice; product/dev roots are refused, not remapped.
pub struct PreviewConfig {
    pub root: PathBuf,
    pub library_id: LibraryId,
    pub host_executable: PathBuf,
    pub environment: Environment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing,
    NotAbsolute,
    NotRegularFile,
    Invalid,
    UnsupportedVersion,
    UnsupportedEnvironment,
}

/// Resolved client target: the OS-verified endpoint of the configured root and the fixed
/// trusted server image checked by `sm-pipe` before any credential is read.
pub struct ResolvedTarget {
    pub endpoint: Endpoint,
    pub image: TrustedServerImage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetError {
    Root,
    Endpoint,
    Image,
}

pub fn load_config(path: &Path) -> Result<PreviewConfig, ConfigError> {
    if !path.is_absolute() {
        return Err(ConfigError::NotAbsolute);
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ConfigError::Missing)?;
    if !metadata.is_file() {
        return Err(ConfigError::NotRegularFile);
    }
    if is_link(&metadata) {
        return Err(ConfigError::Invalid);
    }
    let bytes = std::fs::read(path).map_err(|_| ConfigError::Invalid)?;
    let raw: RawConfig = serde_json::from_slice(&bytes).map_err(|_| ConfigError::Invalid)?;
    if raw.schema_version != SUPPORTED_SCHEMA {
        return Err(ConfigError::UnsupportedVersion);
    }
    if raw.environment != "test" {
        return Err(ConfigError::UnsupportedEnvironment);
    }
    if !absolute_local(&raw.root) || !absolute_local(&raw.host_executable) {
        return Err(ConfigError::NotAbsolute);
    }
    Ok(PreviewConfig {
        root: raw.root,
        library_id: raw.library_id,
        host_executable: raw.host_executable,
        environment: Environment::Test,
    })
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

pub fn resolve_target(config: &PreviewConfig) -> Result<ResolvedTarget, TargetError> {
    let donor: [&Path; 0] = [];
    let layout = LibraryRootResolver::resolve(RootSource::Test {
        root: &config.root,
        forbidden_donor_roots: &donor,
    })
    .map_err(|_| TargetError::Root)?;
    if layout.environment != config.environment {
        return Err(TargetError::Root);
    }
    let endpoint = Endpoint::for_layout(&layout).map_err(|_| TargetError::Endpoint)?;
    let image = TrustedServerImage::from_configuration(&config.host_executable)
        .map_err(|_| TargetError::Image)?;
    Ok(ResolvedTarget { endpoint, image })
}

/// Absolute `X:\...` form only; UNC, device and ADS spellings are refused.
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
