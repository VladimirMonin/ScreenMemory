//! Real synthetic round-trips through the bundled exact-lossless WebP codec.
//! All pixels are generated in-process; no personal image or donor fixture is read.
use sm_codec::{
    decode_canonical, decode_rgba, encode_canonical, encode_exact_lossless,
    encode_exact_lossless_verified, encode_pixels_verified, encoder_fingerprint, inspect,
    CodecFormat, PixelLayout, RgbaImage, VerifiedCanonicalWebp, WEBP_EXACT_LOSSLESS_PROFILE,
    WEBP_MIME,
};

/// Deterministic synthetic byte stream (LCG), so a failure is fully reproducible.
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

fn rgba(width: u32, height: u32, seed: u64) -> RgbaImage {
    let len = (width as usize) * (height as usize) * 4;
    RgbaImage::from_rgba(width, height, synthetic(len, seed)).expect("valid synthetic RGBA")
}

#[test]
fn s02_codec_01_rgba_roundtrip_is_exact_for_synthetic_sizes() {
    for (width, height) in [(1, 1), (2, 1), (3, 5), (257, 129), (2000, 2000)] {
        let original = rgba(width, height, u64::from(width) * 31 + u64::from(height) + 7);
        let verified = encode_exact_lossless_verified(&original).expect("encode+verify");
        assert_eq!(verified.canonical().dimensions().width, width);
        assert_eq!(verified.canonical().dimensions().height, height);
        assert_eq!(verified.canonical().profile(), WEBP_EXACT_LOSSLESS_PROFILE);
        assert_eq!(verified.canonical().mime(), WEBP_MIME);
        assert_eq!(
            verified.canonical().bytes().len() % 2,
            0,
            "RIFF size is even"
        );
        assert_eq!(&verified.canonical().bytes()[..4], b"RIFF");
        assert_eq!(&verified.canonical().bytes()[8..12], b"WEBP");
        assert_eq!(
            inspect(verified.canonical().bytes()).expect("header"),
            original.dimensions()
        );
        let decoded = decode_rgba(verified.canonical().bytes()).expect("decode");
        assert_eq!(decoded.dimensions(), original.dimensions());
        assert_eq!(
            decoded.pixels(),
            original.pixels(),
            "{width}x{height} must round-trip byte for byte"
        );
    }
}

#[test]
fn s02_codec_02_transparent_pixels_keep_their_rgb_values() {
    // Alpha 0 with non-zero RGB: only `exact = 1` lossless preserves those bytes.
    let width = 64u32;
    let height = 4u32;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        for column in 0..width {
            let rgb = [
                (row as u8).wrapping_mul(17).wrapping_add(1),
                (column as u8).wrapping_mul(3),
                0xf0,
            ];
            for channel in rgb {
                pixels.push(channel);
            }
            pixels.push(if column % 2 == 0 { 0 } else { 255 });
        }
    }
    let image = RgbaImage::from_rgba(width, height, pixels).expect("valid RGBA");
    let verified = encode_exact_lossless_verified(&image).expect("encode+verify");
    let decoded = decode_rgba(verified.canonical().bytes()).expect("decode");
    assert_eq!(decoded.pixels(), image.pixels());
    let transparent = decoded
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] == 0)
        .count();
    assert_eq!(transparent, (width * height / 2) as usize);
    assert!(
        decoded
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] == 0)
            .any(|pixel| pixel[..3] != [0, 0, 0]),
        "transparent pixels must not be flattened to black"
    );
}

#[test]
fn s02_codec_03_bgr_roundtrip_drops_no_colour_and_stays_opaque() {
    for (width, height) in [(1, 1), (5, 3), (257, 129)] {
        let len = (width as usize) * (height as usize) * 3;
        let bgr = synthetic(len, u64::from(width) * 13 + u64::from(height) + 3);
        let verified =
            encode_pixels_verified(PixelLayout::Bgr8, width, height, &bgr).expect("bgr encode");
        let decoded = decode_rgba(verified.canonical().bytes()).expect("decode");
        let expected =
            RgbaImage::from_layout(PixelLayout::Bgr8, width, height, &bgr).expect("norm");
        assert_eq!(decoded.pixels(), expected.pixels(), "{width}x{height} BGR");
        assert!(
            decoded
                .pixels()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255),
            "a 24-bit source is opaque"
        );
    }
}

#[test]
fn s02_codec_04_grayscale_with_alpha_roundtrip_is_exact() {
    let width = 129u32;
    let height = 33u32;
    let mut gray_alpha = Vec::with_capacity((width * height * 2) as usize);
    for index in 0..(width * height) {
        gray_alpha.push((index % 256) as u8);
        gray_alpha.push(if index % 7 == 0 { 0 } else { 0xff });
    }
    let normalized = RgbaImage::from_gray_alpha(width, height, &gray_alpha).expect("normalize");
    let verified = encode_exact_lossless_verified(&normalized).expect("encode+verify");
    let decoded = decode_rgba(verified.canonical().bytes()).expect("decode");
    assert_eq!(decoded.pixels(), normalized.pixels());
    assert!(
        decoded
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2]),
        "neutral pixels stay neutral"
    );
}

#[test]
fn s02_codec_05_verified_encoding_rejects_a_different_picture() {
    let expected = rgba(32, 16, 11);
    let other = rgba(32, 16, 12);
    let candidate = encode_exact_lossless(&other).expect("encode");
    let rejection = sm_codec::verify_exact_lossless(candidate, &expected);
    assert_eq!(
        rejection,
        Err(sm_codec::CodecError::RoundTripMismatch { offset: 0 }),
        "a verified canonical may not describe different pixels"
    );
    let wider = rgba(64, 16, 11);
    let candidate = encode_exact_lossless(&wider).expect("encode");
    assert_eq!(
        sm_codec::verify_exact_lossless(candidate, &expected),
        Err(sm_codec::CodecError::RoundTripMismatch { offset: 0 })
    );
    let verified: VerifiedCanonicalWebp =
        encode_exact_lossless_verified(&expected).expect("encode+verify");
    assert_eq!(
        verified.into_canonical().dimensions(),
        expected.dimensions()
    );
}

#[test]
fn s02_codec_06_canonical_entry_points_and_profile_fingerprint() {
    let image = rgba(9, 7, 5);
    let canonical = encode_canonical(CodecFormat::Webp, &image).expect("webp canonical");
    assert_eq!(canonical.profile(), WEBP_EXACT_LOSSLESS_PROFILE);
    assert_eq!(canonical.mime(), WEBP_MIME);
    assert_eq!(
        decode_canonical(CodecFormat::Webp, canonical.bytes())
            .expect("webp decode")
            .pixels(),
        image.pixels()
    );
    let fingerprint = canonical.encoder_fingerprint();
    assert!(fingerprint.starts_with(WEBP_EXACT_LOSSLESS_PROFILE));
    assert!(fingerprint.contains("exact=1") && fingerprint.contains("lossless=1"));
    assert!(
        fingerprint.contains("libwebp="),
        "profile pins the native revision"
    );
    assert_eq!(fingerprint, encoder_fingerprint());
    // Debug output of encoded/decoded values must not carry pixels, versions or paths.
    let debug = format!("{canonical:?}");
    assert!(!debug.contains("libwebp") && !debug.contains('\\') && !debug.contains('/'));
    let decoded_debug = format!("{:?}", decode_rgba(canonical.bytes()).expect("decode"));
    assert!(decoded_debug.contains("dimensions") && !decoded_debug.contains("pixels"));
}
