//! Source-aware views; imported records never fabricate a CaptureContext.
use crate::images::Cursor;
use rusqlite::{params, Connection};
use sm_application::storage::{CaptureContext, RecordPageQuery, StorageError};
use sm_domain::{AssetId, RecordId};
use sm_protocol::application::{LibraryRecordEntry, LibraryRecordsPage};

pub(crate) fn page(
    db: &Connection,
    query: RecordPageQuery,
) -> Result<LibraryRecordsPage, StorageError> {
    let error = |_| StorageError::IntegrityFailed;
    let library: String = db
        .query_row(
            "SELECT library_id FROM library_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(error)?;
    let cursor = query
        .cursor()
        .map(|c| Cursor::parse(c, &library))
        .transpose()?;
    let upper = match &cursor {
        Some(c) => c.snapshot_upper,
        None => db
            .query_row("SELECT COALESCE(MAX(record_pk),0) FROM records", [], |r| {
                r.get(0)
            })
            .map_err(error)?,
    };
    let before = cursor.map_or(i64::MAX, |c| c.before);
    let mut statement = db.prepare("SELECT r.record_pk,r.id,r.created_at,r.source_kind,r.capture_context,a.id,a.size_bytes,p.report_json FROM records r LEFT JOIN assets a ON a.record_id=r.id AND a.asset_type='original' LEFT JOIN asset_processing p ON p.asset_id=a.id WHERE r.record_pk<=?1 AND r.record_pk<?2 ORDER BY r.record_pk DESC LIMIT ?3").map_err(error)?;
    let rows = statement
        .query_map(params![upper, before, i64::from(query.limit()) + 1], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<u64>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    let more = rows.len() > usize::from(query.limit());
    let mut items = Vec::new();
    let mut last = upper;
    for (pk, id, created, kind, context, asset, size, report) in
        rows.into_iter().take(usize::from(query.limit()))
    {
        let (title, target, width, height) = if kind == "capture" {
            let c =
                CaptureContext::from_json(&context).map_err(|_| StorageError::IntegrityFailed)?;
            (
                c.title,
                c.target.as_str().to_owned(),
                c.bounds.width,
                c.bounds.height,
            )
        } else if kind == "imported_image" {
            let report: sm_image::Report =
                serde_json::from_str(report.as_deref().ok_or(StorageError::IntegrityFailed)?)
                    .map_err(|_| StorageError::IntegrityFailed)?;
            (
                "Импортированное изображение".into(),
                kind,
                report.dimensions.width,
                report.dimensions.height,
            )
        } else {
            return Err(StorageError::IntegrityFailed);
        };
        items.push(LibraryRecordEntry {
            id: RecordId::new(
                uuid::Uuid::parse_str(&id).map_err(|_| StorageError::IntegrityFailed)?,
            ),
            created_at: created,
            title,
            target,
            width,
            height,
            byte_size: size.unwrap_or(0).to_string(),
            preview_asset_id: asset
                .map(|a| {
                    uuid::Uuid::parse_str(&a)
                        .map(AssetId::new)
                        .map_err(|_| StorageError::IntegrityFailed)
                })
                .transpose()?,
        });
        last = pk;
    }
    Ok(LibraryRecordsPage {
        items,
        next_cursor: more.then(|| {
            Cursor {
                snapshot_upper: upper,
                before: last,
            }
            .encode(&library)
        }),
    })
}
