use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior};
use sha2::{Digest, Sha256};
use sm_application::storage::{
    ImagesSchema, SchemaSnapshot, StorageError, CANONICAL_FORMAT_WEBP_LOSSLESS,
};
use std::{io::Read, path::Path};
use uuid::Uuid;

/// Shipped migrations in application order. `0001` owns the foundation tables;
/// `0002` adds the S02 images schema (`records`, `assets`); `0003` adds the durable
/// capture-operation journal (`ingest_operations`); `0004` adds pending capture
/// intents without changing the images contract. All are applied in a single
/// transaction on a fresh file; interrupted upgrades roll back their migration rows.
const SHIPPED: [(&str, &str); 9] = [
    (
        "0001_foundation",
        include_str!("../migrations/0001_foundation.sql"),
    ),
    ("0002_images", include_str!("../migrations/0002_images.sql")),
    (
        "0003_operations",
        include_str!("../migrations/0003_operations.sql"),
    ),
    (
        "0004_capture_intents",
        include_str!("../migrations/0004_capture_intents.sql"),
    ),
    (
        "0005_import_export",
        include_str!("../migrations/0005_import_export.sql"),
    ),
    (
        "0006_processing",
        include_str!("../migrations/0006_processing.sql"),
    ),
    (
        "0007_knowledge",
        include_str!("../migrations/0007_knowledge.sql"),
    ),
    ("0008_search", include_str!("../migrations/0008_search.sql")),
    (
        "0009_agent_access",
        include_str!("../migrations/0009_agent_access.sql"),
    ),
];
/// Applied images schema version, reported by `SchemaSnapshot::images`. The
/// `0003_operations` and `0004_capture_intents` are global journal migrations;
/// the images contract stays at `0002` while the global version advances.
const IMAGES_VERSION: u32 = 2;
#[cfg(test)]
mod access_migration_tests;
pub(super) const BOOTSTRAP: &str = "CREATE TABLE schema_migrations (\n    version       INTEGER PRIMARY KEY CHECK (version > 0),\n    name          TEXT NOT NULL,\n    checksum      TEXT NOT NULL,\n    applied_at    TEXT NOT NULL\n)";
// Candidate-only foundation format. Written on a newly created file, before WAL
// and before the atomic schema migration; never stamped onto an existing DB.
pub(super) const OWNER_ID: u32 = 0x534D_4631; // "SMF1"; not an authentication secret.

pub(super) fn mark_new_file(db: &Connection) -> Result<(), StorageError> {
    db.execute_batch(&format!("PRAGMA application_id={OWNER_ID};"))
        .map_err(|_| StorageError::MigrationFailed)?;
    let value: u32 = db
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(|_| StorageError::MigrationFailed)?;
    if value != OWNER_ID {
        return Err(StorageError::MigrationFailed);
    }
    Ok(())
}

// Main-file header is a provenance gate, NOT a replacement for the live SQLite
// compatibility check below. Marker is committed in rollback-journal mode before
// WAL is enabled, so its absence in the main file cannot be excused by a WAL.
fn has_owner_header(path: &Path) -> Result<bool, StorageError> {
    let mut file = std::fs::File::open(path).map_err(|_| StorageError::PermissionDenied)?;
    let mut header = [0u8; 72];
    if file.read_exact(&mut header).is_err() {
        return Ok(false);
    }
    Ok(&header[..16] == b"SQLite format 3\0" && header[68..72] == OWNER_ID.to_be_bytes())
}

fn objects(db: &Connection) -> Result<Vec<(String, String)>, StorageError> {
    let mut stmt = db.prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT GLOB 'sqlite_*' ORDER BY name")
        .map_err(|_| StorageError::SchemaIncompatible)?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| StorageError::SchemaIncompatible)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| StorageError::SchemaIncompatible)
}

/// Derive the exact schema by applying the shipped prefix to an empty memory DB.
/// Unlike splitting DDL text, this also models ALTER/rename/rebuild migrations.
fn expected_objects(version: usize) -> Vec<(String, String)> {
    let db = Connection::open_in_memory().expect("shipped schema model");
    db.execute_batch(BOOTSTRAP).expect("shipped bootstrap");
    for (_, migration) in SHIPPED.iter().take(version) {
        db.execute_batch(migration)
            .expect("shipped migration syntax");
    }
    objects(&db).expect("shipped schema objects")
}

fn table_only() -> Vec<(String, String)> {
    vec![("schema_migrations".to_owned(), BOOTSTRAP.to_owned())]
}

fn checksum_of(migration: &str) -> String {
    format!("{:x}", Sha256::digest(migration.as_bytes()))
}

/// Migration rows must be exactly the shipped prefix, in order, with shipped names.
fn applied_version(db: &Connection) -> Result<usize, StorageError> {
    let rows = db
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
        .and_then(|mut stmt| {
            stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| StorageError::SchemaIncompatible)?;
    if rows.is_empty() || rows.len() > SHIPPED.len() {
        return Err(StorageError::SchemaIncompatible);
    }
    for (index, (version, name, checksum)) in rows.iter().enumerate() {
        let (expected_name, migration) = SHIPPED[index];
        if *version != index as i64 + 1
            || name != expected_name
            || checksum != &checksum_of(migration)
        {
            return Err(StorageError::SchemaIncompatible);
        }
    }
    Ok(rows.len())
}

fn validate_settings(db: &Connection) -> Result<bool, StorageError> {
    let mut stmt = db
        .prepare("SELECT key, value_json, revision FROM library_settings ORDER BY key")
        .map_err(|_| StorageError::IntegrityFailed)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(|_| StorageError::IntegrityFailed)?;
    let rows = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StorageError::IntegrityFailed)?;
    if rows.len() != 2 || rows[0].0 != "ai.enabled" || rows[1].0 != "model.autostart" {
        return Err(StorageError::IntegrityFailed);
    }
    let processing_exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='processing_policy')",
            [],
            |r| r.get(0),
        )
        .map_err(|_| StorageError::IntegrityFailed)?;
    let enabled = if processing_exists {
        crate::processing::policy(db)
            .map_err(|_| StorageError::IntegrityFailed)?
            .value
            .mode
            != sm_protocol::processing::ProcessingMode::Off
    } else {
        false
    };
    for (key, json, rev) in &rows {
        let expected = key == "ai.enabled" && enabled;
        if *rev < 1 || serde_json::from_str::<bool>(json).ok() != Some(expected) {
            return Err(StorageError::IntegrityFailed);
        }
    }
    Ok(enabled)
}

/// Identity and settings checks shared by every accepted version.
fn validate_identity(db: &Connection) -> Result<Uuid, StorageError> {
    let metas = db
        .prepare("SELECT singleton, library_id, format_major, created_at FROM library_meta")
        .and_then(|mut stmt| {
            stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| StorageError::IntegrityFailed)?;
    if metas.len() != 1 || metas[0].0 != 1 || metas[0].2 != 1 || metas[0].3.is_empty() {
        return Err(StorageError::IntegrityFailed);
    }
    let library_id = Uuid::parse_str(&metas[0].1).map_err(|_| StorageError::IntegrityFailed)?;
    if library_id.is_nil() || library_id.to_string() != metas[0].1 {
        return Err(StorageError::IntegrityFailed);
    }
    validate_settings(db)?;
    Ok(library_id)
}

/// Structural check of exactly the applied version: object set and identity.
fn validate_applied(db: &Connection, version: usize) -> Result<Uuid, StorageError> {
    if objects(db)? != expected_objects(version) {
        return Err(StorageError::IntegrityFailed);
    }
    validate_identity(db)
}

/// Full validation of an opened library; every shipped migration must be applied.
pub(super) fn validate(db: &Connection) -> Result<SchemaSnapshot, StorageError> {
    let version = applied_version(db)?;
    if version != SHIPPED.len() {
        return Err(StorageError::SchemaIncompatible);
    }
    let library_id = validate_applied(db, version)?;
    Ok(SchemaSnapshot {
        library_id,
        schema_version: version as u32,
        format_major: 1,
        images: ImagesSchema {
            migration_version: IMAGES_VERSION,
            canonical_format: CANONICAL_FORMAT_WEBP_LOSSLESS,
        },
    })
}

/// Read-only preflight precedes *all* writer connection pragmas. No immutable URI:
/// a real WAL library must see its uncheckpointed commits.
pub(super) fn preflight(path: &Path, existed: bool) -> Result<bool, StorageError> {
    if !existed {
        return Ok(true); // DatabaseEntry already rejects stray sidecars.
    }
    // Reject unclaimed/zero-byte/foreign files without invoking SQLite: even a
    // read-only SQLite open can recover a journal or create shared-memory files.
    if !has_owner_header(path)? {
        return Err(StorageError::SchemaIncompatible);
    }
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|err| match err {
        rusqlite::Error::SqliteFailure(code, _) if code.code == ErrorCode::CannotOpen => {
            StorageError::PermissionDenied
        }
        _ => StorageError::SchemaIncompatible,
    })?;
    let owner: u32 = db
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(|_| StorageError::SchemaIncompatible)?;
    if owner != OWNER_ID {
        return Err(StorageError::SchemaIncompatible);
    }
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|_| StorageError::SchemaIncompatible)?;
    if version != 0 {
        return Err(StorageError::SchemaIncompatible);
    }
    let found = objects(&db)?;
    if found.is_empty() {
        return Ok(true);
    }
    if found == table_only() {
        let count: i64 = db
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .map_err(|_| StorageError::SchemaIncompatible)?;
        if count == 0 {
            return Ok(true);
        }
    }
    // A valid library is either the accepted foundation version awaiting its
    // upgrade or the fully migrated version; anything else is refused here.
    let applied = applied_version(&db)?;
    if found != expected_objects(applied) {
        return Err(StorageError::SchemaIncompatible);
    }
    validate_applied(&db, applied)?;
    // Populated pre-S03 libraries require a verified backup before the asset-table
    // structural rebuild. S08 owns backup authorization; no production bypass flag.
    // This read-only refusal precedes writer PRAGMAs and all destructive SQL.
    if (2..5).contains(&applied) {
        let populated: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM records) OR EXISTS(SELECT 1 FROM assets)",
                [],
                |r| r.get(0),
            )
            .map_err(|_| StorageError::SchemaIncompatible)?;
        if populated {
            return Err(StorageError::SchemaIncompatible);
        }
    }
    Ok(false)
}

/// Apply every shipped migration to an owned schema-empty database or upgrade an
/// accepted earlier version. DDL and its migration row share one transaction.
pub(super) fn initialize(
    db: &mut Connection,
    fresh: bool,
    fail_before_commit: bool,
) -> Result<SchemaSnapshot, StorageError> {
    if !fresh {
        let current = applied_version(db)?;
        if current < SHIPPED.len() {
            upgrade(db, current, fail_before_commit)?;
        }
        return validate(db);
    }
    let tx = db
        .transaction()
        .map_err(|_| StorageError::MigrationFailed)?;
    let found = objects(&tx)?;
    if found.is_empty() {
        tx.execute_batch(BOOTSTRAP)
            .map_err(|_| StorageError::MigrationFailed)?;
    } else if found != table_only() {
        return Err(StorageError::SchemaIncompatible);
    }
    for (_, migration) in SHIPPED {
        tx.execute_batch(migration)
            .map_err(|_| StorageError::MigrationFailed)?;
    }
    let library_id = Uuid::new_v4();
    tx.execute(
        "INSERT INTO library_meta VALUES (1, ?1, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        [library_id.to_string()],
    )
    .map_err(|_| StorageError::MigrationFailed)?;
    for key in ["ai.enabled", "model.autostart"] {
        tx.execute(
            "INSERT INTO library_settings VALUES (?1, 'false', 1)",
            [key],
        )
        .map_err(|_| StorageError::MigrationFailed)?;
    }
    validate_settings(&tx)?;
    record_migrations(&tx, 0)?;
    if fail_before_commit {
        return Err(StorageError::MigrationFailed);
    }
    tx.commit().map_err(|_| StorageError::MigrationFailed)?;
    validate(db)
}

/// Upgrade an accepted earlier version inside one transaction; a failure leaves
/// the previous schema, its rows and its file bytes untouched.
fn upgrade(
    db: &mut Connection,
    current: usize,
    fail_before_commit: bool,
) -> Result<(), StorageError> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| StorageError::MigrationFailed)?;
    if applied_version(&tx)? != current {
        return Err(StorageError::SchemaIncompatible);
    }
    for (_, migration) in SHIPPED.iter().skip(current) {
        tx.execute_batch(migration)
            .map_err(|_| StorageError::MigrationFailed)?;
    }
    record_migrations(&tx, current)?;
    if fail_before_commit {
        return Err(StorageError::MigrationFailed);
    }
    tx.commit().map_err(|_| StorageError::MigrationFailed)
}

/// Register the shipped migration rows that are not applied yet, in order.
fn record_migrations(tx: &rusqlite::Transaction<'_>, from: usize) -> Result<(), StorageError> {
    for (index, (name, migration)) in SHIPPED.iter().enumerate().skip(from) {
        tx.execute(
            "INSERT INTO schema_migrations VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            (index as i64 + 1, name, checksum_of(migration)),
        )
        .map_err(|_| StorageError::MigrationFailed)?;
    }
    Ok(())
}

pub(super) fn pragmas(db: &Connection) -> Result<(), StorageError> {
    db.busy_timeout(std::time::Duration::from_millis(5000))
        .map_err(|_| StorageError::Unavailable)?;
    let journal: String = db
        .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
        .map_err(|_| StorageError::Unavailable)?;
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
        .map_err(|_| StorageError::Unavailable)?;
    let read_string = |sql| {
        db.query_row(sql, [], |r| r.get::<_, String>(0))
            .map_err(|_| StorageError::Unavailable)
    };
    let read_int = |sql| {
        db.query_row(sql, [], |r| r.get::<_, i64>(0))
            .map_err(|_| StorageError::Unavailable)
    };
    if journal != "wal"
        || read_string("PRAGMA journal_mode")? != "wal"
        || read_int("PRAGMA foreign_keys")? != 1
        || read_int("PRAGMA synchronous")? != 2
        || read_int("PRAGMA busy_timeout")? != 5000
    {
        return Err(StorageError::Unavailable);
    }
    Ok(())
}

pub(super) fn sqlite_version(db: &Connection) -> Result<String, StorageError> {
    db.query_row("SELECT sqlite_version()", [], |r| r.get(0))
        .map_err(|_| StorageError::Unavailable)
}
