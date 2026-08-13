//! Review-subgraph tests for issue #213, owned by WS-6.

use std::collections::BTreeSet;

use crate::orchestrator::work_graph::review::{
    checkpoint_aware_claimable_nodes, instantiate_checkpoint_wave,
    instantiate_review_templates, route_failed_verdict, CheckpointWave, ReviewLens, ReviewTemplate,
    JUDGE_PRINCE_REMEDIATION_TEMPLATE,
};
use crate::orchestrator::work_graph::{
    topological_sort, BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind,
    NodeStatus, TaskGraph, WorkNode,
};
use crate::session::SessionState;

fn node(
    id: &str,
    kind: NodeKind,
    outputs: &[&str],
    status: NodeStatus,
) -> WorkNode {
    WorkNode::new(
        id,
        kind,
        format!("Node {id}"),
        NodeContract {
            inputs: Vec::new(),
            outputs: outputs.iter().map(|output| (*output).to_string()).collect(),
            acceptance: vec![format!("{id} is accepted")],
        },
        BindingRef::Role("backend".to_string()),
        status,
    )
}

#[test]
fn node_class_template_automatically_instantiates_review_subgraph() {
    let mut graph = TaskGraph::new(
        vec![
            node("implement-api", NodeKind::Task, &["code"], NodeStatus::Pending),
            node("write-brief", NodeKind::Task, &["document"], NodeStatus::Pending),
            node("context", NodeKind::Context, &["code"], NodeStatus::Pending),
        ],
        vec![],
    );
    let unchanged = graph.clone();
    assert!(instantiate_review_templates(&mut graph, &[]).unwrap().is_empty());
    assert_eq!(graph, unchanged, "a plan without templates must remain unchanged");

    let template = ReviewTemplate::code_tasks("code-quality");
    let expansions = instantiate_review_templates(&mut graph, &[template]).unwrap();

    assert_eq!(expansions.len(), 1, "only the matching Task+code class expands");
    assert_eq!(expansions[0].target_id, "implement-api");
    assert_eq!(expansions[0].rounds[0].lens_ids.len(), 3);
    assert!(!graph
        .nodes
        .iter()
        .any(|node| node.id.starts_with("write-brief::review")));
    assert!(!graph
        .nodes
        .iter()
        .any(|node| node.id.starts_with("context::review")));

    for lens_id in &expansions[0].rounds[0].lens_ids {
        assert!(graph.edges.iter().any(|edge| {
            edge.source == "implement-api"
                && edge.target == *lens_id
                && edge.kind == EdgeKind::Reviews
                && edge.provenance == EdgeProvenance::Planner
        }));
    }
}

#[test]
fn failing_verdict_routes_through_bounded_remediation_and_re_review() {
    let mut graph = TaskGraph::new(
        vec![node(
            "implement-api",
            NodeKind::Task,
            &["code"],
            NodeStatus::Pending,
        )],
        vec![],
    );
    let template = ReviewTemplate::code_tasks("qa");
    let mut expansions = instantiate_review_templates(&mut graph, &[template.clone()]).unwrap();
    assert_eq!(expansions[0].rounds.len(), 1);
    assert!(expansions[0].remediation_ids.is_empty());
    route_failed_verdict(&mut graph, &template, &mut expansions[0]).unwrap();
    let expansion = &expansions[0];

    assert_eq!(expansion.rounds.len(), 2);
    assert_eq!(expansion.remediation_ids.len(), 1);
    let first_verdict = &expansion.rounds[0].verdict_id;
    let remediation = &expansion.remediation_ids[0];
    let second_round_lens = &expansion.rounds[1].lens_ids[0];

    assert!(graph.edges.iter().any(|edge| {
        edge.source == *first_verdict
            && edge.target == *remediation
            && edge.kind == EdgeKind::DependsOn
    }));
    assert!(graph.edges.iter().any(|edge| {
        edge.source == *remediation
            && edge.target == *second_round_lens
            && edge.kind == EdgeKind::Reviews
            && edge.provenance == EdgeProvenance::Planner
    }));
    assert!(graph.edges.iter().any(|edge| {
        edge.source == *remediation
            && edge.target == *second_round_lens
            && edge.kind == EdgeKind::DependsOn
    }));

    let remediation_node = graph
        .nodes
        .iter()
        .find(|node| node.id == *remediation)
        .unwrap();
    let metadata = remediation_node.expansion.as_ref().unwrap();
    assert_eq!(metadata.template, JUDGE_PRINCE_REMEDIATION_TEMPLATE);
    assert_eq!(metadata.parameters.get("activation").unwrap(), "verdict_fail");

    let order = topological_sort(&graph).expect("bounded expansion must remain schedulable");
    let position = |id: &str| order.iter().position(|candidate| candidate == id).unwrap();
    assert!(position(first_verdict) < position(remediation));
    assert!(position(remediation) < position(second_round_lens));

    let states = existing_session_state_samples();
    assert_eq!(states.len(), 28);
    assert!(states
        .iter()
        .all(|state| !existing_session_state_variant_name(state).is_empty()));
}

#[test]
fn checkpoint_withholds_then_releases_downstream_claim() {
    let mut graph = TaskGraph::new(
        vec![
            node("left", NodeKind::Task, &["code"], NodeStatus::Ready),
            node("right", NodeKind::Task, &["code"], NodeStatus::Ready),
            node("publish", NodeKind::Task, &["artifact"], NodeStatus::Ready),
        ],
        vec![],
    );
    let disabled = CheckpointWave::new(
        "review-wave",
        vec!["left".to_string(), "right".to_string()],
        vec!["publish".to_string()],
    );
    let unchanged = graph.clone();
    assert_eq!(instantiate_checkpoint_wave(&mut graph, &disabled).unwrap(), None);
    assert_eq!(graph, unchanged, "checkpoint barriers are opt-in");

    let wave = disabled.enabled();
    assert_eq!(
        instantiate_checkpoint_wave(&mut graph, &wave).unwrap(),
        Some("review-wave".to_string())
    );
    assert_eq!(
        checkpoint_aware_claimable_nodes(&graph),
        vec!["left", "right"],
        "the downstream claim must be withheld before its checkpoint passes"
    );

    set_status(&mut graph, "left", NodeStatus::Completed);
    set_status(&mut graph, "right", NodeStatus::Completed);
    assert_eq!(
        checkpoint_aware_claimable_nodes(&graph),
        vec!["review-wave"],
        "only the gate is claimable once every sibling completes"
    );

    set_status(&mut graph, "review-wave", NodeStatus::Completed);
    assert_eq!(
        checkpoint_aware_claimable_nodes(&graph),
        vec!["publish"],
        "passing the gate releases the observed downstream claim"
    );
}

#[test]
fn default_review_lenses_are_distinct_not_repeated_reviewers() {
    let template = ReviewTemplate::code_tasks("diverse-review");
    let focuses: BTreeSet<_> = template
        .lenses
        .iter()
        .map(|lens| lens.focus.as_str())
        .collect();
    assert_eq!(focuses.len(), template.lenses.len());
    assert_eq!(focuses.len(), 3);

    let mut duplicate = template.clone();
    duplicate.lenses.push(ReviewLens::new(
        "another-name",
        duplicate.lenses[0].focus.clone(),
    ));
    let mut graph = TaskGraph::new(
        vec![node("code", NodeKind::Task, &["code"], NodeStatus::Pending)],
        vec![],
    );
    let unchanged = graph.clone();
    let error = instantiate_review_templates(&mut graph, &[duplicate]).unwrap_err();
    assert!(error.to_string().contains("repeats lens focus"));
    assert_eq!(graph, unchanged, "invalid repeated lenses must not partially expand");
}

fn set_status(graph: &mut TaskGraph, id: &str, status: NodeStatus) {
    graph
        .nodes
        .iter_mut()
        .find(|node| node.id == id)
        .unwrap()
        .status = status;
}

fn existing_session_state_samples() -> Vec<SessionState> {
    vec![
        SessionState::Planning,
        SessionState::PlanReady,
        SessionState::Starting,
        SessionState::SpawningWorker(1),
        SessionState::WaitingForWorker(1),
        SessionState::SpawningPlanner(1),
        SessionState::WaitingForPlanner(1),
        SessionState::SpawningFusionVariant(1),
        SessionState::WaitingForFusionVariants,
        SessionState::SpawningDebateRound(1),
        SessionState::WaitingForDebateRound(1),
        SessionState::SpawningJudge,
        SessionState::Judging,
        SessionState::AwaitingVerdictSelection,
        SessionState::MergingWinner,
        SessionState::SpawningEvaluator,
        SessionState::QaInProgress { iteration: Some(1) },
        SessionState::QaPassed,
        SessionState::QaFailed { iteration: 1 },
        SessionState::QaMaxRetriesExceeded,
        SessionState::PrinceRemediation,
        SessionState::QaInconclusive,
        SessionState::Running,
        SessionState::Paused,
        SessionState::Completed,
        SessionState::Closing,
        SessionState::Closed,
        SessionState::Failed("failure".to_string()),
    ]
}

/// This deliberately exhaustive match is a compile-time tripwire: adding a
/// `SessionState` variant makes this owned acceptance test fail to compile.
fn existing_session_state_variant_name(state: &SessionState) -> &'static str {
    match state {
        SessionState::Planning => "Planning",
        SessionState::PlanReady => "PlanReady",
        SessionState::Starting => "Starting",
        SessionState::SpawningWorker(_) => "SpawningWorker",
        SessionState::WaitingForWorker(_) => "WaitingForWorker",
        SessionState::SpawningPlanner(_) => "SpawningPlanner",
        SessionState::WaitingForPlanner(_) => "WaitingForPlanner",
        SessionState::SpawningFusionVariant(_) => "SpawningFusionVariant",
        SessionState::WaitingForFusionVariants => "WaitingForFusionVariants",
        SessionState::SpawningDebateRound(_) => "SpawningDebateRound",
        SessionState::WaitingForDebateRound(_) => "WaitingForDebateRound",
        SessionState::SpawningJudge => "SpawningJudge",
        SessionState::Judging => "Judging",
        SessionState::AwaitingVerdictSelection => "AwaitingVerdictSelection",
        SessionState::MergingWinner => "MergingWinner",
        SessionState::SpawningEvaluator => "SpawningEvaluator",
        SessionState::QaInProgress { .. } => "QaInProgress",
        SessionState::QaPassed => "QaPassed",
        SessionState::QaFailed { .. } => "QaFailed",
        SessionState::QaMaxRetriesExceeded => "QaMaxRetriesExceeded",
        SessionState::PrinceRemediation => "PrinceRemediation",
        SessionState::QaInconclusive => "QaInconclusive",
        SessionState::Running => "Running",
        SessionState::Paused => "Paused",
        SessionState::Completed => "Completed",
        SessionState::Closing => "Closing",
        SessionState::Closed => "Closed",
        SessionState::Failed(_) => "Failed",
    }
}
