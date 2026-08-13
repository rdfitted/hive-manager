//! Deterministic Kahn topological sorting for dependency edges.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::error::Error;
use std::fmt;

use super::schema::{EdgeKind, TaskGraph, TaskId};

/// Exact graph members that participate in at least one dependency cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleError {
    pub members: Vec<TaskId>,
}

impl fmt::Display for CycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "dependency cycle contains: {}",
            self.members.join(", ")
        )
    }
}

impl Error for CycleError {}

/// Sort the graph's nodes by `depends_on` relationships.
///
/// `source` is treated as the prerequisite and `target` as its dependent. Other
/// edge kinds are descriptive and do not participate. Unknown endpoints are
/// ignored here so the PlanReady validator can report them as dangling rather
/// than mislabeling them as a cycle.
pub fn topological_sort(graph: &TaskGraph) -> Result<Vec<TaskId>, CycleError> {
    let mut indegree: BTreeMap<TaskId, usize> = graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), 0))
        .collect();
    let mut dependents: BTreeMap<TaskId, Vec<TaskId>> = indegree
        .keys()
        .cloned()
        .map(|id| (id, Vec::new()))
        .collect();

    for edge in graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::DependsOn)
    {
        if indegree.contains_key(&edge.source) && indegree.contains_key(&edge.target) {
            *indegree
                .get_mut(&edge.target)
                .expect("known target has an indegree entry") += 1;
            dependents
                .get_mut(&edge.source)
                .expect("known source has a dependents entry")
                .push(edge.target.clone());
        }
    }
    for targets in dependents.values_mut() {
        targets.sort();
    }

    let mut ready: BTreeSet<TaskId> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| id.clone())
        .collect();
    let mut order = Vec::with_capacity(indegree.len());

    while let Some(id) = ready.iter().next().cloned() {
        ready.remove(&id);
        order.push(id.clone());

        for target in dependents
            .get(&id)
            .expect("every node has a dependents entry")
        {
            let degree = indegree
                .get_mut(target)
                .expect("known target has an indegree entry");
            *degree -= 1;
            if *degree == 0 {
                ready.insert(target.clone());
            }
        }
    }

    if order.len() == indegree.len() {
        return Ok(order);
    }

    let residual: BTreeSet<TaskId> = indegree
        .iter()
        .filter(|(_, degree)| **degree > 0)
        .map(|(id, _)| id.clone())
        .collect();
    let members = residual
        .iter()
        .filter(|id| participates_in_cycle(id, &residual, &dependents))
        .cloned()
        .collect();

    Err(CycleError { members })
}

fn participates_in_cycle(
    start: &str,
    residual: &BTreeSet<TaskId>,
    dependents: &BTreeMap<TaskId, Vec<TaskId>>,
) -> bool {
    let mut pending: Vec<&str> = dependents
        .get(start)
        .into_iter()
        .flatten()
        .filter(|target| residual.contains(target.as_str()))
        .map(String::as_str)
        .collect();
    let mut visited = HashSet::new();

    while let Some(current) = pending.pop() {
        if current == start {
            return true;
        }
        if !visited.insert(current) {
            continue;
        }
        pending.extend(
            dependents
                .get(current)
                .into_iter()
                .flatten()
                .filter(|target| residual.contains(target.as_str()))
                .map(String::as_str),
        );
    }

    false
}
