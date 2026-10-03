use image::ImageEncoder;
use sm_image::{
    metadata::{extract_xmp, parse_exif_time, Metadata, MetadataWarning},
    process, sha256,
    transform::{fit_inside, ResizeLimits},
    Dimensions, ImageError, Options, OutputProfile,
};

fn pixels(w: u32, h: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| {
            [
                (i * 17) as u8,
                (i * 37) as u8,
                (i * 71) as u8,
                if i % 3 == 0 { 0 } else { (i * 43) as u8 },
            ]
        })
        .collect()
}
fn png(w: u32, h: u32, p: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    image::codecs::png::PngEncoder::new(&mut b)
        .write_image(p, w, h, image::ExtendedColorType::Rgba8)
        .unwrap();
    b
}
fn decode(b: &[u8]) -> image::RgbaImage {
    image::load_from_memory(b).unwrap().to_rgba8()
}
fn options(profile: OutputProfile) -> Options {
    Options {
        method: sm_image::Operation::DeriveExport,
        profile,
        resize: ResizeLimits::default(),
        jpeg_background: None,
    }
}
fn crc(b: &[u8]) -> u32 {
    let mut c = 0xffffffffu32;
    for b in b {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = (c >> 1) ^ if c & 1 == 1 { 0xedb88320 } else { 0 };
        }
    }
    !c
}
fn chunk(t: &[u8; 4], d: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(d.len() as u32).to_be_bytes());
    b.extend_from_slice(t);
    b.extend_from_slice(d);
    let c = crc(&b[4..]);
    b.extend_from_slice(&c.to_be_bytes());
    b
}
fn tagged_png(p: &[u8], tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut b = p[..33].to_vec();
    b.extend(chunk(tag, data));
    b.extend_from_slice(&p[33..]);
    b
}
fn exif(o: u16) -> Vec<u8> {
    let mut b = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    b.extend_from_slice(&o.to_le_bytes());
    b.extend_from_slice(&[0; 6]);
    b
}

fn avif_oriented_fixture(orientation: u16, exif_orientation: Option<u16>) -> Vec<u8> {
    use libavif_sys::*;
    unsafe {
        let im = avifImageCreate(3, 2, 8, AVIF_PIXEL_FORMAT_YUV444);
        assert!(!im.is_null());
        (*im).colorPrimaries = 1;
        (*im).transferCharacteristics = 13;
        (*im).matrixCoefficients = 1;
        (*im).yuvRange = AVIF_RANGE_FULL;
        let pixels = (1..=6)
            .flat_map(|n| [40, 80, 120, n * 30])
            .collect::<Vec<u8>>();
        let mut rgb = avifRGBImage::default();
        avifRGBImageSetDefaults(&mut rgb, im);
        rgb.pixels = pixels.as_ptr().cast_mut();
        rgb.rowBytes = 12;
        rgb.format = AVIF_RGB_FORMAT_RGBA;
        assert_eq!(avifImageRGBToYUV(im, &rgb), AVIF_RESULT_OK);
        if let Some(o) = exif_orientation {
            let packet = exif(o);
            assert_eq!(
                avifImageSetMetadataExif(im, packet.as_ptr(), packet.len()),
                AVIF_RESULT_OK
            );
        }
        let (flags, angle, axis) = [
            (0, 0, 0),
            (AVIF_TRANSFORM_IMIR, 0, 1),
            (AVIF_TRANSFORM_IROT, 2, 0),
            (AVIF_TRANSFORM_IMIR, 0, 0),
            (AVIF_TRANSFORM_IROT | AVIF_TRANSFORM_IMIR, 1, 0),
            (AVIF_TRANSFORM_IROT, 3, 0),
            (AVIF_TRANSFORM_IROT | AVIF_TRANSFORM_IMIR, 3, 0),
            (AVIF_TRANSFORM_IROT, 1, 0),
        ][orientation as usize - 1];
        (*im).transformFlags = flags;
        (*im).irot.angle = angle;
        (*im).imir.axis = axis;
        let encoder = avifEncoderCreate();
        assert!(!encoder.is_null());
        (*encoder).codecChoice = AVIF_CODEC_CHOICE_AOM;
        (*encoder).maxThreads = 2;
        (*encoder).quality = 90;
        (*encoder).qualityAlpha = 100;
        (*encoder).speed = 6;
        let mut out = avifRWData::default();
        let code = avifEncoderWrite(encoder, im, &mut out);
        let bytes = if code == AVIF_RESULT_OK {
            std::slice::from_raw_parts(out.data, out.size).to_vec()
        } else {
            Vec::new()
        };
        avifRWDataFree(&mut out);
        avifEncoderDestroy(encoder);
        avifImageDestroy(im);
        assert_eq!(code, AVIF_RESULT_OK);
        bytes
    }
}
#[test]
fn s03_avif_container_orientation_once_and_exif_conflict_retained() {
    let expected = [
        vec![1, 2, 3, 4, 5, 6],
        vec![3, 2, 1, 6, 5, 4],
        vec![6, 5, 4, 3, 2, 1],
        vec![4, 5, 6, 1, 2, 3],
        vec![1, 4, 2, 5, 3, 6],
        vec![4, 1, 5, 2, 6, 3],
        vec![6, 3, 5, 2, 4, 1],
        vec![3, 6, 2, 5, 1, 4],
    ];
    for o in 1..=8 {
        let source = avif_oriented_fixture(o, None);
        let hash = sha256(&source);
        let c = process(&source, &options(OutputProfile::Png)).unwrap();
        let d = decode(c.bytes());
        assert_eq!(d.dimensions(), if o >= 5 { (2, 3) } else { (3, 2) });
        assert_eq!(
            d.as_raw()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[3] / 30)
                .collect::<Vec<_>>(),
            expected[o as usize - 1]
        );
        assert_eq!(c.report().metadata.applied_orientation, o);
        assert_eq!(hash, sha256(&source));
    }
    let source = avif_oriented_fixture(6, Some(1));
    let hash = sha256(&source);
    assert!(matches!(
        process(&source, &options(OutputProfile::Png)),
        Err(ImageError::OrientationConflict)
    ));
    assert_eq!(hash, sha256(&source));
    let source = avif_oriented_fixture(6, Some(6));
    let c = process(&source, &options(OutputProfile::Png)).unwrap();
    assert_eq!(
        decode(c.bytes())
            .as_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3] / 30)
            .collect::<Vec<_>>(),
        vec![4, 1, 5, 2, 6, 3]
    );
}
#[test]
fn s03_golden_fixture_hashes_independent_normalized_pixels() {
    for (source, expected) in [
        (
            include_bytes!("../../../tests/fixtures/images/odd-alpha.png").as_slice(),
            "4bc511ba28e4e0b855574e40523c4baf702888c1ee4c6afdb0f5db7754819c2a",
        ),
        (
            include_bytes!("../../../tests/fixtures/images/portrait-alpha.png").as_slice(),
            "21fefb7e5588677fb89f5a69ccb22571d94b0901a3d93e32da6f93579b8808e2",
        ),
        (
            include_bytes!("../../../tests/fixtures/images/numbered-grid.png").as_slice(),
            "23d226292491b08bbf24b4fe90c915a8812eaae5a967f0f7ce4d5e15538ef562",
        ),
    ] {
        let c = process(
            source,
            &Options::canonical(OutputProfile::WebpExact).unwrap(),
        )
        .unwrap();
        let actual = c
            .report()
            .pixels_sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(actual, expected);
        assert_eq!(c.report().pixels_sha256, c.report().decoded_pixels_sha256);
    }
}
#[test]
fn s03_codec_webp_odd_portrait_alpha_zero_rgb_exact() {
    for (w, h) in [(7, 3), (3, 17), (1, 1)] {
        let p = pixels(w, h);
        let input = png(w, h, &p);
        let c = process(&input, &options(OutputProfile::WebpExact)).unwrap();
        assert_eq!(c.report().pixels_sha256, sha256(&p));
        assert_eq!(c.report().decoded_pixels_sha256, sha256(&p));
        let got = sm_codec::decode_rgba(c.bytes()).unwrap();
        assert_eq!(got.pixels(), p);
        assert!(c.report().alpha_verified_exact);
    }
}
#[test]
fn s03_codec_avif_real_444_alpha_exact_separate_rgb_hash() {
    let p = pixels(7, 13);
    let input = png(7, 13, &p);
    let c = process(&input, &options(OutputProfile::AvifQuality90)).unwrap();
    assert_eq!(
        c.report().dimensions,
        Dimensions {
            width: 7,
            height: 13
        }
    );
    assert_eq!(c.report().pixels_sha256, sha256(&p));
    assert_ne!(c.report().pixels_sha256, c.report().decoded_pixels_sha256);
    assert!(c.report().alpha_verified_exact);
    assert!(c
        .report()
        .profile_fingerprint
        .contains("quality=90;alpha=100;speed=6;threads=2"));
    // Independent BMFF property oracle (not sm-image/libavif adapter): av1C
    // sequence_profile=1 and chroma_subsampling_x/y=0 for 8-bit 4:4:4.
    let properties = c
        .bytes()
        .windows(4)
        .enumerate()
        .filter(|(_, s)| *s == b"av1C")
        .map(|(i, _)| &c.bytes()[i + 4..i + 8])
        .collect::<Vec<_>>();
    assert!(properties
        .iter()
        .any(|p| p[1] >> 5 == 1 && p[2] & 0x0c == 0));
    let round = process(c.bytes(), &options(OutputProfile::Png)).unwrap();
    let got = decode(round.bytes());
    assert_eq!(got.dimensions(), (7, 13));
    for (a, b) in p
        .as_chunks::<4>()
        .0
        .iter()
        .zip(got.as_raw().as_chunks::<4>().0)
    {
        assert_eq!(a[3], b[3]);
        if a[3] != 0 {
            for channel in 0..3 {
                assert!(
                    a[channel].abs_diff(b[channel]) <= 12,
                    "quality90 synthetic SDR patch error beyond rubric"
                );
            }
        }
    }
}
#[test]
fn s03_orientation_one_to_eight_numbered_grid() {
    let p = (1..=6).flat_map(|n| [n, 0, 0, 255]).collect::<Vec<_>>();
    let source = png(3, 2, &p);
    let expected = [
        vec![1, 2, 3, 4, 5, 6],
        vec![3, 2, 1, 6, 5, 4],
        vec![6, 5, 4, 3, 2, 1],
        vec![4, 5, 6, 1, 2, 3],
        vec![1, 4, 2, 5, 3, 6],
        vec![4, 1, 5, 2, 6, 3],
        vec![6, 3, 5, 2, 4, 1],
        vec![3, 6, 2, 5, 1, 4],
    ];
    for o in 1..=8 {
        let input = tagged_png(&source, b"eXIf", &exif(o));
        let result = process(&input, &options(OutputProfile::Png)).unwrap();
        let d = decode(result.bytes());
        assert_eq!(d.dimensions(), if o >= 5 { (2, 3) } else { (3, 2) });
        assert_eq!(
            d.as_raw()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0])
                .collect::<Vec<_>>(),
            expected[o as usize - 1]
        );
        assert_eq!(result.report().metadata.applied_orientation, o);
        assert!(!result.bytes().windows(4).any(|p| p == b"eXIf"));
    }
    assert!(matches!(
        process(
            &tagged_png(&source, b"eXIf", &exif(9)),
            &options(OutputProfile::Png)
        ),
        Err(ImageError::GeometryConversionRequired)
    ));
}
#[test]
fn s03_transform_exact_tables_and_ten_thousand_seeded_bounds() {
    for (w, h, side, ow, oh) in [
        (3840, 2160, 1000, 1000, 562),
        (2160, 3840, 1000, 562, 1000),
        (5120, 2880, 2048, 2048, 1152),
        (3840, 2160, 1024, 1024, 576),
        (800, 600, 1000, 800, 600),
        (100000, 1, 320, 320, 1),
    ] {
        assert_eq!(
            fit_inside(
                Dimensions {
                    width: w,
                    height: h
                },
                ResizeLimits {
                    max_side: Some(side),
                    ..Default::default()
                }
            )
            .unwrap()
            .output,
            Dimensions {
                width: ow,
                height: oh
            }
        );
    }
    let mut seed = 19u64;
    for _ in 0..10000 {
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 32) as u32 % 100000 + 1
        };
        let d = Dimensions {
            width: next(),
            height: next(),
        };
        let l = ResizeLimits {
            max_side: Some(next()),
            max_width: Some(next()),
            max_height: Some(next()),
        };
        let p = fit_inside(d, l).unwrap();
        assert!(
            p.output.width <= d.width
                && p.output.height <= d.height
                && p.output.width > 0
                && p.output.height > 0
        );
        assert!(
            p.output.width <= l.max_width.unwrap()
                && p.output.height <= l.max_height.unwrap()
                && p.output.width.max(p.output.height) <= l.max_side.unwrap()
        );
        assert!(p.numerator <= p.denominator);
        assert_eq!(fit_inside(p.output, l).unwrap().output, p.output);
    }
    assert!(fit_inside(
        Dimensions {
            width: 0,
            height: 1
        },
        ResizeLimits::default()
    )
    .is_err());
    assert!(fit_inside(
        Dimensions {
            width: 1,
            height: 1
        },
        ResizeLimits {
            max_side: Some(0),
            ..Default::default()
        }
    )
    .is_err());
}
#[test]
fn s03_transform_linear_premultiplied_no_halo_no_upscale_canonical_unchanged() {
    let p = vec![255, 0, 0, 0, 0, 255, 0, 255, 255, 0, 0, 0, 0, 255, 0, 255];
    let input = png(2, 2, &p);
    let digest = sha256(&input);
    let mut o = options(OutputProfile::Png);
    o.resize.max_side = Some(1);
    let c = process(&input, &o).unwrap();
    let d = decode(c.bytes());
    let px = d.get_pixel(0, 0).0;
    assert_eq!(px[0], 0);
    assert_eq!(px[1], 255);
    assert_eq!(px[2], 0);
    assert!((127..=128).contains(&px[3]));
    assert_eq!(sha256(&input), digest);
    o.resize.max_side = Some(2048);
    let c = process(&input, &o).unwrap();
    assert_eq!(decode(c.bytes()).into_raw(), p);
}
#[test]
fn s03_export_four_real_formats_strip_tags_explicit_jpeg_background() {
    let p = pixels(7, 9);
    let input = tagged_png(&png(7, 9, &p), b"eXIf", &exif(1));
    let hash = sha256(&input);
    for profile in [
        OutputProfile::Png,
        OutputProfile::WebpExact,
        OutputProfile::WebpQuality90,
        OutputProfile::AvifQuality90,
        OutputProfile::JpegQuality90,
    ] {
        let mut o = options(profile);
        if profile == OutputProfile::JpegQuality90 {
            o.jpeg_background = Some([255; 3]);
        }
        let c = process(&input, &o).unwrap();
        assert_eq!(c.report().output_format, profile.format());
        assert_eq!(
            c.report().dimensions,
            Dimensions {
                width: 7,
                height: 9
            }
        );
        for private in [b"eXIf".as_slice(), b"Exif\0\0", b"XMP "] {
            assert!(!c.bytes().windows(private.len()).any(|p| p == private));
        }
        assert_eq!(sha256(&input), hash);
    }
    assert!(matches!(
        process(&input, &options(OutputProfile::JpegQuality90)),
        Err(ImageError::BackgroundRequired)
    ));
}
#[test]
fn s03_canonical_options_no_resize_and_jpeg_444_explicit_background_pixels() {
    let source = png(1, 1, &[255, 0, 0, 0]);
    let mut canonical = Options::canonical(OutputProfile::WebpExact).unwrap();
    canonical.resize.max_side = Some(320);
    assert!(matches!(
        process(&source, &canonical),
        Err(ImageError::InvalidOptions)
    ));
    assert!(Options::canonical(OutputProfile::Png).is_err());
    let mut o = options(OutputProfile::JpegQuality90);
    o.jpeg_background = Some([0, 0, 255]);
    let c = process(&source, &o).unwrap();
    let pixel = decode(c.bytes()).get_pixel(0, 0).0;
    assert!(pixel[0] <= 3 && pixel[1] <= 3 && pixel[2] >= 252 && pixel[3] == 255);
    let i = c
        .bytes()
        .windows(2)
        .position(|b| b == [0xff, 0xc0])
        .unwrap();
    let components = c.bytes()[i + 9];
    assert_eq!(components, 3);
    for n in 0..3 {
        assert_eq!(
            c.bytes()[i + 11 + n * 3],
            0x11,
            "actual JPEG sampling must be 4:4:4"
        );
    }
}
#[test]
fn s03_metadata_absent_timezone_and_calendar_offsets_not_machine_timezone() {
    let a = parse_exif_time("2025:06:12 14:30:45", None, None).unwrap();
    assert_eq!(a.utc, None);
    assert_eq!(a.offset_minutes, None);
    assert_eq!(a.local_datetime, "2025-06-12T14:30:45.000000000");
    let b = parse_exif_time("2025:06:12 14:30:45", Some("+03:00"), Some("125")).unwrap();
    assert_eq!(b.utc.as_deref(), Some("2025-06-12T11:30:45.125Z"));
    for (raw, offset) in [
        ("2025:02:30 14:30:45", None),
        ("0000:00:00 00:00:00", None),
        ("2025:06:12 14:30:45", Some("+15:00")),
    ] {
        assert!(parse_exif_time(raw, offset, None).is_none());
    }
}
#[test]
fn s03_metadata_namespace_aliases_dtd_entities_depth_and_time_provenance() {
    let x=br#"<r xmlns:z="http://ns.adobe.com/exif/1.0/" xmlns:c="http://ns.adobe.com/tiff/1.0/" z:DateTimeOriginal="2025-06-12T14:30:45" c:Make="Synthetic"><z:DateTimeOriginal>2025-06-12T14:30:45</z:DateTimeOriginal></r>"#;
    let mut m = Metadata::default();
    extract_xmp(x, &mut m).unwrap();
    assert_eq!(m.camera_make.as_deref(), Some("Synthetic"));
    assert_eq!(m.xmp_taken_time.unwrap().utc, None);
    for bad in [
        b"<!DOCTYPE r SYSTEM 'https://invalid.example/x'><r/>".to_vec(),
        b"<!DOCTYPE r [<!ENTITY x 'payload'>]><r>&x;</r>".to_vec(),
        format!("{}{}", "<r>".repeat(65), "</r>".repeat(65)).into_bytes(),
    ] {
        assert!(extract_xmp(&bad, &mut Metadata::default()).is_err());
    }
    let huge = vec![b'x'; 8 * 1024 * 1024 + 1];
    assert!(extract_xmp(&huge, &mut Metadata::default()).is_err());
}
#[test]
fn s03_inputs_damaged_truncated_oversized_animation_raw_hdr_rejected() {
    let input = png(7, 3, &pixels(7, 3));
    for n in [0, 8, 20, input.len() - 1] {
        assert!(process(&input[..n], &options(OutputProfile::WebpExact)).is_err());
    }
    let animated = tagged_png(&input, b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
    assert!(matches!(
        process(&animated, &options(OutputProfile::Png)),
        Err(ImageError::MultiframeNotSupported)
    ));
    let hdr = tagged_png(&input, b"cICP", &[9, 16, 9, 0]);
    assert!(matches!(
        process(&hdr, &options(OutputProfile::Png)),
        Err(ImageError::ColorConversionRequired)
    ));
    let mut giant = input.clone();
    giant[16..20].copy_from_slice(&100000u32.to_be_bytes());
    assert!(matches!(
        process(&giant, &options(OutputProfile::Png)),
        Err(ImageError::LimitExceeded)
    ));
    let mut raw = exif(1);
    raw[10..12].copy_from_slice(&50706u16.to_le_bytes());
    assert!(matches!(
        process(&raw, &options(OutputProfile::Png)),
        Err(ImageError::RawNotSupported)
    ));
    let mut avif = Vec::new();
    avif.extend_from_slice(&24u32.to_be_bytes());
    avif.extend_from_slice(b"ftypavis\0\0\0\0avifavis");
    assert!(matches!(
        process(&avif, &options(OutputProfile::Png)),
        Err(ImageError::MultiframeNotSupported)
    ));
}
#[test]
fn s03_color_real_bundled_icc_rgb_gray_and_malformed_no_silent_strip() {
    let white = lcms2::CIExyY {
        x: 0.3127,
        y: 0.3290,
        Y: 1.0,
    };
    let primaries = lcms2::CIExyYTRIPLE {
        Red: lcms2::CIExyY {
            x: 0.64,
            y: 0.33,
            Y: 1.0,
        },
        Green: lcms2::CIExyY {
            x: 0.30,
            y: 0.60,
            Y: 1.0,
        },
        Blue: lcms2::CIExyY {
            x: 0.15,
            y: 0.06,
            Y: 1.0,
        },
    };
    let gamma = lcms2::ToneCurve::new(1.0);
    let profile = lcms2::Profile::new_rgb(&white, &primaries, &[&gamma, &gamma, &gamma]).unwrap();
    let icc = profile.icc().unwrap();
    let mut input = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut input);
    encoder.set_icc_profile(icc.clone()).unwrap();
    encoder
        .write_image(&[128, 128, 128, 73], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let c = process(&input, &options(OutputProfile::Png)).unwrap();
    let p = decode(c.bytes()).get_pixel(0, 0).0;
    assert!(
        (187..=189).contains(&p[0]) && (187..=189).contains(&p[1]) && (187..=189).contains(&p[2])
    );
    assert_eq!(p[3], 73);
    assert_eq!(c.report().color, sm_image::ColorProvenance::IccConverted);
    assert_eq!(c.report().metadata.icc_sha256, Some(sha256(&icc)));
    let gray = lcms2::Profile::new_gray(&white, &gamma)
        .unwrap()
        .icc()
        .unwrap();
    let mut input = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut input);
    encoder.set_icc_profile(gray).unwrap();
    encoder
        .write_image(&[128], 1, 1, image::ExtendedColorType::L8)
        .unwrap();
    let c = process(&input, &options(OutputProfile::Png)).unwrap();
    assert!((187..=189).contains(&decode(c.bytes()).get_pixel(0, 0)[0]));
    let mut malformed = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut malformed);
    encoder.set_icc_profile(vec![0; 128]).unwrap();
    encoder
        .write_image(&[128, 128, 128, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    assert!(matches!(
        process(&malformed, &options(OutputProfile::Png)),
        Err(ImageError::ColorConversionRequired)
    ));
}
#[test]
fn s03_multiframe_gif_webp_tiff_and_sdr16_refusals() {
    let frame = image::Frame::new(image::RgbaImage::from_raw(1, 1, vec![20, 40, 60, 255]).unwrap());
    let mut gif = Vec::new();
    let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
    encoder.encode_frames([frame.clone(), frame]).unwrap();
    drop(encoder);
    assert!(matches!(
        process(&gif, &options(OutputProfile::Png)),
        Err(ImageError::MultiframeNotSupported)
    ));
    let mut webp = b"RIFF".to_vec();
    webp.extend(16u32.to_le_bytes());
    webp.extend(b"WEBPANIM");
    webp.extend(4u32.to_le_bytes());
    webp.extend([0; 4]);
    assert!(matches!(
        process(&webp, &options(OutputProfile::Png)),
        Err(ImageError::MultiframeNotSupported)
    ));
    let mut tiff = exif(1);
    tiff[22..26].copy_from_slice(&8u32.to_le_bytes());
    assert!(matches!(
        process(&tiff, &options(OutputProfile::Png)),
        Err(ImageError::MultiframeNotSupported)
    ));
    let mut png16 = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png16)
        .write_image(
            &[0, 128, 0, 128, 0, 128],
            1,
            1,
            image::ExtendedColorType::Rgb16,
        )
        .unwrap();
    assert!(matches!(
        process(&png16, &options(OutputProfile::Png)),
        Err(ImageError::ColorConversionRequired)
    ));
}
#[test]
fn s03_tiff_required_compressions_and_real_multipage_rejection() {
    use tiff::encoder::{colortype::RGB8, Compression, DeflateLevel, TiffEncoder};
    let rgb = (0..63).map(|n| (n * 3) as u8).collect::<Vec<_>>();
    for compression in [
        Compression::Uncompressed,
        Compression::Lzw,
        Compression::Deflate(DeflateLevel::Balanced),
        Compression::Packbits,
    ] {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut encoder = TiffEncoder::new(&mut out)
            .unwrap()
            .with_compression(compression);
        encoder.write_image::<RGB8>(7, 3, &rgb).unwrap();
        let c = process(out.get_ref(), &options(OutputProfile::Png)).unwrap();
        let p = decode(c.bytes());
        for (a, b) in rgb
            .as_chunks::<3>()
            .0
            .iter()
            .zip(p.as_raw().as_chunks::<4>().0)
        {
            assert_eq!(a, &b[..3]);
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut out).unwrap();
    encoder.write_image::<RGB8>(7, 3, &rgb).unwrap();
    encoder.write_image::<RGB8>(7, 3, &rgb).unwrap();
    assert!(matches!(
        process(out.get_ref(), &options(OutputProfile::Png)),
        Err(ImageError::MultiframeNotSupported)
    ));
}
#[test]
fn s03_static_jpeg_bmp_gif_tiff_magic_not_suffix() {
    // No suffix is supplied to the engine. Known magic selects the actual decoder.
    let rgb = (0..63).map(|i| (i * 3) as u8).collect::<Vec<_>>();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode(&rgb, 7, 3, image::ExtendedColorType::Rgb8)
        .unwrap();
    let mut bmp = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut bmp)
        .encode(&rgb, 7, 3, image::ExtendedColorType::Rgb8)
        .unwrap();
    let mut gif = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut gif)
        .encode(&pixels(7, 3), 7, 3, image::ExtendedColorType::Rgba8)
        .unwrap();
    let mut tiff = std::io::Cursor::new(Vec::new());
    image::codecs::tiff::TiffEncoder::new(&mut tiff)
        .write_image(&rgb, 7, 3, image::ExtendedColorType::Rgb8)
        .unwrap();
    for input in [&jpeg, &bmp, &gif, tiff.get_ref()] {
        let c = process(input, &options(OutputProfile::WebpExact)).unwrap();
        assert_eq!(
            c.report().dimensions,
            Dimensions {
                width: 7,
                height: 3
            }
        );
    }
}
#[test]
fn s03_metadata_full_exif_camera_gps_and_time_conflict() {
    // Independent hand-authored little-endian TIFF fields; date/coordinates are
    // literal fixture facts, not expectations calculated by the production parser.
    fn field(tag: u16, kind: u16, count: u32, value: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(tag.to_le_bytes());
        b.extend(kind.to_le_bytes());
        b.extend(count.to_le_bytes());
        b.extend(value.to_le_bytes());
        b
    }
    let mut raw = b"II*\0\x08\0\0\0".to_vec();
    raw.extend(3u16.to_le_bytes());
    raw.extend(field(271, 2, 10, 50));
    raw.extend(field(34665, 4, 1, 60));
    raw.extend(field(34853, 4, 1, 90));
    raw.extend([0; 4]);
    raw.extend(b"Synthetic\0");
    raw.extend(2u16.to_le_bytes());
    raw.extend(field(36867, 2, 20, 156));
    raw.extend(field(36881, 2, 7, 176));
    raw.extend([0; 4]);
    raw.extend(5u16.to_le_bytes());
    raw.extend(field(1, 2, 2, u32::from_le_bytes(*b"S\0\0\0")));
    raw.extend(field(2, 5, 3, 183));
    raw.extend(field(3, 2, 2, u32::from_le_bytes(*b"W\0\0\0")));
    raw.extend(field(4, 5, 3, 207));
    raw.extend(field(5, 1, 1, 1));
    raw.extend([0; 4]);
    raw.extend(b"2025:06:12 14:30:45\0");
    raw.extend(b"+03:00\0");
    for n in [12u32, 30, 0, 45, 15, 0] {
        raw.extend(n.to_le_bytes());
        raw.extend(1u32.to_le_bytes());
    }
    let input = tagged_png(&png(1, 1, &[20, 40, 60, 255]), b"eXIf", &raw);
    let xmp=br#"<r xmlns:z="http://ns.adobe.com/exif/1.0/" z:DateTimeOriginal="2025-06-13T14:30:45+03:00"/>"#;
    let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    itxt.extend(xmp);
    let input = tagged_png(&input, b"iTXt", &itxt);
    let c = process(&input, &options(OutputProfile::Png)).unwrap();
    let m = &c.report().metadata;
    assert_eq!(m.camera_make.as_deref(), Some("Synthetic"));
    assert_eq!(
        m.taken_time.as_ref().unwrap().utc.as_deref(),
        Some("2025-06-12T11:30:45Z")
    );
    let gps = m.gps.as_ref().unwrap();
    assert_eq!(gps.latitude, -12.5);
    assert_eq!(gps.longitude, -45.25);
    assert!(m.warnings.contains(&MetadataWarning::TakenTimeConflict));
    assert!(!c.bytes().windows(9).any(|s| s == b"Synthetic"));
    for (lat_ref, lon_ref, lat, lon) in [
        (b'N', b'E', 12.5, 45.25),
        (b'N', b'W', 12.5, -45.25),
        (b'S', b'E', -12.5, 45.25),
    ] {
        let mut variant = raw.clone();
        variant[100] = lat_ref;
        variant[124] = lon_ref;
        let input = tagged_png(&png(1, 1, &[20, 40, 60, 255]), b"eXIf", &variant);
        let c = process(&input, &options(OutputProfile::Png)).unwrap();
        let gps = c.report().metadata.gps.as_ref().unwrap();
        assert_eq!((gps.latitude, gps.longitude), (lat, lon));
    }
    let mut zero = raw.clone();
    for p in [183, 191, 199, 207, 215, 223] {
        zero[p..p + 4].fill(0);
    }
    let input = tagged_png(&png(1, 1, &[20, 40, 60, 255]), b"eXIf", &zero);
    let c = process(&input, &options(OutputProfile::Png)).unwrap();
    let gps = c.report().metadata.gps.as_ref().unwrap();
    assert_eq!((gps.latitude, gps.longitude), (0.0, 0.0));
    let mut invalid = raw.clone();
    invalid[187..191].fill(0);
    let input = tagged_png(&png(1, 1, &[20, 40, 60, 255]), b"eXIf", &invalid);
    let c = process(&input, &options(OutputProfile::Png)).unwrap();
    assert!(c.report().metadata.gps.is_none());
    assert!(c
        .report()
        .metadata
        .warnings
        .contains(&MetadataWarning::InvalidGps));
    let mut nooffset = raw.clone();
    nooffset[74..76].copy_from_slice(&60000u16.to_le_bytes());
    let input = tagged_png(&png(1, 1, &[20, 40, 60, 255]), b"eXIf", &nooffset);
    let c = process(&input, &options(OutputProfile::Png)).unwrap();
    assert!(c
        .report()
        .metadata
        .taken_time
        .as_ref()
        .unwrap()
        .utc
        .is_none());
}
