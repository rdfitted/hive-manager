//! Plan-emission and PlanReady-gate tests for issue #211.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{Duration, Utc};
use parking_lot::RwLock;
use tempfile::TempDir;

use crate::actions::coordination::{parse_plan_markdown, parse_plan_markdown_checked};
use crate::coordination::StateManager;
use crate::domain::HiveExecutionPolicy;
use crate::orchestrator::work_graph::review::{
    MULTI_LENS_REVIEW_TEMPLATE, VERIFICATION_DUTY_PARAMETER,
};
use crate::orchestrator::work_graph::validate::{
    validate_plan_ready, PlanReadyWarning,
};
use crate::orchestrator::work_graph::{
    EdgeKind, EdgeProvenance, NodeKind, NodeStatus, WorkGraphOmissionReason,
};
use crate::pty::PtyManager;
use crate::session::{
    AuthStrategy, Session, SessionController, SessionState, SessionType,
};
use crate::storage::SessionStorage;

fn planning_session(session_id: &str, project_path: &Path) -> Session {
    Session {
        id: session_id.to_string(),
        name: None,
        color: None,
        session_type: SessionType::Hive { worker_count: 0 },
        project_path: project_path.to_path_buf(),
        state: SessionState::Planning,
        created_at: Utc::now(),
        last_activity_at: Utc::now(),
        agents: Vec::new(),
        default_cli: "codex".to_string(),
        default_model: None,
        default_principal_cli: None,
        default_principal_model: None,
        default_principal_flags: Vec::new(),
        execution_policy: HiveExecutionPolicy::default(),
        qa_workers: Vec::new(),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        auth_strategy: AuthStrategy::default(),
        worktree_path: None,
        worktree_branch: None,
        no_git: true,
        resume_report: None,
    }
}

fn controller_with_plan(
    session_id: &str,
    plan_content: Option<&str>,
) -> (TempDir, SessionController, Arc<SessionStorage>) {
    controller_with_plan_and_type(
        session_id,
        plan_content,
        SessionType::Hive { worker_count: 0 },
    )
}

fn controller_with_plan_and_type(
    session_id: &str,
    plan_content: Option<&str>,
    session_type: SessionType,
) -> (TempDir, SessionController, Arc<SessionStorage>) {
    let temp = tempfile::tempdir().expect("temporary plan fixture");
    let project_path = temp.path().join("project");
    let project_session_dir = project_path.join(".hive-manager").join(session_id);
    std::fs::create_dir_all(&project_session_dir).expect("project session directory");
    if let Some(content) = plan_content {
        std::fs::write(project_session_dir.join("plan.md"), content).expect("plan fixture");
    }

    let storage = Arc::new(
        SessionStorage::new_with_base(temp.path().join("storage"))
            .expect("isolated session storage"),
    );
    storage
        .create_session_dir(session_id)
        .expect("session state directory");

    let mut controller = SessionController::new(Arc::new(RwLock::new(PtyManager::new())));
    controller.set_storage(Arc::clone(&storage));
    let mut session = planning_session(session_id, &project_path);
    session.session_type = session_type;
    controller.insert_test_session(session);
    (temp, controller, storage)
}

fn assert_session_state(
    controller: &SessionController,
    session_id: &str,
    expected: SessionState,
) {
    assert_eq!(
        controller
            .get_session(session_id)
            .expect("session remains queryable")
            .state,
        expected
    );
}

#[test]
fn production_continue_builds_plan_graph_before_every_running_dispatch() {
    let plan = r#"# Five-task production graph

## Tasks
- [ ] T1: Establish the root
- [ ] T2: Build the first branch (deps: T1)
- [ ] T3: Build the second branch (deps: T1)
- [ ] T4: Join both branches (deps: T2, T3)
- [ ] T5: Finish the workflow (deps: T4)
"#;
    let cases = [
        (
            "continue-hive",
            SessionType::Hive { worker_count: 0 },
            "Failed to read pending config",
        ),
        (
            "continue-swarm",
            SessionType::Swarm { planner_count: 1 },
            "Failed to read pending swarm config",
        ),
        (
            "continue-fusion",
            SessionType::Fusion {
                variants: vec!["Variant A".to_string()],
            },
            "Failed to read pending fusion config",
        ),
        (
            "continue-debate",
            SessionType::Debate {
                variants: vec!["Debater A".to_string()],
            },
            "Failed to read pending debate config",
        ),
    ];

    for (session_id, session_type, expected_dispatch_error) in cases {
        let (_temp, controller, storage) =
            controller_with_plan_and_type(session_id, Some(plan), session_type);

        for attempt in 1..=2 {
            let error = controller
                .continue_after_planning(session_id)
                .expect_err("missing launch config should stop inside the selected dispatch branch");
            assert!(
                error.contains(expected_dispatch_error),
                "attempt {attempt} did not reach {session_id}'s dispatch branch: {error}"
            );

            let graph = StateManager::new(storage.session_dir(session_id))
                .read_work_graph()
                .expect("graph state is readable")
                .expect("production continue persisted the plan graph");
            assert_eq!(graph.nodes.len(), 5, "{session_id} node count");
            assert_eq!(graph.edges.len(), 5, "{session_id} edge count");
            assert!(graph.omissions.is_empty(), "{session_id} omissions");
        }

        assert_session_state(&controller, session_id, SessionState::PlanReady);
    }
}

#[test]
fn production_continue_replaces_empty_seed_with_typed_source_omissions() {
    let malformed_plan = r#"# Malformed production graph

## Tasks
- [ ] T1: Keep launching despite malformed graph metadata (deps: T0
"#;
    let cases = [
        (
            "continue-malformed",
            Some(malformed_plan),
            1,
            WorkGraphOmissionReason::ResolutionIncomplete,
            "unterminated",
        ),
        (
            "continue-missing",
            None,
            0,
            WorkGraphOmissionReason::SourceUnreadable,
            "plan.md",
        ),
    ];

    for (session_id, plan, expected_nodes, expected_reason, expected_example) in cases {
        let (_temp, controller, storage) = controller_with_plan(session_id, plan);

        let error = controller
            .continue_after_planning(session_id)
            .expect_err("missing launch config should fail after the non-blocking graph build");
        assert!(
            error.contains("Failed to read pending config"),
            "plan source failure blocked before Hive dispatch: {error}"
        );
        assert_session_state(&controller, session_id, SessionState::PlanReady);

        let graph = StateManager::new(storage.session_dir(session_id))
            .read_work_graph()
            .expect("graph state is readable")
            .expect("production continue replaced the creation-time seed");
        assert_eq!(graph.nodes.len(), expected_nodes);
        assert!(graph.edges.is_empty());
        assert_eq!(graph.omissions.len(), 1);
        assert_eq!(graph.omissions[0].reason, expected_reason);
        assert!(graph.omissions[0].examples[0].contains(expected_example));
    }
}

#[test]
fn mark_plan_ready_rejects_cycle_and_leaves_session_planning() {
    let plan = r#"# Cyclic plan

## Tasks
- [ ] T1: First task (deps: T2)
- [ ] T2: Second task (deps: T1)
"#;
    let (_temp, controller, _storage) = controller_with_plan("cycle-plan", Some(plan));

    let error = controller
        .mark_plan_ready("cycle-plan")
        .expect_err("a dependency cycle must block the PlanReady transition");
    assert!(error.contains("T1"), "cycle error omitted T1: {error}");
    assert!(error.contains("T2"), "cycle error omitted T2: {error}");
    assert_session_state(&controller, "cycle-plan", SessionState::Planning);
}

#[test]
fn mark_plan_ready_rejects_missing_review_signal_and_leaves_session_planning() {
    let plan = "# Review authority plan\n\n## Tasks\n- [ ] T1: Planner extension (deps: design)\n";
    let (_temp, controller, storage) = controller_with_plan("review-authority", Some(plan));
    let project_path = controller
        .get_session("review-authority")
        .unwrap()
        .project_path;
    controller
        .prepare_initial_work_graph(
            &project_path,
            "review-authority",
            Some("feature-build"),
            &BTreeMap::from([("component".to_string(), "billing".to_string())]),
        )
        .expect("review skeleton composes")
        .expect("feature-build provides a review skeleton");

    let state_manager = StateManager::new(storage.session_dir("review-authority"));
    let mut composition = state_manager
        .read_graph_composition_state()
        .unwrap()
        .unwrap();
    let review = composition
        .graph
        .nodes
        .iter_mut()
        .find(|node| {
            node.kind == NodeKind::Join
                && node
                    .expansion
                    .as_ref()
                    .is_some_and(|expansion| expansion.template == MULTI_LENS_REVIEW_TEMPLATE)
        })
        .expect("feature-build includes a multi-lens review gate");
    let review_id = review.id.clone();
    review
        .expansion
        .as_mut()
        .expect("review expansion exists")
        .parameters
        .remove(VERIFICATION_DUTY_PARAMETER);
    state_manager
        .write_graph_composition_state(&composition)
        .unwrap();

    let error = controller
        .mark_plan_ready("review-authority")
        .expect_err("a review gate without a verification signal must block PlanReady");
    assert!(
        error.contains(&review_id),
        "review-authority error omitted the review id: {error}"
    );
    assert!(
        error.contains("no named verification signal"),
        "unexpected review-authority error: {error}"
    );
    assert_session_state(&controller, "review-authority", SessionState::Planning);
}

#[test]
fn mark_plan_ready_is_always_ok_and_quarantines_dangling_dependency() {
    let plan = r#"# Dangling plan

## Tasks
- [ ] T1: Root task
- [ ] T2: Dependent task (deps: MISSING)
"#;
    let (_temp, controller, storage) = controller_with_plan("dangling-plan", Some(plan));

    controller
        .mark_plan_ready("dangling-plan")
        .expect("a readable dangling plan must not block PlanReady");
    assert_session_state(&controller, "dangling-plan", SessionState::PlanReady);

    let graph = StateManager::new(storage.session_dir("dangling-plan"))
        .read_work_graph()
        .unwrap()
        .unwrap();
    assert_eq!(graph.nodes.len(), 2);
    validate_plan_ready(&graph).expect("the persisted graph must be valid after edge quarantine");
    assert!(
        graph.edges.is_empty(),
        "unresolved dependencies must not survive quarantine"
    );
    let diagnostic = graph
        .omissions
        .iter()
        .flat_map(|omission| omission.examples.iter())
        .find(|example| example.contains("MISSING"))
        .expect("the quarantined dependency must remain operator-visible");
    assert!(diagnostic.contains("T2"));
}

#[test]
fn valid_plan_persists_queryable_dependency_graph_and_contracts() {
    let plan = r#"# Valid graph

## Tasks
- [ ] T1: Build schema (inputs: issue) (outputs: schema.rs) (acceptance: serde round-trip) -> P1
- [ ] [P1] T2: Wire parser (deps: T1) (inputs: schema.rs) (outputs: plan_parse.rs) (acceptance: dependency is queryable) -> P2
"#;
    let (_temp, controller, storage) = controller_with_plan("valid-plan", Some(plan));

    controller
        .mark_plan_ready("valid-plan")
        .expect("valid graph reaches PlanReady");
    assert_session_state(&controller, "valid-plan", SessionState::PlanReady);

    let graph = StateManager::new(storage.session_dir("valid-plan"))
        .read_work_graph()
        .expect("graph state is readable")
        .expect("graph state was persisted");
    assert_eq!(
        graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["T1", "T2"]
    );
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].source, "T1");
    assert_eq!(graph.edges[0].target, "T2");
    assert_eq!(graph.edges[0].kind, EdgeKind::DependsOn);
    assert_eq!(graph.edges[0].provenance, EdgeProvenance::Planner);
    assert_eq!(graph.nodes[0].contract.inputs, vec!["issue"]);
    assert_eq!(graph.nodes[0].contract.outputs, vec!["schema.rs"]);
    assert_eq!(graph.nodes[0].contract.acceptance, vec!["serde round-trip"]);
    assert_eq!(
        storage.load_session("valid-plan").unwrap().state,
        "PlanReady",
        "the validated state must survive a controller restart"
    );
}

#[test]
fn no_archetype_preparation_is_an_exact_persistence_noop() {
    let (_temp, controller, storage) = controller_with_plan(
        "no-archetype",
        Some("# Legacy plan\n\n## Tasks\n- [ ] T1: Existing root\n"),
    );
    let project_path = controller.get_session("no-archetype").unwrap().project_path;

    let composition = controller
        .prepare_initial_work_graph(
            &project_path,
            "no-archetype",
            None,
            &BTreeMap::new(),
        )
        .expect("missing archetype keeps the legacy launch path");

    assert!(composition.is_none());
    assert!(
        StateManager::new(storage.session_dir("no-archetype"))
            .read_graph_composition_state()
            .unwrap()
            .is_none(),
        "legacy sessions must not gain a composition sidecar"
    );
}

#[test]
fn selected_archetype_persists_skeleton_and_planner_contract_before_plan_ready() {
    let (_temp, controller, storage) = controller_with_plan(
        "phase-a",
        Some("# Planner output\n\n## Tasks\n- [ ] T1: Planner-specific task\n"),
    );
    let project_path = controller.get_session("phase-a").unwrap().project_path;
    let parameters = BTreeMap::from([("component".to_string(), "billing".to_string())]);

    let composition = controller
        .prepare_initial_work_graph(
            &project_path,
            "phase-a",
            Some("feature-build"),
            &parameters,
        )
        .expect("selected archetype composes")
        .expect("selected archetype produces state");

    let persisted = StateManager::new(storage.session_dir("phase-a"))
        .read_graph_composition_state()
        .unwrap()
        .expect("Phase A state is durable before PlanReady");
    assert_eq!(persisted, composition);
    assert_eq!(
        persisted.lineage.as_ref().unwrap().template_id,
        "feature-build"
    );
    assert!(persisted.graph.nodes.iter().any(|node| node.id == "design"));
    assert!(
        persisted
            .graph
            .nodes
            .iter()
            .filter(|node| node.kind == crate::orchestrator::work_graph::NodeKind::Task)
            .any(|node| node.status == NodeStatus::Ready),
        "initial schedulable nodes are promoted after stamping"
    );

    let prompt = SessionController::graph_skeleton_prompt_section(&persisted).unwrap();
    assert!(prompt.contains("Every listed node ID is reserved and stable"));
    assert!(prompt.contains("\"id\": \"design\""));
    assert!(prompt.contains("\"contract\""));
    assert!(prompt.contains("(deps: ...)"));
}

#[test]
fn launch_time_retro_provenance_preserves_departed_identity_sets() {
    let (_temp, controller, storage) = controller_with_plan(
        "retro-provenance",
        Some("# Plan\n\n## Tasks\n- [ ] T1: Root\n"),
    );
    let project_path = controller
        .get_session("retro-provenance")
        .unwrap()
        .project_path;
    controller
        .persist_retro_provenance(
            &project_path,
            "retro-provenance",
            vec!["retro-provenance-master-planner".to_string()],
            vec![
                "retro-provenance-queen".to_string(),
                "retro-provenance-prince".to_string(),
            ],
        )
        .expect("identity provenance is persisted before agents depart");

    let path = storage
        .session_dir("retro-provenance")
        .join("state")
        .join("retro-evaluator-provenance.json");
    let provenance: crate::orchestrator::work_graph::archive::RetroEvaluatorProvenance =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        provenance.evaluator_id,
        "retro-provenance-retro-evaluator"
    );
    assert_eq!(
        provenance.planner_agent_ids,
        vec!["retro-provenance-master-planner"]
    );
    assert_eq!(
        provenance.supervisor_agent_ids,
        vec!["retro-provenance-queen", "retro-provenance-prince"]
    );
}

#[test]
fn phase_b_reconciliation_is_idempotent_after_a_persisted_retry() {
    let (_temp, controller, storage) = controller_with_plan(
        "phase-b-retry",
        Some(
            "# Reconciled plan\n\n## Tasks\n- [ ] T1: Planner extension (deps: design) (outputs: integration)\n",
        ),
    );
    let project_path = controller
        .get_session("phase-b-retry")
        .unwrap()
        .project_path;
    controller
        .prepare_initial_work_graph(
            &project_path,
            "phase-b-retry",
            Some("feature-build"),
            &BTreeMap::from([("component".to_string(), "billing".to_string())]),
        )
        .unwrap();
    let planner = parse_plan_markdown_checked(
        "# Reconciled plan\n\n## Tasks\n- [ ] T1: Planner extension (deps: design) (outputs: integration)\n",
    )
    .map(|plan| crate::orchestrator::work_graph::plan_parse::task_graph_from_plan(&plan))
    .unwrap();
    let state_manager = StateManager::new(storage.session_dir("phase-b-retry"));

    let once = SessionController::reconcile_planner_graph_with_persisted_skeleton(
        &state_manager,
        &planner,
    )
    .unwrap()
    .unwrap();
    state_manager.write_graph_composition_state(&once).unwrap();
    let twice = SessionController::reconcile_planner_graph_with_persisted_skeleton(
        &state_manager,
        &planner,
    )
    .unwrap()
    .unwrap();

    assert_eq!(once, twice, "Phase B retry must be an exact no-op");
    assert!(twice.graph.nodes.iter().any(|node| node.id == "design"));
    assert!(twice.graph.nodes.iter().any(|node| node.id == "T1"));
    assert!(twice.graph.edges.iter().any(|edge| {
        edge.source == "design" && edge.target == "T1" && edge.kind == EdgeKind::DependsOn
    }));
}

#[test]
fn mark_plan_ready_persists_one_authoritative_reconciled_graph() {
    let plan = "# Reconciled plan\n\n## Tasks\n- [ ] T1: Planner extension (deps: design, MISSING) (acceptance: reconciled)\n";
    let (_temp, controller, storage) =
        controller_with_plan("composed-plan-ready", Some(plan));
    let project_path = controller
        .get_session("composed-plan-ready")
        .unwrap()
        .project_path;
    controller
        .prepare_initial_work_graph(
            &project_path,
            "composed-plan-ready",
            Some("feature-build"),
            &BTreeMap::from([("component".to_string(), "billing".to_string())]),
        )
        .unwrap();

    controller
        .mark_plan_ready("composed-plan-ready")
        .expect("Phase B reaches the real PlanReady boundary");

    let state_manager = StateManager::new(storage.session_dir("composed-plan-ready"));
    let composition = state_manager
        .read_graph_composition_state()
        .unwrap()
        .unwrap();
    let authoritative = state_manager.read_work_graph().unwrap().unwrap();
    assert_eq!(authoritative, composition.graph);
    assert!(authoritative.nodes.iter().any(|node| node.id == "design"));
    assert!(authoritative.nodes.iter().any(|node| node.id == "T1"));
    assert!(authoritative.edges.iter().any(|edge| {
        edge.source == "design" && edge.target == "T1" && edge.kind == EdgeKind::DependsOn
    }));
    assert!(
        authoritative
            .edges
            .iter()
            .all(|edge| edge.source != "MISSING" && edge.target != "MISSING"),
        "quarantine runs after reconciliation so skeleton references survive and unknown ones do not"
    );
    assert!(authoritative
        .omissions
        .iter()
        .flat_map(|omission| omission.examples.iter())
        .any(|example| example.contains("T1") && example.contains("MISSING")));
    assert!(composition.lineage.is_some());
    assert!(
        !composition.graph.omissions.is_empty(),
        "missing codegraph/wiki inputs remain explicitly reported"
    );
}

#[test]
fn completed_session_schedules_the_composed_archive_to_retro_hook() {
    let temp = tempfile::tempdir().expect("temporary completion fixture");
    let project_path = temp.path().join("project");
    std::fs::create_dir_all(&project_path).unwrap();
    let storage = Arc::new(
        SessionStorage::new_with_base(temp.path().join("storage"))
            .expect("isolated session storage"),
    );
    let mut session = planning_session("retro-completion", &project_path);
    session.state = SessionState::Running;
    session.last_activity_at = Utc::now() - Duration::minutes(11);
    let mut controller = SessionController::new(Arc::new(RwLock::new(PtyManager::new())));
    controller.set_storage(Arc::clone(&storage));
    controller.insert_test_session(session);

    controller
        .mark_session_completed("retro-completion")
        .expect("terminal transition is not reversed by detached retro work");

    let retro_dir = storage
        .session_dir("retro-completion")
        .join("archive")
        .join("work-graph-retros");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let report_exists = std::fs::read_dir(&retro_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"));
        if report_exists {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "composed archive-to-retro hook did not persist a report"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_session_state(&controller, "retro-completion", SessionState::Completed);
}

#[test]
fn explicit_graph_ignores_legacy_bullets_and_numbered_task_details() {
    let plan = r#"# Generated-plan shape

## Domain Tasks
- [ ] [P1] T1: Root domain (inputs: issue) (outputs: root result) (acceptance: root done) -> P1
Files: src/root.rs
Workers: 2
- [ ] [P2] T2: Dependent domain (deps: T1) (inputs: root result) (outputs: final result) (acceptance: final done) -> P2

## Task Details
- This legacy bullet remains visible to the UI parser.
1. This numbered instruction remains visible too.
"#;
    let parsed = parse_plan_markdown_checked(plan).expect("generated graph syntax parses");
    assert_eq!(
        parsed.tasks.len(),
        4,
        "legacy parser behavior still includes ordinary task-section list items"
    );

    let graph = crate::orchestrator::work_graph::plan_parse::task_graph_from_plan(&parsed);
    assert_eq!(
        graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["T1", "T2"]
    );
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].source, "T1");
    assert_eq!(graph.edges[0].target, "T2");
}

#[test]
fn legacy_plan_without_deps_is_edgeless_and_preserves_parse_fields() {
    let plan = r#"# Legacy plan

## Summary
Keep the existing parser behavior.

## Tasks
- [ ] [P1]   Fix launch regression   -> worker-8
2. Preserve a positional task
"#;
    let parsed = parse_plan_markdown(plan);
    assert_eq!(parsed.title, "Legacy plan");
    assert_eq!(parsed.summary, "Keep the existing parser behavior.");
    assert_eq!(parsed.raw_content, plan);
    assert_eq!(parsed.tasks.len(), 2);
    assert_eq!(parsed.tasks[0].id, "task-1");
    assert_eq!(parsed.tasks[0].title, "Fix launch regression");
    assert_eq!(parsed.tasks[0].priority.as_deref(), Some("high"));
    assert_eq!(parsed.tasks[0].assignee.as_deref(), Some("worker-8"));
    assert_eq!(parsed.tasks[1].id, "task-2");
    assert_eq!(parsed.tasks[1].title, "Preserve a positional task");
    assert!(parsed.tasks.iter().all(|task| task.depends_on.is_empty()));

    let (_temp, controller, storage) = controller_with_plan("legacy-plan", Some(plan));
    controller
        .mark_plan_ready("legacy-plan")
        .expect("edgeless legacy plan remains valid");
    assert_session_state(&controller, "legacy-plan", SessionState::PlanReady);
    let graph = StateManager::new(storage.session_dir("legacy-plan"))
        .read_work_graph()
        .unwrap()
        .unwrap();
    assert_eq!(graph.nodes.len(), 2);
    assert!(graph.edges.is_empty());
    assert_eq!(graph.omissions.len(), 1);
    assert_eq!(
        graph.omissions[0].reason,
        WorkGraphOmissionReason::ResolutionIncomplete
    );
    assert_eq!(graph.omissions[0].count, 1);
    assert!(graph.omissions[0].examples[0].contains("Fix launch regression"));
    assert!(
        StateManager::new(storage.session_dir("legacy-plan"))
            .read_graph_composition_state()
            .unwrap()
            .is_none(),
        "no-archetype PlanReady remains on the exact legacy persistence path"
    );
}

#[test]
fn malformed_graph_metadata_preserves_task_and_degrades_to_edgeless_graph() {
    let plan = r#"# Legacy-compatible malformed graph metadata

## Tasks
- [ ] T1: Keep running even with an unterminated graph suffix (deps: T0
"#;
    assert!(parse_plan_markdown_checked(plan).is_err());
    let (_temp, controller, storage) = controller_with_plan("malformed-plan", Some(plan));

    controller
        .mark_plan_ready("malformed-plan")
        .expect("malformed graph metadata degrades instead of wedging planning");
    assert_session_state(&controller, "malformed-plan", SessionState::PlanReady);
    let graph = StateManager::new(storage.session_dir("malformed-plan"))
        .read_work_graph()
        .unwrap()
        .unwrap();
    assert_eq!(graph.nodes.len(), 1);
    assert_eq!(graph.nodes[0].id, "T1");
    assert!(graph.edges.is_empty());
    assert_eq!(graph.omissions.len(), 1);
    assert_eq!(
        graph.omissions[0].reason,
        WorkGraphOmissionReason::ResolutionIncomplete
    );
    assert!(graph.omissions[0].examples[0].contains("unterminated"));
}

#[test]
fn explicit_graph_preserves_resolved_tasks_and_reports_every_lost_reference() {
    let plan = r#"# Mixed malformed graph

## Tasks
- [ ] T1: Stable root task
- [ ] Crucial workstream missing its stable id
- [ ] T2: Stable dependent task (deps: T3)
"#;
    let error = parse_plan_markdown_checked(plan)
        .expect_err("an explicit graph cannot silently discard a schedulable checkbox");
    assert!(
        error.to_string().contains("Crucial workstream"),
        "parse diagnostic must name the unidentified task: {error}"
    );

    let (_temp, controller, storage) = controller_with_plan("mixed-malformed", Some(plan));
    controller
        .mark_plan_ready("mixed-malformed")
        .expect("partial graph syntax must preserve usable work without blocking PlanReady");
    assert_session_state(&controller, "mixed-malformed", SessionState::PlanReady);
    let graph = StateManager::new(storage.session_dir("mixed-malformed"))
        .read_work_graph()
        .unwrap()
        .unwrap();
    assert_eq!(
        graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["T1", "T2"],
        "every successfully identified task must be preserved"
    );
    assert!(
        graph.edges.is_empty(),
        "the dependency on the unresolved T3 endpoint must be quarantined"
    );
    assert!(graph
        .omissions
        .iter()
        .all(|omission| omission.reason == WorkGraphOmissionReason::ResolutionIncomplete));
    let examples = graph
        .omissions
        .iter()
        .flat_map(|omission| omission.examples.iter())
        .collect::<Vec<_>>();
    assert!(
        examples
            .iter()
            .any(|example| example.contains("Crucial workstream")),
        "the unidentified schedulable line must not disappear silently"
    );
    assert!(
        examples
            .iter()
            .any(|example| example.contains("T2") && example.contains("T3")),
        "the quarantined dependency must name both endpoints"
    );
}

#[test]
fn partially_parsed_twenty_two_line_plan_preserves_eighteen_nodes_and_four_omissions() {
    let mut plan = String::from("# Partially parsed plan\n\n## Tasks\n");
    for id in 1..=18 {
        plan.push_str(&format!("- [ ] T{id}: Stable task {id}\n"));
    }
    for id in 19..=22 {
        plan.push_str(&format!("- [ ] Unidentified task {id}\n"));
    }

    let (_temp, controller, storage) = controller_with_plan("partial-18-of-22", Some(&plan));
    controller
        .mark_plan_ready("partial-18-of-22")
        .expect("a readable partially parsed plan must not block PlanReady");
    assert_session_state(&controller, "partial-18-of-22", SessionState::PlanReady);

    let graph = StateManager::new(storage.session_dir("partial-18-of-22"))
        .read_work_graph()
        .unwrap()
        .unwrap();
    assert_eq!(graph.nodes.len(), 18);
    let parser_omission = graph
        .omissions
        .iter()
        .find(|omission| omission.count == 4)
        .expect("the four unresolved lines must be counted as omissions");
    assert_eq!(
        parser_omission.reason,
        WorkGraphOmissionReason::ResolutionIncomplete
    );
    assert_eq!(parser_omission.examples.len(), 4);
    for id in 19..=22 {
        assert!(parser_omission
            .examples
            .iter()
            .any(|example| example.contains(&format!("Unidentified task {id}"))));
    }
}

#[test]
fn all_legacy_checkbox_tasks_are_preserved_but_reported_without_stable_ids() {
    let plan = r#"# Positional-only plan

## Tasks
- [ ] First positional task
- [ ] Second positional task
"#;
    let (_temp, controller, storage) = controller_with_plan("all-positional", Some(plan));

    controller
        .mark_plan_ready("all-positional")
        .expect("positional compatibility must not block PlanReady");
    let graph = StateManager::new(storage.session_dir("all-positional"))
        .read_work_graph()
        .unwrap()
        .unwrap();
    assert_eq!(
        graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["task-1", "task-2"]
    );
    assert_eq!(graph.omissions.len(), 1);
    assert_eq!(graph.omissions[0].count, 2);
    assert!(graph.omissions[0]
        .examples
        .iter()
        .any(|example| example.contains("First positional")));
    assert!(graph.omissions[0]
        .examples
        .iter()
        .any(|example| example.contains("Second positional")));
}

#[test]
fn unavailable_state_manager_does_not_block_a_valid_plan() {
    let temp = tempfile::tempdir().expect("temporary plan fixture");
    let session_id = "no-state-manager";
    let project_path = temp.path().join("project");
    let project_session_dir = project_path.join(".hive-manager").join(session_id);
    std::fs::create_dir_all(&project_session_dir).unwrap();
    std::fs::write(
        project_session_dir.join("plan.md"),
        "# Plan\n\n## Tasks\n- [ ] T1: Valid root task\n",
    )
    .unwrap();

    let controller = SessionController::new(Arc::new(RwLock::new(PtyManager::new())));
    controller.insert_test_session(planning_session(session_id, &project_path));
    controller
        .mark_plan_ready(session_id)
        .expect("absence of optional state storage must degrade gracefully");
    assert_session_state(&controller, session_id, SessionState::PlanReady);
}

#[test]
fn unreadable_plan_degrades_and_persists_source_omission() {
    let (_temp, controller, storage) = controller_with_plan("missing-plan", None);
    controller
        .mark_plan_ready("missing-plan")
        .expect("missing legacy plan source must not wedge planning");
    assert_session_state(&controller, "missing-plan", SessionState::PlanReady);

    let graph = StateManager::new(storage.session_dir("missing-plan"))
        .read_work_graph()
        .unwrap()
        .unwrap();
    assert_eq!(graph.omissions.len(), 1);
    assert_eq!(
        graph.omissions[0].reason,
        WorkGraphOmissionReason::SourceUnreadable
    );
}

#[test]
fn duplicate_explicit_ids_are_reported_without_blocking_readable_plan() {
    let plan = r#"# Duplicate ids

## Tasks
- [ ] T1: First declaration
- [ ] T1: Second declaration
"#;
    let (_temp, controller, storage) = controller_with_plan("duplicate-plan", Some(plan));
    controller
        .mark_plan_ready("duplicate-plan")
        .expect("duplicate ids in a readable plan must not block PlanReady");
    assert_session_state(&controller, "duplicate-plan", SessionState::PlanReady);

    let graph = StateManager::new(storage.session_dir("duplicate-plan"))
        .read_work_graph()
        .unwrap()
        .unwrap();
    assert_eq!(graph.nodes.len(), 1);
    validate_plan_ready(&graph).expect("the persisted graph must retain one unambiguous T1 node");
    assert!(graph
        .omissions
        .iter()
        .flat_map(|omission| omission.examples.iter())
        .any(|example| example.contains("duplicate") && example.contains("T1")));
}

#[test]
fn disconnected_dependency_component_warns_but_remains_valid() {
    let plan = r#"# Orphan warning

## Tasks
- [ ] T1: Main root
- [ ] T2: Main dependent (deps: T1)
- [ ] T3: Orphan root
- [ ] T4: Orphan dependent (deps: T3)
"#;
    let parsed = parse_plan_markdown_checked(plan).expect("well-formed graph syntax");
    let graph = crate::orchestrator::work_graph::plan_parse::task_graph_from_plan(&parsed);
    let validation = validate_plan_ready(&graph).expect("orphans warn rather than reject");

    assert_eq!(
        validation.warnings,
        vec![PlanReadyWarning::OrphanSubtrees {
            task_ids: vec!["T3".to_string(), "T4".to_string()]
        }]
    );
}
