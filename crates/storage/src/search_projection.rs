//! Synchronous SQLite transaction projection of the accepted manual effective source.
use crate::knowledge::{KnowledgeError, SearchProjectionTx};
use rusqlite::{params, OptionalExtension, Transaction};
use sm_protocol::knowledge::{CardView, SqlRevision};
use std::collections::BTreeSet;

pub const FINGERPRINT: &str = "seven_fields_6_3_1_2_2_2_1_win_nfc_unicase2.9_v1";
#[link(name = "Normaliz")]
unsafe extern "system" {
    fn NormalizeString(
        form: i32,
        source: *const u16,
        source_len: i32,
        destination: *mut u16,
        destination_len: i32,
    ) -> i32;
}
/// Same exact normalizer for query and source, NFC then full pinned unicase2.9 casefold.
pub fn normalize(s: &str) -> Result<String, KnowledgeError> {
    let input: Vec<u16> = s.encode_utf16().collect();
    let mut output = vec![0u16; input.len() * 3 + 16];
    let n = unsafe {
        NormalizeString(
            1,
            input.as_ptr(),
            input.len() as i32,
            output.as_mut_ptr(),
            output.len() as i32,
        )
    };
    if n <= 0 {
        return Err(KnowledgeError::Invalid);
    }
    output.truncate(n as usize);
    Ok(
        unicase::UniCase::unicode(
            String::from_utf16(&output).map_err(|_| KnowledgeError::Invalid)?,
        )
        .to_folded_case(),
    )
}
/// Maximal atoms preserve technical punctuation. Sentence-final dot/colon/hyphen
/// is stripped, but C++/C# suffixes remain meaningful. Oversize atoms are never split.
pub fn atoms(s: &str) -> Vec<String> {
    s.split(|c: char| {
        !c.is_alphanumeric()
            && !matches!(c, '_' | '.' | ':' | '+' | '#' | '-')
            && !(('\u{0300}'..='\u{036f}').contains(&c))
    })
    .map(|s| s.trim_matches(|c| matches!(c, '.' | ':' | '-')))
    .filter(|s| s.chars().any(char::is_alphanumeric))
    .map(str::to_owned)
    .collect()
}
pub fn technical(s: &str) -> bool {
    s.chars()
        .any(|c| c.is_numeric() || matches!(c, '_' | '.' | ':' | '+' | '#' | '-'))
}
pub fn exact_keys(fields: &[&str]) -> Result<(Vec<String>, bool), KnowledgeError> {
    let mut seen = BTreeSet::new();
    let mut keys = Vec::new();
    let mut truncated = false;
    for field in fields {
        for atom in atoms(field) {
            if technical(&atom) && atom.chars().count() <= 128 {
                let key = normalize(&atom)?;
                if key.chars().count() > 128 {
                    continue;
                }
                if seen.insert(key.clone()) {
                    if keys.len() < 4096 {
                        keys.push(key)
                    } else {
                        truncated = true
                    }
                }
            }
        }
    }
    Ok((keys, truncated))
}
pub fn scope_mask(scopes: &[String]) -> Result<u8, KnowledgeError> {
    let mut mask = 0;
    for s in scopes {
        mask |= match s.as_str() {
            "read_metadata" => 1,
            "read_source_metadata" => 2,
            "read_paths" => 4,
            _ => return Err(KnowledgeError::Integrity),
        };
    }
    if mask & 1 == 0 {
        return Err(KnowledgeError::Integrity);
    }
    Ok(mask)
}
fn sql(_: rusqlite::Error) -> KnowledgeError {
    KnowledgeError::Integrity
}
pub struct Projection;
impl SearchProjectionTx for Projection {
    fn on_source_changed(
        &self,
        tx: &Transaction<'_>,
        _: Option<SqlRevision>,
        card: &CardView,
    ) -> Result<(), KnowledgeError> {
        let id = card.record_id.0.to_string();
        let mask = scope_mask(&card.source.required_read_scopes)?;
        let old:Option<(u64,String,u8)>=tx.query_row("SELECT content_revision,source_sha256,required_scope_mask FROM search_documents WHERE record_id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
        let (kind, date): (String, String) = tx
            .query_row(
                "SELECT source_kind,created_at FROM records WHERE id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(sql)?;
        let pk: i64 = tx
            .query_row("SELECT rowid FROM records WHERE id=?1", [&id], |r| r.get(0))
            .map_err(sql)?;
        let category = card
            .source
            .category_label
            .as_ref()
            .map_or("", |v| v.0.as_str());
        // Accepted source does not yet own app/window/filename/OCR. Empty columns are
        // explicit absence, not guesses from raw context/history/absolute origin paths.
        let (keys, truncated) =
            exact_keys(&[&card.source.title.0, &card.source.description.0, category])?;
        let consistent:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM search_documents d JOIN image_fts f ON f.rowid=d.record_pk WHERE d.record_id=?1 AND d.record_pk=?2 AND d.source_asset_id=?3 AND d.title=?4 AND d.description=?5 AND d.category_text=?6 AND d.source_kind=?7 AND d.ingested_at_utc=?8 AND d.exact_terms_truncated=?9 AND f.title=d.title AND f.description=d.description AND f.category_text=d.category_text AND f.application_text='' AND f.window_title='' AND f.filename='' AND f.ocr_text='')",params![id,pk,card.source_asset_id.0.to_string(),card.source.title.0,card.source.description.0,category,kind,date,truncated],|r|r.get(0)).map_err(sql)?;
        let stored: Vec<String> = {
            let mut s = tx
                .prepare(
                    "SELECT term_key FROM search_exact_terms WHERE record_pk=?1 ORDER BY term_key",
                )
                .map_err(sql)?;
            let rows = s.query_map([pk], |r| r.get(0)).map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        let mut sorted = keys.clone();
        sorted.sort();
        if consistent
            && old == Some((card.content_revision.0, card.source_sha256.clone(), mask))
            && stored == sorted
        {
            return Ok(());
        }
        tx.execute("INSERT INTO search_documents VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'','','','',?11,?12,?13) ON CONFLICT(record_id) DO UPDATE SET source_asset_id=excluded.source_asset_id,content_revision=excluded.content_revision,source_sha256=excluded.source_sha256,required_scope_mask=excluded.required_scope_mask,title=excluded.title,description=excluded.description,category_id=excluded.category_id,category_text=excluded.category_text,source_kind=excluded.source_kind,ingested_at_utc=excluded.ingested_at_utc,exact_terms_truncated=excluded.exact_terms_truncated",params![pk,id,card.source_asset_id.0.to_string(),card.content_revision.0,card.source_sha256,mask,card.source.title.0,card.source.description.0,card.source.category_id.map(|v|v.0.to_string()),category,kind,date,truncated]).map_err(sql)?;
        tx.execute("DELETE FROM image_fts WHERE rowid=?1", [pk])
            .map_err(sql)?;
        tx.execute("INSERT INTO image_fts(rowid,title,description,category_text,application_text,window_title,filename,ocr_text) VALUES(?1,?2,?3,?4,'','','','')",params![pk,card.source.title.0,card.source.description.0,category]).map_err(sql)?;
        tx.execute("DELETE FROM search_exact_terms WHERE record_pk=?1", [pk])
            .map_err(sql)?;
        for key in keys {
            tx.execute(
                "INSERT INTO search_exact_terms VALUES(?1,?2)",
                params![pk, key],
            )
            .map_err(sql)?;
        }
        tx.execute(
            "DELETE FROM knowledge_projection_debts WHERE record_id=?1",
            [&id],
        )
        .map_err(sql)?;
        tx.execute("UPDATE search_projection_meta SET projection_epoch=projection_epoch+1 WHERE singleton=1",[]).map_err(sql)?;
        Ok(())
    }
}
