//! Pure staging-manifest-v1 and recovery-envelope-v2 validators (S02 §8.3).
//!
//! This module is deliberately **not** a staging coordinator: it performs no filesystem,
//! SQL or wire IO, it never creates a directory, it never writes or deletes a file and it
//! publishes no durable receipt. It answers exactly one question, on caller-supplied
//! bounded bytes: *does this manifest/context/pixel triple agree with trusted identity?*
//! The original v1 validator remains separate from the v2 recovery-ready envelope.
//!
//! Ownership rules encoded here:
//! * the library, principal, operation and capture mode are **trusted inputs** of the host;
//!   a manifest that names any other identity is refused instead of adopted;
//! * the encoder fingerprint is compared with the trusted expected profile, never taken
//!   from the manifest itself;
//! * the manifest carries no path at all — file names come from validated UUIDs and a
//!   fixed layout, so a hostile manifest cannot describe where recovery should read;
//! * geometry, packed stride and byte length are recomputed with checked arithmetic and
//!   must stay inside the documented ceilings, so a truncated or oversized buffer is
//!   refused before any pixel byte is read as content;
//! * digests are computed over the borrowed slices, so validating a large accepted frame
//!   never copies the pixel buffer.
//!
//! `staged` here means exactly one thing: the supplied manifest names the state that a
//! written-but-unpublished staging input would have. It is **not** a durable receipt,
//! and no caller may treat a [`ValidatedStagingManifest`] as proof that anything reached
//! a disk.
use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use sm_application::{
    capture::{MAX_CAPTURE_PIXELS, MAX_CAPTURE_SIDE},
    storage::{is_canonical_utc_timestamp, CaptureContext, CaptureTarget},
};
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId, RecordId};
use std::collections::HashSet;
use uuid::Uuid;

/// The original staging entry point accepts only schema version 1; never upgrade it.
pub const STAGING_MANIFEST_SCHEMA_VERSION: u32 = 1;
/// Separate recovery envelope schema; never interpreted by the original v1 validator.
pub const RECOVERY_ENVELOPE_SCHEMA_VERSION: u32 = 2;
/// Closed description of the current commit-month/record-id/original.webp layout.
pub const RECOVERY_PATH_POLICY: &str = "commit_month_record_original_v1";
/// Largest accepted manifest document (S02 §8.3).
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// Largest accepted capture-context document (S02 §8.3).
pub const MAX_CONTEXT_BYTES: usize = 256 * 1024;

/// Fixed manifest kind of one capture operation.
const CAPTURE_KIND: &str = "capture";
/// Fixed reached-state boundary of an operation whose pixels and context are staged.
const STAGED_STATE: &str = "staged";
/// Exact length of a lowercase hexadecimal SHA-256.
const SHA256_HEX_LENGTH: usize = 64;

/// Exact set of manifest-v1 members. Anything else — including any path-like member a
/// damaged or hostile manifest might carry — is an unknown field.
const MANIFEST_FIELDS: [&str; 18] = [
    "schema_version",
    "kind",
    "state",
    "library_id",
    "principal_id",
    "operation_id",
    "record_id",
    "asset_id",
    "frame_id",
    "mode",
    "width",
    "height",
    "stride",
    "byte_length",
    "pixels_sha256",
    "context_sha256",
    "encoder_fingerprint",
    "captured_at",
];

/// Closed diagnostic vocabulary of the validator. No variant carries a path, a title, a
/// digest value or any other manifest content: a refused manifest stays untrusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagingError {
    /// Manifest document is larger than the documented schema limit.
    ManifestBeyondLimit,
    /// Context document is larger than the documented schema limit.
    ContextBeyondLimit,
    /// Manifest or context bytes are not UTF-8 text.
    NotUtf8,
    /// Manifest JSON is unparseable, not an object, or a member has the wrong type.
    MalformedManifest,
    /// A member outside the fixed manifest-v1 set is present.
    UnknownField,
    /// `schema_version` is not the one supported version.
    UnsupportedSchemaVersion,
    /// `kind` is not the fixed capture kind.
    WrongKind,
    /// `state` is not the fixed staged state.
    NotStaged,
    /// An identity is not canonical lower-hyphen UUID text.
    NonCanonicalIdentity,
    /// An identity is the nil UUID.
    NilIdentity,
    /// Library, principal or operation id differs from the trusted expectation.
    ForeignIdentity,
    /// `mode` is outside the closed capture-target vocabulary.
    UnknownMode,
    /// `mode` disagrees with the trusted capture mode.
    ModeMismatch,
    /// Zero, over-side, over-pixel-budget or overflowing geometry, or a stride that is
    /// not exactly `width * 4`.
    InvalidGeometry,
    /// Declared byte length is not `stride * height` or does not match the pixel buffer.
    PixelLengthMismatch,
    /// `pixels_sha256` is not exact lowercase hex or does not match the supplied bytes.
    PixelDigestMismatch,
    /// `context_sha256` is not exact lowercase hex or does not match the supplied bytes.
    ContextDigestMismatch,
    /// Manifest fingerprint differs from the trusted expected encoder profile.
    ProfileMismatch,
    /// `captured_at` is not a canonical UTC millisecond timestamp.
    InvalidTimestamp,
    /// Context bytes are not a valid stored capture context (unknown field, empty title,
    /// duplicate source or otherwise refused by the application seam).
    InvalidContext,
    /// Context target or context bounds disagree with the manifest geometry/mode.
    ContextMismatch,
    /// The required v2 warning list is not a valid bounded stable-code receipt.
    InvalidWarnings,
    /// The v2 provenance rule is not the closed automatic derivation.
    InvalidProvenanceKind,
    /// The v2 path policy differs from the current fixed canonical layout.
    InvalidPathPolicy,
}

/// Raw manifest-v1 document. Identities stay text here so every refusal is typed.
///
/// The exact field/type checks remain in `decode_manifest`; the map visitor rejects
/// repeated raw member names before the document can become a JSON object.
struct ManifestWire {
    schema_version: u32,
    kind: String,
    state: String,
    library_id: String,
    principal_id: String,
    operation_id: String,
    record_id: String,
    asset_id: String,
    frame_id: String,
    mode: String,
    width: u32,
    height: u32,
    stride: u32,
    byte_length: u64,
    pixels_sha256: String,
    context_sha256: String,
    encoder_fingerprint: String,
    captured_at: String,
}

/// Trusted expectations the host already holds for this operation. None of these values
/// is ever read from the manifest; the manifest has to *agree* with them.
#[derive(Clone, Copy, Debug)]
pub struct StagingExpectations<'a> {
    pub library_id: LibraryId,
    pub principal_id: PrincipalId,
    pub operation_id: OperationId,
    /// Typed capture mode of the accepted request.
    pub mode: CaptureTarget,
    /// Encoder profile fingerprint of the trusted configuration, not of the document.
    pub encoder_fingerprint: &'a str,
}

/// One manifest that passed every schema-v1 check. Holding this value is a statement
/// about the supplied bytes only: no file was written, no row was committed and nothing
/// is durable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedStagingManifest {
    library_id: LibraryId,
    principal_id: PrincipalId,
    operation_id: OperationId,
    record_id: RecordId,
    asset_id: AssetId,
    frame_id: Uuid,
    mode: CaptureTarget,
    width: u32,
    height: u32,
    stride: u32,
    byte_length: u64,
    pixels_sha256: String,
    context_sha256: String,
    encoder_fingerprint: String,
    captured_at: String,
    context: CaptureContext,
}

impl ValidatedStagingManifest {
    pub const fn schema_version(&self) -> u32 {
        STAGING_MANIFEST_SCHEMA_VERSION
    }
    pub const fn library_id(&self) -> LibraryId {
        self.library_id
    }
    pub const fn principal_id(&self) -> PrincipalId {
        self.principal_id
    }
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }
    pub const fn record_id(&self) -> RecordId {
        self.record_id
    }
    pub const fn asset_id(&self) -> AssetId {
        self.asset_id
    }
    /// Frame identity of the frozen frame; validated as a canonical non-nil UUID.
    pub const fn frame_id(&self) -> Uuid {
        self.frame_id
    }
    pub const fn mode(&self) -> CaptureTarget {
        self.mode
    }
    pub const fn width(&self) -> u32 {
        self.width
    }
    pub const fn height(&self) -> u32 {
        self.height
    }
    /// Exact tight packed stride of the staged RGBA bytes (`width * 4`).
    pub const fn stride(&self) -> u32 {
        self.stride
    }
    /// Exact staged pixel byte length (`stride * height`).
    pub const fn byte_length(&self) -> u64 {
        self.byte_length
    }
    /// SHA-256 of the staged normalized RGBA bytes, recomputed from the supplied slice.
    pub fn pixels_sha256(&self) -> &str {
        &self.pixels_sha256
    }
    /// SHA-256 of the staged context document bytes, recomputed from the supplied slice.
    pub fn context_sha256(&self) -> &str {
        &self.context_sha256
    }
    /// Fingerprint that already matched the trusted expected profile.
    pub fn encoder_fingerprint(&self) -> &str {
        &self.encoder_fingerprint
    }
    pub fn captured_at(&self) -> &str {
        &self.captured_at
    }
    /// Re-validated stored capture context that agrees with the manifest geometry.
    pub const fn context(&self) -> &CaptureContext {
        &self.context
    }
}

/// Validate one manifest/context/pixel triple against trusted expectations.
///
/// The pixel buffer is only borrowed: no copy of it is made, and the declared geometry is
/// checked before the digests are computed. Every refusal is a typed [`StagingError`].
pub fn validate_staging(
    manifest_bytes: &[u8],
    context_bytes: &[u8],
    pixels: &[u8],
    expectations: StagingExpectations<'_>,
) -> Result<ValidatedStagingManifest, StagingError> {
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(StagingError::ManifestBeyondLimit);
    }
    if context_bytes.len() > MAX_CONTEXT_BYTES {
        return Err(StagingError::ContextBeyondLimit);
    }
    let manifest_text = std::str::from_utf8(manifest_bytes).map_err(|_| StagingError::NotUtf8)?;
    let context_text = std::str::from_utf8(context_bytes).map_err(|_| StagingError::NotUtf8)?;

    let document: UniqueManifestObject =
        serde_json::from_str(manifest_text).map_err(|_| StagingError::MalformedManifest)?;
    let manifest = decode_manifest(&document.0)?;

    if manifest.schema_version != STAGING_MANIFEST_SCHEMA_VERSION {
        return Err(StagingError::UnsupportedSchemaVersion);
    }
    if manifest.kind != CAPTURE_KIND {
        return Err(StagingError::WrongKind);
    }
    if manifest.state != STAGED_STATE {
        return Err(StagingError::NotStaged);
    }

    // Trusted identity first: a foreign manifest is refused before its geometry or its
    // pixel bytes are examined at all.
    let library_id = LibraryId::new(canonical_uuid(&manifest.library_id)?);
    let principal_id = PrincipalId::new(canonical_uuid(&manifest.principal_id)?);
    let operation_id = OperationId::new(canonical_uuid(&manifest.operation_id)?);
    if library_id != expectations.library_id
        || principal_id != expectations.principal_id
        || operation_id != expectations.operation_id
    {
        return Err(StagingError::ForeignIdentity);
    }
    let record_id = RecordId::new(canonical_uuid(&manifest.record_id)?);
    let asset_id = AssetId::new(canonical_uuid(&manifest.asset_id)?);
    let frame_id = canonical_uuid(&manifest.frame_id)?;

    let mode = CaptureTarget::parse(&manifest.mode).map_err(|_| StagingError::UnknownMode)?;
    if mode != expectations.mode {
        return Err(StagingError::ModeMismatch);
    }

    // Checked geometry: zero, over-budget and overflowing declarations fail closed here,
    // long before anything is read as pixel content.
    let (width, height) = (manifest.width, manifest.height);
    if width == 0 || height == 0 || width > MAX_CAPTURE_SIDE || height > MAX_CAPTURE_SIDE {
        return Err(StagingError::InvalidGeometry);
    }
    let pixel_count = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(StagingError::InvalidGeometry)?;
    if pixel_count > MAX_CAPTURE_PIXELS {
        return Err(StagingError::InvalidGeometry);
    }
    let stride = width.checked_mul(4).ok_or(StagingError::InvalidGeometry)?;
    if manifest.stride != stride {
        return Err(StagingError::InvalidGeometry);
    }
    let byte_length = u64::from(stride)
        .checked_mul(u64::from(height))
        .ok_or(StagingError::InvalidGeometry)?;
    if manifest.byte_length != byte_length || pixels.len() as u64 != byte_length {
        return Err(StagingError::PixelLengthMismatch);
    }

    if digest_matches(&manifest.pixels_sha256, &sha256_hex(pixels)) {
        // Exact recomputed digest: nothing to record beyond the accepted value.
    } else {
        return Err(StagingError::PixelDigestMismatch);
    }
    if !digest_matches(&manifest.context_sha256, &sha256_hex(context_bytes)) {
        return Err(StagingError::ContextDigestMismatch);
    }

    if manifest.encoder_fingerprint != expectations.encoder_fingerprint {
        return Err(StagingError::ProfileMismatch);
    }
    if !is_canonical_utc_timestamp(&manifest.captured_at) {
        return Err(StagingError::InvalidTimestamp);
    }

    // The stored context is re-validated by the application seam and must describe the
    // same target and the same geometry as the manifest.
    let context =
        CaptureContext::from_json(context_text).map_err(|_| StagingError::InvalidContext)?;
    if context.target != mode || context.bounds.width != width || context.bounds.height != height {
        return Err(StagingError::ContextMismatch);
    }

    Ok(ValidatedStagingManifest {
        library_id,
        principal_id,
        operation_id,
        record_id,
        asset_id,
        frame_id,
        mode,
        width,
        height,
        stride,
        byte_length,
        pixels_sha256: manifest.pixels_sha256,
        context_sha256: manifest.context_sha256,
        encoder_fingerprint: manifest.encoder_fingerprint,
        captured_at: manifest.captured_at,
        context,
    })
}

/// Deserialize the top-level object once, retaining the raw stream's duplicate-key
/// distinction until every member has been checked. Nested values still use serde_json;
/// the manifest schema accepts only scalar members and `decode_manifest` enforces that.
struct UniqueManifestObject(serde_json::Value);

impl<'de> Deserialize<'de> for UniqueManifestObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueObjectVisitor;

        impl<'de> Visitor<'de> for UniqueObjectVisitor {
            type Value = UniqueManifestObject;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a manifest object with unique members")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut seen = HashSet::new();
                let mut object = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(M::Error::custom("duplicate manifest member"));
                    }
                    let value = map.next_value::<serde_json::Value>()?;
                    object.insert(key, value);
                }
                Ok(UniqueManifestObject(serde_json::Value::Object(object)))
            }
        }

        deserializer.deserialize_map(UniqueObjectVisitor)
    }
}

/// Strict manifest decoder: the member set must be exactly schema v1 (a path-like member
/// has no home in a manifest and cannot be smuggled in as an "extra" field) and every
/// member must carry the declared JSON type.
fn decode_manifest(document: &serde_json::Value) -> Result<ManifestWire, StagingError> {
    let object = document
        .as_object()
        .ok_or(StagingError::MalformedManifest)?;
    if object
        .keys()
        .any(|key| !MANIFEST_FIELDS.contains(&key.as_str()))
    {
        return Err(StagingError::UnknownField);
    }
    // Every schema-v1 member is mandatory; a missing member is never defaulted.
    if object.len() != MANIFEST_FIELDS.len() {
        return Err(StagingError::MalformedManifest);
    }
    let text = |key: &str| -> Result<String, StagingError> {
        object
            .get(key)
            .and_then(|value| match value {
                serde_json::Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .ok_or(StagingError::MalformedManifest)
    };
    // Counts are exact, never saturating: `u32::MAX + 1` is refused, not wrapped.
    let unsigned = |key: &str, ceiling: u64| -> Result<u64, StagingError> {
        object
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .filter(|value| *value <= ceiling)
            .ok_or(StagingError::MalformedManifest)
    };
    let u32_field =
        |key: &str| -> Result<u32, StagingError> { Ok(unsigned(key, u64::from(u32::MAX))? as u32) };
    Ok(ManifestWire {
        schema_version: u32_field("schema_version")?,
        kind: text("kind")?,
        state: text("state")?,
        library_id: text("library_id")?,
        principal_id: text("principal_id")?,
        operation_id: text("operation_id")?,
        record_id: text("record_id")?,
        asset_id: text("asset_id")?,
        frame_id: text("frame_id")?,
        mode: text("mode")?,
        width: u32_field("width")?,
        height: u32_field("height")?,
        stride: u32_field("stride")?,
        byte_length: unsigned("byte_length", u64::MAX)?,
        pixels_sha256: text("pixels_sha256")?,
        context_sha256: text("context_sha256")?,
        encoder_fingerprint: text("encoder_fingerprint")?,
        captured_at: text("captured_at")?,
    })
}

/// Canonical lower-hyphen UUID text of a non-nil identity.
fn canonical_uuid(raw: &str) -> Result<Uuid, StagingError> {
    let id = Uuid::parse_str(raw).map_err(|_| StagingError::NonCanonicalIdentity)?;
    if id.to_string() != raw {
        return Err(StagingError::NonCanonicalIdentity);
    }
    if id.is_nil() {
        return Err(StagingError::NilIdentity);
    }
    Ok(id)
}

/// `expected` must be exact lowercase SHA-256 hex that equals the recomputed digest.
fn digest_matches(expected: &str, computed: &str) -> bool {
    expected.len() == SHA256_HEX_LENGTH
        && expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && expected == computed
}

/// Digest of a borrowed slice; the input is never copied.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Versioned pure validation result. V1 is valid staged input but lacks receipt warnings;
/// only a fully validated v2 carries the recovery-ready marker (no IO/durability claim).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidatedRecoveryEnvelope {
    LegacyV1(ValidatedStagingManifest),
    RecoveryReadyV2 {
        staging: ValidatedStagingManifest,
        warnings: Vec<String>,
    },
}

impl ValidatedRecoveryEnvelope {
    pub const fn is_recovery_ready(&self) -> bool {
        matches!(self, Self::RecoveryReadyV2 { .. })
    }

    pub const fn schema_version(&self) -> u32 {
        match self {
            Self::LegacyV1(_) => STAGING_MANIFEST_SCHEMA_VERSION,
            Self::RecoveryReadyV2 { .. } => RECOVERY_ENVELOPE_SCHEMA_VERSION,
        }
    }

    /// Exact ordered receipt codes; `None` means unknown, not an empty receipt.
    pub fn warnings(&self) -> Option<&[String]> {
        match self {
            Self::LegacyV1(_) => None,
            Self::RecoveryReadyV2 { warnings, .. } => Some(warnings),
        }
    }

    pub const fn staging(&self) -> &ValidatedStagingManifest {
        match self {
            Self::LegacyV1(staging) | Self::RecoveryReadyV2 { staging, .. } => staging,
        }
    }

    /// V2 provenance is `Automatic { when: captured_at, source: context.target }`.
    pub fn automatic_provenance(&self) -> Option<sm_application::storage::Provenance> {
        match self {
            Self::LegacyV1(_) => None,
            Self::RecoveryReadyV2 { staging, .. } => {
                Some(sm_application::storage::Provenance::Automatic {
                    when: staging.captured_at().to_owned(),
                    source: staging.context().target,
                })
            }
        }
    }

    /// No path comes from the manifest: the policy is an exact closed discriminator.
    pub const fn path_policy(&self) -> Option<&'static str> {
        match self {
            Self::LegacyV1(_) => None,
            Self::RecoveryReadyV2 { .. } => Some(RECOVERY_PATH_POLICY),
        }
    }
}

/// Validate a versioned in-memory envelope without reading a file, cloning pixels, or
/// synthesizing missing v1 receipt fields. Used by the v2 stage/reopen file lane;
/// matching an intent does not wire recovery into host startup or commit.
pub fn validate_recovery_envelope(
    manifest_bytes: &[u8],
    context_bytes: &[u8],
    pixels: &[u8],
    expectations: StagingExpectations<'_>,
) -> Result<ValidatedRecoveryEnvelope, StagingError> {
    use sm_application::storage::{warnings_from_json, warnings_to_json};
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(StagingError::ManifestBeyondLimit);
    }
    if context_bytes.len() > MAX_CONTEXT_BYTES {
        return Err(StagingError::ContextBeyondLimit);
    }
    let text = std::str::from_utf8(manifest_bytes).map_err(|_| StagingError::NotUtf8)?;
    let document: UniqueManifestObject =
        serde_json::from_str(text).map_err(|_| StagingError::MalformedManifest)?;
    let object = document
        .0
        .as_object()
        .ok_or(StagingError::MalformedManifest)?;
    let version = object
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or(StagingError::MalformedManifest)?;
    match version {
        1 => validate_staging(manifest_bytes, context_bytes, pixels, expectations)
            .map(ValidatedRecoveryEnvelope::LegacyV1),
        2 => {
            if expectations.encoder_fingerprint.is_empty()
                || expectations.encoder_fingerprint.len() > 512
            {
                return Err(StagingError::ProfileMismatch);
            }
            const V2_FIELDS: [&str; 3] = ["warnings", "provenance_kind", "path_policy"];
            if object.keys().any(|key| {
                !MANIFEST_FIELDS.contains(&key.as_str()) && !V2_FIELDS.contains(&key.as_str())
            }) {
                return Err(StagingError::UnknownField);
            }
            if object.len() != MANIFEST_FIELDS.len() + V2_FIELDS.len() {
                return Err(StagingError::MalformedManifest);
            }
            let provenance_kind = object
                .get("provenance_kind")
                .and_then(serde_json::Value::as_str)
                .ok_or(StagingError::MalformedManifest)?;
            if provenance_kind != "automatic" {
                return Err(StagingError::InvalidProvenanceKind);
            }
            let path_policy = object
                .get("path_policy")
                .and_then(serde_json::Value::as_str)
                .ok_or(StagingError::MalformedManifest)?;
            if path_policy != RECOVERY_PATH_POLICY {
                return Err(StagingError::InvalidPathPolicy);
            }
            let warning_value = object
                .get("warnings")
                .ok_or(StagingError::MalformedManifest)?;
            // Application is the sole owner of the warning vocabulary/bounds.
            let warning_json =
                serde_json::to_string(warning_value).map_err(|_| StagingError::InvalidWarnings)?;
            let warnings =
                warnings_from_json(&warning_json).map_err(|_| StagingError::InvalidWarnings)?;
            warnings_to_json(&warnings).map_err(|_| StagingError::InvalidWarnings)?;

            // Reuse the *unchanged* v1 geometry, hash, identity, context, timestamp and
            // trusted-profile checks on the same borrowed pixel slice. The temporary
            // projection is bounded JSON metadata, not a pixel clone or v1 upgrade.
            let mut base = object.clone();
            for field in V2_FIELDS {
                base.remove(field);
            }
            base.insert("schema_version".into(), serde_json::Value::from(1));
            let projected =
                serde_json::to_vec(&base).map_err(|_| StagingError::MalformedManifest)?;
            let staging = validate_staging(&projected, context_bytes, pixels, expectations)?;
            Ok(ValidatedRecoveryEnvelope::RecoveryReadyV2 { staging, warnings })
        }
        _ => Err(StagingError::UnsupportedSchemaVersion),
    }
}

#[cfg(test)]
#[path = "staging_recovery_tests.rs"]
mod staging_recovery_tests;

#[cfg(test)]
#[path = "staging_tests.rs"]
mod staging_tests;

#[cfg(test)]
mod purity {
    /// The validator must stay free of filesystem, SQLite and process dependencies. The
    /// needles are assembled at runtime so this test's own text cannot satisfy them.
    #[test]
    fn s02_t09b_01_validator_has_no_io_dependency() {
        let source = include_str!("staging.rs");
        for needle in [
            ["std", "::fs"].concat(),
            ["Path", "Buf"].concat(),
            ["std", "::process"].concat(),
            ["rus", "qlite"].concat(),
            ["std", "::net"].concat(),
        ] {
            assert!(
                !source.contains(&needle),
                "staging validator must stay pure; found {needle}"
            );
        }
        assert!(!source.contains(&["durable", "_input"].concat()));
    }
}
