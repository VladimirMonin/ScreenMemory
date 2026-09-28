//! Bounds, empty/truncated/malformed input and the honest per-format capability.
//! Every rejected input must produce a typed error instead of a panic or a decode.
use sm_codec::{
    capability, decode_canonical, decode_rgba, decode_rgba_with_limits, encode_canonical,
    encode_exact_lossless, encode_pixels_verified, inspect, CanonicalWebp, CodecCapability,
    CodecError, CodecFormat, CodecLimits, Dimensions, PixelLayout, RgbaImage, DEFAULT_CODEC_LIMITS,
};

fn synthetic(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        })
        .collect()
}

fn small_image() -> RgbaImage {
    RgbaImage::from_rgba(8, 4, synthetic(8 * 4 * 4, 99)).expect("valid synthetic RGBA")
}

#[test]
fn s02_codec_07_geometry_bounds_reject_before_the_native_call() {
    let over_side = 16_384u32;
    assert_eq!(
        RgbaImage::from_rgba(over_side, 1, Vec::new()),
        Err(CodecError::CanonicalSizeUnsupported {
            width: over_side,
            height: 1
        })
    );
    assert_eq!(
        RgbaImage::from_rgba(0, 0, Vec::new()),
        Err(CodecError::CanonicalSizeUnsupported {
            width: 0,
            height: 0
        })
    );
    // Both sides are inside the WebP limit, the declared area is not.
    assert_eq!(
        RgbaImage::from_rgba(16_383, 16_383, Vec::new()),
        Err(CodecError::ImageLimitExceeded {
            pixels: 16_383 * 16_383,
            budget: DEFAULT_CODEC_LIMITS.max_pixels
        })
    );
    assert_eq!(
        RgbaImage::from_layout(PixelLayout::Bgr8, 1, 1, &[0, 0]),
        Err(CodecError::BufferLengthMismatch {
            expected: 3,
            actual: 2
        })
    );
    assert_eq!(
        RgbaImage::from_gray_alpha(2, 2, &[0, 0, 0]),
        Err(CodecError::BufferLengthMismatch {
            expected: 8,
            actual: 3
        })
    );
}

#[test]
fn s02_codec_07_pixel_budget_is_enforced_on_a_real_container() {
    let image = small_image();
    let canonical = encode_exact_lossless(&image).expect("encode");
    let tight = CodecLimits {
        max_side: 16_383,
        max_pixels: 31,
    };
    assert_eq!(
        decode_rgba_with_limits(canonical.bytes(), tight),
        Err(CodecError::ImageLimitExceeded {
            pixels: 32,
            budget: 31
        })
    );
    let side_only = CodecLimits {
        max_side: 4,
        max_pixels: 96_000_000,
    };
    assert_eq!(
        decode_rgba_with_limits(canonical.bytes(), side_only),
        Err(CodecError::CanonicalSizeUnsupported {
            width: 8,
            height: 4
        })
    );
    assert_eq!(
        decode_rgba_with_limits(canonical.bytes(), DEFAULT_CODEC_LIMITS)
            .expect("default limits")
            .pixels(),
        image.pixels()
    );
}

#[test]
fn s02_codec_08_empty_truncated_and_malformed_input_fail_closed() {
    let canonical = encode_exact_lossless(&small_image()).expect("encode");
    let valid = canonical.bytes();

    assert_eq!(inspect(&[]), Err(CodecError::EmptyInput));
    assert_eq!(decode_rgba(&[]), Err(CodecError::EmptyInput));
    assert_eq!(inspect(b"RIFF"), Err(CodecError::TruncatedInput));
    assert_eq!(decode_rgba(&valid[..8]), Err(CodecError::TruncatedInput));
    assert_eq!(decode_rgba(&valid[..20]), Err(CodecError::TruncatedInput));
    assert_eq!(
        decode_rgba(&valid[..valid.len() - 1]),
        Err(CodecError::TruncatedInput),
        "a container whose declared payload is missing bytes is truncated"
    );

    let garbage = synthetic(64, 424_242);
    assert_eq!(inspect(&garbage), Err(CodecError::MalformedInput));
    assert_eq!(decode_rgba(&garbage), Err(CodecError::MalformedInput));
    let riff_shaped = [
        b"RIFF".as_slice(),
        &13u32.to_le_bytes(),
        b"WEBP",
        &garbage[..32],
    ]
    .concat();
    assert_eq!(inspect(&riff_shaped), Err(CodecError::MalformedInput));

    // Empty and malformed input never yields pixels and never reaches the decoder.
    for rejected in [Vec::new(), b"x".to_vec(), garbage] {
        assert!(decode_rgba(&rejected).is_err());
    }
}

#[test]
fn s02_codec_09_capability_is_honest_and_avif_fails_typed() {
    assert_eq!(capability(CodecFormat::Webp), CodecCapability::Implemented);
    assert_eq!(
        capability(CodecFormat::Avif),
        CodecCapability::NotImplemented
    );
    assert_eq!(CodecFormat::Webp.mime(), "image/webp");
    assert_eq!(CodecFormat::Avif.mime(), "image/avif");
    let image = small_image();
    assert_eq!(
        encode_canonical(CodecFormat::Avif, &image),
        Err(CodecError::NotImplemented {
            format: CodecFormat::Avif
        })
    );
    let canonical = encode_exact_lossless(&image).expect("encode");
    assert_eq!(
        decode_canonical(CodecFormat::Avif, canonical.bytes()),
        Err(CodecError::NotImplemented {
            format: CodecFormat::Avif
        })
    );
    assert_eq!(
        CodecError::NotImplemented {
            format: CodecFormat::Avif
        }
        .stable_code(),
        "NOT_IMPLEMENTED"
    );
    assert_eq!(
        CodecError::CanonicalSizeUnsupported {
            width: 1,
            height: 1
        }
        .stable_code(),
        "CANONICAL_SIZE_UNSUPPORTED"
    );
    assert_eq!(
        CodecError::ImageLimitExceeded {
            pixels: 2,
            budget: 1
        }
        .stable_code(),
        "IMAGE_LIMIT_EXCEEDED"
    );
    assert_eq!(
        CodecError::RoundTripMismatch { offset: 3 }.stable_code(),
        "ASSET_INTEGRITY_FAILED"
    );
}

#[test]
fn s02_codec_10_oversized_declared_canvas_is_refused_before_any_decode() {
    // A real VP8X container header declaring an oversized canvas: the guard must reject
    // it from the header alone, so no native decode buffer is ever allocated.
    fn vp8x_container(canvas_width: u32, canvas_height: u32) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(b"WEBP");
        body.extend_from_slice(b"VP8X");
        body.extend_from_slice(&10u32.to_le_bytes());
        body.push(0);
        body.extend_from_slice(&[0, 0, 0]);
        for side in [canvas_width - 1, canvas_height - 1] {
            body.extend_from_slice(&[
                (side & 0xff) as u8,
                ((side >> 8) & 0xff) as u8,
                ((side >> 16) & 0xff) as u8,
            ]);
        }
        let mut container = Vec::new();
        container.extend_from_slice(b"RIFF");
        container.extend_from_slice(&(body.len() as u32).to_le_bytes());
        container.extend_from_slice(&body);
        container
    }
    let oversized = vp8x_container(20_000, 20_000);
    assert_eq!(
        inspect(&oversized),
        Err(CodecError::CanonicalSizeUnsupported {
            width: 20_000,
            height: 20_000
        })
    );
    assert_eq!(
        decode_rgba(&oversized),
        Err(CodecError::CanonicalSizeUnsupported {
            width: 20_000,
            height: 20_000
        })
    );
    // An explicit wider ceiling cannot raise the shared product budget, and a wider
    // request still refuses the oversized header instead of decoding it.
    let widened = CodecLimits {
        max_side: 32_000,
        max_pixels: u64::MAX,
    };
    assert!(decode_rgba_with_limits(&oversized, widened).is_err());
    let inside = encode_exact_lossless(&small_image()).expect("encode");
    assert!(
        decode_rgba_with_limits(inside.bytes(), widened)
            .expect("an image inside the shared budget still decodes")
            .pixels()
            == small_image().pixels()
    );
}

#[test]
fn s02_codec_09_errors_and_debug_carry_no_native_revision_or_path() {
    let errors = [
        CodecError::NotImplemented {
            format: CodecFormat::Avif,
        },
        CodecError::CanonicalSizeUnsupported {
            width: 0,
            height: 1,
        },
        CodecError::ImageLimitExceeded {
            pixels: 9,
            budget: 1,
        },
        CodecError::BufferLengthMismatch {
            expected: 4,
            actual: 3,
        },
        CodecError::EmptyInput,
        CodecError::TruncatedInput,
        CodecError::MalformedInput,
        CodecError::EncodeFailed { native_code: 5 },
        CodecError::DecodeFailed { native_code: 3 },
        CodecError::RoundTripMismatch { offset: 1 },
    ];
    for error in errors {
        let text = format!("{error}");
        assert!(!text.is_empty());
        for forbidden in ["libwebp", "\\\\", "/", "C:", ".dll", ".lib"] {
            assert!(
                !text.contains(forbidden),
                "error text must not disclose native provenance: {text}"
            );
        }
        let debug = format!("{error:?}");
        assert!(!debug.contains("libwebp") && !debug.contains('/') && !debug.contains('\\'));
    }
    // The encoder fingerprint is evidence-only; it must never embed a filesystem path.
    let fingerprint = sm_codec::encoder_fingerprint();
    assert!(!fingerprint.contains('/') && !fingerprint.contains('\\'));
    assert!(!fingerprint.to_lowercase().contains("c:"));
    // Encoded values and pixel buffers print geometry, not content.
    let canonical: CanonicalWebp = encode_exact_lossless(&small_image()).expect("encode");
    assert!(!format!("{canonical:?}").contains("libwebp"));
    assert!(!format!("{:?}", small_image()).contains("pixels"));
    assert_eq!(
        encode_pixels_verified(PixelLayout::Rgba8, 8, 4, small_image().pixels())
            .expect("rgba verified")
            .canonical()
            .dimensions(),
        Dimensions {
            width: 8,
            height: 4
        }
    );
}
