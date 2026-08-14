//! Org-graph role schema and resolution tests for issues #229-#231.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use parking_lot::RwLock;

use crate::cli::{CliBehavior, CliRegistry};
use crate::domain::{HiveExecutionPolicy, WorkspaceStrategy};
use crate::orchestrator::org_graph::composition::{
    compose_context, lint_role_knowledge_hubs, render_composed_context, ContextBudget,
    spawn_context_from_work_graph_task, ContextOrigin, SpawnContext,
};
use crate::orchestrator::org_graph::definitions::{
    resolve_role_definition, ResolvedRoleDefinition, RoleDefinitionSource,
    RoleResolutionIssueKind,
};
use crate::orchestrator::org_graph::{
    evaluator_role_definition, AuthorityScope, KnowledgeRef, KnowledgeSource, RoleDefinition,
};
use crate::orchestrator::work_graph::{
    BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract, NodeKind,
    NodeStatus, TaskGraph, WorkEdge, WorkNode,
};
use crate::pty::{AgentConfig, PtyManager, WorkerRole};
use crate::session::{
    AuthStrategy, Session, SessionController, SessionState, SessionType,
    DEFAULT_MAX_QA_ITERATIONS,
};
use crate::templates::CellTemplate;

#[test]
fn a11_cell_template_semantics_serde_round_trip() {
    let template = CellTemplate {
        role: "reviewer".to_string(),
        cli: "claude".to_string(),
        model: Some("opus".to_string()),
        prompt_template: "roles/reviewer".to_string(),
        knowledge_scope: vec![KnowledgeRef {
            source: KnowledgeSource::Project,
            pointer: "patterns/testing-strategy.md".to_string(),
            summary: Some("Mutation proofs must fail at the production seam.".to_string()),
            priority: 100,
        }],
        lens: Some("test-quality".to_string()),
        authority: AuthorityScope {
            may_spawn_subordinates: true,
            may_adjudicate: true,
            ..AuthorityScope::default()
        },
        behavior: Some(CliBehavior::InstructionFollowing),
    };

    let encoded = serde_json::to_string(&template).expect("serialize CellTemplate");
    let decoded: CellTemplate =
        serde_json::from_str(&encoded).expect("deserialize CellTemplate");

    assert_eq!(decoded, template);
    assert_eq!(decoded.knowledge_scope[0].priority, 100);
    assert!(decoded.authority.may_adjudicate);
    assert_eq!(decoded.behavior, Some(CliBehavior::InstructionFollowing));
    assert!(encoded.contains(r#""knowledge_scope""#));
    assert!(encoded.contains(r#""lens":"test-quality""#));
    assert!(encoded.contains(r#""authority""#));
    assert!(encoded.contains(r#""behavior":"instruction_following""#));

    let legacy: CellTemplate = serde_json::from_str(
        r#"{"role":"backend","cli":"codex","model":null,"prompt_template":"roles/backend"}"#,
    )
    .expect("deserialize pre-T1 CellTemplate");
    assert!(legacy.knowledge_scope.is_empty());
    assert!(legacy.lens.is_none());
    assert_eq!(legacy.authority, AuthorityScope::default());
    assert!(legacy.behavior.is_none());
}

#[test]
fn a12_evaluator_behavior_comes_from_declared_definition() {
    let evaluator = evaluator_role_definition();

    assert_eq!(
        CliRegistry::get_behavior_for_role("claude", Some(&evaluator)),
        CliBehavior::InstructionFollowing
    );
    assert_eq!(evaluator.behavior, Some(CliBehavior::InstructionFollowing));

    let project = tempfile::tempdir().expect("temp project");
    let resolved = resolve_role_definition(project.path(), None, "evaluator");
    let prompt = SessionController::build_worker_prompt(
        1,
        &AgentConfig {
            cli: "claude".to_string(),
            role: Some(WorkerRole::new("evaluator", "Evaluator", "claude")),
            ..AgentConfig::default()
        },
        &resolved,
        &crate::orchestrator::org_graph::composition::SpawnContext::default(),
        "session-role-queen",
        "session-role",
        project.path(),
        project.path(),
        &HiveExecutionPolicy::default(),
    );
    assert!(
        prompt.contains("Read and follow the complete task file."),
        "the declared evaluator profile must reach emitted spawn instructions"
    );

    let registry_source = include_str!("../cli/registry.rs");
    assert!(
        !registry_source.contains(r#"Some("evaluator") =>"#),
        "the evaluator must not reappear as a CLI registry match arm"
    );
}

#[test]
fn a13_empty_definition_preserves_spawn_prompt_bytes() {
    let empty = RoleDefinition::empty("backend");
    let empty_report = resolved_for_test(empty.clone());
    let absent_report = ResolvedRoleDefinition {
        requested_id: "backend".to_string(),
        definition: None,
        base_source: None,
        applied_override: None,
        issues: Vec::new(),
    };
    let project = tempfile::tempdir().expect("temp project");

    assert!(empty.knowledge_scope.is_empty());
    assert!(empty.lens.is_none());
    assert!(empty.behavior.is_none());

    for cli in ["claude", "qwen", "codex", "droid", "unknown-cli"] {
        assert_eq!(
            CliRegistry::get_behavior_for_role(cli, Some(&empty)),
            CliRegistry::get_behavior(cli),
            "an empty definition must inherit {cli}'s existing capability behavior"
        );
        assert_eq!(
            SessionController::polling_instructions_for_definition(
                cli,
                "worker-1-task.md",
                Some(&empty),
            ),
            SessionController::polling_instructions_for_definition(
                cli,
                "worker-1-task.md",
                None,
            ),
            "an empty definition changed the emitted spawn prompt for {cli}"
        );
        let config = AgentConfig {
            cli: cli.to_string(),
            role: Some(WorkerRole::new("backend", "Backend", cli)),
            ..AgentConfig::default()
        };
        let build = |resolved| {
            SessionController::build_worker_prompt(
                1,
                &config,
                resolved,
                &SpawnContext::default(),
                "byte-parity-queen",
                "byte-parity",
                project.path(),
                project.path(),
                &HiveExecutionPolicy::default(),
            )
        };
        assert_eq!(
            build(&empty_report),
            build(&absent_report),
            "an empty definition changed the full emitted spawn prompt for {cli}"
        );
    }
}

fn role_test_session(id: &str, project_path: PathBuf) -> Session {
    let shared_workspace = project_path.to_string_lossy().to_string();
    Session {
        id: id.to_string(),
        name: None,
        color: None,
        session_type: SessionType::Hive { worker_count: 0 },
        project_path,
        state: SessionState::Running,
        created_at: Utc::now(),
        last_activity_at: Utc::now(),
        agents: Vec::new(),
        default_cli: "claude".to_string(),
        default_model: None,
        default_principal_cli: None,
        default_principal_model: None,
        default_principal_flags: Vec::new(),
        execution_policy: HiveExecutionPolicy {
            workspace_strategy: WorkspaceStrategy::SharedCell,
            ..HiveExecutionPolicy::default()
        },
        qa_workers: Vec::new(),
        max_qa_iterations: DEFAULT_MAX_QA_ITERATIONS,
        qa_timeout_secs: 300,
        auth_strategy: AuthStrategy::default(),
        worktree_path: Some(shared_workspace),
        worktree_branch: Some(format!("hive/{id}/primary")),
        no_git: false,
        resume_report: None,
    }
}

#[test]
fn a14_reviewer_spawn_records_resolved_definition_and_version() {
    let project = tempfile::tempdir().expect("temp project");
    let session_id = "role-reviewer-spawn";
    let controller = SessionController::new(Arc::new(RwLock::new(PtyManager::new())));
    controller.insert_test_session(role_test_session(
        session_id,
        project.path().to_path_buf(),
    ));

    let agent = controller
        .add_worker(
            session_id,
            AgentConfig::default(),
            WorkerRole::new("reviewer", "Reviewer", "claude"),
            None,
            None,
        )
        .expect("spawn reviewer from embedded definition");

    assert_eq!(agent.role_definition_id.as_deref(), Some("reviewer"));
    assert_eq!(agent.role_definition_version, Some(1));
    let role_identity = agent
        .config
        .role
        .as_ref()
        .and_then(|role| role.resolved_definition.as_ref())
        .expect("worker position must carry its separate semantic identity");
    assert_eq!(role_identity.id, "reviewer");
    assert_eq!(role_identity.version, 1);

    let stored = controller
        .get_session(session_id)
        .expect("stored session")
        .agents
        .into_iter()
        .find(|candidate| candidate.id == agent.id)
        .expect("stored reviewer agent");
    assert_eq!(stored.role_definition_id, agent.role_definition_id);
    assert_eq!(stored.role_definition_version, agent.role_definition_version);
    let persisted = SessionController::persisted_snapshot_for_test(
        &controller.get_session(session_id).expect("session to archive"),
    );
    let archived_agent = persisted
        .agents
        .iter()
        .find(|candidate| candidate.id == agent.id)
        .expect("persisted reviewer agent");
    assert_eq!(archived_agent.role_definition_id.as_deref(), Some("reviewer"));
    assert_eq!(archived_agent.role_definition_version, Some(1));
}

#[test]
fn a15_project_path_override_wins_without_mutating_institutional_definition() {
    let project = tempfile::tempdir().expect("temp project");
    let institutional = tempfile::tempdir().expect("temp institutional wiki");
    std::fs::create_dir_all(project.path().join(".ai-docs/roles"))
        .expect("create project role directory");
    std::fs::create_dir_all(institutional.path().join("roles"))
        .expect("create institutional role directory");

    let institutional_definition = r#"---
{"id":"reviewer","version":3,"domain":"Institutional review","knowledge_scope":[{"source":"institutional","pointer":"review/base.md","summary":"Institutional base guidance.","priority":50}],"lens":{"id":"base","question":"What does the shared standard require?"},"authority":{},"context_boundary":"artifact","signal_class":"judgmental","prompt_template":"roles/reviewer","non_goals":[]}
---
# Institutional reviewer
"#;
    let institutional_path = institutional.path().join("roles/reviewer.md");
    std::fs::write(&institutional_path, institutional_definition)
        .expect("write institutional definition");
    let institutional_before = std::fs::read(&institutional_path).expect("read base bytes");

    let override_path = project.path().join(".ai-docs/roles/reviewer.md");
    std::fs::write(
        &override_path,
        r#"---
{"id":"reviewer","version":7,"knowledge_scope":[{"source":"project","pointer":"repo-only/review-contract.md","summary":"This repository's independent review contract.","priority":100}]}
---
# Project reviewer override
"#,
    )
    .expect("write project override");

    assert_ne!(
        std::env::current_dir().expect("current directory"),
        project.path(),
        "fixture must fail if the resolver consults CWD"
    );
    let resolved = resolve_role_definition(
        project.path(),
        Some(institutional.path()),
        "reviewer",
    );
    let definition = resolved.definition.expect("resolved reviewer");

    assert_eq!(resolved.base_source, Some(RoleDefinitionSource::Institutional));
    assert_eq!(
        resolved.applied_override.as_deref().map(PathBuf::from),
        Some(override_path.clone())
    );
    assert_eq!(definition.version, 7);
    assert_eq!(definition.domain.as_deref(), Some("Institutional review"));
    assert_eq!(definition.knowledge_scope.len(), 1);
    assert_eq!(
        definition.knowledge_scope[0].pointer,
        "repo-only/review-contract.md"
    );
    assert_eq!(
        std::fs::read(&institutional_path).expect("re-read base bytes"),
        institutional_before,
        "Tier 1 resolution must never rewrite the institutional definition"
    );
}

#[test]
fn a16_prompt_templates_are_explicit_and_absence_is_reported() {
    for (site, source) in [
        ("workers", include_str!("handlers/workers.rs")),
        ("planners", include_str!("handlers/planners.rs")),
        ("coordination state", include_str!("../coordination/state.rs")),
    ] {
        assert!(
            !source.contains("prompt_template: None"),
            "{site} still passes an implicit missing role definition"
        );
        assert!(
            source.contains("role_prompt_template"),
            "{site} does not declare which role template it intends to resolve"
        );
    }

    let project = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(project.path().join(".ai-docs/roles"))
        .expect("create known-empty project role directory");
    let missing = resolve_role_definition(project.path(), None, "not-a-declared-role");
    assert!(
        missing.definition.is_none(),
        "an unknown role must not resolve to RoleDefinition::empty"
    );
    assert!(missing.issues.iter().any(|issue| {
        issue.kind == RoleResolutionIssueKind::DefinitionNotFound
            && issue.source_ref == "not-a-declared-role"
    }));
}

#[test]
fn a17_overlapping_role_and_task_source_is_loaded_once_in_spawn_prompt() {
    let reference = KnowledgeRef {
        source: KnowledgeSource::Project,
        pointer: "shared/review-contract.md".to_string(),
        summary: Some("Apply the repository review contract.".to_string()),
        priority: 90,
    };
    let mut role = RoleDefinition::empty("reviewer");
    role.version = 1;
    role.knowledge_scope.push(reference.clone());
    let resolved = resolved_for_test(role);
    let task = WorkNode::new(
        "T17",
        NodeKind::Task,
        "Review the implementation",
        NodeContract::default(),
        BindingRef::Role("reviewer".to_string()),
        NodeStatus::Ready,
    );
    let mut context_node = WorkNode::new(
        "context:T17",
        NodeKind::Context,
        "Apply the repository review contract.",
        NodeContract::default(),
        BindingRef::Zone("knowledge".to_string()),
        NodeStatus::Completed,
    );
    context_node.expansion = Some(CompositeExpansion {
        template: "derived-project-context".to_string(),
        parameters: BTreeMap::from([
            (
                "source_ref".to_string(),
                reference.pointer.clone(),
            ),
            (
                "summary".to_string(),
                reference.summary.clone().expect("reference summary"),
            ),
            ("priority".to_string(), "90".to_string()),
        ]),
    });
    let graph = TaskGraph::new(
        vec![task, context_node],
        vec![WorkEdge::new(
            "context:T17",
            "T17",
            EdgeKind::Informs,
            EdgeProvenance::Knowledge,
        )],
    );
    let spawn = spawn_context_from_work_graph_task(&graph, "T17");
    assert_eq!(spawn.task_scope, vec![reference]);
    let project = tempfile::tempdir().expect("temp project");
    let prompt = SessionController::build_worker_prompt(
        1,
        &AgentConfig {
            cli: "claude".to_string(),
            role: Some(WorkerRole::new("reviewer", "Reviewer", "claude")),
            ..AgentConfig::default()
        },
        &resolved,
        &spawn,
        "composition-queen",
        "composition-session",
        project.path(),
        project.path(),
        &HiveExecutionPolicy::default(),
    );

    assert_eq!(
        prompt.matches("project:shared/review-contract.md").count(),
        1,
        "the composed spawn prompt must emit an overlapping reference once"
    );
    assert!(prompt.contains("[role+task]"));
}

#[test]
fn a18_conflicting_role_and_task_guidance_is_rendered() {
    let mut role = RoleDefinition::empty("reviewer");
    role.version = 1;
    role.knowledge_scope.push(KnowledgeRef {
        source: KnowledgeSource::Project,
        pointer: "shared/review-contract.md".to_string(),
        summary: Some("Require independent evidence before approval.".to_string()),
        priority: 90,
    });
    let spawn = SpawnContext {
        task_scope: vec![KnowledgeRef {
            source: KnowledgeSource::Project,
            pointer: "shared/review-contract.md".to_string(),
            summary: Some("Approve once the implementation compiles.".to_string()),
            priority: 80,
        }],
        ..SpawnContext::default()
    };
    let composed = compose_context(Some(&role), &spawn);
    assert_eq!(composed.conflicts.len(), 1);
    let rendered = render_composed_context(&composed);

    assert!(rendered.contains("Context Conflicts"));
    assert!(rendered.contains("shared/review-contract.md"));
    assert!(rendered.contains("Require independent evidence before approval."));
    assert!(rendered.contains("Approve once the implementation compiles."));
}

#[test]
fn a19_budget_overflow_drops_by_priority_and_names_every_drop() {
    let mut role = RoleDefinition::empty("reviewer");
    role.knowledge_scope = vec![
        KnowledgeRef {
            source: KnowledgeSource::Project,
            pointer: "high.md".to_string(),
            summary: Some("highest".to_string()),
            priority: 100,
        },
        KnowledgeRef {
            source: KnowledgeSource::Project,
            pointer: "low.md".to_string(),
            summary: Some("low".to_string()),
            priority: 1,
        },
    ];
    let spawn = SpawnContext {
        budget: ContextBudget {
            role_chars: 60,
            ..ContextBudget::default()
        },
        ..SpawnContext::default()
    };
    let composed = compose_context(Some(&role), &spawn);

    assert_eq!(composed.knowledge.len(), 1);
    assert_eq!(composed.knowledge[0].reference.pointer, "high.md");
    assert_eq!(composed.knowledge[0].origin, ContextOrigin::Role);
    assert!(composed
        .dropped
        .iter()
        .any(|dropped| dropped.pointer == "low.md"));
    let rendered = render_composed_context(&composed);
    let knowledge_section = rendered
        .split("### Dropped Context")
        .next()
        .expect("knowledge section");
    assert!(!knowledge_section.contains("low.md"));
    assert!(rendered.contains("`low.md` [role]: role context budget exceeded"));
}

#[test]
fn a20_anti_hub_lint_flags_whole_tree_scope() {
    let mut reviewer = RoleDefinition::empty("reviewer");
    reviewer.knowledge_scope.push(KnowledgeRef {
        source: KnowledgeSource::Project,
        pointer: "*".to_string(),
        summary: None,
        priority: 0,
    });
    let tester = RoleDefinition::empty("tester");

    let lints = lint_role_knowledge_hubs(&[reviewer, tester]);
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].role_id, "reviewer");
    assert_eq!(lints[0].affected_role_ids, vec!["reviewer", "tester"]);
    assert_eq!(lints[0].role_fraction, 1.0);
}

#[test]
fn a20_anti_hub_lint_allows_one_shared_page() {
    let shared_reference = KnowledgeRef {
        source: KnowledgeSource::Institutional,
        pointer: "standards/review.md".to_string(),
        summary: None,
        priority: 0,
    };
    let mut reviewer = RoleDefinition::empty("reviewer");
    reviewer.knowledge_scope.push(shared_reference.clone());
    let mut tester = RoleDefinition::empty("tester");
    tester.knowledge_scope.push(shared_reference);

    assert!(lint_role_knowledge_hubs(&[reviewer, tester]).is_empty());
}

fn resolved_for_test(definition: RoleDefinition) -> ResolvedRoleDefinition {
    ResolvedRoleDefinition {
        requested_id: definition.id.clone(),
        definition: Some(definition),
        base_source: Some(RoleDefinitionSource::EmbeddedDefault),
        applied_override: None,
        issues: Vec::new(),
    }
}
