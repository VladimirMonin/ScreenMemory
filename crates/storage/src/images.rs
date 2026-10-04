//! Canonical asset publication and bounded record reads for the images schema.
//!
//! The record row, the canonical file and the asset row share one `BEGIN IMMEDIATE`
//! transaction: the file is created inside the transaction and deleted again when the
//! transaction is rolled back, so a failed commit can never leave an orphan asset.
//! No caller-supplied path, SQL fragment or absolute path reaches this module.
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use sm_application::storage::{
    provenances_to_json, warnings_from_json, warnings_to_json, Asset, AssetChunk, AssetType,
    CaptureContext, CaptureTarget, CommitImageCapture, CommittedImage, LookupOutcome, Page, Record,
    RecordPageQuery, StorageError, ASSET_CHUNK_BYTES, CANONICAL_FORMAT_WEBP_LOSSLESS,
    CANONICAL_MIME_WEBP, MAX_CANONICAL_FILE_BYTES, MAX_CAPTURE_SIDE,
};
use sm_domain::{AssetId, RecordId};
use std::{
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Canonical container header length (RIFF tag, declared size, WEBP tag).
const CONTAINER_HEADER: usize = 12;
/// One extra row reveals whether another page exists without a second query.
const PAGE_PROBE: i64 = 1;

/// Assets directory of the leased library root. Derived from the leased database
/// entry only; the caller can never pass a path.
pub(super) fn assets_root(database_entry: &Path) -> Result<PathBuf, StorageError> {
    let data = database_entry.parent().ok_or(StorageError::RootRejected)?;
    if !data
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("data"))
    {
        return Err(StorageError::RootRejected);
    }
    let root = data.parent().ok_or(StorageError::RootRejected)?;
    Ok(root.join("assets"))
}

/// `true` only for an exact RIFF/WEBP container whose declared length matches.
fn is_canonical_webp_container(bytes: &[u8]) -> bool {
    bytes.len() >= CONTAINER_HEADER
        && &bytes[..4] == b"RIFF"
        && &bytes[8..12] == b"WEBP"
        && u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize + 8 == bytes.len()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// SQL failures are classified, never retried blindly and never repaired in place.
fn sql_error(error: rusqlite::Error) -> StorageError {
    match error {
        rusqlite::Error::SqliteFailure(code, _) => match code.code {
            rusqlite::ErrorCode::ConstraintViolation => StorageError::IntegrityFailed,
            _ => StorageError::Unavailable,
        },
        _ => StorageError::Unavailable,
    }
}

/// A reparse point (junction, mount point or symlink) is never followed for a write.
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        std::os::windows::fs::MetadataExt::file_attributes(metadata) & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn verified_directory(path: &Path) -> Result<(), StorageError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| StorageError::PermissionDenied)?;
    if metadata.is_dir() && !is_reparse(&metadata) {
        Ok(())
    } else {
        Err(StorageError::RootRejected)
    }
}

/// Create and verify every directory component below the leased assets root.
fn secure_directory(root: &Path, components: &[&str]) -> Result<PathBuf, StorageError> {
    if root.exists() {
        verified_directory(root)?;
    } else {
        std::fs::create_dir_all(root).map_err(|error| match error.kind() {
            ErrorKind::PermissionDenied => StorageError::PermissionDenied,
            _ => StorageError::RootRejected,
        })?;
        verified_directory(root)?;
    }
    let mut current = root.to_path_buf();
    for component in components {
        if component.is_empty()
            || *component == "."
            || *component == ".."
            || component.contains(['\\', '/', ':'])
        {
            return Err(StorageError::RootRejected);
        }
        current.push(component);
        if current.exists() {
            verified_directory(&current)?;
        } else {
            std::fs::create_dir(&current).map_err(|error| match error.kind() {
                ErrorKind::PermissionDenied => StorageError::PermissionDenied,
                _ => StorageError::RootRejected,
            })?;
            verified_directory(&current)?;
        }
    }
    Ok(current)
}

/// A newly published canonical file that is removed again unless the commit succeeded.
struct PublishedFile {
    path: PathBuf,
    keep: bool,
}

impl PublishedFile {
    fn create(root: &Path, relative: &str, bytes: &[u8]) -> Result<Self, StorageError> {
        let components: Vec<&str> = relative.split('/').collect();
        let (file_name, directories) = components
            .split_last()
            .ok_or(StorageError::InvalidPayload)?;
        let directory = secure_directory(root, directories)?;
        let path = directory.join(file_name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| match error.kind() {
                // An existing destination is never replaced by a second publication.
                ErrorKind::AlreadyExists => StorageError::IntegrityFailed,
                ErrorKind::PermissionDenied => StorageError::PermissionDenied,
                _ => StorageError::RootRejected,
            })?;
        let published = Self { path, keep: false };
        file.write_all(bytes)
            .map_err(|_| StorageError::PermissionDenied)?;
        file.sync_all()
            .map_err(|_| StorageError::PermissionDenied)?;
        Ok(published)
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for PublishedFile {
    fn drop(&mut self) {
        if !self.keep {
            // Only the exact file this publication created is removed.
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Opaque page cursor: snapshot upper bound, last delivered key and library identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub(crate) snapshot_upper: i64,
    pub(crate) before: i64,
}

impl Cursor {
    pub(crate) fn encode(self, library_id: &str) -> String {
        format!("v1:{library_id}:{}:{}", self.snapshot_upper, self.before)
    }

    pub(crate) fn parse(raw: &str, library_id: &str) -> Result<Self, StorageError> {
        let parts: Vec<&str> = raw.split(':').collect();
        if parts.len() != 4 || parts[0] != "v1" || parts[1] != library_id {
            return Err(StorageError::InvalidQuery);
        }
        if identity(parts[1]).is_err() {
            return Err(StorageError::InvalidQuery);
        }
        let number = |raw: &str| -> Result<i64, StorageError> {
            if raw.is_empty()
                || (raw.len() > 1 && raw.starts_with('0'))
                || !raw.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(StorageError::InvalidQuery);
            }
            raw.parse::<i64>().map_err(|_| StorageError::InvalidQuery)
        };
        Ok(Self {
            snapshot_upper: number(parts[2])?,
            before: number(parts[3])?,
        })
    }
}

/// Canonical non-nil UUID text as stored by the images schema.
fn identity(raw: &str) -> Result<Uuid, StorageError> {
    let parsed = Uuid::parse_str(raw).map_err(|_| StorageError::IntegrityFailed)?;
    if parsed.is_nil() || parsed.to_string() != raw {
        return Err(StorageError::IntegrityFailed);
    }
    Ok(parsed)
}

/// Raw `saved_operation` projection: fingerprint, operation, record, asset, digest,
/// relative path, stored bytes, timestamp, format and the replayable receipt metadata
/// (mode, positive canonical geometry, bounded warnings).
type SavedOperationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    i64,
    i64,
    String,
);

/// Domain tag of the ingest semantic fingerprint. Changing it is a new contract.
const INGEST_FINGERPRINT_DOMAIN: &[u8] = b"ScreenMemory.ingest.v1";

/// Stable, domain-separated fingerprint of one commit request: canonical digest, asset
/// role, capture mode, capture context and provenances. The randomly generated
/// record/asset ids and the wall-clock instant are deliberately absent, so a retry of the
/// same accepted capture matches while a changed image, mode, context, provenance or role
/// does not. The stored `warnings` are diagnostic and are *not* fingerprinted: a replay
/// must return the recorded warnings of the committed capture, never the retry's own.
pub(super) fn semantic_fingerprint(
    asset_type: &str,
    mode: &str,
    canonical_sha256: &str,
    context_json: &str,
    provenances_json: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(INGEST_FINGERPRINT_DOMAIN);
    for field in [
        asset_type,
        mode,
        canonical_sha256,
        context_json,
        provenances_json,
    ] {
        hasher.update([0u8]);
        hasher.update(field.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// The saved operation of exactly this principal, with its stored fingerprint and
/// committed receipt. Only `QueryReturnedNoRows` means "no such operation": every
/// other SQLite failure is surfaced, and a saved row whose committed asset cannot be
/// joined is refused as corruption rather than masked as a missing operation.
pub(super) fn saved_operation(
    db: &Connection,
    operation_id: &str,
    principal_id: &str,
) -> Result<Option<(String, CommittedImage)>, StorageError> {
    let row: Option<SavedOperationRow> = db
        .query_row(
            "SELECT io.semantic_fingerprint, io.operation_id, io.record_id, a.id, a.sha256, a.path, a.size_bytes, a.created_at, a.format, io.mode, io.width, io.height, io.warnings \
             FROM ingest_operations io \
             JOIN assets a ON a.id = io.planned_asset_id AND a.record_id = io.record_id \
             WHERE io.operation_id = ?1 AND io.principal_id = ?2 AND io.state = 'saved'",
            params![operation_id, principal_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, String>(12)?,
                ))
            },
        )
        .optional()
        .map_err(sql_error)?;
    let (
        fingerprint,
        op_id,
        rec_id,
        a_id,
        sha256,
        path,
        size_bytes,
        created_at,
        format,
        mode,
        width,
        height,
        warnings_json,
    ) = match row {
        Some(row) => row,
        None => {
            let saved: Option<i64> = db
                .query_row(
                    "SELECT 1 FROM ingest_operations WHERE operation_id = ?1 AND principal_id = ?2 AND state = 'saved'",
                    params![operation_id, principal_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql_error)?;
            if saved.is_some() {
                // The journal claims a saved operation but its committed asset is gone.
                return Err(StorageError::IntegrityFailed);
            }
            return Ok(None);
        }
    };
    // Stored metadata is re-validated on read: a malformed row is refused as corruption
    // rather than repaired or turned into a fabricated receipt.
    if format != CANONICAL_FORMAT_WEBP_LOSSLESS
        || size_bytes <= 0
        || fingerprint.len() != 64
        || !(1..=i64::from(MAX_CAPTURE_SIDE)).contains(&width)
        || !(1..=i64::from(MAX_CAPTURE_SIDE)).contains(&height)
    {
        return Err(StorageError::IntegrityFailed);
    }
    let mode = CaptureTarget::parse(&mode)?;
    let warnings = warnings_from_json(&warnings_json).map_err(|_| StorageError::IntegrityFailed)?;
    Ok(Some((
        fingerprint,
        CommittedImage {
            operation_id: sm_domain::OperationId::new(
                Uuid::parse_str(&op_id).map_err(|_| StorageError::IntegrityFailed)?,
            ),
            principal_id: sm_domain::PrincipalId::new(
                Uuid::parse_str(principal_id).map_err(|_| StorageError::IntegrityFailed)?,
            ),
            record_id: sm_domain::RecordId::new(
                Uuid::parse_str(&rec_id).map_err(|_| StorageError::IntegrityFailed)?,
            ),
            asset_id: sm_domain::AssetId::new(
                Uuid::parse_str(&a_id).map_err(|_| StorageError::IntegrityFailed)?,
            ),
            mode,
            width: width as u32,
            height: height as u32,
            canonical_mime: CANONICAL_MIME_WEBP,
            warnings,
            created_at,
            relative_path: path,
            size_bytes: size_bytes as u64,
            sha256,
        },
    )))
}

/// Principal that currently owns one operation id in the journal, if any.
fn operation_owner(db: &Connection, operation_id: &str) -> Result<Option<String>, StorageError> {
    db.query_row(
        "SELECT principal_id FROM ingest_operations WHERE operation_id = ?1",
        params![operation_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(sql_error)
}

/// Outcome of one replay branch of [`commit_image_capture`]: only a semantic fingerprint
/// match that also proves its canonical file (existence, size, digest, safe relative path)
/// yields the committed receipt. A replay whose file was deleted or substituted fails
/// closed exactly like a lookup of the same operation, so a public commit retry can never
/// report `Saved` for a capture whose bytes are gone. A fingerprint mismatch stays a safe
/// conflict that discloses no stored metadata.
fn replay_outcome(
    assets: &Path,
    saved: String,
    fingerprint: String,
    receipt: CommittedImage,
) -> Result<CommittedImage, StorageError> {
    if saved != fingerprint {
        return Err(StorageError::IntegrityFailed);
    }
    verify_committed_canonical(assets, &receipt)?;
    Ok(receipt)
}

/// Verify the canonical file of one committed receipt row.
fn verify_committed_canonical(assets: &Path, receipt: &CommittedImage) -> Result<(), StorageError> {
    read_verified_canonical(
        assets,
        &receipt.relative_path,
        receipt.size_bytes,
        &receipt.sha256,
    )
    .map(|_| ())
    .map_err(|error| match error {
        StorageError::AssetNotFound => StorageError::IntegrityFailed,
        other => other,
    })
}

/// Insert the record, write the canonical file and commit; roll back both together.
pub(super) fn commit_image_capture(
    db: &mut Connection,
    assets: &Path,
    command: CommitImageCapture,
    fail_before_commit: bool,
) -> Result<CommittedImage, StorageError> {
    let op_id = command.operation_id.as_uuid().to_string();
    let principal = command.principal_id.as_uuid().to_string();

    // The typed payload is validated and reduced to a stable semantic fingerprint
    // before any SQL. Fresh random record/asset ids are not part of it, so a replay is
    // recognised by content, mode and context rather than by generated UUIDs.
    let context_json = command.capture_context.to_json()?;
    let provenances_json = provenances_to_json(&command.provenances)?;
    let warnings_json = warnings_to_json(&command.warnings)?;
    if command.width == 0
        || command.height == 0
        || command.width > MAX_CAPTURE_SIDE
        || command.height > MAX_CAPTURE_SIDE
    {
        return Err(StorageError::InvalidPayload);
    }
    if !is_canonical_webp_container(&command.canonical) {
        return Err(StorageError::InvalidPayload);
    }
    let size_bytes = command.canonical.len() as u64;
    let sha256 = sha256_hex(&command.canonical);
    let asset_type = command.asset_type.as_str();
    let mode = command.mode.as_str();
    let fingerprint =
        semantic_fingerprint(asset_type, mode, &sha256, &context_json, &provenances_json);
    let width = i64::from(command.width);
    let height = i64::from(command.height);
    let record_id = command.record_id.as_uuid().to_string();
    let asset_id = command.asset_id.as_uuid().to_string();

    // Idempotent replay: a saved (operation_id, principal_id) returns its committed
    // receipt when the semantic fingerprint matches, whatever resource ids the retry
    // carried. A mismatch is a safe conflict that discloses no stored metadata.
    if let Some((saved, receipt)) = saved_operation(db, &op_id, &principal)? {
        return replay_outcome(assets, saved, fingerprint, receipt);
    }
    // An operation id owned by another principal is never disclosed here.
    if operation_owner(db, &op_id)?.is_some() {
        return Err(StorageError::IntegrityFailed);
    }

    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;

    // Re-check inside the transaction — a concurrent retry may have committed first.
    if let Some((saved, receipt)) = saved_operation(&tx, &op_id, &principal)? {
        tx.commit().map_err(sql_error)?;
        return replay_outcome(assets, saved, fingerprint, receipt);
    }
    if operation_owner(&tx, &op_id)?.is_some() {
        return Err(StorageError::IntegrityFailed);
    }

    let created_at: String = tx
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |row| {
            row.get(0)
        })
        .map_err(sql_error)?;
    let library_id: String = tx
        .query_row(
            "SELECT library_id FROM library_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let month = created_at.get(..7).ok_or(StorageError::IntegrityFailed)?;
    let file_relative = format!("{month}/{record_id}/{asset_type}.webp");
    let relative_path = format!("assets/{file_relative}");

    tx.execute(
        "INSERT INTO records (id, library_id, created_at, capture_context, provenances) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![&record_id, &library_id, &created_at, context_json, provenances_json],
    )
    .map_err(sql_error)?;
    tx.execute(
        "INSERT INTO assets (id, record_id, asset_type, format, path, size_bytes, sha256, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &asset_id,
            &record_id,
            asset_type,
            CANONICAL_FORMAT_WEBP_LOSSLESS,
            &relative_path,
            size_bytes as i64,
            &sha256,
            &created_at
        ],
    )
    .map_err(sql_error)?;

    // The journal row is written last and carries the whole saved state: its
    // `record_id` foreign key and the `state='saved'` CHECK require the record row
    // to exist already, the row must record the canonical fingerprint it was
    // committed with (`content_sha256` is NOT NULL), and the replayable receipt
    // metadata (mode, positive geometry, bounded warnings) is written here in the
    // same transaction, so a saved row is never partially populated.
    tx.execute(
        "INSERT INTO ingest_operations (operation_id, principal_id, kind, planned_record_id, planned_asset_id, content_sha256, semantic_fingerprint, mode, width, height, warnings, record_id, state, created_at_utc, updated_at_utc) VALUES (?1, ?2, 'capture', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?3, 'saved', ?11, ?11)",
        params![
            &op_id,
            &principal,
            &record_id,
            &asset_id,
            &sha256,
            &fingerprint,
            mode,
            width,
            height,
            &warnings_json,
            &created_at
        ],
    )
    .map_err(sql_error)?;

    crate::knowledge::initial(
        &tx,
        sm_protocol::knowledge::KnowledgeId(command.record_id.as_uuid()),
    )
    .map_err(|_| StorageError::IntegrityFailed)?;
    let mut published = PublishedFile::create(assets, &file_relative, &command.canonical)?;

    if fail_before_commit {
        return Err(StorageError::Unavailable);
    }
    tx.commit().map_err(sql_error)?;
    published.keep();
    Ok(CommittedImage {
        operation_id: command.operation_id,
        principal_id: command.principal_id,
        record_id: command.record_id,
        asset_id: command.asset_id,
        mode: command.mode,
        width: command.width,
        height: command.height,
        canonical_mime: CANONICAL_MIME_WEBP,
        warnings: command.warnings,
        created_at,
        relative_path,
        size_bytes,
        sha256,
    })
}

/// Typed, ownership-scoped outcome of one operation lookup. The principal and the
/// requested mode are both part of the check, so a foreign principal or a changed mode
/// yields the same safe [`LookupOutcome::Conflict`] without disclosing stored metadata.
/// Only a genuinely absent id is [`LookupOutcome::Unknown`]; a saved row whose metadata
/// is malformed is surfaced as an error instead of a fabricated receipt.
pub(super) fn lookup_operation(
    db: &Connection,
    assets: &Path,
    operation_id: &str,
    principal_id: &str,
    mode: CaptureTarget,
) -> Result<LookupOutcome, StorageError> {
    // The owner of the id, if any, decides conflict vs unknown before any receipt is
    // built, so a foreign or mode-mismatched request never touches the stored metadata.
    let Some(owner) = operation_owner(db, operation_id)? else {
        return Ok(LookupOutcome::Unknown);
    };
    if owner != principal_id {
        return Ok(LookupOutcome::Conflict);
    }
    // The requested mode is compared against the raw stored text before any other stored
    // metadata is read or validated. A mismatch is the same safe conflict even when the
    // rest of the saved row is malformed or its joined asset is gone, so a wrong-mode
    // request never leaks a different failure than a well-formed row would produce.
    let stored_mode: Option<String> = db
        .query_row(
            "SELECT mode FROM ingest_operations WHERE operation_id = ?1 AND principal_id = ?2 AND state = 'saved'",
            params![operation_id, principal_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    match stored_mode {
        // The id is owned by this principal but not in the `saved` state: a conflict, not
        // a receipt.
        None => return Ok(LookupOutcome::Conflict),
        Some(stored) if stored != mode.as_str() => {
            // A replay under a different capture mode is the same safe conflict: the stored
            // mode is not disclosed and no receipt metadata is returned. The check happens
            // before any file IO or receipt deserialization, so a wrong-mode request never
            // reads or validates the stored asset, and a malformed row or a missing joined
            // asset cannot leak a different error.
            return Ok(LookupOutcome::Conflict);
        }
        Some(_) => {}
    }
    let Some((_, receipt)) = saved_operation(db, operation_id, principal_id)? else {
        return Ok(LookupOutcome::Conflict);
    };
    // Only now, for the caller's own matching operation, is the canonical file verified.
    // A `saved` row whose asset is missing or no longer matches its committed size/digest
    // is an integrity failure, never a fabricated receipt (S02 §8.7). Missing maps to the
    // same typed failure as a substitution because the row claims a canonical file exists.
    verify_committed_canonical(assets, &receipt)?;
    Ok(LookupOutcome::Saved(receipt))
}

fn stored_asset(
    row: &(String, String, String, String, String, i64, String, String),
) -> Result<Asset, StorageError> {
    let (asset_id, record_id, asset_type, format, path, size_bytes, sha256, created_at) = row;
    if !matches!(
        format.as_str(),
        CANONICAL_FORMAT_WEBP_LOSSLESS | "avif_sdr90"
    ) || *size_bytes <= 0
    {
        return Err(StorageError::IntegrityFailed);
    }
    Ok(Asset {
        id: AssetId::new(identity(asset_id)?),
        record_id: RecordId::new(identity(record_id)?),
        asset_type: AssetType::parse(asset_type)?,
        format: if format == "avif_sdr90" {
            "avif_sdr90"
        } else {
            CANONICAL_FORMAT_WEBP_LOSSLESS
        },
        relative_path: path.clone(),
        size_bytes: *size_bytes as u64,
        sha256: sha256.clone(),
        created_at: created_at.clone(),
    })
}

/// Absolute path of one committed canonical asset below the leased assets root.
///
/// The stored relative path is re-validated component by component, so a tampered row can
/// never escape the assets directory.
fn asset_path(assets: &Path, relative: &str) -> Result<PathBuf, StorageError> {
    let rest = relative
        .strip_prefix("assets/")
        .ok_or(StorageError::IntegrityFailed)?;
    if rest.contains("..") || rest.contains(':') || rest.contains('\\') {
        return Err(StorageError::IntegrityFailed);
    }
    let mut path = assets.to_path_buf();
    for component in rest.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(StorageError::IntegrityFailed);
        }
        path.push(component);
    }
    if !matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("webp" | "avif")
    ) {
        return Err(StorageError::IntegrityFailed);
    }
    Ok(path)
}

/// Read and verify the whole canonical file of one committed asset row.
///
/// The stored relative path is re-validated before any IO, and the bytes must match both
/// the committed size and digest, so a substituted or truncated file is refused instead
/// of being exported or replayed. The committed size is bounded by
/// [`MAX_CANONICAL_FILE_BYTES`] before any allocation, and a file whose own length already
/// exceeds that bound is refused from its metadata without being read. A missing file is
/// surfaced as [`StorageError::AssetNotFound`]; the caller decides whether that is a
/// missing-asset refusal (bounded read) or an integrity failure (replay of a `saved` row).
fn read_verified_canonical(
    assets: &Path,
    relative: &str,
    size_bytes: u64,
    sha256: &str,
) -> Result<Vec<u8>, StorageError> {
    let path = asset_path(assets, relative)?;
    // The committed size is bounded before any IO or allocation, so a stored row (or a
    // substituted file) cannot make a replay read an unbounded file into memory. The
    // length of the file itself is checked from its metadata first: an obviously oversized
    // file is refused without being read at all.
    if size_bytes == 0 || size_bytes > MAX_CANONICAL_FILE_BYTES {
        return Err(StorageError::IntegrityFailed);
    }
    let declared = std::fs::metadata(&path).map_err(|error| match error.kind() {
        ErrorKind::NotFound => StorageError::AssetNotFound,
        ErrorKind::PermissionDenied => StorageError::PermissionDenied,
        _ => StorageError::IntegrityFailed,
    })?;
    if declared.len() == 0 || declared.len() > MAX_CANONICAL_FILE_BYTES {
        return Err(StorageError::AssetIntegrityFailed);
    }
    let bytes = std::fs::read(&path).map_err(|error| match error.kind() {
        ErrorKind::NotFound => StorageError::AssetNotFound,
        ErrorKind::PermissionDenied => StorageError::PermissionDenied,
        _ => StorageError::IntegrityFailed,
    })?;
    if bytes.len() as u64 != size_bytes || sha256_hex(&bytes) != sha256 {
        return Err(StorageError::AssetIntegrityFailed);
    }
    Ok(bytes)
}

/// Read one bounded chunk of a committed canonical asset after re-verifying the whole file.
///
/// A file whose size or digest differs from its committed row is refused; the caller
/// reports the integrity failure instead of exporting substituted bytes.
pub(super) fn read_asset_chunk(
    db: &Connection,
    assets: &Path,
    asset_id: &str,
    offset: u64,
    length: u32,
) -> Result<AssetChunk, StorageError> {
    let id = identity(asset_id)?;
    let row = db
        .query_row(
            "SELECT path, size_bytes, sha256, format FROM assets WHERE id = ?1",
            params![asset_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => StorageError::AssetNotFound,
            other => sql_error(other),
        })?;
    let (relative, size_bytes, sha256, format) = row;
    if !matches!(
        format.as_str(),
        CANONICAL_FORMAT_WEBP_LOSSLESS | "avif_sdr90"
    ) || size_bytes <= 0
    {
        return Err(StorageError::IntegrityFailed);
    }
    let total = size_bytes as u64;
    if total > MAX_CANONICAL_FILE_BYTES || offset > total {
        return Err(StorageError::InvalidQuery);
    }
    // Stream verification in a fixed scratch buffer, retaining only this range.
    use sha2::Digest;
    use std::io::Read;
    let mut file = std::fs::File::open(asset_path(assets, &relative)?)
        .map_err(|_| StorageError::AssetNotFound)?;
    if file
        .metadata()
        .map_err(|_| StorageError::AssetIntegrityFailed)?
        .len()
        != total
    {
        return Err(StorageError::AssetIntegrityFailed);
    }
    let chunk_len = u64::from(length.min(ASSET_CHUNK_BYTES)).min(total - offset);
    let mut bytes = Vec::with_capacity(chunk_len as usize);
    let mut digest = sha2::Sha256::new();
    let mut scratch = [0u8; 64 * 1024];
    let mut position = 0u64;
    loop {
        let n = file
            .read(&mut scratch)
            .map_err(|_| StorageError::AssetIntegrityFailed)?;
        if n == 0 {
            break;
        }
        let end = position + n as u64;
        if end > total {
            return Err(StorageError::AssetIntegrityFailed);
        }
        digest.update(&scratch[..n]);
        let lo = position.max(offset);
        let hi = end.min(offset + chunk_len);
        if hi > lo {
            bytes.extend_from_slice(&scratch[(lo - position) as usize..(hi - position) as usize]);
        }
        position = end;
    }
    if position != total || format!("{:x}", digest.finalize()) != sha256 {
        return Err(StorageError::AssetIntegrityFailed);
    }
    Ok(AssetChunk {
        asset_id: AssetId::new(id),
        offset,
        next_offset: offset + chunk_len,
        eof: offset + chunk_len >= total,
        total_size: total,
        sha256,
        format: if format == "avif_sdr90" {
            "avif_sdr90"
        } else {
            CANONICAL_FORMAT_WEBP_LOSSLESS
        },
        bytes,
    })
}

/// Bounded, snapshot-stable page of committed records with their assets.
pub(super) fn page_records(
    db: &Connection,
    query: RecordPageQuery,
) -> Result<Page<Record>, StorageError> {
    let limit = i64::from(query.limit());
    let library_id: String = db
        .query_row(
            "SELECT library_id FROM library_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let cursor = match query.cursor() {
        Some(raw) => Some(Cursor::parse(raw, &library_id)?),
        None => None,
    };
    let (snapshot_upper, before) = match cursor {
        Some(cursor) => (cursor.snapshot_upper, cursor.before),
        None => (
            db.query_row(
                "SELECT COALESCE(MAX(record_pk), 0) FROM records",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?,
            i64::MAX,
        ),
    };

    let mut statement = db
        .prepare(
            "SELECT record_pk, id, created_at, capture_context FROM records WHERE source_kind='capture' AND record_pk <= ?1 AND record_pk < ?2 ORDER BY record_pk DESC LIMIT ?3",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map(params![snapshot_upper, before, limit + PAGE_PROBE], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    let has_more = rows.len() as i64 > limit;
    let mut rows = rows;
    rows.truncate(limit as usize);

    // One bounded statement for exactly the page's assets; no per-record query.
    let mut assets_statement = db
        .prepare(
            "SELECT a.id, a.record_id, a.asset_type, a.format, a.path, a.size_bytes, a.sha256, a.created_at \
             FROM assets a WHERE a.record_id IN (SELECT id FROM records WHERE source_kind='capture' AND record_pk <= ?1 AND record_pk < ?2 ORDER BY record_pk DESC LIMIT ?3) \
             ORDER BY a.record_id, a.asset_type",
        )
        .map_err(sql_error)?;
    let assets = assets_statement
        .query_map(params![snapshot_upper, before, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    let stored: Vec<Asset> = assets
        .iter()
        .map(stored_asset)
        .collect::<Result<Vec<_>, _>>()?;

    let mut records = Vec::with_capacity(rows.len());
    let mut last_pk = snapshot_upper;
    for (record_pk, id, created_at, context_json) in &rows {
        let record_id = RecordId::new(identity(id)?);
        let context =
            CaptureContext::from_json(context_json).map_err(|_| StorageError::IntegrityFailed)?;
        let record_assets = stored
            .iter()
            .filter(|asset| asset.record_id == record_id)
            .cloned()
            .collect();
        records.push(Record {
            id: record_id,
            created_at: created_at.clone(),
            title: context.title,
            target: context.target,
            bounds: context.bounds,
            assets: record_assets,
        });
        last_pk = *record_pk;
    }
    let next_cursor = if has_more {
        Some(
            Cursor {
                snapshot_upper,
                before: last_pk,
            }
            .encode(&library_id),
        )
    } else {
        None
    };
    Ok(Page {
        items: records,
        next_cursor,
    })
}
