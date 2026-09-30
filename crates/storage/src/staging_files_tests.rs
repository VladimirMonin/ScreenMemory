//! Focused tests of the manifest-last staging coordinator (S02 §8.3, §8.9 F02–F04).
//!
//! Every case runs in its own synthetic NTFS root under `.pi/temporary`: no SQLite worker,
//! no host, no wire, no clipboard and no real screen. Only the coordinator, the borrowed
//! lease capability and the fixed layout are exercised.
#![cfg(windows)]
use super::*;
use sha2::{Digest, Sha256};
use sm_application::storage::{CaptureContext, CaptureTarget, PhysicalBounds};
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId, RecordId};
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use std::{
    fs,
    path::{Path, PathBuf},
};

const LIBRARY: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
const PRINCIPAL: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);
const RECORD: Uuid = Uuid::from_u128(0x4444_4444_4444_4444_4444_4444_4444_4444);
const ASSET: Uuid = Uuid::from_u128(0x5555_5555_5555_5555_5555_5555_5555_5555);
const PROFILE: &str =
    "webp-lossless-v1;libwebp=1.6.0;exact=1;lossless=1;method=4;quality=100;thread_level=0";
const CAPTURED_AT: &str = "2026-09-28T09:15:00.000Z";

struct Fixture {
    root: PathBuf,
    operation: Uuid,
    frame: Uuid,
}

impl Fixture {
    /// Canonical synthetic base of every owned fixture root (no `\\?\` prefix).
    fn temporary_base() -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"))
    }
    fn new() -> Self {
        // Exact UUIDv4 fixture name: no PID reuse and no coarse wall-clock collision, and
        // create_dir stays atomic — an existing directory is an error, never adoption or
        // deletion of a foreign root.
        Self::create_exact(Self::temporary_base().join(Uuid::new_v4().to_string())).unwrap()
    }
    fn create_exact(root: PathBuf) -> std::io::Result<Self> {
        fs::create_dir(&root)?;
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(child))?;
        }
        Ok(Self {
            root,
            operation: Uuid::new_v4(),
            frame: Uuid::new_v4(),
        })
    }
    fn lease(&self) -> HostLease {
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &self.root,
                forbidden_donor_roots: &[],
            })
            .unwrap(),
        )
        .unwrap()
    }
    fn directory(&self) -> PathBuf {
        self.root.join("staging").join(self.operation.to_string())
    }
    fn context(&self, width: u32, height: u32) -> CaptureContext {
        CaptureContext {
            device: r"\\.\DISPLAY1".to_string(),
            app: "screenmemory-staging-test".to_string(),
            title: "synthetic".to_string(),
            target: CaptureTarget::Region,
            sources: vec![CaptureTarget::Region],
            bounds: PhysicalBounds {
                x: -10,
                y: -20,
                width,
                height,
            },
        }
    }
    fn pixels(width: u32, height: u32) -> Vec<u8> {
        (0..width * 4 * height).map(|i| (i % 251) as u8).collect()
    }
    fn expectations(&self) -> StagingExpectations<'static> {
        StagingExpectations {
            library_id: LibraryId::new(LIBRARY),
            principal_id: PrincipalId::new(PRINCIPAL),
            operation_id: OperationId::new(self.operation),
            mode: CaptureTarget::Region,
            encoder_fingerprint: PROFILE,
        }
    }
    fn input<'a>(&self, context: &'a CaptureContext, pixels: &'a [u8]) -> StageInput<'a> {
        StageInput {
            library_id: LibraryId::new(LIBRARY),
            principal_id: PrincipalId::new(PRINCIPAL),
            operation_id: OperationId::new(self.operation),
            record_id: RecordId::new(RECORD),
            asset_id: AssetId::new(ASSET),
            frame_id: self.frame,
            mode: CaptureTarget::Region,
            context,
            captured_at: CAPTURED_AT,
            encoder_fingerprint: PROFILE,
            pixels,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Only the exact root this fixture created atomically is removed.
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn stage_v2<'a>(
    f: &Fixture,
    root: &StagingRoot<'_>,
    context: &'a CaptureContext,
    pixels: &'a [u8],
    warnings: &'a [String],
) -> Result<ValidatedRecoveryEnvelope, StageFailure> {
    stage_capture_v2(
        root,
        StageInputV2 {
            capture: f.input(context, pixels),
            warnings,
        },
    )
}

fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn s02_t09i_01_v2_exact_document_and_owned_reopen_pixels() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let context = f.context(8, 4);
    let pixels = Fixture::pixels(8, 4);
    let warnings = vec!["topology_changed".into(), "display_unknown".into()];
    let staged = stage_v2(&f, &root, &context, &pixels, &warnings).unwrap();
    assert_eq!(staged.warnings(), Some(warnings.as_slice()));
    let dir = f.directory();
    let raw = fs::read(dir.join("manifest.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(manifest.as_object().unwrap().len(), 21);
    assert_eq!(manifest["schema_version"], 2);
    assert_eq!(manifest["warnings"], serde_json::json!(warnings));
    assert_eq!(manifest["provenance_kind"], "automatic");
    assert_eq!(manifest["path_policy"], RECOVERY_PATH_POLICY);
    assert_eq!(manifest["pixels_sha256"], hex(&pixels));
    assert_eq!(
        manifest["context_sha256"],
        hex(context.to_json().unwrap().as_bytes())
    );
    assert_eq!(fs::read(dir.join("pixels.rgba")).unwrap(), pixels);
    for temp in ["pixels.rgba.tmp", "context.json.tmp", "manifest.json.tmp"] {
        assert!(!dir.join(temp).exists());
    }
    let reopened = reopen_staged_v2(&root, f.expectations()).unwrap();
    assert_eq!(reopened.envelope(), &staged);
    assert_eq!(reopened.pixels(), pixels);
    assert_eq!(
        reopen_staged_v2(&root, f.expectations()).unwrap().pixels(),
        pixels
    );
    assert_eq!(
        stage_v2(&f, &root, &context, &pixels, &warnings).unwrap_err(),
        StageFailure::Storage(StagingRootError::AlreadyExists)
    );
}

#[test]
fn s02_t09i_02_empty_v2_distinct_from_published_v1_and_prewrite_rejection() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let context = f.context(8, 4);
    let pixels = Fixture::pixels(8, 4);
    let bad = vec!["bad/path".into()];
    assert_eq!(
        stage_v2(&f, &root, &context, &pixels, &bad).unwrap_err(),
        StageFailure::Rejected(StagingError::InvalidWarnings)
    );
    assert!(
        !f.directory().exists(),
        "invalid v2 input must not create operation"
    );
    let too_many = vec!["display_unknown".into(); 17];
    assert_eq!(
        stage_v2(&f, &root, &context, &pixels, &too_many).unwrap_err(),
        StageFailure::Rejected(StagingError::InvalidWarnings)
    );
    assert!(!f.directory().exists());
    let empty: Vec<String> = vec![];
    let ready = stage_v2(&f, &root, &context, &pixels, &empty).unwrap();
    assert_eq!(ready.warnings(), Some([].as_slice()));
    assert_eq!(
        reopen_staged_v2(&root, f.expectations())
            .unwrap()
            .envelope()
            .warnings(),
        Some([].as_slice())
    );
    let legacy = Fixture::new();
    let legacy_lease = legacy.lease();
    let legacy_root = legacy_lease.staging_root().unwrap();
    stage_capture(&legacy_root, legacy.input(&context, &pixels)).unwrap();
    assert_eq!(
        reopen_staged_v2(&legacy_root, legacy.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::UnsupportedSchemaVersion)
    );
    assert!(reopen_staged(&legacy_root, legacy.expectations()).is_ok());
    assert!(legacy.directory().join("manifest.json").exists());
}

#[test]
fn s02_t09j_v2_f04_published_without_db_is_reopenable_not_saved() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let context = f.context(8, 4);
    let pixels = Fixture::pixels(8, 4);
    let warnings = vec!["topology_changed".into(), "display_unknown".into()];
    assert_eq!(
        stage_capture_v2_f04(
            &root,
            StageInputV2 {
                capture: f.input(&context, &pixels),
                warnings: &warnings,
            }
        ),
        Err(StageFailure::Injected)
    );
    let reopened = reopen_staged_v2(&root, f.expectations()).unwrap();
    assert_eq!(reopened.envelope().warnings(), Some(warnings.as_slice()));
    assert_eq!(reopened.pixels(), pixels);
    assert!(f.directory().join("manifest.json").exists());
    // No SQLite worker, asset or Saved receipt is produced by the file primitive.
    assert!(!f.root.join("assets").exists());
}

#[test]
fn s02_t09i_03_foreign_tamper_missing_oversize_and_sentinel() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let context = f.context(8, 4);
    let pixels = Fixture::pixels(8, 4);
    let warnings = vec!["display_unknown".into()];
    stage_v2(&f, &root, &context, &pixels, &warnings).unwrap();
    let dir = f.directory();
    fs::write(dir.join("sentinel.bin"), b"UNTOUCHED").unwrap();
    let foreign = StagingExpectations {
        principal_id: PrincipalId::new(Uuid::new_v4()),
        ..f.expectations()
    };
    assert_eq!(
        reopen_staged_v2(&root, foreign).unwrap_err(),
        ReopenFailure::Rejected(StagingError::ForeignIdentity)
    );
    let profile = StagingExpectations {
        encoder_fingerprint: "foreign-profile",
        ..f.expectations()
    };
    assert_eq!(
        reopen_staged_v2(&root, profile).unwrap_err(),
        ReopenFailure::Rejected(StagingError::ProfileMismatch)
    );
    let original = fs::read(dir.join("manifest.json")).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    value["warnings"] = serde_json::json!(["bad/path"]);
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    assert_eq!(
        reopen_staged_v2(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::InvalidWarnings)
    );
    value["warnings"] = serde_json::json!(warnings);
    value["path_policy"] = serde_json::json!("../escape");
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    assert_eq!(
        reopen_staged_v2(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::InvalidPathPolicy)
    );
    fs::write(dir.join("manifest.json"), &original).unwrap();
    fs::write(dir.join("pixels.rgba"), vec![0; pixels.len()]).unwrap();
    assert_eq!(
        reopen_staged_v2(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::PixelDigestMismatch)
    );
    fs::write(dir.join("pixels.rgba"), &pixels).unwrap();
    let original_context = fs::read(dir.join("context.json")).unwrap();
    oversize_stable(&dir, "context.json", MAX_CONTEXT_BYTES as u64 + 1);
    assert_eq!(
        reopen_staged_v2(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::ContextBeyondLimit)
    );
    fs::write(dir.join("context.json"), original_context).unwrap();
    oversize_stable(&dir, "pixels.rgba", PIXELS_CEILING + 1);
    assert_eq!(
        reopen_staged_v2(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::PixelLengthMismatch)
    );
    fs::remove_file(dir.join("pixels.rgba")).unwrap();
    assert_eq!(
        reopen_staged_v2(&root, f.expectations()).unwrap_err(),
        ReopenFailure::MissingArtifact(StagedArtifact::PixelsRgba)
    );
    oversize_stable(&dir, "manifest.json", MAX_MANIFEST_BYTES as u64 + 1);
    assert_eq!(
        reopen_staged_v2(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::ManifestBeyondLimit)
    );
    assert_eq!(fs::read(dir.join("sentinel.bin")).unwrap(), b"UNTOUCHED");
}

#[test]
fn s02_t09b_13_stage_publishes_manifest_last_with_exact_documents() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let (width, height) = (8u32, 4u32);
    let context = f.context(width, height);
    let pixels = Fixture::pixels(width, height);

    let staged = stage_capture(&root, f.input(&context, &pixels)).expect("staged");
    assert_eq!(staged.operation_id(), OperationId::new(f.operation));
    assert_eq!(staged.library_id(), LibraryId::new(LIBRARY));
    assert_eq!(staged.principal_id(), PrincipalId::new(PRINCIPAL));
    assert_eq!(staged.record_id(), RecordId::new(RECORD));
    assert_eq!(staged.asset_id(), AssetId::new(ASSET));
    assert_eq!(staged.frame_id(), f.frame);
    assert_eq!(staged.mode(), CaptureTarget::Region);
    assert_eq!((staged.width(), staged.height()), (width, height));
    assert_eq!(staged.stride(), width * 4);
    assert_eq!(
        staged.byte_length(),
        u64::from(width * 4) * u64::from(height)
    );
    assert_eq!(staged.pixels_sha256(), hex(&pixels));
    assert_eq!(
        staged.context_sha256(),
        hex(context.to_json().unwrap().as_bytes())
    );
    assert_eq!(staged.encoder_fingerprint(), PROFILE);
    assert_eq!(staged.captured_at(), CAPTURED_AT);

    // The three fixed stable files exist and hold exactly the staged bytes.
    let dir = f.directory();
    assert_eq!(fs::read(dir.join("pixels.rgba")).unwrap(), pixels);
    assert_eq!(
        fs::read(dir.join("context.json")).unwrap(),
        context.to_json().unwrap().as_bytes()
    );
    let manifest_bytes = fs::read(dir.join("manifest.json")).unwrap();
    assert!(manifest_bytes.len() <= MAX_MANIFEST_BYTES);
    // Exactly the eighteen schema-v1 members, no path-like member.
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest.as_object().unwrap().len(), 18);
    assert_eq!(manifest["state"], "staged");
    assert_eq!(manifest["kind"], "capture");
    assert!(!manifest_bytes.windows(4).any(|w| w == b"path"));
    // No leftover temp files: each section published exactly once.
    for temp in ["pixels.rgba.tmp", "context.json.tmp", "manifest.json.tmp"] {
        assert!(!dir.join(temp).exists(), "temp {temp} must be published");
    }
}

#[test]
fn s02_t09b_14_f02_partial_pixels_temp_is_incomplete_and_untouched() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let (width, height) = (8u32, 4u32);
    let context = f.context(width, height);
    let pixels = Fixture::pixels(width, height);

    assert_eq!(
        stage_capture_seamed(
            &root,
            f.input(&context, &pixels),
            StageSeams {
                mid_pixels_temp: true,
                ..StageSeams::NONE
            }
        )
        .unwrap_err(),
        StageFailure::Injected
    );
    let dir = f.directory();
    let partial = fs::read(dir.join("pixels.rgba.tmp")).unwrap();
    assert_eq!(partial.len(), pixels.len() / 2);
    // No stable artifact was published, so the operation is incomplete, never "staged".
    assert!(!dir.join("pixels.rgba").exists());
    assert!(!dir.join("context.json").exists());
    assert!(!dir.join("manifest.json").exists());
    assert_eq!(
        reopen_staged(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Incomplete
    );
    // The partial input is not deleted by the failed reopen.
    assert_eq!(fs::read(dir.join("pixels.rgba.tmp")).unwrap(), partial);
}

#[test]
fn s02_t09b_15_f03_stable_files_without_manifest_are_incomplete_untouched() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let (width, height) = (8u32, 4u32);
    let context = f.context(width, height);
    let pixels = Fixture::pixels(width, height);

    assert_eq!(
        stage_capture_seamed(
            &root,
            f.input(&context, &pixels),
            StageSeams {
                stable_before_manifest: true,
                ..StageSeams::NONE
            }
        )
        .unwrap_err(),
        StageFailure::Injected
    );
    let dir = f.directory();
    // Both input files are stable; the manifest is absent.
    assert_eq!(fs::read(dir.join("pixels.rgba")).unwrap(), pixels);
    assert_eq!(
        fs::read(dir.join("context.json")).unwrap(),
        context.to_json().unwrap().as_bytes()
    );
    assert!(!dir.join("manifest.json").exists());
    // Distinct incomplete report: not a rejection, not a corrupt artifact.
    assert_eq!(
        reopen_staged(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Incomplete
    );
    // Nothing was deleted by the incomplete report.
    assert_eq!(fs::read(dir.join("pixels.rgba")).unwrap(), pixels);
}

#[test]
fn s02_t09b_16_f04_published_manifest_reopens_with_exact_ids_and_hash() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let (width, height) = (16u32, 8u32);
    let context = f.context(width, height);
    let pixels = Fixture::pixels(width, height);

    assert_eq!(
        stage_capture_seamed(
            &root,
            f.input(&context, &pixels),
            StageSeams {
                manifest_published: true,
                ..StageSeams::NONE
            }
        )
        .unwrap_err(),
        StageFailure::Injected
    );
    // A published manifest is a complete staged input; a reopen observes the exact IDs
    // and the exact pixel hash that were staged.
    let reopened = reopen_staged(&root, f.expectations()).expect("published staged input");
    assert_eq!(reopened.operation_id(), OperationId::new(f.operation));
    assert_eq!(reopened.record_id(), RecordId::new(RECORD));
    assert_eq!(reopened.asset_id(), AssetId::new(ASSET));
    assert_eq!(reopened.frame_id(), f.frame);
    assert_eq!(reopened.pixels_sha256(), hex(&pixels));
    assert_eq!(
        reopened.context_sha256(),
        hex(context.to_json().unwrap().as_bytes())
    );
    assert_eq!(reopened.captured_at(), CAPTURED_AT);

    // Repeating the reopen adds no capture, no duplicate and no new file.
    let before = directory_listing(&f.directory());
    let again = reopen_staged(&root, f.expectations()).expect("idempotent reopen");
    assert_eq!(again, reopened);
    assert_eq!(directory_listing(&f.directory()), before);
}

#[test]
fn s02_t09b_17_foreign_identity_profile_and_corruption_are_rejected() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let (width, height) = (8u32, 4u32);
    let context = f.context(width, height);
    let pixels = Fixture::pixels(width, height);
    stage_capture(&root, f.input(&context, &pixels)).expect("staged");

    // A foreign principal under a trusted operation ID must never adopt this operation.
    let foreign = StagingExpectations {
        principal_id: PrincipalId::new(Uuid::from_u128(0xabcd)),
        ..f.expectations()
    };
    assert_eq!(
        reopen_staged(&root, foreign).unwrap_err(),
        ReopenFailure::Rejected(StagingError::ForeignIdentity)
    );
    // A different trusted encoder profile is a profile mismatch, not an adoption.
    let profile = StagingExpectations {
        encoder_fingerprint: "webp-lossless-v1;libwebp=9.9.9",
        ..f.expectations()
    };
    assert_eq!(
        reopen_staged(&root, profile).unwrap_err(),
        ReopenFailure::Rejected(StagingError::ProfileMismatch)
    );
    // Unknown operation directory: distinct from incomplete-but-present.
    let unknown = StagingExpectations {
        operation_id: OperationId::new(Uuid::new_v4()),
        ..f.expectations()
    };
    assert_eq!(
        reopen_staged(&root, unknown).unwrap_err(),
        ReopenFailure::Incomplete
    );

    // Corruption: a published manifest whose pixel bytes changed is refused, not repaired.
    let dir = f.directory();
    let manifest_before = fs::read(dir.join("manifest.json")).unwrap();
    fs::write(dir.join("pixels.rgba"), vec![0u8; pixels.len()]).unwrap();
    assert_eq!(
        reopen_staged(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::PixelDigestMismatch)
    );
    // Truncation: a published manifest without its pixels is a distinct missing artifact.
    fs::write(dir.join("pixels.rgba"), &pixels[..pixels.len() - 1]).unwrap();
    assert_eq!(
        reopen_staged(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::PixelLengthMismatch)
    );
    fs::remove_file(dir.join("pixels.rgba")).unwrap();
    assert_eq!(
        reopen_staged(&root, f.expectations()).unwrap_err(),
        ReopenFailure::MissingArtifact(StagedArtifact::PixelsRgba)
    );
    // The published manifest itself was never rewritten by any refused reopen.
    assert_eq!(
        fs::read(dir.join("manifest.json")).unwrap(),
        manifest_before
    );
}

#[test]
fn s02_t09b_18_unknown_sentinel_is_preserved_and_duplicate_operation_refused() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let (width, height) = (8u32, 4u32);
    let context = f.context(width, height);
    let pixels = Fixture::pixels(width, height);

    // A sentinel written before staging survives the whole write and reopen.
    let dir = f.directory();
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("unknown.bin"), b"SYNTHETIC-UNKNOWN").unwrap();
    assert_eq!(
        stage_capture(&root, f.input(&context, &pixels)).unwrap_err(),
        StageFailure::Storage(StagingRootError::AlreadyExists)
    );
    assert_eq!(
        fs::read(dir.join("unknown.bin")).unwrap(),
        b"SYNTHETIC-UNKNOWN"
    );
    fs::remove_file(dir.join("unknown.bin")).unwrap();
    fs::remove_dir(&dir).unwrap();

    // A fresh operation commits, and a second stage with the same trusted ID is refused
    // instead of creating a duplicate capture.
    let staged = stage_capture(&root, f.input(&context, &pixels)).expect("staged");
    fs::write(dir.join("unknown.bin"), b"SYNTHETIC-UNKNOWN").unwrap();
    assert_eq!(
        stage_capture(&root, f.input(&context, &pixels)).unwrap_err(),
        StageFailure::Storage(StagingRootError::AlreadyExists)
    );
    let reopened = reopen_staged(&root, f.expectations()).expect("still staged");
    assert_eq!(reopened, staged);
    assert_eq!(
        fs::read(dir.join("unknown.bin")).unwrap(),
        b"SYNTHETIC-UNKNOWN"
    );
}

/// Exact documented pixel ceiling of the lease capability (`StagingTempKind::PixelsRgba`).
const PIXELS_CEILING: u64 = 512 * 1024 * 1024;

/// Replace an owned published leaf with an explicitly marked NTFS sparse file.
/// Marking and verifying sparseness MUST precede extending the logical length: plain
/// `SetEndOfFile` does not guarantee that an oversized fixture avoids disk allocation.
fn oversize_stable(dir: &Path, leaf: &str, len: u64) {
    use std::os::windows::{fs::MetadataExt, io::AsRawHandle};

    // CTL_CODE(FILE_DEVICE_FILE_SYSTEM=9, function=49, METHOD_BUFFERED=0,
    // FILE_ANY_ACCESS=0), per Win32 FSCTL_SET_SPARSE. DeviceIoControl takes a
    // synchronous file HANDLE, DWORD byte counts, and a non-null bytes-returned pointer.
    const FSCTL_SET_SPARSE: u32 = (9 << 16) | (49 << 2);
    const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;
    #[link(name = "Kernel32")]
    extern "system" {
        #[link_name = "DeviceIoControl"]
        fn device_io_control(
            handle: *mut std::ffi::c_void,
            code: u32,
            in_buffer: *mut std::ffi::c_void,
            in_len: u32,
            out_buffer: *mut std::ffi::c_void,
            out_len: u32,
            bytes_returned: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
    }

    fs::remove_file(dir.join(leaf)).unwrap();
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(leaf))
        .unwrap();
    let mut bytes_returned = 0u32;
    // SAFETY: file owns a valid open synchronous HANDLE for this entire call; the
    // control code takes no input/output buffers, and bytes_returned is writable.
    let marked = unsafe {
        device_io_control(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        marked,
        0,
        "FSCTL_SET_SPARSE unsupported/failed before set_len: {}",
        std::io::Error::last_os_error()
    );
    assert_ne!(
        file.metadata().unwrap().file_attributes() & FILE_ATTRIBUTE_SPARSE_FILE,
        0,
        "sparse attribute missing before set_len"
    );
    file.set_len(len).unwrap();
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.len(), len);
    assert_ne!(
        metadata.file_attributes() & FILE_ATTRIBUTE_SPARSE_FILE,
        0,
        "sparse attribute lost after set_len"
    );
}

#[test]
fn s02_t09b_20_oversize_existing_artifacts_are_refused_not_reported_missing() {
    let f = Fixture::new();
    let lease = f.lease();
    let root = lease.staging_root().unwrap();
    let (width, height) = (8u32, 4u32);
    let context = f.context(width, height);
    let pixels = Fixture::pixels(width, height);
    stage_capture(&root, f.input(&context, &pixels)).expect("staged");
    let dir = f.directory();
    let original = ["manifest.json", "context.json", "pixels.rgba"]
        .map(|leaf| (leaf, fs::read(dir.join(leaf)).unwrap()));
    let restore = |leaf: &str| {
        let (_, bytes) = original.iter().find(|(name, _)| *name == leaf).unwrap();
        fs::write(dir.join(leaf), bytes).unwrap();
    };

    // An existing manifest beyond its schema ceiling is present-but-oversized, never a
    // false absence. The lease refuses it from verified metadata length before any read.
    oversize_stable(&dir, "manifest.json", MAX_MANIFEST_BYTES as u64 + 1);
    assert_eq!(
        reopen_staged(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::ManifestBeyondLimit)
    );
    assert_eq!(
        fs::metadata(dir.join("manifest.json")).unwrap().len(),
        MAX_MANIFEST_BYTES as u64 + 1,
        "an oversize refusal repairs and deletes nothing"
    );
    restore("manifest.json");
    assert!(reopen_staged(&root, f.expectations()).is_ok());

    // An existing oversize context is the typed context refusal, again not an absence.
    oversize_stable(&dir, "context.json", MAX_CONTEXT_BYTES as u64 + 1);
    assert_eq!(
        reopen_staged(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::ContextBeyondLimit)
    );
    assert_eq!(
        fs::metadata(dir.join("context.json")).unwrap().len(),
        MAX_CONTEXT_BYTES as u64 + 1
    );
    restore("context.json");
    assert!(reopen_staged(&root, f.expectations()).is_ok());

    // An existing oversize pixel artifact is refused as a pixel length refusal; the
    // explicitly marked sparse file is refused from metadata without a physical fill.
    oversize_stable(&dir, "pixels.rgba", PIXELS_CEILING + 1);
    assert_eq!(
        reopen_staged(&root, f.expectations()).unwrap_err(),
        ReopenFailure::Rejected(StagingError::PixelLengthMismatch)
    );

    // A truly absent artifact stays the distinct missing report: absence and oversize are
    // never conflated.
    fs::remove_file(dir.join("pixels.rgba")).unwrap();
    assert_eq!(
        reopen_staged(&root, f.expectations()).unwrap_err(),
        ReopenFailure::MissingArtifact(StagedArtifact::PixelsRgba)
    );
}

#[test]
fn s02_t09b_21_fixture_root_is_exact_uuid_and_never_adopts_or_deletes() {
    // Every fixture root is one exact UUIDv4 directory: repeated draws never alias a PID
    // and wall-clock value, and a collision is an error, not adoption or cleanup.
    let base = Fixture::temporary_base();
    assert!(base.is_dir());
    let first = Fixture::new();
    let second = Fixture::new();
    assert_ne!(first.root, second.root);
    for fixture in [&first, &second] {
        assert_eq!(fixture.root.parent().unwrap(), base.as_path());
        let name = fixture
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let parsed = Uuid::parse_str(&name).expect("fixture name is an exact UUID");
        assert_eq!(parsed.get_version_num(), 4);
        assert_eq!(name, parsed.to_string());
        assert!(fixture.lease().staging_root().is_ok());
    }

    // A colliding name refuses atomically and leaves foreign content exactly as it was.
    let occupied = first.root.join("data").join("foreign.bin");
    fs::write(&occupied, b"SYNTHETIC-FOREIGN").unwrap();
    assert!(Fixture::create_exact(first.root.clone()).is_err());
    assert_eq!(fs::read(&occupied).unwrap(), b"SYNTHETIC-FOREIGN");
    assert!(
        first.root.is_dir(),
        "a collision never deletes the foreign root"
    );
}

/// Sorted directory listing; used to prove a refused/repeated reopen changes nothing.
fn directory_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            format!(
                "{}:{}",
                entry.file_name().to_string_lossy(),
                entry.metadata().unwrap().len()
            )
        })
        .collect();
    names.sort();
    names
}
