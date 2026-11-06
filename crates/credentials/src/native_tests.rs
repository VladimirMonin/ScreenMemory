use crate::{AbsenceFailure, WindowsSecretStore};
use sm_application::credentials::{
    Authenticator, CredentialEnvironment, CredentialError, CredentialKey, Secret, SecretStore,
    StoreError,
};
use sm_domain::LibraryId;
use sm_isolation::Environment;
use std::cell::Cell;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CleanupFailure {
    Delete(Option<u32>),
    Verify(AbsenceFailure),
}
impl CleanupFailure {
    fn safe_status(self) -> String {
        match self {
            Self::Delete(code) => format!("delete failure; win32_code={code:?}"),
            Self::Verify(AbsenceFailure::StillExists) => {
                "absence verification failed; still exists".into()
            }
            Self::Verify(AbsenceFailure::Unavailable(code)) => {
                format!("absence verification failed; win32_code={code:?}")
            }
        }
    }
}
// Real guard and synthetic error injections exercise the same fail-closed classifier.
// A failed delete never reports success and never performs a verification call.
fn cleanup_owned(
    delete: impl FnOnce() -> Result<(), Option<u32>>,
    verify: impl FnOnce() -> Result<(), AbsenceFailure>,
) -> Result<(), CleanupFailure> {
    delete().map_err(CleanupFailure::Delete)?;
    verify().map_err(CleanupFailure::Verify)
}

// A private SecretStore seam observes *successful* native create_new completion.
// The guard exists before enrollment and owns only the exact key written by this attempt.
struct OwnedTestKey<'a> {
    store: &'a WindowsSecretStore,
    key: CredentialKey,
    owned: Cell<bool>,
    fail_verification_read: bool,
}
impl<'a> OwnedTestKey<'a> {
    fn new(
        store: &'a WindowsSecretStore,
        key: CredentialKey,
        fail_verification_read: bool,
    ) -> Self {
        Self {
            store,
            key,
            owned: Cell::new(false),
            fail_verification_read,
        }
    }
}
impl SecretStore for OwnedTestKey<'_> {
    fn read(&self, key: &CredentialKey) -> Result<Secret, StoreError> {
        if key != &self.key {
            return Err(StoreError::Unavailable);
        }
        if self.owned.get() && self.fail_verification_read {
            return Err(StoreError::Unavailable);
        }
        self.store.read(key)
    }
    fn create_new(&self, key: &CredentialKey, value: &Secret) -> Result<(), StoreError> {
        if key != &self.key || self.owned.get() {
            return Err(StoreError::Unavailable);
        }
        self.store.create_new(key, value)?;
        // CredWriteW returned success; no fallible operation between completion and arming.
        self.owned.set(true);
        Ok(())
    }
}
impl Drop for OwnedTestKey<'_> {
    fn drop(&mut self) {
        if !self.owned.get() {
            return;
        }
        let result = cleanup_owned(
            || self.store.delete_created_test_key(&self.key),
            || self.store.verify_absent_test_key(&self.key),
        );
        if let Err(failure) = result {
            let target = self.key.target();
            let status = failure.safe_status();
            eprintln!("exact synthetic credential cleanup failed for {target}: {status}");
            // Preserve the original panic on unwind; exact own identity permits
            // manual reconciliation if cleanup did not confirm native absence.
            if !std::thread::panicking() {
                panic!("exact synthetic credential cleanup failed for {target}: {status}");
            }
        }
    }
}

#[test]
fn s01_cred_11_injected_cleanup_branches_safe_and_no_native_keys() {
    let mut verified = false;
    let failed = cleanup_owned(
        || Err(Some(5)),
        || {
            verified = true;
            Ok(())
        },
    );
    assert_eq!(failed, Err(CleanupFailure::Delete(Some(5))));
    assert!(!verified, "failed delete must not claim confirmed absence");
    assert_eq!(
        failed.unwrap_err().safe_status(),
        "delete failure; win32_code=Some(5)"
    );
    let failed = cleanup_owned(|| Ok(()), || Err(AbsenceFailure::Unavailable(Some(87))));
    assert_eq!(
        failed,
        Err(CleanupFailure::Verify(AbsenceFailure::Unavailable(Some(
            87
        ))))
    );
    assert_eq!(
        failed.unwrap_err().safe_status(),
        "absence verification failed; win32_code=Some(87)"
    );
    assert_eq!(
        cleanup_owned(|| Ok(()), || Err(AbsenceFailure::StillExists)),
        Err(CleanupFailure::Verify(AbsenceFailure::StillExists))
    );
    assert_eq!(cleanup_owned(|| Ok(()), || Ok(())), Ok(()));
    let canary = "synthetic-secret-canary";
    assert!(!CleanupFailure::Delete(None).safe_status().contains(canary));
    assert!(!CleanupFailure::Verify(AbsenceFailure::StillExists)
        .safe_status()
        .contains(canary));
}

#[test]
fn s07_agent_exact_native_roundtrip_collision_environment_and_absence() {
    use sm_application::credentials::AgentConnectionId;
    let store = WindowsSecretStore::new(Environment::Test);
    let library = LibraryId::new(Uuid::new_v4());
    let connection = AgentConnectionId::new(Uuid::new_v4()).unwrap();
    let key = CredentialKey::agent(CredentialEnvironment::Test, library, connection);
    assert_eq!(
        key.target(),
        format!(
            "ScreenMemory.Test.V1/library/{}/agent/{}",
            library.as_uuid(),
            connection.0 .0
        )
    );
    let guard = OwnedTestKey::new(&store, key.clone(), false);
    assert!(matches!(store.read(&key), Err(StoreError::Missing)));
    let secret = Secret::generate_session().unwrap();
    guard.create_new(&key, &secret).unwrap();
    assert!(store
        .read(&key)
        .unwrap()
        .matches_session_digest(&secret.session_digest().unwrap()));
    assert!(store
        .create_new(&key, &Secret::generate_session().unwrap())
        .is_err());
    assert!(store
        .read(&key)
        .unwrap()
        .matches_session_digest(&secret.session_digest().unwrap()));
    assert!(WindowsSecretStore::new(Environment::Dev)
        .read(&key)
        .is_err());
    assert!(store
        .delete_agent(&CredentialKey::owner(CredentialEnvironment::Test, library))
        .is_err());
    store.delete_agent(&key).unwrap();
    // Native delete_agent confirmed absence itself; disarm only after that succeeds.
    guard.owned.set(false);
    assert!(matches!(store.read(&key), Err(StoreError::Missing)));
    println!(
        "NATIVE_AGENT_SLOT_ABSENT library={} connection={}",
        library.as_uuid(),
        connection.0 .0
    );
}

fn new_key() -> CredentialKey {
    CredentialKey::owner(CredentialEnvironment::Test, LibraryId::new(Uuid::new_v4()))
}

#[test]
fn s01_cred_05_native_unique_roundtrip_and_cleanup() {
    let store = WindowsSecretStore::new(Environment::Test);
    let key = new_key();
    let target = key.target();
    let attempt = OwnedTestKey::new(&store, key.clone(), false);
    assert!(
        matches!(store.read(&key), Err(StoreError::Missing)),
        "collision/refusal for synthetic target {target}"
    );
    Authenticator::enroll_owner(&attempt, key.clone())
        .unwrap_or_else(|_| panic!("enrollment failed for synthetic target {target}"));
    let first = store
        .read(&key)
        .expect("native readback after confirmed write");
    assert_eq!(first.as_bytes().len(), 33);
    Authenticator::enroll_owner(&attempt, key.clone()).expect("reopen");
    assert_eq!(
        store.read(&key).expect("native reopen readback").as_bytes(),
        first.as_bytes()
    );
    assert!(matches!(
        attempt.create_new(&key, &Secret::from_bytes(vec![1; 33])),
        Err(StoreError::Unavailable)
    ));
    assert_eq!(
        store
            .read(&key)
            .expect("native readback after refusal")
            .as_bytes(),
        first.as_bytes()
    );
    drop(attempt);
    assert!(
        matches!(store.read(&key), Err(StoreError::Missing)),
        "cleanup verification failed for synthetic target {target}"
    );
}

#[test]
fn s01_cred_08_native_readback_failure_cleans_confirmed_write() {
    let store = WindowsSecretStore::new(Environment::Test);
    let key = new_key();
    let target = key.target();
    let attempt = OwnedTestKey::new(&store, key.clone(), true);
    assert!(
        matches!(store.read(&key), Err(StoreError::Missing)),
        "collision/refusal for synthetic target {target}"
    );
    assert!(
        matches!(
            Authenticator::enroll_owner(&attempt, key.clone()),
            Err(CredentialError::CredentialStoreUnavailable)
        ),
        "injected verification failure must remain the original result"
    );
    assert!(attempt.owned.get(), "native write must have completed");
    assert_eq!(
        store
            .read(&key)
            .expect("native write persisted before failed readback")
            .as_bytes()
            .len(),
        33
    );
    drop(attempt);
    assert!(
        matches!(store.read(&key), Err(StoreError::Missing)),
        "cleanup verification failed for synthetic target {target}"
    );
}

#[test]
fn s01_cred_09_native_unwind_cleans_confirmed_write() {
    let store = WindowsSecretStore::new(Environment::Test);
    let key = new_key();
    let target = key.target();
    assert!(
        matches!(store.read(&key), Err(StoreError::Missing)),
        "collision/refusal for synthetic target {target}"
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let attempt = OwnedTestKey::new(&store, key.clone(), false);
        Authenticator::enroll_owner(&attempt, key.clone()).expect("native enrollment");
        assert!(attempt.owned.get());
        assert_eq!(
            store
                .read(&key)
                .expect("native completion")
                .as_bytes()
                .len(),
            33
        );
        panic!("synthetic post-create unwind");
    }));
    assert!(result.is_err(), "expected injected unwind");
    assert!(
        matches!(store.read(&key), Err(StoreError::Missing)),
        "unwind cleanup verification failed for synthetic target {target}"
    );
}

#[test]
fn s01_cred_10_native_collision_preserves_fixture_owner() {
    let store = WindowsSecretStore::new(Environment::Test);
    let key = new_key();
    let target = key.target();
    let fixture = OwnedTestKey::new(&store, key.clone(), false);
    assert!(
        matches!(store.read(&key), Err(StoreError::Missing)),
        "collision/refusal for synthetic target {target}"
    );
    Authenticator::enroll_owner(&fixture, key.clone()).expect("fixture native enrollment");
    let before = store.read(&key).expect("fixture persisted");
    let collision = OwnedTestKey::new(&store, key.clone(), false);
    assert!(matches!(
        collision.create_new(&key, &Secret::from_bytes(vec![1; 33])),
        Err(StoreError::Unavailable)
    ));
    assert!(
        !collision.owned.get(),
        "refused write cannot claim fixture ownership"
    );
    drop(collision);
    assert_eq!(
        store
            .read(&key)
            .expect("fixture remains after collision")
            .as_bytes(),
        before.as_bytes()
    );
    drop(fixture);
    assert!(
        matches!(store.read(&key), Err(StoreError::Missing)),
        "fixture cleanup verification failed for synthetic target {target}"
    );
}
