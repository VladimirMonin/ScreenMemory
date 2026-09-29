//! Focused unit tests of the pure staging-manifest-v1 validator (S02 §8.3).
//!
//! Every case works on in-memory bytes only: no directory, no file, no database and no
//! host process is involved, so a passing test proves the validator's decisions and
//! nothing about durability.
use super::{
    validate_staging, StagingError, StagingExpectations, MAX_CONTEXT_BYTES, MAX_MANIFEST_BYTES,
    STAGING_MANIFEST_SCHEMA_VERSION,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sm_application::storage::{CaptureContext, CaptureTarget, PhysicalBounds};
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId};
use uuid::Uuid;

const LIBRARY: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
const PRINCIPAL: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);
const OPERATION: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);
const RECORD: Uuid = Uuid::from_u128(0x4444_4444_4444_4444_4444_4444_4444_4444);
const ASSET: Uuid = Uuid::from_u128(0x5555_5555_5555_5555_5555_5555_5555_5555);
const FRAME: Uuid = Uuid::from_u128(0x6666_6666_6666_6666_6666_6666_6666_6666);
const PROFILE: &str =
    "webp-lossless-v1;libwebp=1.6.0;exact=1;lossless=1;method=4;quality=100;thread_level=0";

fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn context_json(width: u32, height: u32, target: CaptureTarget) -> String {
    CaptureContext {
        device: r"\\.\DISPLAY1".to_string(),
        app: "screenmemory-staging-test".to_string(),
        title: "synthetic".to_string(),
        target,
        sources: vec![target],
        bounds: PhysicalBounds {
            x: -10,
            y: -20,
            width,
            height,
        },
    }
    .to_json()
    .expect("synthetic context")
}

/// One consistent 4x2 staged capture: manifest document, context document, pixel bytes.
struct Fixture {
    manifest: Value,
    context: String,
    pixels: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let (width, height) = (4u32, 2u32);
        let pixels = vec![0x7f_u8; 32];
        let context = context_json(width, height, CaptureTarget::Region);
        let manifest = json!({
            "schema_version": STAGING_MANIFEST_SCHEMA_VERSION,
            "kind": "capture",
            "state": "staged",
            "library_id": LIBRARY.to_string(),
            "principal_id": PRINCIPAL.to_string(),
            "operation_id": OPERATION.to_string(),
            "record_id": RECORD.to_string(),
            "asset_id": ASSET.to_string(),
            "frame_id": FRAME.to_string(),
            "mode": "region",
            "width": width,
            "height": height,
            "stride": width * 4,
            "byte_length": u64::from(width * 4) * u64::from(height),
            "pixels_sha256": hex(&pixels),
            "context_sha256": hex(context.as_bytes()),
            "encoder_fingerprint": PROFILE,
            "captured_at": "2026-09-28T09:15:00.000Z",
        });
        Self {
            manifest,
            context,
            pixels,
        }
    }

    fn manifest_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.manifest).expect("synthetic manifest")
    }

    fn validate(&self) -> Result<super::ValidatedStagingManifest, StagingError> {
        validate_staging(
            &self.manifest_bytes(),
            self.context.as_bytes(),
            &self.pixels,
            expectations(),
        )
    }
}

fn expectations() -> StagingExpectations<'static> {
    StagingExpectations {
        library_id: LibraryId::new(LIBRARY),
        principal_id: PrincipalId::new(PRINCIPAL),
        operation_id: OperationId::new(OPERATION),
        mode: CaptureTarget::Region,
        encoder_fingerprint: PROFILE,
    }
}

#[test]
fn s02_t09b_02_valid_manifest_is_accepted_with_typed_identity() {
    let fixture = Fixture::new();
    let validated = fixture
        .validate()
        .expect("a consistent manifest is accepted");
    assert_eq!(validated.schema_version(), 1);
    assert_eq!(validated.library_id(), LibraryId::new(LIBRARY));
    assert_eq!(validated.principal_id(), PrincipalId::new(PRINCIPAL));
    assert_eq!(validated.operation_id(), OperationId::new(OPERATION));
    assert_eq!(validated.record_id(), sm_domain::RecordId::new(RECORD));
    assert_eq!(validated.asset_id(), AssetId::new(ASSET));
    assert_eq!(validated.frame_id(), FRAME);
    assert_eq!(validated.mode(), CaptureTarget::Region);
    assert_eq!((validated.width(), validated.height()), (4, 2));
    assert_eq!(validated.stride(), 16);
    assert_eq!(validated.byte_length(), 32);
    assert_eq!(validated.pixels_sha256(), hex(&fixture.pixels));
    assert_eq!(validated.context_sha256(), hex(fixture.context.as_bytes()));
    assert_eq!(validated.encoder_fingerprint(), PROFILE);
    assert_eq!(validated.captured_at(), "2026-09-28T09:15:00.000Z");
    assert_eq!(validated.context().bounds.width, 4);
    assert_eq!(validated.context().bounds.height, 2);
    assert_eq!(validated.context().target, CaptureTarget::Region);
    // The context document itself is never trusted to describe the operation identity.
    assert!(!fixture.context.contains(&OPERATION.to_string()));
}

#[test]
fn s02_t09b_03_truncated_and_oversized_pixel_buffers_are_refused() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_bytes();
    // One byte short of the declared stride*height.
    assert_eq!(
        validate_staging(
            &manifest,
            fixture.context.as_bytes(),
            &fixture.pixels[..31],
            expectations()
        )
        .unwrap_err(),
        StagingError::PixelLengthMismatch
    );
    // One byte too long is refused as well, not silently trimmed.
    let mut longer = fixture.pixels.clone();
    longer.push(0);
    assert_eq!(
        validate_staging(
            &manifest,
            fixture.context.as_bytes(),
            &longer,
            expectations()
        )
        .unwrap_err(),
        StagingError::PixelLengthMismatch
    );
    // Empty pixels can never satisfy a positive geometry.
    assert_eq!(
        validate_staging(&manifest, fixture.context.as_bytes(), &[], expectations()).unwrap_err(),
        StagingError::PixelLengthMismatch
    );
    // A declared length that is not stride*height is refused before any pixel read.
    let mut lying = fixture.manifest.clone();
    lying["byte_length"] = json!(31);
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&lying).unwrap(),
            fixture.context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::PixelLengthMismatch
    );
}

#[test]
fn s02_t09b_04_oversized_documents_are_refused_before_parsing() {
    let fixture = Fixture::new();
    let mut manifest = fixture.manifest_bytes();
    manifest.resize(MAX_MANIFEST_BYTES + 1, b' ');
    assert_eq!(
        validate_staging(
            &manifest,
            fixture.context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::ManifestBeyondLimit
    );
    let mut context = fixture.context.clone().into_bytes();
    context.resize(MAX_CONTEXT_BYTES + 1, b' ');
    assert_eq!(
        validate_staging(
            &fixture.manifest_bytes(),
            &context,
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::ContextBeyondLimit
    );
    // Non-UTF-8 bytes are a refusal, never a lossy read.
    let mut invalid = fixture.manifest_bytes();
    invalid.push(0xff);
    assert_eq!(
        validate_staging(
            &invalid,
            fixture.context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::NotUtf8
    );
}

#[test]
fn s02_t09b_05_foreign_identity_is_refused_against_trusted_expectations() {
    let fixture = Fixture::new();
    for (field, other) in [
        ("library_id", Uuid::from_u128(0xaaaa)),
        ("principal_id", Uuid::from_u128(0xbbbb)),
        ("operation_id", Uuid::from_u128(0xcccc)),
    ] {
        let mut manifest = fixture.manifest.clone();
        manifest[field] = json!(other.to_string());
        assert_eq!(
            validate_staging(
                &serde_json::to_vec(&manifest).unwrap(),
                fixture.context.as_bytes(),
                &fixture.pixels,
                expectations()
            )
            .unwrap_err(),
            StagingError::ForeignIdentity,
            "a manifest naming another {field} must not be adopted"
        );
    }
    // Nil and non-canonical identities never become trusted ids.
    let mut nil = fixture.manifest.clone();
    nil["record_id"] = json!(Uuid::nil().to_string());
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&nil).unwrap(),
            fixture.context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::NilIdentity
    );
    // Lowercase-only text: the same digest-bearing id written with uppercase hex digits
    // and a braced form is not the canonical spelling.
    let spellings = [
        Uuid::from_u128(0x5555_5555_5555_5555_5555_5555_5555_55ab)
            .to_string()
            .to_uppercase(),
        "{55555555-5555-5555-5555-55555555555b}".to_string(),
        "5555555555555555555555555555555b".to_string(),
    ];
    for spelling in spellings {
        let mut noncanonical = fixture.manifest.clone();
        noncanonical["asset_id"] = json!(spelling);
        assert_eq!(
            validate_staging(
                &serde_json::to_vec(&noncanonical).unwrap(),
                fixture.context.as_bytes(),
                &fixture.pixels,
                expectations()
            )
            .unwrap_err(),
            StagingError::NonCanonicalIdentity
        );
    }
}

#[test]
fn s02_t09b_06_unknown_and_path_fields_are_rejected() {
    let fixture = Fixture::new();
    for extra in [
        ("path", json!("../../outside/pixels.rgba")),
        ("pixels_path", json!("C:\\Users\\someone\\pixels.rgba")),
        ("absolute_path", json!("/etc/passwd")),
        ("canonical_path", json!("assets/YYYY-MM/x/original.webp")),
        ("unknown", json!(1)),
    ] {
        let mut manifest = fixture.manifest.clone();
        manifest[extra.0] = extra.1;
        assert_eq!(
            validate_staging(
                &serde_json::to_vec(&manifest).unwrap(),
                fixture.context.as_bytes(),
                &fixture.pixels,
                expectations()
            )
            .unwrap_err(),
            StagingError::UnknownField,
            "manifest member {} must be refused",
            extra.0
        );
    }
    // A missing member is malformed, not defaulted.
    let mut missing = fixture.manifest.clone();
    missing.as_object_mut().unwrap().remove("frame_id");
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&missing).unwrap(),
            fixture.context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::MalformedManifest
    );
    // A non-object document is never parsed as a manifest.
    assert_eq!(
        validate_staging(
            b"[1,2,3]",
            fixture.context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::MalformedManifest
    );
    assert_eq!(
        validate_staging(
            b"{",
            fixture.context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::MalformedManifest
    );
}

#[test]
fn s02_t09b_12_duplicate_raw_manifest_members_are_refused_before_identity_checks() {
    let fixture = Fixture::new();
    let ordinary = fixture.manifest_bytes();
    let ordinary = std::str::from_utf8(&ordinary).unwrap();
    let body = ordinary.strip_suffix('}').unwrap();
    let foreign = Uuid::from_u128(0xaaaa).to_string();
    let trusted = LIBRARY.to_string();
    let digest = hex(&fixture.pixels);

    // These are raw JSON documents: a Value fixture would erase duplicate keys.
    for raw in [
        format!(r#"{{"library_id":"{foreign}",{}"#, &ordinary[1..]),
        format!(r#"{body},"library_id":"{foreign}"}}"#),
        format!(r#"{body},"library_id":"{trusted}"}}"#),
        format!(r#"{body},"pixels_sha256":"{digest}"}}"#),
    ] {
        assert_eq!(
            validate_staging(
                raw.as_bytes(),
                fixture.context.as_bytes(),
                &fixture.pixels,
                expectations(),
            )
            .unwrap_err(),
            StagingError::MalformedManifest,
            "duplicate member must not be erased by JSON object decoding"
        );
    }
    assert!(
        fixture.validate().is_ok(),
        "ordinary document remains valid"
    );
}

#[test]
fn s02_t09b_07_geometry_zero_overflow_and_budget_are_enforced() {
    let fixture = Fixture::new();
    for (width, height, stride) in [
        (0u32, 2u32, 0u32),
        (4, 0, 16),
        (16_384, 1, 65_536),
        (1, 16_384, 4),
        // Sides are legal, the shared pixel budget is not.
        (15_000, 15_000, 60_000),
        // Stride must be exactly width*4, never a padded or narrowed row.
        (4, 2, 8),
        (4, 2, 17),
    ] {
        let mut manifest = fixture.manifest.clone();
        manifest["width"] = json!(width);
        manifest["height"] = json!(height);
        manifest["stride"] = json!(stride);
        manifest["byte_length"] = json!(u64::from(stride) * u64::from(height.max(1)));
        assert_eq!(
            validate_staging(
                &serde_json::to_vec(&manifest).unwrap(),
                fixture.context.as_bytes(),
                &fixture.pixels,
                expectations()
            )
            .unwrap_err(),
            StagingError::InvalidGeometry,
            "geometry {width}x{height} stride {stride} must be refused"
        );
    }
}

#[test]
fn s02_t09b_08_digest_and_profile_mismatches_are_refused() {
    let fixture = Fixture::new();
    // A single flipped pixel byte changes the digest.
    let mut tampered = fixture.pixels.clone();
    tampered[0] ^= 0x01;
    assert_eq!(
        validate_staging(
            &fixture.manifest_bytes(),
            fixture.context.as_bytes(),
            &tampered,
            expectations()
        )
        .unwrap_err(),
        StagingError::PixelDigestMismatch
    );
    // A declared digest that is not exact lowercase hex is refused even if the pixels
    // themselves are consistent.
    for bad in [PROFILE[..64].to_string(), "A".repeat(64), "z".repeat(64)] {
        let mut manifest = fixture.manifest.clone();
        manifest["pixels_sha256"] = json!(bad);
        assert_eq!(
            validate_staging(
                &serde_json::to_vec(&manifest).unwrap(),
                fixture.context.as_bytes(),
                &fixture.pixels,
                expectations()
            )
            .unwrap_err(),
            StagingError::PixelDigestMismatch
        );
    }
    // A context document swapped for other bytes fails its own digest.
    let other_context = context_json(4, 2, CaptureTarget::Monitor);
    assert_eq!(
        validate_staging(
            &fixture.manifest_bytes(),
            other_context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::ContextDigestMismatch
    );
    // The profile is compared with the trusted expectation, so a manifest cannot promote
    // a foreign or weaker encoder profile by naming it.
    let mut manifest = fixture.manifest.clone();
    manifest["encoder_fingerprint"] = json!(
        "webp-lossless-v1;libwebp=0.0.1;exact=0;lossless=0;method=0;quality=1;thread_level=1"
    );
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&manifest).unwrap(),
            fixture.context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::ProfileMismatch
    );
    // An empty or truncated profile never matches either.
    for bad in ["", "webp-lossless-v1"] {
        let mut manifest = fixture.manifest.clone();
        manifest["encoder_fingerprint"] = json!(bad);
        assert_eq!(
            validate_staging(
                &serde_json::to_vec(&manifest).unwrap(),
                fixture.context.as_bytes(),
                &fixture.pixels,
                expectations()
            )
            .unwrap_err(),
            StagingError::ProfileMismatch
        );
    }
}

#[test]
fn s02_t09b_09_version_kind_state_mode_and_time_are_closed_vocabularies() {
    let fixture = Fixture::new();
    let cases: [(&str, Value, StagingError); 5] = [
        (
            "schema_version",
            json!(2),
            StagingError::UnsupportedSchemaVersion,
        ),
        ("kind", json!("edit"), StagingError::WrongKind),
        ("state", json!("saved"), StagingError::NotStaged),
        ("state", json!("accepted_volatile"), StagingError::NotStaged),
        ("mode", json!("screen"), StagingError::UnknownMode),
    ];
    for (field, value, expected) in cases {
        let mut manifest = fixture.manifest.clone();
        manifest[field] = value;
        assert_eq!(
            validate_staging(
                &serde_json::to_vec(&manifest).unwrap(),
                fixture.context.as_bytes(),
                &fixture.pixels,
                expectations()
            )
            .unwrap_err(),
            expected,
            "member {field} is a closed vocabulary"
        );
    }
    // A valid but different target is a trusted-mode mismatch, not an adoption.
    let window_context = context_json(4, 2, CaptureTarget::ActiveWindow);
    let mut manifest = fixture.manifest.clone();
    manifest["mode"] = json!("active_window");
    manifest["context_sha256"] = json!(hex(window_context.as_bytes()));
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&manifest).unwrap(),
            window_context.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::ModeMismatch
    );
    // Timestamps must be the canonical UTC millisecond form.
    for bad in [
        "2026-09-28 09:15:00.000Z",
        "2026-09-28T09:15:00.000",
        "2026-09-28T09:15:00Z",
        "2026-13-28T09:15:00.000Z",
        "2026-09-28T24:15:00.000Z",
        "",
    ] {
        let mut manifest = fixture.manifest.clone();
        manifest["captured_at"] = json!(bad);
        assert_eq!(
            validate_staging(
                &serde_json::to_vec(&manifest).unwrap(),
                fixture.context.as_bytes(),
                &fixture.pixels,
                expectations()
            )
            .unwrap_err(),
            StagingError::InvalidTimestamp,
            "timestamp {bad:?} is not canonical"
        );
    }
}

#[test]
fn s02_t09b_10_context_must_be_storable_and_agree_with_the_manifest() {
    let fixture = Fixture::new();
    // An unknown member inside the context is refused by the application seam.
    let extra = fixture
        .context
        .replace("\"app\":", "\"path\":\"/etc/passwd\",\"app\":");
    let mut manifest = fixture.manifest.clone();
    manifest["context_sha256"] = json!(hex(extra.as_bytes()));
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&manifest).unwrap(),
            extra.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::InvalidContext
    );
    // An empty title is not a storable context either.
    let empty_title = fixture.context.replace("\"synthetic\"", "\"\"");
    let mut manifest = fixture.manifest.clone();
    manifest["context_sha256"] = json!(hex(empty_title.as_bytes()));
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&manifest).unwrap(),
            empty_title.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::InvalidContext
    );
    // Bounds that contradict the manifest geometry are a mismatch, even with a valid
    // digest over the supplied bytes.
    let wrong_bounds = context_json(8, 2, CaptureTarget::Region);
    let mut manifest = fixture.manifest.clone();
    manifest["context_sha256"] = json!(hex(wrong_bounds.as_bytes()));
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&manifest).unwrap(),
            wrong_bounds.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::ContextMismatch
    );
    // A context target that contradicts the manifest mode is a mismatch as well.
    let wrong_target = context_json(4, 2, CaptureTarget::Desktop);
    let mut manifest = fixture.manifest.clone();
    manifest["context_sha256"] = json!(hex(wrong_target.as_bytes()));
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&manifest).unwrap(),
            wrong_target.as_bytes(),
            &fixture.pixels,
            expectations()
        )
        .unwrap_err(),
        StagingError::ContextMismatch
    );
}

#[test]
fn s02_t09b_11_a_real_sized_frame_is_validated_from_borrowed_bytes() {
    let (width, height) = (512u32, 512u32);
    let stride = width * 4;
    let pixels: Vec<u8> = (0..stride * height).map(|i| (i % 251) as u8).collect();
    let context = context_json(width, height, CaptureTarget::Monitor);
    let manifest = json!({
        "schema_version": 1,
        "kind": "capture",
        "state": "staged",
        "library_id": LIBRARY.to_string(),
        "principal_id": PRINCIPAL.to_string(),
        "operation_id": OPERATION.to_string(),
        "record_id": RECORD.to_string(),
        "asset_id": ASSET.to_string(),
        "frame_id": FRAME.to_string(),
        "mode": "monitor",
        "width": width,
        "height": height,
        "stride": stride,
        "byte_length": u64::from(stride) * u64::from(height),
        "pixels_sha256": hex(&pixels),
        "context_sha256": hex(context.as_bytes()),
        "encoder_fingerprint": PROFILE,
        "captured_at": "2026-09-28T09:15:00.000Z",
    });
    let validated = validate_staging(
        &serde_json::to_vec(&manifest).unwrap(),
        context.as_bytes(),
        &pixels,
        StagingExpectations {
            mode: CaptureTarget::Monitor,
            ..expectations()
        },
    )
    .expect("a 512x512 frame is inside the documented ceilings");
    assert_eq!(
        validated.byte_length(),
        u64::from(stride) * u64::from(height)
    );
    assert_eq!(validated.context().target, CaptureTarget::Monitor);
    // The pixel digest is over the supplied bytes; re-validating a mutated buffer of the
    // same length fails, which is exactly what a damaged staged input must do.
    let mut damaged = pixels.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 0xff;
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&manifest).unwrap(),
            context.as_bytes(),
            &damaged,
            StagingExpectations {
                mode: CaptureTarget::Monitor,
                ..expectations()
            }
        )
        .unwrap_err(),
        StagingError::PixelDigestMismatch
    );
}
