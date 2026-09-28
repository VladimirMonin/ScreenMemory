//! Bounded geometry accepted by the canonical encoder and by the decode-check.
use crate::error::CodecError;

/// Decoded-image ceilings. Guards run before any native call, so an oversized
/// input never reaches the encoder/decoder and never allocates a native buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodecLimits {
    /// WebP format limit per side.
    pub max_side: u32,
    /// Global decoded-pixel budget for one canonical image.
    pub max_pixels: u64,
}

/// WebP's documented per-side limit plus the shared 96 MP decoded-image budget.
pub const DEFAULT_CODEC_LIMITS: CodecLimits = CodecLimits {
    max_side: 16_383,
    max_pixels: 96_000_000,
};

impl CodecLimits {
    /// Reject zero sides and every dimension beyond the configured ceilings.
    pub fn check(self, width: u32, height: u32) -> Result<Dimensions, CodecError> {
        if width == 0 || height == 0 || width > self.max_side || height > self.max_side {
            return Err(CodecError::CanonicalSizeUnsupported { width, height });
        }
        let pixels = u64::from(width) * u64::from(height);
        if pixels > self.max_pixels {
            return Err(CodecError::ImageLimitExceeded {
                pixels,
                budget: self.max_pixels,
            });
        }
        Ok(Dimensions { width, height })
    }
}

/// Validated non-zero dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

impl Dimensions {
    /// Checked pixel count; the caller already bounded the value.
    pub const fn pixel_count(self) -> u64 {
        self.width as u64 * self.height as u64
    }
    /// Byte length of a tight row-major buffer for this layout width.
    pub fn byte_len(self, bytes_per_pixel: usize) -> Result<usize, CodecError> {
        let expected = (self.pixel_count() as usize)
            .checked_mul(bytes_per_pixel)
            .ok_or(CodecError::ImageLimitExceeded {
                pixels: self.pixel_count(),
                budget: DEFAULT_CODEC_LIMITS.max_pixels,
            })?;
        Ok(expected)
    }
}
