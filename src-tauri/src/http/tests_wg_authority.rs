use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use tempfile::TempDir;

use crate::coordination::StateManager;
use crate::orchestrator::org_graph::adjudication::{
    adjudicate_contradiction, AdjudicationDeclaration, AdjudicationPolicy,
    AdjudicationResolution, DeclaredAdjudicator, SourceVerdict, SourceVerdictValue,
};
use crate::orchestrator::org_graph::ownership::{
    orchestrator_nodes_for_plan, CollisionDisposition, LivePrincipal, OrchestratorOperation,
    OrchestratorOwnershipNode, OrchestratorRole, OrchestratorWriteAttempt,
    OrchestratorWriteOutcome, OwnershipSessionState,
};
use crate::orchestrator::org_graph::{ContextBoundary, SignalClass};
use crate::orchestrator::work_graph::archive::{archive_completed_session, read_archive};
use crate::orchestrator::work_graph::review::{
    instantiate_review_templates, ReviewTemplate, JUDGE_PRINCE_REMEDIATION_TEMPLATE,
};
use crate::orchestrator::work_graph::runtime::{
    route_contradictory_verdicts_and_record, route_failed_verdict_and_record,
    GraphMutationType,
};
use crate::orchestrator::work_graph::schema::{
    BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus,
    TaskGraph, WorkEdge, WorkNode,
};
use crate::orchestrator::work_graph::validate::{
    validate_plan_ready, PlanReadyError, PlanReadyWarning,
};
use crate::storage::SessionStorage;

const INCIDENT_PATH: &str = "src-tauri/src/session/controller.rs";
const W4_ID: &str = "d1e86179-de61-4b3a-8744-7c62a14ce4b2-worker-4";

fn incident_graph() -> TaskGraph {
    let owner = WorkNode::new(
        "T14/T15",
        NodeKind::Task,
        "W4 get_stalled_agents fix and mutation proof",
        NodeContract {
            inputs: Vec::new(),
            outputs: vec!["code".to_string()],
            acceptance: vec!["preserve the reassignment guard".to_string()],
        },
        BindingRef::Role("worker".to_string()),
        // The historical mirror said completed even while W4 remained write-capable.
        NodeStatus::Completed,
    );
    let mut parameters = BTreeMap::new();
    parameters.insert("module".to_string(), INCIDENT_PATH.to_string());
    let mut module = WorkNode::new(
        "codegraph::controller",
        NodeKind::Context,
        "Module controller.rs",
        NodeContract::default(),
        BindingRef::Zone("codegraph".to_string()),
        NodeStatus::Completed,
    );
    module.expansion = Some(CompositeExpansion {
        template: "codegraph-module".to_string(),
        parameters,
    });
    TaskGraph::new(
        vec![owner, module],
        vec![WorkEdge::new(
            "T14/T15",
            "codegraph::controller",
            EdgeKind::Touches,
            EdgeProvenance::Codegraph,
        )],
    )
}

fn declared_orchestrators() -> Vec<OrchestratorOwnershipNode> {
    orchestrator_nodes_for_plan(&incident_graph())
}

fn incident_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-14T14:23:37Z")
        .expect("historical incident timestamp")
        .with_timezone(&Utc)
}

fn review_fixture(
    template: &ReviewTemplate,
) -> (
    TaskGraph,
    crate::orchestrator::work_graph::review::ReviewExpansion,
) {
    let task = WorkNode::new(
        "implementation",
        NodeKind::Task,
        "Implementation",
        NodeContract {
            inputs: Vec::new(),
            outputs: vec!["code".to_string()],
            acceptance: vec!["implementation works".to_string()],
        },
        BindingRef::Role("worker".to_string()),
        NodeStatus::Pending,
    );
    let mut graph = TaskGraph::new(vec![task], Vec::new());
    let expansion = instantiate_review_templates(&mut graph, std::slice::from_ref(template))
        .expect("review expansion")
        .remove(0);
    (graph, expansion)
}

fn adjudication(policy: AdjudicationPolicy) -> AdjudicationDeclaration {
    AdjudicationDeclaration {
        policy,
        adjudicator: Some(DeclaredAdjudicator::new("prince")),
    }
}

fn contradictory_verdicts() -> Vec<SourceVerdict> {
    vec![
        SourceVerdict {
            source_id: "correctness".to_string(),
            verdict: SourceVerdictValue::Pass,
            rationale: "mechanism satisfies its contract".to_string(),
        },
        SourceVerdict {
            source_id: "security".to_string(),
            verdict: SourceVerdictValue::Fail,
            rationale: "authorization finding remains".to_string(),
        },
    ]
}

#[test]
fn orchestrator_nodes_serialize_footprint_separately_from_authority() {
    let state = OwnershipSessionState::from_plan(
        &incident_graph(),
        declared_orchestrators(),
        &[LivePrincipal {
            principal_id: W4_ID.to_string(),
            task_id: "T14/T15".to_string(),
            write_capable: true,
        }],
    )
    .expect("all orchestrators declare a footprint");
    let serialized = serde_json::to_value(&state).expect("ownership state serializes");
    let orchestrators = serialized["orchestrators"]
        .as_array()
        .expect("orchestrators are visible session state");

    assert_eq!(orchestrators.len(), 3);
    for role in ["queen", "prince", "evaluator"] {
        let node = orchestrators
            .iter()
            .find(|node| node["role"] == role)
            .unwrap_or_else(|| panic!("missing serialized {role} node"));
        assert!(
            !node["footprint"].as_array().expect("footprint array").is_empty(),
            "{role} footprint was erased"
        );
        assert!(node.get("authority").is_some(), "{role} authority missing");
        assert!(
            node.get("ownership_authority").is_some(),
            "{role} ownership authority missing"
        );
    }

    let queen = orchestrators
        .iter()
        .find(|node| node["role"] == "queen")
        .expect("queen node");
    assert!(queen["footprint"].as_array().expect("Queen footprint").iter().any(
        |entry| entry["path"] == INCIDENT_PATH && entry["operation"] == "restore"
    ));
    assert_eq!(queen["authority"]["may_commit"], true);
    assert_eq!(queen["ownership_authority"]["may_mutate_mid_flight"], true);
}

#[test]
fn d1e86179_queen_restore_to_live_principal_path_is_detected_and_surfaced() {
    let graph = incident_graph();
    let mut state = OwnershipSessionState::from_plan(
        &graph,
        declared_orchestrators(),
        &[LivePrincipal {
            principal_id: W4_ID.to_string(),
            task_id: "T14/T15".to_string(),
            // This is the liveness fact that the premature COMPLETED mirror lost.
            write_capable: true,
        }],
    )
    .expect("incident ownership state");

    // Historical interleaving: W4 wrote the correct guard, Queen mutated it for
    // proof and restored it, then still-live W4 wrote its stale mutant again.
    let correct_guard = "info.status != \"completed\" || reassigned_after_completion";
    let stale_mutant = "info.status != \"completed\"";
    let mut working_guard = correct_guard;
    assert_eq!(working_guard, correct_guard); // W4's correct T14 write.
    working_guard = stale_mutant; // Queen mutation proof.
    assert_eq!(working_guard, stale_mutant);
    working_guard = correct_guard; // Queen restore attempt must check ownership now.
    assert_eq!(working_guard, correct_guard);

    let outcome = state.record_write_attempt(OrchestratorWriteAttempt {
        actor_id: "queen".to_string(),
        role: OrchestratorRole::Queen,
        path: INCIDENT_PATH.to_string(),
        operation: OrchestratorOperation::Restore,
        attempted_at: incident_time(),
    });
    let OrchestratorWriteOutcome::Collision { collision } = outcome else {
        panic!("Queen restore raced a write-capable W4 but was allowed silently");
    };
    assert_eq!(collision.actor_id, "queen");
    assert_eq!(collision.actor_role, OrchestratorRole::Queen);
    assert_eq!(collision.owner_principal_id, W4_ID);
    assert_eq!(collision.owner_task_id, "T14/T15");
    assert!(collision.owner_write_capable);
    assert_eq!(collision.path, INCIDENT_PATH);
    assert_eq!(collision.operation, OrchestratorOperation::Restore);
    assert!(collision.within_declared_footprint);
    assert!(collision.override_authorized);
    assert_eq!(
        collision.disposition,
        CollisionDisposition::SurfacedAuthorizedOverride
    );
    assert_eq!(collision.detected_at, incident_time());
    assert_eq!(state.collisions, vec![collision.clone()]);

    working_guard = stale_mutant; // W4's post-restore write proves it was still live.
    assert_eq!(working_guard, stale_mutant);
    assert_eq!(
        serde_json::to_value(&state).expect("surfaced state")["collisions"][0]["path"],
        INCIDENT_PATH
    );
}

#[test]
fn ownership_sidecar_round_trips_and_prewrite_collision_is_durable() {
    let temp = TempDir::new().expect("ownership sidecar root");
    let manager = StateManager::new(temp.path().join("ownership-session"));
    let graph = incident_graph();
    let initial = OwnershipSessionState::from_plan(&graph, declared_orchestrators(), &[])
        .expect("initial ownership state");
    manager
        .write_ownership_session_state(&initial)
        .expect("persist visible ownership state");
    assert_eq!(
        manager
            .read_ownership_session_state()
            .expect("read sidecar"),
        Some(initial)
    );

    let outcome = manager
        .record_orchestrator_write_attempt(
            &graph,
            &[LivePrincipal {
                principal_id: W4_ID.to_string(),
                task_id: "T14/T15".to_string(),
                write_capable: true,
            }],
            OrchestratorWriteAttempt {
                actor_id: "queen".to_string(),
                role: OrchestratorRole::Queen,
                path: INCIDENT_PATH.to_string(),
                operation: OrchestratorOperation::Restore,
                attempted_at: incident_time(),
            },
        )
        .expect("pre-write check persists its result");
    assert!(matches!(outcome, OrchestratorWriteOutcome::Collision { .. }));
    let persisted = manager
        .read_ownership_session_state()
        .expect("read surfaced collision")
        .expect("ownership sidecar exists");
    assert_eq!(persisted.collisions.len(), 1);
    assert_eq!(persisted.collisions[0].path, INCIDENT_PATH);
    assert_eq!(persisted.collisions[0].owner_principal_id, W4_ID);
}

#[test]
fn inactive_owner_and_unowned_path_do_not_create_false_collisions() {
    let mut inactive = OwnershipSessionState::from_plan(
        &incident_graph(),
        declared_orchestrators(),
        &[LivePrincipal {
            principal_id: W4_ID.to_string(),
            task_id: "T14/T15".to_string(),
            write_capable: false,
        }],
    )
    .expect("inactive ownership state");
    let attempt = |path: &str| OrchestratorWriteAttempt {
        actor_id: "queen".to_string(),
        role: OrchestratorRole::Queen,
        path: path.to_string(),
        operation: OrchestratorOperation::Restore,
        attempted_at: incident_time(),
    };
    assert_eq!(
        inactive.record_write_attempt(attempt(INCIDENT_PATH)),
        OrchestratorWriteOutcome::Proceed
    );
    assert_eq!(
        inactive.record_write_attempt(attempt("src-tauri/src/lib.rs")),
        OrchestratorWriteOutcome::Proceed
    );
    assert!(inactive.collisions.is_empty());
}

#[test]
fn unassigned_task_class_verification_duty_is_reported_at_plan_ready() {
    let task = WorkNode::new(
        "implement-auth",
        NodeKind::Task,
        "Implement auth",
        NodeContract {
            inputs: Vec::new(),
            outputs: vec!["security_critical_code".to_string()],
            acceptance: Vec::new(),
        },
        BindingRef::Role("worker".to_string()),
        NodeStatus::Pending,
    );
    let mut graph = TaskGraph::new(vec![task], Vec::new());
    let validation = validate_plan_ready(&graph).expect("gap is a plan-time report");
    assert!(validation.warnings.contains(
        &PlanReadyWarning::UnassignedVerificationDuty {
            task_class: "output:security_critical_code".to_string(),
            task_ids: vec!["implement-auth".to_string()],
        }
    ));

    graph.nodes.push(WorkNode::new(
        "review-auth",
        NodeKind::Review,
        "Review auth",
        NodeContract::default(),
        BindingRef::Role("evaluator".to_string()),
        NodeStatus::Pending,
    ));
    graph.edges.push(WorkEdge::new(
        "implement-auth",
        "review-auth",
        EdgeKind::Reviews,
        EdgeProvenance::Planner,
    ));
    let assigned = validate_plan_ready(&graph).expect("assigned duty remains plan-ready");
    assert!(
        !assigned
            .warnings
            .iter()
            .any(|warning| matches!(warning, PlanReadyWarning::UnassignedVerificationDuty { .. })),
        "a real Reviews edge must satisfy the task class duty"
    );
}

#[test]
fn plan_ready_enforces_named_signal_class_and_judgmental_isolation_separately() {
    let mut missing_signal = ReviewTemplate::code_tasks("missing-signal");
    missing_signal.verification_duty.signal_name = Some("   ".to_string());
    let (graph, _) = review_fixture(&missing_signal);
    assert!(matches!(
        validate_plan_ready(&graph),
        Err(PlanReadyError::MissingVerificationSignal { .. })
    ));

    let mut missing_class = ReviewTemplate::code_tasks("missing-class");
    missing_class.verification_duty.signal_class = None;
    let (graph, _) = review_fixture(&missing_class);
    assert!(matches!(
        validate_plan_ready(&graph),
        Err(PlanReadyError::MissingVerificationSignalClass { .. })
    ));

    let mut contaminated_judgment = ReviewTemplate::code_tasks("contaminated-judgment");
    contaminated_judgment.verification_duty.context_boundary = ContextBoundary::Full;
    let (graph, _) = review_fixture(&contaminated_judgment);
    assert!(matches!(
        validate_plan_ready(&graph),
        Err(PlanReadyError::InsufficientVerificationIsolation {
            signal_class: SignalClass::Judgmental,
            actual: ContextBoundary::Full,
            required: ContextBoundary::Artifact,
            ..
        })
    ));

    let mut contaminated_mechanism = ReviewTemplate::code_tasks("contaminated-mechanism");
    contaminated_mechanism.verification_duty.signal_class = Some(SignalClass::Mechanical);
    contaminated_mechanism.verification_duty.context_boundary = ContextBoundary::Full;
    let (graph, _) = review_fixture(&contaminated_mechanism);
    validate_plan_ready(&graph).expect("mechanical signal may receive full spawner context");
}

#[test]
fn plan_ready_rejects_missing_or_unauthorized_adjudicator_without_queen_fallback() {
    let mut missing = ReviewTemplate::code_tasks("missing-adjudicator");
    missing.adjudication = None;
    let (graph, _) = review_fixture(&missing);
    assert!(matches!(
        validate_plan_ready(&graph),
        Err(PlanReadyError::MissingAdjudicator { .. })
    ));

    let mut unauthorized = ReviewTemplate::code_tasks("unauthorized-adjudicator");
    unauthorized
        .adjudication
        .as_mut()
        .expect("default declaration")
        .adjudicator
        .as_mut()
        .expect("default adjudicator")
        .authority
        .may_adjudicate = false;
    let (graph, _) = review_fixture(&unauthorized);
    assert!(matches!(
        validate_plan_ready(&graph),
        Err(PlanReadyError::AdjudicatorLacksAuthority { ref role_id, .. })
            if role_id == "prince"
    ));
}

#[test]
fn every_disagreement_policy_is_order_independent() {
    let two = contradictory_verdicts();
    let three = vec![
        two[0].clone(),
        two[1].clone(),
        SourceVerdict {
            source_id: "regression".to_string(),
            verdict: SourceVerdictValue::Pass,
            rationale: "compatibility remains intact".to_string(),
        },
    ];
    let cases = vec![
        (
            adjudication(AdjudicationPolicy::Consensus { required: 2 }),
            three,
            AdjudicationResolution::ConsensusPass,
        ),
        (
            adjudication(AdjudicationPolicy::Escalate),
            two.clone(),
            AdjudicationResolution::Escalated {
                role_id: "prince".to_string(),
            },
        ),
        (
            adjudication(AdjudicationPolicy::HumanGate),
            two.clone(),
            AdjudicationResolution::HumanGate,
        ),
        (
            adjudication(AdjudicationPolicy::BothAreFindings),
            two,
            AdjudicationResolution::Findings {
                source_ids: vec!["correctness".to_string(), "security".to_string()],
            },
        ),
    ];
    for (declaration, verdicts, expected) in cases {
        let forward = adjudicate_contradiction(&declaration, &verdicts).expect("policy routes");
        let mut reversed = verdicts.clone();
        reversed.reverse();
        let reverse =
            adjudicate_contradiction(&declaration, &reversed).expect("reverse policy routes");
        assert_eq!(forward, reverse, "arrival order changed adjudication");
        assert_eq!(forward.resolution, expected);
        assert_eq!(forward.source_verdicts.len(), verdicts.len());
    }
}

#[test]
fn contradictory_arrival_order_produces_the_same_runtime_graph() {
    let template = ReviewTemplate::code_tasks("order-independent");
    let (mut forward_graph, expansion) = review_fixture(&template);
    let mut reverse_graph = forward_graph.clone();
    let verdict_id = &expansion.rounds[0].verdict_id;
    let declaration = adjudication(AdjudicationPolicy::Escalate);
    let verdicts = contradictory_verdicts();
    let (forward, forward_delta) = route_contradictory_verdicts_and_record(
        "authority-order-forward",
        &mut forward_graph,
        verdict_id,
        &declaration,
        &verdicts,
    )
    .expect("forward contradiction");
    let mut reversed = verdicts.clone();
    reversed.reverse();
    let (reverse, reverse_delta) = route_contradictory_verdicts_and_record(
        "authority-order-reverse",
        &mut reverse_graph,
        verdict_id,
        &declaration,
        &reversed,
    )
    .expect("reverse contradiction");

    assert_eq!(forward, reverse);
    assert_eq!(forward_graph, reverse_graph);
    assert_eq!(
        forward_delta.expect("forward delta").mutation_type,
        GraphMutationType::ContradictionAdjudicated
    );
    assert_eq!(
        reverse_delta.expect("reverse delta").mutation_type,
        GraphMutationType::ContradictionAdjudicated
    );
}

#[test]
fn ordinary_failure_and_contradiction_take_distinct_graph_paths() {
    let template = ReviewTemplate::code_tasks("distinct-paths");
    let (mut failure_graph, mut failure_expansion) = review_fixture(&template);
    let (_, failure_delta) = route_failed_verdict_and_record(
        "authority-ordinary-failure",
        &mut failure_graph,
        &template,
        &mut failure_expansion,
    )
    .expect("ordinary failure routes to remediation");
    let failure_delta = failure_delta.expect("remediation changes graph");
    assert_eq!(
        failure_delta.mutation_type,
        GraphMutationType::RemediationDetour
    );
    assert!(failure_graph.nodes.iter().any(|node| {
        node.expansion.as_ref().is_some_and(|expansion| {
            expansion.template == JUDGE_PRINCE_REMEDIATION_TEMPLATE
        })
    }));

    let (mut contradiction_graph, contradiction_expansion) = review_fixture(&template);
    let verdict_id = &contradiction_expansion.rounds[0].verdict_id;
    let (_, contradiction_delta) = route_contradictory_verdicts_and_record(
        "authority-contradiction",
        &mut contradiction_graph,
        verdict_id,
        &adjudication(AdjudicationPolicy::HumanGate),
        &contradictory_verdicts(),
    )
    .expect("contradiction routes to adjudication");
    let contradiction_delta = contradiction_delta.expect("adjudication changes graph");
    assert_eq!(
        contradiction_delta.mutation_type,
        GraphMutationType::ContradictionAdjudicated
    );
    assert_ne!(
        contradiction_delta.mutation_type,
        failure_delta.mutation_type
    );
    assert!(contradiction_graph.nodes.iter().any(|node| {
        node.expansion
            .as_ref()
            .is_some_and(|expansion| expansion.template == "review-adjudication")
    }));
    assert!(!contradiction_graph.nodes.iter().any(|node| {
        node.expansion.as_ref().is_some_and(|expansion| {
            expansion.template == JUDGE_PRINCE_REMEDIATION_TEMPLATE
        })
    }));
    assert_eq!(
        contradiction_graph
            .nodes
            .iter()
            .find(|node| node.id == verdict_id.as_str())
            .expect("aggregate verdict retained")
            .status,
        NodeStatus::Blocked
    );
}

#[test]
fn source_verdicts_and_separate_adjudication_survive_real_archive_round_trip() {
    let temp = TempDir::new().expect("temporary archive root");
    let session_id = "authority-contradiction-archive";
    let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).expect("storage");
    let session_dir = storage.create_session_dir(session_id).expect("session dir");
    let manager = StateManager::new(session_dir);
    let template = ReviewTemplate::code_tasks("archive-policy");
    let (mut graph, expansion) = review_fixture(&template);
    manager
        .write_work_graph(&graph)
        .expect("persist pre-adjudication plan graph");
    let verdict_id = &expansion.rounds[0].verdict_id;
    route_contradictory_verdicts_and_record(
        session_id,
        &mut graph,
        verdict_id,
        &adjudication(AdjudicationPolicy::BothAreFindings),
        &contradictory_verdicts(),
    )
    .expect("record contradiction");

    let completion =
        archive_completed_session(temp.path(), None, session_id).expect("archive session");
    let reread = read_archive(&completion.path).expect("read persisted archive");
    assert_eq!(reread, completion.archive);
    assert_eq!(
        reread.deltas[0].mutation_type,
        GraphMutationType::ContradictionAdjudicated
    );
    let source_nodes = reread
        .runtime_graph
        .nodes
        .iter()
        .filter(|node| {
            node.expansion
                .as_ref()
                .is_some_and(|expansion| expansion.template == "source-review-verdict")
        })
        .collect::<Vec<_>>();
    assert_eq!(source_nodes.len(), 2, "archive collapsed a source verdict");
    assert!(source_nodes.iter().any(|node| node.status == NodeStatus::Completed));
    assert!(source_nodes.iter().any(|node| node.status == NodeStatus::Failed));
    let adjudication_node = reread
        .runtime_graph
        .nodes
        .iter()
        .find(|node| {
            node.expansion
                .as_ref()
                .is_some_and(|expansion| expansion.template == "review-adjudication")
        })
        .expect("archive retains separate adjudication");
    let metadata = &adjudication_node
        .expansion
        .as_ref()
        .expect("adjudication metadata")
        .parameters;
    assert!(metadata["policy"].contains("both_are_findings"));
    assert!(metadata["adjudicator"].contains("prince"));
    assert!(metadata["resolution"].contains("findings"));
    assert!(metadata["source_verdicts"].contains("correctness"));
    assert!(metadata["source_verdicts"].contains("security"));
}
