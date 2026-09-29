#![cfg(windows)]
use sm_domain::OperationId;
use sm_isolation::{
    HostLease, LeaseError, LibraryRootResolver, RootSource, StagingError, StagingTempKind,
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let path = parent.join(format!("t09b-native-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        for name in ["data", "config", "cache", "webview"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        Self(path)
    }
    fn lease(&self) -> HostLease {
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &self.0,
                forbidden_donor_roots: &[],
            })
            .unwrap(),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn t09b_native_create_new_flush_and_no_unknown_purge() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let id = OperationId::new(Uuid::new_v4());
    assert!(matches!(
        root.create_operation(OperationId::new(Uuid::nil())),
        Err(StagingError::InvalidOperation)
    ));
    assert!(matches!(
        root.open_operation(OperationId::new(Uuid::nil())),
        Err(StagingError::InvalidOperation)
    ));
    assert!(matches!(
        root.open_operation(OperationId::new(Uuid::new_v4())),
        Err(StagingError::Missing)
    ));
    let op = root.create_operation(id).unwrap();
    let staging = f.0.join("staging");
    let directory = staging.join(id.as_uuid().to_string());
    assert!(fs::rename(&staging, f.0.join("staging-moved")).is_err());
    assert!(fs::rename(&directory, staging.join("operation-moved")).is_err());
    fs::write(directory.join("unknown.bin"), b"SYNTHETIC-UNKNOWN").unwrap();
    for (kind, name) in [
        (StagingTempKind::PixelsRgba, "pixels.rgba.tmp"),
        (StagingTempKind::ContextJson, "context.json.tmp"),
        (StagingTempKind::ManifestJson, "manifest.json.tmp"),
    ] {
        let mut file = op.create_temp(kind).unwrap();
        file.write_all(b"SYNTHETIC-PAYLOAD").unwrap();
        file.sync_all().unwrap();
        assert!(matches!(
            op.create_temp(kind),
            Err(StagingError::AlreadyExists)
        ));
        drop(file);
        assert_eq!(
            fs::read(directory.join(name)).unwrap(),
            b"SYNTHETIC-PAYLOAD"
        );
    }
    assert!(matches!(
        root.create_operation(id),
        Err(StagingError::AlreadyExists)
    ));
    let reopened = root.open_operation(id).unwrap();
    assert!(matches!(
        reopened.open_fixed(StagingTempKind::PixelsRgba),
        Err(StagingError::Missing)
    ));
    assert_eq!(
        fs::read(directory.join("unknown.bin")).unwrap(),
        b"SYNTHETIC-UNKNOWN"
    );
    drop(reopened);
    drop(op);
    drop(root);
    drop(lease);
    assert!(f.lease().staging_root().is_ok());
}

#[test]
fn t09b_publish_fixed_reopen_no_overwrite_or_purge() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let id = OperationId::new(Uuid::new_v4());
    let op = root.create_operation(id).unwrap();
    let dir = f.0.join("staging").join(id.as_uuid().to_string());
    fs::write(dir.join("unknown.bin"), b"UNKNOWN-KEEP").unwrap();
    for (kind, payload) in [
        (StagingTempKind::PixelsRgba, &b"RGBA-SYNTHETIC"[..]),
        (StagingTempKind::ContextJson, &b"{\"synthetic\":true}"[..]),
        (StagingTempKind::ManifestJson, &b"{\"version\":1}"[..]),
    ] {
        let mut temp = op.create_temp(kind).unwrap();
        temp.write_all(payload).unwrap();
        temp.sync_all().unwrap();
        temp.publish().unwrap();
        let read = op.open_fixed(kind).unwrap();
        assert_eq!(read.len(), payload.len() as u64);
        assert_eq!(read.read_all().unwrap(), payload);
        let mut repeat = op.create_temp(kind).unwrap();
        repeat.write_all(b"REPEATED").unwrap();
        repeat.sync_all().unwrap();
        assert_eq!(repeat.publish(), Err(StagingError::AlreadyExists));
        assert_eq!(op.open_fixed(kind).unwrap().read_all().unwrap(), payload);
    }
    let reopened = root.open_operation(id).unwrap();
    for (kind, payload) in [
        (StagingTempKind::PixelsRgba, &b"RGBA-SYNTHETIC"[..]),
        (StagingTempKind::ContextJson, &b"{\"synthetic\":true}"[..]),
        (StagingTempKind::ManifestJson, &b"{\"version\":1}"[..]),
    ] {
        assert_eq!(
            reopened.open_fixed(kind).unwrap().read_all().unwrap(),
            payload
        );
    }
    assert_eq!(fs::read(dir.join("unknown.bin")).unwrap(), b"UNKNOWN-KEEP");
}

#[test]
fn t09b_publication_refuses_unsynced_existing_and_reparse_targets() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let id = OperationId::new(Uuid::new_v4());
    let op = root.create_operation(id).unwrap();
    let dir = f.0.join("staging").join(id.as_uuid().to_string());
    let mut unsynced = op.create_temp(StagingTempKind::ManifestJson).unwrap();
    unsynced.write_all(b"UNSYNCED").unwrap();
    assert_eq!(unsynced.publish(), Err(StagingError::NotSynced));
    assert!(matches!(
        op.open_fixed(StagingTempKind::ManifestJson),
        Err(StagingError::Missing)
    ));
    let stable = dir.join("pixels.rgba");
    fs::write(&stable, b"SENTINEL").unwrap();
    let mut temp = op.create_temp(StagingTempKind::PixelsRgba).unwrap();
    temp.write_all(b"NEW").unwrap();
    temp.sync_all().unwrap();
    assert_eq!(temp.publish(), Err(StagingError::AlreadyExists));
    assert_eq!(fs::read(&stable).unwrap(), b"SENTINEL");
    assert_eq!(
        op.open_fixed(StagingTempKind::PixelsRgba)
            .unwrap()
            .read_all()
            .unwrap(),
        b"SENTINEL"
    );
    fs::remove_file(&stable).unwrap();
    let outside = f.0.join("outside.txt");
    fs::write(&outside, b"OUTSIDE-SENTINEL").unwrap();
    match std::os::windows::fs::symlink_file(&outside, &stable) {
        Ok(()) => {
            assert!(matches!(
                op.open_fixed(StagingTempKind::PixelsRgba),
                Err(StagingError::UnsafeEntry)
            ));
            // Existing temp remains after refused publication, never silently discarded.
            // Create a fresh operation to prove publication refuses the symlink target.
            let other_id = OperationId::new(Uuid::new_v4());
            let other = root.create_operation(other_id).unwrap();
            let other_dir = f.0.join("staging").join(other_id.as_uuid().to_string());
            let mut next = other.create_temp(StagingTempKind::PixelsRgba).unwrap();
            next.write_all(b"NEW").unwrap();
            next.sync_all().unwrap();
            std::os::windows::fs::symlink_file(&outside, other_dir.join("pixels.rgba")).unwrap();
            assert_eq!(next.publish(), Err(StagingError::UnsafeEntry));
            assert_eq!(fs::read(&outside).unwrap(), b"OUTSIDE-SENTINEL");
        }
        Err(error) => panic!(
            "T09B-STABLE-SYMLINK NOT_RUN: raw OS code {:?}",
            error.raw_os_error()
        ),
    }
}

#[test]
fn t09b_fixed_read_rejects_oversized_context_without_cleanup() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let id = OperationId::new(Uuid::new_v4());
    let op = root.create_operation(id).unwrap();
    let dir = f.0.join("staging").join(id.as_uuid().to_string());
    let bytes = vec![b'X'; 256 * 1024 + 1];
    fs::write(dir.join("context.json"), &bytes).unwrap();
    assert!(matches!(
        op.open_fixed(StagingTempKind::ContextJson),
        Err(StagingError::TooLarge)
    ));
    assert_eq!(fs::read(dir.join("context.json")).unwrap(), bytes);
}

#[test]
fn t09b_native_reparse_root_and_operation_do_not_write_outside() {
    let f = Fixture::new();
    let outside = f.0.join("outside");
    fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("sentinel");
    fs::write(&sentinel, b"SYNTHETIC-UNCHANGED").unwrap();
    let lease = f.lease();
    let staging = f.0.join("staging");
    junction(&staging, &outside);
    assert!(matches!(
        lease.staging_root(),
        Err(StagingError::UnsafeEntry)
    ));
    remove_junction(&staging);
    let root = lease.staging_root().unwrap();
    let id = OperationId::new(Uuid::new_v4());
    let path = staging.join(id.as_uuid().to_string());
    junction(&path, &outside);
    assert!(matches!(
        root.create_operation(id),
        Err(StagingError::AlreadyExists)
    ));
    assert!(matches!(
        root.open_operation(id),
        Err(StagingError::UnsafeEntry)
    ));
    remove_junction(&path);
    assert_eq!(fs::read(sentinel).unwrap(), b"SYNTHETIC-UNCHANGED");
    assert!(!outside.join("pixels.rgba.tmp").exists());
}

#[test]
fn t09b_native_existing_reparse_leaf_refused_without_outside_write() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let id = OperationId::new(Uuid::new_v4());
    let op = root.create_operation(id).unwrap();
    let outside = f.0.join("sentinel.txt");
    fs::write(&outside, b"SYNTHETIC-UNCHANGED").unwrap();
    let leaf =
        f.0.join("staging")
            .join(id.as_uuid().to_string())
            .join("pixels.rgba.tmp");
    match std::os::windows::fs::symlink_file(&outside, &leaf) {
        Ok(()) => {
            assert!(matches!(
                op.create_temp(StagingTempKind::PixelsRgba),
                Err(StagingError::AlreadyExists)
            ));
            fs::remove_file(leaf).unwrap();
        }
        Err(err) => {
            // Privilege-dependent symlink is NOT a passing reparse proof on this runner.
            panic!(
                "T09B-LEAF-SYMLINK NOT_RUN: raw OS code {:?}",
                err.raw_os_error()
            );
        }
    }
    assert_eq!(fs::read(outside).unwrap(), b"SYNTHETIC-UNCHANGED");
}

#[test]
fn t09b_native_scoped_borrow_retains_lease_and_pins() {
    let f = Fixture::new();
    let lease = f.lease();
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let root = lease.staging_root().unwrap();
                let op = root
                    .create_operation(OperationId::new(Uuid::new_v4()))
                    .unwrap();
                let mut file = op.create_temp(StagingTempKind::PixelsRgba).unwrap();
                file.write_all(b"SCOPED-SYNTHETIC").unwrap();
                file.sync_all().unwrap();
            })
            .join()
            .unwrap();
        assert!(matches!(
            HostLease::acquire(
                LibraryRootResolver::resolve(RootSource::Test {
                    root: &f.0,
                    forbidden_donor_roots: &[],
                })
                .unwrap()
            ),
            Err(LeaseError::Busy)
        ));
    });
    drop(lease);
    let again = f.lease();
    assert!(again.staging_root().is_ok());
}

fn junction(link: &Path, target: &Path) {
    let result = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "junction creation raw exit {:?}",
        result.status.code()
    );
}
fn remove_junction(link: &Path) {
    let result = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "rmdir"])
        .arg(link)
        .status()
        .unwrap();
    assert_eq!(result.code(), Some(0));
}
