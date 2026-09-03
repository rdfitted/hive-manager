//! Serializable types shared by planned and observed work graphs.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Stable identifier assigned to a work node by the plan.
pub type TaskId = String;

/// Provider-independent effort level requested for a planned task.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TaskTier {
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

/// A complete work graph. Dependency edges describe scheduling; the other edge
/// kinds preserve data flow, review, context, and repository relationships.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskGraph {
    #[serde(default)]
    pub nodes: Vec<WorkNode>,
    #[serde(default)]
    pub edges: Vec<WorkEdge>,
    #[serde(default)]
    pub omissions: Vec<WorkGraphOmission>,
}

/// Naming used by persistence and runtime consumers of the same graph model.
pub type WorkGraph = TaskGraph;

impl TaskGraph {
    pub fn new(nodes: Vec<WorkNode>, edges: Vec<WorkEdge>) -> Self {
        Self {
            nodes,
            edges,
            omissions: Vec::new(),
        }
    }

    /// Absence is derived from structured reasons rather than stored as a bool.
    pub fn has_omissions(&self) -> bool {
        !self.omissions.is_empty()
    }
}

/// One schedulable or descriptive unit in a work graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkNode {
    pub id: TaskId,
    pub kind: NodeKind,
    pub title: String,
    #[serde(default)]
    pub tier: TaskTier,
    pub contract: NodeContract,
    /// A reusable role/zone selector. It is resolved against the session's
    /// `HierarchyNode`s at claim time, never against a plan-time agent id.
    pub binding: BindingRef,
    pub status: NodeStatus,
    /// Declares that runtime may replace this node with a template subgraph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion: Option<CompositeExpansion>,
}

impl WorkNode {
    pub fn new(
        id: impl Into<TaskId>,
        kind: NodeKind,
        title: impl Into<String>,
        contract: NodeContract,
        binding: BindingRef,
        status: NodeStatus,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            title: title.into(),
            tier: TaskTier::default(),
            contract,
            binding,
            status,
            expansion: None,
        }
    }

    pub fn with_tier(mut self, tier: TaskTier) -> Self {
        self.tier = tier;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Task,
    Review,
    Join,
    Checkpoint,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NodeContract {
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
}

/// A plan-time selector whose value is meaningful across session instances.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum BindingRef {
    Role(String),
    Zone(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositeExpansion {
    pub template: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    Ready,
    Running,
    Completed,
    Failed,
    Blocked,
    Cancelled,
}

/// A directed relationship. For `DependsOn`, `source` is the prerequisite and
/// `target` is the dependent node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkEdge {
    pub source: TaskId,
    pub target: TaskId,
    pub kind: EdgeKind,
    /// Required by construction and deserialization. There is intentionally no
    /// `Default` implementation for either an edge or its provenance.
    pub provenance: EdgeProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

impl WorkEdge {
    pub fn new(
        source: impl Into<TaskId>,
        target: impl Into<TaskId>,
        kind: EdgeKind,
        provenance: EdgeProvenance,
    ) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            kind,
            provenance,
            rationale: None,
        }
    }

    pub fn with_rationale(mut self, rationale: impl Into<String>) -> Self {
        self.rationale = Some(rationale.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    DependsOn,
    Produces,
    Consumes,
    Reviews,
    Informs,
    Touches,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeProvenance {
    Planner,
    Codegraph,
    Knowledge,
    Runtime,
}

/// One stated reason that graph construction could not observe all relevant
/// inputs. Consumers receive counts and examples, never an ambiguous bool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkGraphOmission {
    pub reason: WorkGraphOmissionReason,
    pub count: usize,
    pub detail: String,
    #[serde(default)]
    pub examples: Vec<String>,
}

impl WorkGraphOmission {
    pub fn new(reason: WorkGraphOmissionReason, count: usize, examples: Vec<String>) -> Self {
        Self {
            reason,
            count,
            detail: reason.detail().to_string(),
            examples,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkGraphOmissionReason {
    CodegraphUnavailable,
    ProjectKnowledgeUnavailable,
    SourceUnreadable,
    ResolutionIncomplete,
    CompletionUnresolved,
}

impl WorkGraphOmissionReason {
    pub const fn detail(self) -> &'static str {
        match self {
            Self::CodegraphUnavailable => {
                "codegraph output was unavailable, so repository relationships are incomplete"
            }
            Self::ProjectKnowledgeUnavailable => {
                "project knowledge was unavailable, so context relationships are incomplete"
            }
            Self::SourceUnreadable => {
                "a graph source could not be read, so derived relationships are incomplete"
            }
            Self::ResolutionIncomplete => "one or more graph references could not be resolved",
            Self::CompletionUnresolved => {
                "a recorded completion could not be resolved to a plan node"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskTier, WorkNode};

    #[test]
    fn task_tier_wire_names_default_and_order_are_stable() {
        assert_eq!(TaskTier::default(), TaskTier::Medium);
        assert!(TaskTier::Low < TaskTier::Medium);
        assert!(TaskTier::Medium < TaskTier::High);
        assert!(TaskTier::High < TaskTier::Critical);

        let cases = [
            (TaskTier::Low, r#""low""#),
            (TaskTier::Medium, r#""medium""#),
            (TaskTier::High, r#""high""#),
            (TaskTier::Critical, r#""critical""#),
        ];
        for (tier, expected) in cases {
            assert_eq!(serde_json::to_string(&tier).unwrap(), expected);
            assert_eq!(serde_json::from_str::<TaskTier>(expected).unwrap(), tier);
        }
    }

    #[test]
    fn legacy_work_node_without_tier_defaults_medium() {
        let legacy = r#"{
            "id": "legacy",
            "kind": "task",
            "title": "Legacy task",
            "contract": {},
            "binding": { "kind": "role", "value": "backend" },
            "status": "pending"
        }"#;

        let node = serde_json::from_str::<WorkNode>(legacy).expect("legacy work node");
        assert_eq!(node.tier, TaskTier::Medium);
    }
}
