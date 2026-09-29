//! Manifest-last staging file coordinator (S02 §8.3, §8.9 F02–F04).
//!
//! This module is the **only** place that turns a trusted in-memory accepted capture into
//! the three files of one staging operation. It deliberately holds no path, no connection
//! and no wire: every name is derived by the borrowed [`StagingRoot`] capability from the
//! trusted operation UUID and a fixed layout, so no caller can name a file.
//!
//! Sequence encoded here (S02 §8.3):
//! 1. build the schema-v1 manifest and context documents and their hashes;
//! 2. validate the triple with the pure [`validate_staging`] **before** a single byte is
//!    written, so an inconsistent input leaves an untouched operation directory;
//! 3. create a fresh operation directory and write `pixels.rgba.tmp`, sync it and publish
//!    the stable `pixels.rgba`;
//! 4. write, sync and publish `context.json`;
//! 5. write, sync and publish `manifest.json` **last**.
//!
//! [`Staged`] is returned only after every publication succeeded. It is **not** a host
//! `Saved` receipt and it is **not** a durable-input flag: nothing here proves that a disk,
//! a controller or the power supply survives a crash. Publishing the manifest last is a
//! recovery ordering, not a durability or power-loss guarantee.
//!
//! Reopening uses only the fixed filenames and the bounded, no-follow read of the lease
//! capability. A missing manifest is reported as a distinct [`ReopenFailure::Incomplete`]
//! and **no** file is ever deleted: an incomplete operation, an unknown file or a sentinel
//! left in the directory stays exactly where it is.
use crate::staging::{
    validate_staging, StagingError, StagingExpectations, ValidatedStagingManifest,
    MAX_CONTEXT_BYTES, MAX_MANIFEST_BYTES, STAGING_MANIFEST_SCHEMA_VERSION,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sm_application::storage::CaptureContext;
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId, RecordId};
use sm_isolation::{
    StagingError as StagingRootError, StagingOperation, StagingRoot, StagingTemp, StagingTempKind,
};
use std::io::Write;
use uuid::Uuid;

/// Fixed manifest kind of one capture operation.
const CAPTURE_KIND: &str = "capture";
/// Fixed reached-state boundary of an operation whose pixels and context are staged.
const STAGED_STATE: &str = "staged";

/// Trusted inputs of one staging write. Every field is a value the host already owns from
/// the accepted capture and its confirmed configuration; the manifest is built from these
/// values and never from a document.
///
/// There is intentionally no path and no filename: the operation directory and every leaf
/// name come from the borrowed lease capability.
pub struct StageInput<'a> {
    pub library_id: LibraryId,
    pub principal_id: PrincipalId,
    pub operation_id: OperationId,
    pub record_id: RecordId,
    pub asset_id: AssetId,
    pub frame_id: Uuid,
    /// Typed capture mode of the accepted request.
    pub mode: sm_application::storage::CaptureTarget,
    /// Re-validated stored capture context of the accepted request.
    pub context: &'a CaptureContext,
    /// Canonical UTC millisecond capture time; validated before any write.
    pub captured_at: &'a str,
    /// Encoder profile fingerprint of the trusted configuration.
    pub encoder_fingerprint: &'a str,
    /// Borrowed packed normalized RGBA pixels; never copied to be written.
    pub pixels: &'a [u8],
}

/// Exact schema-v1 manifest document. The member order is also the serialized order and
/// the set is closed at eighteen members, matching [`crate::staging`].
#[derive(Serialize)]
struct ManifestV1<'a> {
    schema_version: u32,
    kind: &'static str,
    state: &'static str,
    library_id: String,
    principal_id: String,
    operation_id: String,
    record_id: String,
    asset_id: String,
    frame_id: String,
    mode: &'a str,
    width: u32,
    height: u32,
    stride: u32,
    byte_length: u64,
    pixels_sha256: String,
    context_sha256: String,
    encoder_fingerprint: &'a str,
    captured_at: &'a str,
}

/// One completed staging operation. Holding this value means the pixels, the context and
/// the manifest are published under the fixed names of the trusted operation UUID.
///
/// It is not a host `Saved` and not a durable-input flag: no row was committed, no
/// canonical file exists and nothing here survives a hardware failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    manifest: ValidatedStagingManifest,
}

impl Staged {
    pub const fn operation_id(&self) -> OperationId {
        self.manifest.operation_id()
    }
    pub const fn library_id(&self) -> LibraryId {
        self.manifest.library_id()
    }
    pub const fn principal_id(&self) -> PrincipalId {
        self.manifest.principal_id()
    }
    pub const fn record_id(&self) -> RecordId {
        self.manifest.record_id()
    }
    pub const fn asset_id(&self) -> AssetId {
        self.manifest.asset_id()
    }
    pub const fn frame_id(&self) -> Uuid {
        self.manifest.frame_id()
    }
    pub const fn mode(&self) -> sm_application::storage::CaptureTarget {
        self.manifest.mode()
    }
    pub const fn width(&self) -> u32 {
        self.manifest.width()
    }
    pub const fn height(&self) -> u32 {
        self.manifest.height()
    }
    pub const fn stride(&self) -> u32 {
        self.manifest.stride()
    }
    pub const fn byte_length(&self) -> u64 {
        self.manifest.byte_length()
    }
    /// SHA-256 of the staged normalized RGBA bytes.
    pub fn pixels_sha256(&self) -> &str {
        self.manifest.pixels_sha256()
    }
    /// SHA-256 of the staged context document bytes.
    pub fn context_sha256(&self) -> &str {
        self.manifest.context_sha256()
    }
    pub fn encoder_fingerprint(&self) -> &str {
        self.manifest.encoder_fingerprint()
    }
    pub fn captured_at(&self) -> &str {
        self.manifest.captured_at()
    }
    pub const fn context(&self) -> &CaptureContext {
        self.manifest.context()
    }
    /// The re-validated manifest that describes the staged operation.
    pub const fn manifest(&self) -> &ValidatedStagingManifest {
        &self.manifest
    }
}

/// Typed refusal of one staging write. No variant carries a path or a manifest value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageFailure {
    /// The trusted input is not a consistent staging record; nothing was written.
    Rejected(StagingError),
    /// The lease capability refused the operation directory or a fixed leaf.
    Storage(StagingRootError),
    /// A synced write failed; no stable file was published for that section.
    WriteFailed,
    /// A deterministic test seam aborted the operation. Never returned in production.
    Injected,
}

/// One staged artifact whose published stable file is addressed by a fixed name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagedArtifact {
    PixelsRgba,
    ContextJson,
    ManifestJson,
}

/// Typed outcome of reopening a known operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReopenFailure {
    /// No published manifest: the operation is incomplete. Partial temp files may exist
    /// and are never deleted.
    Incomplete,
    /// A manifest is published but a stable input it references is truly absent.
    MissingArtifact(StagedArtifact),
    /// A published artifact exists but was refused as foreign, corrupt, truncated or
    /// beyond its documented schema ceiling.
    Rejected(StagingError),
    /// The operation directory or a fixed leaf could not be reached.
    Storage(StagingRootError),
}

/// Deterministic failpoints. Production always passes [`StageSeams::NONE`]; only this
/// crate's tests turn a seam on, so no host path ever injects a fault.
#[derive(Clone, Copy, Debug, Default)]
struct StageSeams {
    /// Abort after a partial `pixels.rgba.tmp` write (F02).
    mid_pixels_temp: bool,
    /// Abort after both stable files, before `manifest.json` (F03).
    stable_before_manifest: bool,
    /// Abort after `manifest.json` is published (F04).
    manifest_published: bool,
}

impl StageSeams {
    const NONE: Self = Self {
        mid_pixels_temp: false,
        stable_before_manifest: false,
        manifest_published: false,
    };
}

/// Stage one trusted capture under a borrowed lease capability.
///
/// The manifest is validated against the trusted identity before any file is created, and
/// the manifest is published after both stable input files. The returned [`Staged`] is a
/// completion of this write, not a durability or `Saved` claim.
pub fn stage_capture(
    root: &StagingRoot<'_>,
    input: StageInput<'_>,
) -> Result<Staged, StageFailure> {
    stage_capture_seamed(root, input, StageSeams::NONE)
}

fn stage_capture_seamed(
    root: &StagingRoot<'_>,
    input: StageInput<'_>,
    seams: StageSeams,
) -> Result<Staged, StageFailure> {
    let (manifest_bytes, context_bytes, validated) = prepare(&input)?;

    // Create the operation only after the trusted input proved consistent.
    let operation = root
        .create_operation(input.operation_id)
        .map_err(StageFailure::Storage)?;

    // 1. Pixels temp -> sync -> publish stable.
    let mut pixels_temp = operation
        .create_temp(StagingTempKind::PixelsRgba)
        .map_err(StageFailure::Storage)?;
    if seams.mid_pixels_temp {
        // F02: a truncated temp that is never published and never cleaned. The borrowed
        // slice is written in half so no extra copy of the frame is made.
        let half = input.pixels.len() / 2;
        pixels_temp
            .write_all(&input.pixels[..half])
            .map_err(|_| StageFailure::WriteFailed)?;
        drop(pixels_temp);
        return Err(StageFailure::Injected);
    }
    write_synced_publish(pixels_temp, input.pixels)?;

    // 2. Context temp -> sync -> publish stable.
    let context_temp = operation
        .create_temp(StagingTempKind::ContextJson)
        .map_err(StageFailure::Storage)?;
    write_synced_publish(context_temp, context_bytes.as_bytes())?;

    if seams.stable_before_manifest {
        // F03: both input files are stable, the manifest is absent.
        return Err(StageFailure::Injected);
    }

    // 3. Manifest temp -> sync -> publish LAST.
    let manifest_temp = operation
        .create_temp(StagingTempKind::ManifestJson)
        .map_err(StageFailure::Storage)?;
    write_synced_publish(manifest_temp, &manifest_bytes)?;

    if seams.manifest_published {
        // F04: a complete, published staged input; the caller observes the abort, while a
        // reopen sees the same trusted IDs and hashes.
        return Err(StageFailure::Injected);
    }

    Ok(Staged {
        manifest: validated,
    })
}

/// Build and validate the manifest/context documents. No IO happens here: an inconsistent
/// trusted input is refused before the operation directory is touched.
fn prepare(
    input: &StageInput<'_>,
) -> Result<(Vec<u8>, String, ValidatedStagingManifest), StageFailure> {
    let width = input.context.bounds.width;
    let height = input.context.bounds.height;
    let stride = width
        .checked_mul(4)
        .ok_or(StageFailure::Rejected(StagingError::InvalidGeometry))?;
    let byte_length = u64::from(stride)
        .checked_mul(u64::from(height))
        .ok_or(StageFailure::Rejected(StagingError::InvalidGeometry))?;

    let context_bytes = input
        .context
        .to_json()
        .map_err(|_| StageFailure::Rejected(StagingError::InvalidContext))?;
    if context_bytes.len() > MAX_CONTEXT_BYTES {
        return Err(StageFailure::Rejected(StagingError::ContextBeyondLimit));
    }

    let document = ManifestV1 {
        schema_version: STAGING_MANIFEST_SCHEMA_VERSION,
        kind: CAPTURE_KIND,
        state: STAGED_STATE,
        library_id: input.library_id.as_uuid().to_string(),
        principal_id: input.principal_id.as_uuid().to_string(),
        operation_id: input.operation_id.as_uuid().to_string(),
        record_id: input.record_id.as_uuid().to_string(),
        asset_id: input.asset_id.as_uuid().to_string(),
        frame_id: input.frame_id.to_string(),
        mode: input.mode.as_str(),
        width,
        height,
        stride,
        byte_length,
        pixels_sha256: sha256_hex(input.pixels),
        context_sha256: sha256_hex(context_bytes.as_bytes()),
        encoder_fingerprint: input.encoder_fingerprint,
        captured_at: input.captured_at,
    };
    let manifest_bytes = serde_json::to_vec(&document)
        .map_err(|_| StageFailure::Rejected(StagingError::MalformedManifest))?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(StageFailure::Rejected(StagingError::ManifestBeyondLimit));
    }

    let expectations = StagingExpectations {
        library_id: input.library_id,
        principal_id: input.principal_id,
        operation_id: input.operation_id,
        mode: input.mode,
        encoder_fingerprint: input.encoder_fingerprint,
    };
    // The pure validator is the single authority: it recomputes geometry, digests, profile
    // and context before the first byte reaches the filesystem.
    let validated = validate_staging(
        &manifest_bytes,
        context_bytes.as_bytes(),
        input.pixels,
        expectations,
    )
    .map_err(StageFailure::Rejected)?;
    Ok((manifest_bytes, context_bytes, validated))
}

/// Write a borrowed bounded slice, sync it and publish the stable name.
fn write_synced_publish(
    mut temp: StagingTemp<'_, '_, '_>,
    bytes: &[u8],
) -> Result<(), StageFailure> {
    temp.write_all(bytes)
        .map_err(|_| StageFailure::WriteFailed)?;
    temp.sync_all().map_err(|_| StageFailure::WriteFailed)?;
    temp.publish().map_err(StageFailure::Storage)
}

/// Reopen one known operation through the fixed, bounded, no-follow read.
///
/// * [`ReopenFailure::Incomplete`] — the operation directory exists but has no published
///   manifest, or does not exist at all. No file is deleted or repaired.
/// * [`ReopenFailure::Rejected`] — a published manifest/context/pixel artifact is foreign,
///   corrupt, truncated or beyond its documented schema ceiling.
pub fn reopen_staged(
    root: &StagingRoot<'_>,
    expectations: StagingExpectations<'_>,
) -> Result<Staged, ReopenFailure> {
    let operation = match root.open_operation(expectations.operation_id) {
        Ok(operation) => operation,
        Err(StagingRootError::Missing) => return Err(ReopenFailure::Incomplete),
        Err(error) => return Err(ReopenFailure::Storage(error)),
    };

    let manifest_bytes = match operation.open_fixed(StagingTempKind::ManifestJson) {
        Ok(read) => read
            .read_all()
            .map_err(|_| ReopenFailure::Rejected(StagingError::MalformedManifest))?,
        // No published manifest: the operation is incomplete. Partial temp/stable files
        // are left in place for a later, explicit reconciliation.
        Err(StagingRootError::Missing) => return Err(ReopenFailure::Incomplete),
        // An existing manifest beyond its schema ceiling is present-but-refused, never a
        // missing artifact: absence and oversize must stay distinguishable.
        Err(StagingRootError::TooLarge) => {
            return Err(ReopenFailure::Rejected(StagingError::ManifestBeyondLimit))
        }
        Err(error) => return Err(ReopenFailure::Storage(error)),
    };

    // A published manifest without its two stable inputs is a corrupt/truncated state,
    // not an incomplete one.
    let context_bytes = read_fixed(&operation, StagedArtifact::ContextJson)?;
    let pixels = read_fixed(&operation, StagedArtifact::PixelsRgba)?;

    let validated = validate_staging(&manifest_bytes, &context_bytes, &pixels, expectations)
        .map_err(ReopenFailure::Rejected)?;
    Ok(Staged {
        manifest: validated,
    })
}

/// Bounded fixed-leaf read; a missing stable file is a corrupt staged input.
fn read_fixed(
    operation: &StagingOperation<'_, '_>,
    artifact: StagedArtifact,
) -> Result<Vec<u8>, ReopenFailure> {
    let kind = match artifact {
        StagedArtifact::PixelsRgba => StagingTempKind::PixelsRgba,
        StagedArtifact::ContextJson => StagingTempKind::ContextJson,
        StagedArtifact::ManifestJson => StagingTempKind::ManifestJson,
    };
    match operation.open_fixed(kind) {
        Ok(read) => read
            .read_all()
            .map_err(|_| ReopenFailure::Rejected(StagingError::MalformedManifest)),
        // An existing leaf beyond its schema ceiling is present-but-refused, reported
        // with the typed refusal of that artifact instead of a false absence.
        Err(StagingRootError::TooLarge) => Err(ReopenFailure::Rejected(match artifact {
            StagedArtifact::PixelsRgba => StagingError::PixelLengthMismatch,
            StagedArtifact::ContextJson => StagingError::ContextBeyondLimit,
            StagedArtifact::ManifestJson => StagingError::ManifestBeyondLimit,
        })),
        Err(StagingRootError::Missing) => Err(ReopenFailure::MissingArtifact(artifact)),
        Err(error) => Err(ReopenFailure::Storage(error)),
    }
}

/// Digest of a borrowed slice; the input is never copied.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
#[path = "staging_files_tests.rs"]
mod staging_files_tests;

#[cfg(test)]
mod purity {
    /// The coordinator stays inside the storage owner: no SQL, no wire, no process and no
    /// caller-supplied path, and it never claims a host `Saved` or a durable-input flag.
    #[test]
    fn s02_t09b_19_coordinator_has_no_sql_wire_or_path() {
        let source = include_str!("staging_files.rs");
        for needle in [
            ["rus", "qlite"].concat(),
            ["std", "::net"].concat(),
            ["std", "::process"].concat(),
            ["Path", "Buf"].concat(),
            ["std", "::path"].concat(),
            ["durable", "_input"].concat(),
        ] {
            assert!(
                !source.contains(&needle),
                "staging coordinator must stay pathless and connection-free; found {needle}"
            );
        }
        assert!(source.contains("validate_staging"));
    }
}
