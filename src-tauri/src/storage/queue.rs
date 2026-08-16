//! SQLite-backed durable sub-agent run queue (#126).
//!
//! Built on top of #124's [`ApplicationStateDb`] (the single shared `application_state.db`).
//! This module owns one additive table, `agent_run_queue`, created via an idempotent
//! [`ensure_schema`] (`CREATE TABLE IF NOT EXISTS` + guarded additive migrations + indexes)
//! called once at startup.
//!
//! # The atomic-claim invariant
//!
//! [`QueueRepo::try_claim`] is a SINGLE `UPDATE ... WHERE` statement executed under the
//! shared connection mutex. SQLite executes the statement atomically, and the process-wide
//! `parking_lot::Mutex<Connection>` serializes concurrent callers, so two claimers can never
//! both see a `queued` row — exactly one observes `conn.changes() == 1` and wins; the loser
//! observes `0` and is turned away (HTTP 409). The same `WHERE` clause also treats a
//! `running` row with a stale (or absent) heartbeat as claimable, which is the
//! reclaim-after-cutoff guarantee folded into the same statement. Dependency readiness is
//! part of that SAME `UPDATE`: a correlated `NOT EXISTS` rejects a row until every recorded
//! prerequisite has a finalized queue row. There is deliberately no check-then-claim pre-read.
//!
//! The queue table is the SOURCE OF TRUTH for sub-agent runs; the in-memory
//! `Session.agents` Vec is a UI cache that is reconciled against this table on resume.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::application_state::ApplicationStateDb;
use super::StorageError;

/// Lifecycle status of a queued sub-agent run.
///
/// `snake_case` on the wire so the frontend store can normalize it the same way it
/// normalizes the other serde enums (mirrors `domain/status.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    Queued,
    Running,
    Finalized,
    Failed,
    Blocked,
}

impl QueueStatus {
    /// Stable lowercase tag stored in the `status` TEXT column.
    pub fn as_tag(self) -> &'static str {
        match self {
            QueueStatus::Queued => "queued",
            QueueStatus::Running => "running",
            QueueStatus::Finalized => "finalized",
            QueueStatus::Failed => "failed",
            QueueStatus::Blocked => "blocked",
        }
    }

    /// Parse a stored tag back to a [`QueueStatus`]. Unknown tags fall back to `Queued`.
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "running" => QueueStatus::Running,
            "finalized" => QueueStatus::Finalized,
            "failed" => QueueStatus::Failed,
            "blocked" => QueueStatus::Blocked,
            _ => QueueStatus::Queued,
        }
    }
}

/// One row of the `agent_run_queue` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueueRow {
    pub id: String,
    pub task_id: Option<String>,
    pub session_id: String,
    pub worker_id: String,
    pub role_type: String,
    pub cli: String,
    pub status: QueueStatus,
    /// Full spawn context (worktree_path, prompt_file, wsl-converted path, model,
    /// parent_id) so a claim at a later time has everything it needs — addresses the
    /// stale-path risk.
    pub payload: serde_json::Value,
    pub attempts: i64,
    pub continuation_count: i64,
    pub no_progress_count: i64,
    pub last_status: Option<String>,
    /// Unix epoch millis of the last heartbeat, or `None` if never heartbeated.
    pub heartbeat_at: Option<i64>,
    /// Session-local monotonic identity for the current assignment of this row.
    ///
    /// `0` is the honest sentinel for rows migrated from a schema that did not record
    /// assignment order. Claims/rebinds and assignment-invalidating releases mint a positive
    /// value; heartbeats preserve it so callers can address the same assignment after the row
    /// reaches `finalized`.
    #[serde(default)]
    pub assignment_id: i64,
    /// Human-readable reason recorded when dependency failure/cancellation blocks this row.
    pub blocked_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Snapshot of a session's queue for the dashboard endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueueSnapshot {
    pub queued: usize,
    pub running: usize,
    pub finalized: usize,
    pub failed: usize,
    pub blocked: usize,
    /// Explicit task bindings that could not be resolved against an authoritative DAG.
    pub resolution_incomplete: Vec<QueueResolutionIssue>,
    /// Latest persisted codegraph conflict coverage for this session.
    pub conflict_coverage: Option<QueueConflictCoverage>,
    pub rows: Vec<QueueRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueResolutionIssue {
    pub queue_id: String,
    pub task_id: String,
    pub reason: String,
}

/// How an enqueue transaction reconciles a task's persisted resolution record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueResolutionUpdate {
    /// Legacy/no-graph/degraded/edgeless input does not claim to resolve prior uncertainty.
    Preserve,
    /// The explicit task is present in the authoritative graph.
    Resolved,
    /// The graph is dependency-constrained but does not contain the explicit task.
    ResolutionIncomplete { task_id: String, reason: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueConflictAction {
    Serialize,
    WorktreeIsolate,
}

impl QueueConflictAction {
    fn as_tag(self) -> &'static str {
        match self {
            Self::Serialize => "serialize",
            Self::WorktreeIsolate => "worktree_isolate",
        }
    }
}

/// One directed materialized conflict. Known pairs are stored in both directions so either
/// task's claim can correlate against a running peer without normalizing IDs at claim time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueConflictRow {
    pub session_id: String,
    pub task_id: String,
    pub conflicting_task_id: String,
    pub action: QueueConflictAction,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueConflictCoverage {
    pub state: String,
    pub unresolved_task_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueConflictWait {
    pub task_id: String,
    pub reason: String,
}

/// Result of atomically ending a still-owned claim after worker spawning failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnFailureRelease {
    Requeued { failures: i64 },
    Exhausted { failures: i64 },
    NotHeld,
}

/// Create the `agent_run_queue` table (+ indexes) if absent.
///
/// Additive-only and idempotent: safe to call at every startup. The fresh DDL is followed by
/// guarded `ALTER TABLE ... ADD COLUMN` migrations, leaving #124's `schema_meta` version table
/// untouched.
pub fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_run_queue (
            id                 TEXT PRIMARY KEY,
            task_id            TEXT,
            session_id         TEXT NOT NULL,
            worker_id          TEXT NOT NULL,
            spawned_worker_id  TEXT,
            role_type          TEXT NOT NULL,
            cli                TEXT NOT NULL,
            status             TEXT NOT NULL,
            payload            TEXT NOT NULL,
            attempts           INTEGER NOT NULL DEFAULT 0,
            continuation_count INTEGER NOT NULL DEFAULT 0,
            no_progress_count  INTEGER NOT NULL DEFAULT 0,
            last_status        TEXT,
            heartbeat_at       INTEGER,
            assignment_id      INTEGER NOT NULL DEFAULT 0,
            created_at         INTEGER NOT NULL,
            updated_at         INTEGER NOT NULL
        )",
        [],
    )?;
    let has_assignment_id = {
        let mut stmt = conn.prepare("PRAGMA table_info(agent_run_queue)")?;
        let mut rows = stmt.query([])?;
        let mut found = false;
        while let Some(row) = rows.next()? {
            let column_name: String = row.get(1)?;
            if column_name == "assignment_id" {
                found = true;
                break;
            }
        }
        found
    };
    if !has_assignment_id {
        conn.execute(
            "ALTER TABLE agent_run_queue
             ADD COLUMN assignment_id INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    let has_spawned_worker_id = {
        let mut stmt = conn.prepare("PRAGMA table_info(agent_run_queue)")?;
        let mut rows = stmt.query([])?;
        let mut found = false;
        while let Some(row) = rows.next()? {
            let column_name: String = row.get(1)?;
            if column_name == "spawned_worker_id" {
                found = true;
                break;
            }
        }
        found
    };
    if !has_spawned_worker_id {
        conn.execute(
            "ALTER TABLE agent_run_queue
             ADD COLUMN spawned_worker_id TEXT",
            [],
        )?;
    }
    // A deployed row predates the immutable spawn identity. Its current worker binding is the
    // only honest value available; claims replace it with the actual spawned worker before any
    // sentinel rewrite can sever the completion path.
    conn.execute(
        "UPDATE agent_run_queue
         SET spawned_worker_id = worker_id
         WHERE spawned_worker_id IS NULL",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_run_queue_session_status
         ON agent_run_queue(session_id, status)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_run_queue_status_heartbeat
         ON agent_run_queue(status, heartbeat_at)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_run_queue_session_task_status
         ON agent_run_queue(session_id, task_id, status)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_run_queue_worker_assignment
         ON agent_run_queue(session_id, worker_id, assignment_id DESC)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_run_queue_spawned_worker_assignment
         ON agent_run_queue(session_id, spawned_worker_id, assignment_id DESC)",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_run_queue_deps (
            session_id          TEXT NOT NULL,
            task_id             TEXT NOT NULL,
            prerequisite_task_id TEXT NOT NULL,
            PRIMARY KEY (session_id, task_id, prerequisite_task_id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_run_queue_deps_prerequisite
         ON agent_run_queue_deps(session_id, prerequisite_task_id)",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_run_queue_blocks (
            queue_id           TEXT PRIMARY KEY,
            blocked_by_task_id TEXT,
            reason             TEXT NOT NULL,
            created_at         INTEGER NOT NULL,
            FOREIGN KEY (queue_id) REFERENCES agent_run_queue(id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_run_queue_spawn_failures (
            queue_id TEXT PRIMARY KEY,
            failures INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (queue_id) REFERENCES agent_run_queue(id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_run_queue_resolution (
            queue_id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            reason TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (queue_id) REFERENCES agent_run_queue(id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_run_queue_conflicts (
            session_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            conflicting_task_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('serialize', 'worktree_isolate')),
            reason TEXT NOT NULL,
            PRIMARY KEY (session_id, task_id, conflicting_task_id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_run_queue_conflicts_peer
         ON agent_run_queue_conflicts(session_id, conflicting_task_id, action)",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_run_queue_conflict_coverage (
            session_id TEXT PRIMARY KEY,
            state TEXT NOT NULL CHECK (state IN ('disabled', 'complete', 'partial')),
            unresolved_task_ids TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        )",
        [],
    )?;
    Ok(())
}

/// Owns all SQL for the `agent_run_queue` table, backed by the shared [`ApplicationStateDb`].
///
/// Cheaply clonable (holds an `Arc`).
#[derive(Clone)]
pub struct QueueRepo {
    db: Arc<ApplicationStateDb>,
}

impl QueueRepo {
    /// Wrap a shared [`ApplicationStateDb`]. Call [`QueueRepo::ensure_schema`] once at
    /// startup before first use.
    pub fn new(db: Arc<ApplicationStateDb>) -> Self {
        Self { db }
    }

    /// Run [`ensure_schema`] against the shared connection (idempotent startup step).
    pub fn ensure_schema(&self) -> Result<(), StorageError> {
        self.db.with_conn(ensure_schema)
    }

    /// Insert a freshly-queued run. Idempotent on the primary key `id`: re-enqueuing the
    /// same logical run is a no-op (the existing row — possibly already `running` — is
    /// preserved), so a duplicate POST does not resurrect a claimed row.
    pub fn enqueue(&self, row: &QueueRow) -> Result<(), StorageError> {
        self.enqueue_with_dependencies(row, &[])
    }

    /// Atomically insert a run and the authoritative incoming dependencies for its task.
    ///
    /// The transaction is load-bearing: exposing the queue row before its dependency rows
    /// would create a window in which the readiness subquery sees zero prerequisites and a
    /// concurrent claimer can start dependent work early. Dependencies are recorded only when
    /// this call inserted the queue row; an idempotent duplicate cannot rewrite the graph under
    /// an already-running claim.
    pub fn enqueue_with_dependencies(
        &self,
        row: &QueueRow,
        prerequisite_task_ids: &[String],
    ) -> Result<(), StorageError> {
        let payload_text = serde_json::to_string(&row.payload)?;
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let inserted = tx.execute(
                "INSERT INTO agent_run_queue
                    (id, task_id, session_id, worker_id, spawned_worker_id, role_type, cli, status, payload,
                     attempts, continuation_count, no_progress_count, last_status,
                     heartbeat_at, assignment_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(id) DO NOTHING",
                params![
                    row.id,
                    row.task_id,
                    row.session_id,
                    row.worker_id,
                    row.role_type,
                    row.cli,
                    row.status.as_tag(),
                    payload_text,
                    row.attempts,
                    row.continuation_count,
                    row.no_progress_count,
                    row.last_status,
                    row.heartbeat_at,
                    row.assignment_id,
                    row.created_at,
                    row.updated_at,
                ],
            )?;
            if inserted == 1 {
                if let Some(task_id) = row.task_id.as_deref() {
                    for prerequisite_task_id in prerequisite_task_ids {
                        tx.execute(
                            "INSERT INTO agent_run_queue_deps
                                (session_id, task_id, prerequisite_task_id)
                             VALUES (?1, ?2, ?3)
                             ON CONFLICT(session_id, task_id, prerequisite_task_id) DO NOTHING",
                            params![row.session_id, task_id, prerequisite_task_id],
                        )?;
                    }
                }
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Atomically insert/reconcile a queue intent with dependencies, task resolution, and
    /// known conflict decisions.
    ///
    /// A newly visible row can never temporarily lack its scheduling facts. An idempotent
    /// duplicate may reconcile facts only while the exact same `(session_id, task_id)` row is
    /// still queued; a stale queue ID with another/null binding is left untouched because a
    /// wrong graph binding is more dangerous than an explicit omission.
    pub fn enqueue_with_scheduling(
        &self,
        row: &QueueRow,
        prerequisite_task_ids: &[String],
        resolution: &QueueResolutionUpdate,
        conflicts: &[QueueConflictRow],
        conflict_coverage: Option<&QueueConflictCoverage>,
        reconcile_conflict_task_id: Option<&str>,
    ) -> Result<(), StorageError> {
        if conflicts.iter().any(|conflict| {
            conflict.session_id != row.session_id || conflict.task_id == conflict.conflicting_task_id
        }) {
            return Err(StorageError::Database(rusqlite::Error::InvalidParameterName(
                "conflict rows must be same-session pairs of distinct task ids".to_string(),
            )));
        }
        if conflict_coverage.is_some_and(|coverage| {
            !matches!(coverage.state.as_str(), "disabled" | "complete" | "partial")
        }) {
            return Err(StorageError::Database(rusqlite::Error::InvalidParameterName(
                "unknown conflict coverage state".to_string(),
            )));
        }
        if let QueueResolutionUpdate::ResolutionIncomplete { task_id, .. } = resolution {
            if row.task_id.as_deref() != Some(task_id.as_str()) {
                return Err(StorageError::Database(rusqlite::Error::InvalidParameterName(
                    "resolution task id must equal the queue row binding".to_string(),
                )));
            }
        }
        if reconcile_conflict_task_id.is_some_and(|task_id| row.task_id.as_deref() != Some(task_id)) {
            return Err(StorageError::Database(rusqlite::Error::InvalidParameterName(
                "conflict reconciliation task id must equal the queue row binding".to_string(),
            )));
        }
        let payload_text = serde_json::to_string(&row.payload)?;
        let unresolved_json = conflict_coverage
            .map(|coverage| serde_json::to_string(&coverage.unresolved_task_ids))
            .transpose()?;
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let inserted = tx.execute(
                "INSERT INTO agent_run_queue
                    (id, task_id, session_id, worker_id, spawned_worker_id, role_type, cli, status, payload,
                     attempts, continuation_count, no_progress_count, last_status,
                     heartbeat_at, assignment_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(id) DO NOTHING",
                params![
                    row.id,
                    row.task_id,
                    row.session_id,
                    row.worker_id,
                    row.role_type,
                    row.cli,
                    row.status.as_tag(),
                    payload_text,
                    row.attempts,
                    row.continuation_count,
                    row.no_progress_count,
                    row.last_status,
                    row.heartbeat_at,
                    row.assignment_id,
                    row.created_at,
                    row.updated_at,
                ],
            )?;
            let exact_queued_identity = if inserted == 1 {
                true
            } else {
                tx.query_row(
                    "SELECT session_id, task_id, status FROM agent_run_queue WHERE id = ?1",
                    params![row.id],
                    |existing| {
                        let session_id: String = existing.get(0)?;
                        let task_id: Option<String> = existing.get(1)?;
                        let status: String = existing.get(2)?;
                        Ok(session_id == row.session_id
                            && task_id == row.task_id
                            && status == "queued")
                    },
                )
                .optional()?
                .unwrap_or(false)
            };
            if !exact_queued_identity {
                tx.commit()?;
                return Ok(());
            }

            match resolution {
                QueueResolutionUpdate::Preserve => {
                    if inserted == 1 {
                        insert_dependencies(&tx, row, prerequisite_task_ids)?;
                    }
                }
                QueueResolutionUpdate::Resolved => {
                    tx.execute(
                        "DELETE FROM agent_run_queue_resolution WHERE queue_id = ?1",
                        params![row.id],
                    )?;
                    replace_dependencies(&tx, row, prerequisite_task_ids)?;
                }
                QueueResolutionUpdate::ResolutionIncomplete { task_id, reason } => {
                    if let Some(bound_task_id) = row.task_id.as_deref() {
                        tx.execute(
                            "DELETE FROM agent_run_queue_deps
                             WHERE session_id = ?1 AND task_id = ?2",
                            params![row.session_id, bound_task_id],
                        )?;
                    }
                    tx.execute(
                        "INSERT INTO agent_run_queue_resolution
                            (queue_id, task_id, reason, created_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(queue_id) DO UPDATE SET
                            task_id = excluded.task_id,
                            reason = excluded.reason",
                        params![row.id, task_id, reason, row.updated_at],
                    )?;
                }
            }

            if let Some(task_id) = reconcile_conflict_task_id.filter(|_| {
                conflict_coverage.is_some_and(|coverage| coverage.state == "complete")
            }) {
                // Replace only the active exact task's incident pairs. Deleting the whole
                // session report from a pre-transaction projection would race a peer becoming
                // ready. Partial/disabled coverage cannot prove an omitted decision is stale,
                // so it must preserve every prior conservative serialize pair.
                tx.execute(
                    "DELETE FROM agent_run_queue_conflicts
                     WHERE session_id = ?1
                       AND (task_id = ?2 OR conflicting_task_id = ?2)",
                    params![row.session_id, task_id],
                )?;
            }
            for conflict in conflicts {
                tx.execute(
                    "INSERT INTO agent_run_queue_conflicts
                        (session_id, task_id, conflicting_task_id, action, reason)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(session_id, task_id, conflicting_task_id) DO UPDATE SET
                        action = excluded.action,
                        reason = excluded.reason",
                    params![
                        conflict.session_id,
                        conflict.task_id,
                        conflict.conflicting_task_id,
                        conflict.action.as_tag(),
                        conflict.reason,
                    ],
                )?;
            }
            if let (Some(coverage), Some(unresolved_json)) =
                (conflict_coverage, unresolved_json.as_deref())
            {
                tx.execute(
                    "INSERT INTO agent_run_queue_conflict_coverage
                        (session_id, state, unresolved_task_ids, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(session_id) DO UPDATE SET
                        state = excluded.state,
                        unresolved_task_ids = excluded.unresolved_task_ids,
                        updated_at = excluded.updated_at",
                    params![row.session_id, coverage.state, unresolved_json, row.updated_at],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// THE atomic claim. A single `UPDATE ... WHERE` flips a claimable row to `running`.
    ///
    /// A row is claimable when it is `queued`, OR it is `running` but its heartbeat is
    /// stale (older than `stuck_cutoff_ms`) or absent, its explicit task binding is resolved,
    /// every dependency recorded for its task has at least one finalized queue row in the same
    /// session, and no materialized `serialize` peer is running. Returns `true` iff THIS call
    /// won the claim (`conn.changes() == 1`). Resolution, readiness, and conflicts are
    /// correlated `NOT EXISTS` predicates inside this same atomic statement; none is authorized
    /// by a check-then-claim pre-read.
    ///
    /// The claim STAMPS `heartbeat_at = now_ms` on the won row. This is load-bearing for
    /// the no-double-spawn invariant: without it, a just-claimed row keeps `heartbeat_at`
    /// NULL (every `enqueue` inserts NULL), and a second concurrent claimer would re-match
    /// the `status='running' AND heartbeat_at IS NULL` branch and ALSO win — a double spawn.
    /// Stamping the heartbeat makes the freshly-claimed row fresh, so the loser sees a
    /// non-NULL, non-stale heartbeat and matches 0 rows. The worker's own heartbeats then
    /// keep the row fresh; if its first heartbeat never arrives within `stuck_cutoff_ms`,
    /// the row legitimately becomes reclaimable again (the stuck-detection path).
    /// Returns the claimed row's `attempts` counter on a win — the fencing epoch
    /// for [`Self::requeue_claimed`] / [`Self::fail_claimed`] — or `None` if the
    /// claim was lost.
    pub fn try_claim(
        &self,
        id: &str,
        stuck_cutoff_ms: i64,
        now_ms: i64,
    ) -> Result<Option<i64>, StorageError> {
        self.try_claim_for_worker(id, None, stuck_cutoff_ms, now_ms)
    }

    /// Claim a ready row and atomically bind it to the worker slot that will spawn it.
    ///
    /// Task-backed queue IDs are stable across retries, while worker indexes are allocated
    /// from the live roster at retry time. Rebinding in this SAME UPDATE prevents a task that
    /// waited behind dependencies from retaining another task's former worker slot. The
    /// worker-slot `NOT EXISTS` is also inside the statement, so two distinct ready tasks that
    /// concurrently reserve the same roster index cannot both become `running` under that ID.
    pub fn try_claim_for_worker(
        &self,
        id: &str,
        worker_id: Option<&str>,
        stuck_cutoff_ms: i64,
        now_ms: i64,
    ) -> Result<Option<i64>, StorageError> {
        self.try_claim_for_worker_with_grace(
            id,
            worker_id,
            true,
            None,
            stuck_cutoff_ms,
            stuck_cutoff_ms,
            now_ms,
        )
    }

    /// Claim with separate first-heartbeat and steady-state stale cutoffs.
    ///
    /// `allow_running_reclaim` and `expected_running_worker_id` carry the coordination
    /// layer's process-liveness decision into this atomic statement. The exact worker fence
    /// prevents a stale probe result from reclaiming a row that changed hands after it was
    /// observed. Queued claims do not depend on either value.
    #[allow(clippy::too_many_arguments)]
    pub fn try_claim_for_worker_with_grace(
        &self,
        id: &str,
        worker_id: Option<&str>,
        allow_running_reclaim: bool,
        expected_running_worker_id: Option<&str>,
        stuck_cutoff_ms: i64,
        first_heartbeat_cutoff_ms: i64,
        now_ms: i64,
    ) -> Result<Option<i64>, StorageError> {
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_run_queue
                 SET status = 'running',
                     attempts = attempts + 1,
                     updated_at = ?2,
                     heartbeat_at = ?2,
                     worker_id = COALESCE(?4, worker_id),
                     spawned_worker_id = COALESCE(?4, spawned_worker_id, worker_id),
                     assignment_id = (
                         SELECT COALESCE(MAX(assignments.assignment_id), 0) + 1
                         FROM agent_run_queue AS assignments
                         WHERE assignments.session_id = agent_run_queue.session_id
                     )
                 WHERE id = ?1
                   AND (status = 'queued'
                        OR (status = 'running'
                            AND ?5
                            AND (?6 IS NULL OR worker_id = ?6)
                            AND ((last_status IS NULL
                                  AND (heartbeat_at IS NULL OR heartbeat_at < ?7))
                                 OR (last_status IS NOT NULL
                                     AND (heartbeat_at IS NULL OR heartbeat_at < ?3)))))
                   AND (?4 IS NULL OR NOT EXISTS (
                       SELECT 1
                       FROM agent_run_queue AS active_worker
                       WHERE active_worker.session_id = agent_run_queue.session_id
                         AND active_worker.worker_id = ?4
                         AND active_worker.status = 'running'
                         AND active_worker.id <> agent_run_queue.id
                   ))
                   AND NOT EXISTS (
                       SELECT 1
                       FROM agent_run_queue_resolution AS unresolved
                       WHERE unresolved.queue_id = agent_run_queue.id
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM agent_run_queue_deps AS dependency
                       WHERE dependency.session_id = agent_run_queue.session_id
                         AND dependency.task_id = agent_run_queue.task_id
                         AND NOT EXISTS (
                             SELECT 1
                             FROM agent_run_queue AS prerequisite
                             WHERE prerequisite.session_id = dependency.session_id
                               AND prerequisite.task_id = dependency.prerequisite_task_id
                               AND prerequisite.status = 'finalized'
                         )
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM agent_run_queue_conflicts AS conflict
                       JOIN agent_run_queue AS conflicting_run
                         ON conflicting_run.session_id = conflict.session_id
                        AND conflicting_run.task_id = conflict.conflicting_task_id
                        AND conflicting_run.status = 'running'
                       WHERE conflict.session_id = agent_run_queue.session_id
                         AND conflict.task_id = agent_run_queue.task_id
                         AND conflict.action = 'serialize'
                   )",
                params![
                    id,
                    now_ms,
                    stuck_cutoff_ms,
                    worker_id,
                    allow_running_reclaim,
                    expected_running_worker_id,
                    first_heartbeat_cutoff_ms,
                ],
            )?;
            if conn.changes() != 1 {
                return Ok(None);
            }
            let attempts: i64 = conn.query_row(
                "SELECT attempts FROM agent_run_queue WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )?;
            Ok(Some(attempts))
        })
    }

    /// Explain a lost claim with a best-effort snapshot of unfinished prerequisite IDs.
    ///
    /// This MUST be called only after [`Self::try_claim`]'s atomic UPDATE returned `None`.
    /// The losing UPDATE is authoritative; this post-loss read is diagnostic only and never
    /// feeds back into a claim decision. The returned IDs are advisory and may already have
    /// finalized by the time the caller reports them.
    pub fn pending_dependencies(&self, id: &str) -> Result<Vec<String>, StorageError> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT dependency.prerequisite_task_id
                 FROM agent_run_queue AS candidate
                 JOIN agent_run_queue_deps AS dependency
                   ON dependency.session_id = candidate.session_id
                  AND dependency.task_id = candidate.task_id
                 WHERE candidate.id = ?1
                   AND NOT EXISTS (
                       SELECT 1
                       FROM agent_run_queue AS prerequisite
                       WHERE prerequisite.session_id = dependency.session_id
                         AND prerequisite.task_id = dependency.prerequisite_task_id
                         AND prerequisite.status = 'finalized'
                   )
                 ORDER BY dependency.prerequisite_task_id",
            )?;
            let pending = stmt
                .query_map(params![id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(pending)
        })
    }

    /// Explain an atomic claim loss caused by an unresolved explicit graph binding.
    /// Diagnostic only: callers must invoke this after `try_claim_for_worker` returned `None`.
    pub fn resolution_issue(&self, id: &str) -> Result<Option<QueueResolutionIssue>, StorageError> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT queue_id, task_id, reason
                 FROM agent_run_queue_resolution
                 WHERE queue_id = ?1",
                params![id],
                |row| {
                    Ok(QueueResolutionIssue {
                        queue_id: row.get(0)?,
                        task_id: row.get(1)?,
                        reason: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// Explain an atomic claim loss caused by currently running serialized peers.
    /// Advisory by construction: peer status may change immediately after this read.
    pub fn pending_conflicts(&self, id: &str) -> Result<Vec<QueueConflictWait>, StorageError> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT conflict.conflicting_task_id, conflict.reason
                 FROM agent_run_queue AS candidate
                 JOIN agent_run_queue_conflicts AS conflict
                   ON conflict.session_id = candidate.session_id
                  AND conflict.task_id = candidate.task_id
                  AND conflict.action = 'serialize'
                 WHERE candidate.id = ?1
                   AND EXISTS (
                       SELECT 1
                       FROM agent_run_queue AS conflicting_run
                       WHERE conflicting_run.session_id = conflict.session_id
                         AND conflicting_run.task_id = conflict.conflicting_task_id
                         AND conflicting_run.status = 'running'
                   )
                 ORDER BY conflict.conflicting_task_id",
            )?;
            let waits = stmt
                .query_map(params![id], |row| {
                    Ok(QueueConflictWait {
                        task_id: row.get(0)?,
                        reason: row.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(waits)
        })
    }

    /// Refresh the heartbeat for exactly one still-owned spawn claim.
    ///
    /// This is the durable half of the in-process spawn reservation. All identity fields are
    /// fences: a stale epoch or a worker slot rebound by a newer claim must update zero rows.
    /// The method never changes status or attempts, so it cannot acquire or resurrect a claim.
    pub fn refresh_claimed_spawn(
        &self,
        id: &str,
        expected_attempts: i64,
        worker_id: &str,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_run_queue
                 SET heartbeat_at = ?4, updated_at = ?4
                 WHERE id = ?1
                   AND status = 'running'
                   AND attempts = ?2
                   AND worker_id = ?3",
                params![id, expected_attempts, worker_id, now_ms],
            )?;
            Ok(conn.changes() == 1)
        })
    }

    /// Fenced inverse of a claim: return a row we still own to `queued`.
    ///
    /// The `attempts = ?2` fence is required, not defensive. `status = 'running'`
    /// alone is satisfied by ANY holder's claim and the schema has no owner
    /// column, so this ABA is reachable: A wins (attempts=1) -> A's worktree
    /// creation runs long -> the 30s maintenance pass reclaims the row past the
    /// stuck cutoff -> a retry claims it (attempts=2) and spawns for real -> A
    /// finally fails and releases, requeueing B's LIVE claim -> a third claimer
    /// double-spawns. `attempts` is incremented exactly once per won claim, so it
    /// is a free, already-persisted epoch.
    pub fn requeue_claimed(
        &self,
        id: &str,
        expected_attempts: i64,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        self.db.with_conn(|conn| {
            let assignment_id = next_assignment_id_for_row(conn, id)?;
            conn.execute(
                "UPDATE agent_run_queue
                 SET status = 'queued',
                     worker_id = CASE
                         WHEN task_id IS NOT NULL THEN 'pending:' || id ELSE worker_id END,
                     updated_at = ?3,
                     assignment_id = ?4
                 WHERE id = ?1 AND status = 'running' AND attempts = ?2",
                params![id, expected_attempts, now_ms, assignment_id],
            )?;
            Ok(conn.changes() == 1)
        })
    }

    /// Fenced terminal failure, used once a run has burned its spawn budget.
    pub fn fail_claimed(
        &self,
        id: &str,
        expected_attempts: i64,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let assignment_id = next_assignment_id_for_row(&tx, id)?;
            let changed = tx.execute(
                "UPDATE agent_run_queue
                 SET status = 'failed', updated_at = ?3, assignment_id = ?4
                 WHERE id = ?1 AND status = 'running' AND attempts = ?2",
                params![id, expected_attempts, now_ms, assignment_id],
            )?;
            if changed == 1 {
                let root: Option<(String, Option<String>)> = tx
                    .query_row(
                        "SELECT session_id, task_id FROM agent_run_queue WHERE id = ?1",
                        params![id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if let Some((session_id, Some(task_id))) = root {
                    let reason = format!("dependency task {task_id} failed");
                    block_task_subtree(&tx, &session_id, &task_id, false, &reason, now_ms)?;
                }
            }
            tx.commit()?;
            Ok(changed == 1)
        })
    }

    /// Atomically release an exact spawn claim while advancing its independent failure budget.
    ///
    /// `attempts` remains a monotone fencing epoch for the lifetime of the queue row. Spawn
    /// failures therefore use a separate counter: resetting an epoch after manual recovery
    /// would make an old `(id, attempts, worker_id)` reservation valid again (ABA).
    pub fn release_failed_spawn(
        &self,
        id: &str,
        expected_attempts: i64,
        worker_id: &str,
        max_failures: i64,
        now_ms: i64,
    ) -> Result<SpawnFailureRelease, StorageError> {
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let held: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT session_id, task_id
                     FROM agent_run_queue
                     WHERE id = ?1
                       AND status = 'running'
                       AND attempts = ?2
                       AND worker_id = ?3",
                    params![id, expected_attempts, worker_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((session_id, task_id)) = held else {
                tx.commit()?;
                return Ok(SpawnFailureRelease::NotHeld);
            };

            tx.execute(
                "INSERT INTO agent_run_queue_spawn_failures (queue_id, failures)
                 VALUES (?1, 1)
                 ON CONFLICT(queue_id) DO UPDATE SET failures = failures + 1",
                params![id],
            )?;
            let failures: i64 = tx.query_row(
                "SELECT failures FROM agent_run_queue_spawn_failures WHERE queue_id = ?1",
                params![id],
                |row| row.get(0),
            )?;

            let (status, result) = if failures >= max_failures {
                ("failed", SpawnFailureRelease::Exhausted { failures })
            } else {
                ("queued", SpawnFailureRelease::Requeued { failures })
            };
            let assignment_id = next_assignment_id_for_row(&tx, id)?;
            let changed = tx.execute(
                "UPDATE agent_run_queue
                 SET status = ?4,
                     worker_id = CASE
                         WHEN ?4 = 'queued' AND task_id IS NOT NULL
                         THEN 'pending:' || id ELSE worker_id END,
                     updated_at = ?5,
                     assignment_id = ?6
                 WHERE id = ?1
                   AND status = 'running'
                   AND attempts = ?2
                   AND worker_id = ?3",
                params![id, expected_attempts, worker_id, status, now_ms, assignment_id],
            )?;
            if changed != 1 {
                tx.rollback()?;
                return Ok(SpawnFailureRelease::NotHeld);
            }

            if status == "failed" {
                if let Some(task_id) = task_id {
                    let reason = format!("dependency task {task_id} failed");
                    block_task_subtree(&tx, &session_id, &task_id, false, &reason, now_ms)?;
                }
            }
            tx.commit()?;
            Ok(result)
        })
    }

    /// Cancel one task and block every transitive dependent with a persisted reason.
    /// Finalized/failed rows remain terminal and are never rewritten.
    pub fn cancel_task_and_descendants(
        &self,
        session_id: &str,
        task_id: &str,
        reason: &str,
        now_ms: i64,
    ) -> Result<Vec<String>, StorageError> {
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let detail = format!("task {task_id} cancelled: {reason}");
            let blocked = block_task_subtree(
                &tx,
                session_id,
                task_id,
                true,
                &detail,
                now_ms,
            )?;
            tx.commit()?;
            Ok(blocked)
        })
    }

    /// Operator/agent recovery: force a stuck or failed row back to claimable and
    /// reset its spawn-failure budget. Unfenced by design — this is the documented
    /// exit from a row that no automatic path can recover.
    ///
    /// The claim `attempts` epoch is deliberately never reset. Once epoch 2 has
    /// existed, no recovery path may make a delayed epoch-1 operation valid again.
    pub fn release_claim_manual(&self, id: &str, now_ms: i64) -> Result<bool, StorageError> {
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let assignment_id = next_assignment_id_for_row(&tx, id)?;
            let changed = tx.execute(
                "UPDATE agent_run_queue
                 SET status = 'queued',
                     worker_id = CASE
                         WHEN task_id IS NOT NULL THEN 'pending:' || id ELSE worker_id END,
                     updated_at = ?2,
                     assignment_id = ?3
                 WHERE id = ?1 AND status IN ('running', 'failed')",
                params![id, now_ms, assignment_id],
            )?;
            if changed == 1 {
                tx.execute(
                    "DELETE FROM agent_run_queue_spawn_failures WHERE queue_id = ?1",
                    params![id],
                )?;
            }
            tx.commit()?;
            Ok(changed == 1)
        })
    }

    #[cfg(test)]
    pub(crate) fn spawn_failure_count(&self, id: &str) -> Result<i64, StorageError> {
        self.db.with_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT failures FROM agent_run_queue_spawn_failures WHERE queue_id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0))
        })
    }

    /// Record a heartbeat for an active or finalized row, advancing progress only while active.
    ///
    /// State machine: if the incoming `status` equals the stored `last_status`, the worker
    /// made no progress → `no_progress_count += 1`. If it changed (or there was no prior
    /// status), it is a continuation → `continuation_count += 1`, `no_progress_count = 0`,
    /// and `last_status` is updated. A `completed` heartbeat atomically moves a queued or
    /// running row to the existing terminal `finalized` state so stale-run maintenance cannot
    /// reclaim verified work. A finalized row keeps its lifecycle fields frozen while its
    /// liveness timestamps remain fresh. Failed and blocked rows are never overwritten. Returns
    /// `true` if an active or finalized row was updated.
    pub fn record_heartbeat(
        &self,
        session_id: &str,
        worker_id: &str,
        status: &str,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        Ok(self
            .record_heartbeat_for_assignment(session_id, worker_id, None, status, now_ms)?
            .is_some())
    }

    /// Record a heartbeat against exactly one assignment and return its queue-row id.
    ///
    /// `spawned_worker_id` is immutable for the assignment even when recovery rewrites the
    /// mutable worker slot to a `pending:` sentinel. A later claim replaces the spawn identity,
    /// so a superseded worker cannot resolve the row after a new worker starts.
    ///
    /// When `assignment_id` is supplied, a stale or mismatched identity updates nothing; it
    /// never falls back to another row sharing the worker slot. Legacy prompts omit it, so the
    /// server deterministically resolves the live row first, then the greatest durable
    /// assignment id. The final `id` tie-break applies only to migrated sentinel-0 history,
    /// whose real assignment order cannot be reconstructed without inventing data.
    pub fn record_heartbeat_for_assignment(
        &self,
        session_id: &str,
        worker_id: &str,
        assignment_id: Option<i64>,
        status: &str,
        now_ms: i64,
    ) -> Result<Option<String>, StorageError> {
        self.db.with_conn(|conn| {
            let target: Option<(String, i64)> = conn
                .query_row(
                    "SELECT id, assignment_id
                     FROM agent_run_queue
                     WHERE session_id = ?1
                       AND (worker_id = ?2 OR spawned_worker_id = ?2)
                       AND status IN ('queued', 'running', 'finalized')
                       AND (?3 IS NULL OR assignment_id = ?3)
                     ORDER BY CASE status WHEN 'running' THEN 0 ELSE 1 END,
                              assignment_id DESC,
                              CASE status WHEN 'queued' THEN 0 ELSE 1 END,
                              id DESC
                     LIMIT 1",
                    params![session_id, worker_id, assignment_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((id, resolved_assignment_id)) = target else {
                return Ok(None);
            };
            conn.execute(
                "UPDATE agent_run_queue
                 SET status = CASE
                         WHEN ?4 = 'completed' AND status IN ('queued', 'running')
                         THEN 'finalized' ELSE status END,
                     heartbeat_at = ?3,
                     updated_at = ?3,
                     no_progress_count = CASE
                         WHEN status IN ('queued', 'running')
                              AND last_status IS NOT NULL AND last_status = ?4
                         THEN no_progress_count + 1
                         WHEN status IN ('queued', 'running') THEN 0
                         ELSE no_progress_count END,
                     continuation_count = CASE
                         WHEN status IN ('queued', 'running')
                              AND (last_status IS NULL OR last_status <> ?4)
                         THEN continuation_count + 1 ELSE continuation_count END,
                     last_status = CASE
                         WHEN status IN ('queued', 'running') THEN ?4 ELSE last_status END
                 WHERE id = ?1
                   AND assignment_id = ?2
                   AND status IN ('queued', 'running', 'finalized')",
                params![id, resolved_assignment_id, now_ms, status],
            )?;
            Ok((conn.changes() == 1).then_some(id))
        })
    }

    /// Flip every `running` row whose heartbeat is stale (older than `cutoff`) or absent
    /// back to `queued`, making it claimable again. Returns the ids that were reclaimed.
    ///
    /// This does NOT kill the live PTY — it only marks the row claimable. A genuinely
    /// working-but-quiet worker keeps its `heartbeat_at` fresh and is therefore untouched.
    pub fn reclaim_stuck(
        &self,
        stuck_cutoff_ms: i64,
        now_ms: i64,
    ) -> Result<Vec<String>, StorageError> {
        self.reclaim_stuck_matching(
            stuck_cutoff_ms,
            stuck_cutoff_ms,
            now_ms,
            None,
        )
    }

    /// Reclaim only exact `(queue id, worker id)` pairs approved by the liveness layer, using
    /// a longer cutoff until the first worker-authored heartbeat records `last_status`.
    pub fn reclaim_stuck_with_grace(
        &self,
        stuck_cutoff_ms: i64,
        first_heartbeat_cutoff_ms: i64,
        now_ms: i64,
        reclaimable_workers: &[(String, String)],
    ) -> Result<Vec<String>, StorageError> {
        self.reclaim_stuck_matching(
            stuck_cutoff_ms,
            first_heartbeat_cutoff_ms,
            now_ms,
            Some(reclaimable_workers),
        )
    }

    fn reclaim_stuck_matching(
        &self,
        stuck_cutoff_ms: i64,
        first_heartbeat_cutoff_ms: i64,
        now_ms: i64,
        reclaimable_workers: Option<&[(String, String)]>,
    ) -> Result<Vec<String>, StorageError> {
        self.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let candidates: Vec<(String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT id, worker_id FROM agent_run_queue
                     WHERE status = 'running'
                       AND ((last_status IS NULL
                             AND (heartbeat_at IS NULL OR heartbeat_at < ?2))
                            OR (last_status IS NOT NULL
                                AND (heartbeat_at IS NULL OR heartbeat_at < ?1)))",
                )?;
                let rows = stmt
                    .query_map(
                        params![stuck_cutoff_ms, first_heartbeat_cutoff_ms],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let mut ids = Vec::new();
            for (id, worker_id) in candidates {
                if reclaimable_workers.is_some_and(|allowed| {
                    !allowed
                        .iter()
                        .any(|candidate| candidate.0 == id && candidate.1 == worker_id)
                }) {
                    continue;
                }
                let assignment_id = next_assignment_id_for_row(&tx, &id)?;
                tx.execute(
                    "UPDATE agent_run_queue
                     SET status = 'queued',
                         worker_id = CASE
                             WHEN task_id IS NOT NULL THEN 'pending:' || id ELSE worker_id END,
                         updated_at = ?3,
                         assignment_id = ?4
                     WHERE id = ?1
                       AND status = 'running'
                       AND worker_id = ?5
                       AND ((last_status IS NULL
                             AND (heartbeat_at IS NULL OR heartbeat_at < ?6))
                            OR (last_status IS NOT NULL
                                AND (heartbeat_at IS NULL OR heartbeat_at < ?2)))",
                    params![
                        id,
                        stuck_cutoff_ms,
                        now_ms,
                        assignment_id,
                        worker_id,
                        first_heartbeat_cutoff_ms,
                    ],
                )?;
                if tx.changes() == 1 {
                    ids.push(id);
                }
            }
            tx.commit()?;
            Ok(ids)
        })
    }

    /// Finalize non-terminal rows that have exceeded the continuation / no-progress
    /// budgets. Returns the ids that were finalized.
    pub fn finalize_no_progress(
        &self,
        max_continuations: i64,
        max_no_progress: i64,
        now_ms: i64,
    ) -> Result<Vec<String>, StorageError> {
        self.db.with_conn(|conn| {
            let ids: Vec<String> = {
                let mut stmt = conn.prepare(
                    "SELECT id FROM agent_run_queue
                     WHERE status IN ('queued', 'running')
                       AND (continuation_count >= ?1 OR no_progress_count >= ?2)",
                )?;
                let rows = stmt
                    .query_map(params![max_continuations, max_no_progress], |r| {
                        r.get::<_, String>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            if !ids.is_empty() {
                conn.execute(
                    "UPDATE agent_run_queue
                     SET status = 'finalized', updated_at = ?3
                     WHERE status IN ('queued', 'running')
                       AND (continuation_count >= ?1 OR no_progress_count >= ?2)",
                    params![max_continuations, max_no_progress, now_ms],
                )?;
            }
            Ok(ids)
        })
    }

    /// Flip a specific `running` row back to `queued` (used by resume reconcile when the
    /// PTY no longer exists). Returns `true` if a row changed.
    pub fn requeue_running(&self, id: &str, now_ms: i64) -> Result<bool, StorageError> {
        self.db.with_conn(|conn| {
            let assignment_id = next_assignment_id_for_row(conn, id)?;
            conn.execute(
                "UPDATE agent_run_queue
                 SET status = 'queued',
                     worker_id = CASE
                         WHEN task_id IS NOT NULL THEN 'pending:' || id ELSE worker_id END,
                     updated_at = ?2,
                     assignment_id = ?3
                 WHERE id = ?1 AND status = 'running'",
                params![id, now_ms, assignment_id],
            )?;
            Ok(conn.changes() == 1)
        })
    }

    /// Snapshot all running rows for coordination-layer liveness probing.
    pub fn running_rows(&self) -> Result<Vec<QueueRow>, StorageError> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT queue.id, queue.task_id, queue.session_id, queue.worker_id,
                        queue.role_type, queue.cli, queue.status, queue.payload,
                        queue.attempts, queue.continuation_count, queue.no_progress_count,
                        queue.last_status, queue.heartbeat_at, queue.assignment_id,
                        queue.created_at, queue.updated_at, block.reason
                 FROM agent_run_queue AS queue
                 LEFT JOIN agent_run_queue_blocks AS block ON block.queue_id = queue.id
                 WHERE queue.status = 'running'
                 ORDER BY queue.created_at, queue.id",
            )?;
            let rows = stmt
                .query_map([], row_to_queue_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// All rows for a session that are not terminal-removed, ordered by creation.
    pub fn rows_for_session(&self, session_id: &str) -> Result<Vec<QueueRow>, StorageError> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT queue.id, queue.task_id, queue.session_id, queue.worker_id,
                        queue.role_type, queue.cli, queue.status, queue.payload,
                        queue.attempts, queue.continuation_count, queue.no_progress_count,
                        queue.last_status, queue.heartbeat_at, queue.assignment_id,
                        queue.created_at, queue.updated_at, block.reason
                 FROM agent_run_queue AS queue
                 LEFT JOIN agent_run_queue_blocks AS block ON block.queue_id = queue.id
                 WHERE queue.session_id = ?1
                 ORDER BY queue.created_at, queue.id",
            )?;
            let rows = stmt
                .query_map(params![session_id], row_to_queue_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Look up a single row by primary key.
    pub fn get_row(&self, id: &str) -> Result<Option<QueueRow>, StorageError> {
        self.db.with_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT queue.id, queue.task_id, queue.session_id, queue.worker_id,
                            queue.role_type, queue.cli, queue.status, queue.payload,
                            queue.attempts, queue.continuation_count, queue.no_progress_count,
                            queue.last_status, queue.heartbeat_at, queue.assignment_id,
                            queue.created_at, queue.updated_at, block.reason
                     FROM agent_run_queue AS queue
                     LEFT JOIN agent_run_queue_blocks AS block ON block.queue_id = queue.id
                     WHERE queue.id = ?1",
                    params![id],
                    row_to_queue_row,
                )
                .optional()?;
            Ok(row)
        })
    }

    /// Build a [`QueueSnapshot`] (counts + rows) for a session.
    pub fn snapshot(&self, session_id: &str) -> Result<QueueSnapshot, StorageError> {
        let rows = self.rows_for_session(session_id)?;
        let (resolution_incomplete, conflict_coverage) = self.db.with_conn(|conn| {
            let mut resolution_stmt = conn.prepare(
                "SELECT resolution.queue_id, resolution.task_id, resolution.reason
                 FROM agent_run_queue_resolution AS resolution
                 JOIN agent_run_queue AS queue ON queue.id = resolution.queue_id
                 WHERE queue.session_id = ?1
                 ORDER BY resolution.task_id, resolution.queue_id",
            )?;
            let issues = resolution_stmt
                .query_map(params![session_id], |row| {
                    Ok(QueueResolutionIssue {
                        queue_id: row.get(0)?,
                        task_id: row.get(1)?,
                        reason: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(resolution_stmt);

            let coverage = conn
                .query_row(
                    "SELECT state, unresolved_task_ids
                     FROM agent_run_queue_conflict_coverage
                     WHERE session_id = ?1",
                    params![session_id],
                    |row| {
                        let state: String = row.get(0)?;
                        let unresolved_json: String = row.get(1)?;
                        let unresolved_task_ids = serde_json::from_str(&unresolved_json).map_err(
                            |error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    1,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            },
                        )?;
                        Ok(QueueConflictCoverage {
                            state,
                            unresolved_task_ids,
                        })
                    },
                )
                .optional()?;
            Ok((issues, coverage))
        })?;
        let mut queued = 0;
        let mut running = 0;
        let mut finalized = 0;
        let mut failed = 0;
        let mut blocked = 0;
        for r in &rows {
            match r.status {
                QueueStatus::Queued => queued += 1,
                QueueStatus::Running => running += 1,
                QueueStatus::Finalized => finalized += 1,
                QueueStatus::Failed => failed += 1,
                QueueStatus::Blocked => blocked += 1,
            }
        }
        Ok(QueueSnapshot {
            queued,
            running,
            finalized,
            failed,
            blocked,
            resolution_incomplete,
            conflict_coverage,
            rows,
        })
    }
}

/// Mint the next honest assignment identity within the target row's session.
///
/// The shared application-state connection is process-serialized, so reading the maximum and
/// consuming it in the caller's UPDATE cannot race another queue mutation. Gaps are harmless;
/// ordering and exact equality are the protocol properties.
fn next_assignment_id_for_row(conn: &Connection, id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(assignment_id), 0) + 1
         FROM agent_run_queue
         WHERE session_id = (SELECT session_id FROM agent_run_queue WHERE id = ?1)",
        params![id],
        |row| row.get(0),
    )
}

fn row_to_queue_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueRow> {
    let status_tag: String = row.get(6)?;
    let payload_text: String = row.get(7)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(QueueRow {
        id: row.get(0)?,
        task_id: row.get(1)?,
        session_id: row.get(2)?,
        worker_id: row.get(3)?,
        role_type: row.get(4)?,
        cli: row.get(5)?,
        status: QueueStatus::from_tag(&status_tag),
        payload,
        attempts: row.get(8)?,
        continuation_count: row.get(9)?,
        no_progress_count: row.get(10)?,
        last_status: row.get(11)?,
        heartbeat_at: row.get(12)?,
        assignment_id: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        blocked_reason: row.get(16)?,
    })
}

fn insert_dependencies(
    conn: &Connection,
    row: &QueueRow,
    prerequisite_task_ids: &[String],
) -> rusqlite::Result<()> {
    let Some(task_id) = row.task_id.as_deref() else {
        return Ok(());
    };
    for prerequisite_task_id in prerequisite_task_ids {
        conn.execute(
            "INSERT INTO agent_run_queue_deps
                (session_id, task_id, prerequisite_task_id)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id, task_id, prerequisite_task_id) DO NOTHING",
            params![row.session_id, task_id, prerequisite_task_id],
        )?;
    }
    Ok(())
}

fn replace_dependencies(
    conn: &Connection,
    row: &QueueRow,
    prerequisite_task_ids: &[String],
) -> rusqlite::Result<()> {
    if let Some(task_id) = row.task_id.as_deref() {
        conn.execute(
            "DELETE FROM agent_run_queue_deps WHERE session_id = ?1 AND task_id = ?2",
            params![row.session_id, task_id],
        )?;
    }
    insert_dependencies(conn, row, prerequisite_task_ids)
}

fn block_task_subtree(
    conn: &Connection,
    session_id: &str,
    root_task_id: &str,
    include_root: bool,
    reason: &str,
    now_ms: i64,
) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE descendants(task_id) AS (
             SELECT task_id
             FROM agent_run_queue_deps
             WHERE session_id = ?1 AND prerequisite_task_id = ?2
             UNION
             SELECT dependency.task_id
             FROM agent_run_queue_deps AS dependency
             JOIN descendants ON dependency.prerequisite_task_id = descendants.task_id
             WHERE dependency.session_id = ?1
         )
         SELECT id
         FROM agent_run_queue
         WHERE session_id = ?1
           AND status IN ('queued', 'running')
           AND (task_id IN (SELECT task_id FROM descendants)
                OR (?3 = 1 AND task_id = ?2))
         ORDER BY id",
    )?;
    let ids = stmt
        .query_map(params![session_id, root_task_id, include_root as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    for id in &ids {
        conn.execute(
            "UPDATE agent_run_queue
             SET status = 'blocked', updated_at = ?2
             WHERE id = ?1 AND status IN ('queued', 'running')",
            params![id, now_ms],
        )?;
        conn.execute(
            "INSERT INTO agent_run_queue_blocks
                (queue_id, blocked_by_task_id, reason, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(queue_id) DO NOTHING",
            params![id, root_task_id, reason, now_ms],
        )?;
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo() -> QueueRepo {
        let db = Arc::new(ApplicationStateDb::open_in_memory().unwrap());
        let repo = QueueRepo::new(db);
        repo.ensure_schema().unwrap();
        repo
    }

    /// #175(d). The release must be fenced on the claim epoch, not just on
    /// `status = 'running'`.
    ///
    /// The ABA this prevents: A wins the claim (attempts=1) -> A's worktree
    /// creation runs long -> the maintenance pass reclaims the row past the stuck
    /// cutoff -> a retry claims it (attempts=2) and spawns a real worker -> A
    /// finally fails and releases, requeueing B's LIVE claim -> a third claimer
    /// double-spawns. Without the fence there is no way to tell A's release from
    /// B's.
    #[test]
    fn requeue_claimed_is_fenced_against_a_stale_epoch() {
        let repo = repo();
        repo.enqueue(&sample_row("r1", "s1", "s1-worker-1")).unwrap();

        // A claims (epoch 1).
        let epoch_a = repo.try_claim("r1", 0, 1_000).unwrap().expect("A wins");
        assert_eq!(epoch_a, 1);

        // Maintenance reclaims it as stale, then B claims (epoch 2).
        repo.reclaim_stuck(5_000, 6_000).unwrap();
        let epoch_b = repo.try_claim("r1", 5_000, 7_000).unwrap().expect("B wins");
        assert_eq!(epoch_b, 2);

        // A now fails and tries to release. It must NOT touch B's live claim.
        assert!(
            !repo.requeue_claimed("r1", epoch_a, 8_000).unwrap(),
            "a stale epoch must not release a newer claim"
        );
        assert_eq!(
            repo.get_row("r1").unwrap().unwrap().status,
            QueueStatus::Running,
            "B's claim must survive A's late release"
        );

        // B's own release still works.
        assert!(repo.requeue_claimed("r1", epoch_b, 9_000).unwrap());
        assert_eq!(
            repo.get_row("r1").unwrap().unwrap().status,
            QueueStatus::Queued
        );
    }

    /// The release must never drag a terminal row back to claimable.
    #[test]
    fn requeue_claimed_is_a_noop_on_finalized_and_queued_rows() {
        let repo = repo();
        repo.enqueue(&sample_row("r1", "s1", "s1-worker-1")).unwrap();
        let epoch = repo.try_claim("r1", 0, 1_000).unwrap().unwrap();
        repo.record_heartbeat("s1", "s1-worker-1", "completed", 2_000)
            .unwrap();
        assert_eq!(
            repo.get_row("r1").unwrap().unwrap().status,
            QueueStatus::Finalized
        );

        assert!(!repo.requeue_claimed("r1", epoch, 3_000).unwrap());
        assert_eq!(
            repo.get_row("r1").unwrap().unwrap().status,
            QueueStatus::Finalized,
            "verified work must not be dragged back to queued"
        );

        // A never-claimed queued row is likewise untouched.
        repo.enqueue(&sample_row("r2", "s1", "s1-worker-2")).unwrap();
        assert!(!repo.requeue_claimed("r2", 1, 4_000).unwrap());
        assert_eq!(
            repo.get_row("r2").unwrap().unwrap().status,
            QueueStatus::Queued
        );
    }

    fn sample_row(id: &str, session_id: &str, worker_id: &str) -> QueueRow {
        QueueRow {
            id: id.to_string(),
            task_id: None,
            session_id: session_id.to_string(),
            worker_id: worker_id.to_string(),
            role_type: "backend".to_string(),
            cli: "codex".to_string(),
            status: QueueStatus::Queued,
            payload: json!({ "worktree_path": "D:/wt", "model": "gpt-5.5" }),
            attempts: 0,
            continuation_count: 0,
            no_progress_count: 0,
            last_status: None,
            heartbeat_at: None,
            assignment_id: 0,
            blocked_reason: None,
            created_at: 1000,
            updated_at: 1000,
        }
    }

    #[test]
    fn test_ensure_schema_is_idempotent() {
        let repo = repo();
        repo.ensure_schema().unwrap();
        repo.ensure_schema().unwrap();
    }

    #[test]
    fn ensure_schema_upgrades_legacy_queue_without_losing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy-application-state.db");
        {
            let legacy = Connection::open(&db_path).unwrap();
            legacy
                .execute(
                    "CREATE TABLE agent_run_queue (
                id                 TEXT PRIMARY KEY,
                task_id            TEXT,
                session_id         TEXT NOT NULL,
                worker_id          TEXT NOT NULL,
                role_type          TEXT NOT NULL,
                cli                TEXT NOT NULL,
                status             TEXT NOT NULL,
                payload            TEXT NOT NULL,
                attempts           INTEGER NOT NULL DEFAULT 0,
                continuation_count INTEGER NOT NULL DEFAULT 0,
                no_progress_count  INTEGER NOT NULL DEFAULT 0,
                last_status        TEXT,
                heartbeat_at       INTEGER,
                created_at         INTEGER NOT NULL,
                updated_at         INTEGER NOT NULL
            )",
                    [],
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO agent_run_queue
                (id, task_id, session_id, worker_id, role_type, cli, status, payload,
                 attempts, continuation_count, no_progress_count, last_status,
                 heartbeat_at, created_at, updated_at)
             VALUES ('legacy-run', 'T2', 'legacy-session', 'legacy-worker', 'backend',
                     'codex', 'finalized', '{\"sentinel\":true}', 3, 4, 5, 'completed',
                     1234, 1000, 2000)",
                    [],
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO agent_run_queue
                        SELECT 'legacy-run-2', task_id, session_id, 'legacy-worker-2',
                               role_type, cli, status, payload, attempts, continuation_count,
                               no_progress_count, last_status, heartbeat_at, created_at, updated_at
                        FROM agent_run_queue WHERE id = 'legacy-run'",
                    [],
                )
                .unwrap();
        }

        let conn = Connection::open(&db_path).unwrap();
        ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap();

        let columns = {
            let mut stmt = conn.prepare("PRAGMA table_info(agent_run_queue)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert!(
            columns.iter().any(|column| column == "assignment_id"),
            "the deployed table must gain the assignment identity column"
        );
        assert!(
            columns.iter().any(|column| column == "spawned_worker_id"),
            "the deployed table must gain the immutable spawn identity column"
        );
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_run_queue", [], |row| row.get(0))
            .unwrap();
        assert_eq!(row_count, 2, "every legacy row must survive the migration");
        let retained: (i64, String, String, i64, i64, i64, String) = conn
            .query_row(
                "SELECT COUNT(*), id, payload, attempts, heartbeat_at, assignment_id,
                        spawned_worker_id
                 FROM agent_run_queue WHERE id = 'legacy-run'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            retained,
            (
                1,
                "legacy-run".to_string(),
                "{\"sentinel\":true}".to_string(),
                3,
                1234,
                0,
                "legacy-worker".to_string(),
            ),
            "migration must preserve the full legacy row, leave unknown assignment history \
             unstamped, and backfill the only honest spawn identity"
        );
    }

    #[test]
    fn test_worker_queue_status_round_trip() {
        // Exact-string match, mirroring domain/status.rs.
        for (status, tag) in [
            (QueueStatus::Queued, "\"queued\""),
            (QueueStatus::Running, "\"running\""),
            (QueueStatus::Finalized, "\"finalized\""),
            (QueueStatus::Failed, "\"failed\""),
            (QueueStatus::Blocked, "\"blocked\""),
        ] {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, tag);
            let decoded: QueueStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, status);
            assert_eq!(QueueStatus::from_tag(status.as_tag()), status);
        }
    }

    #[test]
    fn test_enqueue_and_snapshot_roundtrip() {
        let repo = repo();
        repo.enqueue(&sample_row("r1", "s1", "s1-worker-1")).unwrap();
        let snap = repo.snapshot("s1").unwrap();
        assert_eq!(snap.queued, 1);
        assert_eq!(snap.rows.len(), 1);
        // payload is parsed JSON, not double-encoded.
        assert_eq!(snap.rows[0].payload, json!({ "worktree_path": "D:/wt", "model": "gpt-5.5" }));

        // Re-enqueue is a no-op (ON CONFLICT DO NOTHING).
        repo.enqueue(&sample_row("r1", "s1", "s1-worker-1")).unwrap();
        assert_eq!(repo.snapshot("s1").unwrap().rows.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_queue_atomic_claim_no_double_spawn() {
        let repo = repo();
        repo.enqueue(&sample_row("r1", "s1", "s1-worker-1")).unwrap();

        let a = repo.clone();
        let b = repo.clone();
        // Two concurrent claimers race for the same row.
        let (ra, rb) = tokio::join!(
            tokio::task::spawn_blocking(move || a.try_claim("r1", 0, 2000).unwrap()),
            tokio::task::spawn_blocking(move || b.try_claim("r1", 0, 2000).unwrap()),
        );
        let ra = ra.unwrap();
        let rb = rb.unwrap();

        // Exactly one claimer wins.
        assert!(ra.is_some() ^ rb.is_some(), "exactly one of two concurrent claimers must win");

        let row = repo.get_row("r1").unwrap().unwrap();
        assert_eq!(row.status, QueueStatus::Running);
        // The winner incremented attempts exactly once; the loser's UPDATE matched 0 rows.
        assert_eq!(row.attempts, 1);
    }

    #[test]
    fn test_queue_reclaim_after_stuck_cutoff() {
        let repo = repo();
        // A running row with a stale heartbeat (heartbeat_at = 100, cutoff = 1000).
        let mut stale = sample_row("stale", "s1", "s1-worker-1");
        stale.status = QueueStatus::Running;
        stale.heartbeat_at = Some(100);
        repo.enqueue(&stale).unwrap();

        // A running row with a fresh heartbeat (heartbeat_at = 5000, after cutoff).
        let mut fresh = sample_row("fresh", "s1", "s1-worker-2");
        fresh.status = QueueStatus::Running;
        fresh.heartbeat_at = Some(5000);
        repo.enqueue(&fresh).unwrap();

        let reclaimed = repo.reclaim_stuck(1000, 9999).unwrap();
        assert_eq!(reclaimed, vec!["stale".to_string()]);
        assert_eq!(repo.get_row("stale").unwrap().unwrap().status, QueueStatus::Queued);
        // Fresh row untouched.
        assert_eq!(repo.get_row("fresh").unwrap().unwrap().status, QueueStatus::Running);

        // The reclaimed row is now claimable again.
        assert!(repo.try_claim("stale", 1000, 10000).unwrap().is_some());
    }

    #[test]
    fn test_completed_heartbeat_finalizes_and_cannot_be_reclaimed() {
        let repo = repo();
        let mut row = sample_row("r1", "s1", "s1-worker-1");
        row.status = QueueStatus::Running;
        row.heartbeat_at = Some(100);
        repo.enqueue(&row).unwrap();

        assert!(repo
            .record_heartbeat("s1", "s1-worker-1", "completed", 200)
            .unwrap());
        assert!(repo
            .record_heartbeat("s1", "s1-worker-1", "working", 300)
            .unwrap());

        let completed = repo.get_row("r1").unwrap().unwrap();
        assert_eq!(completed.status, QueueStatus::Finalized);
        assert_eq!(completed.last_status.as_deref(), Some("completed"));
        assert_eq!(completed.heartbeat_at, Some(300));
        assert_eq!(completed.continuation_count, 1);
        assert_eq!(completed.no_progress_count, 0);
        assert!(repo.reclaim_stuck(1_000, 2_000).unwrap().is_empty());
        assert!(repo.try_claim("r1", 1_000, 2_000).unwrap().is_none());

        let mut failed = sample_row("failed", "s1", "s1-worker-2");
        failed.status = QueueStatus::Failed;
        repo.enqueue(&failed).unwrap();
        assert!(!repo
            .record_heartbeat("s1", "s1-worker-2", "completed", 300)
            .unwrap());
        assert_eq!(
            repo.get_row("failed").unwrap().unwrap().status,
            QueueStatus::Failed
        );
    }

    #[test]
    fn test_try_claim_treats_stale_running_as_claimable() {
        let repo = repo();
        let mut stale = sample_row("stale", "s1", "s1-worker-1");
        stale.status = QueueStatus::Running;
        stale.heartbeat_at = Some(100);
        repo.enqueue(&stale).unwrap();

        // Even without an explicit reclaim pass, try_claim recovers a stale-running row.
        assert!(repo.try_claim("stale", 1000, 2000).unwrap().is_some());
        // A fresh-running row is NOT claimable.
        let mut fresh = sample_row("fresh", "s1", "s1-worker-2");
        fresh.status = QueueStatus::Running;
        fresh.heartbeat_at = Some(5000);
        repo.enqueue(&fresh).unwrap();
        assert!(repo.try_claim("fresh", 1000, 2000).unwrap().is_none());
    }

    #[test]
    fn test_queue_no_progress_finalized() {
        let repo = repo();
        let mut row = sample_row("r1", "s1", "s1-worker-1");
        row.status = QueueStatus::Running;
        repo.enqueue(&row).unwrap();

        // Heartbeat with identical status N times → no_progress_count climbs.
        for i in 0..5 {
            repo.record_heartbeat("s1", "s1-worker-1", "working", 2000 + i)
                .unwrap();
        }
        let after = repo.get_row("r1").unwrap().unwrap();
        // First heartbeat is a continuation (last_status was NULL); the next four are no-progress.
        assert_eq!(after.no_progress_count, 4);
        assert_eq!(after.continuation_count, 1);

        // One more identical heartbeat pushes no_progress to 5 (the MAX), then finalize.
        repo.record_heartbeat("s1", "s1-worker-1", "working", 3000)
            .unwrap();
        assert_eq!(repo.get_row("r1").unwrap().unwrap().no_progress_count, 5);

        let finalized = repo.finalize_no_progress(8, 5, 4000).unwrap();
        assert_eq!(finalized, vec!["r1".to_string()]);
        assert_eq!(repo.get_row("r1").unwrap().unwrap().status, QueueStatus::Finalized);
    }

    #[test]
    fn test_queue_continuation_exceeded() {
        let repo = repo();
        let mut row = sample_row("r1", "s1", "s1-worker-1");
        row.status = QueueStatus::Running;
        repo.enqueue(&row).unwrap();

        // Alternate statuses to bump continuation_count each time.
        let statuses = ["working", "idle", "working", "idle", "working", "idle", "working", "idle"];
        for (i, s) in statuses.iter().enumerate() {
            repo.record_heartbeat("s1", "s1-worker-1", s, 2000 + i as i64)
                .unwrap();
        }
        let after = repo.get_row("r1").unwrap().unwrap();
        assert!(after.continuation_count >= 8, "continuation_count should reach MAX");
        assert_eq!(after.no_progress_count, 0);

        let finalized = repo.finalize_no_progress(8, 5, 9000).unwrap();
        assert_eq!(finalized, vec!["r1".to_string()]);
        assert_eq!(repo.get_row("r1").unwrap().unwrap().status, QueueStatus::Finalized);
    }

    #[test]
    fn test_queue_survives_restart() {
        // Persist to a real file, drop the DB, reopen on the same path, assert rows intact.
        let dir = tempfile::TempDir::new().unwrap();
        {
            let db = Arc::new(ApplicationStateDb::open(dir.path()).unwrap());
            let repo = QueueRepo::new(db);
            repo.ensure_schema().unwrap();
            let mut row = sample_row("r1", "s1", "s1-worker-1");
            row.status = QueueStatus::Running;
            row.heartbeat_at = Some(100);
            repo.enqueue(&row).unwrap();
        }
        // Reopen — simulating an app restart against the same base_dir.
        let db2 = Arc::new(ApplicationStateDb::open(dir.path()).unwrap());
        let repo2 = QueueRepo::new(db2);
        repo2.ensure_schema().unwrap();
        let snap = repo2.snapshot("s1").unwrap();
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].id, "r1");
        assert_eq!(snap.running, 1);

        // reconcile-style repair: an orphaned 'running' row flips to 'queued'.
        assert!(repo2.requeue_running("r1", 200).unwrap());
        assert_eq!(repo2.get_row("r1").unwrap().unwrap().status, QueueStatus::Queued);
    }
}
