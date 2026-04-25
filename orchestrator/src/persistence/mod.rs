//! Persistent workflow checkpoint store backed by SQLite (sprint `0.3-S2a`).
//!
//! Implements the storage layer specified in
//! [`docs/adr/001-persistence-backend.md`]. This module ships the bottom
//! half of `0.3-S2`: the `RunCheckpointStore` API + tests. Wiring it into
//! `WorkflowRunner::run` and adding `boruna workflow resume` is the next
//! sprint (`0.3-S2b`).
//!
//! # Determinism contract (per ADR 001 + `docs/concepts/determinism.md`)
//!
//! - **Replay-verified state:** `output_json`, `output_hash`, terminal
//!   `status` values (`completed`, `failed`).
//! - **Operational metadata only:** `started_at`, `updated_at`, `ended_at`,
//!   transient `status` values like `running`/`pending`. **NEVER** feed
//!   these into a hash chain or replay-relevant ordering.
//! - **`run_id`** is **caller-provided**. The store does NOT generate it.
//!   Sprint `0.3-S2b` is responsible for derivation per the ADR
//!   (sha256 of workflow_hash + serialized inputs + counter, NOT
//!   `chrono::Utc::now()`).
//!
//! # Connection PRAGMAs (mandatory on every open)
//!
//! - `journal_mode = WAL` — concurrent reads while writing.
//! - `synchronous = NORMAL` — safe under WAL; durable to commit.
//! - `foreign_keys = ON` — **default is OFF.** Without this,
//!   `ON DELETE CASCADE` is silently inert. Locked by
//!   [`tests::foreign_keys_cascade_works`].
//! - `busy_timeout = 5000` — paired with the explicit retry loop in
//!   [`with_busy_retry`] for `BEGIN IMMEDIATE` writes.

use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use rusqlite::{params, Connection, Error as SqlError};
use serde::{Deserialize, Serialize};

/// Wire-format version of the on-disk schema. Bumped on **breaking** schema
/// changes (column rename, removal, type change). Additive changes (new
/// column with default, new optional table) keep the same version. A
/// future migration sprint will add an upgrade path; v1 just refuses to
/// open mismatched databases.
pub const SCHEMA_VERSION: i64 = 1;

/// Embedded schema migration. Applied on first open via `IF NOT EXISTS`,
/// so re-opens are idempotent. When v2 lands, this becomes a chain of
/// `ALTER TABLE` statements gated by the existing `schema_version` row.
const SCHEMA_V1_SQL: &str = include_str!("schema_v1.sql");

/// Errors surfaced by the persistence layer. Distinct kinds so callers can
/// react differently (retry on `Busy`, abort on `SchemaVersionMismatch`,
/// log-and-continue on `NotFound`, etc.).
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// Wrapped low-level SQL error. Use the more specific variants when
    /// possible — this is the catch-all.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] SqlError),

    /// `BEGIN IMMEDIATE` could not acquire the writer lock within the
    /// retry budget. Caller may retry at a coarser granularity.
    #[error("persistence_busy: writer lock held after retry budget exhausted")]
    Busy,

    /// A row-key the caller named does not exist. Surfaced from
    /// [`RunCheckpointStore::update_run_status`] when the target `run_id`
    /// is not in the database — silent-no-op was rejected as a footgun
    /// during review (a stale or typo'd run_id would propagate as success
    /// and corrupt the resume state machine invisibly).
    #[error("not_found: {entity} '{key}' does not exist")]
    NotFound { entity: &'static str, key: String },

    /// On-disk schema version is not what this build supports. Resolution
    /// is operator-driven — either upgrade Boruna to a build that supports
    /// the disk format or migrate the database. v1 has no migration path
    /// since there is no v0.
    #[error("schema_version mismatch: this build expects {expected}, database has {actual}")]
    SchemaVersionMismatch { expected: i64, actual: i64 },

    /// Serialization failure when encoding a struct to its JSON column.
    /// Practically unreachable — the structs we serialize are all
    /// `serde_json::Value`-compatible — but propagated rather than panicked.
    #[error("serialize error: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Lifecycle status of a workflow run. Persisted as a TEXT column for
/// readability in `sqlite3` shell debugging. Terminal values
/// (`Completed`, `Failed`) are replay-verified per the determinism
/// contract; transient values (`Running`, `Paused`) are operational only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Paused,
    Completed,
    Failed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Paused => "paused",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
        }
    }

    /// Parse from the persisted text form. Named `parse_str` rather than
    /// `from_str` to sidestep the inherent-vs-trait collision with
    /// `std::str::FromStr` (clippy lint `should_implement_trait`).
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "running" => Some(RunStatus::Running),
            "paused" => Some(RunStatus::Paused),
            "completed" => Some(RunStatus::Completed),
            "failed" => Some(RunStatus::Failed),
            _ => None,
        }
    }
}

/// Lifecycle status of a single step within a run. Same persistence model
/// as [`RunStatus`] — terminal values feed replay; transients are operational.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    AwaitingApproval,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Running => "running",
            StepStatus::Completed => "completed",
            StepStatus::Failed => "failed",
            StepStatus::AwaitingApproval => "awaiting_approval",
        }
    }

    /// Parse from the persisted text form. See `RunStatus::parse_str` for
    /// the naming rationale.
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(StepStatus::Pending),
            "running" => Some(StepStatus::Running),
            "completed" => Some(StepStatus::Completed),
            "failed" => Some(StepStatus::Failed),
            "awaiting_approval" => Some(StepStatus::AwaitingApproval),
            _ => None,
        }
    }
}

/// One row in the `runs` table. Fields documented per the determinism
/// contract — timestamps are operational, hashes are replay-verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub run_id: String,
    pub workflow_name: String,
    /// SHA-256 of the on-disk `workflow.json` at run-start time. The resume
    /// path (next sprint) refuses to resume against a workflow whose hash
    /// no longer matches.
    pub workflow_hash: String,
    pub status: RunStatus,
    /// Unix epoch milliseconds. **OPERATIONAL ONLY** — never feed into
    /// audit hashes. Recorded once at insert time.
    pub started_at_ms: i64,
    /// Unix epoch milliseconds. **OPERATIONAL ONLY.** Updated on every
    /// status transition.
    pub updated_at_ms: i64,
    /// Serialized capability `Policy` for this run. Replay-verified —
    /// changes to policy invalidate cached results.
    pub policy_json: String,
    /// Free-form JSON for run-scoped metadata (input hash, tenant id, etc).
    /// Not interpreted by the store.
    pub metadata_json: String,
}

/// One row in the `step_checkpoints` table. The `(run_id, step_id)`
/// composite key permits ordered scans for resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepCheckpoint {
    pub run_id: String,
    pub step_id: String,
    pub status: StepStatus,
    /// JSON-encoded step output. **REPLAY-VERIFIED** for terminal states.
    /// `None` until the step completes.
    pub output_json: Option<String>,
    /// SHA-256 of `output_json` UTF-8 bytes. **REPLAY-VERIFIED.**
    pub output_hash: Option<String>,
    /// **OPERATIONAL ONLY.** Unix ms.
    pub started_at_ms: Option<i64>,
    /// **OPERATIONAL ONLY.** Unix ms.
    pub ended_at_ms: Option<i64>,
    /// Human-readable error message for `Failed` steps. Not parsed by
    /// the store; propagated for logging / dashboard display.
    pub error_msg: Option<String>,
}

/// SQLite-backed checkpoint store.
///
/// Owns one `rusqlite::Connection`. The connection is single-threaded by
/// rusqlite design — wrap in a `Mutex` if shared across threads (the
/// orchestrator runs single-threaded today; future scheduler sprint
/// (`0.3-S7`) will revisit).
pub struct RunCheckpointStore {
    conn: Connection,
}

impl std::fmt::Debug for RunCheckpointStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `rusqlite::Connection` doesn't impl Debug; surface the type name
        // only so callers using `.expect()` / `.expect_err()` get readable
        // panic messages without leaking schema/data through Debug formatting.
        f.debug_struct("RunCheckpointStore").finish()
    }
}

impl RunCheckpointStore {
    /// Open or create a database file at `path`. Runs schema migration
    /// on first open. Idempotent on re-open.
    pub fn open(path: &Path) -> Result<Self, PersistenceError> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open an in-memory database (intended for tests).
    pub fn open_in_memory() -> Result<Self, PersistenceError> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, PersistenceError> {
        // Mandatory PRAGMAs. Order matters: `journal_mode = WAL` must be
        // set before any write activity locks the journal mode in.
        // `foreign_keys = ON` MUST be set on every connection — SQLite's
        // default is OFF and connection-scoped, not database-scoped.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000_i64)?;

        // Apply schema. Uses CREATE TABLE IF NOT EXISTS so re-open is
        // idempotent. The schema_version row is INSERT OR IGNORE so
        // re-opening doesn't double-insert.
        //
        // **v2+ migration pattern (when it lands):**
        // ```ignore
        // // 1. Read current version
        // let v: i64 = conn.query_row(
        //     "SELECT version FROM schema_version WHERE id = 1",
        //     [], |r| r.get(0))?;
        // // 2. Apply chained migrations under a single transaction so
        // //    a partial migration rolls back cleanly.
        // let tx = conn.unchecked_transaction()?;
        // if v < 2 { tx.execute_batch(SCHEMA_V1_TO_V2_SQL)?; }
        // if v < 3 { tx.execute_batch(SCHEMA_V2_TO_V3_SQL)?; }
        // // 3. Update the version row INSIDE the same transaction.
        // tx.execute(
        //     "UPDATE schema_version SET version = ?1 WHERE id = 1",
        //     params![SCHEMA_VERSION])?;
        // tx.commit()?;
        // ```
        // The CHECK (id = 1) constraint pins the table to a single row,
        // so `UPDATE ... WHERE id = 1` is the canonical way to bump.
        conn.execute_batch(SCHEMA_V1_SQL)?;

        // Verify version compatibility. The schema enforces a single row
        // (id = 1 CHECK constraint), so this query has at most one result.
        let actual: i64 = conn.query_row(
            "SELECT version FROM schema_version WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        if actual != SCHEMA_VERSION {
            return Err(PersistenceError::SchemaVersionMismatch {
                expected: SCHEMA_VERSION,
                actual,
            });
        }

        Ok(RunCheckpointStore { conn })
    }

    /// Insert a new run. Fails with the wrapped UNIQUE constraint error
    /// if `run_id` already exists.
    pub fn insert_run(&self, run: &RunRow) -> Result<(), PersistenceError> {
        with_busy_retry(|| {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute(
                "INSERT INTO runs \
                 (run_id, workflow_name, workflow_hash, status, started_at, updated_at, policy_json, metadata_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    run.run_id,
                    run.workflow_name,
                    run.workflow_hash,
                    run.status.as_str(),
                    run.started_at_ms,
                    run.updated_at_ms,
                    run.policy_json,
                    run.metadata_json,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Update the status + updated_at timestamp of an existing run.
    ///
    /// Returns `PersistenceError::NotFound { entity: "run", key: run_id }`
    /// if the row does not exist. Silent-no-op was rejected during review
    /// — a stale or typo'd `run_id` from the resume path would propagate
    /// as success and silently corrupt the state machine.
    pub fn update_run_status(
        &self,
        run_id: &str,
        status: RunStatus,
        updated_at_ms: i64,
    ) -> Result<(), PersistenceError> {
        with_busy_retry(|| {
            let tx = self.conn.unchecked_transaction()?;
            let rows_affected = tx.execute(
                "UPDATE runs SET status = ?1, updated_at = ?2 WHERE run_id = ?3",
                params![status.as_str(), updated_at_ms, run_id],
            )?;
            tx.commit()?;
            if rows_affected == 0 {
                return Err(PersistenceError::NotFound {
                    entity: "run",
                    key: run_id.to_string(),
                });
            }
            Ok(())
        })
    }

    /// Fetch one run by id. `Ok(None)` when the row doesn't exist.
    pub fn get_run(&self, run_id: &str) -> Result<Option<RunRow>, PersistenceError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, workflow_name, workflow_hash, status, started_at, updated_at, policy_json, metadata_json \
             FROM runs WHERE run_id = ?1",
        )?;
        let mut rows = stmt.query(params![run_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(parse_run_row(row)?)),
            None => Ok(None),
        }
    }

    /// List runs with the given status, ordered by `(workflow_name, run_id)`
    /// — deterministic, not timestamp-keyed (per the determinism contract).
    /// Uses the `idx_runs_status` index.
    pub fn list_runs_by_status(&self, status: RunStatus) -> Result<Vec<RunRow>, PersistenceError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, workflow_name, workflow_hash, status, started_at, updated_at, policy_json, metadata_json \
             FROM runs WHERE status = ?1 ORDER BY workflow_name, run_id",
        )?;
        let rows = stmt
            .query_map(params![status.as_str()], parse_run_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Insert OR replace a step checkpoint. Each call wraps in
    /// `BEGIN IMMEDIATE; ...; COMMIT;` so partial-step failures roll back
    /// cleanly. Caller writes once per step state transition
    /// (typically: insert as `running`, upsert as `completed` / `failed`).
    ///
    /// **`started_at` and `output_*` use COALESCE-on-conflict** —
    /// passing `None` preserves the existing value rather than overwriting
    /// to NULL. Caller pattern: insert with `Some(t1)` for `started_at`,
    /// later upsert with `None` for `started_at` and `Some(t2)` for
    /// `ended_at`. Without COALESCE, the second upsert would clobber
    /// `started_at` to NULL — a silent data-loss bug the review caught.
    /// `status`, `error_msg`, and `ended_at` always reflect the latest
    /// caller-supplied value (callers SHOULD manage them themselves).
    pub fn upsert_step_checkpoint(&self, cp: &StepCheckpoint) -> Result<(), PersistenceError> {
        with_busy_retry(|| {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute(
                "INSERT INTO step_checkpoints \
                 (run_id, step_id, status, output_json, output_hash, started_at, ended_at, error_msg) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(run_id, step_id) DO UPDATE SET \
                   status      = excluded.status, \
                   output_json = COALESCE(excluded.output_json, step_checkpoints.output_json), \
                   output_hash = COALESCE(excluded.output_hash, step_checkpoints.output_hash), \
                   started_at  = COALESCE(excluded.started_at,  step_checkpoints.started_at), \
                   ended_at    = excluded.ended_at, \
                   error_msg   = excluded.error_msg",
                params![
                    cp.run_id,
                    cp.step_id,
                    cp.status.as_str(),
                    cp.output_json,
                    cp.output_hash,
                    cp.started_at_ms,
                    cp.ended_at_ms,
                    cp.error_msg,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    /// List all checkpoints for one run, ordered by `(run_id, step_id)` —
    /// deterministic. Resume logic walks this in order to find the next
    /// pending step.
    pub fn list_step_checkpoints(
        &self,
        run_id: &str,
    ) -> Result<Vec<StepCheckpoint>, PersistenceError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, step_id, status, output_json, output_hash, started_at, ended_at, error_msg \
             FROM step_checkpoints WHERE run_id = ?1 ORDER BY step_id",
        )?;
        let rows = stmt
            .query_map(params![run_id], parse_step_checkpoint)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// Retry the supplied closure on `SQLITE_BUSY` or `SQLITE_LOCKED`.
/// Exponential backoff 10 ms → 50 ms → 250 ms → 1.25 s → fail with
/// `PersistenceError::Busy`.
///
/// Per the ADR's writer-serialization model: typical orchestrator usage
/// is single-writer-by-process-lifecycle, so this loop only fires under
/// the rare concurrent-process scenario (interactive `approve` racing the
/// scheduler daemon, etc.).
///
/// **Composes with the `busy_timeout = 5000` PRAGMA** set in `init()`:
/// the PRAGMA covers reads transparently up to 5s; this explicit retry
/// covers `BEGIN IMMEDIATE` writes for an additional ~1.56s. Total
/// worst-case wait under contention: ~6.5s. Both halves must remain —
/// removing the PRAGMA breaks read paths under contention; removing the
/// retry loop breaks immediate-acquire writes that fail before the
/// PRAGMA's timeout would help.
fn with_busy_retry<F, T>(mut op: F) -> Result<T, PersistenceError>
where
    F: FnMut() -> Result<T, PersistenceError>,
{
    const BACKOFF_MS: &[u64] = &[10, 50, 250, 1250];
    let mut attempts = 0;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(PersistenceError::Sqlite(SqlError::SqliteFailure(e, _)))
                if matches!(
                    e.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                if attempts >= BACKOFF_MS.len() {
                    return Err(PersistenceError::Busy);
                }
                sleep(Duration::from_millis(BACKOFF_MS[attempts]));
                attempts += 1;
            }
            Err(other) => return Err(other),
        }
    }
}

fn parse_run_row(row: &rusqlite::Row<'_>) -> Result<RunRow, SqlError> {
    let status_str: String = row.get(3)?;
    let status = RunStatus::parse_str(&status_str).ok_or_else(|| {
        SqlError::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown run status '{status_str}'"),
            )),
        )
    })?;
    Ok(RunRow {
        run_id: row.get(0)?,
        workflow_name: row.get(1)?,
        workflow_hash: row.get(2)?,
        status,
        started_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
        policy_json: row.get(6)?,
        metadata_json: row.get(7)?,
    })
}

fn parse_step_checkpoint(row: &rusqlite::Row<'_>) -> Result<StepCheckpoint, SqlError> {
    let status_str: String = row.get(2)?;
    let status = StepStatus::parse_str(&status_str).ok_or_else(|| {
        SqlError::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown step status '{status_str}'"),
            )),
        )
    })?;
    Ok(StepCheckpoint {
        run_id: row.get(0)?,
        step_id: row.get(1)?,
        status,
        output_json: row.get(3)?,
        output_hash: row.get(4)?,
        started_at_ms: row.get(5)?,
        ended_at_ms: row.get(6)?,
        error_msg: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_store() -> RunCheckpointStore {
        RunCheckpointStore::open_in_memory().expect("must open")
    }

    fn sample_run(run_id: &str) -> RunRow {
        RunRow {
            run_id: run_id.to_string(),
            workflow_name: "example".to_string(),
            workflow_hash: "sha256:dead".to_string(),
            status: RunStatus::Running,
            started_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_000,
            policy_json: "{}".to_string(),
            metadata_json: "{}".to_string(),
        }
    }

    fn sample_checkpoint(run_id: &str, step_id: &str, status: StepStatus) -> StepCheckpoint {
        StepCheckpoint {
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            status,
            output_json: None,
            output_hash: None,
            started_at_ms: Some(1_700_000_001_000),
            ended_at_ms: None,
            error_msg: None,
        }
    }

    // ── schema ──

    #[test]
    fn open_in_memory_creates_schema_idempotently() {
        let store = fresh_store();
        // Re-init via a manual re-run of the schema is implicit on every
        // open. Verify by querying the (single-row) version row.
        let version: i64 = store
            .conn
            .query_row("SELECT version FROM schema_version WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn re_open_existing_db_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.db");
        let store1 = RunCheckpointStore::open(&path).unwrap();
        store1.insert_run(&sample_run("R-1")).unwrap();
        drop(store1);

        // Re-opening must not error and must preserve data.
        let store2 = RunCheckpointStore::open(&path).unwrap();
        let row = store2.get_run("R-1").unwrap().expect("row preserved");
        assert_eq!(row.run_id, "R-1");
    }

    #[test]
    fn schema_version_mismatch_is_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.db");
        // Open and corrupt the schema_version row to simulate a future-
        // version database opened by an older Boruna build.
        {
            let store = RunCheckpointStore::open(&path).unwrap();
            store
                .conn
                .execute("UPDATE schema_version SET version = 999 WHERE id = 1", [])
                .unwrap();
        }
        let err = RunCheckpointStore::open(&path).expect_err("must reject");
        match err {
            PersistenceError::SchemaVersionMismatch { expected, actual } => {
                assert_eq!(expected, SCHEMA_VERSION);
                assert_eq!(actual, 999);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn schema_version_table_is_structurally_single_row() {
        // Lock the CHECK (id = 1) constraint — attempts to insert a row
        // with any other id must fail. This is the structural guarantee
        // that the version table can never accumulate stale rows.
        let store = fresh_store();
        let err = store
            .conn
            .execute(
                "INSERT INTO schema_version (id, version) VALUES (2, 99)",
                [],
            )
            .expect_err("CHECK (id = 1) must reject id != 1");
        assert!(matches!(err, SqlError::SqliteFailure(_, _)));
    }

    // ── runs CRUD ──

    #[test]
    fn insert_run_then_get() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        let got = store.get_run("R-1").unwrap().unwrap();
        assert_eq!(got.run_id, "R-1");
        assert_eq!(got.status, RunStatus::Running);
        assert_eq!(got.workflow_hash, "sha256:dead");
    }

    #[test]
    fn get_missing_run_returns_none() {
        let store = fresh_store();
        assert!(store.get_run("nope").unwrap().is_none());
    }

    #[test]
    fn insert_duplicate_run_id_fails() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        let err = store.insert_run(&sample_run("R-1")).unwrap_err();
        // Wrapped UNIQUE constraint error.
        assert!(matches!(err, PersistenceError::Sqlite(_)));
    }

    #[test]
    fn update_run_status_changes_status_and_updated_at() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        store
            .update_run_status("R-1", RunStatus::Completed, 1_700_000_500_000)
            .unwrap();
        let got = store.get_run("R-1").unwrap().unwrap();
        assert_eq!(got.status, RunStatus::Completed);
        assert_eq!(got.updated_at_ms, 1_700_000_500_000);
    }

    #[test]
    fn update_run_status_returns_not_found_on_missing_run() {
        // Documented contract (after review): update_run_status MUST return
        // PersistenceError::NotFound when the run_id doesn't exist.
        // Silent-no-op was rejected as a footgun — a stale or typo'd
        // run_id from resume code would propagate as success and corrupt
        // the state machine invisibly.
        let store = fresh_store();
        let err = store
            .update_run_status("nope", RunStatus::Failed, 0)
            .expect_err("missing run must error, not silent-OK");
        match err {
            PersistenceError::NotFound { entity, key } => {
                assert_eq!(entity, "run");
                assert_eq!(key, "nope");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn list_runs_by_status_returns_only_matching_and_sorted() {
        let store = fresh_store();
        let mut r1 = sample_run("R-1");
        r1.workflow_name = "z-late".into();
        r1.status = RunStatus::Running;
        let mut r2 = sample_run("R-2");
        r2.workflow_name = "a-early".into();
        r2.status = RunStatus::Running;
        let mut r3 = sample_run("R-3");
        r3.status = RunStatus::Completed;
        store.insert_run(&r1).unwrap();
        store.insert_run(&r2).unwrap();
        store.insert_run(&r3).unwrap();

        let listing = store.list_runs_by_status(RunStatus::Running).unwrap();
        assert_eq!(listing.len(), 2);
        // Sorted by (workflow_name, run_id) per the deterministic ordering.
        assert_eq!(listing[0].workflow_name, "a-early");
        assert_eq!(listing[1].workflow_name, "z-late");
    }

    // ── step_checkpoints CRUD ──

    #[test]
    fn upsert_step_checkpoint_inserts_new() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        store
            .upsert_step_checkpoint(&sample_checkpoint("R-1", "S-1", StepStatus::Running))
            .unwrap();
        let cps = store.list_step_checkpoints("R-1").unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].step_id, "S-1");
        assert_eq!(cps[0].status, StepStatus::Running);
    }

    #[test]
    fn upsert_step_checkpoint_updates_existing() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        store
            .upsert_step_checkpoint(&sample_checkpoint("R-1", "S-1", StepStatus::Running))
            .unwrap();
        // Same (run_id, step_id), now Completed with output.
        let mut completed = sample_checkpoint("R-1", "S-1", StepStatus::Completed);
        completed.output_json = Some(r#"{"ok":true}"#.into());
        completed.output_hash = Some("sha256:beef".into());
        completed.ended_at_ms = Some(1_700_000_002_000);
        store.upsert_step_checkpoint(&completed).unwrap();

        let cps = store.list_step_checkpoints("R-1").unwrap();
        assert_eq!(cps.len(), 1, "upsert must not duplicate");
        assert_eq!(cps[0].status, StepStatus::Completed);
        assert_eq!(cps[0].output_hash.as_deref(), Some("sha256:beef"));
    }

    #[test]
    fn upsert_step_checkpoint_preserves_started_at_when_caller_passes_none() {
        // Regression test (review-driven): the natural caller pattern is
        // insert-as-Running with started_at=Some, then upsert-as-Completed
        // with started_at=None (the caller doesn't re-supply it). Without
        // COALESCE on the upsert, the second call would clobber
        // started_at to NULL — silent data loss.
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();

        // Step 1: insert with started_at = Some
        let mut running = sample_checkpoint("R-1", "S-1", StepStatus::Running);
        running.started_at_ms = Some(1_700_000_001_000);
        store.upsert_step_checkpoint(&running).unwrap();

        // Step 2: upsert as Completed without re-supplying started_at
        let mut completed = sample_checkpoint("R-1", "S-1", StepStatus::Completed);
        completed.started_at_ms = None;
        completed.ended_at_ms = Some(1_700_000_002_000);
        completed.output_json = Some(r#"{"ok":true}"#.into());
        store.upsert_step_checkpoint(&completed).unwrap();

        let cps = store.list_step_checkpoints("R-1").unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(
            cps[0].started_at_ms,
            Some(1_700_000_001_000),
            "started_at must be preserved when caller passes None on upsert"
        );
        assert_eq!(cps[0].ended_at_ms, Some(1_700_000_002_000));
        assert_eq!(cps[0].status, StepStatus::Completed);
    }

    #[test]
    fn upsert_step_checkpoint_preserves_output_when_caller_passes_none() {
        // Companion regression: same COALESCE behavior for output_json
        // and output_hash. A caller updating only the status (e.g. a
        // post-completion housekeeping pass) must not lose the output.
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();

        let mut completed = sample_checkpoint("R-1", "S-1", StepStatus::Completed);
        completed.output_json = Some(r#"{"value":42}"#.into());
        completed.output_hash = Some("sha256:cafe".into());
        store.upsert_step_checkpoint(&completed).unwrap();

        // Post-hoc status flip (hypothetical) — outputs must persist.
        let status_only = sample_checkpoint("R-1", "S-1", StepStatus::Failed);
        // status_only.output_* are None by default
        store.upsert_step_checkpoint(&status_only).unwrap();

        let cps = store.list_step_checkpoints("R-1").unwrap();
        assert_eq!(cps[0].output_json.as_deref(), Some(r#"{"value":42}"#));
        assert_eq!(cps[0].output_hash.as_deref(), Some("sha256:cafe"));
        assert_eq!(cps[0].status, StepStatus::Failed);
    }

    #[test]
    fn list_step_checkpoints_ordered_deterministically() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        // Insert in non-sorted order; query must return sorted.
        for sid in ["S-3", "S-1", "S-2"] {
            store
                .upsert_step_checkpoint(&sample_checkpoint("R-1", sid, StepStatus::Pending))
                .unwrap();
        }
        let cps = store.list_step_checkpoints("R-1").unwrap();
        let ids: Vec<&str> = cps.iter().map(|c| c.step_id.as_str()).collect();
        assert_eq!(ids, vec!["S-1", "S-2", "S-3"]);
    }

    // ── foreign keys = ON (the silent-footgun PRAGMA) ──

    #[test]
    fn foreign_keys_cascade_works() {
        // The PRAGMA foreign_keys = ON line is the critical one — without
        // it, `ON DELETE CASCADE` is silently inert and orphan rows
        // accumulate forever. This test locks the cascade behavior.
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        store
            .upsert_step_checkpoint(&sample_checkpoint("R-1", "S-1", StepStatus::Pending))
            .unwrap();
        store
            .upsert_step_checkpoint(&sample_checkpoint("R-1", "S-2", StepStatus::Pending))
            .unwrap();

        // Delete the parent run — children must cascade.
        store
            .conn
            .execute("DELETE FROM runs WHERE run_id = ?1", params!["R-1"])
            .unwrap();

        let orphans = store.list_step_checkpoints("R-1").unwrap();
        assert!(
            orphans.is_empty(),
            "ON DELETE CASCADE failed — foreign_keys PRAGMA is OFF? \
             Got orphan checkpoints: {orphans:?}"
        );
    }

    #[test]
    fn step_checkpoint_without_parent_run_fails_fk_check() {
        // Negative companion to the cascade test: inserting a checkpoint
        // whose parent run doesn't exist must fail the FK constraint
        // (proves the constraint is actually being enforced).
        let store = fresh_store();
        let err = store
            .upsert_step_checkpoint(&sample_checkpoint("MISSING", "S-1", StepStatus::Pending))
            .expect_err("FK violation expected");
        assert!(matches!(err, PersistenceError::Sqlite(_)));
    }

    // ── retry policy ──

    #[test]
    fn with_busy_retry_succeeds_immediately_on_ok() {
        let mut calls = 0;
        let result: Result<i32, PersistenceError> = with_busy_retry(|| {
            calls += 1;
            Ok(42)
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls, 1, "no retry on success");
    }

    #[test]
    fn with_busy_retry_propagates_non_busy_errors_immediately() {
        let mut calls = 0;
        let result: Result<i32, PersistenceError> = with_busy_retry(|| {
            calls += 1;
            // SqliteFailure with code != DatabaseBusy → no retry.
            Err(PersistenceError::Sqlite(SqlError::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some("unique violation".into()),
            )))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1, "non-busy errors must not retry");
    }
}
