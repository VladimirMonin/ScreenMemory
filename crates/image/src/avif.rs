//! All pointers stay local to worker calls; every allocation has its native owner.
use crate::{ColorProvenance, Dimensions, ImageError, Pixels, MAX_OUTPUT_BYTES, MAX_PIXELS};
use libavif_sys::*;
use std::ptr::NonNull;

struct NativeImage(NonNull<avifImage>);
impl Drop for NativeImage {
    fn drop(&mut self) {
        unsafe {
            avifImageDestroy(self.0.as_ptr());
        }
    }
}
struct Decoder(NonNull<avifDecoder>);
impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            avifDecoderDestroy(self.0.as_ptr());
        }
    }
}
struct Encoder(NonNull<avifEncoder>);
impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            avifEncoderDestroy(self.0.as_ptr());
        }
    }
}
struct Data(avifRWData);
impl Drop for Data {
    fn drop(&mut self) {
        unsafe {
            avifRWDataFree(&mut self.0);
        }
    }
}
fn checked(result: avifResult) -> Result<(), ImageError> {
    if result == AVIF_RESULT_OK {
        Ok(())
    } else {
        Err(ImageError::CodecFailed)
    }
}

// Decoder borrows input memory for the duration of this private function's caller.
fn parse(bytes: &[u8]) -> Result<Decoder, ImageError> {
    unsafe {
        let d = Decoder(NonNull::new(avifDecoderCreate()).ok_or(ImageError::CodecFailed)?);
        let decoder = &mut *d.0.as_ptr();
        decoder.codecChoice = AVIF_CODEC_CHOICE_AOM;
        decoder.maxThreads = 2;
        decoder.imageSizeLimit = MAX_PIXELS as u32;
        decoder.imageDimensionLimit = 32768;
        decoder.imageCountLimit = 2;
        checked(avifDecoderSetIOMemory(
            d.0.as_ptr(),
            bytes.as_ptr(),
            bytes.len(),
        ))?;
        checked(avifDecoderParse(d.0.as_ptr()))?;
        if decoder.imageCount != 1 {
            return Err(ImageError::MultiframeNotSupported);
        }
        let image = &*decoder.image;
        Dimensions {
            width: image.width,
            height: image.height,
        }
        .working_budget(16)?;
        if image.depth != 8
            || !matches!(image.transferCharacteristics, 0 | 2 | 13)
            || !matches!(image.colorPrimaries, 0..=2)
            || image.clli.maxCLL != 0
            || image.clli.maxPALL != 0
        {
            return Err(ImageError::ColorConversionRequired);
        }
        container_orientation(image)?;
        if image.icc.size > 4 * 1024 * 1024
            || image.exif.size + image.xmp.size + image.icc.size > crate::metadata::MAX_PACKET_BYTES
        {
            return Err(ImageError::MetadataRejected);
        }
        Ok(d)
    }
}
type MetadataPackets = (Vec<u8>, Vec<u8>, Option<u16>);
pub(crate) fn packets(bytes: &[u8]) -> Result<MetadataPackets, ImageError> {
    let d = parse(bytes)?;
    unsafe {
        let image = &*(*d.0.as_ptr()).image;
        let copy = |data: &avifRWData| {
            if data.size == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(data.data, data.size).to_vec()
            }
        };
        Ok((
            copy(&image.exif),
            copy(&image.xmp),
            container_orientation(image)?,
        ))
    }
}
pub(crate) fn decode(bytes: &[u8]) -> Result<crate::Decoded, ImageError> {
    let d = parse(bytes)?;
    unsafe {
        checked(avifDecoderNextImage(d.0.as_ptr()))?;
        let image = &*(*d.0.as_ptr()).image;
        let dimensions = Dimensions {
            width: image.width,
            height: image.height,
        };
        let mut rgba = vec![0; dimensions.byte_len()?];
        let mut rgb = avifRGBImage::default();
        avifRGBImageSetDefaults(&mut rgb, image);
        rgb.depth = 8;
        rgb.format = AVIF_RGB_FORMAT_RGBA;
        rgb.maxThreads = 2;
        rgb.alphaPremultiplied = 0;
        rgb.pixels = rgba.as_mut_ptr();
        rgb.rowBytes = dimensions.width * 4;
        checked(avifImageYUVToRGB(image, &mut rgb))?;
        let mut color = if image.transferCharacteristics == 13 && image.colorPrimaries == 1 {
            ColorProvenance::DeclaredSrgb
        } else {
            ColorProvenance::AssumedSrgb
        };
        let mut icc_provenance = None;
        if image.icc.size != 0 {
            let icc = std::slice::from_raw_parts(image.icc.data, image.icc.size);
            crate::convert_icc(&mut rgba, icc)?;
            icc_provenance = Some((crate::sha256(icc), icc.len()));
            color = ColorProvenance::IccConverted;
        }
        Ok((Pixels::new(dimensions, rgba)?, color, icc_provenance))
    }
}
fn container_orientation(image: &avifImage) -> Result<Option<u16>, ImageError> {
    let flags = image.transformFlags;
    if flags & !(AVIF_TRANSFORM_PASP | AVIF_TRANSFORM_IROT | AVIF_TRANSFORM_IMIR) != 0
        || (flags & AVIF_TRANSFORM_PASP != 0
            && (image.pasp.hSpacing == 0 || image.pasp.hSpacing != image.pasp.vSpacing))
    {
        return Err(ImageError::GeometryConversionRequired);
    }
    if flags & (AVIF_TRANSFORM_IROT | AVIF_TRANSFORM_IMIR) == 0 {
        return Ok(None);
    }
    let angle = if flags & AVIF_TRANSFORM_IROT != 0 {
        image.irot.angle as usize
    } else {
        0
    };
    if angle > 3 {
        return Err(ImageError::GeometryConversionRequired);
    }
    // Locked libavif exif.c/MIAF: anti-clockwise rotation BEFORE mirror.
    let orientation = if flags & AVIF_TRANSFORM_IMIR == 0 {
        [1, 8, 3, 6][angle]
    } else if image.imir.axis == 0 {
        [4, 5, 2, 7][angle]
    } else if image.imir.axis == 1 {
        [2, 7, 4, 5][angle]
    } else {
        return Err(ImageError::GeometryConversionRequired);
    };
    Ok(Some(orientation))
}

pub(crate) fn encode(p: &Pixels) -> Result<Vec<u8>, ImageError> {
    p.dimensions.working_budget(16)?;
    unsafe {
        let image = NativeImage(
            NonNull::new(avifImageCreate(
                p.dimensions.width,
                p.dimensions.height,
                8,
                AVIF_PIXEL_FORMAT_YUV444,
            ))
            .ok_or(ImageError::CodecFailed)?,
        );
        let im = &mut *image.0.as_ptr();
        im.colorPrimaries = 1;
        im.transferCharacteristics = 13;
        im.matrixCoefficients = 1;
        im.yuvRange = AVIF_RANGE_FULL;
        im.alphaPremultiplied = 0;
        let mut rgb = avifRGBImage::default();
        avifRGBImageSetDefaults(&mut rgb, im);
        rgb.depth = 8;
        rgb.format = AVIF_RGB_FORMAT_RGBA;
        rgb.maxThreads = 2;
        rgb.alphaPremultiplied = 0;
        // RGBToYUV reads the caller's immutable packed buffer; the C API exposes a
        // mutable pointer but does not mutate it. Native image owns resulting planes.
        rgb.pixels = p.rgba.as_ptr().cast_mut();
        rgb.rowBytes = p.dimensions.width * 4;
        checked(avifImageRGBToYUV(im, &rgb))?;
        let encoder = Encoder(NonNull::new(avifEncoderCreate()).ok_or(ImageError::CodecFailed)?);
        let e = &mut *encoder.0.as_ptr();
        e.codecChoice = AVIF_CODEC_CHOICE_AOM;
        e.maxThreads = 2;
        e.speed = 6;
        e.quality = 90;
        e.qualityAlpha = 100;
        let mut data = Data(avifRWData::default());
        checked(avifEncoderWrite(encoder.0.as_ptr(), im, &mut data.0))?;
        if data.0.size > MAX_OUTPUT_BYTES {
            return Err(ImageError::LimitExceeded);
        }
        let bytes = std::slice::from_raw_parts(data.0.data, data.0.size).to_vec();
        let verified = parse(&bytes)?;
        let encoded = &*(*verified.0.as_ptr()).image;
        if encoded.depth != 8
            || encoded.yuvFormat != AVIF_PIXEL_FORMAT_YUV444
            || encoded.yuvRange != AVIF_RANGE_FULL
            || encoded.colorPrimaries != 1
            || encoded.transferCharacteristics != 13
            || encoded.matrixCoefficients != 1
        {
            return Err(ImageError::VerificationFailed);
        }
        Ok(bytes)
    }
}
