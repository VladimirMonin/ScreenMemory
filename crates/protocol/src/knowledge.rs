//! Manual knowledge v1. No AI publication or runtime authority on this boundary.
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use uuid::Uuid;

pub const V4_PATTERN: &str =
    "^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";
pub const SQL_REV_PATTERN: &str = "^(?:0|[1-9][0-9]{0,17}|[1-8][0-9]{18}|9[01][0-9]{17}|920[0-9]{16}|921[0-9]{16}|922[0-2][0-9]{15}|9223[0-2][0-9]{14}|92233[0-6][0-9]{13}|922337[01][0-9]{12}|92233720[0-2][0-9]{10}|922337203[0-5][0-9]{9}|9223372036[0-7][0-9]{8}|92233720368[0-4][0-9]{7}|922337203685[0-3][0-9]{6}|9223372036854[0-6][0-9]{5}|92233720368547[0-6][0-9]{4}|922337203685477[0-4][0-9]{3}|9223372036854775[0-7][0-9]{2}|922337203685477580[0-7])$";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct KnowledgeId(pub Uuid);
impl<'de> Deserialize<'de> for KnowledgeId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let id = Uuid::parse_str(&s).map_err(|_| D::Error::custom("invalid identity"))?;
        if s != id.to_string()
            || id.get_version_num() != 4
            || id.get_variant() != uuid::Variant::RFC4122
        {
            return Err(D::Error::custom("invalid identity"));
        }
        Ok(Self(id))
    }
}
impl JsonSchema for KnowledgeId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "KnowledgeId".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string","pattern":V4_PATTERN,"minLength":36,"maxLength":36})
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(into = "String")]
pub struct SqlRevision(pub u64);
impl From<SqlRevision> for String {
    fn from(v: SqlRevision) -> String {
        v.0.to_string()
    }
}
impl<'de> Deserialize<'de> for SqlRevision {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let n = s
            .parse::<u64>()
            .map_err(|_| D::Error::custom("invalid revision"))?;
        if n > i64::MAX as u64 || s != n.to_string() {
            return Err(D::Error::custom("invalid revision"));
        }
        Ok(Self(n))
    }
}
impl JsonSchema for SqlRevision {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SqlRevision".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string","pattern":SQL_REV_PATTERN,"minLength":1,"maxLength":19})
    }
}
// Unicode White_Space (the Rust trim contract), not platform-dependent regex \s.
// U+FEFF and U+001C..U+001F are not White_Space; NUL is forbidden separately.
const NONBLANK_TEXT_PATTERN: &str = r"^[^\u0000]*[^\u0000\u0009-\u000D\u0020\u0085\u00A0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000][^\u0000]*$";
macro_rules! text {
    ($name:ident, $max:expr, $nonblank:expr) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                if s.chars().count() > $max || ($nonblank && s.trim().is_empty()) || s.contains('\0') { return Err(D::Error::custom("invalid text")); }
                Ok(Self(s))
            }
        }
        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                let pattern = if $nonblank { NONBLANK_TEXT_PATTERN } else { "^[^\\u0000]*$" };
                json_schema!({"type":"string","maxLength":$max,"pattern":pattern})
            }
        }
    }
}
text!(CardTitle, 160, true);
text!(CardDescription, 8000, false);
text!(CategoryName, 80, true);
text!(CategoryDescription, 400, false);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, Default)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum FieldPatch<T> {
    #[default]
    Keep,
    Inherit,
    Set {
        value: T,
    },
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for FieldPatch<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Serde unit enum variants ignore extra fields; empty STRUCT variants enforce
        // the same closed shape as generated JSON Schema for Keep/Inherit.
        #[derive(Deserialize)]
        #[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
        enum Closed<T> {
            Keep {},
            Inherit {},
            Set { value: T },
        }
        Ok(match Closed::deserialize(d)? {
            Closed::Keep {} => Self::Keep,
            Closed::Inherit {} => Self::Inherit,
            Closed::Set { value } => Self::Set { value },
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct CardPatch {
    #[serde(default)]
    pub title: FieldPatch<CardTitle>,
    #[serde(default)]
    pub description: FieldPatch<CardDescription>,
    #[serde(default)]
    pub category_id: FieldPatch<Option<KnowledgeId>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardEdit {
    pub operation_id: KnowledgeId,
    pub record_id: KnowledgeId,
    pub expected_record_revision: SqlRevision,
    pub expected_catalog_revision: SqlRevision,
    pub expected_category_revision: Option<SqlRevision>,
    pub patch: CardPatch,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CategoryState {
    Active,
    Archived,
    Deleted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoryView {
    pub id: KnowledgeId,
    pub revision: SqlRevision,
    pub name: CategoryName,
    pub description: CategoryDescription,
    pub state: CategoryState,
    pub assigned_records: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoryCatalog {
    pub revision: SqlRevision,
    pub items: Vec<CategoryView>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum CategoryChange {
    Create {
        name: CategoryName,
        description: CategoryDescription,
    },
    Rename {
        category_id: KnowledgeId,
        expected_revision: SqlRevision,
        name: CategoryName,
        description: CategoryDescription,
    },
    State {
        category_id: KnowledgeId,
        expected_revision: SqlRevision,
        state: CategoryState,
        expected_assigned_records: u32,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoryMutation {
    pub operation_id: KnowledgeId,
    pub expected_catalog_revision: SqlRevision,
    pub change: CategoryChange,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldSource {
    User,
    Ai,
    Fallback,
    None,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectiveCard {
    pub title: CardTitle,
    pub description: CardDescription,
    pub category_id: Option<KnowledgeId>,
    pub category_label: Option<CategoryName>,
    pub title_source: FieldSource,
    pub description_source: FieldSource,
    pub category_source: FieldSource,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSource {
    pub source_version: String,
    pub record_id: KnowledgeId,
    pub source_asset_id: KnowledgeId,
    pub title: CardTitle,
    pub description: CardDescription,
    pub category_id: Option<KnowledgeId>,
    pub category_label: Option<CategoryName>,
    pub required_read_scopes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardView {
    pub record_id: KnowledgeId,
    pub source_asset_id: KnowledgeId,
    pub record_revision: SqlRevision,
    pub content_revision: SqlRevision,
    pub overrides: CardPatch,
    pub effective: EffectiveCard,
    pub source: KnowledgeSource,
    pub source_sha256: String,
    pub projection_state: ProjectionState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionState {
    HandlerUnavailable,
    Current,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardReceipt {
    pub operation_id: KnowledgeId,
    pub card: CardView,
    pub effective_changed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoryReceipt {
    pub operation_id: KnowledgeId,
    pub category: CategoryView,
    pub catalog_revision: SqlRevision,
    pub affected_records: u32,
    pub projection_state: ProjectionState,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum KnowledgeRequest {
    Read { record_id: KnowledgeId },
    Edit(CardEdit),
    Categories {},
    Category(CategoryMutation),
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum KnowledgeReply {
    Card(Box<CardView>),
    Edited(Box<CardReceipt>),
    Categories(CategoryCatalog),
    Category(CategoryReceipt),
}
