use crate::{Dimensions, Format, ImageError};
type Chunks<'a> = Vec<(&'a [u8], &'a [u8])>;

pub(crate) fn classify(b: &[u8]) -> Result<Format, ImageError> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(Format::Png)
    } else if b.starts_with(b"\xff\xd8\xff") {
        Ok(Format::Jpeg)
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Ok(Format::Webp)
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Ok(Format::Gif)
    } else if b.starts_with(b"BM") {
        Ok(Format::Bmp)
    } else if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        Ok(Format::Tiff)
    } else if b.get(4..8) == Some(b"ftyp") {
        let n = be32(b, 0)? as usize;
        let ftyp = b.get(8..n).ok_or(ImageError::Malformed)?;
        if ftyp.len() < 8 || !(ftyp.len() - 8).is_multiple_of(4) {
            return Err(ImageError::Malformed);
        }
        let brands = std::iter::once(&ftyp[0..4])
            .chain(ftyp[8..].as_chunks::<4>().0.iter().map(|a| a.as_slice()));
        let mut avif = false;
        for brand in brands {
            if brand == b"avis" {
                return Err(ImageError::MultiframeNotSupported);
            }
            if brand == b"avif" {
                avif = true;
            }
        }
        if avif {
            Ok(Format::Avif)
        } else {
            Err(ImageError::UnsupportedFormat)
        }
    } else {
        Err(ImageError::UnsupportedFormat)
    }
}
pub(crate) fn be32(b: &[u8], p: usize) -> Result<u32, ImageError> {
    Ok(u32::from_be_bytes(
        b.get(p..p + 4)
            .ok_or(ImageError::Malformed)?
            .try_into()
            .map_err(|_| ImageError::Malformed)?,
    ))
}
pub(crate) fn png_chunks(b: &[u8]) -> Result<Chunks<'_>, ImageError> {
    let mut p = 8;
    let mut out = Vec::new();
    let mut end = false;
    while p < b.len() {
        if out.len() >= 10000 {
            return Err(ImageError::LimitExceeded);
        }
        let n = be32(b, p)? as usize;
        let t = b.get(p + 4..p + 8).ok_or(ImageError::Malformed)?;
        let data = b.get(p + 8..p + 8 + n).ok_or(ImageError::Malformed)?;
        p = p.checked_add(n + 12).ok_or(ImageError::Malformed)?;
        if p > b.len() {
            return Err(ImageError::Malformed);
        }
        out.push((t, data));
        if t == b"IEND" {
            if !data.is_empty() || p != b.len() {
                return Err(ImageError::Malformed);
            }
            end = true;
            break;
        }
    }
    if !end {
        return Err(ImageError::Malformed);
    }
    Ok(out)
}
pub(crate) fn webp_chunks(b: &[u8]) -> Result<Chunks<'_>, ImageError> {
    let len = b.get(4..8).ok_or(ImageError::Malformed)?;
    let len = u32::from_le_bytes(len.try_into().map_err(|_| ImageError::Malformed)?) as usize;
    if len.checked_add(8) != Some(b.len()) {
        return Err(ImageError::Malformed);
    }
    let mut p = 12;
    let mut out = Vec::new();
    while p < b.len() {
        if out.len() >= 10000 {
            return Err(ImageError::LimitExceeded);
        }
        let t = b.get(p..p + 4).ok_or(ImageError::Malformed)?;
        let len = b.get(p + 4..p + 8).ok_or(ImageError::Malformed)?;
        let n = u32::from_le_bytes(len.try_into().map_err(|_| ImageError::Malformed)?) as usize;
        let data = b.get(p + 8..p + 8 + n).ok_or(ImageError::Malformed)?;
        p = p
            .checked_add(8 + n + (n % 2))
            .ok_or(ImageError::Malformed)?;
        if p > b.len() {
            return Err(ImageError::Malformed);
        }
        out.push((t, data));
    }
    Ok(out)
}
pub(crate) fn jpeg_segments(b: &[u8]) -> Result<Vec<(u8, &[u8])>, ImageError> {
    let mut p = 2;
    let mut out = Vec::new();
    loop {
        if out.len() >= 10000 {
            return Err(ImageError::LimitExceeded);
        }
        if b.get(p) != Some(&255) {
            return Err(ImageError::Malformed);
        }
        while b.get(p) == Some(&255) {
            p += 1;
        }
        let marker = *b.get(p).ok_or(ImageError::Malformed)?;
        p += 1;
        if marker == 0xd9 {
            break;
        }
        if marker == 0xda {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let n = u16::from_be_bytes(
            b.get(p..p + 2)
                .ok_or(ImageError::Malformed)?
                .try_into()
                .map_err(|_| ImageError::Malformed)?,
        ) as usize;
        if n < 2 {
            return Err(ImageError::Malformed);
        }
        let data = b.get(p + 2..p + n).ok_or(ImageError::Malformed)?;
        out.push((marker, data));
        p += n;
    }
    Ok(out)
}
pub(crate) fn check_container(b: &[u8], format: Format) -> Result<(), ImageError> {
    for marker in [
        b"urn:com:apple:photo:2020:aux:hdrgainmap".as_slice(),
        b"http://ns.adobe.com/hdr-gain-map/",
        b"urn:iso:std:iso:ts:21496",
    ] {
        if b.windows(marker.len()).any(|v| v == marker) {
            return Err(ImageError::ColorConversionRequired);
        }
    }
    match format {
        Format::Png => {
            for (t, d) in png_chunks(b)? {
                if t == b"acTL" || t == b"fcTL" || t == b"fdAT" {
                    return Err(ImageError::MultiframeNotSupported);
                }
                if t == b"IHDR" {
                    if d.len() != 13 {
                        return Err(ImageError::Malformed);
                    }
                    Dimensions {
                        width: be32(d, 0)?,
                        height: be32(d, 4)?,
                    }
                    .working_budget(12)?;
                    if d[8] != 8 && d[8] != 1 && d[8] != 2 && d[8] != 4 {
                        return Err(ImageError::ColorConversionRequired);
                    }
                }
                if t == b"cICP" && d != [1, 13, 0, 1] {
                    return Err(ImageError::ColorConversionRequired);
                }
                if t == b"gAMA" && be32(d, 0)? != 45455 {
                    return Err(ImageError::ColorConversionRequired);
                }
                // Unknown chromaticities are not assumed sRGB.
                if t == b"cHRM"
                    && d != [
                        0, 0, 122, 38, 0, 0, 128, 132, 0, 0, 250, 0, 0, 0, 128, 232, 0, 0, 117, 48,
                        0, 0, 234, 96, 0, 0, 58, 152, 0, 0, 23, 112,
                    ]
                {
                    return Err(ImageError::ColorConversionRequired);
                }
            }
        }
        Format::Webp => {
            for (t, d) in webp_chunks(b)? {
                if t == b"ANIM"
                    || t == b"ANMF"
                    || (t == b"VP8X" && d.first().is_some_and(|v| v & 2 != 0))
                {
                    return Err(ImageError::MultiframeNotSupported);
                }
                if t == b"ICCP" && d.len() > 4 * 1024 * 1024 {
                    return Err(ImageError::MetadataRejected);
                }
            }
        }
        Format::Jpeg => {
            for (m, d) in jpeg_segments(b)? {
                if matches!(m, 0xc0..=0xc2) {
                    if d.len() < 6 {
                        return Err(ImageError::Malformed);
                    }
                    if d[0] != 8 || d[5] == 4 {
                        return Err(ImageError::ColorConversionRequired);
                    }
                }
                if m == 0xe2
                    && (d.starts_with(b"MPF\0") || d.starts_with(b"urn:iso:std:iso:ts:21496"))
                {
                    return Err(ImageError::ColorConversionRequired);
                }
            }
        }
        Format::Gif => check_gif(b)?,
        Format::Tiff => {
            tiff_extent(b, true)?;
        }
        Format::Avif => {
            crate::avif::packets(b)?;
        }
        Format::Bmp => {
            let header = u32::from_le_bytes(
                b.get(14..18)
                    .ok_or(ImageError::Malformed)?
                    .try_into()
                    .map_err(|_| ImageError::Malformed)?,
            );
            if header >= 108 {
                let color = u32::from_le_bytes(
                    b.get(70..74)
                        .ok_or(ImageError::Malformed)?
                        .try_into()
                        .map_err(|_| ImageError::Malformed)?,
                );
                if color != 0x73524742 {
                    return Err(ImageError::ColorConversionRequired);
                }
            }
        }
    }
    Ok(())
}
fn check_gif(b: &[u8]) -> Result<(), ImageError> {
    let header = b.get(..13).ok_or(ImageError::Malformed)?;
    let w = u16::from_le_bytes([header[6], header[7]]) as u32;
    let h = u16::from_le_bytes([header[8], header[9]]) as u32;
    Dimensions {
        width: w,
        height: h,
    }
    .working_budget(12)?;
    let mut p = 13;
    if header[10] & 128 != 0 {
        p += 3 * (1usize << ((header[10] & 7) + 1));
    }
    let mut count = 0;
    loop {
        let tag = *b.get(p).ok_or(ImageError::Malformed)?;
        p += 1;
        match tag {
            0x3b => {
                if p != b.len() || count != 1 {
                    return Err(ImageError::Malformed);
                }
                return Ok(());
            }
            0x21 => {
                let label = *b.get(p).ok_or(ImageError::Malformed)?;
                p += 1;
                if label == 0xff
                    && b.get(p) == Some(&11)
                    && b.get(p + 1..p + 12) == Some(b"ICCRGBG1012")
                {
                    return Err(ImageError::ColorConversionRequired);
                }
            }
            0x2c => {
                count += 1;
                if count > 1 {
                    return Err(ImageError::MultiframeNotSupported);
                }
                let d = b.get(p..p + 9).ok_or(ImageError::Malformed)?;
                p += 9;
                if d[8] & 128 != 0 {
                    p += 3 * (1usize << ((d[8] & 7) + 1));
                }
                p += 1;
            }
            _ => return Err(ImageError::Malformed),
        }
        loop {
            let n = *b.get(p).ok_or(ImageError::Malformed)? as usize;
            p += 1;
            if n == 0 {
                break;
            }
            p = p.checked_add(n).ok_or(ImageError::Malformed)?;
            if p > b.len() {
                return Err(ImageError::Malformed);
            }
        }
    }
}
/// Validate classic TIFF IFD pointers/tag payloads before EXIF allocations. Never
/// walk pixel data; strip/tile offset arrays are metadata, not their pixel targets.
pub(crate) fn tiff_extent(b: &[u8], image: bool) -> Result<usize, ImageError> {
    let le = b.starts_with(b"II");
    let u16at = |p| -> Result<u16, ImageError> {
        let a: [u8; 2] = b
            .get(p..p + 2)
            .ok_or(ImageError::Malformed)?
            .try_into()
            .map_err(|_| ImageError::Malformed)?;
        Ok(if le {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        })
    };
    let u32at = |p| -> Result<u32, ImageError> {
        let a: [u8; 4] = b
            .get(p..p + 4)
            .ok_or(ImageError::Malformed)?
            .try_into()
            .map_err(|_| ImageError::Malformed)?;
        Ok(if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    };
    if u16at(2)? != 42 {
        return Err(ImageError::UnsupportedFormat);
    }
    let first = u32at(4)? as usize;
    let mut stack = vec![first];
    let mut seen = Vec::new();
    let mut extent = 8;
    let mut tags = 0;
    while let Some(p) = stack.pop() {
        if seen.contains(&p) || seen.len() >= 16 {
            return Err(ImageError::MetadataRejected);
        }
        seen.push(p);
        let n = u16at(p)? as usize;
        tags += n;
        if tags > 2000 {
            return Err(ImageError::MetadataRejected);
        }
        let end = p.checked_add(2 + n * 12 + 4).ok_or(ImageError::Malformed)?;
        if end > b.len() {
            return Err(ImageError::Malformed);
        }
        extent = extent.max(end);
        let next = u32at(end - 4)?;
        if next != 0 {
            if image {
                return Err(ImageError::MultiframeNotSupported);
            }
            return Err(ImageError::MetadataRejected);
        }
        for i in 0..n {
            let q = p + 2 + i * 12;
            let tag = u16at(q)?;
            let kind = u16at(q + 2)?;
            let count = u32at(q + 4)? as usize;
            if matches!(tag, 33421 | 33422 | 50706 | 50707 | 50829 | 50964) {
                return Err(ImageError::RawNotSupported);
            }
            let unit = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => return Err(ImageError::MetadataRejected),
            };
            let len = count
                .checked_mul(unit)
                .ok_or(ImageError::MetadataRejected)?;
            let off = if len <= 4 {
                q + 8
            } else {
                u32at(q + 8)? as usize
            };
            if b.get(off..off + len).is_none() {
                return Err(ImageError::Malformed);
            }
            extent = extent.max(off + len);
            if matches!(tag, 34665 | 34853 | 40965) {
                if kind != 4 || count != 1 {
                    return Err(ImageError::MetadataRejected);
                }
                stack.push(u32at(off)? as usize);
            }
            if image && p == first && kind == 3 && count == 1 {
                let value = u16at(off)?;
                if tag == 259 && !matches!(value, 1 | 5 | 8 | 32946 | 32773) {
                    return Err(ImageError::UnsupportedFormat);
                }
                if tag == 262 && !matches!(value, 0..=3) {
                    return Err(ImageError::ColorConversionRequired);
                }
                if tag == 258 && value != 8 {
                    return Err(ImageError::ColorConversionRequired);
                }
            }
        }
    }
    if extent > crate::metadata::MAX_PACKET_BYTES {
        return Err(ImageError::MetadataRejected);
    }
    Ok(extent)
}
