//! Post-run graph retro tests for issue #217, owned by WS-10.

use std::collections::BTreeMap;
use std::fs;

use chrono::{Duration, TimeZone, Utc};
use tempfile::TempDir;

use crate::domain::run_journal::Confidence;
use crate::orchestrator::work_graph::archive::{
    ArchiveSourceKind, ArchiveSourceReport, WorkGraphArchive,
    WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
};
use crate::orchestrator::work_graph::divergence::{
    DivergenceKind, DivergenceRecord, DivergenceSummary,
};
use crate::orchestrator::work_graph::retro::{
    evaluate_archives, EvidenceMetric, IndependentEvaluator,
    ReviewEvidenceState, RetroOmissionReason, RetroRunInput,
    UNREVIEWED_OUTCOME,
};
use crate::orchestrator::work_graph::review::MULTI_LENS_REVIEW_TEMPLATE;
use crate::orchestrator::work_graph::runtime::{
    GraphMutationDelta, GraphMutationType, RuntimeEffect, RuntimeOutcome,
    RuntimeOutcomeStatus,
};
use crate::orchestrator::work_graph::{
    BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract,
    NodeKind, NodeStatus, TaskGraph, WorkEdge, WorkNode,
};
use crate::orchestrator::work_graph::review::JUDGE_PRINCE_REMEDIATION_TEMPLATE;

fn instant(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_900_000_000 + seconds, 0)
        .single()
        .expect("fixture timestamp")
}

fn node(id: &str, kind: NodeKind) -> WorkNode {
    WorkNode::new(
        id,
        kind,
        format!("Node {id}"),
        NodeContract::default(),
        BindingRef::Role("worker".to_string()),
        NodeStatus::Pending,
    )
}

fn lineage(mut node: WorkNode, template: &str, version: u32) -> WorkNode {
    node.expansion = Some(CompositeExpansion {
        template: "graph-archetype".to_string(),
        parameters: BTreeMap::from([
            ("template_id".to_string(), template.to_string()),
            ("template_version".to_string(), version.to_string()),
            ("lane_id".to_string(), node.id.clone()),
        ]),
    });
    node
}

fn source(kind: ArchiveSourceKind, available: bool) -> ArchiveSourceReport {
    ArchiveSourceReport {
        kind,
        location: format!("fixture/{kind:?}"),
        available,
        record_count: usize::from(available),
        omissions: Vec::new(),
    }
}

fn sources() -> Vec<ArchiveSourceReport> {
    vec![
        source(ArchiveSourceKind::PlanGraph, true),
        source(ArchiveSourceKind::EventLog, true),
        source(ArchiveSourceKind::RunJournal, true),
        source(ArchiveSourceKind::RunLedger, true),
        source(ArchiveSourceKind::MutationLog, true),
    ]
}

fn outcome(
    id: &str,
    status: RuntimeOutcomeStatus,
    started_at: Option<chrono::DateTime<Utc>>,
    finished_at: Option<chrono::DateTime<Utc>>,
    attempts: usize,
) -> RuntimeOutcome {
    RuntimeOutcome {
        subject_id: id.to_string(),
        task_id: Some(id.to_string()),
        agent_ids: vec![format!("agent-{id}")],
        status,
        started_at,
        finished_at,
        attempt_count: attempts,
        effects: Vec::new(),
        source_refs: vec![format!("event:{id}")],
    }
}

fn archive(
    id: &str,
    session_id: &str,
    archived_at: chrono::DateTime<Utc>,
    plan_graph: Option<TaskGraph>,
    runtime_graph: TaskGraph,
    deltas: Vec<GraphMutationDelta>,
    outcomes: Vec<RuntimeOutcome>,
    divergence: DivergenceSummary,
) -> WorkGraphArchive {
    WorkGraphArchive {
        schema_version: WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
        archive_id: id.to_string(),
        session_id: session_id.to_string(),
        archived_at,
        plan_graph,
        runtime_graph,
        deltas,
        outcomes,
        divergence,
        sources: sources(),
    }
}

fn evaluator() -> IndependentEvaluator {
    IndependentEvaluator::new(
        "independent-retro",
        vec!["planner".to_string()],
        vec!["supervisor".to_string()],
    )
    .expect("the evaluator did not plan or supervise these runs")
}

fn added_node_archive(
    archive_id: &str,
    session_id: &str,
    archived_at: chrono::DateTime<Utc>,
    template: &str,
    version: u32,
) -> WorkGraphArchive {
    let plan = TaskGraph::new(
        vec![lineage(node("task-a", NodeKind::Task), template, version)],
        Vec::new(),
    );
    let mut actual = plan.clone();
    actual.nodes.push(node("runtime-added", NodeKind::Task));
    let record = DivergenceRecord {
        kind: DivergenceKind::NodeAdded,
        node_id: Some("runtime-added".to_string()),
        source: None,
        target: None,
        replacement_source: None,
        replacement_target: None,
    };
    archive(
        archive_id,
        session_id,
        archived_at,
        Some(plan),
        actual,
        Vec::new(),
        vec![outcome(
            "task-a",
            RuntimeOutcomeStatus::Completed,
            Some(archived_at - Duration::seconds(5)),
            Some(archived_at),
            1,
        )],
        DivergenceSummary {
            counts_by_mutation_type: BTreeMap::from([(DivergenceKind::NodeAdded, 1)]),
            recorded_runtime_mutations: BTreeMap::new(),
            records: vec![record],
        },
    )
}

#[test]
fn completed_retro_returns_metrics_and_unreviewed_submissions_without_writes() {
    let temp = TempDir::new().expect("temp workspace");
    let template_path = temp.path().join("template.json");
    let wiki_path = temp.path().join("wiki.md");
    let learning_path = temp.path().join("learnings.jsonl");
    fs::write(&template_path, b"template-before").expect("seed template");
    fs::write(&wiki_path, b"wiki-before").expect("seed wiki");
    fs::write(&learning_path, b"learning-before\n").expect("seed learnings");
    let snapshots = [
        fs::read(&template_path).expect("template snapshot"),
        fs::read(&wiki_path).expect("wiki snapshot"),
        fs::read(&learning_path).expect("learning snapshot"),
    ];

    let inputs = vec![
        RetroRunInput {
            repo_id: "repo-a".to_string(),
            archive: added_node_archive("archive-a", "session-a", instant(10), "feature", 1),
        },
        RetroRunInput {
            repo_id: "repo-a".to_string(),
            archive: added_node_archive("archive-b", "session-b", instant(20), "feature", 1),
        },
    ];
    let report = evaluate_archives(&evaluator(), &inputs).expect("retro evaluates");

    assert_eq!(report.runs.len(), 2);
    assert!(report.runs.iter().all(|run| matches!(
        run.edit_distance,
        EvidenceMetric::Available { .. }
    )));
    assert_eq!(report.learning_submissions.len(), 1);
    assert!(report.learning_submissions.iter().all(|learning| {
        learning.outcome == UNREVIEWED_OUTCOME
            && learning.endpoint_path()
                == format!("/api/sessions/{}/learnings", learning.session)
    }));
    assert_eq!(fs::read(template_path).expect("template after"), snapshots[0]);
    assert_eq!(fs::read(wiki_path).expect("wiki after"), snapshots[1]);
    assert_eq!(fs::read(learning_path).expect("learning after"), snapshots[2]);
}

#[test]
fn systematic_divergence_requires_two_runs_of_the_same_template_version() {
    let first = RetroRunInput {
        repo_id: "repo-a".to_string(),
        archive: added_node_archive("archive-1", "session-1", instant(10), "feature", 3),
    };
    let single = evaluate_archives(&evaluator(), std::slice::from_ref(&first))
        .expect("single-run retro");
    assert!(
        single.promotion_proposals.is_empty(),
        "one occurrence is noise and must not emit a promotion proposal"
    );

    let second = RetroRunInput {
        repo_id: "repo-a".to_string(),
        archive: added_node_archive("archive-2", "session-2", instant(20), "feature", 3),
    };
    let report =
        evaluate_archives(&evaluator(), &[first, second]).expect("two-run retro");
    assert_eq!(report.template_aggregates.len(), 1);
    assert_eq!(report.template_aggregates[0].run_count, 2);
    assert_eq!(report.promotion_proposals.len(), 1);
    assert_eq!(report.promotion_proposals[0].observation_count, 2);
    assert_eq!(report.promotion_proposals[0].archetype_id, "feature@3");

    let other_version = RetroRunInput {
        repo_id: "repo-a".to_string(),
        archive: added_node_archive("archive-3", "session-3", instant(30), "feature", 4),
    };
    let split_versions = evaluate_archives(
        &evaluator(),
        &[
            RetroRunInput {
                repo_id: "repo-a".to_string(),
                archive: added_node_archive(
                    "archive-4",
                    "session-4",
                    instant(40),
                    "feature",
                    3,
                ),
            },
            other_version,
        ],
    )
    .expect("version-separated retro");
    assert!(split_versions.promotion_proposals.is_empty());
}

fn review_plan(status: NodeStatus) -> TaskGraph {
    let target = lineage(node("target", NodeKind::Task), "reviewed-feature", 1);
    let mut verdict = node("verdict", NodeKind::Join);
    verdict.status = status;
    verdict.expansion = Some(CompositeExpansion {
        template: MULTI_LENS_REVIEW_TEMPLATE.to_string(),
        parameters: BTreeMap::from([
            ("target".to_string(), "target".to_string()),
            ("round".to_string(), "0".to_string()),
        ]),
    });
    TaskGraph::new(vec![target, verdict], Vec::new())
}

#[test]
fn later_explicit_escape_evidence_revises_an_earlier_review_verdict() {
    let before = review_plan(NodeStatus::Pending);
    let after = review_plan(NodeStatus::Completed);
    let old = archive(
        "archive-old",
        "session-old",
        instant(10),
        Some(before.clone()),
        after.clone(),
        vec![GraphMutationDelta {
            sequence: 1,
            observed_at: instant(9),
            mutation_type: GraphMutationType::ReviewVerdictRecorded,
            before,
            after,
            source_refs: vec!["verdict:verdict".to_string()],
        }],
        Vec::new(),
        DivergenceSummary::default(),
    );
    let later_plan = TaskGraph::new(
        vec![lineage(node("target", NodeKind::Task), "reviewed-feature", 1)],
        Vec::new(),
    );
    let mut later_outcome = outcome(
        "target",
        RuntimeOutcomeStatus::Failed,
        Some(instant(19)),
        Some(instant(20)),
        1,
    );
    later_outcome.effects.push(RuntimeEffect {
        kind: "review_escape".to_string(),
        reference: Some("archive-old#verdict".to_string()),
        confirmed: true,
        confidence: Confidence::High,
        source_ref: "ledger:escape-1".to_string(),
    });
    let later = archive(
        "archive-later",
        "session-later",
        instant(20),
        Some(later_plan.clone()),
        later_plan,
        Vec::new(),
        vec![later_outcome],
        DivergenceSummary::default(),
    );
    let report = evaluate_archives(
        &evaluator(),
        &[
            RetroRunInput {
                repo_id: "repo-a".to_string(),
                archive: old,
            },
            RetroRunInput {
                repo_id: "repo-a".to_string(),
                archive: later,
            },
        ],
    )
    .expect("review retro");
    let reviews = report.runs[0]
        .reviews
        .value()
        .expect("review evidence is available");
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].state, ReviewEvidenceState::Escaped);
    assert_eq!(reviews[0].escaped_defects, 1);
    assert_eq!(reviews[0].revisions.len(), 1);
    assert_eq!(
        reviews[0].revisions[0].discovering_archive_id,
        "archive-later"
    );
}

#[test]
fn absent_plan_and_unsupported_schema_are_reported_instead_of_zeroed() {
    let empty = evaluate_archives(&evaluator(), &[]).expect("empty corpus report");
    assert!(empty
        .omissions
        .iter()
        .any(|omission| omission.reason == RetroOmissionReason::NoArchives));

    let legacy = archive(
        "legacy",
        "legacy-session",
        instant(10),
        None,
        TaskGraph::default(),
        Vec::new(),
        Vec::new(),
        DivergenceSummary::default(),
    );
    let legacy_report = evaluate_archives(
        &evaluator(),
        &[RetroRunInput {
            repo_id: "repo-a".to_string(),
            archive: legacy,
        }],
    )
    .expect("legacy report");
    assert!(legacy_report.omissions.iter().any(|omission| {
        omission.reason == RetroOmissionReason::PlanGraphUnavailable
    }));
    assert!(matches!(
        legacy_report.runs[0].edit_distance,
        EvidenceMetric::Unavailable { .. }
    ));

    let mut unsupported = added_node_archive(
        "future",
        "future-session",
        instant(20),
        "feature",
        1,
    );
    unsupported.schema_version = WORK_GRAPH_ARCHIVE_SCHEMA_VERSION + 1;
    let unsupported_report = evaluate_archives(
        &evaluator(),
        &[RetroRunInput {
            repo_id: "repo-a".to_string(),
            archive: unsupported,
        }],
    )
    .expect("unsupported report");
    assert!(unsupported_report.runs.is_empty());
    assert!(unsupported_report.omissions.iter().any(|omission| {
        omission.reason == RetroOmissionReason::UnsupportedSchemaVersion
    }));
}

#[test]
fn checkpoint_idle_is_attributed_to_each_checkpoint_without_causal_claims() {
    let fast = lineage(node("fast", NodeKind::Task), "parallel-wave", 2);
    let slow = lineage(node("slow", NodeKind::Task), "parallel-wave", 2);
    let checkpoint = lineage(
        node("barrier", NodeKind::Checkpoint),
        "parallel-wave",
        2,
    );
    let plan = TaskGraph::new(
        vec![fast, slow, checkpoint],
        vec![
            WorkEdge::new(
                "fast",
                "barrier",
                EdgeKind::DependsOn,
                EdgeProvenance::Planner,
            ),
            WorkEdge::new(
                "slow",
                "barrier",
                EdgeKind::DependsOn,
                EdgeProvenance::Planner,
            ),
        ],
    );
    let archived = archive(
        "checkpoint-archive",
        "checkpoint-session",
        instant(30),
        Some(plan.clone()),
        plan,
        Vec::new(),
        vec![
            outcome(
                "fast",
                RuntimeOutcomeStatus::Completed,
                Some(instant(1)),
                Some(instant(10)),
                1,
            ),
            outcome(
                "slow",
                RuntimeOutcomeStatus::Completed,
                Some(instant(1)),
                Some(instant(20)),
                1,
            ),
            outcome(
                "barrier",
                RuntimeOutcomeStatus::Running,
                Some(instant(25)),
                None,
                1,
            ),
        ],
        DivergenceSummary::default(),
    );
    let report = evaluate_archives(
        &evaluator(),
        &[RetroRunInput {
            repo_id: "repo-a".to_string(),
            archive: archived,
        }],
    )
    .expect("checkpoint retro");
    let checkpoints = report.runs[0]
        .checkpoints
        .value()
        .expect("checkpoint timing available");
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].checkpoint_id, "barrier");
    assert_eq!(checkpoints[0].sibling_barrier_idle_millis, Some(10_000));
    assert_eq!(checkpoints[0].gate_release_delay_millis, Some(5_000));
    assert_eq!(
        checkpoints[0].total_pre_checkpoint_wait_millis,
        Some(20_000)
    );
}

#[test]
fn per_node_attempts_remediation_and_gotcha_reach_are_evidence_backed() {
    let task = lineage(node("task-a", NodeKind::Task), "evidence", 1);
    let context = node("gotcha", NodeKind::Context);
    let plan = TaskGraph::new(
        vec![task, context],
        vec![WorkEdge::new(
            "gotcha",
            "task-a",
            EdgeKind::Informs,
            EdgeProvenance::Knowledge,
        )],
    );
    let mut after = plan.clone();
    let mut remediation = node("remediation", NodeKind::Task);
    remediation.expansion = Some(CompositeExpansion {
        template: JUDGE_PRINCE_REMEDIATION_TEMPLATE.to_string(),
        parameters: BTreeMap::from([(
            "target".to_string(),
            "task-a".to_string(),
        )]),
    });
    after.nodes.push(remediation);
    let archived = archive(
        "evidence-archive",
        "evidence-session",
        instant(30),
        Some(plan.clone()),
        after.clone(),
        vec![GraphMutationDelta {
            sequence: 1,
            observed_at: instant(20),
            mutation_type: GraphMutationType::RemediationDetour,
            before: plan,
            after,
            source_refs: vec!["verdict:failed-review".to_string()],
        }],
        vec![outcome(
            "task-a",
            RuntimeOutcomeStatus::Completed,
            Some(instant(1)),
            Some(instant(30)),
            3,
        )],
        DivergenceSummary::default(),
    );
    let report = evaluate_archives(
        &evaluator(),
        &[RetroRunInput {
            repo_id: "repo-a".to_string(),
            archive: archived,
        }],
    )
    .expect("evidence retro");
    let nodes = report.runs[0]
        .nodes
        .value()
        .expect("node evidence is available");
    let task_metric = nodes
        .iter()
        .find(|metric| metric.node_id == "task-a")
        .expect("task metric");
    assert_eq!(task_metric.additional_attempts, Some(2));
    assert_eq!(task_metric.remediation_detours, Some(1));
    let gotcha = report.runs[0]
        .gotcha_edge_hit_rate
        .value()
        .expect("gotcha reach is available");
    assert_eq!(gotcha.eligible_knowledge_edges, 1);
    assert_eq!(gotcha.targets_attempted, 1);
    assert!(gotcha.rate_defined);
}

#[test]
fn evaluator_that_planned_or_supervised_the_run_is_rejected() {
    assert!(IndependentEvaluator::new(
        "same-agent",
        vec!["same-agent".to_string()],
        Vec::<String>::new(),
    )
    .is_err());
    assert!(IndependentEvaluator::new(
        "same-agent",
        Vec::<String>::new(),
        vec!["same-agent".to_string()],
    )
    .is_err());
}
