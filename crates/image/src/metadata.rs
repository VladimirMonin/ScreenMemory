//! Bounded factual source observations, never instructions, URLs to fetch or export tags.
use crate::{inspect, Format, ImageError};
use exif::{In, Tag, Value};
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use serde::{Deserialize, Serialize};
use time::{PrimitiveDateTime, UtcOffset};

pub const MAX_PACKET_BYTES: usize = 8 * 1024 * 1024;
const MAX_TEXT: usize = 4096;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub orientation: Option<u16>,
    pub icc_sha256: Option<[u8; 32]>,
    pub applied_orientation: u16,
    pub container_orientation: Option<u16>,
    pub camera_make: Option<String>,
    pub camera_make_source: Option<MetadataSource>,
    pub camera_model: Option<String>,
    pub camera_model_source: Option<MetadataSource>,
    pub lens_model: Option<String>,
    pub lens_model_source: Option<MetadataSource>,
    pub software: Option<String>,
    pub software_source: Option<MetadataSource>,
    pub taken_time: Option<ObservedTime>,
    pub xmp_taken_time: Option<ObservedTime>,
    pub gps: Option<Geo>,
    pub warnings: Vec<MetadataWarning>,
    pub packet_bytes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObservedTime {
    pub raw: String,
    pub local_datetime: String,
    pub utc: Option<String>,
    pub offset_minutes: Option<i16>,
    pub source: TimeSource,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TimeSource {
    ExifDateTimeOriginal,
    XmpDateTimeOriginal,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MetadataSource {
    Exif,
    Xmp,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Geo {
    pub source: MetadataSource,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_metres: Option<f64>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MetadataWarning {
    InvalidTime,
    InvalidGps,
    TakenTimeConflict,
    TextBeyondLimit,
    TagsBeyondLimit,
    UnsupportedMetadata,
}
fn text(v: &[u8], m: &mut Metadata) -> Option<String> {
    let s = std::str::from_utf8(v).ok()?.trim_matches('\0').trim();
    if s.is_empty() {
        return None;
    }
    if s.chars().count() > MAX_TEXT {
        m.warnings.push(MetadataWarning::TextBeyondLimit);
        return None;
    }
    Some(
        s.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect(),
    )
}

pub fn extract(input: &[u8], format: Format) -> Result<Metadata, ImageError> {
    let mut exif_packet: Option<&[u8]> = None;
    let mut xmp_packet: Option<&[u8]> = None;
    let owned;
    let mut container_orientation = None;
    match format {
        Format::Png => {
            for (t, d) in inspect::png_chunks(input)? {
                if t == b"eXIf" && exif_packet.replace(d).is_some() {
                    return Err(ImageError::MetadataRejected);
                }
                if t == b"iTXt" && d.starts_with(b"XML:com.adobe.xmp\0") {
                    let d = &d[18..];
                    if d.len() < 2 || d[0] != 0 || d[1] != 0 {
                        return Err(ImageError::MetadataRejected);
                    }
                    let d = &d[2..];
                    let lang = d
                        .iter()
                        .position(|b| *b == 0)
                        .ok_or(ImageError::MetadataRejected)?;
                    let d = &d[lang + 1..];
                    let translated = d
                        .iter()
                        .position(|b| *b == 0)
                        .ok_or(ImageError::MetadataRejected)?;
                    if xmp_packet.replace(&d[translated + 1..]).is_some() {
                        return Err(ImageError::MetadataRejected);
                    }
                }
            }
        }
        Format::Jpeg => {
            for (marker, d) in inspect::jpeg_segments(input)? {
                if marker == 0xe1
                    && d.starts_with(b"Exif\0\0")
                    && exif_packet.replace(&d[6..]).is_some()
                {
                    return Err(ImageError::MetadataRejected);
                }
                if marker == 0xe1
                    && d.starts_with(b"http://ns.adobe.com/xap/1.0/\0")
                    && xmp_packet.replace(&d[29..]).is_some()
                {
                    return Err(ImageError::MetadataRejected);
                }
            }
        }
        Format::Webp => {
            for (t, d) in inspect::webp_chunks(input)? {
                if t == b"EXIF"
                    && exif_packet
                        .replace(d.strip_prefix(b"Exif\0\0").unwrap_or(d))
                        .is_some()
                {
                    return Err(ImageError::MetadataRejected);
                }
                if t == b"XMP " && xmp_packet.replace(d).is_some() {
                    return Err(ImageError::MetadataRejected);
                }
            }
        }
        Format::Tiff => {
            let n = inspect::tiff_extent(input, true)?;
            exif_packet = Some(&input[..n]);
        }
        Format::Avif => {
            owned = crate::avif::packets(input)?;
            container_orientation = owned.2;
            if !owned.0.is_empty() {
                exif_packet = Some(&owned.0);
            }
            if !owned.1.is_empty() {
                xmp_packet = Some(&owned.1);
            }
        }
        _ => {}
    }
    let packet_bytes = exif_packet.map_or(0, <[u8]>::len) + xmp_packet.map_or(0, <[u8]>::len);
    if packet_bytes > MAX_PACKET_BYTES {
        return Err(ImageError::MetadataRejected);
    }
    let mut m = Metadata {
        packet_bytes,
        container_orientation,
        ..Metadata::default()
    };
    if let Some(raw) = exif_packet {
        inspect::tiff_extent(raw, false)?;
        let exif = exif::Reader::new()
            .read_raw(raw.to_vec())
            .map_err(|_| ImageError::MetadataRejected)?;
        if exif.fields().len() > 2000 {
            return Err(ImageError::MetadataRejected);
        }
        let ascii = |tag| -> Option<&[u8]> {
            let field = exif.get_field(tag, In::PRIMARY)?;
            if let Value::Ascii(v) = &field.value {
                v.first().map(Vec::as_slice)
            } else {
                None
            }
        };
        let uint = |tag| -> Option<u32> { exif.get_field(tag, In::PRIMARY)?.value.get_uint(0) };
        m.camera_make = ascii(Tag::Make).and_then(|s| text(s, &mut m));
        m.camera_model = ascii(Tag::Model).and_then(|s| text(s, &mut m));
        m.lens_model = ascii(Tag::LensModel).and_then(|s| text(s, &mut m));
        m.software = ascii(Tag::Software).and_then(|s| text(s, &mut m));
        m.camera_make_source = m.camera_make.as_ref().map(|_| MetadataSource::Exif);
        m.camera_model_source = m.camera_model.as_ref().map(|_| MetadataSource::Exif);
        m.lens_model_source = m.lens_model.as_ref().map(|_| MetadataSource::Exif);
        m.software_source = m.software.as_ref().map(|_| MetadataSource::Exif);
        if let Some(o) = uint(Tag::Orientation) {
            if !(1..=8).contains(&o) {
                return Err(ImageError::GeometryConversionRequired);
            }
            m.orientation = Some(o as u16);
        }
        if let Some(raw) = ascii(Tag::DateTimeOriginal).and_then(|s| text(s, &mut m)) {
            let offset = ascii(Tag::OffsetTimeOriginal)
                .and_then(|s| std::str::from_utf8(s).ok())
                .map(|s| s.trim_matches('\0'));
            let sub = ascii(Tag::SubSecTimeOriginal)
                .and_then(|s| std::str::from_utf8(s).ok())
                .map(|s| s.trim_matches('\0'));
            m.taken_time = parse_exif_time(&raw, offset, sub);
            if m.taken_time.is_none() {
                m.warnings.push(MetadataWarning::InvalidTime);
            }
        }
        let rat = |tag| -> Option<&[exif::Rational]> {
            if let Value::Rational(v) = &exif.get_field(tag, In::PRIMARY)?.value {
                Some(v)
            } else {
                None
            }
        };
        let lat = rat(Tag::GPSLatitude);
        let lon = rat(Tag::GPSLongitude);
        if lat.is_some() || lon.is_some() {
            let coord =
                |v: Option<&[exif::Rational]>, r: Option<&[u8]>, latitude: bool| -> Option<f64> {
                    let v = v?;
                    if v.len() != 3 {
                        return None;
                    }
                    let a = rational(v[0])?;
                    let b = rational(v[1])?;
                    let c = rational(v[2])?;
                    if b >= 60. || c >= 60. {
                        return None;
                    }
                    let limit = if latitude { 90. } else { 180. };
                    let n = a + b / 60. + c / 3600.;
                    if n > limit {
                        return None;
                    }
                    let sign = match r?.first()? {
                        b'N' if latitude => 1.,
                        b'S' if latitude => -1.,
                        b'E' if !latitude => 1.,
                        b'W' if !latitude => -1.,
                        _ => return None,
                    };
                    Some(n * sign)
                };
            if let (Some(latitude), Some(longitude)) = (
                coord(lat, ascii(Tag::GPSLatitudeRef), true),
                coord(lon, ascii(Tag::GPSLongitudeRef), false),
            ) {
                let altitude = rat(Tag::GPSAltitude)
                    .and_then(|v| v.first())
                    .and_then(|v| rational(*v));
                let altitude_metres = match (altitude, uint(Tag::GPSAltitudeRef)) {
                    (Some(a), Some(0)) => Some(a),
                    (Some(a), Some(1)) => Some(-a),
                    _ => None,
                };
                m.gps = Some(Geo {
                    source: MetadataSource::Exif,
                    latitude,
                    longitude,
                    altitude_metres,
                });
            } else {
                m.warnings.push(MetadataWarning::InvalidGps);
            }
        }
    }
    if let Some(raw) = xmp_packet {
        extract_xmp(raw, &mut m)?;
    }
    if let (Some(a), Some(b)) = (&m.taken_time, &m.xmp_taken_time) {
        if a.local_datetime != b.local_datetime
            || (a.utc.is_some() && b.utc.is_some() && a.utc != b.utc)
        {
            m.warnings.push(MetadataWarning::TakenTimeConflict);
        }
    }
    if m.taken_time.is_none() {
        m.taken_time = m.xmp_taken_time.clone();
    }
    Ok(m)
}
fn rational(r: exif::Rational) -> Option<f64> {
    if r.denom == 0 {
        None
    } else {
        let v = f64::from(r.num) / f64::from(r.denom);
        v.is_finite().then_some(v)
    }
}
pub fn parse_exif_time(raw: &str, offset: Option<&str>, sub: Option<&str>) -> Option<ObservedTime> {
    let f =
        time::format_description::parse("[year]:[month]:[day] [hour]:[minute]:[second]").ok()?;
    let mut date = PrimitiveDateTime::parse(raw, &f).ok()?;
    if let Some(sub) = sub {
        if sub.is_empty() || sub.len() > 9 || !sub.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let nanos = sub.parse::<u32>().ok()? * 10u32.pow(9 - sub.len() as u32);
        date = date.replace_nanosecond(nanos).ok()?;
    }
    let offset = if let Some(o) = offset {
        let f =
            time::format_description::parse("[offset_hour sign:mandatory]:[offset_minute]").ok()?;
        let parsed = UtcOffset::parse(o, &f).ok()?;
        if parsed.whole_minutes().abs() > 14 * 60 {
            return None;
        }
        Some(parsed)
    } else {
        None
    };
    observation(raw, date, offset, TimeSource::ExifDateTimeOriginal)
}
fn observation(
    raw: &str,
    date: PrimitiveDateTime,
    offset: Option<UtcOffset>,
    source: TimeSource,
) -> Option<ObservedTime> {
    let f = time::format_description::parse(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]",
    )
    .ok()?;
    Some(ObservedTime {
        raw: raw.into(),
        local_datetime: date.format(&f).ok()?,
        utc: offset.and_then(|o| {
            date.assume_offset(o)
                .to_offset(UtcOffset::UTC)
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        }),
        offset_minutes: offset.map(UtcOffset::whole_minutes),
        source,
    })
}
fn xmp_time(raw: &str) -> Option<ObservedTime> {
    if let Ok(date) =
        time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
    {
        return observation(
            raw,
            PrimitiveDateTime::new(date.date(), date.time()),
            Some(date.offset()),
            TimeSource::XmpDateTimeOriginal,
        );
    }
    let f =
        time::format_description::parse("[year]-[month]-[day]T[hour]:[minute]:[second]").ok()?;
    observation(
        raw,
        PrimitiveDateTime::parse(raw, &f).ok()?,
        None,
        TimeSource::XmpDateTimeOriginal,
    )
}
fn known(ns: &[u8], name: &[u8]) -> Option<u8> {
    match (ns, name) {
        (b"http://ns.adobe.com/exif/1.0/", b"DateTimeOriginal") => Some(0),
        (b"http://ns.adobe.com/tiff/1.0/", b"Make") => Some(1),
        (b"http://ns.adobe.com/tiff/1.0/", b"Model") => Some(2),
        (b"http://ns.adobe.com/xap/1.0/", b"CreatorTool") => Some(3),
        _ => None,
    }
}
fn apply(key: u8, raw: &[u8], m: &mut Metadata) {
    if let Some(s) = text(raw, m) {
        match key {
            0 => {
                m.xmp_taken_time = xmp_time(&s);
                if m.xmp_taken_time.is_none() {
                    m.warnings.push(MetadataWarning::InvalidTime);
                }
            }
            1 if m.camera_make.is_none() => {
                m.camera_make = Some(s);
                m.camera_make_source = Some(MetadataSource::Xmp);
            }
            2 if m.camera_model.is_none() => {
                m.camera_model = Some(s);
                m.camera_model_source = Some(MetadataSource::Xmp);
            }
            3 if m.software.is_none() => {
                m.software = Some(s);
                m.software_source = Some(MetadataSource::Xmp);
            }
            _ => {}
        }
    }
}
/// Namespace-aware streaming parser. No DTD/entity/URL resolution or filesystem API.
pub fn extract_xmp(raw: &[u8], m: &mut Metadata) -> Result<(), ImageError> {
    if raw.len() > MAX_PACKET_BYTES {
        return Err(ImageError::MetadataRejected);
    }
    let mut reader = NsReader::from_reader(raw);
    let mut stack = Vec::new();
    let mut events = 0;
    let mut tags = 0;
    loop {
        events += 1;
        if events > 100000 {
            return Err(ImageError::MetadataRejected);
        }
        let (ns, event) = reader
            .read_resolved_event()
            .map_err(|_| ImageError::MetadataRejected)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                tags += 1;
                if tags > 2000 || stack.len() >= 64 {
                    return Err(ImageError::MetadataRejected);
                }
                let key = if let ResolveResult::Bound(ns) = ns {
                    known(ns.as_ref().as_bytes(), e.local_name().as_ref().as_bytes())
                } else {
                    None
                };
                for a in e.attributes() {
                    let a = a.map_err(|_| ImageError::MetadataRejected)?;
                    if a.value.len() > MAX_TEXT * 4 {
                        return Err(ImageError::MetadataRejected);
                    }
                    let (ns, local) = reader.resolver().resolve_attribute(a.key);
                    if let ResolveResult::Bound(ns) = ns {
                        if let Some(key) = known(ns.as_ref().as_bytes(), local.as_ref().as_bytes())
                        {
                            apply(key, a.value.as_bytes(), m);
                        }
                    }
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(key);
                }
            }
            Event::End(_) => {
                if stack.pop().is_none() {
                    return Err(ImageError::MetadataRejected);
                }
            }
            Event::Text(e) => {
                if let Some(Some(key)) = stack.last() {
                    apply(*key, e.as_ref().as_bytes(), m);
                }
            }
            Event::DocType(_) | Event::GeneralRef(_) => return Err(ImageError::MetadataRejected),
            Event::Eof => {
                if !stack.is_empty() {
                    return Err(ImageError::MetadataRejected);
                }
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
