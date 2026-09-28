//! Canonical asset publication and bounded record reads for the images schema.
//!
//! The record row, the canonical file and the asset row share one `BEGIN IMMEDIATE`
//! transaction: the file is created inside the transaction and deleted again when the
//! transaction is rolled back, so a failed commit can never leave an orphan asset.
//! No caller-supplied path, SQL fragment or absolute path reaches this module.
use rusqlite::{params, Connection, TransactionBehavior};
use sha2::{Digest, Sha256};
use sm_application::storage::{
    provenances_to_json, Asset, AssetType, CaptureContext, CommitImageCapture, CommittedImage,
    Page, Record, RecordPageQuery, StorageError, CANONICAL_FORMAT_WEBP_LOSSLESS,
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
struct Cursor {
    snapshot_upper: i64,
    before: i64,
}

impl Cursor {
    fn encode(self, library_id: &str) -> String {
        format!("v1:{library_id}:{}:{}", self.snapshot_upper, self.before)
    }

    fn parse(raw: &str, library_id: &str) -> Result<Self, StorageError> {
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

/// Insert the record, write the canonical file and commit; roll back both together.
pub(super) fn commit_image_capture(
    db: &mut Connection,
    assets: &Path,
    command: CommitImageCapture,
    fail_before_commit: bool,
) -> Result<CommittedImage, StorageError> {
    let context_json = command.capture_context.to_json()?;
    let provenances_json = provenances_to_json(&command.provenances)?;
    if !is_canonical_webp_container(&command.canonical) {
        return Err(StorageError::InvalidPayload);
    }
    let size_bytes = command.canonical.len() as u64;
    let sha256 = sha256_hex(&command.canonical);
    let asset_type = command.asset_type.as_str();
    let record_id = command.record_id.as_uuid().to_string();
    let asset_id = command.asset_id.as_uuid().to_string();

    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
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
    // Stored path is library-relative; the file lives directly below the assets root.
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

    let mut published = PublishedFile::create(assets, &file_relative, &command.canonical)?;
    if fail_before_commit {
        // Injected failure between the file write and COMMIT: the transaction rolls
        // back and the guard removes exactly the file this attempt created.
        return Err(StorageError::Unavailable);
    }
    tx.commit().map_err(sql_error)?;
    published.keep();
    Ok(CommittedImage {
        record_id: command.record_id,
        asset_id: command.asset_id,
        created_at,
        relative_path,
        size_bytes,
        sha256,
    })
}

fn stored_asset(
    row: &(String, String, String, String, String, i64, String, String),
) -> Result<Asset, StorageError> {
    let (asset_id, record_id, asset_type, format, path, size_bytes, sha256, created_at) = row;
    if format != CANONICAL_FORMAT_WEBP_LOSSLESS || *size_bytes <= 0 {
        return Err(StorageError::IntegrityFailed);
    }
    Ok(Asset {
        id: AssetId::new(identity(asset_id)?),
        record_id: RecordId::new(identity(record_id)?),
        asset_type: AssetType::parse(asset_type)?,
        format: CANONICAL_FORMAT_WEBP_LOSSLESS,
        relative_path: path.clone(),
        size_bytes: *size_bytes as u64,
        sha256: sha256.clone(),
        created_at: created_at.clone(),
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
            "SELECT record_pk, id, created_at, capture_context FROM records WHERE record_pk <= ?1 AND record_pk < ?2 ORDER BY record_pk DESC LIMIT ?3",
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
             FROM assets a WHERE a.record_id IN (SELECT id FROM records WHERE record_pk <= ?1 AND record_pk < ?2 ORDER BY record_pk DESC LIMIT ?3) \
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
