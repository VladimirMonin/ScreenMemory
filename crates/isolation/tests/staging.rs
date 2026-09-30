#![cfg(windows)]
use sm_domain::OperationId;
use sm_isolation::{
    HostLease, LeaseError, LibraryRootResolver, RootSource, StagingError, StagingTempKind,
};
use std::{
    fs,
    io::Write,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
};
use uuid::Uuid;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&parent).unwrap();
        let parent = fs::canonicalize(parent).unwrap();
        let parent = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
        let path = parent.join(Uuid::new_v4().to_string());
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

#[test]
fn t09l1_candidate_scan_reports_incomplete_and_unsafe_without_touching_sentinels() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let valid = OperationId::new(Uuid::new_v4());
    let incomplete = OperationId::new(Uuid::new_v4());
    let non_v4 = OperationId::new(Uuid::parse_str("00000000-0000-1000-8000-000000000001").unwrap());
    let non_rfc_variant =
        OperationId::new(Uuid::parse_str("00000000-0000-4000-0000-000000000001").unwrap());
    let op = root.create_operation(valid).unwrap();
    let staged = root.create_operation(incomplete).unwrap();
    let staged_non_v4 = root.create_operation(non_v4).unwrap();
    let staged_non_rfc = root.create_operation(non_rfc_variant).unwrap();
    let staging = f.0.join("staging");
    let incomplete_path = staging.join(incomplete.as_uuid().to_string());
    fs::write(
        incomplete_path.join("pixels.rgba.tmp"),
        b"INCOMPLETE-SENTINEL",
    )
    .unwrap();
    let outside = f.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"OUTSIDE-SENTINEL").unwrap();
    fs::write(staging.join("unrelated-sentinel"), b"UNRELATED-SENTINEL").unwrap();
    fs::create_dir(staging.join("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA")).unwrap();
    fs::create_dir(staging.join("00000000-0000-0000-0000-000000000000")).unwrap();
    fs::create_dir(staging.join("00000000-0000-4000-0000-00000000000g")).unwrap();
    fs::create_dir(staging.join("00000000-0000-4000-0000-00000000001")).unwrap();
    fs::write(staging.join(Uuid::new_v4().to_string()), b"NOT-A-DIRECTORY").unwrap();
    let link = staging.join(Uuid::new_v4().to_string());
    junction(&link, &outside);
    let found = root.enumerate_candidates().unwrap();
    assert_eq!(found.ids.len(), 4);
    assert!(found.ids.contains(&valid));
    assert!(found.ids.contains(&incomplete));
    assert!(found.ids.contains(&non_v4));
    assert!(found.ids.contains(&non_rfc_variant));
    assert_eq!(found.malformed_entries, 4);
    assert_eq!(found.unsafe_entries, 2);
    assert_eq!(found.reparse_entries, 1);
    assert_eq!(
        fs::read(incomplete_path.join("pixels.rgba.tmp")).unwrap(),
        b"INCOMPLETE-SENTINEL"
    );
    assert_eq!(
        fs::read(staging.join("unrelated-sentinel")).unwrap(),
        b"UNRELATED-SENTINEL"
    );
    assert_eq!(
        fs::read(outside.join("sentinel")).unwrap(),
        b"OUTSIDE-SENTINEL"
    );
    drop(found);
    drop(staged);
    drop(staged_non_v4);
    drop(staged_non_rfc);
    drop(op);
    remove_junction(&link);
}

#[test]
fn t09l1_candidate_scan_reopen_share_failure_is_incomplete() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let available = OperationId::new(Uuid::new_v4());
    let blocked = OperationId::new(Uuid::new_v4());
    root.create_operation(available).unwrap();
    root.create_operation(blocked).unwrap();
    let directory = f.0.join("staging").join(blocked.as_uuid().to_string());
    // Only our synthetic UUIDv4 root: hold this directory with no sharing so
    // attribute enumeration succeeds but the no-follow candidate reopen fails.
    let blocker = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .custom_flags(0x0200_0000 | 0x0020_0000) // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
        .open(&directory)
        .unwrap();
    assert!(matches!(
        root.open_operation(blocked),
        Err(StagingError::AccessDenied)
    ));
    assert!(matches!(
        root.enumerate_candidates(),
        Err(StagingError::EnumerationIncomplete)
    ));
    drop(blocker);
    let complete = root.enumerate_candidates().unwrap();
    assert_eq!(complete.ids.len(), 2);
    assert!(complete.ids.contains(&available));
    assert!(complete.ids.contains(&blocked));
    assert_eq!(complete.unsafe_entries, 0);
}

#[test]
fn t09l1_candidate_scan_bounds_enumeration_without_cleanup() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let staging = f.0.join("staging");
    let sentinel = staging.join("sentinel");
    fs::write(&sentinel, b"KEEP-SYNTHETIC").unwrap();
    // The sentinel plus 4095 directories are exactly 4096 native entries.
    // Multiple records fit in each native buffer; both scans must consume batches.
    for i in 0..4095 {
        fs::create_dir(staging.join(format!("noncandidate-{i:04}"))).unwrap();
    }
    let exact = root.enumerate_candidates().unwrap();
    assert!(exact.ids.is_empty());
    assert_eq!(exact.malformed_entries, 4095);
    assert_eq!(exact.unsafe_entries, 1);
    fs::create_dir(staging.join("noncandidate-4095")).unwrap();
    assert!(matches!(
        root.enumerate_candidates(),
        Err(StagingError::EnumerationIncomplete)
    ));
    assert_eq!(fs::read(&sentinel).unwrap(), b"KEEP-SYNTHETIC");
    assert!(staging.join("noncandidate-4095").is_dir());
}

#[test]
fn t09l1_parallel_scans_share_one_pinned_root_without_cursor_interleaving() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let mut expected = std::collections::HashSet::new();
    for _ in 0..128 {
        let id = OperationId::new(Uuid::new_v4());
        root.create_operation(id).unwrap();
        expected.insert(id.as_uuid());
    }
    let barrier = Arc::new(Barrier::new(4));
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let barrier = Arc::clone(&barrier);
            let root = &root;
            let expected = &expected;
            scope.spawn(move || {
                for _ in 0..8 {
                    barrier.wait(); // simultaneous attempts on the very same &StagingRoot
                    let found = root.enumerate_candidates().unwrap();
                    assert_eq!(found.ids.len(), expected.len());
                    assert_eq!(found.malformed_entries, 0);
                    assert_eq!(found.unsafe_entries, 0);
                    assert_eq!(found.reparse_entries, 0);
                    let actual: std::collections::HashSet<_> =
                        found.ids.into_iter().map(|id| id.as_uuid()).collect();
                    assert_eq!(actual, *expected); // no skipped or duplicated IDs
                }
            });
        }
    });
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
