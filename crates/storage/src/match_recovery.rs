//! Pure equality gate between a validated SQLite pending intent and one reopened v2 stage.
//! No file, codec, SQL, receipt or path authority is obtained here.
use crate::{
    staging::{ValidatedRecoveryEnvelope, RECOVERY_PATH_POLICY},
    staging_files::ReopenedV2,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use sm_application::{
    capture::CaptureRequest,
    storage::{CaptureContext, PendingCaptureIntent, Provenance, ReadIntentOutcome, StorageError},
};
use sm_domain::{LibraryId, PrincipalId};

/// Deliberately content-free refusal: neither stored nor staged metadata is disclosed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchRecoveryError {
    Mismatch,
    IntegrityFailed,
}

/// Storage-owned proof of one scoped, fully validated SQLite pending-row read.
/// Fields and construction are private: a mutable application DTO (including its
/// caller-writable planned date) cannot be used as a matching authority.
/// This is a read-time snapshot, not a reservation or proof the DB cannot change later.
#[derive(Debug)]
pub struct DbValidatedIntent {
    intent: PendingCaptureIntent,
}

impl DbValidatedIntent {
    pub(crate) fn trusted(&self) -> &PendingCaptureIntent {
        &self.intent
    }
}

/// No metadata for a missing, Saved, foreign or conflicting operation.
#[derive(Debug)]
pub enum ValidatedIntentRead {
    Pending(Box<DbValidatedIntent>),
    Unknown,
    Conflict,
}

// Called only by the scoped SQLite worker request; reuses the existing Saved-first,
// ownership/fingerprint and complete row-validation path without a second projection.
pub(crate) fn read_validated(
    db: &Connection,
    library_id: LibraryId,
    principal_id: PrincipalId,
    request: CaptureRequest,
) -> Result<ValidatedIntentRead, StorageError> {
    match crate::intents::read(db, library_id, principal_id, request)? {
        ReadIntentOutcome::Pending(intent) => {
            Ok(ValidatedIntentRead::Pending(Box::new(DbValidatedIntent {
                intent: *intent,
            })))
        }
        ReadIntentOutcome::Unknown => Ok(ValidatedIntentRead::Unknown),
        ReadIntentOutcome::Conflict => Ok(ValidatedIntentRead::Conflict),
    }
}

/// Borrowed, immutable view; the SQLite timestamp is not supplied by the stage or caller.
/// This value is not a saved receipt, grants no path/commit authority, and the DB
/// snapshot may change after this read.
#[derive(Debug)]
pub struct MatchedRecovery<'a> {
    intent: &'a DbValidatedIntent,
    reopened: &'a ReopenedV2,
}

impl MatchedRecovery<'_> {
    pub fn pixels(&self) -> &[u8] {
        self.reopened.pixels()
    }

    pub fn context(&self) -> &CaptureContext {
        &self.intent.intent.input.context
    }

    pub fn planned_created_at_utc(&self) -> &str {
        &self.intent.intent.planned_created_at_utc
    }

    pub fn captured_at_utc(&self) -> &str {
        &self.intent.intent.input.captured_at_utc
    }

    pub fn warnings(&self) -> &[String] {
        &self.intent.intent.input.warnings
    }
}

/// Check only exact equality. A valid v1 input has no warnings and can never match.
/// A reopened v2's pixels and envelope have already been validated together, but the
/// digest is recomputed here to keep the borrowed view tied to the same bytes.
///
/// A public, mutable application DTO is not a DB witness, even if its contents
/// appear identical. In particular its planned month can be forged:
/// ```compile_fail
/// use sm_application::storage::PendingCaptureIntent;
/// use sm_storage::{match_recovery::match_pending_reopened_v2, staging_files::ReopenedV2};
/// fn forged(dto: &PendingCaptureIntent, stage: &ReopenedV2) {
///     let _ = match_pending_reopened_v2(dto, stage);
/// }
/// ```
/// Witness construction is private to storage:
/// ```compile_fail
/// use sm_application::storage::PendingCaptureIntent;
/// use sm_storage::match_recovery::DbValidatedIntent;
/// fn forge(dto: PendingCaptureIntent) -> DbValidatedIntent {
///     DbValidatedIntent { intent: dto }
/// }
/// ```
pub fn match_pending_reopened_v2<'a>(
    intent: &'a DbValidatedIntent,
    reopened: &'a ReopenedV2,
) -> Result<MatchedRecovery<'a>, MatchRecoveryError> {
    match_envelope(&intent.intent, reopened.envelope(), reopened.pixels())?;
    Ok(MatchedRecovery { intent, reopened })
}

fn match_envelope(
    intent: &PendingCaptureIntent,
    envelope: &ValidatedRecoveryEnvelope,
    pixels: &[u8],
) -> Result<(), MatchRecoveryError> {
    let input = &intent.input;
    let stage = envelope.staging();
    let context_json = input
        .context
        .to_json()
        .map_err(|_| MatchRecoveryError::IntegrityFailed)?;
    let expected_context_sha = format!("{:x}", Sha256::digest(context_json.as_bytes()));
    let expected_pixel_sha = format!("{:x}", Sha256::digest(pixels));
    let expected_provenance = Provenance::Automatic {
        when: input.captured_at_utc.clone(),
        source: input.context.target,
    };
    if !envelope.is_recovery_ready()
        || envelope.schema_version() != 2
        || stage.library_id() != input.library_id
        || stage.principal_id() != input.principal_id
        || stage.operation_id().as_uuid() != input.request.operation_id()
        || stage.mode() != input.request.target()
        || stage.record_id() != input.record_id
        || stage.asset_id() != input.asset_id
        || stage.frame_id() != input.frame_id
        || stage.width() != input.width
        || stage.height() != input.height
        || stage.byte_length() != pixels.len() as u64
        || stage.pixels_sha256() != input.pixel_sha256
        || stage.pixels_sha256() != expected_pixel_sha
        || stage.context() != &input.context
        || stage.context_sha256() != expected_context_sha
        || envelope.warnings() != Some(input.warnings.as_slice())
        || stage.captured_at() != input.captured_at_utc
        || stage.encoder_fingerprint() != input.encoder_fingerprint
        || envelope.automatic_provenance() != Some(expected_provenance)
        || envelope.path_policy() != Some(RECOVERY_PATH_POLICY)
    {
        return Err(MatchRecoveryError::Mismatch);
    }
    Ok(())
}

#[cfg(test)]
#[path = "match_recovery_tests.rs"]
mod tests;
