//! Conversion from the backward-compatible markdown plan model into a work graph.

use crate::actions::coordination::SessionPlan;

use super::schema::{
    BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus, TaskGraph,
    WorkEdge, WorkNode,
};

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
    let graph_tasks = plan
        .tasks
        .iter()
        .filter(|task| !has_explicit_graph || task.explicit_id)
        .collect::<Vec<_>>();
    let nodes = plan
        .tasks
        .iter()
        .filter(|task| !has_explicit_graph || task.explicit_id)
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
        .into_iter()
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

    TaskGraph::new(nodes, edges)
}
