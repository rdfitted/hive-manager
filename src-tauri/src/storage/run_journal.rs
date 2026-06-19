//! SQLite-backed run journal + side-effect ledger (#125).
//!
//! Built on top of #124's [`ApplicationStateDb`] (the single shared `application_state.db`).
//! This module owns two additive tables, `run_journal` and `run_ledger`, created via an
//! idempotent [`ensure_schema`] (`CREATE TABLE IF NOT EXISTS`) called once at startup.
//!
//! # Write-step contract
//!
//! For a destructive op the caller:
//! 1. [`RunJournalStore::record_step_started`] (status `Started`),
//! 2. [`RunJournalStore::record_ledger`] BEFORE the op (effect_ref = SHA/branch, unconfirmed),
//! 3. performs the op,
//! 4. [`RunJournalStore::record_step_finished`] (`Completed`) AND
//!    [`RunJournalStore::confirm_ledger`] AFTER.
//!
//! A crash between (2) and (4) leaves a `Started` step + unconfirmed ledger row, which the
//! resume classifier flags `Unknown` and the recovery pass verifies.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::application_state::ApplicationStateDb;
use super::StorageError;
use crate::domain::run_journal::{
    Confidence, LedgerEntry, RunJournalEntry, StepKind, StepStatus,
};

/// Create the `run_journal` and `run_ledger` tables (+ indexes) if absent.
///
/// Additive-only and idempotent: safe to call at every startup. Does NOT touch #124's
/// `schema_meta` version table — these tables use plain `IF NOT EXISTS`.
pub fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS run_journal (
            run_id      TEXT NOT NULL,
            step_id     TEXT NOT NULL,
            kind        TEXT NOT NULL,
            status      TEXT NOT NULL,
            started_at  TEXT NOT NULL,
            finished_at TEXT,
            detail      TEXT,
            PRIMARY KEY (run_id, step_id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_run_journal_run ON run_journal(run_id)",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS run_ledger (
            run_id      TEXT NOT NULL,
            step_id     TEXT NOT NULL,
            effect_kind TEXT NOT NULL,
            effect_ref  TEXT,
            confirmed   INTEGER NOT NULL,
            confidence  TEXT NOT NULL,
            recorded_at TEXT NOT NULL,
            PRIMARY KEY (run_id, step_id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_run_ledger_run ON run_ledger(run_id)",
        [],
    )?;
    Ok(())
}

/// Pure classifier: given a journal row, derive its effective status for a resume.
///
/// - `Completed`/`Failed`/`Skipped` are terminal and returned as-is.
/// - A `Started` row with a present + unconfirmed ledger row is `Unknown` (recovery candidate).
/// - A `Started` row with no recovery signal is `Interrupted` (app killed mid-step).
///
/// `has_unconfirmed_ledger` is the caller's lookup into the ledger for this step.
pub fn classify_step(entry: &RunJournalEntry, has_unconfirmed_ledger: bool) -> StepStatus {
    match entry.status {
        StepStatus::Completed => StepStatus::Completed,
        StepStatus::Failed => StepStatus::Failed,
        StepStatus::Skipped => StepStatus::Skipped,
        StepStatus::Started | StepStatus::Interrupted | StepStatus::Unknown => {
            if has_unconfirmed_ledger {
                StepStatus::Unknown
            } else {
                StepStatus::Interrupted
            }
        }
    }
}

/// Store for the run journal + ledger, backed by the shared [`ApplicationStateDb`].
///
/// Cheaply clonable (holds an `Arc`); thread a clone into the [`SessionController`].
#[derive(Clone)]
pub struct RunJournalStore {
    db: Arc<ApplicationStateDb>,
}

impl RunJournalStore {
    /// Wrap a shared [`ApplicationStateDb`]. Call [`RunJournalStore::ensure_schema`] once
    /// at startup before first use.
    pub fn new(db: Arc<ApplicationStateDb>) -> Self {
        Self { db }
    }

    /// Run [`ensure_schema`] against the shared connection (idempotent startup step).
    pub fn ensure_schema(&self) -> Result<(), StorageError> {
        self.db.with_conn(|conn| ensure_schema(conn))
    }

    /// Record a step as `Started` and return its deterministic id.
    ///
    /// Idempotent on `(run_id, step_id)`: re-recording the same logical step (same
    /// run/kind/ordinal) updates the row in place rather than duplicating it.
    pub fn record_step_started(
        &self,
        run_id: &str,
        kind: StepKind,
        ordinal: u64,
        detail: Option<&str>,
    ) -> Result<String, StorageError> {
        let step_id = RunJournalEntry::deterministic_step_id(run_id, kind, ordinal);
        let started_at = Utc::now().to_rfc3339();
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO run_journal (run_id, step_id, kind, status, started_at, finished_at, detail)
                 VALUES (?1, ?2, ?3, 'started', ?4, NULL, ?5)
                 ON CONFLICT(run_id, step_id)
                 DO UPDATE SET status='started', started_at=excluded.started_at, finished_at=NULL,
                               detail=excluded.detail",
                params![run_id, step_id, kind.as_tag(), started_at, detail],
            )?;
            Ok(())
        })?;
        Ok(step_id)
    }

    /// Mark a previously-started step as finished with the given terminal status.
    pub fn record_step_finished(
        &self,
        run_id: &str,
        step_id: &str,
        status: StepStatus,
    ) -> Result<(), StorageError> {
        let finished_at = Utc::now().to_rfc3339();
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE run_journal SET status=?1, finished_at=?2 WHERE run_id=?3 AND step_id=?4",
                params![status.as_tag(), finished_at, run_id, step_id],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    /// Write an (initially unconfirmed) ledger row BEFORE a destructive op.
    pub fn record_ledger(
        &self,
        run_id: &str,
        step_id: &str,
        effect_kind: &str,
        effect_ref: Option<&str>,
        confidence: Confidence,
    ) -> Result<(), StorageError> {
        let recorded_at = Utc::now().to_rfc3339();
        let confidence_tag = confidence_tag(confidence);
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO run_ledger (run_id, step_id, effect_kind, effect_ref, confirmed, confidence, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)
                 ON CONFLICT(run_id, step_id)
                 DO UPDATE SET effect_kind=excluded.effect_kind, effect_ref=excluded.effect_ref,
                               confidence=excluded.confidence, recorded_at=excluded.recorded_at",
                params![run_id, step_id, effect_kind, effect_ref, confidence_tag, recorded_at],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    /// Confirm a ledger row AFTER the op landed, optionally updating the effect_ref and
    /// setting the recovery confidence.
    pub fn confirm_ledger(
        &self,
        run_id: &str,
        step_id: &str,
        effect_ref: Option<&str>,
        confidence: Confidence,
    ) -> Result<(), StorageError> {
        let confidence_tag = confidence_tag(confidence);
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE run_ledger
                 SET confirmed=1, confidence=?1,
                     effect_ref=COALESCE(?2, effect_ref)
                 WHERE run_id=?3 AND step_id=?4",
                params![confidence_tag, effect_ref, run_id, step_id],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    /// All journal rows for a run, ordered by `started_at`.
    pub fn read_journal(&self, run_id: &str) -> Result<Vec<RunJournalEntry>, StorageError> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, step_id, kind, status, started_at, finished_at, detail
                 FROM run_journal WHERE run_id=?1 ORDER BY started_at, step_id",
            )?;
            let rows = stmt
                .query_map(params![run_id], row_to_journal_entry)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// All ledger rows for a run, ordered by `recorded_at`.
    pub fn read_ledger(&self, run_id: &str) -> Result<Vec<LedgerEntry>, StorageError> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, step_id, effect_kind, effect_ref, confirmed, confidence, recorded_at
                 FROM run_ledger WHERE run_id=?1 ORDER BY recorded_at, step_id",
            )?;
            let rows = stmt
                .query_map(params![run_id], row_to_ledger_entry)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Look up a single ledger row for a `(run_id, step_id)`.
    pub fn read_ledger_for_step(
        &self,
        run_id: &str,
        step_id: &str,
    ) -> Result<Option<LedgerEntry>, StorageError> {
        self.db.with_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT run_id, step_id, effect_kind, effect_ref, confirmed, confidence, recorded_at
                     FROM run_ledger WHERE run_id=?1 AND step_id=?2",
                    params![run_id, step_id],
                    row_to_ledger_entry,
                )
                .optional()?;
            Ok(row)
        })
    }
}

fn confidence_tag(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::High => "high",
        Confidence::Likely => "likely",
        Confidence::Uncertain => "uncertain",
    }
}

fn confidence_from_tag(tag: &str) -> Confidence {
    match tag {
        "high" => Confidence::High,
        "likely" => Confidence::Likely,
        _ => Confidence::Uncertain,
    }
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_journal_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunJournalEntry> {
    let kind_tag: String = row.get(2)?;
    let status_tag: String = row.get(3)?;
    let started_at: String = row.get(4)?;
    let finished_at: Option<String> = row.get(5)?;
    Ok(RunJournalEntry {
        run_id: row.get(0)?,
        step_id: row.get(1)?,
        kind: StepKind::from_tag(&kind_tag),
        status: StepStatus::from_tag(&status_tag),
        started_at: parse_dt(&started_at),
        finished_at: finished_at.as_deref().map(parse_dt),
        detail: row.get(6)?,
    })
}

fn row_to_ledger_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<LedgerEntry> {
    let confirmed_int: i64 = row.get(4)?;
    let confidence_tag: String = row.get(5)?;
    let recorded_at: String = row.get(6)?;
    Ok(LedgerEntry {
        run_id: row.get(0)?,
        step_id: row.get(1)?,
        effect_kind: row.get(2)?,
        effect_ref: row.get(3)?,
        confirmed: confirmed_int != 0,
        confidence: confidence_from_tag(&confidence_tag),
        recorded_at: parse_dt(&recorded_at),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> RunJournalStore {
        let db = Arc::new(ApplicationStateDb::open_in_memory().unwrap());
        let store = RunJournalStore::new(db);
        store.ensure_schema().unwrap();
        store
    }

    #[test]
    fn test_ensure_schema_is_idempotent() {
        let store = store();
        // Second call must be a no-op (no error).
        store.ensure_schema().unwrap();
        store.ensure_schema().unwrap();
    }

    #[test]
    fn test_journal_roundtrip() {
        let store = store();
        let step_id = store
            .record_step_started("run-1", StepKind::WorkerSpawn, 1, Some("worker 1"))
            .unwrap();
        store
            .record_step_finished("run-1", &step_id, StepStatus::Completed)
            .unwrap();

        let journal = store.read_journal("run-1").unwrap();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].step_id, step_id);
        assert_eq!(journal[0].kind, StepKind::WorkerSpawn);
        assert_eq!(journal[0].status, StepStatus::Completed);
        assert!(journal[0].finished_at.is_some());
        assert_eq!(journal[0].detail.as_deref(), Some("worker 1"));
    }

    #[test]
    fn test_deterministic_step_id_matches_domain() {
        let store = store();
        let a = store
            .record_step_started("run-1", StepKind::GitCommit, 2, None)
            .unwrap();
        // Re-recording the same logical step must UPSERT (one row, same id).
        let b = store
            .record_step_started("run-1", StepKind::GitCommit, 2, None)
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(store.read_journal("run-1").unwrap().len(), 1);
        assert_eq!(
            a,
            RunJournalEntry::deterministic_step_id("run-1", StepKind::GitCommit, 2)
        );
    }

    #[test]
    fn test_classify_completed_interrupted_unknown() {
        let now = Utc::now();
        let started = RunJournalEntry {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: StepKind::GitCommit,
            status: StepStatus::Started,
            started_at: now,
            finished_at: None,
            detail: None,
        };

        // Started, no ledger -> Interrupted.
        assert_eq!(classify_step(&started, false), StepStatus::Interrupted);
        // Started, unconfirmed ledger present -> Unknown (recovery candidate).
        assert_eq!(classify_step(&started, true), StepStatus::Unknown);

        // Completed passes through regardless of ledger.
        let mut completed = started.clone();
        completed.status = StepStatus::Completed;
        completed.finished_at = Some(now);
        assert_eq!(classify_step(&completed, true), StepStatus::Completed);
    }

    #[test]
    fn test_ledger_confirm_flow() {
        let store = store();
        let step_id = store
            .record_step_started("run-1", StepKind::GitCommit, 1, None)
            .unwrap();
        store
            .record_ledger(
                "run-1",
                &step_id,
                "git_commit",
                Some("deadbeef"),
                Confidence::Uncertain,
            )
            .unwrap();

        let before = store.read_ledger_for_step("run-1", &step_id).unwrap().unwrap();
        assert!(!before.confirmed);
        assert_eq!(before.effect_ref.as_deref(), Some("deadbeef"));
        assert_eq!(before.confidence, Confidence::Uncertain);

        store
            .confirm_ledger("run-1", &step_id, None, Confidence::High)
            .unwrap();
        let after = store.read_ledger_for_step("run-1", &step_id).unwrap().unwrap();
        assert!(after.confirmed);
        assert_eq!(after.confidence, Confidence::High);
        // effect_ref preserved via COALESCE when not overridden.
        assert_eq!(after.effect_ref.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn test_ledger_recover_unconfirmed() {
        // Simulate a crash between commit and confirmation: Started step + unconfirmed
        // ledger row. The classifier flags Unknown; recovery confirms with High.
        let store = store();
        let step_id = store
            .record_step_started("run-1", StepKind::GitCommit, 1, None)
            .unwrap();
        store
            .record_ledger("run-1", &step_id, "git_commit", Some("abc123"), Confidence::Uncertain)
            .unwrap();

        let entry = &store.read_journal("run-1").unwrap()[0];
        let ledger = store.read_ledger_for_step("run-1", &step_id).unwrap().unwrap();
        let has_unconfirmed = !ledger.confirmed;
        assert_eq!(classify_step(entry, has_unconfirmed), StepStatus::Unknown);

        // Recovery: the SHA "exists", so confirm with High.
        store
            .confirm_ledger("run-1", &step_id, None, Confidence::High)
            .unwrap();
        let recovered = store.read_ledger_for_step("run-1", &step_id).unwrap().unwrap();
        assert!(recovered.confirmed);
        assert_eq!(recovered.confidence, Confidence::High);
    }
}
