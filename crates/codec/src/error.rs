//! Typed codec failures with stable codes. No native version, path or pixel payload
//! is part of an error, so an error can be recorded without leaking library bytes.
use std::fmt;

/// Canonical image formats the product must eventually support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecFormat {
    Webp,
    Avif,
}

/// Honest capability state: a declared format is never reported as working.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecCapability {
    Implemented,
    NotImplemented,
}

impl CodecFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Avif => "avif",
        }
    }
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
        }
    }
    /// S02 owns WebP only; AVIF encode/decode belongs to S03 and is not substituted.
    pub const fn capability(self) -> CodecCapability {
        match self {
            Self::Webp => CodecCapability::Implemented,
            Self::Avif => CodecCapability::NotImplemented,
        }
    }
}

/// Rejected input or native codec failure. Every variant carries only safe values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// The requested format has no implementation in this stage (AVIF).
    NotImplemented { format: CodecFormat },
    /// Zero side, or a side beyond the WebP 16383 dimension limit.
    CanonicalSizeUnsupported { width: u32, height: u32 },
    /// Declared pixel count exceeds the configured decoded-image budget.
    ImageLimitExceeded { pixels: u64, budget: u64 },
    /// Buffer length does not match width * height * bytes-per-pixel.
    BufferLengthMismatch { expected: usize, actual: usize },
    /// Nothing can be parsed and no native call is made for an empty buffer.
    EmptyInput,
    /// Input is shorter than a complete WebP container header.
    TruncatedInput,
    /// A complete-looking container was refused by the decoder.
    MalformedInput,
    /// Native encoder refused the configuration or the picture.
    EncodeFailed { native_code: u32 },
    /// Native decoder refused the bitstream after the container header was accepted.
    DecodeFailed { native_code: u32 },
    /// The bounded decode-check did not reproduce the accepted normalized pixels.
    RoundTripMismatch { offset: usize },
}

impl CodecError {
    /// Stable machine code. S02 §5.6 names are reused instead of inventing a second vocabulary.
    pub const fn stable_code(self) -> &'static str {
        match self {
            Self::NotImplemented { .. } => "NOT_IMPLEMENTED",
            Self::CanonicalSizeUnsupported { .. } => "CANONICAL_SIZE_UNSUPPORTED",
            Self::ImageLimitExceeded { .. } => "IMAGE_LIMIT_EXCEEDED",
            Self::BufferLengthMismatch { .. }
            | Self::EmptyInput
            | Self::TruncatedInput
            | Self::MalformedInput => "IMAGE_INPUT_INVALID",
            Self::EncodeFailed { .. } | Self::DecodeFailed { .. } => "CODEC_FAILED",
            Self::RoundTripMismatch { .. } => "ASSET_INTEGRITY_FAILED",
        }
    }
}

/// Fixed safe text; deliberately free of native revisions, paths and image content.
impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::NotImplemented { format } => {
                return write!(
                    f,
                    "format {} is not implemented in this stage",
                    format.as_str()
                )
            }
            Self::CanonicalSizeUnsupported { .. } => {
                "image dimensions are outside the supported range"
            }
            Self::ImageLimitExceeded { .. } => "image exceeds the decoded pixel budget",
            Self::BufferLengthMismatch { .. } => "pixel buffer length does not match dimensions",
            Self::EmptyInput => "image input is empty",
            Self::TruncatedInput => "image input is truncated",
            Self::MalformedInput => "image input is not a valid bitstream",
            Self::EncodeFailed { .. } => "the encoder refused the picture",
            Self::DecodeFailed { .. } => "the decoder refused the bitstream",
            Self::RoundTripMismatch { .. } => "decoded pixels differ from the accepted pixels",
        };
        f.write_str(text)
    }
}

impl std::error::Error for CodecError {}
