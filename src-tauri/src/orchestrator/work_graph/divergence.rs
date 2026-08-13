//! Neutral plan-versus-runtime divergence data for issue #214, owned by WS-5.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use super::runtime::{
    structural_projection, GraphMutationDelta, GraphMutationType,
};
use super::schema::{EdgeKind, TaskGraph, TaskId, WorkEdge, WorkGraph, WorkNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DivergenceKind {
    NodeAdded,
    NodeRemoved,
    NodeRestructured,
    EdgeAdded,
    EdgeRemoved,
    EdgeRewired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DivergenceRecord {
    pub kind: DivergenceKind,
    pub node_id: Option<TaskId>,
    pub source: Option<TaskId>,
    pub target: Option<TaskId>,
    pub replacement_source: Option<TaskId>,
    pub replacement_target: Option<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DivergenceSummary {
    pub counts_by_mutation_type: BTreeMap<DivergenceKind, usize>,
    pub recorded_runtime_mutations: BTreeMap<GraphMutationType, usize>,
    pub records: Vec<DivergenceRecord>,
}

impl DivergenceSummary {
    pub fn count(&self, kind: DivergenceKind) -> usize {
        self.counts_by_mutation_type
            .get(&kind)
            .copied()
            .unwrap_or(0)
    }
}

pub fn compute_divergence(
    plan_graph: Option<&TaskGraph>,
    runtime_graph: &WorkGraph,
    deltas: &[GraphMutationDelta],
) -> DivergenceSummary {
    let mut summary = DivergenceSummary::default();

    for delta in deltas {
        *summary
            .recorded_runtime_mutations
            .entry(delta.mutation_type)
            .or_insert(0) += 1;
    }

    // Without a plan there is no comparison baseline. Runtime structure is not
    // evidence that every node was added, so report only observed mutations.
    let Some(plan) = plan_graph else {
        return summary;
    };
    let actual = structural_projection(runtime_graph);

    let plan_nodes: BTreeMap<_, _> = plan
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let actual_nodes: BTreeMap<_, _> = actual
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    for (id, node) in &plan_nodes {
        match actual_nodes.get(id) {
            None => push_node_record(&mut summary, DivergenceKind::NodeRemoved, id),
            Some(actual_node) if !structurally_equal_node(node, actual_node) => {
                push_node_record(&mut summary, DivergenceKind::NodeRestructured, id)
            }
            Some(_) => {}
        }
    }
    for id in actual_nodes.keys() {
        if !plan_nodes.contains_key(id) {
            push_node_record(&mut summary, DivergenceKind::NodeAdded, id);
        }
    }

    let plan_edges = structural_edges(&plan.edges);
    let actual_edges = structural_edges(&actual.edges);
    let mut removed: Vec<_> = plan_edges.difference(&actual_edges).cloned().collect();
    let mut added: Vec<_> = actual_edges.difference(&plan_edges).cloned().collect();
    removed.sort_by(edge_key_order);
    added.sort_by(edge_key_order);

    let mut removed_index = 0;
    while removed_index < removed.len() {
        let old = removed[removed_index].clone();
        if let Some(new_index) = added.iter().position(|new| {
            old.2 == new.2 && (old.0 == new.0 || old.1 == new.1)
        }) {
            let new = added.remove(new_index);
            removed.remove(removed_index);
            push_record(
                &mut summary,
                DivergenceRecord {
                    kind: DivergenceKind::EdgeRewired,
                    node_id: None,
                    source: Some(old.0),
                    target: Some(old.1),
                    replacement_source: Some(new.0),
                    replacement_target: Some(new.1),
                },
            );
        } else {
            removed_index += 1;
        }
    }

    for (source, target, _) in removed {
        push_record(
            &mut summary,
            DivergenceRecord {
                kind: DivergenceKind::EdgeRemoved,
                node_id: None,
                source: Some(source),
                target: Some(target),
                replacement_source: None,
                replacement_target: None,
            },
        );
    }
    for (source, target, _) in added {
        push_record(
            &mut summary,
            DivergenceRecord {
                kind: DivergenceKind::EdgeAdded,
                node_id: None,
                source: Some(source),
                target: Some(target),
                replacement_source: None,
                replacement_target: None,
            },
        );
    }

    summary
}

fn structurally_equal_node(left: &WorkNode, right: &WorkNode) -> bool {
    left.id == right.id
        && left.kind == right.kind
        && left.title == right.title
        && left.contract == right.contract
        && left.binding == right.binding
        && left.expansion == right.expansion
}

fn structural_edges(
    edges: &[WorkEdge],
) -> HashSet<(String, String, EdgeKind)> {
    edges
        .iter()
        .map(|edge| (edge.source.clone(), edge.target.clone(), edge.kind))
        .collect()
}

fn edge_key_order(
    left: &(String, String, EdgeKind),
    right: &(String, String, EdgeKind),
) -> std::cmp::Ordering {
    left.0
        .cmp(&right.0)
        .then_with(|| left.1.cmp(&right.1))
        .then_with(|| edge_kind_rank(left.2).cmp(&edge_kind_rank(right.2)))
}

fn edge_kind_rank(kind: EdgeKind) -> u8 {
    match kind {
        EdgeKind::DependsOn => 0,
        EdgeKind::Produces => 1,
        EdgeKind::Consumes => 2,
        EdgeKind::Reviews => 3,
        EdgeKind::Informs => 4,
        EdgeKind::Touches => 5,
    }
}

fn push_node_record(summary: &mut DivergenceSummary, kind: DivergenceKind, id: &str) {
    push_record(
        summary,
        DivergenceRecord {
            kind,
            node_id: Some(id.to_string()),
            source: None,
            target: None,
            replacement_source: None,
            replacement_target: None,
        },
    );
}

fn push_record(summary: &mut DivergenceSummary, record: DivergenceRecord) {
    *summary
        .counts_by_mutation_type
        .entry(record.kind)
        .or_insert(0) += 1;
    summary.records.push(record);
}
