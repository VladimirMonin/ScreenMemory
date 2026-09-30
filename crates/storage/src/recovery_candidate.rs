//! Sealed, read-only recovery encoding boundary. Not a canonical publication or DB commit.
use crate::{match_recovery, staging_files::ReopenedV2};
use sha2::{Digest, Sha256};
use sm_codec::{encode_exact_lossless_verified, encoder_fingerprint, CodecError, RgbaImage};
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId, RecordId};
use sm_isolation::{CanonicalError, CanonicalPlan, CanonicalRead, CanonicalSeal, HostLease};
use std::io::Write;

/// Temporary conservative pre-encode guard; this is NOT the normal 96 MP capture limit.
pub const MAX_RECOVERY_CODEC_PIXELS: u64 = 1_048_576;
pub const MAX_RECOVERY_ENCODED_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug)]
pub enum PrepareCandidateError {
    Unavailable,
    Mismatch(match_recovery::MatchRecoveryError),
    EncoderProfileMismatch,
    PixelCapExceeded,
    EncodedTooLarge,
    Codec(CodecError),
}

/// Non-Clone witness and exact-lossless, decode-verified encoded bytes. A DB snapshot,
/// not a reservation, file path, published file or Saved receipt. No public constructor.
pub struct VerifiedCandidate {
    witness: Box<match_recovery::DbValidatedIntent>,
    encoded: Vec<u8>,
    encoded_sha256: [u8; 32],
    length: u64,
    width: u32,
    height: u32,
    encoder_fingerprint: String,
}

impl std::fmt::Debug for VerifiedCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedCandidate")
            .field("length", &self.length)
            .field("geometry", &self.geometry())
            .finish_non_exhaustive()
    }
}

impl VerifiedCandidate {
    pub fn encoded_bytes(&self) -> &[u8] {
        &self.encoded
    }
    pub fn encoded_sha256(&self) -> &[u8; 32] {
        &self.encoded_sha256
    }
    pub const fn length(&self) -> u64 {
        self.length
    }
    pub const fn geometry(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub fn encoder_fingerprint(&self) -> &str {
        &self.encoder_fingerprint
    }
    pub fn trusted_ids(&self) -> (LibraryId, PrincipalId, OperationId, RecordId, AssetId) {
        let input = &self.witness.trusted().input;
        (
            input.library_id,
            input.principal_id,
            OperationId::new(input.request.operation_id()),
            input.record_id,
            input.asset_id,
        )
    }
}

/// A file-lane result, not Saved or a DB reservation. Retains the sealed pending
/// witness, candidate bytes, and a pinned no-follow handle. SHARE_WRITE means the
/// handle does not freeze contents: a later SQL transaction MUST revalidate its
/// still-pending row and call `revalidate` just before any Saved insertion.
pub struct VerifiedPublished {
    candidate: VerifiedCandidate,
    final_handle: CanonicalRead,
    newly_published: bool,
}

impl std::fmt::Debug for VerifiedPublished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedPublished")
            .field("newly_published", &self.newly_published)
            .finish_non_exhaustive()
    }
}

impl VerifiedPublished {
    pub fn newly_published(&self) -> bool {
        self.newly_published
    }

    /// Recheck the pinned file, not a name re-opened by an untrusted caller. This
    /// must run on the file lane, never on the SQLite worker. It is not a lock on
    /// the pending row and cannot alone issue a Saved receipt.
    pub(crate) fn revalidate(&mut self) -> Result<(), PublishCandidateError> {
        compare_bytes(
            &self.candidate,
            self.final_handle.len(),
            self.final_handle.read_all_borrowed(),
        )
    }

    /// One-shot file-lane transition; even failure consumes the old proof.
    pub(crate) fn seal(self) -> Result<SealedPublished, PublishCandidateError> {
        let final_handle = self
            .final_handle
            .seal()
            .map_err(PublishCandidateError::Canonical)?;
        let mut sealed = SealedPublished {
            candidate: self.candidate,
            final_handle,
            newly_published: self.newly_published,
        };
        sealed.revalidate()?;
        Ok(sealed)
    }
}

/// Non-Clone pending witness and fixed write-denying HANDLE. No DB write or
/// Saved authority; an existing mapped view may still mutate these bytes.
pub struct SealedPublished {
    candidate: VerifiedCandidate,
    final_handle: CanonicalSeal,
    newly_published: bool,
}
impl std::fmt::Debug for SealedPublished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedPublished")
            .field("newly_published", &self.newly_published)
            .finish_non_exhaustive()
    }
}
impl SealedPublished {
    pub fn newly_published(&self) -> bool {
        self.newly_published
    }
    pub(crate) fn revalidate(&mut self) -> Result<(), PublishCandidateError> {
        compare_bytes(
            &self.candidate,
            self.final_handle.len(),
            self.final_handle.read_all_borrowed(),
        )
    }
}

fn compare_bytes(
    candidate: &VerifiedCandidate,
    len: u64,
    read: std::io::Result<Vec<u8>>,
) -> Result<(), PublishCandidateError> {
    let bytes = read.map_err(|_| PublishCandidateError::IntegrityFailed)?;
    if len != candidate.length
        || bytes.len() as u64 != candidate.length
        || Sha256::digest(&bytes).as_slice() != candidate.encoded_sha256.as_slice()
        || bytes != candidate.encoded
    {
        return Err(PublishCandidateError::IntegrityFailed);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishCandidateError {
    Unavailable,
    Canonical(CanonicalError),
    WriteFailed,
    IntegrityFailed,
}

/// Derive the only canonical path from the sealed SQLite witness, never from
/// stage input or retry wall clock. No cleanup, overwrite, adoption by name or
/// database write is permitted. A failed attempt temp remains on disk.
pub(crate) fn publish(
    lease: &HostLease,
    candidate: VerifiedCandidate,
) -> Result<VerifiedPublished, PublishCandidateError> {
    let trusted = candidate.witness.trusted();
    let plan = CanonicalPlan::new(trusted.input.record_id, &trusted.planned_created_at_utc)
        .map_err(PublishCandidateError::Canonical)?;
    let root = lease
        .canonical_root()
        .map_err(PublishCandidateError::Canonical)?;
    let newly_published = match root.begin(&plan) {
        Ok(mut temp) => {
            temp.write_all(&candidate.encoded)
                .map_err(|_| PublishCandidateError::WriteFailed)?;
            temp.sync_all()
                .map_err(|_| PublishCandidateError::WriteFailed)?;
            match temp.publish() {
                Ok(()) => true,
                Err(CanonicalError::Existing) => false,
                Err(e) => return Err(PublishCandidateError::Canonical(e)),
            }
        }
        Err(CanonicalError::Existing) => false,
        Err(e) => return Err(PublishCandidateError::Canonical(e)),
    };
    let final_handle = root
        .open_final(&plan)
        .map_err(PublishCandidateError::Canonical)?;
    let mut published = VerifiedPublished {
        candidate,
        final_handle,
        newly_published,
    };
    published.revalidate()?;
    Ok(published)
}

/// Execute only in the existing single-slot file lane after admission. The match ties
/// the sealed SQLite witness to the reopened stage before its pixel Vec is moved into
/// the codec. Native libwebp scratch is not bounded by this Rust guard: 1 GiB RSS
/// remains NOT_PROVEN even with the conservative pixel and encoded limits.
pub(crate) fn prepare(
    witness: Box<match_recovery::DbValidatedIntent>,
    reopened: ReopenedV2,
) -> Result<VerifiedCandidate, PrepareCandidateError> {
    match_recovery::match_pending_reopened_v2(&witness, &reopened)
        .map_err(PrepareCandidateError::Mismatch)?;
    let input = &witness.trusted().input;
    let current = encoder_fingerprint();
    if input.encoder_fingerprint != current {
        return Err(PrepareCandidateError::EncoderProfileMismatch);
    }
    let pixels = u64::from(input.width)
        .checked_mul(u64::from(input.height))
        .ok_or(PrepareCandidateError::PixelCapExceeded)?;
    if pixels == 0 || pixels > MAX_RECOVERY_CODEC_PIXELS {
        return Err(PrepareCandidateError::PixelCapExceeded);
    }
    let (width, height) = (input.width, input.height);
    let rgba = RgbaImage::from_rgba(width, height, reopened.into_pixels())
        .map_err(PrepareCandidateError::Codec)?;
    let verified = encode_exact_lossless_verified(&rgba).map_err(PrepareCandidateError::Codec)?;
    if verified.canonical().encoder_fingerprint() != current {
        return Err(PrepareCandidateError::EncoderProfileMismatch);
    }
    let encoded = verified.into_canonical().into_bytes();
    if encoded.is_empty() || encoded.len() > MAX_RECOVERY_ENCODED_BYTES {
        return Err(PrepareCandidateError::EncodedTooLarge);
    }
    let encoded_sha256: [u8; 32] = Sha256::digest(&encoded).into();
    let length = encoded.len() as u64;
    Ok(VerifiedCandidate {
        witness,
        encoded,
        encoded_sha256,
        length,
        width,
        height,
        encoder_fingerprint: current,
    })
}
