//! Pending capture-intent rows only. All SQL runs on the one SQLite worker thread.
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use sm_application::{
    capture::{CaptureRequest, CaptureTrigger, MAX_CAPTURE_PIXELS},
    storage::{
        is_canonical_utc_timestamp, warnings_from_json, warnings_to_json, CaptureContext,
        CreateCaptureIntent, CreateIntentOutcome, PendingCaptureIntent, ReadIntentOutcome,
        StorageError, MAX_CAPTURE_SIDE,
    },
};
use sm_domain::{AssetId, LibraryId, PrincipalId, RecordId};
use uuid::{Uuid, Variant};

const REQUEST_DOMAIN: &[u8] = b"ScreenMemory.capture-intent.request.v1";
const MAX_CONTEXT_BYTES: usize = 256 * 1024;

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

fn eligible(id: Uuid) -> bool {
    !id.is_nil()
}
fn v4(id: Uuid) -> bool {
    eligible(id) && id.get_variant() == Variant::RFC4122 && id.get_version_num() == 4
}
fn lower_sha(raw: &str) -> bool {
    raw.len() == 64
        && raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn request_fingerprint(request: &CaptureRequest) -> String {
    let mut hash = Sha256::new();
    hash.update(REQUEST_DOMAIN);
    for value in [
        request.operation_id().to_string(),
        request.target().as_str().to_owned(),
        request.trigger().as_str().to_owned(),
    ] {
        hash.update([0]);
        hash.update(value.as_bytes());
    }
    format!("{:x}", hash.finalize())
}
fn checked_identity(
    library: LibraryId,
    principal: PrincipalId,
    request: &CaptureRequest,
) -> Result<(), StorageError> {
    if !eligible(library.as_uuid())
        || !eligible(principal.as_uuid())
        || !eligible(request.operation_id())
    {
        return Err(StorageError::InvalidPayload);
    }
    Ok(())
}
fn checked(input: &CreateCaptureIntent) -> Result<(String, String, String), StorageError> {
    checked_identity(input.library_id, input.principal_id, &input.request)?;
    let ids = [
        input.record_id.as_uuid(),
        input.asset_id.as_uuid(),
        input.frame_id,
    ];
    if ids.iter().any(|id| !v4(*id))
        || ids[0] == ids[1]
        || ids[0] == ids[2]
        || ids[1] == ids[2]
        || ids.contains(&input.request.operation_id())
    {
        return Err(StorageError::InvalidPayload);
    }
    if input.width == 0
        || input.height == 0
        || input.width > MAX_CAPTURE_SIDE
        || input.height > MAX_CAPTURE_SIDE
        || u64::from(input.width) * u64::from(input.height) > MAX_CAPTURE_PIXELS
        || input.context.target != input.request.target()
        || input.context.bounds.width != input.width
        || input.context.bounds.height != input.height
        || input.context.sources.len() > 4
        || !lower_sha(&input.pixel_sha256)
        || input.encoder_fingerprint.is_empty()
        || input.encoder_fingerprint.len() > 512
        || !is_canonical_utc_timestamp(&input.captured_at_utc)
    {
        return Err(StorageError::InvalidPayload);
    }
    let context_json = input.context.to_json()?;
    if context_json.len() > MAX_CONTEXT_BYTES {
        return Err(StorageError::InvalidPayload);
    }
    let warnings_json = warnings_to_json(&input.warnings)?;
    if warnings_json.len() > 8192 {
        return Err(StorageError::InvalidPayload);
    }
    let context_sha = format!("{:x}", Sha256::digest(context_json.as_bytes()));
    Ok((context_json, warnings_json, context_sha))
}
fn library(db: &Connection, expected: LibraryId) -> Result<(), StorageError> {
    let actual: String = db
        .query_row(
            "SELECT library_id FROM library_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if actual != expected.as_uuid().to_string() {
        return Err(StorageError::IntegrityFailed);
    }
    Ok(())
}
fn saved(db: &Connection, operation: Uuid) -> Result<bool, StorageError> {
    db.query_row(
        "SELECT 1 FROM ingest_operations WHERE operation_id=?1",
        [operation.to_string()],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map(|v| v.is_some())
    .map_err(sql_error)
}

// Read only the authentication prefix before projecting ANY pending metadata.
fn scoped_row(
    db: &Connection,
    principal: PrincipalId,
    request: &CaptureRequest,
) -> Result<Option<bool>, StorageError> {
    let prefix: Option<(String, String, String)> = db.query_row(
        "SELECT principal_id, mode, request_fingerprint FROM capture_intents WHERE operation_id=?1",
        [request.operation_id().to_string()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional().map_err(sql_error)?;
    let Some((owner, mode, fingerprint)) = prefix else {
        return Ok(None);
    };
    // A foreign principal or different mode cannot inspect even a corrupted digest.
    if owner != principal.as_uuid().to_string() || mode != request.target().as_str() {
        return Ok(Some(false));
    }
    if fingerprint == request_fingerprint(request) {
        return Ok(Some(true));
    }
    // The stored trigger is not a column: only the two closed v1 trigger digests
    // are possible. A real alternate request conflicts; arbitrary hex is corruption.
    let alternate_trigger = match request.trigger() {
        CaptureTrigger::Hotkey => CaptureTrigger::ClientCommand,
        CaptureTrigger::ClientCommand => CaptureTrigger::Hotkey,
    };
    let alternate = CaptureRequest::new(request.operation_id(), request.mode(), alternate_trigger);
    if fingerprint == request_fingerprint(&alternate) {
        Ok(Some(false))
    } else {
        Err(StorageError::IntegrityFailed)
    }
}

// Projection order follows the immutable v4 table, after the scoped prefix gate.
type Raw = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);
fn owned_row(
    db: &Connection,
    library_id: LibraryId,
    principal_id: PrincipalId,
    request: CaptureRequest,
) -> Result<PendingCaptureIntent, StorageError> {
    let kind: String = db
        .query_row(
            "SELECT kind FROM capture_intents WHERE operation_id=?1",
            [request.operation_id().to_string()],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if kind != "capture" {
        return Err(StorageError::IntegrityFailed);
    }
    let raw: Raw = db.query_row(
        "SELECT library_id, principal_id, planned_record_id, planned_asset_id, frame_id, mode, pixel_sha256, width, height, context_sha256, request_fingerprint, context_json, warnings, encoder_fingerprint, captured_at_utc, planned_created_at_utc FROM capture_intents WHERE operation_id=?1",
        [request.operation_id().to_string()],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?,r.get(11)?,r.get(12)?,r.get(13)?,r.get(14)?,r.get(15)?)),
    ).map_err(sql_error)?;
    let (
        lib,
        owner,
        rec,
        asset,
        frame,
        mode,
        pixels,
        width,
        height,
        context_sha,
        fingerprint,
        context_json,
        warnings_json,
        encoder,
        captured,
        planned,
    ) = raw;
    if lib != library_id.as_uuid().to_string()
        || owner != principal_id.as_uuid().to_string()
        || mode != request.target().as_str()
        || fingerprint != request_fingerprint(&request)
        || !is_canonical_utc_timestamp(&planned)
        || !lower_sha(&context_sha)
        || context_json.len() > MAX_CONTEXT_BYTES
        || warnings_json.len() > 8192
        || format!("{:x}", Sha256::digest(context_json.as_bytes())) != context_sha
    {
        return Err(StorageError::IntegrityFailed);
    }
    let parse_v4 = |raw: String| -> Result<Uuid, StorageError> {
        let id = Uuid::parse_str(&raw).map_err(|_| StorageError::IntegrityFailed)?;
        if !v4(id) || id.to_string() != raw {
            return Err(StorageError::IntegrityFailed);
        }
        Ok(id)
    };
    let input = CreateCaptureIntent {
        library_id,
        principal_id,
        request,
        record_id: RecordId::new(parse_v4(rec)?),
        asset_id: AssetId::new(parse_v4(asset)?),
        frame_id: parse_v4(frame)?,
        width: u32::try_from(width).map_err(|_| StorageError::IntegrityFailed)?,
        height: u32::try_from(height).map_err(|_| StorageError::IntegrityFailed)?,
        pixel_sha256: pixels,
        context: CaptureContext::from_json(&context_json)
            .map_err(|_| StorageError::IntegrityFailed)?,
        warnings: warnings_from_json(&warnings_json).map_err(|_| StorageError::IntegrityFailed)?,
        encoder_fingerprint: encoder,
        captured_at_utc: captured,
    };
    let (canonical_context, canonical_warnings, calculated_sha) =
        checked(&input).map_err(|_| StorageError::IntegrityFailed)?;
    if canonical_context != context_json
        || canonical_warnings != warnings_json
        || calculated_sha != context_sha
    {
        return Err(StorageError::IntegrityFailed);
    }
    Ok(PendingCaptureIntent {
        input,
        planned_created_at_utc: planned,
    })
}

pub(super) fn create(
    db: &mut Connection,
    input: CreateCaptureIntent,
) -> Result<CreateIntentOutcome, StorageError> {
    // No metadata projection, timestamp allocation or write before the Saved check.
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    if saved(&tx, input.request.operation_id())? {
        return Ok(CreateIntentOutcome::Conflict);
    }
    checked_identity(input.library_id, input.principal_id, &input.request)?;
    library(&tx, input.library_id)?;
    if let Some(owned) = scoped_row(&tx, input.principal_id, &input.request)? {
        if !owned {
            return Ok(CreateIntentOutcome::Conflict);
        }
        let previous = owned_row(
            &tx,
            input.library_id,
            input.principal_id,
            input.request.clone(),
        )?;
        return if previous.input == input {
            Ok(CreateIntentOutcome::Existing(Box::new(previous)))
        } else {
            // A changed caller payload is an idempotency conflict, not stored corruption.
            Ok(CreateIntentOutcome::Conflict)
        };
    }
    let (context_json, warnings_json, context_sha) = checked(&input)?;
    let planned: String = tx
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })
        .map_err(sql_error)?;
    if !is_canonical_utc_timestamp(&planned) {
        return Err(StorageError::IntegrityFailed);
    }
    tx.execute("INSERT INTO capture_intents (operation_id, library_id, principal_id, kind, planned_record_id, planned_asset_id, frame_id, mode, width, height, pixel_sha256, context_sha256, request_fingerprint, context_json, warnings, encoder_fingerprint, captured_at_utc, planned_created_at_utc) VALUES (?1,?2,?3,'capture',?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![input.request.operation_id().to_string(), input.library_id.as_uuid().to_string(), input.principal_id.as_uuid().to_string(), input.record_id.as_uuid().to_string(), input.asset_id.as_uuid().to_string(), input.frame_id.to_string(), input.request.target().as_str(), input.width, input.height, input.pixel_sha256, context_sha, request_fingerprint(&input.request), context_json, warnings_json, input.encoder_fingerprint, input.captured_at_utc, planned]
    ).map_err(sql_error)?;
    tx.commit().map_err(sql_error)?;
    Ok(CreateIntentOutcome::Created(Box::new(
        PendingCaptureIntent {
            input,
            planned_created_at_utc: planned,
        },
    )))
}

pub(super) fn read(
    db: &Connection,
    library_id: LibraryId,
    principal_id: PrincipalId,
    request: CaptureRequest,
) -> Result<ReadIntentOutcome, StorageError> {
    checked_identity(library_id, principal_id, &request)?;
    if saved(db, request.operation_id())? {
        return Ok(ReadIntentOutcome::Conflict);
    }
    library(db, library_id)?;
    match scoped_row(db, principal_id, &request)? {
        None => Ok(ReadIntentOutcome::Unknown),
        Some(false) => Ok(ReadIntentOutcome::Conflict),
        Some(true) => owned_row(db, library_id, principal_id, request)
            .map(|pending| ReadIntentOutcome::Pending(Box::new(pending))),
    }
}
