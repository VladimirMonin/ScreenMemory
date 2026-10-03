//! Host-owned config and exact vault authority. No RPC/renderer/ENV paths or tokens.
use async_trait::async_trait;
use serde::Deserialize;
use sm_application::credentials::{CredentialEnvironment, CredentialKey, Secret, SecretStore};
use sm_domain::LibraryId;
use sm_runtime::{RuntimeError, SecretAuthority, Settlement};
use std::{io::Read, path::Path, sync::Arc};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeConfig {
    schema_version: u32,
    library_id: uuid::Uuid,
    port: u16,
    authentication: Authentication,
    ask_load_approved: bool,
    image_side: u32,
    model_autostart: bool,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Authentication {
    Guest,
    Credential,
}
struct HostSecretAuthority {
    store: Arc<dyn SecretStore + Send + Sync>,
    key: CredentialKey,
    authentication: Authentication,
}
fn refused() -> RuntimeError {
    RuntimeError {
        code: "HOST_CREDENTIAL_UNAVAILABLE",
        settlement: Settlement::Settled,
    }
}
#[async_trait]
impl SecretAuthority for HostSecretAuthority {
    async fn lm_studio_token(&self) -> Result<Zeroizing<String>, RuntimeError> {
        if matches!(self.authentication, Authentication::Guest) {
            return Ok(Zeroizing::new(String::new()));
        }
        let secret = self.store.read(&self.key).map_err(|_| refused())?;
        let text = std::str::from_utf8(secret.as_bytes()).map_err(|_| refused())?;
        if !(1..=2048).contains(&text.len())
            || text.chars().any(char::is_control)
            || text.trim().is_empty()
        {
            return Err(refused());
        }
        Ok(Zeroizing::new(text.to_owned()))
    }
}
/// Called only after native root validation and acquiring the sole library lease.
/// Fixed sibling package comes from the current trusted host image, not settings paths.
pub(crate) fn open(
    root: &Path,
    library: LibraryId,
    store: Arc<dyn SecretStore + Send + Sync>,
    worker: Option<Arc<sm_image_worker::TrustedWorker>>,
) -> Result<Option<crate::processing::TrustedProcessingRuntime>, crate::HostError> {
    let path = root.join("config").join("processing-runtime.json");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(crate::HostError::Compose),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
        return Err(crate::HostError::Compose);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(crate::HostError::Compose);
        }
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .map_err(|_| crate::HostError::Compose)?
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| crate::HostError::Compose)?;
    if bytes.len() > 4096 {
        return Err(crate::HostError::Compose);
    }
    let c: RuntimeConfig = serde_json::from_slice(&bytes).map_err(|_| crate::HostError::Compose)?;
    if c.schema_version != 1
        || c.library_id != library.as_uuid()
        || c.port == 0
        || c.model_autostart
        || ![1024, 2048].contains(&c.image_side)
    {
        return Err(crate::HostError::Compose);
    }
    let install = std::env::current_exe().map_err(|_| crate::HostError::Compose)?;
    let package = install
        .parent()
        .ok_or(crate::HostError::Compose)?
        .join("sdk-runtime");
    let authority = Arc::new(HostSecretAuthority {
        store,
        key: CredentialKey::runtime_backend(CredentialEnvironment::Test, library),
        authentication: c.authentication,
    });
    crate::processing::TrustedProcessingRuntime::open(
        &package,
        format!("ws://127.0.0.1:{}", c.port),
        authority,
        worker.ok_or(crate::HostError::Compose)?,
        c.ask_load_approved,
        c.image_side,
    )
    .map(Some)
    .map_err(|_| crate::HostError::Compose)
}
/// Non-interactive stdin only, UTF-8 1..2048 bytes. No argv/ENV/plaintext config fallback.
/// EOF terminates input; callers must pipe from their own secret authority, not a terminal.
pub fn read_backend_secret(input: &mut impl Read) -> Result<Secret, crate::HostError> {
    let mut bytes = Zeroizing::new(Vec::new());
    input
        .take(2049)
        .read_to_end(&mut bytes)
        .map_err(|_| crate::HostError::CredentialUnavailable)?;
    let s = std::str::from_utf8(&bytes).map_err(|_| crate::HostError::CredentialDenied)?;
    if !(1..=2048).contains(&bytes.len()) || s.trim().is_empty() || s.chars().any(char::is_control)
    {
        return Err(crate::HostError::CredentialDenied);
    }
    Ok(Secret::from_bytes(bytes.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_secret_input_refuses_overflow_control_empty_and_non_utf8() {
        for bytes in [vec![], vec![b'x'; 2049], b"token\n".to_vec(), vec![255]] {
            assert!(read_backend_secret(&mut bytes.as_slice()).is_err());
        }
        assert_eq!(
            read_backend_secret(&mut b"synthetic-token".as_slice())
                .unwrap()
                .as_bytes(),
            b"synthetic-token"
        );
    }
    #[test]
    fn config_never_accepts_path_token_or_load_authority() {
        let base = serde_json::json!({"schema_version":1,"library_id":uuid::Uuid::new_v4(),"port":1234,"authentication":"guest","ask_load_approved":false,"image_side":1024,"model_autostart":false});
        assert!(serde_json::from_value::<RuntimeConfig>(base.clone()).is_ok());
        for key in ["package_path", "token", "endpoint", "load_model"] {
            let mut bad = base.clone();
            bad[key] = serde_json::json!("injected");
            assert!(serde_json::from_value::<RuntimeConfig>(bad).is_err());
        }
    }
}
