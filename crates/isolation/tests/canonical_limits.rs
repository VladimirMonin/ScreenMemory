#![cfg(windows)]
use sm_domain::RecordId;
use sm_isolation::{
    CanonicalError, CanonicalPlan, HostLease, LibraryRootResolver, RootSource, MAX_CANONICAL_BYTES,
};
use std::{
    fs,
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
        let path = parent.join(Uuid::new_v4().to_string());
        fs::create_dir(&path).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(path.join(child)).unwrap();
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
fn t09n1_bounded_final_and_single_link_refusal_without_read_or_cleanup() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.canonical_root().unwrap();
    let id = Uuid::new_v4();
    let plan = CanonicalPlan::new(RecordId::new(id), "2026-09-28T09:00:00.000Z").unwrap();
    let temp = root.begin(&plan).unwrap();
    drop(temp);
    let final_path =
        f.0.join("assets/2026-09")
            .join(id.to_string())
            .join("original.webp");
    let file = fs::File::create(&final_path).unwrap();
    file.set_len(MAX_CANONICAL_BYTES + 1).unwrap(); // sparse synthetic, no 128MiB allocation
    drop(file);
    assert!(matches!(
        root.open_final(&plan),
        Err(CanonicalError::TooLarge)
    ));
    assert!(matches!(root.begin(&plan), Err(CanonicalError::Existing))); // no large read
    assert_eq!(
        fs::metadata(&final_path).unwrap().len(),
        MAX_CANONICAL_BYTES + 1
    );
    // Another record proves a multi-linked final is rejected rather than adopted.
    let second_id = Uuid::new_v4();
    let second = CanonicalPlan::new(RecordId::new(second_id), "2026-09-28T09:00:00.000Z").unwrap();
    drop(root.begin(&second).unwrap());
    let second_path =
        f.0.join("assets/2026-09")
            .join(second_id.to_string())
            .join("original.webp");
    fs::hard_link(&final_path, &second_path).unwrap();
    assert!(matches!(
        root.open_final(&second),
        Err(CanonicalError::UnsafeEntry)
    ));
    assert!(matches!(
        root.begin(&second),
        Err(CanonicalError::UnsafeEntry)
    ));
    assert_eq!(
        fs::metadata(&second_path).unwrap().len(),
        MAX_CANONICAL_BYTES + 1
    );
}
