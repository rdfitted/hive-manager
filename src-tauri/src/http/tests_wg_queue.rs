//! Readiness-queue tests for issue #212, owned by WS-4.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use parking_lot::RwLock;
use serde_json::json;
use tower::ServiceExt;

use crate::coordination::queue_manager::{
    ClaimOutcome, QueueManager, FIRST_HEARTBEAT_GRACE_MS, FIRST_HEARTBEAT_LATENCY_MS_BY_CLI,
    STUCK_CUTOFF_MS,
};
use crate::coordination::{InjectionManager, StateManager};
use crate::domain::event::{EventType, Severity};
use crate::domain::HiveExecutionPolicy;
use crate::events::EventBus;
use crate::http::handlers::heartbeats::PostHeartbeatRequest;
use crate::http::handlers::workers::AddWorkerRequest;
use crate::http::routes::create_router;
use crate::http::state::AppState;
use crate::orchestrator::work_graph::completion_ledger::{
    read_node_completion_facts, NodeCompletionProvenance,
};
use crate::orchestrator::work_graph::plan_parse::promote_initial_ready_nodes;
use crate::orchestrator::work_graph::review::checkpoint_aware_claimable_nodes;
use crate::orchestrator::work_graph::{
    BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus, TaskGraph, WorkEdge,
    WorkNode,
};
use crate::pty::{AgentConfig, AgentRole, AgentStatus, PtyManager};
use crate::session::{
    AgentInfo, AuthStrategy, Session, SessionController, SessionState, SessionType,
};
use crate::storage::queue::{
    QueueConflictAction, QueueConflictCoverage, QueueConflictRow, QueueResolutionUpdate, QueueRow,
    QueueStatus,
};
use crate::storage::{ApplicationStateDb, QueueRepo, SessionStorage};

const SESSION_ID: &str = "readiness-session";

fn queue_repo() -> QueueRepo {
    let db = Arc::new(ApplicationStateDb::open_in_memory().expect("in-memory queue database"));
    let repo = QueueRepo::new(db);
    repo.ensure_schema().expect("queue schema");
    repo
}

fn queued_row(id: &str, worker_id: &str, task_id: Option<&str>, created_at: i64) -> QueueRow {
    queue_row(id, worker_id, task_id, QueueStatus::Queued, created_at)
}

fn queue_row(
    id: &str,
    worker_id: &str,
    task_id: Option<&str>,
    status: QueueStatus,
    created_at: i64,
) -> QueueRow {
    QueueRow {
        id: id.to_string(),
        task_id: task_id.map(str::to_string),
        session_id: SESSION_ID.to_string(),
        worker_id: worker_id.to_string(),
        role_type: "backend".to_string(),
        cli: "codex".to_string(),
        status,
        payload: json!({"prompt": id}),
        attempts: 0,
        continuation_count: 0,
        no_progress_count: 0,
        last_status: None,
        heartbeat_at: None,
        assignment_id: 0,
        blocked_reason: None,
        created_at,
        updated_at: created_at,
    }
}

fn enqueue_diamond(repo: &QueueRepo) {
    repo.enqueue_with_dependencies(&queued_row("run-a", "worker-a", Some("A"), 1), &[])
        .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-b", "worker-b", Some("B"), 2),
        &["A".to_string()],
    )
    .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-c", "worker-c", Some("C"), 3),
        &["A".to_string()],
    )
    .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-d", "worker-d", Some("D"), 4),
        &["B".to_string(), "C".to_string()],
    )
    .unwrap();
}

fn race_claims(repo: &QueueRepo, ids: &[&str], now_ms: i64) -> Vec<Option<i64>> {
    let barrier = Arc::new(Barrier::new(ids.len() + 1));
    let handles = ids
        .iter()
        .map(|id| {
            let repo = repo.clone();
            let barrier = Arc::clone(&barrier);
            let id = (*id).to_string();
            std::thread::spawn(move || {
                barrier.wait();
                repo.try_claim(&id, now_ms - 90_000, now_ms).unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    handles
        .into_iter()
        .map(|handle| handle.join().expect("claim thread"))
        .collect()
}

fn race_bound_claims(
    repo: &QueueRepo,
    ids: &[&str],
    worker_id: &str,
    now_ms: i64,
) -> Vec<Option<i64>> {
    let barrier = Arc::new(Barrier::new(ids.len() + 1));
    let handles = ids
        .iter()
        .map(|id| {
            let repo = repo.clone();
            let barrier = Arc::clone(&barrier);
            let id = (*id).to_string();
            let worker_id = worker_id.to_string();
            std::thread::spawn(move || {
                barrier.wait();
                repo.try_claim_for_worker(&id, Some(&worker_id), now_ms - 90_000, now_ms)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    handles
        .into_iter()
        .map(|handle| handle.join().expect("bound claim thread"))
        .collect()
}

#[tokio::test]
async fn slow_spawn_reservation_prevents_stale_reclaim_and_second_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    let manager = QueueManager::new(Arc::clone(&repo), EventBus::new(temp.path().to_path_buf()));
    manager
        .enqueue_worker(
            "run-slow",
            SESSION_ID,
            "pending:run-slow",
            "backend",
            "codex",
            json!({}),
            Some("SLOW".to_string()),
        )
        .await
        .unwrap();

    let spawn_invocations = AtomicUsize::new(0);
    let first_claim = manager
        .claim_and_reserve_spawn("run-slow", SESSION_ID, "worker-1", None)
        .await
        .unwrap();
    if first_claim.is_some() {
        spawn_invocations.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(first_claim, ClaimOutcome::Claimed { epoch: 1 });
    let claimed_heartbeat = repo
        .get_row("run-slow")
        .unwrap()
        .unwrap()
        .heartbeat_at
        .unwrap();

    // Simulate a spawn that ran past an arbitrarily short stale cutoff. Maintenance must first
    // refresh the exact in-flight epoch, so its bulk stale reclaim sees no candidate. A retry
    // then remains denied instead of binding this task to a second worker at epoch 2.
    let reclaimed = manager
        .reclaim_stuck_at(claimed_heartbeat + 1, claimed_heartbeat + 2)
        .await
        .unwrap();
    let retry = manager
        .claim_and_reserve_spawn("run-slow", SESSION_ID, "worker-2", None)
        .await
        .unwrap();
    // This is the production spawn boundary: the handler invokes worktree/PTY creation for
    // every `Claimed` outcome. Counting it makes the mutation prove the concrete double spawn,
    // not merely an intermediate queue status.
    if retry.is_some() {
        spawn_invocations.fetch_add(1, Ordering::SeqCst);
    }
    let protected = repo.get_row("run-slow").unwrap().unwrap();
    assert_eq!(
        (
            spawn_invocations.load(Ordering::SeqCst),
            reclaimed,
            retry,
            protected.attempts,
            protected.worker_id.as_str(),
        ),
        (
            1,
            Vec::<String>::new(),
            ClaimOutcome::AlreadyClaimed,
            1,
            "worker-1",
        ),
        "a slow epoch-1 spawn must not be reclaimed and double-spawned as epoch 2"
    );
    assert_eq!(protected.heartbeat_at, Some(claimed_heartbeat + 2));
    assert!(manager
        .complete_spawn_handoff("run-slow", 1, "worker-1")
        .unwrap());
    assert!(manager.spawn_in_flight("run-slow").is_none());
}

#[tokio::test]
async fn stale_epoch_one_cannot_release_or_reacquire_after_epoch_two_exists() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    let manager = QueueManager::new(Arc::clone(&repo), EventBus::new(temp.path().to_path_buf()));
    manager
        .enqueue_worker(
            "run-epoch",
            SESSION_ID,
            "pending:run-epoch",
            "backend",
            "codex",
            json!({}),
            Some("EPOCH".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(
        manager
            .claim_and_reserve_spawn("run-epoch", SESSION_ID, "worker-1", None)
            .await
            .unwrap(),
        ClaimOutcome::Claimed { epoch: 1 }
    );
    assert_eq!(
        manager
            .release_after_failed_spawn(SESSION_ID, "worker-1", "run-epoch", 1)
            .await
            .unwrap(),
        crate::coordination::ReleaseAfterFailure::Released
    );
    assert_eq!(
        manager
            .claim_and_reserve_spawn("run-epoch", SESSION_ID, "worker-2", None)
            .await
            .unwrap(),
        ClaimOutcome::Claimed { epoch: 2 }
    );

    // Late cleanup/handoff from epoch 1 must neither mutate epoch 2 nor clear its reservation.
    assert_eq!(
        manager
            .release_after_failed_spawn(SESSION_ID, "worker-1", "run-epoch", 1)
            .await
            .unwrap(),
        crate::coordination::ReleaseAfterFailure::NotHeld
    );
    assert!(!manager
        .complete_spawn_handoff("run-epoch", 1, "worker-1")
        .unwrap());
    assert_eq!(
        manager.spawn_in_flight("run-epoch").unwrap().epoch,
        2,
        "stale epoch-1 cleanup must leave epoch 2 reserved"
    );
    assert!(!repo
        .refresh_claimed_spawn("run-epoch", 1, "worker-1", 1)
        .unwrap());

    let epoch_two_heartbeat = repo
        .get_row("run-epoch")
        .unwrap()
        .unwrap()
        .heartbeat_at
        .unwrap();
    let reclaimed = manager
        .reclaim_stuck_at(epoch_two_heartbeat + 1, epoch_two_heartbeat + 2)
        .await
        .unwrap();
    let retry = manager
        .claim_and_reserve_spawn("run-epoch", SESSION_ID, "worker-3", None)
        .await
        .unwrap();
    let row = repo.get_row("run-epoch").unwrap().unwrap();
    assert_eq!(
        (reclaimed, retry, row.attempts, row.worker_id.as_str()),
        (
            Vec::<String>::new(),
            ClaimOutcome::AlreadyClaimed,
            2,
            "worker-2"
        )
    );
    assert!(manager
        .complete_spawn_handoff("run-epoch", 2, "worker-2")
        .unwrap());

    // Manual recovery resets only the independent spawn-failure budget, never the fencing
    // epoch. Reuse worker-1 deliberately: even matching the old worker ID cannot make the stale
    // epoch-1 reservation valid once epoch 2 has existed.
    assert_eq!(
        manager.release_claim(SESSION_ID, "worker-2").await.unwrap(),
        crate::coordination::ReleaseOutcome::Released {
            previous: QueueStatus::Running,
        }
    );
    assert_eq!(
        manager
            .claim_and_reserve_spawn("run-epoch", SESSION_ID, "worker-1", None)
            .await
            .unwrap(),
        ClaimOutcome::Claimed { epoch: 3 }
    );
    assert_eq!(repo.get_row("run-epoch").unwrap().unwrap().attempts, 3);
    assert!(!repo
        .refresh_claimed_spawn("run-epoch", 1, "worker-1", 2)
        .unwrap());
    assert!(!manager
        .complete_spawn_handoff("run-epoch", 1, "worker-1")
        .unwrap());
    assert_eq!(
        manager
            .release_after_failed_spawn(SESSION_ID, "worker-1", "run-epoch", 1)
            .await
            .unwrap(),
        crate::coordination::ReleaseAfterFailure::NotHeld
    );
    assert_eq!(manager.spawn_in_flight("run-epoch").unwrap().epoch, 3);
}

#[tokio::test]
async fn spawn_failure_budget_resets_without_resetting_claim_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    let manager = QueueManager::new(Arc::clone(&repo), EventBus::new(temp.path().to_path_buf()));
    manager
        .enqueue_worker(
            "run-budget",
            SESSION_ID,
            "pending:run-budget",
            "backend",
            "codex",
            json!({}),
            Some("BUDGET".to_string()),
        )
        .await
        .unwrap();

    for epoch in 1..=3 {
        assert_eq!(
            manager
                .claim_and_reserve_spawn("run-budget", SESSION_ID, "worker-1", Some("BUDGET"),)
                .await
                .unwrap(),
            ClaimOutcome::Claimed { epoch }
        );
        let released = manager
            .release_after_failed_spawn(SESSION_ID, "worker-1", "run-budget", epoch)
            .await
            .unwrap();
        if epoch < 3 {
            assert_eq!(released, crate::coordination::ReleaseAfterFailure::Released);
        } else {
            assert_eq!(
                released,
                crate::coordination::ReleaseAfterFailure::Exhausted { attempts: 3 }
            );
        }
    }
    let exhausted = repo.get_row("run-budget").unwrap().unwrap();
    assert_eq!(
        (exhausted.status, exhausted.attempts),
        (QueueStatus::Failed, 3)
    );

    assert_eq!(
        manager.release_claim(SESSION_ID, "worker-1").await.unwrap(),
        crate::coordination::ReleaseOutcome::Released {
            previous: QueueStatus::Failed,
        }
    );
    assert_eq!(
        manager
            .claim_and_reserve_spawn("run-budget", SESSION_ID, "worker-1", Some("BUDGET"),)
            .await
            .unwrap(),
        ClaimOutcome::Claimed { epoch: 4 },
        "manual recovery resets only the failure budget; epoch 1 must never return"
    );
    assert_eq!(
        manager
            .release_after_failed_spawn(SESSION_ID, "worker-1", "run-budget", 4)
            .await
            .unwrap(),
        crate::coordination::ReleaseAfterFailure::Released,
        "the independent failure budget restarts at one after manual recovery"
    );
    assert_eq!(repo.get_row("run-budget").unwrap().unwrap().attempts, 4);
}

#[tokio::test]
async fn in_flight_spawn_denies_manual_and_same_process_reconcile_but_drops_on_restart() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    let manager = QueueManager::new(Arc::clone(&repo), EventBus::new(temp.path().to_path_buf()));
    manager
        .enqueue_worker(
            "run-restart",
            SESSION_ID,
            "pending:run-restart",
            "backend",
            "codex",
            json!({}),
            Some("RESTART".to_string()),
        )
        .await
        .unwrap();
    manager
        .claim_and_reserve_spawn("run-restart", SESSION_ID, "worker-1", None)
        .await
        .unwrap();

    assert_eq!(
        manager
            .spawn_in_flight_for_worker("worker-1")
            .unwrap()
            .queue_id,
        "run-restart"
    );
    assert_eq!(
        manager.release_claim(SESSION_ID, "worker-1").await.unwrap(),
        crate::coordination::ReleaseOutcome::SpawnInFlight { epoch: 1 }
    );
    assert!(manager.reconcile(SESSION_ID, &[]).await.unwrap().is_empty());
    assert_eq!(
        repo.get_row("run-restart").unwrap().unwrap().status,
        QueueStatus::Running
    );

    // A new manager models process restart: its in-memory marker set is empty, so the normal
    // durable startup reconciliation repairs the orphaned running row.
    drop(manager);
    let restarted = QueueManager::new(Arc::clone(&repo), EventBus::new(temp.path().to_path_buf()));
    assert_eq!(
        restarted.reconcile(SESSION_ID, &[]).await.unwrap(),
        vec!["run-restart".to_string()]
    );
    let repaired = repo.get_row("run-restart").unwrap().unwrap();
    assert_eq!(repaired.status, QueueStatus::Queued);
    assert_eq!(repaired.worker_id, "pending:run-restart");
}

#[test]
fn diamond_claims_are_dependency_gated_atomic_and_eventually_unblock() {
    let repo = queue_repo();
    enqueue_diamond(&repo);

    let premature = race_claims(&repo, &["run-b", "run-c"], 10);
    assert_eq!(
        premature,
        vec![None, None],
        "B and C must remain queued until A finalizes"
    );

    let a_epoch = repo
        .try_claim("run-a", -90_000, 20)
        .unwrap()
        .expect("root A is ready");
    assert_eq!(a_epoch, 1, "attempts remains the fencing epoch");
    let claimed_a = repo.get_row("run-a").unwrap().unwrap();
    assert_eq!(claimed_a.heartbeat_at, Some(20), "claim stamps heartbeat");
    repo.record_heartbeat(SESSION_ID, "worker-a", "completed", 30)
        .unwrap();

    let siblings = race_claims(&repo, &["run-b", "run-c"], 40);
    assert!(
        siblings.iter().all(Option::is_some),
        "B and C must both become independently claimable after A finalizes: {siblings:?}"
    );
    assert_eq!(repo.try_claim("run-d", -90_000, 50).unwrap(), None);

    repo.record_heartbeat(SESSION_ID, "worker-b", "completed", 60)
        .unwrap();
    assert_eq!(
        repo.try_claim("run-d", -90_000, 70).unwrap(),
        None,
        "D still waits for C"
    );
    repo.record_heartbeat(SESSION_ID, "worker-c", "completed", 80)
        .unwrap();

    let same_ready_row = race_claims(&repo, &["run-d", "run-d"], 90);
    assert_eq!(
        same_ready_row
            .iter()
            .filter(|epoch| epoch.is_some())
            .count(),
        1,
        "exactly one concurrent claimer may win the same ready row: {same_ready_row:?}"
    );
    let claimed_d = repo.get_row("run-d").unwrap().unwrap();
    assert_eq!(claimed_d.attempts, 1);
    assert_eq!(claimed_d.heartbeat_at, Some(90));
}

fn serialized_conflict_rows(session_id: &str) -> Vec<QueueConflictRow> {
    [("T1", "T2"), ("T2", "T1")]
        .into_iter()
        .map(|(task_id, conflicting_task_id)| QueueConflictRow {
            session_id: session_id.to_string(),
            task_id: task_id.to_string(),
            conflicting_task_id: conflicting_task_id.to_string(),
            action: QueueConflictAction::Serialize,
            reason: "T1 and T2 overlap src/shared.rs".to_string(),
        })
        .collect()
}

#[test]
fn materialized_serialize_conflict_is_atomic_and_partial_coverage_stays_visible() {
    let repo = queue_repo();
    let conflicts = serialized_conflict_rows(SESSION_ID);
    let coverage = QueueConflictCoverage {
        state: "partial".to_string(),
        unresolved_task_ids: vec!["T3".to_string()],
    };
    repo.enqueue_with_scheduling(
        &queued_row("run-t1", "worker-t1", Some("T1"), 1),
        &[],
        &QueueResolutionUpdate::Resolved,
        &conflicts,
        Some(&coverage),
        Some("T1"),
    )
    .unwrap();
    repo.enqueue_with_scheduling(
        &queued_row("run-t2", "worker-t2", Some("T2"), 2),
        &[],
        &QueueResolutionUpdate::Resolved,
        &conflicts,
        Some(&coverage),
        Some("T2"),
    )
    .unwrap();

    let raced = race_claims(&repo, &["run-t1", "run-t2"], 10);
    assert_eq!(
        raced.iter().filter(|claim| claim.is_some()).count(),
        1,
        "the correlated conflict predicate must serialize concurrent conflicting claims: {raced:?}"
    );
    let (winner_id, winner_worker, loser_id) = if raced[0].is_some() {
        ("run-t1", "worker-t1", "run-t2")
    } else {
        ("run-t2", "worker-t2", "run-t1")
    };
    assert_eq!(repo.try_claim(loser_id, -90_000, 20).unwrap(), None);
    let waits = repo.pending_conflicts(loser_id).unwrap();
    assert_eq!(waits.len(), 1);
    assert!(waits[0].reason.contains("src/shared.rs"));

    repo.record_heartbeat(SESSION_ID, winner_worker, "completed", 30)
        .unwrap();
    assert!(repo.try_claim(loser_id, -90_000, 40).unwrap().is_some());
    assert_eq!(
        repo.get_row(winner_id).unwrap().unwrap().status,
        QueueStatus::Finalized
    );
    assert_eq!(
        repo.snapshot(SESSION_ID).unwrap().conflict_coverage,
        Some(coverage)
    );
}

#[test]
fn worktree_isolate_conflicts_are_materialized_but_do_not_serialize() {
    let repo = queue_repo();
    let conflicts = serialized_conflict_rows(SESSION_ID)
        .into_iter()
        .map(|mut row| {
            row.action = QueueConflictAction::WorktreeIsolate;
            row
        })
        .collect::<Vec<_>>();
    for (id, worker, task, created_at) in [
        ("run-t1", "worker-t1", "T1", 1),
        ("run-t2", "worker-t2", "T2", 2),
    ] {
        repo.enqueue_with_scheduling(
            &queued_row(id, worker, Some(task), created_at),
            &[],
            &QueueResolutionUpdate::Resolved,
            &conflicts,
            None,
            Some(task),
        )
        .unwrap();
    }
    assert!(race_claims(&repo, &["run-t1", "run-t2"], 10)
        .iter()
        .all(Option::is_some));
}

#[test]
fn active_exact_task_reconciliation_removes_obsolete_serialize_pair() {
    let repo = queue_repo();
    let conflicts = serialized_conflict_rows(SESSION_ID);
    repo.enqueue_with_scheduling(
        &queued_row("run-t1", "worker-t1", Some("T1"), 1),
        &[],
        &QueueResolutionUpdate::Resolved,
        &conflicts,
        None,
        Some("T1"),
    )
    .unwrap();
    repo.enqueue_with_scheduling(
        &queued_row("run-t2", "worker-t2", Some("T2"), 2),
        &[],
        &QueueResolutionUpdate::Resolved,
        &conflicts,
        None,
        Some("T2"),
    )
    .unwrap();
    assert!(repo.try_claim("run-t1", -90_000, 10).unwrap().is_some());
    assert_eq!(repo.try_claim("run-t2", -90_000, 20).unwrap(), None);

    // The authoritative retry now reports complete no-overlap coverage for active T2. Replace
    // only T2's incident materialization, leaving unrelated task pairs untouched.
    repo.enqueue_with_scheduling(
        &queued_row("run-t2", "worker-t2", Some("T2"), 2),
        &[],
        &QueueResolutionUpdate::Resolved,
        &[],
        Some(&QueueConflictCoverage {
            state: "complete".to_string(),
            unresolved_task_ids: Vec::new(),
        }),
        Some("T2"),
    )
    .unwrap();
    assert!(repo.pending_conflicts("run-t2").unwrap().is_empty());
    assert!(repo.try_claim("run-t2", -90_000, 30).unwrap().is_some());
}

#[test]
fn partial_conflict_reconciliation_preserves_a_known_serialize_pair() {
    let repo = queue_repo();
    let conflicts = serialized_conflict_rows(SESSION_ID);
    repo.enqueue_with_scheduling(
        &queued_row("run-t1", "worker-t1", Some("T1"), 1),
        &[],
        &QueueResolutionUpdate::Resolved,
        &conflicts,
        None,
        None,
    )
    .unwrap();
    repo.enqueue_with_scheduling(
        &queued_row("run-t2", "worker-t2", Some("T2"), 2),
        &[],
        &QueueResolutionUpdate::Resolved,
        &conflicts,
        None,
        None,
    )
    .unwrap();
    assert!(repo.try_claim("run-t1", -90_000, 10).unwrap().is_some());

    // Even a mistaken caller request may not treat an absent partial-analysis decision as
    // proof that the previously known conflict disappeared.
    repo.enqueue_with_scheduling(
        &queued_row("run-t2", "worker-t2", Some("T2"), 2),
        &[],
        &QueueResolutionUpdate::Resolved,
        &[],
        Some(&QueueConflictCoverage {
            state: "partial".to_string(),
            unresolved_task_ids: vec!["T1".to_string()],
        }),
        Some("T2"),
    )
    .unwrap();
    assert_eq!(
        repo.pending_conflicts("run-t2").unwrap(),
        vec![crate::storage::queue::QueueConflictWait {
            task_id: "T1".to_string(),
            reason: "T1 and T2 overlap src/shared.rs".to_string(),
        }]
    );
    assert_eq!(repo.try_claim("run-t2", -90_000, 20).unwrap(), None);
}

#[test]
fn unresolved_binding_blocks_until_exact_queued_intent_is_reconciled() {
    let repo = queue_repo();
    let row = queued_row("run-unknown", "pending:run-unknown", Some("UNKNOWN"), 1);
    repo.enqueue_with_scheduling(
        &row,
        &[],
        &QueueResolutionUpdate::ResolutionIncomplete {
            task_id: "UNKNOWN".to_string(),
            reason: "resolution_incomplete: UNKNOWN is absent".to_string(),
        },
        &[],
        None,
        None,
    )
    .unwrap();
    assert_eq!(repo.try_claim("run-unknown", -90_000, 10).unwrap(), None);
    assert_eq!(
        repo.snapshot(SESSION_ID)
            .unwrap()
            .resolution_incomplete
            .len(),
        1
    );

    // A wrong binding under the same queue id cannot clear or rewrite the omission.
    let wrong = queued_row("run-unknown", "pending:run-unknown", Some("OTHER"), 2);
    repo.enqueue_with_scheduling(
        &wrong,
        &[],
        &QueueResolutionUpdate::Resolved,
        &[],
        None,
        None,
    )
    .unwrap();
    assert!(repo.resolution_issue("run-unknown").unwrap().is_some());

    // Reconciliation is allowed only for the exact still-queued intent.
    repo.enqueue_with_scheduling(&row, &[], &QueueResolutionUpdate::Resolved, &[], None, None)
        .unwrap();
    assert!(repo.resolution_issue("run-unknown").unwrap().is_none());
    assert!(repo
        .try_claim("run-unknown", -90_000, 20)
        .unwrap()
        .is_some());
}

#[test]
fn terminal_failure_blocks_only_transitive_descendants_with_reason() {
    let repo = queue_repo();
    enqueue_diamond(&repo);
    repo.enqueue(&queued_row("run-e", "worker-e", Some("E"), 5))
        .unwrap();

    let epoch = repo.try_claim("run-a", -90_000, 10).unwrap().unwrap();
    assert!(repo.fail_claimed("run-a", epoch, 20).unwrap());

    let snapshot = repo.snapshot(SESSION_ID).unwrap();
    assert_eq!(snapshot.failed, 1);
    assert_eq!(snapshot.blocked, 3);
    assert_eq!(snapshot.queued, 1);
    for id in ["run-b", "run-c", "run-d"] {
        let row = repo.get_row(id).unwrap().unwrap();
        assert_eq!(row.status, QueueStatus::Blocked);
        assert!(
            row.blocked_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("A") && reason.contains("failed")),
            "{id} omitted its failed ancestor: {:?}",
            row.blocked_reason
        );
        assert_eq!(repo.try_claim(id, -90_000, 30).unwrap(), None);
    }
    assert!(repo.try_claim("run-e", -90_000, 30).unwrap().is_some());
}

#[test]
fn cancellation_blocks_root_and_transitive_descendants_with_reason() {
    let repo = queue_repo();
    enqueue_diamond(&repo);
    repo.enqueue(&queued_row("run-e", "worker-e", Some("E"), 5))
        .unwrap();

    let blocked = repo
        .cancel_task_and_descendants(SESSION_ID, "A", "operator cancelled", 10)
        .unwrap();
    assert_eq!(blocked, vec!["run-a", "run-b", "run-c", "run-d"]);
    for id in &blocked {
        let row = repo.get_row(id).unwrap().unwrap();
        assert_eq!(row.status, QueueStatus::Blocked);
        assert!(row
            .blocked_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("A") && reason.contains("cancelled")));
        assert_eq!(repo.try_claim(id, -90_000, 20).unwrap(), None);
    }
    assert!(repo.try_claim("run-e", -90_000, 20).unwrap().is_some());
}

#[test]
fn edgeless_and_legacy_rows_keep_existing_claim_behavior_and_order() {
    let repo = queue_repo();
    repo.ensure_schema().expect("idempotent additive upgrade");
    repo.enqueue(&queued_row("run-b", "worker-b", Some("B"), 1))
        .unwrap();
    repo.enqueue(&queued_row("run-a", "worker-a", None, 1))
        .unwrap();

    let claims = race_claims(&repo, &["run-a", "run-b"], 10);
    assert!(claims.iter().all(Option::is_some));
    assert!(race_claims(&repo, &["run-a", "run-b"], 20)
        .iter()
        .all(Option::is_none));
    let snapshot = repo.snapshot(SESSION_ID).unwrap();
    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec!["run-a", "run-b"],
        "legacy snapshot ordering remains created_at then id"
    );
}

#[test]
fn task_requeue_releases_worker_binding_before_slot_reuse() {
    let repo = queue_repo();
    repo.enqueue(&queued_row("run-b", "pending:run-b", Some("B"), 1))
        .unwrap();
    repo.enqueue(&queued_row("run-c", "pending:run-c", Some("C"), 2))
        .unwrap();

    let b_epoch = repo
        .try_claim_for_worker("run-b", Some("worker-1"), -90_000, 10)
        .unwrap()
        .unwrap();
    assert!(repo.requeue_claimed("run-b", b_epoch, 20).unwrap());
    let requeued_b = repo.get_row("run-b").unwrap().unwrap();
    assert_eq!(requeued_b.status, QueueStatus::Queued);
    assert_eq!(requeued_b.worker_id, "pending:run-b");

    repo.try_claim_for_worker("run-c", Some("worker-1"), -90_000, 30)
        .unwrap()
        .expect("C reuses the released slot");
    repo.record_heartbeat(SESSION_ID, "worker-1", "completed", 40)
        .unwrap();

    let b_after_c = repo.get_row("run-b").unwrap().unwrap();
    let c_after_completion = repo.get_row("run-c").unwrap().unwrap();
    assert_eq!(b_after_c.status, QueueStatus::Queued);
    assert_eq!(b_after_c.worker_id, "pending:run-b");
    assert_eq!(c_after_completion.status, QueueStatus::Finalized);
}

#[test]
fn distinct_tasks_cannot_concurrently_claim_the_same_worker_slot() {
    let repo = queue_repo();
    repo.enqueue(&queued_row("run-b", "pending:run-b", Some("B"), 1))
        .unwrap();
    repo.enqueue(&queued_row("run-c", "pending:run-c", Some("C"), 2))
        .unwrap();

    let claims = race_bound_claims(&repo, &["run-b", "run-c"], "worker-1", 10);
    assert_eq!(
        claims.iter().filter(|claim| claim.is_some()).count(),
        1,
        "one roster slot cannot back two running task rows: {claims:?}"
    );
    let rows = repo.snapshot(SESSION_ID).unwrap().rows;
    assert_eq!(
        rows.iter()
            .filter(|row| row.status == QueueStatus::Running && row.worker_id == "worker-1")
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.status == QueueStatus::Queued)
            .count(),
        1
    );
}

#[test]
fn assignment_ids_advance_across_claim_rebind_and_release_paths() {
    let repo = queue_repo();
    repo.enqueue(&queued_row("run-a", "pending:run-a", Some("A"), 1))
        .unwrap();

    let epoch_1 = repo
        .try_claim_for_worker("run-a", Some("worker-1"), -90_000, 10)
        .unwrap()
        .unwrap();
    let claim_1 = repo.get_row("run-a").unwrap().unwrap().assignment_id;
    assert!(claim_1 > 0, "a won claim must mint an assignment identity");

    assert!(repo.requeue_claimed("run-a", epoch_1, 10).unwrap());
    let released = repo.get_row("run-a").unwrap().unwrap().assignment_id;
    assert!(
        released > claim_1,
        "a fenced release must invalidate the prior assignment even in the same millisecond"
    );

    let epoch_2 = repo
        .try_claim_for_worker("run-a", Some("worker-1"), -90_000, 10)
        .unwrap()
        .unwrap();
    let claim_2 = repo.get_row("run-a").unwrap().unwrap().assignment_id;
    assert!(
        claim_2 > released,
        "a rebind must mint a new identity without relying on wall-clock ordering"
    );

    assert!(repo.fail_claimed("run-a", epoch_2, 10).unwrap());
    let failed = repo.get_row("run-a").unwrap().unwrap().assignment_id;
    assert!(
        failed > claim_2,
        "terminal claim failure invalidates its identity"
    );
    assert!(repo.release_claim_manual("run-a", 10).unwrap());
    let manual_release = repo.get_row("run-a").unwrap().unwrap().assignment_id;
    assert!(
        manual_release > failed,
        "manual recovery mints a new identity"
    );

    repo.try_claim_for_worker("run-a", Some("worker-1"), -90_000, 10)
        .unwrap()
        .unwrap();
    let before_reclaim = repo.get_row("run-a").unwrap().unwrap().assignment_id;
    assert_eq!(
        repo.reclaim_stuck(11, 10).unwrap(),
        vec!["run-a".to_string()]
    );
    let reclaimed = repo.get_row("run-a").unwrap().unwrap().assignment_id;
    assert!(
        reclaimed > before_reclaim,
        "stale reclaim invalidates the assignment"
    );

    let epoch_4 = repo
        .try_claim_for_worker("run-a", Some("worker-1"), -90_000, 10)
        .unwrap()
        .unwrap();
    let before_spawn_release = repo.get_row("run-a").unwrap().unwrap().assignment_id;
    assert!(matches!(
        repo.release_failed_spawn("run-a", epoch_4, "worker-1", 3, 10)
            .unwrap(),
        crate::storage::queue::SpawnFailureRelease::Requeued { .. }
    ));
    assert!(
        repo.get_row("run-a").unwrap().unwrap().assignment_id > before_spawn_release,
        "failed-spawn release must fence the abandoned assignment"
    );

    repo.try_claim_for_worker("run-a", Some("worker-1"), -90_000, 10)
        .unwrap()
        .unwrap();
    let before_resume_requeue = repo.get_row("run-a").unwrap().unwrap().assignment_id;
    assert!(repo.requeue_running("run-a", 10).unwrap());
    assert!(
        repo.get_row("run-a").unwrap().unwrap().assignment_id > before_resume_requeue,
        "resume reconciliation must invalidate the orphaned assignment"
    );
}

#[test]
fn completed_heartbeat_from_reclaimed_original_worker_finalizes_via_spawn_identity() {
    let repo = queue_repo();
    repo.enqueue(&queued_row(
        "run-reclaimed",
        "pending:run-reclaimed",
        Some("RECLAIMED"),
        1,
    ))
    .unwrap();
    repo.try_claim_for_worker("run-reclaimed", Some("worker-original"), -90_000, 10)
        .unwrap()
        .expect("original worker claims the task");

    assert_eq!(repo.reclaim_stuck(11, 20).unwrap(), vec!["run-reclaimed"]);
    let reclaimed = repo.get_row("run-reclaimed").unwrap().unwrap();
    assert_eq!(reclaimed.status, QueueStatus::Queued);
    assert_eq!(reclaimed.worker_id, "pending:run-reclaimed");

    assert!(repo
        .record_heartbeat(SESSION_ID, "worker-original", "completed", 30)
        .unwrap());
    let finalized = repo.get_row("run-reclaimed").unwrap().unwrap();
    assert_eq!(finalized.status, QueueStatus::Finalized);
    assert_eq!(finalized.heartbeat_at, Some(30));
}

#[test]
fn spawned_identity_survives_every_worker_sentinel_rewrite() {
    use crate::storage::queue::SpawnFailureRelease;

    let repo = queue_repo();
    for (index, release_path) in [
        "requeue_claimed",
        "release_failed_spawn",
        "release_claim_manual",
        "reclaim_stuck",
        "requeue_running",
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("run-rewrite-{index}");
        let task_id = format!("REWRITE-{index}");
        let worker_id = format!("worker-rewrite-{index}");
        repo.enqueue(&queued_row(
            &id,
            &format!("pending:{id}"),
            Some(&task_id),
            index as i64,
        ))
        .unwrap();
        let epoch = repo
            .try_claim_for_worker(&id, Some(&worker_id), -90_000, 10)
            .unwrap()
            .expect("rewrite fixture claim");

        match release_path {
            "requeue_claimed" => assert!(repo.requeue_claimed(&id, epoch, 20).unwrap()),
            "release_failed_spawn" => assert_eq!(
                repo.release_failed_spawn(&id, epoch, &worker_id, 3, 20)
                    .unwrap(),
                SpawnFailureRelease::Requeued { failures: 1 }
            ),
            "release_claim_manual" => {
                assert!(repo.release_claim_manual(&id, 20).unwrap())
            }
            "reclaim_stuck" => assert_eq!(repo.reclaim_stuck(11, 20).unwrap(), vec![id.clone()]),
            "requeue_running" => assert!(repo.requeue_running(&id, 20).unwrap()),
            _ => unreachable!(),
        }

        let released = repo.get_row(&id).unwrap().unwrap();
        assert_eq!(released.status, QueueStatus::Queued, "{release_path}");
        assert_eq!(
            released.worker_id,
            format!("pending:{id}"),
            "{release_path}"
        );
        assert!(
            repo.record_heartbeat(SESSION_ID, &worker_id, "completed", 30)
                .unwrap(),
            "{release_path} severed the original spawn identity"
        );
        assert_eq!(
            repo.get_row(&id).unwrap().unwrap().status,
            QueueStatus::Finalized,
            "{release_path} completion did not finalize"
        );
    }
}

#[test]
fn superseded_assignment_id_cannot_finalize_newer_claim() {
    let repo = queue_repo();
    repo.enqueue(&queued_row(
        "run-fenced",
        "pending:run-fenced",
        Some("FENCED"),
        1,
    ))
    .unwrap();
    let first_epoch = repo
        .try_claim_for_worker("run-fenced", Some("worker-reused"), -90_000, 10)
        .unwrap()
        .unwrap();
    let first_assignment = repo.get_row("run-fenced").unwrap().unwrap().assignment_id;
    assert!(repo.requeue_claimed("run-fenced", first_epoch, 20).unwrap());
    repo.try_claim_for_worker("run-fenced", Some("worker-reused"), -90_000, 30)
        .unwrap()
        .expect("newer assignment reuses the worker slot");
    let newer_claim = repo.get_row("run-fenced").unwrap().unwrap();
    assert!(newer_claim.assignment_id > first_assignment);

    assert_eq!(
        repo.record_heartbeat_for_assignment(
            SESSION_ID,
            "worker-reused",
            Some(first_assignment),
            "completed",
            40,
        )
        .unwrap(),
        None,
        "a superseded assignment must not finalize the newer claim"
    );
    assert_eq!(repo.get_row("run-fenced").unwrap().unwrap(), newer_claim);
}

#[tokio::test]
async fn live_worker_is_never_reclaimed_regardless_of_heartbeat_age() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    let mut live_row = queue_row(
        "run-live",
        "worker-live",
        Some("LIVE"),
        QueueStatus::Running,
        1,
    );
    live_row.heartbeat_at = Some(1);
    live_row.last_status = Some("working".to_string());
    repo.enqueue(&live_row).unwrap();

    let manager = QueueManager::new_with_liveness_probe(
        Arc::clone(&repo),
        EventBus::new(temp.path().to_path_buf()),
        |worker_id| worker_id == "worker-live",
    );

    assert!(manager
        .reclaim_stuck_at(2, FIRST_HEARTBEAT_GRACE_MS + STUCK_CUTOFF_MS + 2)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        repo.get_row("run-live").unwrap().unwrap().status,
        QueueStatus::Running,
        "maintenance must preserve a live worker even when every heartbeat cutoff elapsed"
    );
    assert_eq!(
        manager
            .claim_and_spawn("run-live", SESSION_ID, "worker-replacement")
            .await
            .unwrap(),
        ClaimOutcome::AlreadyClaimed,
        "an opportunistic claim must use the same liveness guard as maintenance"
    );
}

#[tokio::test]
async fn first_heartbeat_grace_is_separate_from_steady_state_cutoff() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    let mut awaiting_first = queue_row(
        "run-awaiting-first",
        "worker-awaiting-first",
        Some("FIRST"),
        QueueStatus::Running,
        1,
    );
    awaiting_first.heartbeat_at = Some(1);
    let mut steady = queue_row(
        "run-steady",
        "worker-steady",
        Some("STEADY"),
        QueueStatus::Running,
        2,
    );
    steady.heartbeat_at = Some(1);
    steady.last_status = Some("working".to_string());
    repo.enqueue(&awaiting_first).unwrap();
    repo.enqueue(&steady).unwrap();

    let manager = QueueManager::new(Arc::clone(&repo), EventBus::new(temp.path().to_path_buf()));
    assert_eq!(
        manager
            .reclaim_stuck_at(2, STUCK_CUTOFF_MS + 2)
            .await
            .unwrap(),
        vec!["run-steady"],
        "the steady worker remains governed by the unchanged 90-second cutoff"
    );
    assert_eq!(
        repo.get_row("run-awaiting-first").unwrap().unwrap().status,
        QueueStatus::Running,
        "a worker awaiting its first heartbeat receives the separate startup grace"
    );

    assert_eq!(
        manager
            .reclaim_stuck_at(
                FIRST_HEARTBEAT_GRACE_MS + 2 - STUCK_CUTOFF_MS,
                FIRST_HEARTBEAT_GRACE_MS + 2,
            )
            .await
            .unwrap(),
        vec!["run-awaiting-first"],
        "the pre-first-heartbeat row becomes reclaimable only after its own grace expires"
    );
}

#[test]
fn new_assignment_resets_first_heartbeat_grace_across_every_retry_route() {
    use crate::storage::queue::SpawnFailureRelease;

    const FIRST_CLAIM_AT: i64 = 1_000;
    const FIRST_HEARTBEAT_AT: i64 = 1_100;
    const RELEASE_AT: i64 = 1_200;
    const RETRY_CLAIM_AT: i64 = 2_000;

    for retry_route in [
        "requeue_claimed",
        "release_failed_spawn",
        "release_claim_manual",
        "reclaim_stuck",
        "requeue_running",
        "direct_running_reclaim",
    ] {
        let repo = queue_repo();
        let id = format!("run-assignment-grace-{retry_route}");
        let task_id = format!("ASSIGNMENT-GRACE-{retry_route}");
        let first_worker = format!("worker-a-{retry_route}");
        let retry_worker = format!("worker-b-{retry_route}");
        repo.enqueue(&queued_row(
            &id,
            &format!("pending:{id}"),
            Some(&task_id),
            1,
        ))
        .unwrap();
        let first_epoch = repo
            .try_claim_for_worker(&id, Some(&first_worker), -90_000, FIRST_CLAIM_AT)
            .unwrap()
            .expect("first worker claims the row");
        assert!(repo
            .record_heartbeat(SESSION_ID, &first_worker, "working", FIRST_HEARTBEAT_AT,)
            .unwrap());
        assert_eq!(
            repo.get_row(&id).unwrap().unwrap().last_status.as_deref(),
            Some("working"),
            "{retry_route}: fixture must establish the prior assignment heartbeat"
        );

        if retry_route == "direct_running_reclaim" {
            repo.try_claim_for_worker_with_grace(
                &id,
                Some(&retry_worker),
                true,
                Some(&first_worker),
                FIRST_HEARTBEAT_AT + 1,
                -1,
                RETRY_CLAIM_AT,
            )
            .unwrap()
            .expect("stale running assignment is reclaimed directly");
        } else {
            match retry_route {
                "requeue_claimed" => {
                    assert!(repo.requeue_claimed(&id, first_epoch, RELEASE_AT).unwrap())
                }
                "release_failed_spawn" => assert_eq!(
                    repo.release_failed_spawn(&id, first_epoch, &first_worker, 3, RELEASE_AT,)
                        .unwrap(),
                    SpawnFailureRelease::Requeued { failures: 1 }
                ),
                "release_claim_manual" => {
                    assert!(repo.release_claim_manual(&id, RELEASE_AT).unwrap())
                }
                "reclaim_stuck" => assert_eq!(
                    repo.reclaim_stuck(FIRST_HEARTBEAT_AT + 1, RELEASE_AT)
                        .unwrap(),
                    vec![id.clone()]
                ),
                "requeue_running" => {
                    assert!(repo.requeue_running(&id, RELEASE_AT).unwrap())
                }
                _ => unreachable!(),
            }
            repo.try_claim_for_worker(&id, Some(&retry_worker), -90_000, RETRY_CLAIM_AT)
                .unwrap()
                .expect("retry worker claims the released row");
        }

        let retry = repo.get_row(&id).unwrap().unwrap();
        assert_eq!(retry.worker_id, retry_worker, "{retry_route}");
        assert_eq!(retry.status, QueueStatus::Running, "{retry_route}");
        assert_eq!(retry.heartbeat_at, Some(RETRY_CLAIM_AT), "{retry_route}");
        assert_eq!(
            retry.last_status, None,
            "{retry_route}: a new assignment must await its own first heartbeat"
        );

        let just_past_steady_cutoff = RETRY_CLAIM_AT + STUCK_CUTOFF_MS + 1;
        let reclaimable_worker = vec![(id.clone(), retry.worker_id.clone())];
        assert!(
            repo.reclaim_stuck_with_grace(
                just_past_steady_cutoff - STUCK_CUTOFF_MS,
                just_past_steady_cutoff - FIRST_HEARTBEAT_GRACE_MS,
                just_past_steady_cutoff,
                &reclaimable_worker,
            )
            .unwrap()
            .is_empty(),
            "{retry_route}: retry was reclaimed under the prior assignment's steady cutoff"
        );
        assert_eq!(
            repo.get_row(&id).unwrap().unwrap().status,
            QueueStatus::Running,
            "{retry_route}: retry must survive until its first-heartbeat grace expires"
        );

        let just_past_first_heartbeat_grace = RETRY_CLAIM_AT + FIRST_HEARTBEAT_GRACE_MS + 1;
        assert_eq!(
            repo.reclaim_stuck_with_grace(
                just_past_first_heartbeat_grace - STUCK_CUTOFF_MS,
                just_past_first_heartbeat_grace - FIRST_HEARTBEAT_GRACE_MS,
                just_past_first_heartbeat_grace,
                &reclaimable_worker,
            )
            .unwrap(),
            vec![id],
            "{retry_route}: no retry heartbeat eventually expires the assignment grace"
        );
    }
}

#[test]
fn first_heartbeat_latency_record_covers_every_cli_without_fabrication() {
    let recorded_clis = FIRST_HEARTBEAT_LATENCY_MS_BY_CLI
        .iter()
        .map(|(cli, _)| *cli)
        .collect::<Vec<_>>();
    assert_eq!(recorded_clis, crate::adapters::VALID_CLIS);
    assert!(
        FIRST_HEARTBEAT_LATENCY_MS_BY_CLI
            .iter()
            .all(|(_, latency_ms)| latency_ms.is_none()),
        "the 2026-08-16 evidence captured no durable heartbeat receipt timestamps"
    );
}

#[test]
fn heartbeat_updates_only_the_current_finalized_assignment_for_a_reused_slot() {
    let repo = queue_repo();
    repo.enqueue(&queued_row("zz-old", "pending:zz-old", Some("OLD"), 1))
        .unwrap();
    repo.try_claim_for_worker("zz-old", Some("worker-1"), -90_000, 10)
        .unwrap()
        .unwrap();
    repo.record_heartbeat(SESSION_ID, "worker-1", "completed", 20)
        .unwrap();

    repo.enqueue(&queued_row(
        "aa-current",
        "pending:aa-current",
        Some("CURRENT"),
        2,
    ))
    .unwrap();
    repo.try_claim_for_worker("aa-current", Some("worker-1"), -90_000, 30)
        .unwrap()
        .unwrap();
    repo.record_heartbeat(SESSION_ID, "worker-1", "completed", 40)
        .unwrap();

    let old_before = repo.get_row("zz-old").unwrap().unwrap();
    let current_before = repo.get_row("aa-current").unwrap().unwrap();
    assert!(current_before.assignment_id > old_before.assignment_id);
    assert_eq!(
        repo.record_heartbeat_for_assignment(SESSION_ID, "worker-1", None, "working", 999)
            .unwrap(),
        Some("aa-current".to_string()),
        "the legacy fallback must resolve the greatest durable assignment, not row order"
    );

    let old_after = repo.get_row("zz-old").unwrap().unwrap();
    let current_after = repo.get_row("aa-current").unwrap().unwrap();
    assert_eq!(
        (old_after.heartbeat_at, old_after.updated_at),
        (old_before.heartbeat_at, old_before.updated_at),
        "the sibling's historical liveness bytes must remain unchanged"
    );
    assert_eq!(
        (current_after.heartbeat_at, current_after.updated_at),
        (Some(999), 999)
    );

    let impossible_assignment = current_after.assignment_id + 100;
    assert_eq!(
        repo.record_heartbeat_for_assignment(
            SESSION_ID,
            "worker-1",
            Some(impossible_assignment),
            "working",
            1_111,
        )
        .unwrap(),
        None,
        "an explicit stale identity must not cascade into the fallback"
    );
    assert_eq!(repo.get_row("zz-old").unwrap().unwrap(), old_after);
    assert_eq!(repo.get_row("aa-current").unwrap().unwrap(), current_after);
}

#[test]
fn heartbeat_body_keeps_assignment_identity_optional_for_legacy_prompts() {
    let legacy: PostHeartbeatRequest = serde_json::from_value(json!({
        "agent_id": "worker-1",
        "status": "working"
    }))
    .unwrap();
    assert_eq!(legacy.assignment_id, None);

    let scoped: PostHeartbeatRequest = serde_json::from_value(json!({
        "agent_id": "worker-1",
        "status": "working",
        "assignment_id": 42
    }))
    .unwrap();
    assert_eq!(scoped.assignment_id, Some(42));
}

#[tokio::test]
async fn worker_finalized_event_uses_the_current_assignment_row_under_slot_reuse() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    repo.enqueue(&queued_row(
        "old-run",
        "pending:old-run",
        Some("old-task"),
        1,
    ))
    .unwrap();
    repo.try_claim_for_worker("old-run", Some("worker-1"), -90_000, 10)
        .unwrap()
        .unwrap();
    repo.record_heartbeat(SESSION_ID, "worker-1", "completed", 20)
        .unwrap();
    let old_assignment = repo.get_row("old-run").unwrap().unwrap().assignment_id;

    repo.enqueue(&queued_row(
        "current-run",
        "pending:current-run",
        Some("current-task"),
        2,
    ))
    .unwrap();
    repo.try_claim_for_worker("current-run", Some("worker-1"), -90_000, 30)
        .unwrap()
        .unwrap();
    let current_assignment = repo.get_row("current-run").unwrap().unwrap().assignment_id;
    assert!(current_assignment > old_assignment);

    let event_bus = EventBus::new(temp.path().to_path_buf());
    let mut events = event_bus.subscribe();
    let manager = QueueManager::new(Arc::clone(&repo), event_bus);
    assert!(manager
        .record_heartbeat(SESSION_ID, "worker-1", "completed")
        .await
        .unwrap());

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
        .await
        .expect("completion heartbeat must emit WorkerFinalized promptly")
        .unwrap();
    assert_eq!(event.event_type, EventType::WorkerFinalized);
    assert_eq!(event.payload["task_id"], "current-task");
    let completion_event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
        .await
        .expect("queue completion must emit WorkNodeCompleted promptly")
        .unwrap();
    assert_eq!(completion_event.event_type, EventType::WorkNodeCompleted);
    assert_eq!(completion_event.payload["task_id"], "current-task");
    let completion_facts =
        read_node_completion_facts(&temp.path().join("sessions").join(SESSION_ID)).unwrap();
    assert_eq!(completion_facts.len(), 1);
    assert_eq!(completion_facts[0].task_id, "current-task");
    assert_eq!(completion_facts[0].agent_id, "worker-1");
    assert_eq!(
        completion_facts[0].provenance,
        NodeCompletionProvenance::QueueFinalize
    );
    assert_eq!(
        repo.get_row("current-run").unwrap().unwrap().status,
        QueueStatus::Finalized
    );
    assert_eq!(
        repo.get_row("old-run").unwrap().unwrap().status,
        QueueStatus::Finalized
    );
}

#[tokio::test]
async fn recovery_prioritizes_live_claim_over_older_terminal_slot_history() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    repo.enqueue(&queue_row(
        "old-finalized",
        "worker-1",
        Some("A"),
        QueueStatus::Finalized,
        1,
    ))
    .unwrap();
    repo.enqueue(&queue_row(
        "old-failed",
        "worker-1",
        Some("B"),
        QueueStatus::Failed,
        2,
    ))
    .unwrap();
    repo.enqueue(&queue_row(
        "live-running",
        "worker-1",
        Some("C"),
        QueueStatus::Running,
        3,
    ))
    .unwrap();
    let manager = QueueManager::new(Arc::clone(&repo), EventBus::new(temp.path().to_path_buf()));

    assert_eq!(
        manager.release_claim(SESSION_ID, "worker-1").await.unwrap(),
        crate::coordination::ReleaseOutcome::Released {
            previous: QueueStatus::Running
        }
    );
    assert_eq!(
        repo.get_row("live-running").unwrap().unwrap().status,
        QueueStatus::Queued
    );
    assert_eq!(
        repo.get_row("old-failed").unwrap().unwrap().status,
        QueueStatus::Failed
    );
    assert_eq!(
        repo.get_row("old-finalized").unwrap().unwrap().status,
        QueueStatus::Finalized
    );

    assert_eq!(
        manager.release_claim(SESSION_ID, "worker-1").await.unwrap(),
        crate::coordination::ReleaseOutcome::Released {
            previous: QueueStatus::Failed
        }
    );
    assert_eq!(
        repo.get_row("old-failed").unwrap().unwrap().status,
        QueueStatus::Queued
    );
    assert_eq!(
        manager.release_claim(SESSION_ID, "worker-1").await.unwrap(),
        crate::coordination::ReleaseOutcome::Terminal {
            status: QueueStatus::Finalized
        }
    );
}

#[tokio::test]
async fn typed_pending_outcome_retries_to_real_claim_and_events_keep_task_id() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    let event_bus = EventBus::new(temp.path().to_path_buf());
    let mut events = event_bus.subscribe();
    let manager = QueueManager::new(Arc::clone(&repo), event_bus);

    manager
        .enqueue_worker_with_dependencies(
            "run-a",
            SESSION_ID,
            "worker-a",
            "backend",
            "codex",
            json!({}),
            Some("A".to_string()),
            &[],
        )
        .await
        .unwrap();
    manager
        .enqueue_worker_with_dependencies(
            "run-b",
            SESSION_ID,
            "worker-b",
            "backend",
            "codex",
            json!({}),
            Some("B".to_string()),
            &["A".to_string()],
        )
        .await
        .unwrap();

    assert_eq!(
        manager
            .claim_and_spawn("run-b", SESSION_ID, "worker-b")
            .await
            .unwrap(),
        ClaimOutcome::DependenciesPending {
            task_ids: vec!["A".to_string()]
        }
    );
    assert!(matches!(
        manager
            .claim_and_spawn("run-a", SESSION_ID, "worker-a")
            .await
            .unwrap(),
        ClaimOutcome::Claimed { epoch: 1 }
    ));
    manager
        .record_heartbeat(SESSION_ID, "worker-a", "completed")
        .await
        .unwrap();
    assert!(matches!(
        manager
            .claim_and_spawn("run-b", SESSION_ID, "worker-b")
            .await
            .unwrap(),
        ClaimOutcome::Claimed { epoch: 1 }
    ));
    assert_eq!(
        repo.get_row("run-b").unwrap().unwrap().task_id.as_deref(),
        Some("B")
    );

    let mut observed = Vec::new();
    for _ in 0..6 {
        observed.push(events.recv().await.unwrap());
    }
    let pending = observed
        .iter()
        .find(|event| {
            event.event_type == EventType::WorkerClaimFailed
                && event.agent_id.as_deref() == Some("worker-b")
        })
        .expect("normal dependency wait event");
    assert_eq!(pending.severity, Severity::Info);
    assert_eq!(pending.payload["task_id"], "B");
    assert!(observed
        .iter()
        .all(|event| event.payload.get("task_id").is_some()));
}

#[test]
fn checkpoint_projection_and_queue_sql_agree_on_claimability() {
    fn node(id: &str, status: NodeStatus) -> WorkNode {
        WorkNode::new(
            id,
            if id == "gate" {
                NodeKind::Checkpoint
            } else {
                NodeKind::Task
            },
            id,
            NodeContract::default(),
            BindingRef::Role("worker".to_string()),
            status,
        )
    }

    let graph = TaskGraph::new(
        vec![
            node("A", NodeStatus::Completed),
            node("gate", NodeStatus::Ready),
            node("B", NodeStatus::Ready),
        ],
        vec![
            WorkEdge::new("A", "gate", EdgeKind::DependsOn, EdgeProvenance::Planner),
            WorkEdge::new("gate", "B", EdgeKind::DependsOn, EdgeProvenance::Planner),
        ],
    );
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    let manager = QueueManager::new(Arc::clone(&repo), EventBus::new(temp.path().to_path_buf()));
    repo.enqueue(&queue_row(
        "run-a",
        "worker-a",
        Some("A"),
        QueueStatus::Finalized,
        1,
    ))
    .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-gate", "worker-gate", Some("gate"), 2),
        &["A".to_string()],
    )
    .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-b", "worker-b", Some("B"), 3),
        &["gate".to_string()],
    )
    .unwrap();
    let before_gate = manager.project_queue_statuses(SESSION_ID, &graph).unwrap();
    assert_eq!(checkpoint_aware_claimable_nodes(&before_gate), vec!["gate"]);
    assert!(repo.try_claim("run-gate", -90_000, 10).unwrap().is_some());
    assert_eq!(repo.try_claim("run-b", -90_000, 10).unwrap(), None);

    repo.record_heartbeat(SESSION_ID, "worker-gate", "completed", 20)
        .unwrap();
    let after_gate = manager.project_queue_statuses(SESSION_ID, &graph).unwrap();
    assert_eq!(checkpoint_aware_claimable_nodes(&after_gate), vec!["B"]);
    assert!(repo.try_claim("run-b", -90_000, 30).unwrap().is_some());
}

#[test]
fn operational_queue_projection_preserves_worker_conflict_claimability() {
    let node = |id: &str, status: NodeStatus| {
        WorkNode::new(
            id,
            NodeKind::Task,
            id,
            NodeContract::default(),
            BindingRef::Role("worker".to_string()),
            status,
        )
    };
    let graph = TaskGraph::new(
        vec![
            node("persisted-completed", NodeStatus::Completed),
            node("dependent", NodeStatus::Pending),
        ],
        vec![WorkEdge::new(
            "persisted-completed",
            "dependent",
            EdgeKind::DependsOn,
            EdgeProvenance::Planner,
        )],
    );
    let mut legacy_operational_projection = graph.clone();
    legacy_operational_projection.nodes[0].status = NodeStatus::Pending;
    promote_initial_ready_nodes(&mut legacy_operational_projection);
    let before = checkpoint_aware_claimable_nodes(&legacy_operational_projection);

    let temp = tempfile::tempdir().unwrap();
    let manager = QueueManager::new(
        Arc::new(queue_repo()),
        EventBus::new(temp.path().to_path_buf()),
    );
    let after = checkpoint_aware_claimable_nodes(
        &manager
            .project_queue_statuses(SESSION_ID, &graph)
            .expect("operational projection"),
    );

    assert_eq!(before, vec!["persisted-completed"]);
    assert_eq!(
        after, before,
        "workers.rs conflict analysis must retain the pre-split claimable set"
    );
}

#[test]
fn add_worker_request_task_id_is_explicit_or_null_and_backward_compatible() {
    let legacy: AddWorkerRequest = serde_json::from_value(json!({
        "role_type": "backend",
        "initial_task": "Implement T7, but never infer from this prose"
    }))
    .unwrap();
    assert_eq!(legacy.task_id, None);

    let linked: AddWorkerRequest = serde_json::from_value(json!({
        "role_type": "backend",
        "initial_task": "arbitrary prose",
        "task_id": "T7"
    }))
    .unwrap();
    assert_eq!(linked.task_id.as_deref(), Some("T7"));
}

fn dependency_http_fixture(
    temp: &tempfile::TempDir,
) -> (axum::Router, Arc<AppState>, Arc<RwLock<SessionController>>) {
    let base = temp.path().join("http-storage");
    let storage = Arc::new(SessionStorage::new_with_base(base.clone()).unwrap());
    let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let controller = Arc::new(RwLock::new(SessionController::new(Arc::clone(
        &pty_manager,
    ))));
    controller.write().set_storage(Arc::clone(&storage));
    let injection = Arc::new(RwLock::new(InjectionManager::new(
        Arc::clone(&pty_manager),
        SessionStorage::new_with_base(base).unwrap(),
    )));
    let event_bus = EventBus::new(storage.base_dir().clone());
    let db = Arc::new(ApplicationStateDb::open(storage.base_dir()).unwrap());
    let repo = Arc::new(QueueRepo::new(Arc::clone(&db)));
    repo.ensure_schema().unwrap();
    let queue_manager = Arc::new(QueueManager::new(repo, Arc::clone(&event_bus)));
    let state = Arc::new(AppState::new(
        config,
        pty_manager,
        Arc::clone(&controller),
        injection,
        Arc::clone(&storage),
        event_bus,
        db,
        queue_manager,
        None,
    ));
    state.set_registry(Arc::new(crate::actions::build_registry()));
    (create_router(Arc::clone(&state)), state, controller)
}

async fn post_task_worker(app: &axum::Router, task_id: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/http-dependency-session/workers")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "role_type": "researcher",
                        "cli": "codex",
                        "task_id": task_id,
                        "initial_task": format!("Execute {task_id}")
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post_heartbeat(
    app: &axum::Router,
    session_id: &str,
    agent_id: &str,
    status: &str,
) -> axum::response::Response {
    post_heartbeat_with_assignment(app, session_id, agent_id, status, None).await
}

async fn post_heartbeat_with_assignment(
    app: &axum::Router,
    session_id: &str,
    agent_id: &str,
    status: &str,
    assignment_id: Option<i64>,
) -> axum::response::Response {
    let mut payload = json!({
        "agent_id": agent_id,
        "status": status,
        "summary": format!("HTTP {status} heartbeat")
    });
    if let Some(assignment_id) = assignment_id {
        payload["assignment_id"] = json!(assignment_id);
    }
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{session_id}/heartbeat"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post_heartbeat_with_completed_nodes(
    app: &axum::Router,
    session_id: &str,
    agent_id: &str,
    completed_nodes: &[&str],
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{session_id}/heartbeat"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "agent_id": agent_id,
                        "status": "completed",
                        "summary": "declared node completion",
                        "completed_nodes": completed_nodes,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn completed_nodes_are_all_or_nothing_and_project_into_the_live_view() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "declared-heartbeat-session";
    let worker_id = format!("{session_id}-worker-1");
    let project = temp.path().join("declared-project");
    std::fs::create_dir_all(&project).unwrap();
    let session_dir = state.storage.create_session_dir(session_id).unwrap();
    StateManager::new(session_dir.clone())
        .write_work_graph(&TaskGraph::new(
            vec![
                queue_test_node("T1", NodeStatus::Pending),
                queue_test_node("T2", NodeStatus::Pending),
                queue_test_node("T3", NodeStatus::Pending),
            ],
            Vec::new(),
        ))
        .unwrap();
    let mut session = quiet_hive_session(session_id, &project);
    session.agents.push(AgentInfo {
        id: worker_id.clone(),
        role: AgentRole::Worker {
            index: 1,
            parent: Some(format!("{session_id}-queen")),
        },
        status: AgentStatus::Running,
        config: AgentConfig::default(),
        parent_id: Some(format!("{session_id}-queen")),
        commit_sha: None,
        base_commit_sha: None,
        role_definition_id: None,
        role_definition_version: None,
    });
    controller.read().insert_test_session(session);

    let rejected =
        post_heartbeat_with_completed_nodes(&app, session_id, "worker-1", &["T2", "UNKNOWN"]).await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let rejected_body = String::from_utf8(
        to_bytes(rejected.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(rejected_body.contains("UNKNOWN"));
    assert!(StateManager::new(session_dir.clone())
        .read_node_completion_facts()
        .unwrap()
        .is_empty());
    assert!(!controller
        .read()
        .get_heartbeat_info(session_id)
        .contains_key(&worker_id));

    let accepted =
        post_heartbeat_with_completed_nodes(&app, session_id, "worker-1", &["T2", "T3"]).await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let facts = StateManager::new(session_dir)
        .read_node_completion_facts()
        .unwrap();
    assert_eq!(facts.len(), 2);
    assert!(facts.iter().all(|fact| {
        fact.agent_id == worker_id && fact.provenance == NodeCompletionProvenance::Heartbeat
    }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/sessions/{session_id}/work-graph?view=runtime&source=live"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    for task_id in ["T2", "T3"] {
        let node = body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == task_id)
            .unwrap();
        assert_eq!(node["status"], "completed");
        assert_eq!(node["progress"]["agent_id"], worker_id);
        assert_eq!(body["completion_provenance"][task_id], "declared");
    }
}

async fn post_queen_injection(
    app: &axum::Router,
    session_id: &str,
    queen_id: &str,
    worker_id: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{session_id}/inject/queen"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "queen_id": queen_id,
                        "target_worker_id": worker_id,
                        "message": "The task file contains a released follow-up assignment",
                        "submit": true
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn http_reassignment_refreshes_finalized_liveness_and_restores_stall_coverage() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-heartbeat-reassignment";
    let worker_id = format!("{session_id}-worker-1");
    let queen_id = format!("{session_id}-queen");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();

    let task_file = project
        .join(".hive-manager")
        .join(session_id)
        .join("tasks")
        .join("worker-1-task.md");
    std::fs::create_dir_all(task_file.parent().unwrap()).unwrap();
    std::fs::write(&task_file, "# Task\n\n## Status: COMPLETED\n").unwrap();

    let mut session = quiet_hive_session(session_id, &project);
    session.agents.push(AgentInfo {
        id: worker_id.clone(),
        role: AgentRole::Worker {
            index: 1,
            parent: Some(queen_id.clone()),
        },
        status: AgentStatus::Running,
        config: AgentConfig::default(),
        parent_id: Some(queen_id.clone()),
        commit_sha: None,
        base_commit_sha: None,
        role_definition_id: None,
        role_definition_version: None,
    });
    controller.read().insert_test_session(session);
    state
        .pty_manager
        .write()
        .create_session(
            worker_id.clone(),
            AgentRole::Worker {
                index: 1,
                parent: Some(queen_id.clone()),
            },
            "claude",
            &[],
            project.to_str(),
            80,
            24,
        )
        .unwrap();

    state
        .queue_manager
        .enqueue_worker(
            "run-reassignment",
            session_id,
            &worker_id,
            "backend",
            "claude",
            json!({}),
            Some("T14".to_string()),
        )
        .await
        .unwrap();
    assert!(matches!(
        state
            .queue_manager
            .claim_and_spawn("run-reassignment", session_id, &worker_id)
            .await
            .unwrap(),
        ClaimOutcome::Claimed { .. }
    ));

    // Post a bare alias so the HTTP handler must resolve it to the roster/queue identity.
    assert_eq!(
        post_heartbeat(&app, session_id, "worker-1", "completed")
            .await
            .status(),
        StatusCode::OK
    );
    let completed_row = state
        .queue_manager
        .repo()
        .get_row("run-reassignment")
        .unwrap()
        .unwrap();
    assert_eq!(completed_row.status, QueueStatus::Finalized);
    let completed_heartbeat = completed_row.heartbeat_at.expect("completed liveness");

    std::thread::sleep(std::time::Duration::from_millis(1_100));
    assert!(
        controller
            .read()
            .get_stalled_agents(session_id, std::time::Duration::ZERO)
            .is_empty(),
        "a completed agent with no later assignment must stay silent"
    );

    // Reassignment is durable direction in the standing task file; injection only wakes the
    // existing agent loop. Coverage must resume even if that injection were never submitted.
    std::fs::write(
        &task_file,
        "# Task\n\n## Status: ACTIVE\n\n## Wave 2 Assignment — RELEASED\n",
    )
    .unwrap();
    assert_eq!(
        post_queen_injection(&app, session_id, &queen_id, &worker_id)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        controller
            .read()
            .get_stalled_agents(session_id, std::time::Duration::ZERO)
            .into_iter()
            .map(|(agent_id, _)| agent_id)
            .collect::<Vec<_>>(),
        vec![worker_id.clone()],
        "the task-file reassignment must make the stale completed heartbeat eligible"
    );

    assert_eq!(
        post_heartbeat_with_assignment(
            &app,
            session_id,
            "worker-1",
            "working",
            Some(completed_row.assignment_id),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let refreshed_row = state
        .queue_manager
        .repo()
        .get_row("run-reassignment")
        .unwrap()
        .unwrap();
    assert_eq!(refreshed_row.status, QueueStatus::Finalized);
    assert_eq!(refreshed_row.last_status.as_deref(), Some("completed"));
    assert!(
        refreshed_row.heartbeat_at.unwrap() > completed_heartbeat,
        "the finalized row must expose the reassigned agent's fresh liveness"
    );

    let heartbeat_info = controller.read().get_heartbeat_info(session_id);
    assert!(!heartbeat_info.contains_key("worker-1"));
    assert_eq!(heartbeat_info[&worker_id].status, "working");
    assert!(
        controller
            .read()
            .get_stalled_agents(session_id, std::time::Duration::ZERO)
            .is_empty(),
        "the reassigned agent is fresh immediately after its working heartbeat"
    );
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    assert_eq!(
        controller
            .read()
            .get_stalled_agents(session_id, std::time::Duration::ZERO)
            .into_iter()
            .map(|(agent_id, _)| agent_id)
            .collect::<Vec<_>>(),
        vec![worker_id],
        "stall coverage must keep applying after the reassigned agent's working heartbeat"
    );
}

#[tokio::test]
async fn http_manual_release_fails_closed_before_touching_an_in_flight_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let worker_id = "http-dependency-session-worker-1";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));
    state
        .queue_manager
        .enqueue_worker(
            "run-in-flight",
            session_id,
            "pending:run-in-flight",
            "backend",
            "codex",
            json!({}),
            Some("IN_FLIGHT".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(
        state
            .queue_manager
            .claim_and_reserve_spawn("run-in-flight", session_id, worker_id, None)
            .await
            .unwrap(),
        ClaimOutcome::Claimed { epoch: 1 }
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/sessions/{session_id}/workers/{worker_id}/release"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["reason"], "spawn_in_flight");
    assert_eq!(body["epoch"], 1);
    let protected = state
        .queue_manager
        .queue_snapshot(session_id)
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.id == "run-in-flight")
        .unwrap();
    assert_eq!(protected.status, QueueStatus::Running);
    assert_eq!(protected.attempts, 1);
    assert!(controller
        .read()
        .get_session(session_id)
        .unwrap()
        .agents
        .is_empty());
}

#[tokio::test]
async fn http_retry_spawns_dependent_without_reusing_another_tasks_queue_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));

    let graph = TaskGraph::new(
        vec![
            queue_test_node("A", NodeStatus::Ready),
            queue_test_node("B", NodeStatus::Ready),
            queue_test_node("C", NodeStatus::Ready),
        ],
        vec![WorkEdge::new(
            "A",
            "B",
            EdgeKind::DependsOn,
            EdgeProvenance::Planner,
        )],
    );
    StateManager::new(state.storage.session_dir(session_id))
        .write_work_graph(&graph)
        .unwrap();

    let waiting_b = post_task_worker(&app, "B").await;
    let waiting_status = waiting_b.status();
    let waiting_body: serde_json::Value =
        serde_json::from_slice(&to_bytes(waiting_b.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(
        waiting_status,
        StatusCode::CONFLICT,
        "unexpected dependency wait response: {waiting_body}"
    );
    assert_eq!(waiting_body["reason"], "dependencies_pending");
    assert_eq!(
        controller
            .read()
            .get_session(session_id)
            .unwrap()
            .agents
            .len(),
        0
    );

    // C is ready and must not collide with B's dependency-pending queue intent even though
    // both requests initially reserve worker-1.
    let spawned_c = post_task_worker(&app, "C").await;
    assert_eq!(spawned_c.status(), StatusCode::CREATED);
    assert_eq!(
        controller
            .read()
            .get_session(session_id)
            .unwrap()
            .agents
            .len(),
        1
    );
    let after_c = state.queue_manager.queue_snapshot(session_id).unwrap();
    assert!(after_c
        .rows
        .iter()
        .find(|row| row.task_id.as_deref() == Some("B"))
        .unwrap()
        .worker_id
        .starts_with("pending:task:"));
    assert!(after_c
        .rows
        .iter()
        .find(|row| row.task_id.as_deref() == Some("C"))
        .unwrap()
        .worker_id
        .ends_with("worker-1"));

    state
        .queue_manager
        .enqueue_worker(
            "run-a",
            session_id,
            "prerequisite-a",
            "backend",
            "codex",
            json!({}),
            Some("A".to_string()),
        )
        .await
        .unwrap();
    assert!(matches!(
        state
            .queue_manager
            .claim_and_spawn("run-a", session_id, "prerequisite-a")
            .await
            .unwrap(),
        ClaimOutcome::Claimed { .. }
    ));
    state
        .queue_manager
        .record_heartbeat(session_id, "prerequisite-a", "completed")
        .await
        .unwrap();

    // Retrying the same task request now claims its preserved row, atomically rebinds it to
    // the roster's current worker-2 slot, and reaches the real controller/PTy spawn path.
    let spawned_b = post_task_worker(&app, "B").await;
    let spawned_b_status = spawned_b.status();
    let spawned_b_body: serde_json::Value =
        serde_json::from_slice(&to_bytes(spawned_b.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(spawned_b_status, StatusCode::CREATED);
    let task_file = spawned_b_body["task_file"].as_str().unwrap();
    let task_file_body = std::fs::read_to_string(task_file).unwrap();
    assert!(task_file_body.contains("## Plan Task ID"));
    assert!(task_file_body.contains("\"B\""));
    let session = controller.read().get_session(session_id).unwrap();
    assert_eq!(session.agents.len(), 2);
    assert!(session
        .agents
        .iter()
        .any(|agent| agent.id.ends_with("worker-2")));

    let snapshot = state.queue_manager.queue_snapshot(session_id).unwrap();
    let b = snapshot
        .rows
        .iter()
        .find(|row| row.task_id.as_deref() == Some("B"))
        .unwrap();
    let c = snapshot
        .rows
        .iter()
        .find(|row| row.task_id.as_deref() == Some("C"))
        .unwrap();
    assert!(b.worker_id.ends_with("worker-2"));
    assert!(c.worker_id.ends_with("worker-1"));
    assert_ne!(b.id, c.id, "stable task queue identities must not collide");
    assert!(
        state.queue_manager.spawn_in_flight(&b.id).is_none(),
        "the real HTTP success path must finish the fenced spawn handoff"
    );
}

#[tokio::test]
async fn http_unknown_task_is_persisted_nonclaimable_then_reconciles_and_spawns() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));
    let mut graph = TaskGraph::new(
        vec![
            queue_test_node("A", NodeStatus::Ready),
            queue_test_node("B", NodeStatus::Pending),
        ],
        vec![WorkEdge::new(
            "A",
            "B",
            EdgeKind::DependsOn,
            EdgeProvenance::Planner,
        )],
    );
    let state_manager = StateManager::new(state.storage.session_dir(session_id));
    state_manager.write_work_graph(&graph).unwrap();

    let unresolved = post_task_worker(&app, "UNKNOWN").await;
    assert_eq!(unresolved.status(), StatusCode::CONFLICT);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(unresolved.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(body["reason"], "resolution_incomplete");
    assert_eq!(body["task_id"], "UNKNOWN");
    assert_eq!(
        controller
            .read()
            .get_session(session_id)
            .unwrap()
            .agents
            .len(),
        0
    );
    let pending = state.queue_manager.queue_snapshot(session_id).unwrap();
    assert_eq!(pending.queued, 1);
    assert_eq!(pending.resolution_incomplete.len(), 1);

    graph
        .nodes
        .push(queue_test_node("UNKNOWN", NodeStatus::Ready));
    state_manager.write_work_graph(&graph).unwrap();
    let spawned = post_task_worker(&app, "UNKNOWN").await;
    assert_eq!(spawned.status(), StatusCode::CREATED);
    let reconciled = state.queue_manager.queue_snapshot(session_id).unwrap();
    assert!(reconciled.resolution_incomplete.is_empty());
    assert_eq!(reconciled.running, 1);
    assert_eq!(
        controller
            .read()
            .get_session(session_id)
            .unwrap()
            .agents
            .len(),
        1
    );
}

#[tokio::test]
async fn http_materializes_known_code_conflict_then_spawns_after_peer_finalizes() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    let mut session = quiet_hive_session(session_id, &project);
    session.execution_policy.workspace_strategy = crate::domain::WorkspaceStrategy::SharedCell;
    controller.read().insert_test_session(session);

    let graph = TaskGraph::new(
        vec![
            queue_test_node("T1", NodeStatus::Ready),
            queue_test_node("T2", NodeStatus::Ready),
        ],
        Vec::new(),
    );
    let composition = crate::orchestrator::work_graph::runtime::GraphCompositionState {
        graph,
        lineage: None,
        codegraph: crate::orchestrator::work_graph::codegraph::CodegraphDerivationReport {
            available: true,
            artifact_languages: BTreeSet::from(["rust".to_string()]),
            touches: BTreeMap::from([
                (
                    "T1".to_string(),
                    BTreeSet::from(["src/shared.rs".to_string()]),
                ),
                (
                    "T2".to_string(),
                    BTreeSet::from(["src/shared.rs".to_string()]),
                ),
            ]),
            unresolved_task_ids: Vec::new(),
            module_node_count: 1,
            touch_edge_count: 2,
        },
        context: crate::orchestrator::work_graph::context::ContextDerivationReport {
            gotchas: Vec::new(),
            hub_lints: Vec::new(),
            source_fingerprints: Vec::new(),
            knowledge_available: false,
            touches_available: true,
            knowledge_edge_count: 0,
        },
        reviews: Default::default(),
    };
    StateManager::new(state.storage.session_dir(session_id))
        .write_graph_composition_state(&composition)
        .unwrap();

    assert_eq!(
        post_task_worker(&app, "T1").await.status(),
        StatusCode::CREATED
    );
    let waiting = post_task_worker(&app, "T2").await;
    assert_eq!(waiting.status(), StatusCode::CONFLICT);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(waiting.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["reason"], "conflicts_pending");
    assert_eq!(body["blocking_task_ids"], json!(["T1"]));
    assert!(body["conflict_reasons"][0]
        .as_str()
        .unwrap()
        .contains("src/shared.rs"));
    assert_eq!(
        controller
            .read()
            .get_session(session_id)
            .unwrap()
            .agents
            .len(),
        1
    );
    assert_eq!(
        state
            .queue_manager
            .queue_snapshot(session_id)
            .unwrap()
            .conflict_coverage
            .unwrap()
            .state,
        "complete"
    );

    state
        .queue_manager
        .record_heartbeat(session_id, "http-dependency-session-worker-1", "completed")
        .await
        .unwrap();
    assert_eq!(
        post_task_worker(&app, "T2").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        controller
            .read()
            .get_session(session_id)
            .unwrap()
            .agents
            .len(),
        2
    );
}

#[tokio::test]
async fn http_staggered_readiness_serializes_against_an_already_running_peer() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    let mut session = quiet_hive_session(session_id, &project);
    session.execution_policy.workspace_strategy = crate::domain::WorkspaceStrategy::SharedCell;
    controller.read().insert_test_session(session);

    let graph = TaskGraph::new(
        vec![
            queue_test_node("T1", NodeStatus::Ready),
            queue_test_node("DEP", NodeStatus::Ready),
            queue_test_node("T2", NodeStatus::Pending),
        ],
        vec![WorkEdge::new(
            "DEP",
            "T2",
            EdgeKind::DependsOn,
            EdgeProvenance::Planner,
        )],
    );
    let composition = queue_test_composition(
        graph,
        BTreeMap::from([
            (
                "T1".to_string(),
                BTreeSet::from(["src/shared.rs".to_string()]),
            ),
            ("DEP".to_string(), BTreeSet::new()),
            (
                "T2".to_string(),
                BTreeSet::from(["src/shared.rs".to_string()]),
            ),
        ]),
        Vec::new(),
    );
    StateManager::new(state.storage.session_dir(session_id))
        .write_graph_composition_state(&composition)
        .unwrap();

    assert_eq!(
        post_task_worker(&app, "T1").await.status(),
        StatusCode::CREATED
    );
    let dependency_wait = post_task_worker(&app, "T2").await;
    assert_eq!(dependency_wait.status(), StatusCode::CONFLICT);
    let dependency_body: serde_json::Value = serde_json::from_slice(
        &to_bytes(dependency_wait.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(dependency_body["reason"], "dependencies_pending");

    state
        .queue_manager
        .enqueue_worker(
            "run-dep",
            session_id,
            "dependency-worker",
            "backend",
            "codex",
            json!({}),
            Some("DEP".to_string()),
        )
        .await
        .unwrap();
    assert!(matches!(
        state
            .queue_manager
            .claim_and_spawn("run-dep", session_id, "dependency-worker")
            .await
            .unwrap(),
        ClaimOutcome::Claimed { .. }
    ));
    state
        .queue_manager
        .record_heartbeat(session_id, "dependency-worker", "completed")
        .await
        .unwrap();

    // T2 only became ready after T1 had already started. The queue projection must include
    // that running peer in conflict analysis, while the claim UPDATE remains sole authority.
    let conflict_wait = post_task_worker(&app, "T2").await;
    assert_eq!(conflict_wait.status(), StatusCode::CONFLICT);
    let conflict_body: serde_json::Value = serde_json::from_slice(
        &to_bytes(conflict_wait.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(conflict_body["reason"], "conflicts_pending");
    assert_eq!(conflict_body["blocking_task_ids"], json!(["T1"]));

    state
        .queue_manager
        .record_heartbeat(session_id, "http-dependency-session-worker-1", "completed")
        .await
        .unwrap();
    assert_eq!(
        post_task_worker(&app, "T2").await.status(),
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn unresolved_running_task_keeps_conflict_coverage_partial() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    let mut session = quiet_hive_session(session_id, &project);
    session.execution_policy.workspace_strategy = crate::domain::WorkspaceStrategy::SharedCell;
    controller.read().insert_test_session(session);

    let composition = queue_test_composition(
        TaskGraph::new(
            vec![
                queue_test_node("UNRESOLVED", NodeStatus::Ready),
                queue_test_node("RESOLVED", NodeStatus::Ready),
            ],
            Vec::new(),
        ),
        BTreeMap::from([(
            "RESOLVED".to_string(),
            BTreeSet::from(["src/resolved.rs".to_string()]),
        )]),
        vec!["UNRESOLVED".to_string()],
    );
    StateManager::new(state.storage.session_dir(session_id))
        .write_graph_composition_state(&composition)
        .unwrap();

    assert_eq!(
        post_task_worker(&app, "UNRESOLVED").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        post_task_worker(&app, "RESOLVED").await.status(),
        StatusCode::CREATED
    );
    let coverage = state
        .queue_manager
        .queue_snapshot(session_id)
        .unwrap()
        .conflict_coverage
        .expect("composition coverage is visible");
    assert_eq!(coverage.state, "partial");
    assert_eq!(coverage.unresolved_task_ids, vec!["UNRESOLVED"]);
}

#[tokio::test]
async fn complete_edgeless_graph_rejects_unknown_explicit_task_without_spawning() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));
    StateManager::new(state.storage.session_dir(session_id))
        .write_work_graph(&TaskGraph::new(
            vec![queue_test_node("KNOWN", NodeStatus::Ready)],
            Vec::new(),
        ))
        .unwrap();

    let response = post_task_worker(&app, "UNKNOWN").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["reason"], "resolution_incomplete");
    assert_eq!(body["task_id"], "UNKNOWN");
    assert_eq!(
        controller
            .read()
            .get_session(session_id)
            .unwrap()
            .agents
            .len(),
        0
    );
    let snapshot = state.queue_manager.queue_snapshot(session_id).unwrap();
    assert_eq!(snapshot.queued, 1);
    assert_eq!(snapshot.running, 0);
    assert_eq!(snapshot.resolution_incomplete.len(), 1);
}

#[tokio::test]
async fn complete_edgeless_graph_admits_known_explicit_task() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));
    StateManager::new(state.storage.session_dir(session_id))
        .write_work_graph(&TaskGraph::new(
            vec![queue_test_node("KNOWN", NodeStatus::Ready)],
            Vec::new(),
        ))
        .unwrap();

    assert_eq!(
        post_task_worker(&app, "KNOWN").await.status(),
        StatusCode::CREATED
    );
    let snapshot = state.queue_manager.queue_snapshot(session_id).unwrap();
    assert!(snapshot.resolution_incomplete.is_empty());
    assert_eq!(snapshot.running, 1);
    assert_eq!(
        controller
            .read()
            .get_session(session_id)
            .unwrap()
            .agents
            .len(),
        1
    );
}

#[tokio::test]
async fn degraded_empty_graph_rejects_unknown_and_retains_its_omission() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));
    let mut degraded = TaskGraph::default();
    degraded
        .omissions
        .push(crate::orchestrator::work_graph::WorkGraphOmission::new(
            crate::orchestrator::work_graph::WorkGraphOmissionReason::ResolutionIncomplete,
            1,
            vec!["planner metadata malformed".to_string()],
        ));
    let state_manager = StateManager::new(state.storage.session_dir(session_id));
    state_manager.write_work_graph(&degraded).unwrap();

    let response = post_task_worker(&app, "UNKNOWN").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["reason"], "resolution_incomplete");
    assert_eq!(body["task_id"], "UNKNOWN");
    assert_eq!(state_manager.read_work_graph().unwrap(), Some(degraded));
    let snapshot = state.queue_manager.queue_snapshot(session_id).unwrap();
    assert_eq!(snapshot.resolution_incomplete.len(), 1);
    assert_eq!(
        controller
            .read()
            .get_session(session_id)
            .unwrap()
            .agents
            .len(),
        0
    );
}

#[tokio::test]
async fn unreadable_authoritative_graph_fails_closed_before_enqueue() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let session_dir = state.storage.create_session_dir(session_id).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));
    std::fs::write(
        session_dir.join("state").join("work-graph.json"),
        "not-json",
    )
    .unwrap();

    let response = post_task_worker(&app, "B").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(state
        .queue_manager
        .queue_snapshot(session_id)
        .unwrap()
        .rows
        .is_empty());
}

#[tokio::test]
async fn absent_legacy_graph_keeps_edgeless_http_spawn_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let session_dir = state.storage.create_session_dir(session_id).unwrap();
    std::fs::remove_file(session_dir.join("state").join("work-graph.json")).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));

    let response = post_task_worker(&app, "legacy-task").await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let snapshot = state.queue_manager.queue_snapshot(session_id).unwrap();
    assert_eq!(snapshot.running, 1);
    assert_eq!(snapshot.rows[0].task_id.as_deref(), Some("legacy-task"));
}

fn queue_test_node(id: &str, status: NodeStatus) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Task,
        id,
        NodeContract::default(),
        BindingRef::Role("worker".to_string()),
        status,
    )
}

fn queue_test_composition(
    graph: TaskGraph,
    touches: BTreeMap<String, BTreeSet<String>>,
    unresolved_task_ids: Vec<String>,
) -> crate::orchestrator::work_graph::runtime::GraphCompositionState {
    crate::orchestrator::work_graph::runtime::GraphCompositionState {
        graph,
        lineage: None,
        codegraph: crate::orchestrator::work_graph::codegraph::CodegraphDerivationReport {
            available: true,
            artifact_languages: BTreeSet::from(["rust".to_string()]),
            module_node_count: touches.values().map(BTreeSet::len).sum(),
            touch_edge_count: touches.values().map(BTreeSet::len).sum(),
            touches,
            unresolved_task_ids,
        },
        context: crate::orchestrator::work_graph::context::ContextDerivationReport {
            gotchas: Vec::new(),
            hub_lints: Vec::new(),
            source_fingerprints: Vec::new(),
            knowledge_available: false,
            touches_available: true,
            knowledge_edge_count: 0,
        },
        reviews: Default::default(),
    }
}

fn quiet_hive_session(session_id: &str, project_path: &Path) -> Session {
    let mut session = quiet_fusion_session(session_id, project_path);
    session.session_type = SessionType::Hive { worker_count: 0 };
    session.last_activity_at = Utc::now();
    session
}

fn quiet_fusion_session(session_id: &str, project_path: &Path) -> Session {
    Session {
        id: session_id.to_string(),
        name: None,
        color: None,
        session_type: SessionType::Fusion {
            variants: vec!["alpha".to_string()],
        },
        project_path: project_path.to_path_buf(),
        state: SessionState::Running,
        created_at: Utc::now() - Duration::minutes(12),
        last_activity_at: Utc::now() - Duration::minutes(11),
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

#[test]
fn mark_session_completed_schedules_archive_after_persistence() {
    let temp = tempfile::tempdir().unwrap();
    let session_id = "completion-archive";
    let storage = Arc::new(SessionStorage::new_with_base(temp.path().join("storage")).unwrap());
    let session_dir = storage.create_session_dir(session_id).unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let mut controller = SessionController::new(Arc::new(RwLock::new(PtyManager::new())));
    controller.set_storage(Arc::clone(&storage));
    controller.insert_test_session(quiet_fusion_session(session_id, &project));
    controller
        .mark_session_completed(session_id)
        .expect("completion succeeds independently of archive scheduling");
    assert_eq!(
        controller.get_session(session_id).unwrap().state,
        SessionState::Completed
    );
    assert!(
        storage
            .load_session(session_id)
            .unwrap()
            .state
            .contains("Completed"),
        "session must be durable before the archive hook runs"
    );

    let archive_dir = session_dir.join("archive").join("work-graphs");
    let archive = (0..200)
        .find_map(|_| {
            let found = std::fs::read_dir(&archive_dir)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"));
            if found.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            found
        })
        .expect("completion schedules the idempotent work-graph archive");
    assert!(archive.is_file());
}
