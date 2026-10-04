//! Ordinary lexical/exact search. No model or semantic execution intent.
use crate::knowledge::{KnowledgeId, SqlRevision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CategoryFilter {
    Any {},
    Uncategorized {},
    AnyOf { ids: Vec<KnowledgeId> },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApplicationFilter {
    Any {},
    Unattributed {},
    AnyOf { ids: Vec<KnowledgeId> },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Capture,
    ImportedImage,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DateAxis {
    Ingested,
    Captured,
    Taken,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DateFilter {
    Any {},
    UtcRange {
        axis: DateAxis,
        from_inclusive: String,
        to_exclusive: String,
    },
    TakenOffsetUnknown {},
    TakenAbsent {},
}
impl Default for CategoryFilter {
    fn default() -> Self {
        Self::Any {}
    }
}
impl Default for ApplicationFilter {
    fn default() -> Self {
        Self::Any {}
    }
}
impl Default for DateFilter {
    fn default() -> Self {
        Self::Any {}
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchFilters {
    #[serde(default)]
    pub category: CategoryFilter,
    #[serde(default)]
    pub application: ApplicationFilter,
    #[serde(default)]
    pub date: DateFilter,
    pub source_kind: Option<SourceKind>,
    pub has_ocr: Option<bool>,
    pub has_applied_ai: Option<bool>,
}
fn limit() -> u16 {
    30
}
fn depth() -> u16 {
    100
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchStart {
    pub search_id: KnowledgeId,
    #[schemars(length(max = 1024))]
    pub text: String,
    #[serde(default)]
    pub filters: SearchFilters,
    #[serde(default = "limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: u16,
    #[serde(default = "depth")]
    #[schemars(range(min = 100, max = 2000))]
    pub candidate_depth: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SearchRequest {
    Start(SearchStart),
    Page { cursor: KnowledgeId },
    Cancel { search_id: KnowledgeId },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchHit {
    pub record_id: KnowledgeId,
    pub asset_id: KnowledgeId,
    pub content_revision: SqlRevision,
    pub title: String,
    pub excerpt: String,
    pub category_id: Option<KnowledgeId>,
    pub category_label: Option<String>,
    pub exact_match: bool,
    pub lexical_match: bool,
    pub exact_terms_truncated: bool,
    pub position: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchPage {
    pub search_id: KnowledgeId,
    pub projection_epoch: SqlRevision,
    pub candidate_depth: u16,
    pub window_truncated: bool,
    pub next_cursor: Option<KnowledgeId>,
    pub items: Vec<SearchHit>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SearchReply {
    Page(SearchPage),
    Cancelled { search_id: KnowledgeId },
}

/// Shared native validation before SQL admission. Unsupported source fields remain explicit.
pub fn validate(r: &SearchStart) -> Result<(), crate::errors::ErrorCode> {
    use crate::errors::ErrorCode::*;
    let ids_valid = |ids: &[KnowledgeId]| {
        !ids.is_empty()
            && ids.len() <= 32
            && ids.iter().enumerate().all(|(i, id)| !ids[..i].contains(id))
    };
    if r.text.len() > 4096
        || r.text.chars().count() > 1024
        || r.text.contains('\0')
        || !(1..=200).contains(&r.limit)
        || !(100..=2000).contains(&r.candidate_depth)
        || r.candidate_depth < r.limit
    {
        return Err(InvalidRequest);
    }
    if matches!(&r.filters.category, CategoryFilter::AnyOf{ids} if !ids_valid(ids))
        || matches!(&r.filters.application, ApplicationFilter::AnyOf{ids} if !ids_valid(ids))
    {
        return Err(InvalidRequest);
    }
    if let DateFilter::UtcRange {
        from_inclusive,
        to_exclusive,
        ..
    } = &r.filters.date
    {
        let parse = |s: &str| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        };
        match (parse(from_inclusive), parse(to_exclusive)) {
            (Ok(a), Ok(b)) if a < b => {}
            _ => return Err(InvalidRequest),
        }
    }
    if !matches!(r.filters.application, ApplicationFilter::Any {})
        || r.filters.has_ocr.is_some()
        || r.filters.has_applied_ai.is_some()
        || !matches!(
            r.filters.date,
            DateFilter::Any {}
                | DateFilter::UtcRange {
                    axis: DateAxis::Ingested,
                    ..
                }
        )
    {
        return Err(SearchFilterUnsupported);
    }
    Ok(())
}
