//! Private image engine. Untrusted bitstreams MUST execute in sm-image-worker.
//! No paths, publication authority, database, network, or model runtime.
mod avif;
mod inspect;
pub mod metadata;
pub mod transform;

use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Cursor;

pub const MAX_INPUT_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 96_000_000;
pub const MAX_WORK_BYTES: u64 = 768 * 1024 * 1024;
pub const ABI: &str = "sm-image-worker-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageError {
    Malformed,
    UnsupportedFormat,
    MultiframeNotSupported,
    RawNotSupported,
    ColorConversionRequired,
    GeometryConversionRequired,
    OrientationConflict,
    LimitExceeded,
    InvalidOptions,
    BackgroundRequired,
    CodecFailed,
    VerificationFailed,
    MetadataRejected,
}
impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ImageError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}
impl Dimensions {
    pub fn byte_len(self) -> Result<usize, ImageError> {
        let n = u64::from(self.width) * u64::from(self.height);
        if self.width == 0
            || self.height == 0
            || self.width > 32768
            || self.height > 32768
            || n > MAX_PIXELS
        {
            return Err(ImageError::LimitExceeded);
        }
        usize::try_from(n * 4).map_err(|_| ImageError::LimitExceeded)
    }
    pub fn working_budget(self, bytes_per_pixel: u64) -> Result<(), ImageError> {
        self.byte_len()?;
        if u64::from(self.width) * u64::from(self.height) * bytes_per_pixel > MAX_WORK_BYTES {
            return Err(ImageError::LimitExceeded);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Avif,
    Bmp,
    Gif,
    Tiff,
}
impl Format {
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            Self::Bmp => "image/bmp",
            Self::Gif => "image/gif",
            Self::Tiff => "image/tiff",
        }
    }
    fn image_format(self) -> Result<ImageFormat, ImageError> {
        match self {
            Self::Png => Ok(ImageFormat::Png),
            Self::Jpeg => Ok(ImageFormat::Jpeg),
            Self::Bmp => Ok(ImageFormat::Bmp),
            Self::Gif => Ok(ImageFormat::Gif),
            Self::Tiff => Ok(ImageFormat::Tiff),
            _ => Err(ImageError::UnsupportedFormat),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputProfile {
    WebpExact,
    WebpQuality90,
    AvifQuality90,
    Png,
    JpegQuality90,
}
impl OutputProfile {
    pub const fn format(self) -> Format {
        match self {
            Self::WebpExact | Self::WebpQuality90 => Format::Webp,
            Self::AvifQuality90 => Format::Avif,
            Self::Png => Format::Png,
            Self::JpegQuality90 => Format::Jpeg,
        }
    }
    pub fn fingerprint(self) -> String {
        match self {
        Self::WebpExact=>sm_codec::encoder_fingerprint(),
        Self::WebpQuality90=>{let exact=sm_codec::encoder_fingerprint();let native=exact.split(';').nth(1).unwrap_or("libwebp=unknown");format!("webp-export-quality90-v1;{native};lossless=0;quality=90;alpha_quality=100;exact=0;method=4;thread_level=0")},
        Self::AvifQuality90=>"avif-quality-v1;libavif=1.0.4;libaom=3.11.0;cpu=generic;depth=8;444;quality=90;alpha=100;speed=6;threads=2;cp=1;tc=13;mc=1;full".into(),
        Self::Png=>"png-export-v1;image=0.25.10;rgba8;strip-source".into(),
        Self::JpegQuality90=>"jpeg-export-v1;image=0.25.10;quality=90;444;explicit-background;strip-source".into(),
    }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    NormalizeCanonical,
    DeriveExport,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub method: Operation,
    pub profile: OutputProfile,
    pub resize: transform::ResizeLimits,
    pub jpeg_background: Option<[u8; 3]>,
}
impl Options {
    pub fn canonical(profile: OutputProfile) -> Result<Self, ImageError> {
        if !matches!(
            profile,
            OutputProfile::WebpExact | OutputProfile::AvifQuality90
        ) {
            return Err(ImageError::InvalidOptions);
        }
        Ok(Self {
            method: Operation::NormalizeCanonical,
            profile,
            resize: transform::ResizeLimits::default(),
            jpeg_background: None,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub method: Operation,
    pub source_format: Format,
    pub source_sha256: [u8; 32],
    /// Always normalized input, NEVER the lossy decoded AVIF digest.
    pub pixels_sha256: [u8; 32],
    pub decoded_pixels_sha256: [u8; 32],
    pub output_sha256: [u8; 32],
    pub dimensions: Dimensions,
    pub output_format: Format,
    pub byte_length: u64,
    pub profile_fingerprint: String,
    pub color: ColorProvenance,
    pub normalized_dimensions: Dimensions,
    pub metadata: metadata::Metadata,
    pub resized: bool,
    pub alpha_verified_exact: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ColorProvenance {
    AssumedSrgb,
    DeclaredSrgb,
    IccConverted,
}
/// Engine-created result; not Saved and not a filesystem publication capability.
pub struct Candidate {
    bytes: Vec<u8>,
    report: Report,
}
impl Candidate {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn report(&self) -> &Report {
        &self.report
    }
    pub fn into_parts(self) -> (Vec<u8>, Report) {
        (self.bytes, self.report)
    }
}
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(crate) struct Pixels {
    pub dimensions: Dimensions,
    pub rgba: Vec<u8>,
}
impl Pixels {
    fn new(dimensions: Dimensions, rgba: Vec<u8>) -> Result<Self, ImageError> {
        if dimensions.byte_len()? != rgba.len() {
            return Err(ImageError::Malformed);
        }
        Ok(Self { dimensions, rgba })
    }
}

pub fn process(input: &[u8], options: &Options) -> Result<Candidate, ImageError> {
    process_with_observer(input, options, || Ok(()))
}
/// Worker-local phase notification; no publication authority is granted by it.
pub fn process_with_observer(
    input: &[u8],
    options: &Options,
    on_encoding: impl FnOnce() -> Result<(), ImageError>,
) -> Result<Candidate, ImageError> {
    if input.is_empty() || input.len() > MAX_INPUT_BYTES {
        return Err(ImageError::LimitExceeded);
    }
    if options.method == Operation::NormalizeCanonical
        && (!matches!(
            options.profile,
            OutputProfile::WebpExact | OutputProfile::AvifQuality90
        ) || options.resize.max_side.is_some()
            || options.resize.max_width.is_some()
            || options.resize.max_height.is_some())
    {
        return Err(ImageError::InvalidOptions);
    }
    if options.jpeg_background.is_some() && options.profile != OutputProfile::JpegQuality90 {
        return Err(ImageError::InvalidOptions);
    }
    let format = inspect::classify(input)?;
    inspect::check_container(input, format)?;
    let mut metadata = metadata::extract(input, format)?;
    let (mut pixels, color, icc) = decode(input, format)?;
    if let Some((digest, len)) = icc {
        metadata.icc_sha256 = Some(digest);
        metadata.packet_bytes = metadata
            .packet_bytes
            .checked_add(len)
            .ok_or(ImageError::MetadataRejected)?;
        if metadata.packet_bytes > metadata::MAX_PACKET_BYTES {
            return Err(ImageError::MetadataRejected);
        }
    }
    let orientation = if let Some(container) = metadata.container_orientation {
        if metadata.orientation.is_some_and(|exif| exif != container) {
            return Err(ImageError::OrientationConflict);
        }
        container
    } else {
        metadata.orientation.unwrap_or(1)
    };
    pixels = transform::orient(pixels, orientation)?;
    let normalized_hash = sha256(&pixels.rgba);
    let normalized_dimensions = pixels.dimensions;
    let plan = transform::fit_inside(pixels.dimensions, options.resize)?;
    let resized = plan.output != pixels.dimensions;
    if resized {
        pixels = transform::resize(pixels, plan.output)?;
    }
    // For derivatives this remains the normalized full-input hash. Verification
    // compares against transformed pixels, not this provenance digest.
    let transformed_hash = sha256(&pixels.rgba);
    on_encoding()?;
    let bytes = encode(&pixels, options)?;
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err(ImageError::LimitExceeded);
    }
    let (decoded, _, _) = decode(&bytes, options.profile.format())?;
    if decoded.dimensions != pixels.dimensions {
        return Err(ImageError::VerificationFailed);
    }
    let alpha_exact = pixels
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(decoded.rgba.as_chunks::<4>().0)
        .all(|(a, b)| a[3] == b[3]);
    if options.profile != OutputProfile::JpegQuality90 && !alpha_exact {
        return Err(ImageError::VerificationFailed);
    }
    if matches!(
        options.profile,
        OutputProfile::WebpExact | OutputProfile::Png
    ) && sha256(&decoded.rgba) != transformed_hash
    {
        return Err(ImageError::VerificationFailed);
    }
    metadata.applied_orientation = orientation;
    let report = Report {
        method: options.method,
        source_format: format,
        source_sha256: sha256(input),
        pixels_sha256: normalized_hash,
        decoded_pixels_sha256: sha256(&decoded.rgba),
        output_sha256: sha256(&bytes),
        dimensions: pixels.dimensions,
        output_format: options.profile.format(),
        byte_length: bytes.len() as u64,
        profile_fingerprint: options.profile.fingerprint(),
        color,
        normalized_dimensions,
        metadata,
        resized,
        alpha_verified_exact: alpha_exact,
    };
    Ok(Candidate { bytes, report })
}

type Decoded = (Pixels, ColorProvenance, Option<([u8; 32], usize)>);
fn decode(input: &[u8], format: Format) -> Result<Decoded, ImageError> {
    match format {
        Format::Avif => avif::decode(input),
        Format::Webp => {
            let d = sm_codec::inspect(input).map_err(|_| ImageError::Malformed)?;
            let dimensions = Dimensions {
                width: d.width,
                height: d.height,
            };
            dimensions.working_budget(12)?;
            let p = sm_codec::decode_rgba(input).map_err(|_| ImageError::CodecFailed)?;
            let mut pixels = Pixels::new(dimensions, p.pixels().to_vec())?;
            let mut color = ColorProvenance::AssumedSrgb;
            let mut icc = None;
            for (tag, data) in inspect::webp_chunks(input)? {
                if tag == b"ICCP" {
                    if icc.is_some() {
                        return Err(ImageError::MetadataRejected);
                    }
                    convert_icc(&mut pixels.rgba, data)?;
                    color = ColorProvenance::IccConverted;
                    icc = Some((sha256(data), data.len()));
                }
            }
            Ok((pixels, color, icc))
        }
        _ => {
            let mut reader = ImageReader::with_format(Cursor::new(input), format.image_format()?);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(32768);
            limits.max_image_height = Some(32768);
            limits.max_alloc = Some(MAX_WORK_BYTES);
            reader.limits(limits);
            let mut decoder = reader.into_decoder().map_err(|_| ImageError::Malformed)?;
            let (width, height) = decoder.dimensions();
            let dimensions = Dimensions { width, height };
            dimensions.working_budget(12)?;
            if !matches!(
                decoder.color_type(),
                image::ColorType::L8
                    | image::ColorType::La8
                    | image::ColorType::Rgb8
                    | image::ColorType::Rgba8
            ) {
                return Err(ImageError::ColorConversionRequired);
            }
            let icc = decoder
                .icc_profile()
                .map_err(|_| ImageError::MetadataRejected)?;
            let pixels = DynamicImage::from_decoder(decoder)
                .map_err(|_| ImageError::CodecFailed)?
                .to_rgba8()
                .into_raw();
            let mut p = Pixels::new(dimensions, pixels)?;
            let icc_provenance = icc.as_ref().map(|v| (sha256(v), v.len()));
            let color = if let Some(icc) = icc {
                if icc.len() > 4 * 1024 * 1024 {
                    return Err(ImageError::MetadataRejected);
                }
                convert_icc(&mut p.rgba, &icc)?;
                ColorProvenance::IccConverted
            } else {
                ColorProvenance::AssumedSrgb
            };
            Ok((p, color, icc_provenance))
        }
    }
}

pub(crate) fn convert_icc(rgba: &mut [u8], icc: &[u8]) -> Result<(), ImageError> {
    let profile = lcms2::Profile::new_icc(icc).map_err(|_| ImageError::ColorConversionRequired)?;
    if profile.color_space() == lcms2::ColorSpaceSignature::GrayData {
        let gray = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[0])
            .collect::<Vec<_>>();
        let mut rgb = vec![[0u8; 3]; gray.len()];
        let transform = lcms2::Transform::new_flags(
            &profile,
            lcms2::PixelFormat::GRAY_8,
            &lcms2::Profile::new_srgb(),
            lcms2::PixelFormat::RGB_8,
            lcms2::Intent::RelativeColorimetric,
            lcms2::Flags::BLACKPOINT_COMPENSATION,
        )
        .map_err(|_| ImageError::ColorConversionRequired)?;
        transform.transform_pixels(&gray, &mut rgb);
        for (p, rgb) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(rgb) {
            p[..3].copy_from_slice(&rgb);
        }
        return Ok(());
    }
    if profile.color_space() != lcms2::ColorSpaceSignature::RgbData {
        return Err(ImageError::ColorConversionRequired);
    }
    let transform = lcms2::Transform::new_flags(
        &profile,
        lcms2::PixelFormat::RGBA_8,
        &lcms2::Profile::new_srgb(),
        lcms2::PixelFormat::RGBA_8,
        lcms2::Intent::RelativeColorimetric,
        lcms2::Flags::COPY_ALPHA | lcms2::Flags::BLACKPOINT_COMPENSATION,
    )
    .map_err(|_| ImageError::ColorConversionRequired)?;
    let pixels: &mut [[u8; 4]] = rgba.as_chunks_mut::<4>().0;
    transform.transform_in_place(pixels);
    Ok(())
}

struct LimitedWriter(Vec<u8>);
impl std::io::Write for LimitedWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if b.len() > MAX_OUTPUT_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.0.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode(p: &Pixels, o: &Options) -> Result<Vec<u8>, ImageError> {
    let d = p.dimensions;
    match o.profile {
        OutputProfile::WebpExact => {
            let image = sm_codec::RgbaImage::from_rgba(d.width, d.height, p.rgba.clone())
                .map_err(|_| ImageError::LimitExceeded)?;
            sm_codec::encode_exact_lossless_verified(&image)
                .map(|c| c.into_canonical().into_bytes())
                .map_err(|_| ImageError::CodecFailed)
        }
        OutputProfile::WebpQuality90 => {
            let image = sm_codec::RgbaImage::from_rgba(d.width, d.height, p.rgba.clone())
                .map_err(|_| ImageError::LimitExceeded)?;
            sm_codec::encode_lossy90_export(&image).map_err(|_| ImageError::CodecFailed)
        }
        OutputProfile::AvifQuality90 => avif::encode(p),
        OutputProfile::Png => {
            let mut out = LimitedWriter(Vec::new());
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(&p.rgba, d.width, d.height, image::ExtendedColorType::Rgba8)
                .map_err(|_| ImageError::CodecFailed)?;
            Ok(out.0)
        }
        OutputProfile::JpegQuality90 => {
            let transparent = p.rgba.as_chunks::<4>().0.iter().any(|px| px[3] != 255);
            if transparent && o.jpeg_background.is_none() {
                return Err(ImageError::BackgroundRequired);
            }
            let rgb = transform::composite_rgb(&p.rgba, o.jpeg_background.unwrap_or([255; 3]));
            let mut out = LimitedWriter(Vec::new());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
                .encode(&rgb, d.width, d.height, image::ExtendedColorType::Rgb8)
                .map_err(|_| ImageError::CodecFailed)?;
            Ok(out.0)
        }
    }
}
