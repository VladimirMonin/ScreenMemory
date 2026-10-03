use crate::{Dimensions, ImageError, Pixels};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResizeLimits {
    pub max_side: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizePlan {
    pub output: Dimensions,
    pub numerator: u64,
    pub denominator: u64,
}
/// Integer-only common fraction, floor rounding, positive minimum, never upscale.
pub fn fit_inside(source: Dimensions, limits: ResizeLimits) -> Result<ResizePlan, ImageError> {
    if source.width == 0 || source.height == 0 {
        return Err(ImageError::InvalidOptions);
    }
    let (mut n, mut d) = (1u64, 1u64);
    for (bound, axis) in [
        (limits.max_side, source.width.max(source.height)),
        (limits.max_width, source.width),
        (limits.max_height, source.height),
    ] {
        if let Some(bound) = bound {
            if bound == 0 {
                return Err(ImageError::InvalidOptions);
            }
            if u128::from(bound) * u128::from(d) < u128::from(n) * u128::from(axis) {
                n = u64::from(bound);
                d = u64::from(axis);
            }
        }
    }
    let (mut a, mut b) = (n, d);
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    n /= a;
    d /= a;
    let scale = |axis| ((u64::from(axis) * n / d).max(1)) as u32;
    Ok(ResizePlan {
        output: Dimensions {
            width: scale(source.width),
            height: scale(source.height),
        },
        numerator: n,
        denominator: d,
    })
}
pub(crate) fn orient(p: Pixels, o: u16) -> Result<Pixels, ImageError> {
    if !(1..=8).contains(&o) {
        return Err(ImageError::GeometryConversionRequired);
    }
    if o == 1 {
        return Ok(p);
    }
    p.dimensions.working_budget(8)?;
    let (w, h) = (p.dimensions.width, p.dimensions.height);
    let dimensions = if o >= 5 {
        Dimensions {
            width: h,
            height: w,
        }
    } else {
        p.dimensions
    };
    let mut rgba = vec![0; p.rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let (tx, ty) = match o {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (h - 1 - y, x),
                7 => (h - 1 - y, w - 1 - x),
                8 => (y, w - 1 - x),
                _ => (x, y),
            };
            let src = ((y as usize * w as usize) + x as usize) * 4;
            let dst = ((ty as usize * dimensions.width as usize) + tx as usize) * 4;
            rgba[dst..dst + 4].copy_from_slice(&p.rgba[src..src + 4]);
        }
    }
    Pixels::new(dimensions, rgba)
}
fn linear(v: u8) -> f32 {
    let v = f32::from(v) / 255.;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn srgb(v: f32) -> u8 {
    let v = v.clamp(0., 1.);
    let v = if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    };
    (v * 255.).round().clamp(0., 255.) as u8
}
pub(crate) fn resize(p: Pixels, target: Dimensions) -> Result<Pixels, ImageError> {
    if target == p.dimensions {
        return Ok(p);
    }
    // RGBA input+linear source+filter scratch+output. Reject rather than shrink a
    // canonical or falsely claim all 96MP fit a float-buffer implementation.
    p.dimensions.working_budget(56)?;
    let mut floats = Vec::with_capacity(p.rgba.len());
    for px in p.rgba.as_chunks::<4>().0 {
        let a = f32::from(px[3]) / 255.;
        floats.extend_from_slice(&[linear(px[0]) * a, linear(px[1]) * a, linear(px[2]) * a, a]);
    }
    let source = image::ImageBuffer::<image::Rgba<f32>, Vec<f32>>::from_raw(
        p.dimensions.width,
        p.dimensions.height,
        floats,
    )
    .ok_or(ImageError::Malformed)?;
    let filtered = image::imageops::resize(
        &source,
        target.width,
        target.height,
        image::imageops::FilterType::Lanczos3,
    );
    let mut rgba = Vec::with_capacity(target.byte_len()?);
    for px in filtered.pixels() {
        let a = px[3].clamp(0., 1.);
        let rgb = if a > 1e-6 {
            [srgb(px[0] / a), srgb(px[1] / a), srgb(px[2] / a)]
        } else {
            [0; 3]
        };
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], (a * 255.).round() as u8]);
    }
    Pixels::new(target, rgba)
}
pub(crate) fn composite_rgb(p: &[u8], bg: [u8; 3]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(p.len() / 4 * 3);
    for px in p.as_chunks::<4>().0 {
        let a = f32::from(px[3]) / 255.;
        for c in 0..3 {
            rgb.push(srgb(linear(px[c]) * a + linear(bg[c]) * (1. - a)));
        }
    }
    rgb
}
