//! Validation performed before a planning session becomes `PlanReady`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use crate::orchestrator::org_graph::adjudication::{
    AdjudicationDeclaration, VerificationDuty,
};
use crate::orchestrator::org_graph::boundary::{
    context_boundary_satisfies, required_context_boundary,
    verification_duty_declares_signal_class, verification_duty_has_named_signal,
};
use crate::orchestrator::org_graph::ownership::verification_duty_gaps;
use crate::orchestrator::org_graph::{ContextBoundary, SignalClass};

use super::review::{
    ADJUDICATION_DECLARATION_PARAMETER, MULTI_LENS_REVIEW_TEMPLATE,
    VERIFICATION_DUTY_PARAMETER,
};
use super::schema::{EdgeKind, NodeKind, TaskGraph, TaskId};
use super::toposort::topological_sort;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReadyValidation {
    pub warnings: Vec<PlanReadyWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanReadyWarning {
    OrphanSubtrees { task_ids: Vec<TaskId> },
    UnassignedVerificationDuty {
        task_class: String,
        task_ids: Vec<TaskId>,
    },
}

impl fmt::Display for PlanReadyWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OrphanSubtrees { task_ids } => write!(
                formatter,
                "orphan task subtrees are disconnected from the first plan component: {}",
                task_ids.join(", ")
            ),
            Self::UnassignedVerificationDuty {
                task_class,
                task_ids,
            } => write!(
                formatter,
                "task class {task_class} has no assigned verification duty for: {}",
                task_ids.join(", ")
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingDependency {
    pub dependent: TaskId,
    pub dependency: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanReadyError {
    DuplicateTaskIds { task_ids: Vec<TaskId> },
    DanglingDependencies { references: Vec<DanglingDependency> },
    Cycle { task_ids: Vec<TaskId> },
    MissingVerificationSignal { review_id: TaskId },
    MissingVerificationSignalClass { review_id: TaskId },
    InsufficientVerificationIsolation {
        review_id: TaskId,
        signal_class: SignalClass,
        actual: ContextBoundary,
        required: ContextBoundary,
    },
    InvalidReviewAuthorityMetadata { review_id: TaskId, field: String },
    MissingAdjudicator { review_id: TaskId },
    AdjudicatorLacksAuthority { review_id: TaskId, role_id: String },
}

impl fmt::Display for PlanReadyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTaskIds { task_ids } => write!(
                formatter,
                "PlanReady rejected: duplicate task ids: {}",
                task_ids.join(", ")
            ),
            Self::DanglingDependencies { references } => {
                let details = references
                    .iter()
                    .map(|reference| {
                        format!(
                            "{} depends on unknown {}",
                            reference.dependent, reference.dependency
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(formatter, "PlanReady rejected: dangling dependencies: {details}")
            }
            Self::Cycle { task_ids } => write!(
                formatter,
                "PlanReady rejected: dependency cycle contains: {}",
                task_ids.join(", ")
            ),
            Self::MissingVerificationSignal { review_id } => write!(
                formatter,
                "PlanReady rejected: review {review_id} has no named verification signal"
            ),
            Self::MissingVerificationSignalClass { review_id } => write!(
                formatter,
                "PlanReady rejected: review {review_id} has no declared verification signal class"
            ),
            Self::InsufficientVerificationIsolation {
                review_id,
                signal_class,
                actual,
                required,
            } => write!(
                formatter,
                "PlanReady rejected: review {review_id} uses {actual:?} context for {signal_class:?} signal; requires {required:?} or stronger isolation"
            ),
            Self::InvalidReviewAuthorityMetadata { review_id, field } => write!(
                formatter,
                "PlanReady rejected: review {review_id} has invalid {field} metadata"
            ),
            Self::MissingAdjudicator { review_id } => write!(
                formatter,
                "PlanReady rejected: review {review_id} declares no adjudicator"
            ),
            Self::AdjudicatorLacksAuthority { review_id, role_id } => write!(
                formatter,
                "PlanReady rejected: review {review_id} adjudicator {role_id} lacks adjudication authority"
            ),
        }
    }
}

impl Error for PlanReadyError {}

/// Retain the first declaration of each task id and quarantine later ones.
pub fn quarantine_duplicate_nodes(graph: &mut TaskGraph) -> Vec<TaskId> {
    let mut seen = BTreeSet::new();
    let mut quarantined = Vec::new();

    graph.nodes.retain(|node| {
        if seen.insert(node.id.clone()) {
            true
        } else {
            quarantined.push(node.id.clone());
            false
        }
    });

    quarantined.sort();
    quarantined
}

/// Remove dependency edges whose source or target is absent from the graph.
/// The returned references preserve the operator-facing detail that would
/// otherwise be lost when the invalid edges are quarantined.
pub fn quarantine_dangling_dependencies(graph: &mut TaskGraph) -> Vec<DanglingDependency> {
    let node_ids = graph
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    let mut quarantined = Vec::new();

    graph.edges.retain(|edge| {
        let dangling = edge.kind == EdgeKind::DependsOn
            && (!node_ids.contains(&edge.source) || !node_ids.contains(&edge.target));
        if dangling {
            quarantined.push(DanglingDependency {
                dependent: edge.target.clone(),
                dependency: edge.source.clone(),
            });
        }
        !dangling
    });

    quarantined.sort_by(|left, right| {
        (&left.dependent, &left.dependency).cmp(&(&right.dependent, &right.dependency))
    });
    quarantined.dedup();
    quarantined
}

pub fn validate_plan_ready(graph: &TaskGraph) -> Result<PlanReadyValidation, PlanReadyError> {
    let mut counts = BTreeMap::<&str, usize>::new();
    for node in &graph.nodes {
        *counts.entry(&node.id).or_default() += 1;
    }
    let duplicate_ids = counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(id, _)| id.to_string())
        .collect::<Vec<_>>();
    if !duplicate_ids.is_empty() {
        return Err(PlanReadyError::DuplicateTaskIds {
            task_ids: duplicate_ids,
        });
    }

    let node_ids = graph
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut dangling = graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::DependsOn)
        .filter(|edge| {
            !node_ids.contains(edge.source.as_str()) || !node_ids.contains(edge.target.as_str())
        })
        .map(|edge| DanglingDependency {
            dependent: edge.target.clone(),
            dependency: edge.source.clone(),
        })
        .collect::<Vec<_>>();
    dangling.sort_by(|left, right| {
        (&left.dependent, &left.dependency).cmp(&(&right.dependent, &right.dependency))
    });
    dangling.dedup();
    if !dangling.is_empty() {
        return Err(PlanReadyError::DanglingDependencies {
            references: dangling,
        });
    }

    if let Err(cycle) = topological_sort(graph) {
        return Err(PlanReadyError::Cycle {
            task_ids: cycle.members,
        });
    }

    validate_review_authority(graph)?;

    let mut warnings = orphan_warnings(graph);
    warnings.extend(
        verification_duty_gaps(graph)
            .into_iter()
            .map(|gap| PlanReadyWarning::UnassignedVerificationDuty {
                task_class: gap.task_class,
                task_ids: gap.unassigned_task_ids,
            }),
    );
    Ok(PlanReadyValidation { warnings })
}

fn validate_review_authority(graph: &TaskGraph) -> Result<(), PlanReadyError> {
    for node in graph.nodes.iter().filter(|node| {
        node.kind == NodeKind::Join
            && node.expansion.as_ref().is_some_and(|expansion| {
                expansion.template == MULTI_LENS_REVIEW_TEMPLATE
            })
    }) {
        let expansion = node.expansion.as_ref().expect("review expansion checked");
        let duty = match expansion.parameters.get(VERIFICATION_DUTY_PARAMETER) {
            Some(encoded) => serde_json::from_str::<VerificationDuty>(encoded).map_err(|_| {
                PlanReadyError::InvalidReviewAuthorityMetadata {
                    review_id: node.id.clone(),
                    field: VERIFICATION_DUTY_PARAMETER.to_string(),
                }
            })?,
            None => VerificationDuty::default(),
        };
        if !verification_duty_has_named_signal(duty.signal_name.as_deref()) {
            return Err(PlanReadyError::MissingVerificationSignal {
                review_id: node.id.clone(),
            });
        }
        if !verification_duty_declares_signal_class(duty.signal_class) {
            return Err(PlanReadyError::MissingVerificationSignalClass {
                review_id: node.id.clone(),
            });
        }
        let signal_class = duty
            .signal_class
            .expect("the explicit signal class was checked above");
        let required = required_context_boundary(signal_class);
        if !context_boundary_satisfies(signal_class, duty.context_boundary) {
            return Err(PlanReadyError::InsufficientVerificationIsolation {
                review_id: node.id.clone(),
                signal_class,
                actual: duty.context_boundary,
                required,
            });
        }

        let declaration = expansion
            .parameters
            .get(ADJUDICATION_DECLARATION_PARAMETER)
            .ok_or_else(|| PlanReadyError::MissingAdjudicator {
                review_id: node.id.clone(),
            })
            .and_then(|encoded| {
                serde_json::from_str::<AdjudicationDeclaration>(encoded).map_err(|_| {
                    PlanReadyError::InvalidReviewAuthorityMetadata {
                        review_id: node.id.clone(),
                        field: ADJUDICATION_DECLARATION_PARAMETER.to_string(),
                    }
                })
            })?;
        let adjudicator = declaration
            .adjudicator
            .filter(|adjudicator| !adjudicator.role_id.trim().is_empty())
            .ok_or_else(|| PlanReadyError::MissingAdjudicator {
                review_id: node.id.clone(),
            })?;
        if !adjudicator.authority.may_adjudicate {
            return Err(PlanReadyError::AdjudicatorLacksAuthority {
                review_id: node.id.clone(),
                role_id: adjudicator.role_id,
            });
        }
    }
    Ok(())
}

fn orphan_warnings(graph: &TaskGraph) -> Vec<PlanReadyWarning> {
    let dependency_edges = graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::DependsOn)
        .collect::<Vec<_>>();
    if dependency_edges.is_empty() || graph.nodes.is_empty() {
        return Vec::new();
    }

    let mut adjacent = graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), Vec::<TaskId>::new()))
        .collect::<BTreeMap<_, _>>();
    for edge in dependency_edges {
        if adjacent.contains_key(&edge.source) && adjacent.contains_key(&edge.target) {
            adjacent
                .get_mut(&edge.source)
                .expect("known source")
                .push(edge.target.clone());
            adjacent
                .get_mut(&edge.target)
                .expect("known target")
                .push(edge.source.clone());
        }
    }

    let first = graph.nodes[0].id.clone();
    let mut reachable = BTreeSet::new();
    let mut pending = VecDeque::from([first]);
    while let Some(current) = pending.pop_front() {
        if !reachable.insert(current.clone()) {
            continue;
        }
        if let Some(neighbors) = adjacent.get(&current) {
            pending.extend(neighbors.iter().cloned());
        }
    }

    let orphan_ids = graph
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .filter(|id| !reachable.contains(id))
        .collect::<Vec<_>>();
    if orphan_ids.is_empty() {
        Vec::new()
    } else {
        vec![PlanReadyWarning::OrphanSubtrees {
            task_ids: orphan_ids,
        }]
    }
}
