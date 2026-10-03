//! Windows Credential Manager adapter. No enumeration, other-product targets or fallback.
#![cfg(windows)]
#[cfg(test)]
mod native_tests;

use sm_application::credentials::CredentialEnvironment;
use sm_application::credentials::{CredentialKey, Profile, Secret, SecretStore, StoreError};
use sm_isolation::Environment;
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::{
    CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};
use zeroize::Zeroize;

// Internal error detail is discarded at the production SecretStore boundary.
// Test-only cleanup may report a Win32 code for the *same* native call.
enum NativeReadFailure {
    Missing,
    // Raw native detail is consumed only by the private test cleanup diagnostic.
    #[cfg_attr(not(test), allow(dead_code))]
    Unavailable(Option<u32>),
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AbsenceFailure {
    StillExists,
    Unavailable(Option<u32>),
}

/// A constructed key must use the environment of the already-resolved library layout.
/// The caller cannot provide an arbitrary Credential Manager target.
pub struct WindowsSecretStore {
    environment: Environment,
}
impl WindowsSecretStore {
    pub fn new(environment: Environment) -> Self {
        Self { environment }
    }
    fn target(&self, key: &CredentialKey) -> Result<Vec<u16>, StoreError> {
        let expected = match self.environment {
            Environment::Product => CredentialEnvironment::Product,
            Environment::Dev => CredentialEnvironment::Dev,
            Environment::Test => CredentialEnvironment::Test,
        };
        let target = key.target();
        if key.library().as_uuid().is_nil()
            || key.environment() != expected
            || !target.starts_with(&format!("{}/library/", self.environment.secret_namespace()))
        {
            return Err(StoreError::Unavailable);
        }
        Ok(target.encode_utf16().chain(std::iter::once(0)).collect())
    }
    /// Native test harness only: deletes EXACT previously-created test target;
    /// never scans the vault and refuses production/dev namespaces.
    #[cfg(test)]
    fn delete_created_test_key(&self, key: &CredentialKey) -> Result<(), Option<u32>> {
        use windows_sys::Win32::Security::Credentials::CredDeleteW;
        if self.environment != Environment::Test {
            return Err(None);
        }
        let target = self.target(key).map_err(|_| None)?;
        // `target` remains allocated and NUL-terminated throughout the native call.
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            return Err(Some(unsafe { GetLastError() }));
        }
        Ok(())
    }
    #[cfg(test)]
    fn verify_absent_test_key(&self, key: &CredentialKey) -> Result<(), AbsenceFailure> {
        match self.read_native(key) {
            Err(NativeReadFailure::Missing) => Ok(()),
            Err(NativeReadFailure::Unavailable(code)) => Err(AbsenceFailure::Unavailable(code)),
            Ok(_) => Err(AbsenceFailure::StillExists),
        }
    }
    fn read_native(&self, key: &CredentialKey) -> Result<Secret, NativeReadFailure> {
        let target = self
            .target(key)
            .map_err(|_| NativeReadFailure::Unavailable(None))?;
        let mut raw = std::ptr::null_mut();
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw) } == 0 {
            let code = unsafe { GetLastError() }; // immediately after the failing native call
            return Err(if code == ERROR_NOT_FOUND {
                NativeReadFailure::Missing
            } else {
                NativeReadFailure::Unavailable(Some(code))
            });
        }
        if raw.is_null() {
            return Err(NativeReadFailure::Unavailable(None));
        }
        let owned = OwnedCredential(raw);
        let credential = unsafe { &*owned.0 };
        if credential.Type != CRED_TYPE_GENERIC
            || !valid_size(key, credential.CredentialBlobSize as usize)
            || credential.CredentialBlob.is_null()
        {
            return Err(NativeReadFailure::Unavailable(None));
        }
        let bytes = unsafe {
            std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
        };
        Ok(Secret::from_bytes(bytes.to_vec()))
    }
}
fn valid_size(key: &CredentialKey, size: usize) -> bool {
    if key.profile() == Profile::RuntimeBackend {
        (1..=2048).contains(&size)
    } else {
        size == 33
    }
}
fn valid_backend_token(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .is_ok_and(|s| !s.chars().any(char::is_control) && !s.trim().is_empty())
}
struct OwnedCredential(*mut CREDENTIALW);
impl Drop for OwnedCredential {
    fn drop(&mut self) {
        unsafe {
            let blob = &*self.0;
            if !blob.CredentialBlob.is_null() && blob.CredentialBlobSize <= 2560 {
                std::slice::from_raw_parts_mut(
                    blob.CredentialBlob,
                    blob.CredentialBlobSize as usize,
                )
                .zeroize();
            }
            CredFree(self.0.cast::<c_void>());
        }
    }
}
impl SecretStore for WindowsSecretStore {
    fn read(&self, key: &CredentialKey) -> Result<Secret, StoreError> {
        self.read_native(key).map_err(|error| match error {
            NativeReadFailure::Missing => StoreError::Missing,
            NativeReadFailure::Unavailable(_) => StoreError::Unavailable,
        })
    }
    fn create_new(&self, key: &CredentialKey, value: &Secret) -> Result<(), StoreError> {
        let target = self.target(key)?;
        // CredWriteW overwrites. A read immediately before writing refuses known collisions;
        // callers must hold exclusive HostLease. The Win32 API has no create-if-absent flag.
        match self.read(key) {
            Err(StoreError::Missing) => (),
            _ => return Err(StoreError::Unavailable),
        }
        if !valid_size(key, value.as_bytes().len())
            || (key.profile() != Profile::RuntimeBackend && value.as_bytes()[0] != 1)
            || (key.profile() == Profile::RuntimeBackend && !valid_backend_token(value.as_bytes()))
        {
            return Err(StoreError::Unavailable);
        }
        // Windows takes pointer to writable buffer, do not expose caller's backing allocation.
        let mut blob = value.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_ptr() as *mut u16,
            Comment: std::ptr::null_mut(),
            LastWritten: Default::default(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: std::ptr::null_mut(),
        };
        let result = unsafe { CredWriteW(&credential, 0) };
        blob.zeroize();
        if result == 0 {
            return Err(StoreError::Unavailable);
        }
        Ok(())
    }
}
