//! Plan-emission and PlanReady-gate tests for issue #211.

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use parking_lot::RwLock;
use tempfile::TempDir;

use crate::actions::coordination::{parse_plan_markdown, parse_plan_markdown_checked};
use crate::coordination::StateManager;
use crate::domain::HiveExecutionPolicy;
use crate::orchestrator::work_graph::validate::{
    validate_plan_ready, PlanReadyWarning,
};
use crate::orchestrator::work_graph::{
    EdgeKind, EdgeProvenance, WorkGraphOmissionReason,
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
    controller.insert_test_session(planning_session(session_id, &project_path));
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
fn mark_plan_ready_rejects_cycle_and_leaves_session_planning() {
    let plan = r#"# Cyclic plan

## Tasks
- [ ] T1: First task (deps: T2)
- [ ] T2: Second task (deps: T1)
"#;
    let (_temp, controller, _storage) = controller_with_plan("cycle-plan", Some(plan));

    let error = controller
        .mark_plan_ready("cycle-plan")
        .expect_err("cycle must fail the real PlanReady transition");
    assert!(error.contains("T1"), "cycle error omitted T1: {error}");
    assert!(error.contains("T2"), "cycle error omitted T2: {error}");
    assert_session_state(&controller, "cycle-plan", SessionState::Planning);
}

#[test]
fn mark_plan_ready_rejects_dangling_dependency_and_names_both_ids() {
    let plan = r#"# Dangling plan

## Tasks
- [ ] T1: Root task
- [ ] T2: Dependent task (deps: MISSING)
"#;
    let (_temp, controller, _storage) = controller_with_plan("dangling-plan", Some(plan));

    let error = controller
        .mark_plan_ready("dangling-plan")
        .expect_err("dangling dependency must fail PlanReady");
    assert!(error.contains("T2"), "error omitted dependent id: {error}");
    assert!(error.contains("MISSING"), "error omitted unknown id: {error}");
    assert_session_state(&controller, "dangling-plan", SessionState::Planning);
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
    assert!(graph.omissions.is_empty());
}

#[test]
fn malformed_graph_metadata_degrades_to_edgeless_graph_with_omission() {
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
    assert!(graph.nodes.is_empty());
    assert!(graph.edges.is_empty());
    assert_eq!(graph.omissions.len(), 1);
    assert_eq!(
        graph.omissions[0].reason,
        WorkGraphOmissionReason::ResolutionIncomplete
    );
    assert!(graph.omissions[0].examples[0].contains("unterminated"));
}

#[test]
fn explicit_graph_with_unidentified_checkbox_degrades_and_reports_it() {
    let plan = r#"# Mixed malformed graph

## Tasks
- [ ] T1: Stable root task
- [ ] Crucial workstream missing its stable id
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
        .expect("malformed graph syntax follows the degrade-and-report guardrail");
    let graph = StateManager::new(storage.session_dir("mixed-malformed"))
        .read_work_graph()
        .unwrap()
        .unwrap();
    assert!(graph.nodes.is_empty());
    assert!(graph.edges.is_empty());
    assert_eq!(
        graph.omissions[0].reason,
        WorkGraphOmissionReason::ResolutionIncomplete
    );
    assert!(graph.omissions[0].examples[0].contains("Crucial workstream"));
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
fn duplicate_explicit_ids_fail_the_real_gate_without_state_change() {
    let plan = r#"# Duplicate ids

## Tasks
- [ ] T1: First declaration
- [ ] T1: Second declaration
"#;
    let (_temp, controller, _storage) = controller_with_plan("duplicate-plan", Some(plan));
    let error = controller
        .mark_plan_ready("duplicate-plan")
        .expect_err("duplicate task ids must fail PlanReady");
    assert!(error.contains("T1"), "duplicate error omitted task id: {error}");
    assert_session_state(&controller, "duplicate-plan", SessionState::Planning);
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
