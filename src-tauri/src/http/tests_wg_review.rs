//! Review-subgraph tests for issue #213, owned by WS-6.

use std::collections::BTreeSet;

use tempfile::TempDir;

use crate::coordination::StateManager;
use crate::http::handlers::evaluator::{
    apply_work_graph_verdict, PostVerdictRequest, WorkGraphVerdictError,
    WorkGraphVerdictRouting,
};
use crate::orchestrator::work_graph::review::{
    checkpoint_aware_claimable_nodes, instantiate_checkpoint_wave,
    instantiate_review_templates, route_failed_verdict, CheckpointWave, ReviewExpansionSidecar,
    ReviewLens, ReviewTemplate, JUDGE_PRINCE_REMEDIATION_TEMPLATE,
};
use crate::orchestrator::work_graph::runtime::{mutation_log, GraphMutationType};
use crate::orchestrator::work_graph::{
    topological_sort, BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind,
    NodeStatus, TaskGraph, WorkGraphOmissionReason, WorkNode,
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

#[test]
fn legacy_verdict_request_defaults_explicit_graph_id_to_none() {
    let request: PostVerdictRequest = serde_json::from_str(r#"{"verdict":"PASS"}"#).unwrap();
    assert_eq!(request.work_graph_verdict_id, None);
}

#[test]
fn production_pass_records_only_the_explicit_review_verdict() {
    let (temp, manager, session_id, verdict_id) = persisted_review_fixture();
    let routing = apply_work_graph_verdict(
        &manager,
        &session_id,
        Some(&verdict_id),
        "PASS",
    )
    .unwrap();
    assert!(matches!(
        routing,
        WorkGraphVerdictRouting::Passed {
            ref verdict_id,
            delta_sequence: Some(1),
        } if verdict_id == &persisted_verdict_id(&manager)
    ));

    let graph = manager.read_work_graph().unwrap().unwrap();
    assert_eq!(
        graph.nodes.iter().find(|node| node.id == verdict_id).unwrap().status,
        NodeStatus::Completed
    );
    assert_eq!(mutation_log(&session_id).len(), 1);
    assert_eq!(
        mutation_log(&session_id)[0].mutation_type,
        GraphMutationType::ReviewVerdictRecorded
    );
    drop(temp);
}

#[test]
fn production_fail_persists_remediation_re_review_and_runtime_delta() {
    let (_temp, manager, session_id, verdict_id) = persisted_review_fixture();
    let routing = apply_work_graph_verdict(
        &manager,
        &session_id,
        Some(&verdict_id),
        "FAIL",
    )
    .unwrap();
    let (next_verdict_id, remediation_id) = match routing {
        WorkGraphVerdictRouting::FailedRouted {
            verdict_id: routed,
            next_verdict_id,
            remediation_id,
            delta_sequence: Some(1),
        } => {
            assert_eq!(routed, verdict_id);
            (next_verdict_id, remediation_id)
        }
        other => panic!("unexpected FAIL routing: {other:?}"),
    };

    let graph = manager.read_work_graph().unwrap().unwrap();
    assert_eq!(
        graph.nodes.iter().find(|node| node.id == verdict_id).unwrap().status,
        NodeStatus::Failed
    );
    assert!(graph.nodes.iter().any(|node| node.id == remediation_id));
    assert!(graph.nodes.iter().any(|node| node.id == next_verdict_id));
    assert!(graph.edges.iter().any(|edge| {
        edge.source == verdict_id
            && edge.target == remediation_id
            && edge.kind == EdgeKind::DependsOn
            && edge.provenance == EdgeProvenance::Runtime
    }));

    let sidecar = manager.read_review_expansion_sidecar().unwrap().unwrap();
    let record = sidecar.record_for_verdict(&next_verdict_id).unwrap();
    assert_eq!(record.expansion.rounds.len(), 2);
    assert_eq!(record.expansion.remediation_ids, vec![remediation_id]);
    let deltas = mutation_log(&session_id);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].mutation_type, GraphMutationType::RemediationDetour);
    assert!(deltas[0].after.nodes.len() > deltas[0].before.nodes.len());
}

#[test]
fn absent_graph_verdict_id_never_guesses_and_emits_an_omission() {
    let (_temp, manager, session_id, _verdict_id) = persisted_review_fixture();
    let before_graph = manager.read_work_graph().unwrap().unwrap();
    let before_sidecar = manager.read_review_expansion_sidecar().unwrap().unwrap();

    let routing = apply_work_graph_verdict(&manager, &session_id, None, "FAIL").unwrap();
    assert_eq!(
        routing,
        WorkGraphVerdictRouting::OmittedMissingVerdictId {
            omission_persisted: true,
        }
    );
    let after_graph = manager.read_work_graph().unwrap().unwrap();
    assert_eq!(after_graph.nodes, before_graph.nodes, "no verdict node may be guessed");
    assert_eq!(after_graph.edges, before_graph.edges, "no remediation may be guessed");
    assert!(after_graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ResolutionIncomplete
            && omission
                .examples
                .iter()
                .any(|example| example == "qa-verdict:missing-work-graph-verdict-id")
    }));
    assert_eq!(
        manager.read_review_expansion_sidecar().unwrap().unwrap(),
        before_sidecar
    );
    assert!(mutation_log(&session_id).is_empty());
}

#[test]
fn unknown_graph_verdict_id_is_rejected_without_mutation() {
    let (_temp, manager, session_id, _verdict_id) = persisted_review_fixture();
    let before_graph = manager.read_work_graph().unwrap().unwrap();
    let before_sidecar = manager.read_review_expansion_sidecar().unwrap().unwrap();
    let error = apply_work_graph_verdict(
        &manager,
        &session_id,
        Some("not-a-real-review-verdict"),
        "PASS",
    )
    .unwrap_err();
    assert_eq!(
        error,
        WorkGraphVerdictError::UnknownVerdict("not-a-real-review-verdict".to_string())
    );
    assert_eq!(manager.read_work_graph().unwrap().unwrap(), before_graph);
    assert_eq!(
        manager.read_review_expansion_sidecar().unwrap().unwrap(),
        before_sidecar
    );
    assert!(mutation_log(&session_id).is_empty());
}

fn persisted_review_fixture() -> (TempDir, StateManager, String, String) {
    let temp = TempDir::new().unwrap();
    let manager = StateManager::new(temp.path().to_path_buf());
    let session_id = format!("review-http-{}", uuid::Uuid::new_v4());
    let mut graph = TaskGraph::new(
        vec![node(
            "implement-api",
            NodeKind::Task,
            &["code"],
            NodeStatus::Pending,
        )],
        vec![],
    );
    let template = ReviewTemplate::code_tasks("qa-http");
    let expansions = instantiate_review_templates(&mut graph, &[template.clone()]).unwrap();
    let verdict_id = expansions[0].rounds[0].verdict_id.clone();
    let sidecar = ReviewExpansionSidecar::from_expansions(&[template], expansions).unwrap();
    manager.write_work_graph(&graph).unwrap();
    manager.write_review_expansion_sidecar(&sidecar).unwrap();
    (temp, manager, session_id, verdict_id)
}

fn persisted_verdict_id(manager: &StateManager) -> String {
    manager
        .read_review_expansion_sidecar()
        .unwrap()
        .unwrap()
        .records[0]
        .expansion
        .rounds[0]
        .verdict_id
        .clone()
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
