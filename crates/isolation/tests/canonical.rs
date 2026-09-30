#![cfg(windows)]
use sm_domain::RecordId;
use sm_isolation::{
    CanonicalError, CanonicalPlan, HostLease, LibraryRootResolver, RootSource, MAX_CANONICAL_BYTES,
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
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        let path = base.join(Uuid::new_v4().to_string());
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
    fn assets(&self) -> PathBuf {
        self.0.join("assets")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn plan() -> CanonicalPlan {
    CanonicalPlan::new(RecordId::new(Uuid::new_v4()), "2026-09-28T09:00:00.000Z").unwrap()
}
fn only_record(root: &Path) -> PathBuf {
    let month = root.join("2026-09");
    fs::read_dir(month).unwrap().next().unwrap().unwrap().path()
}
fn junction(link: &Path, target: &Path) {
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "junction raw exit {:?}",
        output.status.code()
    );
}
fn remove_junction(link: &Path) {
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "rmdir"])
        .arg(link)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn t09n1_unique_collision_incomplete_and_reacquire() {
    let f = Fixture::new();
    let sentinel = f.0.join("outside-sentinel");
    fs::write(&sentinel, b"OUTSIDE-KEEP").unwrap();
    let planned = plan();
    let lease = f.lease();
    let root = lease.canonical_root().unwrap();
    let mut half = root.begin(&planned).unwrap();
    half.write_all(b"HALF").unwrap();
    assert_eq!(half.publish(), Err(CanonicalError::NotSynced)); // incomplete temp retained
    let record = only_record(&f.assets());
    assert!(!record.join("original.webp").exists());
    assert_eq!(fs::read_dir(&record).unwrap().count(), 1);
    let mut complete = root.begin(&planned).unwrap();
    complete.write_all(b"SYNTHETIC-WEBP-CONTAINER").unwrap();
    complete.sync_all().unwrap();
    complete.publish().unwrap();
    assert_eq!(
        root.open_final(&planned).unwrap().read_all().unwrap(),
        b"SYNTHETIC-WEBP-CONTAINER"
    );
    assert_eq!(fs::read_dir(&record).unwrap().count(), 2); // incomplete attempt not cleaned
    assert!(matches!(
        root.begin(&planned),
        Err(CanonicalError::Existing)
    ));
    let different_id = Uuid::new_v4();
    let different =
        CanonicalPlan::new(RecordId::new(different_id), "2026-09-28T09:00:00.000Z").unwrap();
    let mut contender = root.begin(&different).unwrap();
    contender.write_all(b"OTHER").unwrap();
    contender.sync_all().unwrap();
    let other_record = f.assets().join("2026-09").join(different_id.to_string());
    fs::write(other_record.join("original.webp"), b"COLLISION-SENTINEL").unwrap();
    assert_eq!(contender.publish(), Err(CanonicalError::Existing));
    assert_eq!(
        fs::read(other_record.join("original.webp")).unwrap(),
        b"COLLISION-SENTINEL"
    );
    assert_eq!(fs::read_dir(&other_record).unwrap().count(), 2);
    drop(root);
    drop(lease);
    let again = f.lease();
    assert_eq!(
        again
            .canonical_root()
            .unwrap()
            .open_final(&planned)
            .unwrap()
            .read_all()
            .unwrap(),
        b"SYNTHETIC-WEBP-CONTAINER"
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"OUTSIDE-KEEP");
}
#[test]
fn t09n1_synced_empty_and_externally_changed_length_retained() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.canonical_root().unwrap();
    let empty_plan = plan();
    let mut empty = root.begin(&empty_plan).unwrap();
    empty.sync_all().unwrap();
    let empty_record = only_record(&f.assets());
    assert_eq!(empty.publish(), Err(CanonicalError::UnsafeEntry));
    assert!(!empty_record.join("original.webp").exists());
    assert_eq!(fs::read_dir(&empty_record).unwrap().count(), 1);

    let id = Uuid::new_v4();
    let changed_plan = CanonicalPlan::new(RecordId::new(id), "2026-09-28T09:00:00.000Z").unwrap();
    let mut changed = root.begin(&changed_plan).unwrap();
    changed.write_all(b"ORIGINAL").unwrap();
    changed.sync_all().unwrap();
    let changed_record = f.assets().join("2026-09").join(id.to_string());
    let temp_path = fs::read_dir(&changed_record)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let other = fs::OpenOptions::new().write(true).open(&temp_path).unwrap();
    other.set_len(3).unwrap();
    other.sync_all().unwrap();
    drop(other);
    assert_eq!(changed.publish(), Err(CanonicalError::UnsafeEntry));
    assert!(!changed_record.join("original.webp").exists());
    assert_eq!(fs::read_dir(&changed_record).unwrap().count(), 1);
    assert_eq!(fs::metadata(temp_path).unwrap().len(), 3);
}

#[test]
fn t09n1_plan_bounds_and_unsafe_ancestors() {
    for date in [
        "2026-00-28T09:00:00.000Z",
        "2026-13-28T09:00:00.000Z",
        "2026-9-28T09:00:00.000Z",
        "2026-09-28T09:00:00Z",
        "0000-01-01T00:00:00.000Z",
        "2026-02-29T09:00:00.000Z",
        "2026-02-31T09:00:00.000Z",
        "2026-04-31T09:00:00.000Z",
        "1900-02-29T09:00:00.000Z",
    ] {
        assert!(matches!(
            CanonicalPlan::new(RecordId::new(Uuid::new_v4()), date),
            Err(CanonicalError::InvalidPlan)
        ));
    }
    for date in [
        "2024-02-29T09:00:00.000Z",
        "2000-02-29T09:00:00.000Z",
        "2026-04-30T09:00:00.000Z",
    ] {
        CanonicalPlan::new(RecordId::new(Uuid::new_v4()), date).unwrap();
    }
    for id in [
        Uuid::nil(),
        Uuid::parse_str("00000000-0000-1000-8000-000000000001").unwrap(),
        Uuid::parse_str("00000000-0000-4000-0000-000000000001").unwrap(),
    ] {
        assert!(matches!(
            CanonicalPlan::new(RecordId::new(id), "2026-09-28T09:00:00.000Z"),
            Err(CanonicalError::InvalidPlan)
        ));
    }
    let f = Fixture::new();
    let outside = f.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"KEEP").unwrap();
    let lease = f.lease();
    junction(&f.assets(), &outside);
    assert!(matches!(
        lease.canonical_root(),
        Err(CanonicalError::UnsafeEntry)
    ));
    remove_junction(&f.assets());
    let root = lease.canonical_root().unwrap();
    let month = f.assets().join("2026-09");
    junction(&month, &outside);
    assert!(matches!(
        root.begin(&plan()),
        Err(CanonicalError::UnsafeEntry)
    ));
    remove_junction(&month);
    fs::write(&month, b"NONDIR").unwrap();
    assert!(matches!(
        root.begin(&plan()),
        Err(CanonicalError::UnsafeEntry)
    ));
    fs::remove_file(&month).unwrap();
    fs::create_dir(&month).unwrap();
    let blocked_id = Uuid::new_v4();
    let blocked_record = month.join(blocked_id.to_string());
    fs::write(&blocked_record, b"NONDIR-RECORD").unwrap();
    let blocked_plan =
        CanonicalPlan::new(RecordId::new(blocked_id), "2026-09-28T09:00:00.000Z").unwrap();
    assert!(matches!(
        root.begin(&blocked_plan),
        Err(CanonicalError::UnsafeEntry)
    ));
    assert_eq!(fs::read(&blocked_record).unwrap(), b"NONDIR-RECORD");
    let valid_id = Uuid::new_v4();
    let p = CanonicalPlan::new(RecordId::new(valid_id), "2026-09-28T09:00:00.000Z").unwrap();
    let mut temp = root.begin(&p).unwrap();
    let record = month.join(valid_id.to_string());
    assert!(fs::rename(&record, record.with_extension("moved")).is_err());
    assert!(fs::rename(f.assets(), f.0.join("assets-moved")).is_err());
    let chunk = [b'X'; 1024 * 1024];
    assert_eq!(MAX_CANONICAL_BYTES % chunk.len() as u64, 0);
    for _ in 0..MAX_CANONICAL_BYTES / chunk.len() as u64 {
        temp.write_all(&chunk).unwrap();
    }
    assert!(temp.write_all(b"X").is_err()); // exact 128 MiB allowed; next byte refused
    assert_eq!(temp.publish(), Err(CanonicalError::NotSynced));
    assert!(!record.join("original.webp").exists());
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"KEEP");
}

#[test]
fn t09n1_reparse_record_and_final_refused() {
    let f = Fixture::new();
    let outside = f.0.join("outside");
    fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("sentinel");
    fs::write(&sentinel, b"KEEP").unwrap();
    let lease = f.lease();
    let root = lease.canonical_root().unwrap();
    let p = plan();
    let temp = root.begin(&p).unwrap();
    let record = only_record(&f.assets());
    drop(temp);
    let final_path = record.join("original.webp");
    match std::os::windows::fs::symlink_file(&sentinel, &final_path) {
        Ok(()) => {
            assert!(matches!(
                root.open_final(&p),
                Err(CanonicalError::UnsafeEntry)
            ));
            assert!(matches!(root.begin(&p), Err(CanonicalError::UnsafeEntry)));
            fs::remove_file(&final_path).unwrap();
        }
        Err(e) if matches!(e.raw_os_error(), Some(5 | 1314)) => {
            eprintln!(
                "T09N1-FINAL-SYMLINK NOT_RUN: raw OS code {:?}",
                e.raw_os_error()
            );
        }
        Err(e) => panic!(
            "T09N1-FINAL-SYMLINK unexpected raw OS code {:?}",
            e.raw_os_error()
        ),
    }
    // Current record handle is released; a new plan targets an unsafe record entry.
    let record_id = Uuid::new_v4();
    let p2 = CanonicalPlan::new(RecordId::new(record_id), "2026-09-28T09:00:00.000Z").unwrap();
    let record2 = f.assets().join("2026-09").join(record_id.to_string());
    junction(&record2, &outside);
    assert!(matches!(root.begin(&p2), Err(CanonicalError::UnsafeEntry)));
    remove_junction(&record2);
    assert_eq!(fs::read(sentinel).unwrap(), b"KEEP");
}
