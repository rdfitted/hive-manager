//! Verifier-property HTTP integration tests are owned by P6.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use crate::domain::run_journal::Confidence;
use crate::domain::HiveExecutionPolicy;
use crate::orchestrator::org_graph::{
    adjudication::{
        AdjudicationDeclaration, AdjudicationPolicy, DeclaredAdjudicator,
        VerificationDuty,
    },
    boundary::{
        context_boundary_satisfies, includes_artifact_context,
        includes_spawner_conversation, required_context_boundary,
        verification_duty_declares_signal_class, verification_duty_has_named_signal,
    },
    composition::{ConversationContext, SpawnContext},
    definitions::resolve_role_definition,
    ContextBoundary, SignalClass,
};
use crate::orchestrator::work_graph::archive::{
    ArchiveSourceKind, ArchiveSourceReport, WorkGraphArchive,
    WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
};
use crate::orchestrator::work_graph::divergence::DivergenceSummary;
use crate::orchestrator::work_graph::retro::{
    evaluate_archives_with_role_attributions, propose_role_definition_refinements,
    AgentRoleDefinitionAttribution, IndependentEvaluator, RetroRunInput,
    RoleDefinitionKey, RoleDefinitionRefinementObservation, RoleRefinementSignal,
    ScopeImpact, UNREVIEWED_OUTCOME,
};
use crate::orchestrator::work_graph::runtime::{
    RuntimeEffect, RuntimeOutcome, RuntimeOutcomeStatus,
};
use crate::orchestrator::work_graph::review::{
    ADJUDICATION_DECLARATION_PARAMETER, MULTI_LENS_REVIEW_TEMPLATE,
    VERIFICATION_DUTY_PARAMETER,
};
use crate::orchestrator::work_graph::validate::{
    validate_plan_ready, PlanReadyError,
};
use crate::orchestrator::work_graph::{
    BindingRef, CompositeExpansion, NodeContract, NodeKind, NodeStatus,
    TaskGraph, WorkNode,
};
use crate::pty::{AgentConfig, WorkerRole};
use crate::session::SessionController;

const TASK_SENTINEL: &str = "TASK_SENTINEL_VERIFIER_OBJECTIVE";
const COMPOSER_SENTINEL: &str = "COMPOSER_SENTINEL_RENDERED_TASK_CONTEXT";
const ARTIFACT_SENTINEL: &str = "ARTIFACT_SENTINEL_INHERITED_CONTEXT";
const SPAWNER_CONVERSATION_POISON: &str =
    "SPAWNER_CONVERSATION_POISON_EXPECT_PRIOR_VERDICT";

#[test]
fn signal_class_derives_the_least_isolated_permitted_boundary() {
    assert_eq!(
        required_context_boundary(SignalClass::Mechanical),
        ContextBoundary::Full
    );
    assert_eq!(
        required_context_boundary(SignalClass::Judgmental),
        ContextBoundary::Artifact
    );
}

#[test]
fn judgmental_signals_reject_full_context_while_mechanical_signals_allow_it() {
    assert!(context_boundary_satisfies(
        SignalClass::Judgmental,
        ContextBoundary::None
    ));
    assert!(context_boundary_satisfies(
        SignalClass::Judgmental,
        ContextBoundary::Artifact
    ));
    assert!(!context_boundary_satisfies(
        SignalClass::Judgmental,
        ContextBoundary::Full
    ));

    for boundary in [
        ContextBoundary::None,
        ContextBoundary::Artifact,
        ContextBoundary::Full,
    ] {
        assert!(context_boundary_satisfies(
            SignalClass::Mechanical,
            boundary
        ));
    }
}

#[test]
fn verification_duties_require_a_named_signal_and_declared_class() {
    assert!(!verification_duty_has_named_signal(None));
    assert!(!verification_duty_has_named_signal(Some("")));
    assert!(!verification_duty_has_named_signal(Some(" \t\r\n")));
    assert!(verification_duty_has_named_signal(Some("cargo-test")));

    assert!(!verification_duty_declares_signal_class(None));
    assert!(verification_duty_declares_signal_class(Some(
        SignalClass::Mechanical
    )));
    assert!(verification_duty_declares_signal_class(Some(
        SignalClass::Judgmental
    )));
}

#[test]
fn context_inclusion_matches_each_declared_boundary() {
    assert!(!includes_artifact_context(ContextBoundary::None));
    assert!(!includes_spawner_conversation(ContextBoundary::None));

    assert!(includes_artifact_context(ContextBoundary::Artifact));
    assert!(!includes_spawner_conversation(ContextBoundary::Artifact));

    assert!(includes_artifact_context(ContextBoundary::Full));
    assert!(includes_spawner_conversation(ContextBoundary::Full));
}

fn verification_plan(duty: VerificationDuty) -> TaskGraph {
    let mut review = WorkNode::new(
        "review-verdict",
        NodeKind::Join,
        "Review verdict",
        NodeContract::default(),
        BindingRef::Role("evaluator".to_string()),
        NodeStatus::Pending,
    );
    review.expansion = Some(CompositeExpansion {
        template: MULTI_LENS_REVIEW_TEMPLATE.to_string(),
        parameters: BTreeMap::from([
            (
                VERIFICATION_DUTY_PARAMETER.to_string(),
                serde_json::to_string(&duty).expect("verification duty fixture"),
            ),
            (
                ADJUDICATION_DECLARATION_PARAMETER.to_string(),
                serde_json::to_string(&AdjudicationDeclaration {
                    policy: AdjudicationPolicy::HumanGate,
                    adjudicator: Some(DeclaredAdjudicator::new("evaluator")),
                })
                .expect("adjudication fixture"),
            ),
        ]),
    });
    TaskGraph::new(vec![review], Vec::new())
}

fn composed_verifier_prompt(boundary: ContextBoundary) -> String {
    let project = TempDir::new().expect("verifier prompt project");
    let mut resolved = resolve_role_definition(project.path(), None, "evaluator");
    resolved
        .definition
        .as_mut()
        .expect("embedded evaluator definition")
        .context_boundary = boundary;
    let config = AgentConfig {
        initial_prompt: Some(TASK_SENTINEL.to_string()),
        role: Some(WorkerRole::new("evaluator", "Evaluator", "claude")),
        ..AgentConfig::default()
    };
    let spawn_context = SpawnContext {
        task_summary: Some(COMPOSER_SENTINEL.to_string()),
        conversation: ConversationContext {
            artifact_summary: Some(ARTIFACT_SENTINEL.to_string()),
            spawner_conversation: Some(SPAWNER_CONVERSATION_POISON.to_string()),
        },
        ..SpawnContext::default()
    };

    SessionController::build_worker_prompt(
        1,
        &config,
        &resolved,
        &spawn_context,
        "verifier-queen",
        "verifier-session",
        project.path(),
        project.path(),
        &HiveExecutionPolicy::default(),
    )
}

#[test]
fn a24_none_boundary_removes_inherited_context_from_the_composed_prompt() {
    let prompt = composed_verifier_prompt(ContextBoundary::None);

    assert!(prompt.contains(TASK_SENTINEL));
    assert!(prompt.contains("## Composed Role and Task Context"));
    assert!(prompt.contains(COMPOSER_SENTINEL));
    assert!(!prompt.contains(ARTIFACT_SENTINEL));
    assert!(!prompt.contains(SPAWNER_CONVERSATION_POISON));
    assert!(!prompt.contains("### Artifact Context"));
    assert!(!prompt.contains("### Spawner Conversation"));
}

#[test]
fn a25_judgmental_full_context_fails_with_required_artifact() {
    let error = validate_plan_ready(&verification_plan(VerificationDuty {
        signal_name: Some("architecture-review".to_string()),
        signal_class: Some(SignalClass::Judgmental),
        context_boundary: ContextBoundary::Full,
    }))
    .expect_err("judgmental verification must reject full inherited context");

    assert_eq!(
        error,
        PlanReadyError::InsufficientVerificationIsolation {
            review_id: "review-verdict".to_string(),
            signal_class: SignalClass::Judgmental,
            actual: ContextBoundary::Full,
            required: ContextBoundary::Artifact,
        }
    );
    assert_eq!(
        error.to_string(),
        "PlanReady rejected: review review-verdict uses Full context for Judgmental signal; requires Artifact or stronger isolation"
    );
}

#[test]
fn a25_blank_verification_signal_fails_separately() {
    let error = validate_plan_ready(&verification_plan(VerificationDuty {
        signal_name: Some(" \t\r\n".to_string()),
        signal_class: Some(SignalClass::Judgmental),
        context_boundary: ContextBoundary::None,
    }))
    .expect_err("verification duties require a real named signal");

    assert_eq!(
        error,
        PlanReadyError::MissingVerificationSignal {
            review_id: "review-verdict".to_string(),
        }
    );
    assert_eq!(
        error.to_string(),
        "PlanReady rejected: review review-verdict has no named verification signal"
    );
}

#[test]
fn a26_mechanical_full_context_validates_and_keeps_spawner_conversation() {
    let validation = validate_plan_ready(&verification_plan(VerificationDuty {
        signal_name: Some("cargo-test".to_string()),
        signal_class: Some(SignalClass::Mechanical),
        context_boundary: ContextBoundary::Full,
    }))
    .expect("mechanical verification deliberately permits full inherited context");
    assert!(validation.warnings.is_empty());

    let prompt = composed_verifier_prompt(ContextBoundary::Full);
    assert!(prompt.contains(TASK_SENTINEL));
    assert!(prompt.contains(COMPOSER_SENTINEL));
    assert!(prompt.contains(ARTIFACT_SENTINEL));
    assert!(prompt.contains(SPAWNER_CONVERSATION_POISON));
    assert!(prompt.contains("### Spawner Conversation"));
}

fn verifier_evaluator() -> IndependentEvaluator {
    IndependentEvaluator::new(
        "independent-role-retro",
        vec!["planner".to_string()],
        vec!["supervisor".to_string()],
    )
    .expect("the evaluator is independent")
}

fn verifier_sources() -> Vec<ArchiveSourceReport> {
    [
        ArchiveSourceKind::PlanGraph,
        ArchiveSourceKind::EventLog,
        ArchiveSourceKind::RunJournal,
        ArchiveSourceKind::RunLedger,
        ArchiveSourceKind::MutationLog,
    ]
    .into_iter()
    .map(|kind| ArchiveSourceReport {
        kind,
        location: format!("fixture/{kind:?}"),
        available: true,
        record_count: 1,
        omissions: Vec::new(),
    })
    .collect()
}

fn role_archive(
    archive_id: &str,
    session_id: &str,
    agent_id: &str,
    attempts: usize,
    scope_gap: bool,
) -> WorkGraphArchive {
    let graph = TaskGraph::new(
        vec![WorkNode::new(
            "verification-task",
            NodeKind::Task,
            "Verification task",
            NodeContract::default(),
            BindingRef::Role("evaluator".to_string()),
            NodeStatus::Completed,
        )],
        Vec::new(),
    );
    let timestamp = Utc
        .timestamp_opt(1_900_000_000 + attempts as i64, 0)
        .single()
        .expect("fixture timestamp");
    let effects = scope_gap
        .then(|| RuntimeEffect {
            kind: "role_scope_gap".to_string(),
            reference: Some("verification-task".to_string()),
            confirmed: true,
            confidence: Confidence::High,
            source_ref: format!("ledger:{archive_id}:scope-gap"),
        })
        .into_iter()
        .collect();
    WorkGraphArchive {
        schema_version: WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
        archive_id: archive_id.to_string(),
        session_id: session_id.to_string(),
        archived_at: timestamp,
        plan_graph: Some(graph.clone()),
        runtime_graph: graph,
        deltas: Vec::new(),
        outcomes: vec![RuntimeOutcome {
            subject_id: "verification-task".to_string(),
            task_id: Some("verification-task".to_string()),
            agent_ids: vec![agent_id.to_string()],
            status: RuntimeOutcomeStatus::Completed,
            started_at: Some(timestamp),
            finished_at: Some(timestamp),
            attempt_count: attempts,
            effects,
            source_refs: vec![format!("event:{archive_id}")],
        }],
        divergence: DivergenceSummary::default(),
        sources: verifier_sources(),
    }
}

fn role_input(
    repo_id: &str,
    archive_id: &str,
    session_id: &str,
    agent_id: &str,
    attempts: usize,
    scope_gap: bool,
) -> RetroRunInput {
    RetroRunInput {
        repo_id: repo_id.to_string(),
        archive: role_archive(
            archive_id,
            session_id,
            agent_id,
            attempts,
            scope_gap,
        ),
    }
}

fn role_attribution(
    session_id: &str,
    agent_id: &str,
    definition_version: u32,
) -> AgentRoleDefinitionAttribution {
    AgentRoleDefinitionAttribution {
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        definition: RoleDefinitionKey {
            definition_id: "evaluator".to_string(),
            definition_version,
        },
    }
}

fn git_output(repo: &Path, args: &[&str]) -> String {
    let empty_hooks = repo.join(".hive-manager-test-empty-hooks");
    fs::create_dir_all(&empty_hooks).expect("empty repository-local hooks directory");
    let output = Command::new("git")
        .arg("-c")
        .arg("commit.gpgSign=false")
        .arg("-c")
        .arg(format!("core.hooksPath={}", empty_hooks.to_string_lossy()))
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git is available to the repository test suite");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn review_6_git_fixture_ignores_hostile_signing_and_hooks() {
    let temp = TempDir::new().expect("hostile git fixture root");
    git_output(temp.path(), &["init"]);
    git_output(temp.path(), &["config", "user.email", "retro@example.invalid"]);
    git_output(temp.path(), &["config", "user.name", "Retro Fixture"]);

    let hostile_hooks = temp.path().join("hostile-hooks");
    fs::create_dir_all(&hostile_hooks).expect("hostile hooks directory");
    fs::write(
        hostile_hooks.join("pre-commit"),
        "#!/bin/sh\necho hostile-hook >&2\nexit 73\n",
    )
    .expect("hostile pre-commit hook");
    let hostile_hooks = hostile_hooks.to_string_lossy().to_string();
    git_output(temp.path(), &["config", "commit.gpgSign", "true"]);
    git_output(
        temp.path(),
        &["config", "core.hooksPath", &hostile_hooks],
    );

    fs::write(temp.path().join("fixture.txt"), "hermetic fixture")
        .expect("fixture content");
    git_output(temp.path(), &["add", "fixture.txt"]);
    git_output(temp.path(), &["commit", "-m", "hermetic fixture"]);

    assert_eq!(
        git_output(temp.path(), &["log", "-1", "--pretty=%s"]),
        "hermetic fixture"
    );
}

#[test]
fn a30_retro_attribution_separates_versions_of_the_same_role_definition() {
    let inputs = [
        role_input("repo-a", "archive-v1", "session-v1", "agent-v1", 2, false),
        role_input("repo-a", "archive-v2", "session-v2", "agent-v2", 2, false),
    ];
    let attributions = [
        role_attribution("session-v1", "agent-v1", 1),
        role_attribution("session-v2", "agent-v2", 2),
    ];

    let report = evaluate_archives_with_role_attributions(
        &verifier_evaluator(),
        &inputs,
        &attributions,
    )
    .expect("role-attributed retro");

    assert_eq!(report.role_definition_aggregates.len(), 2);
    assert_eq!(
        report
            .role_definition_aggregates
            .iter()
            .map(|aggregate| aggregate.definition.definition_version)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(report
        .role_definition_aggregates
        .iter()
        .all(|aggregate| aggregate.run_count == 1));
    assert!(report.role_refinement_proposals.is_empty());
}

#[test]
fn review_6_scope_gaps_count_once_per_definition_and_preserve_contributors() {
    let mut shared = role_input(
        "repo-a",
        "archive-shared",
        "session-shared",
        "agent-a",
        1,
        true,
    );
    shared.archive.outcomes[0]
        .agent_ids
        .push("agent-b".to_string());
    let shared_report = evaluate_archives_with_role_attributions(
        &verifier_evaluator(),
        &[shared],
        &[
            role_attribution("session-shared", "agent-a", 11),
            role_attribution("session-shared", "agent-b", 11),
        ],
    )
    .expect("shared-definition scope-gap retro");

    let shared_metric = &shared_report.runs[0].role_definitions[0];
    assert_eq!(shared_metric.confirmed_scope_gaps, 1);
    assert_eq!(shared_metric.agent_ids, vec!["agent-a", "agent-b"]);
    assert!(shared_metric
        .evidence_refs
        .iter()
        .any(|reference| reference == "event:archive-shared"));
    assert!(shared_metric
        .evidence_refs
        .iter()
        .any(|reference| reference == "ledger:archive-shared:scope-gap"));
    assert!(shared_report.role_refinement_proposals.is_empty());

    let mut distinct = role_input(
        "repo-a",
        "archive-distinct",
        "session-distinct",
        "agent-c",
        1,
        true,
    );
    distinct.archive.outcomes[0]
        .agent_ids
        .push("agent-d".to_string());
    let distinct_report = evaluate_archives_with_role_attributions(
        &verifier_evaluator(),
        &[distinct],
        &[
            role_attribution("session-distinct", "agent-c", 21),
            role_attribution("session-distinct", "agent-d", 22),
        ],
    )
    .expect("distinct-definition scope-gap retro");
    assert_eq!(
        distinct_report.runs[0]
            .role_definitions
            .iter()
            .map(|metric| (
                metric.definition.definition_version,
                metric.confirmed_scope_gaps,
            ))
            .collect::<Vec<_>>(),
        vec![(21, 1), (22, 1)]
    );
}

#[test]
fn review_6_optional_totals_serialize_their_contributing_run_counts() {
    let complete = role_input(
        "repo-a",
        "archive-complete",
        "session-complete",
        "agent-complete",
        2,
        false,
    );
    let mut unavailable = role_input(
        "repo-a",
        "archive-unavailable",
        "session-unavailable",
        "agent-unavailable",
        2,
        false,
    );
    unavailable.archive.plan_graph = None;
    for source in &mut unavailable.archive.sources {
        if matches!(
            source.kind,
            ArchiveSourceKind::EventLog | ArchiveSourceKind::MutationLog
        ) {
            source.available = false;
            source.record_count = 0;
        }
    }
    let report = evaluate_archives_with_role_attributions(
        &verifier_evaluator(),
        &[complete, unavailable],
        &[
            role_attribution("session-complete", "agent-complete", 31),
            role_attribution("session-unavailable", "agent-unavailable", 31),
        ],
    )
    .expect("partially available role aggregates");

    let aggregate = &report.role_definition_aggregates[0];
    assert_eq!(aggregate.run_count, 2);
    let serialized = serde_json::to_value(aggregate).expect("serialized role aggregate");
    for field in [
        "additional_attempts_contributing_runs",
        "remediation_detours_contributing_runs",
        "caught_defects_contributing_runs",
        "escaped_defects_contributing_runs",
        "gotcha_edges_eligible_contributing_runs",
        "gotcha_targets_attempted_contributing_runs",
    ] {
        assert_eq!(serialized[field], serde_json::json!(1), "{field}");
    }
}

#[test]
fn a31_role_refinement_is_unreviewed_and_does_not_apply_or_commit() {
    let temp = TempDir::new().expect("definition canary root");
    let definition_path = temp.path().join("roles").join("evaluator.md");
    fs::create_dir_all(definition_path.parent().unwrap()).expect("roles directory");
    fs::write(&definition_path, "definition-canary").expect("definition canary");
    git_output(temp.path(), &["init"]);
    git_output(temp.path(), &["config", "user.email", "retro@example.invalid"]);
    git_output(temp.path(), &["config", "user.name", "Retro Fixture"]);
    git_output(temp.path(), &["add", "."]);
    git_output(temp.path(), &["commit", "-m", "definition canary"]);
    let head_before = git_output(temp.path(), &["rev-parse", "HEAD"]);
    let inputs = [
        role_input("repo-a", "archive-a", "session-a", "agent-a", 2, false),
        role_input("repo-a", "archive-b", "session-b", "agent-b", 2, false),
    ];
    let attributions = [
        role_attribution("session-a", "agent-a", 7),
        role_attribution("session-b", "agent-b", 7),
    ];

    let report = evaluate_archives_with_role_attributions(
        &verifier_evaluator(),
        &inputs,
        &attributions,
    )
    .expect("role refinement retro");

    assert_eq!(report.role_refinement_proposals.len(), 1);
    let proposal = &report.role_refinement_proposals[0];
    assert_eq!(proposal.definition.definition_id, "evaluator");
    assert_eq!(proposal.definition.definition_version, 7);
    assert_eq!(proposal.observation_count, 2);
    assert_eq!(proposal.repo_ids, vec!["repo-a"]);
    let learning = report
        .learning_submissions
        .iter()
        .find(|learning| learning.keywords.iter().any(|item| item == "evaluator@7"))
        .expect("role proposal uses the existing learning submission channel");
    assert_eq!(learning.outcome, UNREVIEWED_OUTCOME);
    assert!(learning.files_touched.is_empty());

    let guard_observations = ["guard-a", "guard-b"].map(|id| {
        RoleDefinitionRefinementObservation {
            repo_id: temp.path().display().to_string(),
            session_id: format!("session-{id}"),
            archive_id: format!("archive-{id}"),
            definition: RoleDefinitionKey {
                definition_id: "evaluator".to_string(),
                definition_version: 7,
            },
            signal: RoleRefinementSignal::AdditionalAttempts,
            evidence_refs: vec![definition_path.display().to_string()],
        }
    });
    assert_eq!(
        propose_role_definition_refinements(&guard_observations).len(),
        1
    );
    assert_eq!(
        fs::read_to_string(&definition_path).unwrap(),
        "definition-canary"
    );
    assert_eq!(git_output(temp.path(), &["rev-parse", "HEAD"]), head_before);
    assert!(git_output(temp.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn a32_institutional_promotion_uses_distinct_repos_not_observation_count() {
    let same_repo_inputs = [
        role_input("repo-a", "archive-1", "session-1", "agent-1", 2, false),
        role_input("repo-a", "archive-2", "session-2", "agent-2", 2, false),
        role_input("repo-a", "archive-3", "session-3", "agent-3", 2, false),
    ];
    let same_repo_attributions = [
        role_attribution("session-1", "agent-1", 3),
        role_attribution("session-2", "agent-2", 3),
        role_attribution("session-3", "agent-3", 3),
    ];
    let same_repo = evaluate_archives_with_role_attributions(
        &verifier_evaluator(),
        &same_repo_inputs,
        &same_repo_attributions,
    )
    .expect("same-repo retro");
    assert_eq!(
        same_repo.role_refinement_proposals[0].tier,
        crate::orchestrator::work_graph::archetypes::PromotionTier::ProjectOverride
    );

    let multi_repo_inputs = [
        role_input("repo-a", "archive-x", "session-x", "agent-x", 2, false),
        role_input("repo-b", "archive-y", "session-y", "agent-y", 2, false),
    ];
    let multi_repo_attributions = [
        role_attribution("session-x", "agent-x", 3),
        role_attribution("session-y", "agent-y", 3),
    ];
    let multi_repo = evaluate_archives_with_role_attributions(
        &verifier_evaluator(),
        &multi_repo_inputs,
        &multi_repo_attributions,
    )
    .expect("multi-repo retro");
    assert_eq!(
        multi_repo.role_refinement_proposals[0].tier,
        crate::orchestrator::work_graph::archetypes::PromotionTier::InstitutionalRevision
    );
    assert_eq!(multi_repo.role_refinement_proposals[0].repo_ids.len(), 2);
}

#[test]
fn widening_and_narrowing_role_refinements_are_flagged_distinctly() {
    let definition = RoleDefinitionKey {
        definition_id: "evaluator".to_string(),
        definition_version: 9,
    };
    let widening_inputs = [
        role_input("repo-a", "scope-a", "scope-session-a", "scope-agent-a", 1, true),
        role_input("repo-a", "scope-b", "scope-session-b", "scope-agent-b", 1, true),
    ];
    let widening_attributions = [
        role_attribution("scope-session-a", "scope-agent-a", 9),
        role_attribution("scope-session-b", "scope-agent-b", 9),
    ];
    let widening = evaluate_archives_with_role_attributions(
        &verifier_evaluator(),
        &widening_inputs,
        &widening_attributions,
    )
    .expect("scope-gap retro");
    assert!(widening.role_refinement_proposals.iter().any(|proposal| {
        proposal.change_key == "declared_scope_gap"
            && proposal.scope_impact == ScopeImpact::Widening
            && proposal.rationale.contains("suspect by default")
    }));

    let observations = ["review-a", "review-b"]
    .into_iter()
    .map(|id| RoleDefinitionRefinementObservation {
        repo_id: "repo-a".to_string(),
        session_id: format!("session-{id}"),
        archive_id: format!("archive-{id}"),
        definition: definition.clone(),
        signal: RoleRefinementSignal::ReviewEscapes,
        evidence_refs: vec![format!("evidence:{id}")],
    })
    .collect::<Vec<_>>();

    let proposals = propose_role_definition_refinements(&observations);
    assert_eq!(proposals.len(), 1);
    assert!(proposals.iter().any(|proposal| {
        proposal.change_key == "review_escapes"
            && proposal.scope_impact == ScopeImpact::Narrowing
    }));
}
