//! Pure synthetic v2 recovery-envelope tests; no file, SQL, host or pixel copy.
use super::{
    validate_recovery_envelope, validate_staging, StagingError, StagingExpectations,
    ValidatedRecoveryEnvelope, MAX_CONTEXT_BYTES, MAX_MANIFEST_BYTES, RECOVERY_PATH_POLICY,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sm_application::storage::{CaptureContext, CaptureTarget, PhysicalBounds, Provenance};
use sm_domain::{LibraryId, OperationId, PrincipalId};
use uuid::Uuid;

const LIBRARY: Uuid = Uuid::from_u128(0x11111111111111111111111111111111);
const PRINCIPAL: Uuid = Uuid::from_u128(0x22222222222222222222222222222222);
const OPERATION: Uuid = Uuid::from_u128(0x33333333333333333333333333333333);
const PROFILE: &str = "webp-lossless-synthetic-profile";

fn expected() -> StagingExpectations<'static> {
    StagingExpectations {
        library_id: LibraryId::new(LIBRARY),
        principal_id: PrincipalId::new(PRINCIPAL),
        operation_id: OperationId::new(OPERATION),
        mode: CaptureTarget::Region,
        encoder_fingerprint: PROFILE,
    }
}

fn fixture() -> (Value, String, Vec<u8>) {
    let pixels = vec![17; 32];
    let context = CaptureContext {
        device: "synthetic-device".into(),
        app: "synthetic-app".into(),
        title: "synthetic-title".into(),
        target: CaptureTarget::Region,
        sources: vec![CaptureTarget::Region],
        bounds: PhysicalBounds {
            x: -4,
            y: 2,
            width: 4,
            height: 2,
        },
    }
    .to_json()
    .unwrap();
    let manifest = json!({
        "schema_version": 2, "kind": "capture", "state": "staged",
        "library_id": LIBRARY.to_string(), "principal_id": PRINCIPAL.to_string(),
        "operation_id": OPERATION.to_string(),
        "record_id": Uuid::from_u128(4).to_string(),
        "asset_id": Uuid::from_u128(5).to_string(),
        "frame_id": Uuid::from_u128(6).to_string(),
        "mode": "region", "width": 4, "height": 2, "stride": 16, "byte_length": 32,
        "pixels_sha256": format!("{:x}", Sha256::digest(&pixels)),
        "context_sha256": format!("{:x}", Sha256::digest(context.as_bytes())),
        "encoder_fingerprint": PROFILE, "captured_at": "2026-09-28T09:15:00.000Z",
        "warnings": ["display_unknown", "topology_changed"],
        "provenance_kind": "automatic", "path_policy": RECOVERY_PATH_POLICY,
    });
    (manifest, context, pixels)
}

fn check(
    manifest: &Value,
    context: &str,
    pixels: &[u8],
) -> Result<ValidatedRecoveryEnvelope, StagingError> {
    validate_recovery_envelope(
        &serde_json::to_vec(manifest).unwrap(),
        context.as_bytes(),
        pixels,
        expected(),
    )
}

#[test]
fn s02_t09h_01_v2_exact_receipt_and_derived_provenance() {
    let (manifest, context, pixels) = fixture();
    let result = check(&manifest, &context, &pixels).unwrap();
    assert!(result.is_recovery_ready());
    assert_eq!(result.schema_version(), 2);
    assert_eq!(
        result.warnings().unwrap(),
        ["display_unknown", "topology_changed"]
    );
    assert_eq!(result.path_policy(), Some(RECOVERY_PATH_POLICY));
    assert_eq!(result.staging().context().target, CaptureTarget::Region);
    assert_eq!(result.staging().captured_at(), "2026-09-28T09:15:00.000Z");
    assert_eq!(
        result.automatic_provenance(),
        Some(Provenance::Automatic {
            when: "2026-09-28T09:15:00.000Z".into(),
            source: CaptureTarget::Region,
        })
    );
}

#[test]
fn s02_t09h_02_v1_is_accepted_but_unrecoverable_even_when_v2_empty_warnings() {
    let (mut manifest, context, pixels) = fixture();
    manifest["warnings"] = json!([]);
    assert_eq!(
        check(&manifest, &context, &pixels).unwrap().warnings(),
        Some([].as_slice())
    );
    for name in ["warnings", "provenance_kind", "path_policy"] {
        manifest.as_object_mut().unwrap().remove(name);
    }
    manifest["schema_version"] = json!(1);
    let raw = serde_json::to_vec(&manifest).unwrap();
    assert!(validate_staging(&raw, context.as_bytes(), &pixels, expected()).is_ok());
    let legacy = validate_recovery_envelope(&raw, context.as_bytes(), &pixels, expected()).unwrap();
    assert!(matches!(legacy, ValidatedRecoveryEnvelope::LegacyV1(_)));
    assert!(!legacy.is_recovery_ready());
    assert_eq!(legacy.warnings(), None);
    assert_eq!(legacy.automatic_provenance(), None);
    assert_eq!(legacy.path_policy(), None);
    let (v2, context, pixels) = fixture();
    assert_eq!(
        validate_staging(
            &serde_json::to_vec(&v2).unwrap(),
            context.as_bytes(),
            &pixels,
            expected()
        ),
        Err(StagingError::UnknownField)
    );
}

#[test]
fn s02_t09h_03_closed_members_types_and_duplicates() {
    let (manifest, context, pixels) = fixture();
    for field in ["warnings", "provenance_kind", "path_policy", "frame_id"] {
        let mut altered = manifest.clone();
        altered.as_object_mut().unwrap().remove(field);
        assert_eq!(
            check(&altered, &context, &pixels),
            Err(StagingError::MalformedManifest)
        );
    }
    for field in ["path", "pixels_path", "canonical_path", "unknown"] {
        let mut altered = manifest.clone();
        altered[field] = json!("../outside");
        assert_eq!(
            check(&altered, &context, &pixels),
            Err(StagingError::UnknownField)
        );
    }
    for (field, replacement, error) in [
        (
            "warnings",
            json!("display_unknown"),
            StagingError::InvalidWarnings,
        ),
        (
            "warnings",
            json!(["bad/path"]),
            StagingError::InvalidWarnings,
        ),
        ("warnings", json!(["UPPER"]), StagingError::InvalidWarnings),
        (
            "warnings",
            json!(vec!["x"; 17]),
            StagingError::InvalidWarnings,
        ),
        (
            "warnings",
            json!(["x".repeat(65)]),
            StagingError::InvalidWarnings,
        ),
        (
            "provenance_kind",
            json!("manual"),
            StagingError::InvalidProvenanceKind,
        ),
        (
            "provenance_kind",
            json!(null),
            StagingError::MalformedManifest,
        ),
        (
            "path_policy",
            json!("../../outside"),
            StagingError::InvalidPathPolicy,
        ),
        ("path_policy", json!(7), StagingError::MalformedManifest),
        (
            "schema_version",
            json!(3),
            StagingError::UnsupportedSchemaVersion,
        ),
    ] {
        let mut altered = manifest.clone();
        altered[field] = replacement;
        assert_eq!(check(&altered, &context, &pixels), Err(error), "{field}");
    }
    let raw = serde_json::to_string(&manifest).unwrap();
    for duplicate in ["warnings", "schema_version", "path_policy", "record_id"] {
        let extra = format!(r#", "{duplicate}": null}}"#);
        let tampered = format!("{}{}", &raw[..raw.len() - 1], extra);
        assert_eq!(
            validate_recovery_envelope(
                tampered.as_bytes(),
                context.as_bytes(),
                &pixels,
                expected()
            ),
            Err(StagingError::MalformedManifest)
        );
    }
}

#[test]
fn s02_t09h_04_identity_profile_mode_geometry_and_digests() {
    let (manifest, context, pixels) = fixture();
    let mut foreign = manifest.clone();
    foreign["principal_id"] = json!(Uuid::from_u128(99).to_string());
    assert_eq!(
        check(&foreign, &context, &pixels),
        Err(StagingError::ForeignIdentity)
    );
    let mut changed = expected();
    changed.encoder_fingerprint = "different-trusted-profile";
    assert_eq!(
        validate_recovery_envelope(
            &serde_json::to_vec(&manifest).unwrap(),
            context.as_bytes(),
            &pixels,
            changed
        ),
        Err(StagingError::ProfileMismatch)
    );
    for (field, replacement, error) in [
        ("mode", json!("monitor"), StagingError::ModeMismatch),
        ("stride", json!(15), StagingError::InvalidGeometry),
        ("width", json!(0), StagingError::InvalidGeometry),
        ("byte_length", json!(31), StagingError::PixelLengthMismatch),
        (
            "pixels_sha256",
            json!("0".repeat(64)),
            StagingError::PixelDigestMismatch,
        ),
        (
            "context_sha256",
            json!("0".repeat(64)),
            StagingError::ContextDigestMismatch,
        ),
        (
            "captured_at",
            json!("tomorrow"),
            StagingError::InvalidTimestamp,
        ),
    ] {
        let mut altered = manifest.clone();
        altered[field] = replacement;
        assert_eq!(check(&altered, &context, &pixels), Err(error), "{field}");
    }
    let mut changed_context: Value = serde_json::from_str(&context).unwrap();
    changed_context["bounds"]["width"] = json!(3);
    let changed_context = serde_json::to_string(&changed_context).unwrap();
    let mut matching_hash = manifest.clone();
    matching_hash["context_sha256"] =
        json!(format!("{:x}", Sha256::digest(changed_context.as_bytes())));
    assert_eq!(
        check(&matching_hash, &changed_context, &pixels),
        Err(StagingError::ContextMismatch)
    );
    assert_eq!(
        check(&manifest, &context, &pixels[..31]),
        Err(StagingError::PixelLengthMismatch)
    );
    let mut modified_pixels = pixels.clone();
    modified_pixels[0] ^= 1;
    assert_eq!(
        check(&manifest, &context, &modified_pixels),
        Err(StagingError::PixelDigestMismatch)
    );
    let mut wrong_target: Value = serde_json::from_str(&context).unwrap();
    wrong_target["target"] = json!("monitor");
    let wrong_target = serde_json::to_string(&wrong_target).unwrap();
    matching_hash["context_sha256"] =
        json!(format!("{:x}", Sha256::digest(wrong_target.as_bytes())));
    assert_eq!(
        check(&matching_hash, &wrong_target, &pixels),
        Err(StagingError::ContextMismatch)
    );
}

#[test]
fn s02_t09h_05_document_limits_without_pixel_allocation() {
    let (manifest, context, pixels) = fixture();
    let mut raw = serde_json::to_vec(&manifest).unwrap();
    raw.resize(MAX_MANIFEST_BYTES + 1, b' ');
    assert_eq!(
        validate_recovery_envelope(&raw, context.as_bytes(), &pixels, expected()),
        Err(StagingError::ManifestBeyondLimit)
    );
    let mut long = context.into_bytes();
    long.resize(MAX_CONTEXT_BYTES + 1, b' ');
    assert_eq!(
        validate_recovery_envelope(
            &serde_json::to_vec(&manifest).unwrap(),
            &long,
            &pixels,
            expected()
        ),
        Err(StagingError::ContextBeyondLimit)
    );
}
