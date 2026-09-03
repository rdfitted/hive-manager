//! Runtime-graph tests for issue #214, owned by WS-5.

use std::fs;
use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use tempfile::TempDir;

use crate::coordination::{HierarchyNode, StateManager};
use crate::domain::event::{Event, EventType, Severity};
use crate::domain::run_journal::{Confidence, LedgerEntry, RunJournalEntry, StepKind, StepStatus};
use crate::events::EventBus;
use crate::http::handlers::workers::ExecutedAs;
use crate::orchestrator::work_graph::archive::{
    archive_completed_session, list_archives, read_archive, schedule_completed_session_archive,
    ArchiveSourceKind, ArchiveSourceReport, WorkGraphArchive, WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
};
use crate::orchestrator::work_graph::completion_ledger::{
    append_node_completion_facts, NodeCompletionFact, NodeCompletionProvenance,
};
use crate::orchestrator::work_graph::divergence::{compute_divergence, DivergenceKind};
use crate::orchestrator::work_graph::retro::{
    evaluate_archives, IndependentEvaluator, RetroRunInput,
};
use crate::orchestrator::work_graph::review::{instantiate_review_templates, ReviewTemplate};
use crate::orchestrator::work_graph::runtime::{
    derive_runtime_graph, derive_runtime_graph_with_completion_facts,
    derive_runtime_graph_with_principals, instantiate_review_templates_and_record,
    mutate_and_record, mutation_log, reconstruct_structural_history, record_graph_change,
    record_review_verdict_and_record, route_failed_verdict_and_record, CompletionEvidenceClass,
    GraphMutationDelta, GraphMutationType, ReviewVerdict, RuntimeOutcomeStatus,
};
use crate::orchestrator::work_graph::{
    BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus, TaskGraph, WorkEdge,
    WorkGraphOmissionReason, WorkNode,
};
use crate::storage::{ApplicationStateDb, RunJournalStore, SessionStorage};

fn task(id: &str, outputs: &[&str]) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Task,
        format!("Task {id}"),
        NodeContract {
            inputs: Vec::new(),
            outputs: outputs.iter().map(|value| value.to_string()).collect(),
            acceptance: vec![format!("{id} passes")],
        },
        BindingRef::Role("backend".to_string()),
        NodeStatus::Pending,
    )
}

fn event(
    id: &str,
    event_type: EventType,
    agent_id: Option<&str>,
    payload: serde_json::Value,
) -> Event {
    Event {
        id: id.to_string(),
        session_id: "runtime-session".to_string(),
        cell_id: Some("primary".to_string()),
        agent_id: agent_id.map(str::to_string),
        event_type,
        timestamp: Utc::now(),
        payload,
        severity: Severity::Info,
    }
}

fn evaluate_runtime_retro(
    plan: &TaskGraph,
    runtime: crate::orchestrator::work_graph::runtime::RuntimeDerivation,
    event_count: usize,
) -> crate::orchestrator::work_graph::retro::RetroReport {
    let archive = WorkGraphArchive {
        schema_version: WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
        archive_id: "runtime-test-archive".to_string(),
        session_id: "runtime-session".to_string(),
        archived_at: Utc::now(),
        plan_graph: Some(plan.clone()),
        divergence: compute_divergence(Some(plan), &runtime.runtime_graph, &[]),
        runtime_graph: runtime.runtime_graph,
        deltas: Vec::new(),
        outcomes: runtime.outcomes,
        sources: vec![
            ArchiveSourceReport {
                kind: ArchiveSourceKind::EventLog,
                location: "events.jsonl".to_string(),
                available: true,
                record_count: event_count,
                omissions: Vec::new(),
            },
            ArchiveSourceReport {
                kind: ArchiveSourceKind::MutationLog,
                location: "memory/session-mutation-log".to_string(),
                available: true,
                record_count: 0,
                omissions: Vec::new(),
            },
        ],
    };
    let evaluator = IndependentEvaluator::new(
        "runtime-test-evaluator",
        Vec::<String>::new(),
        Vec::<String>::new(),
    )
    .unwrap();
    evaluate_archives(
        &evaluator,
        &[RetroRunInput {
            repo_id: "runtime-test-repo".to_string(),
            archive,
        }],
    )
    .unwrap()
}

#[test]
fn retry_records_total_attempts_and_retro_reports_one_additional_attempt() {
    let plan = TaskGraph::new(vec![task("task-a", &["code"])], Vec::new());
    let events = vec![
        event(
            "claim",
            EventType::WorkerClaimed,
            Some("worker-a"),
            json!({"worker_id":"worker-a","task_id":"task-a"}),
        ),
        event(
            "retry",
            EventType::WorkerReclaimed,
            Some("worker-a"),
            json!({"worker_id":"worker-a","task_id":"task-a"}),
        ),
        event(
            "complete",
            EventType::AgentCompleted,
            Some("worker-a"),
            json!({}),
        ),
    ];

    let derived = derive_runtime_graph(Some(&plan), &events, &[], &[], &[]);
    let outcome = derived
        .outcomes
        .iter()
        .find(|outcome| outcome.task_id.as_deref() == Some("task-a"))
        .unwrap();
    assert_eq!(
        outcome.attempt_count, 2,
        "attempt_count stores total attempts"
    );

    let report = evaluate_runtime_retro(&plan, derived, events.len());
    let node_metrics = report.runs[0].nodes.value().unwrap();
    let task_metric = node_metrics
        .iter()
        .find(|metric| metric.node_id == "task-a")
        .unwrap();
    assert_eq!(task_metric.additional_attempts, Some(1));
}

#[test]
fn agent_completion_records_finished_at() {
    let plan = TaskGraph::new(vec![task("task-a", &["code"])], Vec::new());
    let claim_at = chrono::DateTime::parse_from_rfc3339("2026-08-16T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let completion_at = chrono::DateTime::parse_from_rfc3339("2026-08-16T12:01:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut claim = event(
        "claim",
        EventType::WorkerClaimed,
        Some("worker-a"),
        json!({"worker_id":"worker-a","task_id":"task-a"}),
    );
    claim.timestamp = claim_at;
    let mut completion = event(
        "complete",
        EventType::AgentCompleted,
        Some("worker-a"),
        json!({}),
    );
    completion.timestamp = completion_at;
    let expected_finished_at = completion.timestamp;
    let events = vec![claim, completion];

    let derived = derive_runtime_graph(Some(&plan), &events, &[], &[], &[]);
    let outcome = derived
        .outcomes
        .iter()
        .find(|outcome| outcome.task_id.as_deref() == Some("task-a"))
        .unwrap();
    assert_eq!(outcome.finished_at, Some(expected_finished_at));
    assert_eq!(outcome.started_at, Some(claim_at));
    assert!(outcome.started_at < outcome.finished_at);
}

fn journal_entry(step_id: &str, status: StepStatus) -> RunJournalEntry {
    let started_at = chrono::DateTime::parse_from_rfc3339("2026-08-16T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    RunJournalEntry {
        run_id: "runtime-session".to_string(),
        step_id: step_id.to_string(),
        kind: StepKind::Other,
        status,
        started_at,
        finished_at: (status == StepStatus::Skipped)
            .then_some(started_at + chrono::Duration::minutes(1)),
        detail: None,
    }
}

#[test]
fn journal_observation_skipped_status_remains_completed() {
    let entry = journal_entry("skipped-step", StepStatus::Skipped);
    let node_id = format!("runtime:journal:{}", entry.step_id);
    let plan = TaskGraph::default();
    let derived = derive_runtime_graph(Some(&plan), &[], &[entry], &[], &[]);

    let node = derived
        .runtime_graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .unwrap();
    assert_eq!(node.status, NodeStatus::Completed);
    let outcome = derived
        .outcomes
        .iter()
        .find(|outcome| outcome.subject_id == node_id)
        .unwrap();
    assert_eq!(outcome.status, RuntimeOutcomeStatus::Skipped);
}

#[test]
fn journal_observation_interrupted_status_remains_blocked() {
    let entry = journal_entry("interrupted-step", StepStatus::Interrupted);
    let node_id = format!("runtime:journal:{}", entry.step_id);
    let plan = TaskGraph::default();
    let derived = derive_runtime_graph(Some(&plan), &[], &[entry], &[], &[]);

    let node = derived
        .runtime_graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .unwrap();
    assert_eq!(node.status, NodeStatus::Blocked);
    let outcome = derived
        .outcomes
        .iter()
        .find(|outcome| outcome.subject_id == node_id)
        .unwrap();
    assert_eq!(outcome.status, RuntimeOutcomeStatus::Interrupted);
}

#[test]
fn lane_completion_records_event_backed_outcomes_and_reaches_retro() {
    let plan = TaskGraph::new(
        vec![
            task("queue-backed", &["root"]),
            task("in-lane", &["follow-up"]),
        ],
        vec![WorkEdge::new(
            "queue-backed",
            "in-lane",
            EdgeKind::Informs,
            EdgeProvenance::Knowledge,
        )],
    );
    let events = vec![
        event(
            "claim-root",
            EventType::WorkerClaimed,
            Some("worker-a"),
            json!({"worker_id":"worker-a","task_id":"queue-backed"}),
        ),
        event(
            "lane-complete",
            EventType::AgentCompleted,
            Some("worker-a"),
            json!({}),
        ),
    ];

    let derived = derive_runtime_graph(Some(&plan), &events, &[], &[], &[]);
    for task_id in ["queue-backed", "in-lane"] {
        let outcome = derived
            .outcomes
            .iter()
            .find(|outcome| outcome.task_id.as_deref() == Some(task_id))
            .unwrap_or_else(|| panic!("missing terminal outcome for {task_id}"));
        assert_eq!(outcome.subject_id, task_id);
        assert_eq!(outcome.status, RuntimeOutcomeStatus::Completed);
        if task_id == "in-lane" {
            assert_eq!(
                outcome.started_at, None,
                "lane completion does not fabricate an unobserved start time"
            );
        }
        assert!(
            outcome
                .source_refs
                .iter()
                .any(|source| source == "event:lane-complete"),
            "{task_id} outcome is not event-backed by the real lane completion"
        );
    }
    assert_eq!(
        derived
            .runtime_graph
            .nodes
            .iter()
            .find(|node| node.id == "in-lane")
            .unwrap()
            .status,
        NodeStatus::Completed,
        "a resolved completion must not remain at the queue projection's pending default"
    );

    let report = evaluate_runtime_retro(&plan, derived, events.len());
    let in_lane_metric = report.runs[0]
        .nodes
        .value()
        .unwrap()
        .iter()
        .find(|metric| metric.node_id == "in-lane")
        .expect("the retro reports the non-queue-backed plan node");
    assert_eq!(in_lane_metric.additional_attempts, Some(0));
    let gotcha = report.runs[0].gotcha_edge_hit_rate.value().unwrap();
    assert_eq!(gotcha.eligible_knowledge_edges, 1);
    assert_eq!(
        gotcha.targets_attempted, 1,
        "retro event_backed/outcome_matches_node predicates must reach in-lane"
    );
}

#[test]
fn anchored_lane_fanout_records_one_observed_and_eight_inferred_through_archive() {
    const SESSION_ID: &str = "archive-lane-provenance";
    let plan = TaskGraph::new(
        (1..=9)
            .map(|index| {
                WorkNode::new(
                    format!("T{index}"),
                    NodeKind::Task,
                    format!("Task T{index}"),
                    NodeContract::default(),
                    BindingRef::Role("P1".to_string()),
                    NodeStatus::Pending,
                )
            })
            .collect(),
        Vec::new(),
    );
    let mut completion = event(
        "single-lane-completion",
        EventType::AgentCompleted,
        Some("worker-1"),
        json!({"task_id":"T1"}),
    );
    completion.session_id = SESSION_ID.to_string();

    let derived = derive_runtime_graph(
        Some(&plan),
        std::slice::from_ref(&completion),
        &[],
        &[],
        &[],
    );
    assert_eq!(
        derived
            .outcomes
            .iter()
            .filter(|outcome| {
                outcome.completion_evidence == Some(CompletionEvidenceClass::Observed)
                    && outcome.task_id.is_some()
            })
            .count(),
        1
    );
    assert_eq!(
        derived
            .outcomes
            .iter()
            .filter(|outcome| {
                outcome.completion_evidence == Some(CompletionEvidenceClass::Inferred)
                    && outcome.task_id.is_some()
            })
            .count(),
        8
    );

    let temp = TempDir::new().unwrap();
    let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
    let session_dir = storage.create_session_dir(SESSION_ID).unwrap();
    StateManager::new(session_dir)
        .write_work_graph(&plan)
        .unwrap();
    let event_bus = EventBus::new(temp.path().to_path_buf());
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(event_bus.publish(completion))
        .unwrap();
    drop(event_bus);

    let completion = archive_completed_session(temp.path(), None, SESSION_ID).unwrap();
    let reread = read_archive(&completion.path).unwrap();
    assert_eq!(
        reread
            .outcomes
            .iter()
            .filter(|outcome| {
                outcome.completion_evidence == Some(CompletionEvidenceClass::Observed)
                    && outcome.task_id.is_some()
            })
            .count(),
        1
    );
    assert_eq!(
        reread
            .outcomes
            .iter()
            .filter(|outcome| {
                outcome.completion_evidence == Some(CompletionEvidenceClass::Inferred)
                    && outcome.task_id.is_some()
            })
            .count(),
        8
    );
}

#[test]
fn recorded_principal_resolves_completion_without_agent_id_suffix_guessing() {
    const SESSION_ID: &str = "recorded-principal-runtime";
    const AGENT_ID: &str = "recorded-principal-runtime-worker-3";
    let plan = TaskGraph::new(
        vec![WorkNode::new(
            "T1",
            NodeKind::Task,
            "Principal task",
            NodeContract::default(),
            BindingRef::Role("P1".to_string()),
            NodeStatus::Pending,
        )],
        Vec::new(),
    );
    let mut completion = event(
        "principal-completion",
        EventType::AgentCompleted,
        Some(AGENT_ID),
        json!({}),
    );
    completion.session_id = SESSION_ID.to_string();
    let principals = std::collections::BTreeMap::from([(AGENT_ID.to_string(), "P1".to_string())]);

    let derived = derive_runtime_graph_with_principals(
        Some(&plan),
        std::slice::from_ref(&completion),
        &[],
        &[],
        &[],
        &principals,
    );
    let task_outcome = derived
        .outcomes
        .iter()
        .find(|outcome| outcome.task_id.as_deref() == Some("T1"))
        .expect("recorded principal resolves the task");
    assert_eq!(task_outcome.status, RuntimeOutcomeStatus::Completed);
    assert_eq!(
        task_outcome.completion_evidence,
        Some(CompletionEvidenceClass::Inferred)
    );
    assert!(!derived.runtime_graph.omissions.iter().any(|omission| {
        matches!(
            omission.reason,
            WorkGraphOmissionReason::CompletionUnresolved
                | WorkGraphOmissionReason::ResolutionIncomplete
        )
    }));

    let temp = TempDir::new().unwrap();
    let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
    let session_dir = storage.create_session_dir(SESSION_ID).unwrap();
    let state = StateManager::new(session_dir);
    state.write_work_graph(&plan).unwrap();
    state
        .update_hierarchy(&[HierarchyNode {
            id: AGENT_ID.to_string(),
            role: "Worker-3".to_string(),
            principal: Some("P1".to_string()),
            parent_id: Some(format!("{SESSION_ID}-queen")),
            children: Vec::new(),
        }])
        .unwrap();
    let event_bus = EventBus::new(temp.path().to_path_buf());
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(event_bus.publish(completion))
        .unwrap();
    drop(event_bus);

    let archived = archive_completed_session(temp.path(), None, SESSION_ID).unwrap();
    let reread = read_archive(&archived.path).unwrap();
    assert_eq!(
        reread
            .outcomes
            .iter()
            .find(|outcome| outcome.task_id.as_deref() == Some("T1"))
            .expect("hierarchy binding survives into archive attribution")
            .status,
        RuntimeOutcomeStatus::Completed
    );

    let legacy = TaskGraph::new(
        vec![WorkNode::new(
            "legacy",
            NodeKind::Task,
            "Legacy worker binding",
            NodeContract::default(),
            BindingRef::Role("worker-8".to_string()),
            NodeStatus::Pending,
        )],
        Vec::new(),
    );
    let legacy_agent = "legacy-session-worker-8";
    let legacy_derived = derive_runtime_graph_with_principals(
        Some(&legacy),
        &[event(
            "legacy-completion",
            EventType::AgentCompleted,
            Some(legacy_agent),
            json!({}),
        )],
        &[],
        &[],
        &[],
        &std::collections::BTreeMap::from([(legacy_agent.to_string(), "worker-8".to_string())]),
    );
    assert_eq!(
        legacy_derived.runtime_graph.nodes[0].status,
        NodeStatus::Completed
    );
}

#[test]
fn missing_principal_binding_is_expected_typed_absence_without_guessing() {
    let plan = TaskGraph::new(vec![task("unresolved-task", &["code"])], Vec::new());
    for agent_id in ["session-queen", "session-evaluator", "session-judge-1"] {
        let derived = derive_runtime_graph(
            Some(&plan),
            &[event(
                &format!("unresolved-{agent_id}"),
                EventType::AgentCompleted,
                Some(agent_id),
                json!({}),
            )],
            &[],
            &[],
            &[],
        );
        let omission = derived
            .runtime_graph
            .omissions
            .iter()
            .find(|omission| omission.reason == WorkGraphOmissionReason::ResolutionIncomplete)
            .expect("an unbound supervisory agent is expected typed absence");
        assert_eq!(omission.count, 1, "{agent_id}");
        assert_eq!(omission.examples, vec![format!("binding:{agent_id}")]);
        assert!(!derived
            .runtime_graph
            .omissions
            .iter()
            .any(|omission| { omission.reason == WorkGraphOmissionReason::CompletionUnresolved }));
        assert!(!derived
            .outcomes
            .iter()
            .any(|outcome| { outcome.task_id.as_deref() == Some("unresolved-task") }));
    }
}

#[test]
fn claims_resolve_by_task_id_and_null_task_ids_report_omissions() {
    let plan = TaskGraph::new(vec![task("task-a", &["code"])], Vec::new());
    let events = vec![
        event(
            "claim-resolved",
            EventType::WorkerClaimed,
            Some("worker-a"),
            json!({"worker_id":"worker-a","task_id":"task-a"}),
        ),
        event(
            "spawn-resolved",
            EventType::AgentLaunched,
            Some("worker-a"),
            json!({"cli":"codex"}),
        ),
        event(
            "retry-resolved",
            EventType::WorkerReclaimed,
            Some("worker-a"),
            json!({"worker_id":"worker-a","task_id":"task-a"}),
        ),
        event(
            "artifact-resolved",
            EventType::ArtifactUpdated,
            Some("worker-a"),
            json!({"path":"artifacts/task-a.json"}),
        ),
        event(
            "complete-resolved",
            EventType::AgentCompleted,
            Some("worker-a"),
            json!({}),
        ),
        event(
            "claim-unresolved",
            EventType::WorkerClaimed,
            Some("worker-b"),
            json!({"worker_id":"worker-b","task_id":null}),
        ),
    ];

    let derived = derive_runtime_graph(Some(&plan), &events, &[], &[], &[]);
    let resolved_edge = derived.runtime_graph.edges.iter().find(|edge| {
        edge.source == "task-a"
            && edge.target == "runtime:event:claim-resolved"
            && edge.kind == EdgeKind::Consumes
    });
    assert!(resolved_edge.is_some());
    assert_eq!(resolved_edge.unwrap().provenance, EdgeProvenance::Runtime);

    let omission = derived
        .runtime_graph
        .omissions
        .iter()
        .find(|omission| omission.reason == WorkGraphOmissionReason::ResolutionIncomplete)
        .unwrap();
    assert_eq!(omission.count, 1);
    assert!(omission.examples[0].contains("null-task-id"));
    assert!(derived
        .runtime_graph
        .nodes
        .iter()
        .any(|node| node.id == "runtime:event:claim-unresolved"));
    assert!(derived.runtime_graph.edges.iter().any(|edge| {
        edge.source == "runtime:event:claim-resolved"
            && edge.target == "runtime:event:spawn-resolved"
            && edge.provenance == EdgeProvenance::Runtime
    }));
    assert!(derived.runtime_graph.edges.iter().any(|edge| {
        edge.source == "task-a"
            && edge.target == "runtime:event:retry-resolved"
            && edge.kind == EdgeKind::Informs
            && edge.provenance == EdgeProvenance::Runtime
    }));
    let task_outcome = derived
        .outcomes
        .iter()
        .find(|outcome| outcome.task_id.as_deref() == Some("task-a"))
        .unwrap();
    assert_eq!(task_outcome.status, RuntimeOutcomeStatus::Completed);
    assert!(task_outcome.effects.iter().any(|effect| {
        effect.kind == "artifact" && effect.reference.as_deref() == Some("artifacts/task-a.json")
    }));
}

#[test]
fn review_and_remediation_mutations_are_append_only_and_reconstructable() {
    let session_id = "mutation-review-session";
    let mut graph = TaskGraph::new(vec![task("implementation", &["code"])], Vec::new());
    let initial = graph.clone();
    let template = ReviewTemplate::code_tasks("required-review");

    let (mut expansions, first_delta) = instantiate_review_templates_and_record(
        session_id,
        &mut graph,
        std::slice::from_ref(&template),
    )
    .unwrap();
    let first_delta = first_delta.expect("review expansion changes the graph");

    let verdict_id = expansions[0].rounds[0].verdict_id.clone();
    let verdict_delta = record_review_verdict_and_record(
        session_id,
        &mut graph,
        &verdict_id,
        ReviewVerdict::Failed,
    )
    .unwrap()
    .expect("failed review verdict changes the graph");

    let expansion = expansions.first_mut().unwrap();
    let (_, second_delta) =
        route_failed_verdict_and_record(session_id, &mut graph, &template, expansion).unwrap();
    let second_delta = second_delta.expect("failed verdict adds remediation");

    assert_eq!(first_delta.sequence, 1);
    assert_eq!(verdict_delta.sequence, 2);
    assert_eq!(second_delta.sequence, 3);
    assert_eq!(
        first_delta.mutation_type,
        GraphMutationType::ReviewRoundAdded
    );
    assert_eq!(
        verdict_delta.mutation_type,
        GraphMutationType::ReviewVerdictRecorded
    );
    assert_eq!(
        second_delta.mutation_type,
        GraphMutationType::RemediationDetour
    );
    assert_eq!(first_delta.before, initial);
    assert_eq!(first_delta.after, verdict_delta.before);
    assert_eq!(verdict_delta.after, second_delta.before);
    assert!(first_delta
        .after
        .edges
        .iter()
        .filter(|edge| !initial.edges.contains(edge))
        .all(|edge| edge.provenance == EdgeProvenance::Runtime));
    assert!(second_delta
        .after
        .edges
        .iter()
        .filter(|edge| !second_delta.before.edges.contains(edge))
        .all(|edge| edge.provenance == EdgeProvenance::Runtime));

    let history = reconstruct_structural_history(
        &graph,
        &[
            first_delta.clone(),
            verdict_delta.clone(),
            second_delta.clone(),
        ],
    )
    .unwrap();
    let mut broken_sequence = second_delta.clone();
    broken_sequence.sequence = 4;
    assert!(reconstruct_structural_history(
        &graph,
        &[first_delta.clone(), verdict_delta.clone(), broken_sequence,],
    )
    .is_err());
    assert_eq!(
        history,
        vec![
            initial.clone(),
            first_delta.after.clone(),
            verdict_delta.after.clone(),
            graph.clone(),
        ]
    );
    let derived = derive_runtime_graph(
        Some(&initial),
        &[],
        &[],
        &[],
        &[first_delta, verdict_delta, second_delta.clone()],
    );
    assert!(derived.outcomes.iter().any(|outcome| {
        outcome.subject_id == verdict_id && outcome.status == RuntimeOutcomeStatus::Failed
    }));

    let before_failed_mutation = history.last().unwrap().clone();
    let failed = mutate_and_record(
        session_id,
        &mut graph,
        GraphMutationType::Other,
        vec!["deliberate-error".to_string()],
        |graph| {
            graph.nodes.push(task("must-roll-back", &[]));
            Err::<(), _>("mutation failed")
        },
    );
    assert!(failed.is_err());
    assert_eq!(graph, before_failed_mutation);
    assert_eq!(mutation_log(session_id).len(), 3);

    let disconnected_before = initial.clone();
    let mut disconnected_after = disconnected_before.clone();
    disconnected_after.nodes.push(task("disconnected", &[]));
    assert!(record_graph_change(
        session_id,
        GraphMutationType::Split,
        &disconnected_before,
        &disconnected_after,
        Vec::new(),
    )
    .is_err());
    assert_eq!(mutation_log(session_id).len(), 3);
}

#[test]
fn structural_history_rejects_shape_divergence_and_duplicates_but_accepts_status_only_changes() {
    let structural = TaskGraph::new(
        vec![task("task-a", &["code"]), task("task-b", &["review"])],
        vec![WorkEdge::new(
            "task-a",
            "task-b",
            EdgeKind::Informs,
            EdgeProvenance::Knowledge,
        )],
    );
    let delta = GraphMutationDelta {
        sequence: 1,
        observed_at: Utc::now(),
        mutation_type: GraphMutationType::Other,
        before: TaskGraph::default(),
        after: structural.clone(),
        source_refs: vec!["test:structural-contract".to_string()],
    };

    let mut divergent = structural.clone();
    divergent.nodes[1].title = "Structurally different title".to_string();
    assert_eq!(divergent.nodes.len(), delta.after.nodes.len());
    assert_eq!(divergent.edges, delta.after.edges);
    assert_eq!(
        reconstruct_structural_history(&divergent, std::slice::from_ref(&delta)).unwrap_err(),
        "final runtime graph does not match mutation delta 1"
    );

    let mut duplicate_ids = structural.clone();
    duplicate_ids.nodes[1].id = duplicate_ids.nodes[0].id.clone();
    let duplicate_delta = GraphMutationDelta {
        after: duplicate_ids.clone(),
        ..delta.clone()
    };
    assert_eq!(duplicate_ids.nodes.len(), duplicate_delta.after.nodes.len());
    assert_eq!(
        reconstruct_structural_history(&duplicate_ids, std::slice::from_ref(&duplicate_delta),)
            .unwrap_err(),
        "final runtime graph does not match mutation delta 1"
    );

    let mut status_only = structural;
    status_only.nodes[0].status = NodeStatus::Completed;
    assert_eq!(
        reconstruct_structural_history(&status_only, std::slice::from_ref(&delta)).unwrap(),
        vec![delta.before.clone(), delta.after.clone()]
    );
}

#[test]
fn archive_round_trip_preserves_corpus_and_never_mutates_runtime_sources() {
    let temp = TempDir::new().unwrap();
    let session_id = "archive-runtime-session";
    let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
    let session_dir = storage.create_session_dir(session_id).unwrap();

    let plan = TaskGraph::new(vec![task("task-a", &["code"])], Vec::new());
    StateManager::new(session_dir.clone())
        .write_work_graph(&plan)
        .unwrap();

    let mut runtime_structure = plan.clone();
    let template = ReviewTemplate::code_tasks("archive-review");
    let (_, archived_delta) = mutate_and_record(
        session_id,
        &mut runtime_structure,
        GraphMutationType::CompositeExpanded,
        vec!["review-template:archive-review".to_string()],
        |graph| instantiate_review_templates(graph, &[template]),
    )
    .unwrap();
    let archived_delta = archived_delta.expect("review expansion is archived");

    let mut events = vec![
        event(
            "claim",
            EventType::WorkerClaimed,
            Some("worker-a"),
            json!({"worker_id":"worker-a","task_id":"task-a"}),
        ),
        event(
            "spawn",
            EventType::AgentLaunched,
            Some("worker-a"),
            json!({"cli":"codex"}),
        ),
        event(
            "artifact",
            EventType::ArtifactUpdated,
            Some("worker-a"),
            json!({"path":"artifacts/primary.json"}),
        ),
        event(
            "complete",
            EventType::AgentCompleted,
            Some("worker-a"),
            json!({}),
        ),
    ];
    for event in &mut events {
        event.session_id = session_id.to_string();
    }
    let event_bus = EventBus::new(temp.path().to_path_buf());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        for event in events.clone() {
            event_bus.publish(event).await.unwrap();
        }
    });
    drop(event_bus);
    drop(runtime);

    let db = Arc::new(ApplicationStateDb::open(temp.path()).unwrap());
    let journal = RunJournalStore::new(db);
    journal.ensure_schema().unwrap();
    let spawn_step = journal
        .record_step_started(
            session_id,
            StepKind::WorkerSpawn,
            1,
            Some("hive/archive/worker-1"),
        )
        .unwrap();
    journal
        .record_step_finished(session_id, &spawn_step, StepStatus::Completed)
        .unwrap();
    let commit_step = journal
        .record_step_started(session_id, StepKind::GitCommit, 1, Some("worker-a"))
        .unwrap();
    journal
        .record_ledger(
            session_id,
            &commit_step,
            "git_commit",
            Some("abc123"),
            Confidence::Uncertain,
        )
        .unwrap();
    journal
        .confirm_ledger(session_id, &commit_step, Some("abc123"), Confidence::High)
        .unwrap();
    journal
        .record_step_finished(session_id, &commit_step, StepStatus::Completed)
        .unwrap();

    let event_path = temp.path().join(session_id).join("events.jsonl");
    let events_before = fs::read(&event_path).unwrap();
    let journal_before = journal.read_journal(session_id).unwrap();
    let ledger_before = journal.read_ledger(session_id).unwrap();
    let expected_deltas = vec![archived_delta];
    let expected_runtime = derive_runtime_graph(
        Some(&plan),
        &events,
        &journal_before,
        &ledger_before,
        &expected_deltas,
    );

    schedule_completed_session_archive(
        temp.path().to_path_buf(),
        Some(journal.clone()),
        session_id.to_string(),
    );
    let paths = (0..200)
        .find_map(|_| {
            let paths = list_archives(&session_dir).unwrap();
            if paths.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                None
            } else {
                Some(paths)
            }
        })
        .expect("terminal archive thread creates the archive");
    assert_eq!(paths.len(), 1);
    let archived = read_archive(&paths[0]).unwrap();
    let retro_report_path = session_dir
        .join("archive")
        .join("work-graph-retros")
        .join(format!("{}.json", archived.archive_id));
    for _ in 0..250 {
        if retro_report_path.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        retro_report_path.exists(),
        "the detached terminal pipeline must finish retro persistence before the test drops its storage"
    );
    assert_eq!(archived.schema_version, WORK_GRAPH_ARCHIVE_SCHEMA_VERSION);
    assert_eq!(archived.plan_graph, Some(plan.clone()));
    assert_eq!(archived.runtime_graph, expected_runtime.runtime_graph);
    assert_eq!(
        archived.deltas.len(),
        expected_deltas.len(),
        "archive dropped a structural delta"
    );
    assert_eq!(archived.deltas, expected_deltas);
    assert_eq!(archived.outcomes, expected_runtime.outcomes);
    assert_eq!(
        archived
            .divergence
            .recorded_runtime_mutations
            .get(&GraphMutationType::CompositeExpanded),
        Some(&1)
    );
    assert!(archived.divergence.count(DivergenceKind::NodeAdded) > 0);
    assert!(archived.outcomes.iter().any(|outcome| {
        outcome.task_id.as_deref() == Some("task-a")
            && outcome.status == RuntimeOutcomeStatus::Completed
    }));
    assert!(archived.outcomes.iter().any(|outcome| {
        outcome.effects.iter().any(|effect| {
            effect.kind == "git_commit"
                && effect.reference.as_deref() == Some("abc123")
                && effect.confirmed
        })
    }));

    let reread = read_archive(&paths[0]).unwrap();
    assert_eq!(reread, archived);
    assert_eq!(
        reread.reconstruct_structural_history().unwrap(),
        vec![plan, runtime_structure]
    );
    let repeated = archive_completed_session(temp.path(), Some(&journal), session_id).unwrap();
    assert!(!repeated.created);
    assert_eq!(repeated.path, paths[0]);
    assert_eq!(repeated.archive, archived);

    // G9: the terminal pipeline writes only its archive and stated retro report.
    // Derivation leaves both existing hot-path sources byte/value-identical.
    assert_eq!(fs::read(event_path).unwrap(), events_before);
    assert_eq!(journal.read_journal(session_id).unwrap(), journal_before);
    assert_eq!(journal.read_ledger(session_id).unwrap(), ledger_before);
}

#[test]
fn legacy_session_without_graph_or_sources_archives_cleanly_with_omissions() {
    let temp = TempDir::new().unwrap();
    assert!(archive_completed_session(temp.path(), None, "../escape").is_err());
    let session_id = "legacy-runtime-session";
    let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
    let session_dir = storage.create_session_dir(session_id).unwrap();
    fs::remove_file(session_dir.join("state").join("work-graph.json")).unwrap();

    let completion = archive_completed_session(temp.path(), None, session_id).unwrap();
    assert!(completion.created);
    assert_eq!(completion.archive.plan_graph, None);
    assert!(completion.archive.runtime_graph.nodes.is_empty());
    assert!(completion.archive.runtime_graph.edges.is_empty());
    assert!(completion.archive.runtime_graph.has_omissions());
    assert!(completion
        .archive
        .sources
        .iter()
        .any(|source| !source.available && !source.omissions.is_empty()));
    let plan_source = completion
        .archive
        .sources
        .iter()
        .find(|source| source.kind == ArchiveSourceKind::PlanGraph)
        .unwrap();
    assert_eq!(plan_source.omissions.len(), 1);
    assert_eq!(plan_source.omissions[0].count, 1);
    let plan_example_count = completion
        .archive
        .runtime_graph
        .omissions
        .iter()
        .flat_map(|omission| &omission.examples)
        .filter(|example| example.as_str() == "state/work-graph.json")
        .count();
    assert_eq!(plan_example_count, 1);
    let mutation_source = completion
        .archive
        .sources
        .iter()
        .find(|source| source.kind == ArchiveSourceKind::MutationLog)
        .unwrap();
    assert!(!mutation_source.available);
    assert_eq!(
        mutation_source.omissions[0].reason,
        WorkGraphOmissionReason::ResolutionIncomplete
    );
}

#[test]
fn declared_completion_facts_resolve_null_task_events_and_archive_task_ids() {
    let temp = TempDir::new().unwrap();
    let session_id = "declared-completion-093d-shape";
    let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
    let session_dir = storage.create_session_dir(session_id).unwrap();
    let plan = TaskGraph::new(
        (1..=10)
            .map(|index| task(&format!("T{index}"), &[]))
            .collect(),
        Vec::new(),
    );
    StateManager::new(session_dir.clone())
        .write_work_graph(&plan)
        .unwrap();

    let executed_as_json = json!({
        "provider": "codex",
        "tier": "high",
        "model": "gpt-5.6-sol",
        "flags": ["-c", "model_reasoning_effort=\"high\""],
        "channel": "native",
        "source": "node"
    });
    let executed_as: ExecutedAs = serde_json::from_value(executed_as_json.clone()).unwrap();
    let declared = vec![
        NodeCompletionFact::new(
            "T2",
            format!("{session_id}-worker-2"),
            NodeCompletionProvenance::Heartbeat,
        )
        .with_executed_as(executed_as),
        NodeCompletionFact::new(
            "T3",
            format!("{session_id}-worker-3"),
            NodeCompletionProvenance::Heartbeat,
        ),
    ];
    append_node_completion_facts(&session_dir, &declared).unwrap();

    let events = (1..=5)
        .map(|index| Event {
            id: format!("completed-{index}"),
            session_id: session_id.to_string(),
            cell_id: None,
            agent_id: Some(format!("{session_id}-worker-{index}")),
            event_type: EventType::AgentCompleted,
            timestamp: Utc::now(),
            payload: json!({}),
            severity: Severity::Info,
        })
        .collect::<Vec<_>>();
    let event_dir = temp.path().join(session_id);
    fs::create_dir_all(&event_dir).unwrap();
    fs::write(
        event_dir.join("events.jsonl"),
        events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();

    let direct = derive_runtime_graph_with_completion_facts(
        Some(&plan),
        &events,
        &[],
        &[],
        &[],
        &declared,
        &Default::default(),
    );
    for task_id in ["T2", "T3"] {
        assert_eq!(
            direct
                .runtime_graph
                .nodes
                .iter()
                .find(|node| node.id == task_id)
                .unwrap()
                .status,
            NodeStatus::Completed
        );
        let outcome = direct
            .outcomes
            .iter()
            .find(|outcome| outcome.subject_id == task_id)
            .unwrap();
        assert_eq!(outcome.task_id.as_deref(), Some(task_id));
        assert_eq!(
            outcome.completion_evidence,
            Some(CompletionEvidenceClass::Observed)
        );
        if task_id == "T2" {
            assert_eq!(
                serde_json::to_value(outcome.executed_as.as_ref().unwrap()).unwrap(),
                executed_as_json
            );
        } else {
            assert!(outcome.executed_as.is_none());
        }
    }
    assert!(direct.runtime_graph.omissions.iter().all(|omission| {
        omission.reason != WorkGraphOmissionReason::CompletionUnresolved
            || omission.examples.iter().all(|example| {
                !example.contains(&format!("{session_id}-worker-2"))
                    && !example.contains(&format!("{session_id}-worker-3"))
            })
    }));

    let archived = archive_completed_session(temp.path(), None, session_id)
        .unwrap()
        .archive;
    for task_id in ["T2", "T3"] {
        assert_eq!(
            archived
                .runtime_graph
                .nodes
                .iter()
                .find(|node| node.id == task_id)
                .unwrap()
                .status,
            NodeStatus::Completed
        );
        let outcome = archived
            .outcomes
            .iter()
            .find(|outcome| {
                outcome.subject_id == task_id && outcome.task_id.as_deref() == Some(task_id)
            })
            .unwrap();
        if task_id == "T2" {
            assert_eq!(
                serde_json::to_value(outcome.executed_as.as_ref().unwrap()).unwrap(),
                executed_as_json
            );
        } else {
            assert!(outcome.executed_as.is_none());
        }
    }
}

#[test]
fn orphan_ledger_effect_is_preserved_and_reported_incomplete() {
    let effect = LedgerEntry {
        run_id: "orphan-ledger-session".to_string(),
        step_id: "missing-step".to_string(),
        effect_kind: "git_commit".to_string(),
        effect_ref: Some("deadbeef".to_string()),
        confirmed: true,
        confidence: Confidence::High,
        recorded_at: Utc::now(),
    };
    let derived = derive_runtime_graph(Some(&TaskGraph::default()), &[], &[], &[effect], &[]);
    assert!(derived.outcomes.iter().any(|outcome| {
        outcome.effects.iter().any(|effect| {
            effect.kind == "git_commit" && effect.reference.as_deref() == Some("deadbeef")
        })
    }));
    assert!(derived.runtime_graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ResolutionIncomplete
            && omission
                .examples
                .iter()
                .any(|example| example == "orphan-ledger-step:missing-step")
    }));
}

#[test]
fn unreadable_and_cross_session_event_lines_are_reported() {
    let temp = TempDir::new().unwrap();
    let session_id = "unreadable-events-session";
    let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
    let session_dir = storage.create_session_dir(session_id).unwrap();
    StateManager::new(session_dir)
        .write_work_graph(&TaskGraph::default())
        .unwrap();
    let event_dir = temp.path().join(session_id);
    fs::create_dir_all(&event_dir).unwrap();
    let foreign_event = event(
        "foreign-event",
        EventType::AgentCompleted,
        Some("foreign-worker"),
        json!({}),
    );
    fs::write(
        event_dir.join("events.jsonl"),
        format!(
            "{{not-json}}\n{}\n",
            serde_json::to_string(&foreign_event).unwrap()
        ),
    )
    .unwrap();

    let completion = archive_completed_session(temp.path(), None, session_id).unwrap();
    let event_source = completion
        .archive
        .sources
        .iter()
        .find(|source| source.kind == ArchiveSourceKind::EventLog)
        .unwrap();
    assert!(event_source.available);
    assert_eq!(event_source.record_count, 0);
    assert!(event_source
        .omissions
        .iter()
        .any(|omission| { omission.reason == WorkGraphOmissionReason::SourceUnreadable }));
    assert!(event_source
        .omissions
        .iter()
        .any(|omission| { omission.reason == WorkGraphOmissionReason::ResolutionIncomplete }));
}

#[test]
fn unreadable_hierarchy_is_reported_in_the_archived_runtime_graph() {
    let temp = TempDir::new().unwrap();
    let session_id = "unreadable-hierarchy-session";
    let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
    let session_dir = storage.create_session_dir(session_id).unwrap();
    StateManager::new(session_dir.clone())
        .write_work_graph(&TaskGraph::default())
        .unwrap();
    fs::write(
        session_dir.join("state").join("hierarchy.json"),
        "{not-json}",
    )
    .unwrap();

    let completion = archive_completed_session(temp.path(), None, session_id).unwrap();
    assert!(completion
        .archive
        .runtime_graph
        .omissions
        .iter()
        .any(|omission| {
            omission.reason == WorkGraphOmissionReason::SourceUnreadable
                && omission
                    .examples
                    .iter()
                    .any(|example| example == "state/hierarchy.json")
        }));
}

#[test]
fn divergence_is_neutral_queryable_data_and_detects_rewiring() {
    let plan = TaskGraph::new(
        vec![task("a", &[]), task("b", &[]), task("c", &[])],
        vec![WorkEdge::new(
            "a",
            "c",
            EdgeKind::DependsOn,
            EdgeProvenance::Planner,
        )],
    );
    let mut actual = plan.clone();
    actual.edges = vec![WorkEdge::new(
        "b",
        "c",
        EdgeKind::DependsOn,
        EdgeProvenance::Runtime,
    )];

    let summary = compute_divergence(Some(&plan), &actual, &[]);
    assert_eq!(summary.count(DivergenceKind::EdgeRewired), 1);
    assert_eq!(summary.count(DivergenceKind::EdgeAdded), 0);
    assert_eq!(summary.count(DivergenceKind::EdgeRemoved), 0);

    let without_baseline = compute_divergence(None, &actual, &[]);
    assert!(without_baseline.records.is_empty());
    assert!(without_baseline.counts_by_mutation_type.is_empty());
}
