//! Conditional, storage-only pending-to-Saved transaction. All file IO happens on
//! the file lane before BEGIN; the seal remains owned through COMMIT/rollback.
//! This is not a sandbox against preexisting same-user writable mappings/admin/raw volume.
use crate::{images, recovery_candidate::SealedPublished};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sm_application::storage::{
    provenances_to_json, warnings_to_json, AssetType, CommittedImage, Provenance,
    ReadIntentOutcome, StorageError, CANONICAL_FORMAT_WEBP_LOSSLESS, CANONICAL_MIME_WEBP,
};
use std::convert::TryFrom;

fn sql_error(error: rusqlite::Error) -> StorageError {
    match error {
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            StorageError::IntegrityFailed
        }
        _ => StorageError::Unavailable,
    }
}

// Check ownership and raw mode BEFORE projecting any saved metadata, even malformed rows.
// A saved row is replayable only when all fields match the original sealed witness.
fn saved_replay(
    db: &Connection,
    expected: &CommittedImage,
    library: &str,
    fingerprint: &str,
    context: &str,
    provenance: &str,
) -> Result<Option<CommittedImage>, StorageError> {
    let operation = expected.operation_id.as_uuid().to_string();
    let prefix: Option<(String, String)> = db
        .query_row(
            "SELECT principal_id, mode FROM ingest_operations WHERE operation_id=?1",
            [&operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    let Some((principal, mode)) = prefix else {
        return Ok(None);
    };
    if principal != expected.principal_id.as_uuid().to_string() || mode != expected.mode.as_str() {
        return Err(StorageError::IntegrityFailed);
    }
    // The saved branch must check the current singleton too; the pending-only
    // check in commit() cannot guard an already committed receipt.
    let current: Option<String> = db
        .query_row(
            "SELECT library_id FROM library_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    if current.as_deref() != Some(library) {
        return Err(StorageError::IntegrityFailed);
    }
    let (saved_fingerprint, receipt) = images::saved_operation(db, &operation, &principal)?
        .ok_or(StorageError::IntegrityFailed)?;
    if saved_fingerprint != fingerprint || receipt != *expected {
        return Err(StorageError::IntegrityFailed);
    }
    // The replay must agree with the complete original fingerprint inputs, not
    // merely the joined asset and the fields projected into the public receipt.
    let stored: (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    ) = db
        .query_row(
            "SELECT io.kind, io.planned_record_id, io.planned_asset_id, io.content_sha256, \
                    r.capture_context, r.provenances, a.asset_type, r.created_at, r.library_id, io.created_at_utc \
             FROM ingest_operations io \
             JOIN records r ON r.id=io.record_id \
             JOIN assets a ON a.id=io.planned_asset_id AND a.record_id=r.id \
             WHERE io.operation_id=?1",
            [&operation],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .optional()
        .map_err(sql_error)?
        .ok_or(StorageError::IntegrityFailed)?;
    if stored.0 != "capture"
        || stored.1 != expected.record_id.as_uuid().to_string()
        || stored.2 != expected.asset_id.as_uuid().to_string()
        || stored.3 != expected.sha256
        || stored.4 != context
        || stored.5 != provenance
        || stored.6 != AssetType::Original.as_str()
        || stored.7 != expected.created_at
        || stored.8 != library
        || stored.9 != expected.created_at
    {
        return Err(StorageError::IntegrityFailed);
    }
    Ok(Some(receipt))
}

pub(super) fn commit(
    db: &mut Connection,
    sealed: SealedPublished,
    fail_after_inserts: bool,
    #[cfg(test)] crash_gate: Option<&crate::recovery_process_tests::CrashGate>,
) -> Result<CommittedImage, StorageError> {
    // `sealed` owns the write-denying HANDLE and ancestor pins until this method exits.
    // Never read the file or run codec in the SQL transaction.
    let candidate = sealed.candidate();
    let witness = candidate.intent();
    let input = &witness.input;
    let operation = input.request.operation_id().to_string();
    let principal = input.principal_id.as_uuid().to_string();
    let record = input.record_id.as_uuid().to_string();
    let asset = input.asset_id.as_uuid().to_string();
    let library = input.library_id.as_uuid().to_string();
    let created = &witness.planned_created_at_utc;
    let context = input
        .context
        .to_json()
        .map_err(|_| StorageError::IntegrityFailed)?;
    let provenance = provenances_to_json(&[Provenance::Automatic {
        when: input.captured_at_utc.clone(),
        source: input.context.target,
    }])
    .map_err(|_| StorageError::IntegrityFailed)?;
    let warnings = warnings_to_json(&input.warnings).map_err(|_| StorageError::IntegrityFailed)?;
    let sha = format!("{:x}", sha2::Sha256::digest(candidate.encoded_bytes()));
    if candidate.encoded_sha256().as_slice()
        != sha2::Sha256::digest(candidate.encoded_bytes()).as_slice()
        || candidate.length() != candidate.encoded_bytes().len() as u64
    {
        return Err(StorageError::IntegrityFailed);
    }
    // CanonicalPlan verifies the Gregorian timestamp; path has no caller component.
    sm_isolation::CanonicalPlan::new(input.record_id, created)
        .map_err(|_| StorageError::IntegrityFailed)?;
    let month = &created[..7];
    let path = format!("assets/{month}/{record}/original.webp");
    let fingerprint = images::semantic_fingerprint(
        AssetType::Original.as_str(),
        input.request.target().as_str(),
        &sha,
        &context,
        &provenance,
    );
    let expected = CommittedImage {
        operation_id: sm_domain::OperationId::new(input.request.operation_id()),
        principal_id: input.principal_id,
        record_id: input.record_id,
        asset_id: input.asset_id,
        mode: input.request.target(),
        width: input.width,
        height: input.height,
        canonical_mime: CANONICAL_MIME_WEBP,
        warnings: input.warnings.clone(),
        created_at: created.clone(),
        relative_path: path,
        size_bytes: candidate.length(),
        sha256: sha,
    };
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    // Saved-first scoped read. A foreign or changed mode never exposes metadata.
    if let Some(saved) = saved_replay(
        &tx,
        &expected,
        &library,
        &fingerprint,
        &context,
        &provenance,
    )? {
        return Ok(saved);
    }
    let current: String = tx
        .query_row(
            "SELECT library_id FROM library_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if current != library {
        return Err(StorageError::IntegrityFailed);
    }
    let pending = crate::intents::read(
        &tx,
        input.library_id,
        input.principal_id,
        input.request.clone(),
    )?;
    let ReadIntentOutcome::Pending(actual) = pending else {
        return Err(StorageError::IntegrityFailed);
    };
    if *actual != *witness {
        return Err(StorageError::IntegrityFailed);
    }
    if tx.execute("DELETE FROM capture_intents WHERE operation_id=?1 AND principal_id=?2 AND library_id=?3 AND mode=?4",
        params![operation, principal, library, input.request.target().as_str()]).map_err(sql_error)? != 1 {
        return Err(StorageError::IntegrityFailed);
    }
    tx.execute("INSERT INTO records (id, library_id, created_at, capture_context, provenances) VALUES (?1,?2,?3,?4,?5)",
        params![record, library, created, context, provenance]).map_err(sql_error)?;
    let size = i64::try_from(expected.size_bytes).map_err(|_| StorageError::IntegrityFailed)?;
    tx.execute("INSERT INTO assets (id, record_id, asset_type, format, path, size_bytes, sha256, created_at) VALUES (?1,?2,'original',?3,?4,?5,?6,?7)",
        params![asset, record, CANONICAL_FORMAT_WEBP_LOSSLESS, expected.relative_path, size, expected.sha256, created]).map_err(sql_error)?;
    let updated: String = tx
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })
        .map_err(sql_error)?;
    tx.execute("INSERT INTO ingest_operations (operation_id, principal_id, kind, planned_record_id, planned_asset_id, content_sha256, semantic_fingerprint, mode, width, height, warnings, record_id, state, created_at_utc, updated_at_utc) VALUES (?1,?2,'capture',?3,?4,?5,?6,?7,?8,?9,?10,?3,'saved',?11,?12)",
        params![operation, principal, record, asset, expected.sha256, fingerprint, input.request.target().as_str(), input.width, input.height, warnings, created, updated]).map_err(sql_error)?;
    #[cfg(test)]
    if let Some(gate) = crash_gate {
        gate.checkpoint(crate::recovery_process_tests::CrashPhase::F07);
    }
    if fail_after_inserts {
        return Err(StorageError::Unavailable);
    }
    // If COMMIT returns an ambiguous error, only a confirmed persisted row can
    // produce Saved; otherwise return the error. Do not retry the INSERT blindly.
    crate::knowledge::initial(
        &tx,
        sm_protocol::knowledge::KnowledgeId(
            uuid::Uuid::parse_str(&record).map_err(|_| StorageError::IntegrityFailed)?,
        ),
    )
    .map_err(|_| StorageError::IntegrityFailed)?;
    let result = tx.commit();
    match saved_replay(db, &expected, &library, &fingerprint, &context, &provenance) {
        Ok(Some(saved)) => Ok(saved),
        _ => Err(result
            .err()
            .map(sql_error)
            .unwrap_or(StorageError::Unavailable)),
    }
}
use sha2::Digest;
