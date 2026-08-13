//! Validation performed before a planning session becomes `PlanReady`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use super::schema::{EdgeKind, TaskGraph, TaskId};
use super::toposort::topological_sort;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanReadyValidation {
    pub warnings: Vec<PlanReadyWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanReadyWarning {
    OrphanSubtrees { task_ids: Vec<TaskId> },
}

impl fmt::Display for PlanReadyWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OrphanSubtrees { task_ids } => write!(
                formatter,
                "orphan task subtrees are disconnected from the first plan component: {}",
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
        }
    }
}

impl Error for PlanReadyError {}

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

    Ok(PlanReadyValidation {
        warnings: orphan_warnings(graph),
    })
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
