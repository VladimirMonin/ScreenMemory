use rusqlite::{Connection, ErrorCode, OpenFlags};
use sha2::{Digest, Sha256};
use sm_application::storage::{SchemaSnapshot, StorageError};
use std::{io::Read, path::Path};
use uuid::Uuid;

const MIGRATION: &str = include_str!("../migrations/0001_foundation.sql");
pub(super) const BOOTSTRAP: &str = "CREATE TABLE schema_migrations (\n    version       INTEGER PRIMARY KEY CHECK (version > 0),\n    name          TEXT NOT NULL,\n    checksum      TEXT NOT NULL,\n    applied_at    TEXT NOT NULL\n)";
const NAME: &str = "0001_foundation";
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
fn expected() -> Vec<(String, String)> {
    let mut names = vec![("schema_migrations".to_owned(), BOOTSTRAP.to_owned())];
    for ddl in MIGRATION
        .split(";\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let table = ddl
            .split_whitespace()
            .nth(2)
            .expect("shipped migration syntax");
        names.push((table.to_owned(), ddl.to_owned()));
    }
    names.sort();
    names
}
fn table_only() -> Vec<(String, String)> {
    vec![("schema_migrations".to_owned(), BOOTSTRAP.to_owned())]
}
fn checksum() -> String {
    format!("{:x}", Sha256::digest(MIGRATION.as_bytes()))
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
    for (_, json, rev) in &rows {
        if *rev < 1
            || !matches!(
                serde_json::from_str::<serde_json::Value>(json),
                Ok(serde_json::Value::Bool(false))
            )
        {
            return Err(StorageError::IntegrityFailed);
        }
    }
    Ok(false)
}

pub(super) fn validate(db: &Connection) -> Result<SchemaSnapshot, StorageError> {
    let migrations = db
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
    if migrations.len() != 1 || migrations[0].0 != 1 || migrations[0].1 != NAME {
        return Err(StorageError::SchemaIncompatible);
    }
    if migrations[0].2 != checksum() || objects(db)? != expected() {
        return Err(StorageError::IntegrityFailed);
    }
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
    Ok(SchemaSnapshot {
        library_id,
        schema_version: 1,
        format_major: 1,
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
    if found != expected() {
        return Err(StorageError::SchemaIncompatible);
    }
    validate(&db)?;
    Ok(false)
}

pub(super) fn initialize(
    db: &mut Connection,
    fresh: bool,
    fail_before_commit: bool,
) -> Result<SchemaSnapshot, StorageError> {
    if !fresh {
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
    tx.execute_batch(MIGRATION)
        .map_err(|_| StorageError::MigrationFailed)?;
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
    tx.execute(
        "INSERT INTO schema_migrations VALUES (1, ?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        (NAME, checksum()),
    )
    .map_err(|_| StorageError::MigrationFailed)?;
    if fail_before_commit {
        return Err(StorageError::MigrationFailed);
    }
    tx.commit().map_err(|_| StorageError::MigrationFailed)?;
    validate(db)
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
