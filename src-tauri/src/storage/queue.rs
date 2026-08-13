//! SQLite-backed durable sub-agent run queue (#126).
//!
//! Built on top of #124's [`ApplicationStateDb`] (the single shared `application_state.db`).
//! This module owns one additive table, `agent_run_queue`, created via an idempotent
//! [`ensure_schema`] (`CREATE TABLE IF NOT EXISTS` + indexes) called once at startup.
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
    pub rows: Vec<QueueRow>,
}

/// Create the `agent_run_queue` table (+ indexes) if absent.
///
/// Additive-only and idempotent: safe to call at every startup. Uses plain
/// `IF NOT EXISTS`, leaving #124's `schema_meta` version table untouched.
pub fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_run_queue (
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
                    (id, task_id, session_id, worker_id, role_type, cli, status, payload,
                     attempts, continuation_count, no_progress_count, last_status,
                     heartbeat_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
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

    /// THE atomic claim. A single `UPDATE ... WHERE` flips a claimable row to `running`.
    ///
    /// A row is claimable when it is `queued`, OR it is `running` but its heartbeat is
    /// stale (older than `stuck_cutoff_ms`) or absent, AND every dependency recorded for its
    /// task has at least one finalized queue row in the same session. Returns `true` iff THIS
    /// call won the claim (`conn.changes() == 1`). Because readiness is a correlated subquery
    /// inside the atomic statement, exactly one of N concurrent claimers wins without a
    /// check-then-claim race.
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
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_run_queue
                 SET status = 'running',
                     attempts = attempts + 1,
                     updated_at = ?2,
                     heartbeat_at = ?2,
                     worker_id = COALESCE(?4, worker_id)
                 WHERE id = ?1
                   AND (status = 'queued'
                        OR (status = 'running'
                            AND (heartbeat_at IS NULL OR heartbeat_at < ?3)))
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
                   )",
                params![id, now_ms, stuck_cutoff_ms, worker_id],
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
            conn.execute(
                "UPDATE agent_run_queue
                 SET status = 'queued',
                     worker_id = CASE
                         WHEN task_id IS NOT NULL THEN 'pending:' || id ELSE worker_id END,
                     updated_at = ?3
                 WHERE id = ?1 AND status = 'running' AND attempts = ?2",
                params![id, expected_attempts, now_ms],
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
            let changed = tx.execute(
                "UPDATE agent_run_queue
                 SET status = 'failed', updated_at = ?3
                 WHERE id = ?1 AND status = 'running' AND attempts = ?2",
                params![id, expected_attempts, now_ms],
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
    /// reset its spawn budget. Unfenced by design — this is the documented exit
    /// from a row that no automatic path can recover.
    pub fn release_claim_manual(&self, id: &str, now_ms: i64) -> Result<bool, StorageError> {
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_run_queue
                 SET status = 'queued',
                     worker_id = CASE
                         WHEN task_id IS NOT NULL THEN 'pending:' || id ELSE worker_id END,
                     attempts = 0,
                     updated_at = ?2
                 WHERE id = ?1 AND status IN ('running', 'failed')",
                params![id, now_ms],
            )?;
            Ok(conn.changes() == 1)
        })
    }

    /// Record a heartbeat for a nonterminal row and advance the progress counters.
    ///
    /// State machine: if the incoming `status` equals the stored `last_status`, the worker
    /// made no progress → `no_progress_count += 1`. If it changed (or there was no prior
    /// status), it is a continuation → `continuation_count += 1`, `no_progress_count = 0`,
    /// and `last_status` is updated. A `completed` heartbeat atomically moves a queued or
    /// running row to the existing terminal `finalized` state so stale-run maintenance cannot
    /// reclaim verified work. Failed rows are never overwritten. Always refreshes
    /// `heartbeat_at`. Returns `true` if a row was updated.
    pub fn record_heartbeat(
        &self,
        session_id: &str,
        worker_id: &str,
        status: &str,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_run_queue
                 SET status = CASE
                         WHEN ?4 = 'completed' AND status IN ('queued', 'running')
                         THEN 'finalized' ELSE status END,
                     heartbeat_at = ?3,
                     updated_at = ?3,
                     no_progress_count = CASE
                         WHEN last_status IS NOT NULL AND last_status = ?4
                         THEN no_progress_count + 1 ELSE 0 END,
                     continuation_count = CASE
                         WHEN last_status IS NULL OR last_status <> ?4
                         THEN continuation_count + 1 ELSE continuation_count END,
                     last_status = ?4
                 WHERE session_id = ?1 AND worker_id = ?2
                   AND status IN ('queued', 'running')",
                params![session_id, worker_id, now_ms, status],
            )?;
            Ok(conn.changes() >= 1)
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
        self.db.with_conn(|conn| {
            let ids: Vec<String> = {
                let mut stmt = conn.prepare(
                    "SELECT id FROM agent_run_queue
                     WHERE status = 'running'
                       AND (heartbeat_at IS NULL OR heartbeat_at < ?1)",
                )?;
                let rows = stmt
                    .query_map(params![stuck_cutoff_ms], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            if !ids.is_empty() {
                conn.execute(
                    "UPDATE agent_run_queue
                     SET status = 'queued',
                         worker_id = CASE
                             WHEN task_id IS NOT NULL THEN 'pending:' || id ELSE worker_id END,
                         updated_at = ?2
                     WHERE status = 'running'
                       AND (heartbeat_at IS NULL OR heartbeat_at < ?1)",
                    params![stuck_cutoff_ms, now_ms],
                )?;
            }
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
            conn.execute(
                "UPDATE agent_run_queue
                 SET status = 'queued',
                     worker_id = CASE
                         WHEN task_id IS NOT NULL THEN 'pending:' || id ELSE worker_id END,
                     updated_at = ?2
                 WHERE id = ?1 AND status = 'running'",
                params![id, now_ms],
            )?;
            Ok(conn.changes() == 1)
        })
    }

    /// All rows for a session that are not terminal-removed, ordered by creation.
    pub fn rows_for_session(&self, session_id: &str) -> Result<Vec<QueueRow>, StorageError> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT queue.id, queue.task_id, queue.session_id, queue.worker_id,
                        queue.role_type, queue.cli, queue.status, queue.payload,
                        queue.attempts, queue.continuation_count, queue.no_progress_count,
                        queue.last_status, queue.heartbeat_at, queue.created_at, queue.updated_at,
                        block.reason
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
                            queue.last_status, queue.heartbeat_at, queue.created_at, queue.updated_at,
                            block.reason
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
            rows,
        })
    }
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
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        blocked_reason: row.get(15)?,
    })
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

        let completed = repo.get_row("r1").unwrap().unwrap();
        assert_eq!(completed.status, QueueStatus::Finalized);
        assert_eq!(completed.last_status.as_deref(), Some("completed"));
        assert_eq!(completed.heartbeat_at, Some(200));
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
