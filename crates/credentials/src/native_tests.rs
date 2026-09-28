use crate::WindowsSecretStore;
use sm_application::credentials::{
    Authenticator, CredentialEnvironment, CredentialError, CredentialKey, Secret, SecretStore,
    StoreError,
};
use sm_domain::LibraryId;
use sm_isolation::Environment;
use std::cell::Cell;
use uuid::Uuid;

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
        let cleaned = self.store.delete_created_test_key(&self.key).is_ok()
            && matches!(self.store.read(&self.key), Err(StoreError::Missing));
        if !cleaned {
            let target = self.key.target();
            eprintln!("exact synthetic credential cleanup failed for {target}: CREDENTIAL_STORE_UNAVAILABLE");
            // Preserve the original panic on unwind; the bounded diagnostic identifies
            // the possibly remaining own key for manual investigation.
            if !std::thread::panicking() {
                panic!("exact synthetic credential cleanup failed for {target}: CREDENTIAL_STORE_UNAVAILABLE");
            }
        }
    }
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
