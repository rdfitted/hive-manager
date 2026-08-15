//! Serializable org-graph types shared by definition, composition, and runtime consumers.

use serde::{Deserialize, Serialize};

use crate::cli::CliBehavior;

/// A durable semantic role definition. Runtime agents record the definition id
/// and version, but the definition itself remains an external, reusable artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDefinition {
    pub id: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge_scope: Vec<KnowledgeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens: Option<RoleLens>,
    #[serde(default)]
    pub authority: AuthorityScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<CliBehavior>,
    #[serde(default)]
    pub context_boundary: ContextBoundary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_class: Option<SignalClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_template: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_goals: Vec<String>,
}

impl RoleDefinition {
    /// Backward-compatible role identity: task context and CLI capability only.
    pub fn empty(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: 0,
            domain: None,
            knowledge_scope: Vec::new(),
            lens: None,
            authority: AuthorityScope::default(),
            behavior: None,
            context_boundary: ContextBoundary::default(),
            signal_class: None,
            prompt_template: None,
            non_goals: Vec::new(),
        }
    }
}

/// The evaluator's behavior is data on its role definition, not a CLI registry exception.
pub fn evaluator_role_definition() -> RoleDefinition {
    let mut definition = RoleDefinition::empty("evaluator");
    definition.version = 1;
    definition.behavior = Some(CliBehavior::InstructionFollowing);
    definition
}

/// A pointer into a knowledge graph plus a bounded synopsis. It never embeds
/// the referenced document or subtree.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KnowledgeRef {
    pub source: KnowledgeSource,
    pub pointer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub priority: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeSource {
    Institutional,
    Project,
}

/// The question a role consistently brings to otherwise task-specific context.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoleLens {
    pub id: String,
    pub question: String,
}

/// Role-level authority independent of a particular task assignment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityScope {
    #[serde(default)]
    pub may_commit: bool,
    #[serde(default)]
    pub may_push: bool,
    #[serde(default)]
    pub may_spawn_subordinates: bool,
    #[serde(default)]
    pub may_adjudicate: bool,
    #[serde(default)]
    pub may_mutate_unowned_paths: bool,
}

/// How much spawner context may cross into a role's composed prompt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextBoundary {
    None,
    Artifact,
    #[default]
    Full,
}

/// Verification signals differ in how much contextual independence they require.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalClass {
    Mechanical,
    Judgmental,
}
