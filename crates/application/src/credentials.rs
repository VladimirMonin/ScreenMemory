//! Host-owned credential policy. No wire value may construct a RequestContext here.
use crate::{RequestContext, Scope};
use sm_domain::{CorrelationId, LibraryId, PrincipalId, RequestId};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::Zeroize;

const VERSION: u8 = 1;
const SIZE: usize = 33;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialEnvironment {
    Product,
    Dev,
    Test,
}
impl CredentialEnvironment {
    fn namespace(self) -> &'static str {
        match self {
            Self::Product => "ScreenMemory.V1",
            Self::Dev => "ScreenMemory.Dev.V1",
            Self::Test => "ScreenMemory.Test.V1",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Owner,
    /// Reserved solely for S01 policy verification; no S07 provisioning implied.
    TestReadOnly,
}
impl Profile {
    fn label(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::TestReadOnly => "test-readonly",
        }
    }
    fn scopes(self) -> Vec<Scope> {
        match self {
            Self::Owner => vec![
                Scope::ReadStatus,
                Scope::RunDoctor,
                Scope::ReadRuntimeInventory,
                Scope::RunAiProbe,
                Scope::LoadModelForProbe,
                Scope::ManageHost,
                // S02 §5.6: a new capture and a bounded read of an owned canonical asset
                // are owner actions. The read-only principal never receives them.
                Scope::CaptureScreen,
                Scope::ReadImages,
            ],
            Self::TestReadOnly => vec![Scope::ReadStatus, Scope::RunDoctor],
        }
    }
}

/// Target is derived only from product mode, a typed library identity, and fixed profile.
/// It cannot be constructed from an arbitrary caller-supplied Credential Manager target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialKey {
    environment: CredentialEnvironment,
    library: LibraryId,
    profile: Profile,
}
impl CredentialKey {
    pub fn owner(environment: CredentialEnvironment, library: LibraryId) -> Self {
        Self {
            environment,
            library,
            profile: Profile::Owner,
        }
    }
    pub fn test_read_only(library: LibraryId) -> Self {
        Self {
            environment: CredentialEnvironment::Test,
            library,
            profile: Profile::TestReadOnly,
        }
    }
    pub fn target(&self) -> String {
        format!(
            "{}/library/{}/{}",
            self.environment.namespace(),
            self.library.as_uuid(),
            self.profile.label()
        )
    }
    pub fn environment(&self) -> CredentialEnvironment {
        self.environment
    }
    pub fn library(&self) -> LibraryId {
        self.library
    }
    pub fn profile(&self) -> Profile {
        self.profile
    }
    pub fn principal(&self) -> PrincipalId {
        use sha2::{Digest, Sha256};
        // Non-secret identity scoped by namespace, library and profile, never client_kind.
        let digest = Sha256::digest(self.target().as_bytes());
        PrincipalId::new(Uuid::from_slice(&digest[..16]).expect("fixed SHA-256 prefix"))
    }
}

/// Secret is never printable or serializable. Its backing allocation is wiped on drop.
/// ```compile_fail
/// use sm_application::credentials::Secret;
/// let secret = Secret::from_bytes(vec![1; 33]);
/// let _ = format!("{secret:?}");
/// ```
pub struct Secret(Vec<u8>);
impl Secret {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    Missing,
    Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialError {
    CredentialStoreUnavailable,
    PermissionDenied,
    LibraryMismatch,
}

/// Implementations must not log secrets, overwrite existing entries in `create_new`,
/// or map access failures to Missing. Store ownership and native cleanup are adapter duties.
pub trait SecretStore {
    fn read(&self, key: &CredentialKey) -> Result<Secret, StoreError>;
    fn create_new(&self, key: &CredentialKey, value: &Secret) -> Result<(), StoreError>;
}

pub struct Authenticator {
    key: CredentialKey,
    verifier: [u8; 32],
}
impl Authenticator {
    /// Explicit enrollment only, under the future host's exclusive library lease.
    /// Authentication itself never provisions a key. Reopen does not rotate it.
    pub fn enroll_owner(
        store: &impl SecretStore,
        key: CredentialKey,
    ) -> Result<Self, CredentialError> {
        if key.profile != Profile::Owner {
            return Err(CredentialError::PermissionDenied);
        }
        Self::enroll(store, key)
    }
    /// Explicitly opt into a synthetic read-only principal in a Test namespace only.
    pub fn enroll_test_read_only(
        store: &impl SecretStore,
        key: CredentialKey,
    ) -> Result<Self, CredentialError> {
        if key.environment != CredentialEnvironment::Test || key.profile != Profile::TestReadOnly {
            return Err(CredentialError::PermissionDenied);
        }
        Self::enroll(store, key)
    }
    fn enroll(store: &impl SecretStore, key: CredentialKey) -> Result<Self, CredentialError> {
        match store.read(&key) {
            Ok(existing) => Self::from_secret(key, &existing),
            Err(StoreError::Missing) => {
                let mut bytes = [0u8; SIZE];
                bytes[0] = VERSION;
                if getrandom::fill(&mut bytes[1..]).is_err() {
                    bytes.zeroize();
                    return Err(CredentialError::CredentialStoreUnavailable);
                }
                let generated = Secret::from_bytes(bytes.to_vec());
                bytes.zeroize();
                store
                    .create_new(&key, &generated)
                    .map_err(|_| CredentialError::CredentialStoreUnavailable)?;
                // Verify durable readback; do not accept a different concurrent writer's bytes.
                let saved = store
                    .read(&key)
                    .map_err(|_| CredentialError::CredentialStoreUnavailable)?;
                if saved.as_bytes().ct_eq(generated.as_bytes()).unwrap_u8() != 1 {
                    return Err(CredentialError::CredentialStoreUnavailable);
                }
                Self::from_secret(key, &saved)
            }
            Err(StoreError::Unavailable) => Err(CredentialError::CredentialStoreUnavailable),
        }
    }
    pub fn open(store: &impl SecretStore, key: CredentialKey) -> Result<Self, CredentialError> {
        let stored = store
            .read(&key)
            .map_err(|_| CredentialError::CredentialStoreUnavailable)?;
        Self::from_secret(key, &stored)
    }
    fn from_secret(key: CredentialKey, secret: &Secret) -> Result<Self, CredentialError> {
        if secret.as_bytes().len() != SIZE || secret.as_bytes()[0] != VERSION {
            return Err(CredentialError::CredentialStoreUnavailable);
        }
        use sha2::{Digest, Sha256};
        let verifier: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        Ok(Self { key, verifier })
    }
    /// Principal derived from the credential's fixed identity key.
    pub fn principal(&self) -> PrincipalId {
        self.key.principal()
    }
    /// Private handshake boundary: reject library mismatch before entering any handler.
    /// `client_kind` is deliberately absent; caller metadata never confers scopes.
    pub fn authenticate(
        &self,
        library: LibraryId,
        supplied: &Secret,
        request: RequestId,
        correlation: CorrelationId,
    ) -> Result<RequestContext, CredentialError> {
        if library != self.key.library {
            return Err(CredentialError::LibraryMismatch);
        }
        if supplied.as_bytes().len() != SIZE || supplied.as_bytes()[0] != VERSION {
            return Err(CredentialError::PermissionDenied);
        }
        use sha2::{Digest, Sha256};
        let mut digest: [u8; 32] = Sha256::digest(supplied.as_bytes()).into();
        let valid = self.verifier.ct_eq(&digest).unwrap_u8() == 1;
        digest.zeroize();
        if !valid {
            return Err(CredentialError::PermissionDenied);
        }
        Ok(RequestContext::from_authenticated_peer(
            request,
            correlation,
            self.key.principal(),
            self.key.profile.scopes(),
        ))
    }
}
impl Drop for Authenticator {
    fn drop(&mut self) {
        self.verifier.zeroize();
    }
}

/// Private pre-handler policy: no privileged callback executes on insufficient scope.
pub fn authorize<T>(
    ctx: &RequestContext,
    required: Scope,
    handler: impl FnOnce() -> T,
) -> Result<T, CredentialError> {
    if !ctx.allows(required) {
        return Err(CredentialError::PermissionDenied);
    }
    Ok(handler())
}
