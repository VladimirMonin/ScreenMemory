//! Audited, private wrapper over the bundled libwebp build.
//!
//! Every native buffer crossing this boundary is owned by an RAII guard, every
//! size argument is validated before the call, and nothing here performs I/O,
//! logging or path access. Native revision strings exist only in the caller's
//! encoder fingerprint, never in ordinary log events.
use crate::{
    error::CodecError,
    limits::{CodecLimits, Dimensions},
    PixelLayout,
};
use libwebp_sys::*;
use std::{ffi::c_void, mem::MaybeUninit, slice};

/// Smallest complete WebP container header (RIFF tag, size, WEBP tag).
const WEBP_CONTAINER_MIN: usize = 12;
const RIFF_MAGIC: &[u8; 4] = b"RIFF";

/// Bytes per pixel of a normalized RGBA buffer.
const RGBA_BYTES: usize = 4;

/// Bundled encoder revision as `(major, minor, revision)`; diagnostics only.
pub(crate) fn encoder_version() -> (u32, u32, u32) {
    // SAFETY: no arguments, no allocation; the bundled build always answers.
    let raw = unsafe { WebPGetEncoderVersion() } as u32;
    ((raw >> 16) & 0xff, (raw >> 8) & 0xff, raw & 0xff)
}

fn unsupported(dimensions: Dimensions) -> CodecError {
    CodecError::CanonicalSizeUnsupported {
        width: dimensions.width,
        height: dimensions.height,
    }
}

/// A native picture that is always released exactly once.
struct PictureGuard(WebPPicture);
impl Drop for PictureGuard {
    fn drop(&mut self) {
        // SAFETY: the guard owns the initialized picture exactly once.
        unsafe { WebPPictureFree(&mut self.0) };
    }
}

/// A native memory writer whose heap buffer is always released exactly once.
struct WriterGuard(Box<WebPMemoryWriter>);
impl Drop for WriterGuard {
    fn drop(&mut self) {
        // SAFETY: the guard owns the initialized writer exactly once.
        unsafe { WebPMemoryWriterClear(&mut *self.0) };
    }
}

/// A decoded native buffer released by the owning decoder allocator.
struct DecodeGuard(*mut u8);
impl Drop for DecodeGuard {
    fn drop(&mut self) {
        // SAFETY: the pointer came from WebPDecodeRGBA and is freed exactly once.
        unsafe { WebPFree(self.0 as *mut c_void) };
    }
}

fn lossless_exact_config() -> Result<WebPConfig, CodecError> {
    let mut config = MaybeUninit::<WebPConfig>::uninit();
    // SAFETY: writable uninitialized storage plus the bundled encoder ABI version.
    let initialized = unsafe {
        WebPConfigInitInternal(
            config.as_mut_ptr(),
            WebPPreset::WEBP_PRESET_DEFAULT,
            75.0,
            WEBP_ENCODER_ABI_VERSION as i32,
        )
    };
    if initialized == 0 {
        return Err(CodecError::EncodeFailed { native_code: 0 });
    }
    let mut config = unsafe { config.assume_init() };
    config.lossless = 1;
    config.quality = 100.0;
    config.method = 4;
    // exact=1 keeps the RGB of fully transparent pixels through a lossless round-trip.
    config.exact = 1;
    // Single-threaded encoding keeps a pinned image byte-reproducible and bounded.
    config.thread_level = 0;
    // SAFETY: the configuration pointer is valid for the duration of the call.
    if unsafe { WebPValidateConfig(&config) } == 0 {
        return Err(CodecError::EncodeFailed { native_code: 0 });
    }
    Ok(config)
}

/// Exact lossless encode of a tight row-major buffer into a WebP container.
pub(crate) fn encode_lossless_exact(
    layout: PixelLayout,
    dimensions: Dimensions,
    data: &[u8],
) -> Result<Vec<u8>, CodecError> {
    encode_with_config(layout, dimensions, data, lossless_exact_config()?)
}

/// S03 export-only profile. Never used by capture/recovery canonical callers.
pub(crate) fn encode_lossy90(dimensions: Dimensions, data: &[u8]) -> Result<Vec<u8>, CodecError> {
    let mut config = lossless_exact_config()?;
    config.lossless = 0;
    config.quality = 90.0;
    config.exact = 0;
    config.alpha_quality = 100;
    if unsafe { WebPValidateConfig(&config) } == 0 {
        return Err(CodecError::EncodeFailed { native_code: 0 });
    }
    encode_with_config(PixelLayout::Rgba8, dimensions, data, config)
}

fn encode_with_config(
    layout: PixelLayout,
    dimensions: Dimensions,
    data: &[u8],
    config: WebPConfig,
) -> Result<Vec<u8>, CodecError> {
    let expected = dimensions.byte_len(layout.bytes_per_pixel())?;
    if data.len() != expected {
        return Err(CodecError::BufferLengthMismatch {
            expected,
            actual: data.len(),
        });
    }
    let width = i32::try_from(dimensions.width).map_err(|_| unsupported(dimensions))?;
    let height = i32::try_from(dimensions.height).map_err(|_| unsupported(dimensions))?;
    let stride = i32::try_from(expected / dimensions.height as usize)
        .map_err(|_| unsupported(dimensions))?;
    let mut picture = MaybeUninit::<WebPPicture>::uninit();
    // SAFETY: writable uninitialized storage plus the bundled encoder ABI version.
    let initialized =
        unsafe { WebPPictureInitInternal(picture.as_mut_ptr(), WEBP_ENCODER_ABI_VERSION as i32) };
    if initialized == 0 {
        return Err(CodecError::EncodeFailed { native_code: 0 });
    }
    let mut picture = unsafe { picture.assume_init() };
    picture.use_argb = 1;
    picture.width = width;
    picture.height = height;

    // SAFETY: picture is initialized, data is a tight buffer of width*height*bpp.
    let imported = unsafe {
        match layout {
            PixelLayout::Rgba8 => WebPPictureImportRGBA(&mut picture, data.as_ptr(), stride),
            PixelLayout::Bgr8 => WebPPictureImportBGR(&mut picture, data.as_ptr(), stride),
        }
    };
    if imported == 0 {
        let native_code = picture.error_code as u32;
        // SAFETY: the picture is initialized and not yet owned by a guard.
        unsafe { WebPPictureFree(&mut picture) };
        return Err(CodecError::EncodeFailed { native_code });
    }

    let mut picture = PictureGuard(picture);
    let mut writer = Box::new(MaybeUninit::<WebPMemoryWriter>::uninit());
    // SAFETY: the boxed writer is writable and uninitialized.
    unsafe { WebPMemoryWriterInit(writer.as_mut_ptr()) };
    let mut writer =
        WriterGuard(unsafe { Box::<MaybeUninit<WebPMemoryWriter>>::assume_init(writer) });
    picture.0.writer = Some(WebPMemoryWrite);
    // SAFETY: the boxed writer address is stable for the whole encode call.
    picture.0.custom_ptr = (&mut *writer.0) as *mut WebPMemoryWriter as *mut c_void;

    // SAFETY: config is validated and picture owns imported pixels plus a writer.
    let encoded = unsafe { WebPEncode(&config, &mut picture.0) };
    let (memory, size) = (writer.0.mem, writer.0.size);
    if encoded == 0 || memory.is_null() || size == 0 {
        return Err(CodecError::EncodeFailed {
            native_code: picture.0.error_code as u32,
        });
    }
    // SAFETY: the native writer reported exactly this allocation and length.
    Ok(unsafe { slice::from_raw_parts(memory, size) }.to_vec())
}

/// `true` when the declared RIFF payload length exceeds the bytes actually given.
fn riff_is_truncated(data: &[u8]) -> bool {
    if data.len() < WEBP_CONTAINER_MIN || &data[..4] != RIFF_MAGIC {
        return false;
    }
    let declared = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    declared.saturating_add(8) > data.len()
}

/// Header-only inspection with the same guards as a full decode.
pub(crate) fn inspect(data: &[u8], limits: CodecLimits) -> Result<Dimensions, CodecError> {
    if data.is_empty() {
        return Err(CodecError::EmptyInput);
    }
    if data.len() < WEBP_CONTAINER_MIN {
        return Err(CodecError::TruncatedInput);
    }
    let mut features = MaybeUninit::<WebPBitstreamFeatures>::uninit();
    // SAFETY: features points to writable storage and data is a complete header.
    let status = unsafe { WebPGetFeatures(data.as_ptr(), data.len(), features.as_mut_ptr()) };
    match status {
        VP8StatusCode::VP8_STATUS_OK => {}
        VP8StatusCode::VP8_STATUS_NOT_ENOUGH_DATA => return Err(CodecError::TruncatedInput),
        VP8StatusCode::VP8_STATUS_BITSTREAM_ERROR
        | VP8StatusCode::VP8_STATUS_UNSUPPORTED_FEATURE
        | VP8StatusCode::VP8_STATUS_INVALID_PARAM => return Err(CodecError::MalformedInput),
        other => {
            return Err(CodecError::DecodeFailed {
                native_code: other as u32,
            })
        }
    }
    let features = unsafe { features.assume_init() };
    if features.width <= 0 || features.height <= 0 {
        return Err(CodecError::MalformedInput);
    }
    limits.check(features.width as u32, features.height as u32)
}

/// Bounded decode into normalized RGBA. Never panics on damaged input.
pub(crate) fn decode_rgba(
    data: &[u8],
    limits: CodecLimits,
) -> Result<(Dimensions, Vec<u8>), CodecError> {
    let dimensions = inspect(data, limits)?;
    let mut width = 0i32;
    let mut height = 0i32;
    // SAFETY: data and both out-parameters are valid for the call.
    let pointer = unsafe { WebPDecodeRGBA(data.as_ptr(), data.len(), &mut width, &mut height) };
    if pointer.is_null() {
        return Err(if riff_is_truncated(data) {
            CodecError::TruncatedInput
        } else {
            CodecError::MalformedInput
        });
    }
    let guard = DecodeGuard(pointer);
    if width <= 0
        || height <= 0
        || width as u32 != dimensions.width
        || height as u32 != dimensions.height
    {
        return Err(CodecError::MalformedInput);
    }
    let expected = dimensions.byte_len(RGBA_BYTES)?;
    // SAFETY: the decoder returned a tight width*height*4 buffer for this bitstream.
    let pixels = unsafe { slice::from_raw_parts(guard.0, expected) }.to_vec();
    Ok((dimensions, pixels))
}
