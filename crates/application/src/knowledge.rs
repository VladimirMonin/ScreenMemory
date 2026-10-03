//! Manual metadata service and the single pure effective-field resolver.
use crate::RequestContext;
use async_trait::async_trait;
use sm_protocol::{errors::AppError, knowledge::*};
#[async_trait]
pub trait KnowledgeApi: Send + Sync {
    async fn knowledge(
        &self,
        ctx: RequestContext,
        request: KnowledgeRequest,
    ) -> Result<KnowledgeReply, AppError>;
}
/// Pure accepted/applicable proposal values ONLY. Not a publication capability or a wire DTO.
/// Future publisher must establish execution/asset/intent/authorization/privacy fences before
/// supplying these values, then merge CURRENT overrides (not frozen record_revision).
pub struct ApplicableProposal {
    pub title: CardTitle,
    pub description: CardDescription,
    pub category_id: Option<KnowledgeId>,
}
pub fn resolve(
    overrides: &CardPatch,
    proposal: Option<&ApplicableProposal>,
    fallback: CardTitle,
    label: Option<CategoryName>,
) -> EffectiveCard {
    let (title, title_source) = match &overrides.title {
        FieldPatch::Set { value } => (value.clone(), FieldSource::User),
        _ => proposal.map_or((fallback, FieldSource::Fallback), |p| {
            (p.title.clone(), FieldSource::Ai)
        }),
    };
    let (description, description_source) = match &overrides.description {
        FieldPatch::Set { value } => (value.clone(), FieldSource::User),
        _ => proposal.map_or((CardDescription(String::new()), FieldSource::None), |p| {
            (p.description.clone(), FieldSource::Ai)
        }),
    };
    let (category_id, category_source) = match overrides.category_id {
        FieldPatch::Set { value } => (value, FieldSource::User),
        _ => proposal.map_or((None, FieldSource::None), |p| {
            (p.category_id, FieldSource::Ai)
        }),
    };
    EffectiveCard {
        title,
        description,
        category_id,
        category_label: label,
        title_source,
        description_source,
        category_source,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_applicable_proposal_merges_current_overrides_not_record_revision() {
        let ai = ApplicableProposal {
            title: CardTitle("AI".into()),
            description: CardDescription("proposal".into()),
            category_id: None,
        };
        let mut overrides = CardPatch {
            description: FieldPatch::Set {
                value: CardDescription(String::new()),
            },
            category_id: FieldPatch::Set { value: None },
            ..CardPatch::default()
        };
        let card = resolve(&overrides, Some(&ai), CardTitle("fallback".into()), None);
        assert_eq!(card.title.0, "AI");
        assert_eq!(card.title_source, FieldSource::Ai);
        assert_eq!(card.description.0, "");
        assert_eq!(card.description_source, FieldSource::User);
        assert_eq!(card.category_source, FieldSource::User);
        overrides.description = FieldPatch::Inherit;
        assert_eq!(
            resolve(&overrides, Some(&ai), CardTitle("fallback".into()), None)
                .description
                .0,
            "proposal"
        );
    }
}
