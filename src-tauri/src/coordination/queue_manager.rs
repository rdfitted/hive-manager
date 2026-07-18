//! [`QueueManager`] — the coordination-layer subsystem over the durable run queue (#126).
//!
//! Wraps the SQLite-backed [`QueueRepo`] plus the [`EventBus`]. Every mutation persists to
//! the DB FIRST and emits the matching lifecycle event SECOND, so the UI never observes an
//! event that the database has not yet committed.
//!
//! The queue table is the source of truth for sub-agent runs; the in-memory
//! `Session.agents` Vec is a UI cache reconciled against the table on resume.

use std::sync::Arc;

use chrono::Utc;

use crate::domain::event::{Event, EventType, Severity};
use crate::events::EventBus;
use crate::storage::queue::{QueueRepo, QueueRow, QueueSnapshot, QueueStatus};
use crate::storage::StorageError;

/// A `running` row whose heartbeat is older than this (millis) is treated as stuck and is
/// reclaimable. 90s = 3x the 30s maintenance interval, comfortably inside the 180s stall
/// threshold. Reclaim only flips the row back to claimable — it never kills a live PTY, so a
/// genuinely-working-but-quiet worker that keeps heartbeating is never reclaimed.
pub const STUCK_CUTOFF_MS: i64 = 90_000;

/// Finalize a run once it has produced this many continuations (distinct status changes).
pub const MAX_CONTINUATIONS: i64 = 8;

/// Finalize a run once it has produced this many consecutive no-progress heartbeats.
pub const MAX_NO_PROGRESS_CONTINUATIONS: i64 = 5;

/// Coordination-layer façade over the durable queue. Cheaply clonable.
#[derive(Clone)]
pub struct QueueManager {
    repo: Arc<QueueRepo>,
    event_bus: Arc<EventBus>,
}

impl QueueManager {
    pub fn new(repo: Arc<QueueRepo>, event_bus: Arc<EventBus>) -> Self {
        Self { repo, event_bus }
    }

    /// Current millis-since-epoch.
    fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }

    /// Enqueue a worker run BEFORE spawning it. Persists a `queued` row, then emits
    /// `WorkerQueued`. Idempotent on `id` — a duplicate enqueue does not overwrite an
    /// already-claimed row.
    ///
    /// The arguments map 1:1 onto the queue row's identity columns; bundling them into a
    /// struct would add ceremony without clarifying the call site.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_worker(
        &self,
        id: &str,
        session_id: &str,
        worker_id: &str,
        role_type: &str,
        cli: &str,
        payload: serde_json::Value,
        task_id: Option<String>,
    ) -> Result<(), StorageError> {
        let now = Self::now_ms();
        let row = QueueRow {
            id: id.to_string(),
            task_id,
            session_id: session_id.to_string(),
            worker_id: worker_id.to_string(),
            role_type: role_type.to_string(),
            cli: cli.to_string(),
            status: QueueStatus::Queued,
            payload,
            attempts: 0,
            continuation_count: 0,
            no_progress_count: 0,
            last_status: None,
            heartbeat_at: None,
            created_at: now,
            updated_at: now,
        };
        self.repo.enqueue(&row)?;
        self.emit(session_id, worker_id, EventType::WorkerQueued, Severity::Info)
            .await;
        Ok(())
    }

    /// Atomically claim a queued (or stale-running) row, flipping it to `running`. Returns
    /// `true` iff THIS call won the claim. Emits `WorkerClaimed` on a win and
    /// `WorkerClaimFailed` on a loss. The HTTP handler proceeds to spawn only on `true`.
    pub async fn claim_and_spawn(
        &self,
        id: &str,
        session_id: &str,
        worker_id: &str,
    ) -> Result<bool, StorageError> {
        let now = Self::now_ms();
        let cutoff = now - STUCK_CUTOFF_MS;
        let won = self.repo.try_claim(id, cutoff, now)?;
        if won {
            self.emit(session_id, worker_id, EventType::WorkerClaimed, Severity::Info)
                .await;
        } else {
            self.emit(
                session_id,
                worker_id,
                EventType::WorkerClaimFailed,
                Severity::Warning,
            )
            .await;
        }
        Ok(won)
    }

    /// Record a heartbeat against the queue row, advancing continuation / no-progress
    /// counters. A completed heartbeat atomically finalizes the row and emits the matching
    /// lifecycle event. Returns `true` if a row was updated.
    pub async fn record_heartbeat(
        &self,
        session_id: &str,
        worker_id: &str,
        status: &str,
    ) -> Result<bool, StorageError> {
        let now = Self::now_ms();
        let updated = self
            .repo
            .record_heartbeat(session_id, worker_id, status, now)?;
        if updated && status == "completed" {
            self.emit(
                session_id,
                worker_id,
                EventType::WorkerFinalized,
                Severity::Info,
            )
            .await;
        }
        Ok(updated)
    }

    /// Maintenance: flip stale `running` rows back to `queued`. Emits `WorkerReclaimed`
    /// per reclaimed row.
    pub async fn reclaim_stuck(&self) -> Result<Vec<String>, StorageError> {
        let now = Self::now_ms();
        let cutoff = now - STUCK_CUTOFF_MS;
        let ids = self.repo.reclaim_stuck(cutoff, now)?;
        for id in &ids {
            self.emit_for_row(id, EventType::WorkerReclaimed, Severity::Warning)
                .await;
        }
        Ok(ids)
    }

    /// Maintenance: finalize runs over the continuation / no-progress budgets. Emits
    /// `WorkerFinalized` per finalized row.
    pub async fn finalize_no_progress(&self) -> Result<Vec<String>, StorageError> {
        let now = Self::now_ms();
        let ids = self
            .repo
            .finalize_no_progress(MAX_CONTINUATIONS, MAX_NO_PROGRESS_CONTINUATIONS, now)?;
        for id in &ids {
            self.emit_for_row(id, EventType::WorkerFinalized, Severity::Info)
                .await;
        }
        Ok(ids)
    }

    /// Run both maintenance passes (called from the 30s background task).
    pub async fn run_maintenance(&self) -> Result<(), StorageError> {
        self.reclaim_stuck().await?;
        self.finalize_no_progress().await?;
        Ok(())
    }

    /// Repair orphaned `running` rows on resume: any `running` row whose worker is NOT in
    /// `live_worker_ids` (its PTY did not survive the crash) is flipped back to `queued`.
    /// Returns the reclaimed ids.
    pub async fn reconcile(
        &self,
        session_id: &str,
        live_worker_ids: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let now = Self::now_ms();
        let rows = self.repo.rows_for_session(session_id)?;
        let mut reclaimed = Vec::new();
        for row in rows {
            let orphaned = row.status == QueueStatus::Running
                && !live_worker_ids.iter().any(|w| w == &row.worker_id);
            if orphaned && self.repo.requeue_running(&row.id, now)? {
                reclaimed.push(row.id.clone());
                self.emit(session_id, &row.worker_id, EventType::WorkerReclaimed, Severity::Warning)
                    .await;
            }
        }
        Ok(reclaimed)
    }

    /// Counts + rows for a session's queue (dashboard endpoint).
    pub fn queue_snapshot(&self, session_id: &str) -> Result<QueueSnapshot, StorageError> {
        self.repo.snapshot(session_id)
    }

    /// Borrow the underlying repo (tests / resume reconcile lookups).
    pub fn repo(&self) -> &Arc<QueueRepo> {
        &self.repo
    }

    /// Publish a queue lifecycle event AFTER the DB commit.
    async fn emit(
        &self,
        session_id: &str,
        worker_id: &str,
        event_type: EventType,
        severity: Severity,
    ) {
        let event = Event {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            cell_id: None,
            agent_id: Some(worker_id.to_string()),
            event_type,
            timestamp: Utc::now(),
            payload: serde_json::json!({ "worker_id": worker_id }),
            severity,
        };
        if let Err(e) = self.event_bus.publish(event).await {
            tracing::warn!("Failed to publish queue event: {e}");
        }
    }

    /// Emit using the session/worker resolved from a row id (used by maintenance passes,
    /// which only have the id).
    async fn emit_for_row(&self, id: &str, event_type: EventType, severity: Severity) {
        match self.repo.get_row(id) {
            Ok(Some(row)) => {
                self.emit(&row.session_id, &row.worker_id, event_type, severity)
                    .await
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("Failed to load queue row {id} for event: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::application_state::ApplicationStateDb;
    use serde_json::json;
    use tempfile::TempDir;

    fn manager() -> (TempDir, QueueManager) {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(ApplicationStateDb::open(dir.path()).unwrap());
        let repo = Arc::new(QueueRepo::new(db));
        repo.ensure_schema().unwrap();
        let event_bus = EventBus::new(dir.path().to_path_buf());
        (dir, QueueManager::new(repo, event_bus))
    }

    #[tokio::test]
    async fn test_enqueue_then_claim_lifecycle() {
        let (_dir, mgr) = manager();
        mgr.enqueue_worker(
            "s1-worker-1",
            "s1",
            "s1-worker-1",
            "backend",
            "codex",
            json!({ "model": "gpt-5.5" }),
            None,
        )
        .await
        .unwrap();

        let snap = mgr.queue_snapshot("s1").unwrap();
        assert_eq!(snap.queued, 1);

        // First claim wins, second loses (already running, fresh).
        assert!(mgr.claim_and_spawn("s1-worker-1", "s1", "s1-worker-1").await.unwrap());
        assert!(!mgr.claim_and_spawn("s1-worker-1", "s1", "s1-worker-1").await.unwrap());

        let snap = mgr.queue_snapshot("s1").unwrap();
        assert_eq!(snap.running, 1);
        assert_eq!(snap.queued, 0);
    }

    #[tokio::test]
    async fn test_queue_events_emitted() {
        let (_dir, mgr) = manager();
        // Subscribe BEFORE the operations so we capture every event.
        let mut rx = mgr.event_bus.subscribe();

        mgr.enqueue_worker("r1", "s1", "s1-worker-1", "backend", "codex", json!({}), None)
            .await
            .unwrap();
        mgr.claim_and_spawn("r1", "s1", "s1-worker-1").await.unwrap();

        let e1 = rx.recv().await.unwrap();
        assert_eq!(e1.event_type, EventType::WorkerQueued);
        assert_eq!(e1.session_id, "s1");
        assert_eq!(e1.agent_id.as_deref(), Some("s1-worker-1"));
        let e2 = rx.recv().await.unwrap();
        assert_eq!(e2.event_type, EventType::WorkerClaimed);

        // A lost claim emits WorkerClaimFailed.
        mgr.claim_and_spawn("r1", "s1", "s1-worker-1").await.unwrap();
        let e3 = rx.recv().await.unwrap();
        assert_eq!(e3.event_type, EventType::WorkerClaimFailed);
    }

    #[tokio::test]
    async fn test_reconcile_repairs_orphaned_running() {
        let (_dir, mgr) = manager();
        mgr.enqueue_worker("r1", "s1", "s1-worker-1", "backend", "codex", json!({}), None)
            .await
            .unwrap();
        mgr.claim_and_spawn("r1", "s1", "s1-worker-1").await.unwrap();
        // After a crash there is no live PTY for s1-worker-1 → reconcile requeues it.
        let reclaimed = mgr.reconcile("s1", &[]).await.unwrap();
        assert_eq!(reclaimed, vec!["r1".to_string()]);
        assert_eq!(mgr.queue_snapshot("s1").unwrap().queued, 1);

        // If the worker is still live, reconcile leaves it running.
        mgr.claim_and_spawn("r1", "s1", "s1-worker-1").await.unwrap();
        let reclaimed = mgr.reconcile("s1", &["s1-worker-1".to_string()]).await.unwrap();
        assert!(reclaimed.is_empty());
        assert_eq!(mgr.queue_snapshot("s1").unwrap().running, 1);
    }
}
