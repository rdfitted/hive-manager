//! SQLite-backed `application_state` key/value layer.
//!
//! # Agent turn-start contract
//!
//! This module owns a single SQLite database file (`application_state.db`) under the
//! app-data base dir. It stores an additive-only `application_state(session_id, key,
//! value, updated_at)` table holding arbitrary JSON values keyed by `(session_id, key)`.
//!
//! [`ApplicationStateDb::read_application_state`] is THE documented accessor the agent
//! orchestration loop should call at the start of each turn:
//!
//! ```ignore
//! let rows = app_state_db.read_application_state(session_id)?;
//! // rows: Vec<ApplicationStateRow> with `value` already parsed to serde_json::Value.
//! // A MISSING key means "unset — use defaults". Sessions with no rows return an empty Vec.
//! ```
//!
//! The `value` column is stored as JSON TEXT and parsed back to a clean
//! `serde_json::Value` on read (never a double-encoded string), so callers can match on
//! the JSON shape directly.
//!
//! # Concurrency invariant
//!
//! All access goes through a single `parking_lot::Mutex<Connection>`. The guard is
//! synchronous and must NOT be held across an `.await` — HTTP handlers wrap these calls
//! in `tokio::task::spawn_blocking`. Atomic operations ([`ApplicationStateDb::take_key`])
//! run as a single short `BEGIN IMMEDIATE` transaction under the mutex.

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::StorageError;

/// A single row of application state.
///
/// `value` is the parsed JSON value (NOT a double-encoded string). Missing keys are
/// simply absent from the returned collection — callers treat absence as "unset".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApplicationStateRow {
    pub session_id: String,
    pub key: String,
    pub value: serde_json::Value,
    pub updated_at: i64,
}

/// Thread-safe wrapper around the shared `application_state.db` connection.
///
/// Cloning is cheap (the inner `Arc` is shared); pass `Arc<ApplicationStateDb>` around
/// to any subsystem that needs the database.
pub struct ApplicationStateDb {
    conn: Arc<Mutex<Connection>>,
}

impl ApplicationStateDb {
    /// Open (or create) `base_dir/application_state.db`, enable WAL, and run migrations.
    ///
    /// Migrations are additive-only and idempotent, so calling `open` repeatedly on the
    /// same directory is safe and is the startup path.
    pub fn open(base_dir: &Path) -> Result<Self, StorageError> {
        let db_path = base_dir.join("application_state.db");
        let conn = Connection::open(&db_path)?;
        // WAL improves concurrent read/write behavior; foreign_keys for future tables.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        run_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory database (used by tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        run_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Escape hatch for subsystems (#125 journal/ledger, #126 queue) that need to run
    /// their own SQL against the shared connection.
    ///
    /// The guard must not be held across an `.await`.
    #[allow(dead_code)]
    pub fn handle(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// Run an arbitrary closure with the raw connection. Convenience over [`handle`].
    ///
    /// [`handle`]: ApplicationStateDb::handle
    #[allow(dead_code)]
    pub fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, StorageError> {
        let conn = self.conn.lock();
        Ok(f(&conn)?)
    }

    /// Upsert a `(session_id, key)` value with the given millisecond timestamp.
    pub fn write(
        &self,
        session_id: &str,
        key: &str,
        value: &serde_json::Value,
        updated_at_ms: i64,
    ) -> Result<(), StorageError> {
        let value_text = serde_json::to_string(value)?;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO application_state (session_id, key, value, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, key)
             DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![session_id, key, value_text, updated_at_ms],
        )?;
        Ok(())
    }

    /// Return ALL rows for a session, ordered by key.
    ///
    /// This is the documented agent turn-start accessor. An empty `Vec` means the
    /// session has no persisted state yet (all keys unset → use defaults).
    pub fn read_application_state(
        &self,
        session_id: &str,
    ) -> Result<Vec<ApplicationStateRow>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT session_id, key, value, updated_at
             FROM application_state
             WHERE session_id = ?1
             ORDER BY key",
        )?;
        let rows = stmt
            .query_map(params![session_id], row_to_application_state)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Single-key convenience accessor. Returns `None` when the key is unset.
    #[allow(dead_code)]
    pub fn read_key(
        &self,
        session_id: &str,
        key: &str,
    ) -> Result<Option<ApplicationStateRow>, StorageError> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT session_id, key, value, updated_at
                 FROM application_state
                 WHERE session_id = ?1 AND key = ?2",
                params![session_id, key],
                row_to_application_state,
            )
            .optional()?;
        Ok(row)
    }

    /// Return rows changed strictly after `since_updated_at_ms` (exclusive watermark),
    /// ordered by `updated_at`. Uses the `updated_at` index.
    pub fn poll_changed(
        &self,
        session_id: &str,
        since_updated_at_ms: i64,
    ) -> Result<Vec<ApplicationStateRow>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT session_id, key, value, updated_at
             FROM application_state
             WHERE session_id = ?1 AND updated_at > ?2
             ORDER BY updated_at",
        )?;
        let rows = stmt
            .query_map(params![session_id, since_updated_at_ms], row_to_application_state)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Atomically read-and-delete a single key in one transaction.
    ///
    /// Returns the row if it existed (and deletes it), or `None`. Used by #128 for
    /// one-shot context keys (e.g. `pending_selection_context`) with exactly-one-turn
    /// semantics — no TTL needed.
    pub fn take_key(
        &self,
        session_id: &str,
        key: &str,
    ) -> Result<Option<ApplicationStateRow>, StorageError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let row = tx
            .query_row(
                "SELECT session_id, key, value, updated_at
                 FROM application_state
                 WHERE session_id = ?1 AND key = ?2",
                params![session_id, key],
                row_to_application_state,
            )
            .optional()?;
        if row.is_some() {
            tx.execute(
                "DELETE FROM application_state WHERE session_id = ?1 AND key = ?2",
                params![session_id, key],
            )?;
        }
        tx.commit()?;
        Ok(row)
    }
}

/// Map a SQLite row into an [`ApplicationStateRow`], parsing the JSON `value` column.
fn row_to_application_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApplicationStateRow> {
    let value_text: String = row.get(2)?;
    let value: serde_json::Value = serde_json::from_str(&value_text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(ApplicationStateRow {
        session_id: row.get(0)?,
        key: row.get(1)?,
        value,
        updated_at: row.get(3)?,
    })
}

/// Current schema version owned by this module.
const SCHEMA_VERSION: i64 = 1;

/// Additive-only, idempotent migrations.
///
/// Uses a `schema_meta(version)` table so future modules can add migration N+1 by
/// bumping [`SCHEMA_VERSION`] and gating new DDL on the stored version. Every statement
/// is `CREATE ... IF NOT EXISTS`, so re-running is a no-op.
fn run_migrations(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_meta (version INTEGER NOT NULL)",
        [],
    )?;

    let current: Option<i64> = conn
        .query_row("SELECT MAX(version) FROM schema_meta", [], |row| row.get(0))
        .optional()?
        .flatten();

    // Migration 1: application_state table + updated_at index.
    if current.unwrap_or(0) < 1 {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS application_state (
                session_id TEXT NOT NULL,
                key        TEXT NOT NULL,
                value      TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (session_id, key)
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_application_state_updated_at
             ON application_state(updated_at)",
            [],
        )?;
    }

    // Record the version exactly once (idempotent: only insert when not already present).
    let recorded: Option<i64> = conn
        .query_row(
            "SELECT version FROM schema_meta WHERE version = ?1",
            params![SCHEMA_VERSION],
            |row| row.get(0),
        )
        .optional()?;
    if recorded.is_none() {
        conn.execute(
            "INSERT INTO schema_meta (version) VALUES (?1)",
            params![SCHEMA_VERSION],
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn open_temp() -> (TempDir, ApplicationStateDb) {
        let dir = TempDir::new().unwrap();
        let db = ApplicationStateDb::open(dir.path()).unwrap();
        (dir, db)
    }

    #[test]
    fn test_open_creates_schema_idempotently() {
        let dir = TempDir::new().unwrap();
        // First open creates the schema.
        let _db = ApplicationStateDb::open(dir.path()).unwrap();
        // Second open on the same dir must be a no-op (no duplicate version rows, table present).
        let db = ApplicationStateDb::open(dir.path()).unwrap();

        db.with_conn(|conn| {
            // schema_meta has exactly one row for version 1.
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM schema_meta WHERE version = 1",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 1, "schema version recorded exactly once");

            // application_state table exists.
            let table: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='application_state'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(table, 1, "application_state table exists");

            // The updated_at index exists.
            let idx: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_application_state_updated_at'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(idx, 1, "updated_at index exists");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_write_read_roundtrip() {
        let (_dir, db) = open_temp();
        db.write("s1", "route", &json!({ "route": "dashboard" }), 100)
            .unwrap();

        let rows = db.read_application_state("s1").unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.session_id, "s1");
        assert_eq!(row.key, "route");
        // Value is parsed JSON, NOT a double-encoded string.
        assert_eq!(row.value, json!({ "route": "dashboard" }));
        assert_eq!(row.updated_at, 100);

        // read_key returns the same row.
        let single = db.read_key("s1", "route").unwrap().unwrap();
        assert_eq!(single, *row);
        assert!(db.read_key("s1", "missing").unwrap().is_none());
    }

    #[test]
    fn test_upsert_updates_in_place() {
        let (_dir, db) = open_temp();
        db.write("s1", "k", &json!(1), 100).unwrap();
        db.write("s1", "k", &json!(2), 200).unwrap();

        let rows = db.read_application_state("s1").unwrap();
        assert_eq!(rows.len(), 1, "upsert keeps a single row per (session,key)");
        assert_eq!(rows[0].value, json!(2));
        assert_eq!(rows[0].updated_at, 200);
    }

    #[test]
    fn test_poll_watermark_exclusive() {
        let (_dir, db) = open_temp();
        db.write("s1", "a", &json!("a"), 100).unwrap();
        db.write("s1", "b", &json!("b"), 200).unwrap();
        db.write("s1", "c", &json!("c"), 300).unwrap();

        // Exclusive `>`: watermark 200 returns only the t=300 row.
        let changed = db.poll_changed("s1", 200).unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].key, "c");
        assert_eq!(changed[0].updated_at, 300);

        // Watermark 0 returns all rows, ordered by updated_at.
        let all = db.poll_changed("s1", 0).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].updated_at, 100);
        assert_eq!(all[2].updated_at, 300);
    }

    #[test]
    fn test_session_isolation() {
        let (_dir, db) = open_temp();
        db.write("a", "k", &json!("from-a"), 100).unwrap();
        db.write("b", "k", &json!("from-b"), 100).unwrap();

        let a = db.read_application_state("a").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].value, json!("from-a"));

        let b_poll = db.poll_changed("b", 0).unwrap();
        assert_eq!(b_poll.len(), 1);
        assert_eq!(b_poll[0].value, json!("from-b"));

        // Reading an unknown session yields nothing.
        assert!(db.read_application_state("c").unwrap().is_empty());
    }

    #[test]
    fn test_take_key_atomic_read_and_delete() {
        let (_dir, db) = open_temp();
        db.write("s1", "pending_selection_context", &json!({ "cell": "x" }), 100)
            .unwrap();

        // First take returns the row and deletes it.
        let taken = db.take_key("s1", "pending_selection_context").unwrap();
        assert!(taken.is_some());
        assert_eq!(taken.unwrap().value, json!({ "cell": "x" }));

        // Second take returns None (already consumed) — exactly-one-turn semantics.
        let again = db.take_key("s1", "pending_selection_context").unwrap();
        assert!(again.is_none());
        assert!(db.read_application_state("s1").unwrap().is_empty());

        // Taking a missing key is a no-op None.
        assert!(db.take_key("s1", "never").unwrap().is_none());
    }
}
