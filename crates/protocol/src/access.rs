//! Metadata-only agent access. No credential, principal or authority in request DTOs.
use crate::knowledge::{CategoryName, KnowledgeId, SqlRevision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct AgentConnectionId(pub KnowledgeId);
impl AgentConnectionId {
    pub fn new(id: uuid::Uuid) -> Option<Self> {
        if id.get_version_num() == 4 && id.get_variant() == uuid::Variant::RFC4122 {
            Some(Self(KnowledgeId(id)))
        } else {
            None
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentScope {
    ReadMetadata,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentPolicy {
    #[schemars(length(min = 1, max = 1))]
    pub scopes: Vec<AgentScope>,
    #[schemars(length(max = 0))]
    pub roots: Vec<()>,
    #[schemars(length(max = 0))]
    pub capture_modes: Vec<()>,
    #[schemars(range(min = 1, max = 1))]
    pub max_active_mutations: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFields {
    scopes: Vec<AgentScope>,
    roots: Vec<()>,
    capture_modes: Vec<()>,
    max_active_mutations: u8,
}
impl<'de> Deserialize<'de> for AgentPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::try_from(PolicyFields::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
impl TryFrom<PolicyFields> for AgentPolicy {
    type Error = &'static str;
    fn try_from(p: PolicyFields) -> Result<Self, Self::Error> {
        if p.scopes != [AgentScope::ReadMetadata]
            || !p.roots.is_empty()
            || !p.capture_modes.is_empty()
            || p.max_active_mutations != 1
        {
            return Err("unsupported policy");
        }
        Ok(Self::default())
    }
}
impl Default for AgentPolicy {
    fn default() -> Self {
        Self {
            scopes: vec![AgentScope::ReadMetadata],
            roots: vec![],
            capture_modes: vec![],
            max_active_mutations: 1,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentProfileState {
    Active,
    Disabled,
    Revoked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentConnectionState {
    Enrolling,
    Active,
    Revoked,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileView {
    pub profile_id: KnowledgeId,
    pub principal_id: KnowledgeId,
    pub revision: SqlRevision,
    pub security_revision: SqlRevision,
    pub name: CategoryName,
    pub state: AgentProfileState,
    pub policy: AgentPolicy,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentConnectionView {
    pub connection_id: AgentConnectionId,
    pub profile_id: KnowledgeId,
    pub principal_id: KnowledgeId,
    pub revision: SqlRevision,
    pub credential_generation: SqlRevision,
    pub display_name: CategoryName,
    pub state: AgentConnectionState,
    pub credential_ready: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccessRequest {
    List {},
    CreateProfile {
        operation_id: KnowledgeId,
        name: CategoryName,
        policy: AgentPolicy,
    },
    UpdateProfile {
        operation_id: KnowledgeId,
        profile_id: KnowledgeId,
        expected_revision: SqlRevision,
        name: CategoryName,
        policy: AgentPolicy,
        state: AgentProfileState,
    },
    IssueConnection {
        operation_id: KnowledgeId,
        profile_id: KnowledgeId,
        expected_profile_revision: SqlRevision,
        display_name: CategoryName,
    },
    RevokeConnection {
        operation_id: KnowledgeId,
        connection_id: AgentConnectionId,
        expected_revision: SqlRevision,
    },
}
impl AccessRequest {
    pub fn operation_id(&self) -> Option<KnowledgeId> {
        match self {
            Self::List {} => None,
            Self::CreateProfile { operation_id, .. }
            | Self::UpdateProfile { operation_id, .. }
            | Self::IssueConnection { operation_id, .. }
            | Self::RevokeConnection { operation_id, .. } => Some(*operation_id),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AccessReply {
    Profile(Box<AgentProfileView>),
    Connection(Box<AgentConnectionView>),
    Catalog {
        #[schemars(length(max = 64))]
        profiles: Vec<AgentProfileView>,
        #[schemars(length(max = 128))]
        connections: Vec<AgentConnectionView>,
    },
}
