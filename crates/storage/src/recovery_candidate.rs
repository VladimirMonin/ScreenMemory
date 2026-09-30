//! Sealed, read-only recovery encoding boundary. Not a canonical publication or DB commit.
use crate::{match_recovery, staging_files::ReopenedV2};
use sha2::{Digest, Sha256};
use sm_codec::{encode_exact_lossless_verified, encoder_fingerprint, CodecError, RgbaImage};
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId, RecordId};

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
