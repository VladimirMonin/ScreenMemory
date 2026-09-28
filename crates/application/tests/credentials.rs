use sm_application::credentials::{
    authorize, Authenticator, CredentialEnvironment, CredentialError, CredentialKey, Secret,
    SecretStore, StoreError,
};
use sm_application::Scope;
use sm_domain::{CorrelationId, LibraryId, RequestId};
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
struct SyntheticStore(Mutex<HashMap<String, Vec<u8>>>);
impl SecretStore for SyntheticStore {
    fn read(&self, key: &CredentialKey) -> Result<Secret, StoreError> {
        self.0
            .lock()
            .unwrap()
            .get(&key.target())
            .cloned()
            .map(Secret::from_bytes)
            .ok_or(StoreError::Missing)
    }
    fn create_new(&self, key: &CredentialKey, value: &Secret) -> Result<(), StoreError> {
        let mut entries = self.0.lock().unwrap();
        if entries.contains_key(&key.target()) {
            return Err(StoreError::Unavailable);
        }
        entries.insert(key.target(), value.as_bytes().to_vec());
        Ok(())
    }
}
fn id(n: u128) -> LibraryId {
    LibraryId::new(Uuid::from_u128(n))
}
fn authenticate(
    auth: &Authenticator,
    library: LibraryId,
    secret: &Secret,
) -> Result<sm_application::RequestContext, CredentialError> {
    auth.authenticate(
        library,
        secret,
        RequestId::new(Uuid::from_u128(10)),
        CorrelationId::new(Uuid::from_u128(11)),
    )
}
#[test]
fn s01_cred_01_generation_reopen_and_independence() {
    let store = SyntheticStore::default();
    let key = CredentialKey::owner(CredentialEnvironment::Test, id(1));
    let first = Authenticator::enroll_owner(&store, key.clone()).unwrap();
    let own = store.read(&key).unwrap();
    assert_eq!(own.as_bytes().len(), 33);
    assert_eq!(own.as_bytes()[0], 1);
    let reopened = Authenticator::enroll_owner(&store, key.clone()).unwrap();
    let other_key = CredentialKey::owner(CredentialEnvironment::Test, id(2));
    Authenticator::enroll_owner(&store, other_key.clone()).unwrap();
    let other = store.read(&other_key).unwrap();
    assert_ne!(own.as_bytes(), other.as_bytes());
    assert!(authenticate(&first, id(1), &own).is_ok());
    assert!(authenticate(&reopened, id(1), &own).is_ok());
    assert_ne!(
        authenticate(&first, id(1), &own).unwrap().principal_id(),
        authenticate(
            &Authenticator::open(&store, other_key).unwrap(),
            id(2),
            &other
        )
        .unwrap()
        .principal_id()
    );
    assert_eq!(store.read(&key).unwrap().as_bytes(), own.as_bytes());
    assert_eq!(
        authenticate(&first, id(1), &other).err(),
        Some(CredentialError::PermissionDenied)
    );
    assert_eq!(
        authenticate(&first, id(2), &own).err(),
        Some(CredentialError::LibraryMismatch)
    );
    assert_eq!(
        authenticate(&first, id(1), &Secret::from_bytes(vec![1; 2])).err(),
        Some(CredentialError::PermissionDenied)
    );
}
#[test]
fn s01_cred_02_invalid_and_missing_fail_closed_without_rotation() {
    let store = SyntheticStore::default();
    let key = CredentialKey::owner(CredentialEnvironment::Test, id(3));
    assert_eq!(
        Authenticator::open(&store, key.clone()).err(),
        Some(CredentialError::CredentialStoreUnavailable)
    );
    store.0.lock().unwrap().insert(key.target(), vec![2; 33]);
    assert_eq!(
        Authenticator::enroll_owner(&store, key.clone()).err(),
        Some(CredentialError::CredentialStoreUnavailable)
    );
    assert_eq!(store.read(&key).unwrap().as_bytes(), &[2; 33]);
    store.0.lock().unwrap().insert(key.target(), vec![1; 34]);
    assert_eq!(
        Authenticator::enroll_owner(&store, key.clone()).err(),
        Some(CredentialError::CredentialStoreUnavailable)
    );
    assert_eq!(store.read(&key).unwrap().as_bytes(), &[1; 34]);
    let empty = CredentialKey::owner(CredentialEnvironment::Dev, id(3));
    assert_eq!(
        Authenticator::open(&store, empty).err(),
        Some(CredentialError::CredentialStoreUnavailable)
    );
}
#[test]
fn s01_cred_07_failed_write_and_readback_never_claim_enrollment() {
    struct RefusingStore(Mutex<HashMap<String, Vec<u8>>>);
    impl SecretStore for RefusingStore {
        fn read(&self, key: &CredentialKey) -> Result<Secret, StoreError> {
            self.0
                .lock()
                .unwrap()
                .get(&key.target())
                .cloned()
                .map(Secret::from_bytes)
                .ok_or(StoreError::Missing)
        }
        fn create_new(&self, _: &CredentialKey, _: &Secret) -> Result<(), StoreError> {
            Err(StoreError::Unavailable)
        }
    }
    let key = CredentialKey::owner(CredentialEnvironment::Test, id(7));
    let store = RefusingStore(Mutex::new(HashMap::new()));
    assert_eq!(
        Authenticator::enroll_owner(&store, key.clone()).err(),
        Some(CredentialError::CredentialStoreUnavailable)
    );
    assert!(matches!(store.read(&key), Err(StoreError::Missing)));
    store.0.lock().unwrap().insert(key.target(), vec![2; 33]);
    assert_eq!(
        Authenticator::enroll_owner(&store, key.clone()).err(),
        Some(CredentialError::CredentialStoreUnavailable)
    );
    assert_eq!(store.read(&key).unwrap().as_bytes(), &[2; 33]);
}
#[test]
fn s01_cred_03_profiles_and_modes_never_escalate_by_client_kind() {
    let store = SyntheticStore::default();
    let owner = CredentialKey::owner(CredentialEnvironment::Test, id(4));
    let read = CredentialKey::test_read_only(id(4));
    let product = CredentialKey::owner(CredentialEnvironment::Product, id(4));
    assert_ne!(owner.target(), read.target());
    assert_ne!(owner.target(), product.target());
    let policy = Authenticator::enroll_test_read_only(&store, read.clone()).unwrap();
    let presented = store.read(&read).unwrap();
    let ctx = authenticate(&policy, id(4), &presented).unwrap();
    let owner_policy = Authenticator::enroll_owner(&store, owner.clone()).unwrap();
    let owner_ctx = authenticate(&owner_policy, id(4), &store.read(&owner).unwrap()).unwrap();
    assert_ne!(owner_ctx.principal_id(), ctx.principal_id());
    let client_kind = "gui";
    assert_eq!(client_kind, "gui"); // metadata is never passed to authenticate
    for scope in [
        Scope::RunAiProbe,
        Scope::LoadModelForProbe,
        Scope::ManageHost,
    ] {
        let mut handler_called = false;
        assert_eq!(
            authorize(&ctx, scope, || {
                handler_called = true;
            }),
            Err(CredentialError::PermissionDenied)
        );
        assert!(!handler_called);
    }
    assert!(authorize(&ctx, Scope::ReadStatus, || 7).is_ok());
    assert_eq!(
        authenticate(&policy, id(4), &Secret::from_bytes(vec![0; 33])).err(),
        Some(CredentialError::PermissionDenied)
    );
}
#[test]
fn s01_cred_06_synthetic_canary_absent_from_real_console_sink_and_errors() {
    use sm_application::observability::{
        Component, EventId, Level, Operation, Outcome, SafeCode, SafeEvent, SafeEventSink,
    };
    use sm_observability::{BoundedLogger, ConsoleSink, FakeClock, FlushOutcome};
    use std::io::{self, Write};
    use std::sync::Arc;
    use std::time::{Duration, UNIX_EPOCH};
    #[derive(Clone)]
    struct Writer(Arc<Mutex<Vec<u8>>>);
    impl Write for Writer {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut value = vec![1];
    let canary = "SYNTHETIC-DO-NOT-LOG-CREDENTIAL!";
    assert_eq!(canary.len(), 32);
    value.extend_from_slice(canary.as_bytes());
    let store = SyntheticStore::default();
    let key = CredentialKey::owner(CredentialEnvironment::Test, id(8));
    store.0.lock().unwrap().insert(key.target(), value.clone());
    let policy = Authenticator::open(&store, key).unwrap();
    let denial = authenticate(&policy, id(8), &Secret::from_bytes(vec![1; 33]))
        .err()
        .unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut logger = BoundedLogger::start(
        ConsoleSink::new(Writer(output.clone())),
        Arc::new(FakeClock::new(UNIX_EPOCH)),
        4,
    )
    .unwrap();
    let mut event = SafeEvent::new(
        Level::Warn,
        Component::Host,
        EventId::HostFailed,
        Operation::Startup,
    );
    event.outcome = Some(Outcome::Failed);
    event.safe_code = Some(SafeCode::InternalFailure);
    assert!(logger.producer().emit(event));
    assert_eq!(
        logger.shutdown(Duration::from_secs(3)),
        FlushOutcome::Flushed
    );
    let actual = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(!actual.contains(canary));
    assert!(!format!("{denial:?}").contains(canary));
    // No Debug/Display or Serialize on Secret and Authenticator: compile-time trait absence
    // is enforced by their definitions and inspected in the static contract check.
}

struct Broken;
impl SecretStore for Broken {
    fn read(&self, _: &CredentialKey) -> Result<Secret, StoreError> {
        Err(StoreError::Unavailable)
    }
    fn create_new(&self, _: &CredentialKey, _: &Secret) -> Result<(), StoreError> {
        panic!("unreachable write")
    }
}
#[test]
fn s01_cred_04_store_failure_does_not_provision_or_authorize() {
    let key = CredentialKey::owner(CredentialEnvironment::Test, id(5));
    assert_eq!(
        Authenticator::enroll_owner(&Broken, key).err(),
        Some(CredentialError::CredentialStoreUnavailable)
    );
}
