//! Work-graph schema tests for issue #210, owned by WS-1.

use std::collections::BTreeMap;

use tempfile::TempDir;

use crate::coordination::StateManager;
use crate::orchestrator::work_graph::{
    topological_sort, BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract,
    NodeKind, NodeStatus, TaskGraph, WorkEdge, WorkGraphOmission, WorkGraphOmissionReason,
    WorkNode,
};

fn node(id: &str) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Task,
        format!("Task {id}"),
        NodeContract::default(),
        BindingRef::Role("backend".to_string()),
        NodeStatus::Pending,
    )
}

fn dependency(source: &str, target: &str) -> WorkEdge {
    WorkEdge::new(
        source,
        target,
        EdgeKind::DependsOn,
        EdgeProvenance::Planner,
    )
}

#[test]
fn work_graph_serde_round_trip_preserves_typed_contract() {
    let mut parameters = BTreeMap::new();
    parameters.insert("lenses".to_string(), "security,correctness".to_string());
    let mut review = WorkNode::new(
        "review-api",
        NodeKind::Review,
        "Review the API",
        NodeContract {
            inputs: vec!["implementation".to_string()],
            outputs: vec!["verdict".to_string()],
            acceptance: vec!["all checks pass".to_string()],
        },
        BindingRef::Zone("review".to_string()),
        NodeStatus::Ready,
    );
    review.expansion = Some(CompositeExpansion {
        template: "multi-lens-review".to_string(),
        parameters,
    });

    let mut graph = TaskGraph::new(
        vec![node("implement-api"), review],
        vec![WorkEdge::new(
            "implement-api",
            "review-api",
            EdgeKind::Reviews,
            EdgeProvenance::Runtime,
        )
        .with_rationale("review follows implementation")],
    );
    graph.omissions.push(WorkGraphOmission::new(
        WorkGraphOmissionReason::CodegraphUnavailable,
        1,
        vec!["repository-index".to_string()],
    ));

    let json = serde_json::to_string(&graph).unwrap();
    let decoded: TaskGraph = serde_json::from_str(&json).unwrap();

    assert_eq!(decoded, graph);
    assert!(decoded.has_omissions());
    assert!(json.contains(r#""kind":"review""#));
    assert!(json.contains(r#""provenance":"runtime""#));
    assert!(json.contains(r#""reason":"codegraph_unavailable""#));
}

#[test]
fn edge_provenance_is_required_and_partitionable() {
    let provenances = [
        EdgeProvenance::Planner,
        EdgeProvenance::Codegraph,
        EdgeProvenance::Knowledge,
        EdgeProvenance::Runtime,
    ];
    let edges: Vec<_> = provenances
        .into_iter()
        .map(|provenance| WorkEdge::new("a", "b", EdgeKind::Informs, provenance))
        .collect();

    for provenance in provenances {
        assert_eq!(
            edges
                .iter()
                .filter(|edge| edge.provenance == provenance)
                .count(),
            1
        );
    }

    let missing = r#"{"source":"a","target":"b","kind":"depends_on"}"#;
    assert!(serde_json::from_str::<WorkEdge>(missing).is_err());

    assert_eq!(
        serde_json::to_string(&EdgeKind::DependsOn).unwrap(),
        r#""depends_on""#
    );
    assert_eq!(
        serde_json::to_string(&EdgeProvenance::Codegraph).unwrap(),
        r#""codegraph""#
    );
    assert_eq!(
        serde_json::to_string(&NodeKind::Checkpoint).unwrap(),
        r#""checkpoint""#
    );
}

#[test]
fn topological_sort_is_deterministic_for_a_diamond() {
    assert!(topological_sort(&TaskGraph::default()).unwrap().is_empty());

    let graph = TaskGraph::new(
        vec![node("d"), node("c"), node("b"), node("a")],
        vec![
            dependency("a", "b"),
            dependency("a", "c"),
            dependency("b", "d"),
            dependency("c", "d"),
            WorkEdge::new(
                "d",
                "a",
                EdgeKind::Informs,
                EdgeProvenance::Knowledge,
            ),
        ],
    );

    assert_eq!(topological_sort(&graph).unwrap(), vec!["a", "b", "c", "d"]);
}

#[test]
fn topological_sort_returns_only_actual_cycle_members() {
    let graph = TaskGraph::new(
        vec![node("a"), node("b"), node("downstream")],
        vec![
            dependency("a", "b"),
            dependency("b", "a"),
            dependency("b", "downstream"),
        ],
    );

    let error = topological_sort(&graph).unwrap_err();
    assert_eq!(error.members, vec!["a", "b"]);

    let self_loop = TaskGraph::new(vec![node("self")], vec![dependency("self", "self")]);
    assert_eq!(
        topological_sort(&self_loop).unwrap_err().members,
        vec!["self"]
    );
}

#[test]
fn state_manager_persists_work_graph_atomically_and_distinguishes_absence() {
    let temp = TempDir::new().unwrap();
    let manager = StateManager::new(temp.path().to_path_buf());
    assert_eq!(manager.read_work_graph().unwrap(), None);

    let first = TaskGraph::new(vec![node("first")], vec![]);
    manager.write_work_graph(&first).unwrap();
    assert_eq!(manager.read_work_graph().unwrap(), Some(first));

    let second = TaskGraph::new(vec![node("second")], vec![]);
    manager.write_work_graph(&second).unwrap();
    assert_eq!(manager.read_work_graph().unwrap(), Some(second));

    let state_dir = temp.path().join("state");
    assert_eq!(state_dir.read_dir().unwrap().count(), 1);
}
