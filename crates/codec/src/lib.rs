//! S02 canonical image codec: WebP exact lossless, bundled and statically linked.
//!
//! This crate owns no files, no paths, no SQL and no logging. It converts between
//! normalized in-memory pixels and a verified canonical WebP container, and it
//! reports an honest capability state per format so an unimplemented AVIF path can
//! never be presented as a working encoder.
//!
//! Invariants proven by the crate tests:
//! * `exact = 1` lossless round-trips every byte of a normalized RGBA buffer,
//!   including the RGB values of fully transparent pixels;
//! * geometry is bounded before the native call (WebP side limit and pixel budget);
//! * empty, truncated or malformed input produces a typed error, never a panic and
//!   never an arbitrary decode;
//! * a [`VerifiedCanonicalWebp`] can only be produced after a bounded decode-check
//!   reproduced the accepted normalized pixels byte for byte.

mod error;
mod limits;
mod native;

pub use error::{CodecCapability, CodecError, CodecFormat};
pub use limits::{CodecLimits, Dimensions, DEFAULT_CODEC_LIMITS};

use std::fmt;

/// Encoder profile recorded with every canonical asset.
pub const WEBP_EXACT_LOSSLESS_PROFILE: &str = "webp-lossless-v1";
/// Container MIME for the canonical WebP asset.
pub const WEBP_MIME: &str = "image/webp";

/// Tight row-major input layouts accepted by the encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelLayout {
    /// 4 bytes per pixel, `R G B A`.
    Rgba8,
    /// 3 bytes per pixel, `B G R`; alpha is opaque.
    Bgr8,
}

impl PixelLayout {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Bgr8 => 3,
        }
    }
}

/// Normalized, owned pixels: tight RGBA8, `4 * width * height` bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    dimensions: Dimensions,
    pixels: Vec<u8>,
}

impl RgbaImage {
    /// Adopt an already normalized RGBA buffer after bounds and length checks.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, CodecError> {
        let dimensions = DEFAULT_CODEC_LIMITS.check(width, height)?;
        let expected = dimensions.byte_len(PixelLayout::Rgba8.bytes_per_pixel())?;
        if pixels.len() != expected {
            return Err(CodecError::BufferLengthMismatch {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self { dimensions, pixels })
    }

    /// Normalize any accepted layout into RGBA8. BGR becomes opaque RGBA.
    pub fn from_layout(
        layout: PixelLayout,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Result<Self, CodecError> {
        let dimensions = DEFAULT_CODEC_LIMITS.check(width, height)?;
        let expected = dimensions.byte_len(layout.bytes_per_pixel())?;
        if data.len() != expected {
            return Err(CodecError::BufferLengthMismatch {
                expected,
                actual: data.len(),
            });
        }
        let pixels = match layout {
            PixelLayout::Rgba8 => data.to_vec(),
            PixelLayout::Bgr8 => {
                let mut pixels = Vec::with_capacity(dimensions.byte_len(4)?);
                for pixel in data.as_chunks::<3>().0 {
                    pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0xff]);
                }
                pixels
            }
        };
        Ok(Self { dimensions, pixels })
    }

    /// Normalize 2-byte `gray, alpha` pixels into RGBA with `R = G = B = gray`.
    pub fn from_gray_alpha(width: u32, height: u32, data: &[u8]) -> Result<Self, CodecError> {
        let dimensions = DEFAULT_CODEC_LIMITS.check(width, height)?;
        let expected = dimensions.byte_len(2)?;
        if data.len() != expected {
            return Err(CodecError::BufferLengthMismatch {
                expected,
                actual: data.len(),
            });
        }
        let mut pixels = Vec::with_capacity(dimensions.byte_len(4)?);
        for pixel in data.as_chunks::<2>().0 {
            pixels.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
        }
        Ok(Self { dimensions, pixels })
    }

    pub fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// Pixel content stays out of diagnostics: only geometry and length are printable.
impl fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RgbaImage")
            .field("dimensions", &self.dimensions)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

/// An encoded canonical container that has not yet been decode-checked.
#[derive(Clone, PartialEq, Eq)]
pub struct CanonicalWebp {
    bytes: Vec<u8>,
    dimensions: Dimensions,
    encoder_fingerprint: String,
}

impl CanonicalWebp {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn dimensions(&self) -> Dimensions {
        self.dimensions
    }
    pub fn profile(&self) -> &'static str {
        WEBP_EXACT_LOSSLESS_PROFILE
    }
    pub fn mime(&self) -> &'static str {
        WEBP_MIME
    }
    /// Encoder provenance for DB evidence. Never emit this into an ordinary log event.
    pub fn encoder_fingerprint(&self) -> &str {
        &self.encoder_fingerprint
    }
}

/// Only geometry and encoded length are printable; the fingerprint carries a native
/// revision and stays out of `Debug` output on purpose.
impl fmt::Debug for CanonicalWebp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CanonicalWebp")
            .field("profile", &WEBP_EXACT_LOSSLESS_PROFILE)
            .field("dimensions", &self.dimensions)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// A canonical container whose bounded decode-check reproduced the accepted pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCanonicalWebp(CanonicalWebp);

impl VerifiedCanonicalWebp {
    pub fn canonical(&self) -> &CanonicalWebp {
        &self.0
    }
    pub fn into_canonical(self) -> CanonicalWebp {
        self.0
    }
}

/// Encoder provenance of the bundled build, including its native revision.
///
/// This belongs in the asset profile fingerprint; it must never be written to an
/// ordinary log event, so callers use it only for stored evidence.
pub fn encoder_fingerprint() -> String {
    let (major, minor, revision) = native::encoder_version();
    format!(
        "{WEBP_EXACT_LOSSLESS_PROFILE};libwebp={major}.{minor}.{revision};exact=1;lossless=1;method=4;quality=100;thread_level=0"
    )
}

/// Capability of a canonical format in this stage.
pub fn capability(format: CodecFormat) -> CodecCapability {
    format.capability()
}

/// Exact lossless encode. The result is not yet decode-checked.
pub fn encode_exact_lossless(image: &RgbaImage) -> Result<CanonicalWebp, CodecError> {
    let bytes = native::encode_lossless_exact(PixelLayout::Rgba8, image.dimensions, &image.pixels)?;
    Ok(CanonicalWebp {
        bytes,
        dimensions: image.dimensions,
        encoder_fingerprint: encoder_fingerprint(),
    })
}

/// Exact lossless encode followed by the mandatory bounded decode-check.
pub fn encode_exact_lossless_verified(
    image: &RgbaImage,
) -> Result<VerifiedCanonicalWebp, CodecError> {
    let candidate = encode_exact_lossless(image)?;
    verify_exact_lossless(candidate, image)
}

/// Encode a raw buffer of the given layout and verify it against the normalized input.
pub fn encode_pixels_verified(
    layout: PixelLayout,
    width: u32,
    height: u32,
    data: &[u8],
) -> Result<VerifiedCanonicalWebp, CodecError> {
    let normalized = RgbaImage::from_layout(layout, width, height, data)?;
    let bytes = native::encode_lossless_exact(layout, normalized.dimensions(), data)?;
    let candidate = CanonicalWebp {
        bytes,
        dimensions: normalized.dimensions(),
        encoder_fingerprint: encoder_fingerprint(),
    };
    verify_exact_lossless(candidate, &normalized)
}

/// Bounded decode-check: decoded normalized pixels must equal the accepted pixels.
pub fn verify_exact_lossless(
    candidate: CanonicalWebp,
    expected: &RgbaImage,
) -> Result<VerifiedCanonicalWebp, CodecError> {
    let (dimensions, pixels) = native::decode_rgba(&candidate.bytes, DEFAULT_CODEC_LIMITS)?;
    if dimensions != expected.dimensions {
        return Err(CodecError::RoundTripMismatch { offset: 0 });
    }
    if let Some(offset) = first_difference(&pixels, &expected.pixels) {
        return Err(CodecError::RoundTripMismatch { offset });
    }
    Ok(VerifiedCanonicalWebp(candidate))
}

fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
    if left.len() != right.len() {
        return Some(left.len().min(right.len()));
    }
    left.iter().zip(right).position(|(a, b)| a != b)
}

/// Header-only inspection with full bounds checking.
pub fn inspect(bytes: &[u8]) -> Result<Dimensions, CodecError> {
    native::inspect(bytes, DEFAULT_CODEC_LIMITS)
}

/// Decode into normalized RGBA with the default ceilings.
pub fn decode_rgba(bytes: &[u8]) -> Result<RgbaImage, CodecError> {
    decode_rgba_with_limits(bytes, DEFAULT_CODEC_LIMITS)
}

/// Decode into normalized RGBA with explicit ceilings.
///
/// Explicit ceilings may only *tighten* the shared product budget: the returned
/// image is additionally validated against [`DEFAULT_CODEC_LIMITS`], so a caller
/// cannot raise the decoded-image ceiling by passing wider values.
pub fn decode_rgba_with_limits(bytes: &[u8], limits: CodecLimits) -> Result<RgbaImage, CodecError> {
    let (dimensions, pixels) = native::decode_rgba(bytes, limits)?;
    RgbaImage::from_rgba(dimensions.width, dimensions.height, pixels)
}

/// Canonical encode entry point for a negotiated format; unimplemented formats fail closed.
///
/// Only the exact-lossless WebP profile exists in S02, so an AVIF request returns a
/// typed `NotImplemented` error rather than a silently substituted WebP container.
pub fn encode_canonical(
    format: CodecFormat,
    image: &RgbaImage,
) -> Result<CanonicalWebp, CodecError> {
    match format {
        CodecFormat::Webp => encode_exact_lossless(image),
        CodecFormat::Avif => Err(CodecError::NotImplemented { format }),
    }
}

/// Canonical decode entry point for a negotiated format; unimplemented formats fail closed.
pub fn decode_canonical(format: CodecFormat, bytes: &[u8]) -> Result<RgbaImage, CodecError> {
    match format {
        CodecFormat::Webp => decode_rgba(bytes),
        CodecFormat::Avif => Err(CodecError::NotImplemented { format }),
    }
}
