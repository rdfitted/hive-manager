//! Orchestrator ownership and write-collision semantics.
//!
//! Orchestrators are explicit session principals. Their authority answers what
//! they may do; their footprint answers where and why they expect to write.
//! Keeping those declarations separate preserves collision evidence even when
//! an orchestrator is authorized to override live ownership.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::AuthorityScope;
use crate::orchestrator::work_graph::schema::{
    BindingRef, EdgeKind, NodeKind, TaskGraph, TaskId,
};

const CODEGRAPH_ZONE: &str = "codegraph";
const CODEGRAPH_MODULE_TEMPLATE: &str = "codegraph-module";

/// A first-class orchestrator node in visible session ownership state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestratorRole {
    Queen,
    Prince,
    Evaluator,
}

/// Why an orchestrator expects to touch a path outside ordinary task ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestratorOperation {
    IntegrationEdit,
    Restore,
    MutationProof,
    Verification,
    Remediation,
}

/// One exact path in an orchestrator's declared footprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipFootprint {
    pub path: String,
    pub operation: OrchestratorOperation,
}

impl OwnershipFootprint {
    pub fn new(path: impl Into<String>, operation: OrchestratorOperation) -> Self {
        Self {
            path: normalize_path(&path.into()),
            operation,
        }
    }
}

/// Ownership-specific authority that cannot be inferred from broad role power.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipAuthority {
    #[serde(default)]
    pub may_adjudicate_blocked_tasks: bool,
    #[serde(default)]
    pub may_override_live_ownership: bool,
    #[serde(default)]
    pub may_mutate_mid_flight: bool,
}

/// The role, footprint, and authority declarations serialized for one
/// orchestrator. `authority` deliberately does not replace `footprint`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorOwnershipNode {
    pub principal_id: String,
    pub role: OrchestratorRole,
    pub footprint: Vec<OwnershipFootprint>,
    pub authority: AuthorityScope,
    pub ownership_authority: OwnershipAuthority,
}

impl OrchestratorOwnershipNode {
    pub fn queen(
        principal_id: impl Into<String>,
        footprint: Vec<OwnershipFootprint>,
    ) -> Self {
        Self {
            principal_id: principal_id.into(),
            role: OrchestratorRole::Queen,
            footprint,
            authority: AuthorityScope {
                may_commit: true,
                may_push: true,
                may_spawn_subordinates: true,
                may_adjudicate: true,
                may_mutate_unowned_paths: true,
            },
            ownership_authority: OwnershipAuthority {
                may_adjudicate_blocked_tasks: true,
                may_override_live_ownership: true,
                may_mutate_mid_flight: true,
            },
        }
    }

    pub fn prince(
        principal_id: impl Into<String>,
        footprint: Vec<OwnershipFootprint>,
    ) -> Self {
        Self {
            principal_id: principal_id.into(),
            role: OrchestratorRole::Prince,
            footprint,
            authority: AuthorityScope {
                may_commit: false,
                may_push: false,
                may_spawn_subordinates: true,
                may_adjudicate: true,
                may_mutate_unowned_paths: false,
            },
            ownership_authority: OwnershipAuthority {
                may_adjudicate_blocked_tasks: true,
                may_override_live_ownership: false,
                may_mutate_mid_flight: false,
            },
        }
    }

    pub fn evaluator(
        principal_id: impl Into<String>,
        footprint: Vec<OwnershipFootprint>,
    ) -> Self {
        Self {
            principal_id: principal_id.into(),
            role: OrchestratorRole::Evaluator,
            footprint,
            authority: AuthorityScope::default(),
            ownership_authority: OwnershipAuthority::default(),
        }
    }

    fn declares(&self, path: &str, operation: OrchestratorOperation) -> bool {
        let path = normalize_path(path);
        self.footprint
            .iter()
            .any(|entry| entry.operation == operation && entry.path == path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipDeclarationError {
    MissingRole(OrchestratorRole),
    DuplicateRole(OrchestratorRole),
    EmptyFootprint(OrchestratorRole),
}

impl fmt::Display for OwnershipDeclarationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRole(role) => write!(formatter, "missing {role:?} ownership node"),
            Self::DuplicateRole(role) => write!(formatter, "duplicate {role:?} ownership node"),
            Self::EmptyFootprint(role) => {
                write!(formatter, "{role:?} ownership node has no declared footprint")
            }
        }
    }
}

impl Error for OwnershipDeclarationError {}

/// A live worker/principal assignment. `write_capable` is explicit because a
/// task status mirror may say completed while its process can still write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivePrincipal {
    pub principal_id: String,
    pub task_id: TaskId,
    pub write_capable: bool,
}

/// File ownership derived from a real `Touches` edge and a live principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalPathOwnership {
    pub principal_id: String,
    pub task_id: TaskId,
    pub path: String,
    pub write_capable: bool,
}

/// The write attempt that must be checked before mutating the filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorWriteAttempt {
    pub actor_id: String,
    pub role: OrchestratorRole,
    pub path: String,
    pub operation: OrchestratorOperation,
    pub attempted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionDisposition {
    /// The live owner must finish or explicitly yield before this write runs.
    RequiresSerialization,
    /// The override is allowed, but the collision remains surfaced and durable.
    SurfacedAuthorizedOverride,
}

/// Durable evidence of an orchestrator/live-principal path collision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipCollision {
    pub actor_id: String,
    pub actor_role: OrchestratorRole,
    pub owner_principal_id: String,
    pub owner_task_id: TaskId,
    pub owner_write_capable: bool,
    pub path: String,
    pub operation: OrchestratorOperation,
    pub within_declared_footprint: bool,
    pub override_authorized: bool,
    pub disposition: CollisionDisposition,
    pub detected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum OrchestratorWriteOutcome {
    Proceed,
    Collision { collision: OwnershipCollision },
}

/// One output-defined task class whose members lack a review duty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationDutyGap {
    pub task_class: String,
    pub unassigned_task_ids: Vec<TaskId>,
}

/// Visible, serializable ownership state for the running session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipSessionState {
    pub schema_version: u32,
    pub orchestrators: Vec<OrchestratorOwnershipNode>,
    pub live_principal_ownership: Vec<PrincipalPathOwnership>,
    #[serde(default)]
    pub collisions: Vec<OwnershipCollision>,
    #[serde(default)]
    pub verification_duty_gaps: Vec<VerificationDutyGap>,
}

impl OwnershipSessionState {
    pub fn from_plan(
        graph: &TaskGraph,
        mut orchestrators: Vec<OrchestratorOwnershipNode>,
        live_principals: &[LivePrincipal],
    ) -> Result<Self, OwnershipDeclarationError> {
        validate_orchestrator_declarations(&orchestrators)?;
        orchestrators.sort_by_key(|node| node.role);
        Ok(Self {
            schema_version: 1,
            orchestrators,
            live_principal_ownership: derive_path_ownership(graph, live_principals),
            collisions: Vec::new(),
            verification_duty_gaps: verification_duty_gaps(graph),
        })
    }

    /// Detect and persist a collision before the caller performs the write.
    pub fn record_write_attempt(
        &mut self,
        attempt: OrchestratorWriteAttempt,
    ) -> OrchestratorWriteOutcome {
        let path = normalize_path(&attempt.path);
        let Some(actor) = self.orchestrators.iter().find(|node| {
            node.principal_id == attempt.actor_id && node.role == attempt.role
        }) else {
            return OrchestratorWriteOutcome::Proceed;
        };
        let Some(owner) = self
            .live_principal_ownership
            .iter()
            .find(|owner| owner.write_capable && owner.path == path)
        else {
            return OrchestratorWriteOutcome::Proceed;
        };

        let override_authorized = actor.ownership_authority.may_override_live_ownership
            && actor.ownership_authority.may_mutate_mid_flight;
        let collision = OwnershipCollision {
            actor_id: actor.principal_id.clone(),
            actor_role: actor.role,
            owner_principal_id: owner.principal_id.clone(),
            owner_task_id: owner.task_id.clone(),
            owner_write_capable: owner.write_capable,
            path,
            operation: attempt.operation,
            within_declared_footprint: actor.declares(&attempt.path, attempt.operation),
            override_authorized,
            disposition: if override_authorized {
                CollisionDisposition::SurfacedAuthorizedOverride
            } else {
                CollisionDisposition::RequiresSerialization
            },
            detected_at: attempt.attempted_at,
        };
        self.collisions.push(collision.clone());
        OrchestratorWriteOutcome::Collision { collision }
    }
}

/// Build the three explicit orchestrator nodes for a plan. Their session-state
/// artifacts are always in scope, while repository paths come only from the
/// plan's real Codegraph `Touches` surface rather than an implicit Queen=all
/// rule.
pub fn orchestrator_nodes_for_plan(graph: &TaskGraph) -> Vec<OrchestratorOwnershipNode> {
    let paths = module_paths(graph).into_values().collect::<BTreeSet<_>>();
    let mut queen = vec![OwnershipFootprint::new(
        "state/work-graph.json",
        OrchestratorOperation::IntegrationEdit,
    )];
    let mut prince = vec![OwnershipFootprint::new(
        "state/assignments.json",
        OrchestratorOperation::Remediation,
    )];
    let mut evaluator = vec![OwnershipFootprint::new(
        "state/work-graph-reviews.json",
        OrchestratorOperation::Verification,
    )];
    for path in paths {
        queen.extend([
            OwnershipFootprint::new(&path, OrchestratorOperation::IntegrationEdit),
            OwnershipFootprint::new(&path, OrchestratorOperation::Restore),
            OwnershipFootprint::new(&path, OrchestratorOperation::MutationProof),
        ]);
        prince.push(OwnershipFootprint::new(
            &path,
            OrchestratorOperation::Remediation,
        ));
        evaluator.extend([
            OwnershipFootprint::new(&path, OrchestratorOperation::Verification),
            OwnershipFootprint::new(&path, OrchestratorOperation::MutationProof),
        ]);
    }
    vec![
        OrchestratorOwnershipNode::queen("queen", queen),
        OrchestratorOwnershipNode::prince("prince", prince),
        OrchestratorOwnershipNode::evaluator("evaluator", evaluator),
    ]
}

fn validate_orchestrator_declarations(
    orchestrators: &[OrchestratorOwnershipNode],
) -> Result<(), OwnershipDeclarationError> {
    for role in [
        OrchestratorRole::Queen,
        OrchestratorRole::Prince,
        OrchestratorRole::Evaluator,
    ] {
        let matching = orchestrators.iter().filter(|node| node.role == role).count();
        if matching == 0 {
            return Err(OwnershipDeclarationError::MissingRole(role));
        }
        if matching > 1 {
            return Err(OwnershipDeclarationError::DuplicateRole(role));
        }
        if orchestrators
            .iter()
            .find(|node| node.role == role)
            .is_some_and(|node| node.footprint.is_empty())
        {
            return Err(OwnershipDeclarationError::EmptyFootprint(role));
        }
    }
    Ok(())
}

/// Resolve live path ownership from Codegraph-produced `Touches` edges. No
/// separate ownership matrix exists; the module node's zone and expansion
/// parameter are the file-level source of truth.
pub fn derive_path_ownership(
    graph: &TaskGraph,
    live_principals: &[LivePrincipal],
) -> Vec<PrincipalPathOwnership> {
    let modules = module_paths(graph);
    let principals = live_principals
        .iter()
        .map(|principal| (principal.task_id.as_str(), principal))
        .collect::<BTreeMap<_, _>>();

    let mut ownership = graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Touches)
        .filter_map(|edge| {
            let principal = principals.get(edge.source.as_str())?;
            let path = modules.get(edge.target.as_str())?;
            Some(PrincipalPathOwnership {
                principal_id: principal.principal_id.clone(),
                task_id: principal.task_id.clone(),
                path: path.clone(),
                write_capable: principal.write_capable,
            })
        })
        .collect::<Vec<_>>();
    ownership.sort_by(|left, right| {
        (&left.path, &left.principal_id, &left.task_id).cmp(&(
            &right.path,
            &right.principal_id,
            &right.task_id,
        ))
    });
    ownership.dedup();
    ownership
}

fn module_paths(graph: &TaskGraph) -> BTreeMap<&str, String> {
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            let BindingRef::Zone(zone) = &node.binding else {
                return None;
            };
            if zone != CODEGRAPH_ZONE {
                return None;
            }
            let expansion = node.expansion.as_ref()?;
            if expansion.template != CODEGRAPH_MODULE_TEMPLATE {
                return None;
            }
            let path = expansion.parameters.get("module")?;
            Some((node.id.as_str(), normalize_path(path)))
        })
        .collect()
}

/// Report task-class verification gaps while the plan is being validated.
/// Task output names are the existing class selector used by review templates.
pub fn verification_duty_gaps(graph: &TaskGraph) -> Vec<VerificationDutyGap> {
    let reviewed_tasks = graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Reviews)
        .map(|edge| edge.source.as_str())
        .collect::<BTreeSet<_>>();
    let mut classes = BTreeMap::<String, BTreeSet<TaskId>>::new();
    for node in graph.nodes.iter().filter(|node| node.kind == NodeKind::Task) {
        if reviewed_tasks.contains(node.id.as_str()) {
            continue;
        }
        for output in node
            .contract
            .outputs
            .iter()
            .map(|output| output.trim())
            .filter(|output| !output.is_empty())
        {
            classes
                .entry(format!("output:{output}"))
                .or_default()
                .insert(node.id.clone());
        }
    }
    classes
        .into_iter()
        .map(|(task_class, task_ids)| VerificationDutyGap {
            task_class,
            unassigned_task_ids: task_ids.into_iter().collect(),
        })
        .collect()
}

fn normalize_path(path: &str) -> String {
    let mut normalized = path.trim().replace('\\', "/");
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    normalized
}
