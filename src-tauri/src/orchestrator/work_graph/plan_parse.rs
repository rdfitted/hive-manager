//! Conversion from the backward-compatible markdown plan model into a work graph.

use crate::actions::coordination::SessionPlan;

use super::schema::{
    BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus, TaskGraph, TaskId,
    WorkEdge, WorkGraphOmission, WorkGraphOmissionReason, WorkNode,
};
use super::review::JUDGE_PRINCE_REMEDIATION_TEMPLATE;

/// Convert parsed plan tasks into schedulable nodes and planner-provenance edges.
///
/// `depends_on` is stored on the dependent task, while `DependsOn` graph edges
/// point from prerequisite (`source`) to dependent (`target`).
pub fn task_graph_from_plan(plan: &SessionPlan) -> TaskGraph {
    // New graph-emitting plans declare stable IDs. Once any such declaration is
    // present, ordinary bullets/numbered instructions retained by the legacy UI
    // parser are descriptive rather than schedulable. Plans with no explicit IDs
    // retain positional `task-N` nodes for backward compatibility.
    let has_explicit_graph = plan.tasks.iter().any(|task| task.explicit_id);
    let mut seen_task_ids = std::collections::BTreeSet::new();
    let mut duplicate_task_ids = Vec::new();
    let graph_tasks = plan
        .tasks
        .iter()
        .filter(|task| !has_explicit_graph || task.explicit_id)
        .filter(|task| {
            if seen_task_ids.insert(task.id.clone()) {
                true
            } else {
                duplicate_task_ids.push(task.id.clone());
                false
            }
        })
        .collect::<Vec<_>>();
    let nodes = graph_tasks
        .iter()
        .map(|task| {
            let status = if task.status == "completed" {
                NodeStatus::Completed
            } else {
                NodeStatus::Pending
            };
            WorkNode::new(
                task.id.clone(),
                NodeKind::Task,
                task.title.clone(),
                NodeContract {
                    inputs: task.inputs.clone(),
                    outputs: task.outputs.clone(),
                    acceptance: task.acceptance.clone(),
                },
                BindingRef::Role(
                    task.assignee
                        .clone()
                        .unwrap_or_else(|| "unassigned".to_string()),
                ),
                status,
            )
        })
        .collect();

    let edges = graph_tasks
        .iter()
        .flat_map(|task| {
            task.depends_on.iter().map(move |dependency| {
                WorkEdge::new(
                    dependency.clone(),
                    task.id.clone(),
                    EdgeKind::DependsOn,
                    EdgeProvenance::Planner,
                )
            })
        })
        .collect();

    let mut graph = TaskGraph::new(nodes, edges);
    if !duplicate_task_ids.is_empty() {
        duplicate_task_ids.sort();
        let examples = duplicate_task_ids
            .iter()
            .map(|task_id| {
                format!("duplicate task {task_id} was omitted after its first declaration")
            })
            .collect();
        graph.omissions.push(WorkGraphOmission::new(
            WorkGraphOmissionReason::ResolutionIncomplete,
            duplicate_task_ids.len(),
            examples,
        ));
    }
    graph
}

/// Promote only the initial plan-time frontier after every structural stage
/// has been stamped. Runtime remediation is excluded even if a retry invokes
/// reconciliation after its prerequisite verdict completed.
pub fn promote_initial_ready_nodes(graph: &mut TaskGraph) -> Vec<TaskId> {
    let completed: std::collections::BTreeSet<_> = graph
        .nodes
        .iter()
        .filter(|node| node.status == NodeStatus::Completed)
        .map(|node| node.id.clone())
        .collect();
    let ready_ids: std::collections::BTreeSet<_> = graph
        .nodes
        .iter()
        .filter(|node| node.status == NodeStatus::Pending)
        .filter(|node| {
            !node.expansion.as_ref().is_some_and(|expansion| {
                expansion.template == JUDGE_PRINCE_REMEDIATION_TEMPLATE
            })
        })
        .filter(|node| {
            graph
                .edges
                .iter()
                .filter(|edge| edge.kind == EdgeKind::DependsOn && edge.target == node.id)
                .all(|edge| completed.contains(&edge.source))
        })
        .map(|node| node.id.clone())
        .collect();
    for node in &mut graph.nodes {
        if ready_ids.contains(&node.id) {
            node.status = NodeStatus::Ready;
        }
    }
    ready_ids.into_iter().collect()
}
