use sha2::{Digest, Sha256};
use sm_domain::AssetId;
use sm_gui::{parse_preview_asset_id, PreviewAssembler, MAX_PREVIEW_BYTES};
use sm_protocol::application::LibraryAssetChunk;
use uuid::Uuid;

fn chunk(
    id: AssetId,
    bytes: &[u8],
    offset: u32,
    total: usize,
    eof: bool,
    sha: &str,
) -> LibraryAssetChunk {
    LibraryAssetChunk {
        asset_id: id,
        offset,
        next_offset: offset + bytes.len() as u32,
        eof,
        total_size: total.to_string(),
        sha256: sha.to_owned(),
        mime: "image/webp".into(),
        data_hex: bytes.iter().map(|b| format!("{b:02x}")).collect(),
    }
}

#[test]
fn preview_checks_uuid_v4_canonical_only() {
    let valid = Uuid::new_v4().to_string();
    assert!(parse_preview_asset_id(&valid).is_some());
    assert!(parse_preview_asset_id(&valid.to_uppercase()).is_none());
    assert!(parse_preview_asset_id("../assets/example.webp").is_none());
    assert!(parse_preview_asset_id("01900000-0000-7000-8000-000000000001").is_none());
    assert!(parse_preview_asset_id("00000000000000000000000000000001").is_none());
}

#[test]
fn preview_stream_validates_hash_offsets_size_mime_and_asset() {
    let id = AssetId::new(Uuid::new_v4());
    let other = AssetId::new(Uuid::new_v4());
    let data = b"RIFFsyntheticWEBP";
    let sha = format!("{:x}", Sha256::digest(data));
    let first = chunk(id, &data[..8], 0, data.len(), false, &sha);
    let second = chunk(id, &data[8..], 8, data.len(), true, &sha);
    let mut stream = PreviewAssembler::new(id);
    assert!(stream.push(first.clone()).unwrap().is_none());
    let image = stream.push(second.clone()).unwrap().unwrap();
    assert_eq!(
        image.data_hex,
        data.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    assert_eq!(image.mime, "image/webp");
    assert!(image.safe_code.is_none());

    let mut wrong = second.clone();
    wrong.offset = 7;
    assert_eq!(
        PreviewAssembler::new(id).push(wrong).err(),
        Some("PREVIEW_UNAVAILABLE")
    );
    let mut wrong = first.clone();
    wrong.asset_id = other;
    assert_eq!(
        PreviewAssembler::new(id).push(wrong).err(),
        Some("PREVIEW_UNAVAILABLE")
    );
    let mut wrong = first.clone();
    wrong.mime = "text/html".into();
    assert_eq!(
        PreviewAssembler::new(id).push(wrong).err(),
        Some("PREVIEW_UNAVAILABLE")
    );
    let mut wrong = first.clone();
    wrong.sha256 = "not-a-sha".into();
    assert_eq!(
        PreviewAssembler::new(id).push(wrong).err(),
        Some("PREVIEW_UNAVAILABLE")
    );
    let mut wrong = first.clone();
    wrong.total_size = (MAX_PREVIEW_BYTES + 1).to_string();
    assert_eq!(
        PreviewAssembler::new(id).push(wrong).err(),
        Some("PREVIEW_TOO_LARGE")
    );
    let mut wrong = second.clone();
    wrong.data_hex = "zz".into();
    assert_eq!(
        PreviewAssembler::new(id).push(wrong).err(),
        Some("PREVIEW_UNAVAILABLE")
    );
    let mut wrong = first.clone();
    wrong.eof = true;
    assert_eq!(
        PreviewAssembler::new(id).push(wrong).err(),
        Some("PREVIEW_UNAVAILABLE")
    );
    let mut stream = PreviewAssembler::new(id);
    stream.push(first).unwrap();
    let mut wrong = second.clone();
    wrong.sha256 = "0".repeat(64);
    assert_eq!(stream.push(wrong).err(), Some("PREVIEW_UNAVAILABLE"));
    let mut wrong = second;
    wrong.data_hex.replace_range(..2, "00");
    let mut stream = PreviewAssembler::new(id);
    stream
        .push(chunk(id, &data[..8], 0, data.len(), false, &sha))
        .unwrap();
    assert_eq!(stream.push(wrong).err(), Some("PREVIEW_UNAVAILABLE"));
}
