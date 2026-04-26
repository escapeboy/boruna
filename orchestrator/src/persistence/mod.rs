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

use rusqlite::{params, Connection, Error as SqlError, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Wire-format version of the on-disk schema.
///
/// History:
/// - **v1** (sprint `0.3-S2a`): initial schema — `runs`,
///   `step_checkpoints`, `schema_version`.
/// - **v2** (sprint `0.3-S11`): adds `step_checkpoints.attempt_count`
///   column to record retry attempts per step. Migrated additively
///   via `schema_v1_to_v2.sql` (`ALTER TABLE ADD COLUMN`); existing
///   rows default to 1.
///
/// Bumped on EITHER additive or breaking changes when there's a
/// migration to run on existing databases (the bump signals "v_n
/// builds need v_n schema; older builds refuse to open"). For
/// fresh databases, [`SCHEMA_V1_SQL`] is the canonical creation
/// script and reflects the latest schema; older versions matter
/// only for the migration chain.
pub const SCHEMA_VERSION: i64 = 2;

/// Canonical creation script for fresh databases. Reflects the
/// LATEST schema (currently v2 — includes `attempt_count`). Applied
/// via `IF NOT EXISTS`, so re-opens are idempotent and existing
/// databases see no DDL from this script. Migrations from older
/// versions (v1 → v2 etc.) run separately via the
/// `SCHEMA_V*_TO_V*_SQL` chain in `init()`.
const SCHEMA_V1_SQL: &str = include_str!("schema_v1.sql");

/// v1 → v2 migration: adds `step_checkpoints.attempt_count`. Applied
/// in [`RunCheckpointStore::init`] when opening a v1 database.
const SCHEMA_V1_TO_V2_SQL: &str = include_str!("schema_v1_to_v2.sql");

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
    /// Step is paused waiting for an external event (sprint 0.3-S15).
    /// Resume after `boruna workflow trigger` to advance with the
    /// trigger payload as the step's output.
    AwaitingExternalEvent,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Running => "running",
            StepStatus::Completed => "completed",
            StepStatus::Failed => "failed",
            StepStatus::AwaitingApproval => "awaiting_approval",
            StepStatus::AwaitingExternalEvent => "awaiting_external_event",
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
            "awaiting_external_event" => Some(StepStatus::AwaitingExternalEvent),
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

/// Outcome of [`RunCheckpointStore::insert_run_with_derived_id_skip_if_in_flight`].
///
/// `Inserted(run_id)` — the new run was inserted and is now Running.
/// `Skipped(prior_row)` — a prior in-flight run was found; the new
/// run was NOT inserted. The carried `RunRow` is the prior run that
/// caused the skip (the FIRST one matched by deterministic
/// `(workflow_name, run_id)` order).
///
/// Introduced in `0.3-S10` to close the race window in the prior
/// 0.3-S7 two-call check-then-insert flow.
#[derive(Debug, Clone)]
pub enum InsertOrSkip {
    Inserted(String),
    Skipped(RunRow),
}

/// Outcome of [`RunCheckpointStore::commit_external_trigger`] (sprint
/// 0.3-S16). Distinguishes the three terminal states of the atomic
/// trigger-commit operation so the caller can decide whether to retry,
/// surface an error, or treat the write as successful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerCommitOutcome {
    /// Both the metadata CAS and the checkpoint transition committed.
    Committed,
    /// The on-disk metadata moved between the caller's read and the
    /// CAS-write. Caller should re-read metadata, re-validate, and
    /// retry within its CAS-retry budget.
    MetadataChanged,
    /// The step's checkpoint was not in `awaiting_external_event`
    /// state (e.g., already `completed`, or a concurrent process
    /// transitioned it to `running` and hasn't yet re-paused). Caller
    /// surfaces a typed error to the operator.
    CheckpointStateMismatch { current_status: String },
}

/// Replay-verified subset of a [`RunRow`]. Audit, replay, and any code
/// path whose output enters a hash chain MUST consume `RunRecord` rather
/// than `RunRow`. Operational columns (`started_at_ms`, `updated_at_ms`,
/// transient `status` values) are structurally absent so that
/// `ORDER BY started_at` and similar non-deterministic sorts cannot
/// compile against this type.
///
/// `terminal_status` is `Some` only for terminal lifecycle states
/// (`Completed`, `Failed`). Transient states map to `None` — replay code
/// MUST branch on `Some(_)` to assert a run actually finished, never
/// pattern-match a `RunStatus` directly.
///
/// Introduced in sprint `0.3-S2b` per the H1 review finding from `0.3-S2a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    pub run_id: String,
    pub workflow_name: String,
    /// **REPLAY-VERIFIED** — SHA-256 of the on-disk workflow definition.
    pub workflow_hash: String,
    /// `Some` only for terminal states (`Completed`, `Failed`); `None`
    /// otherwise. Replay code reads this to decide whether to compare
    /// outputs.
    pub terminal_status: Option<RunStatus>,
    /// **REPLAY-VERIFIED** — serialized capability `Policy`.
    pub policy_json: String,
    /// Caller-defined free-form JSON (workflow_dir, inputs_hash, etc).
    pub metadata_json: String,
}

/// Operational-only subset of a [`RunRow`]. Status dashboards, progress
/// trackers, and alerting consume this. NEVER feeds an audit hash; NEVER
/// orders a replay-relevant query.
///
/// `transient_status` carries any [`RunStatus`] including transients
/// (`Running`, `Paused`). Audit code that needs to assert a run completed
/// MUST go through [`RunRecord::terminal_status`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOperational {
    pub run_id: String,
    pub transient_status: RunStatus,
    /// **OPERATIONAL ONLY** — Unix epoch ms.
    pub started_at_ms: i64,
    /// **OPERATIONAL ONLY** — Unix epoch ms.
    pub updated_at_ms: i64,
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
    /// Number of attempts the step took to reach its terminal state.
    /// `1` = first-try success or single-attempt failure; `>1` = retry
    /// policy fired (sprint `0.3-S5`). **OPERATIONAL ONLY** —
    /// wall-clock-keyed (depends on whether transient failures
    /// happened); never feeds an audit hash. Added in schema v2
    /// (sprint `0.3-S11`). For pre-v2 rows, the migration defaults
    /// to `1`.
    pub attempt_count: u32,
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

        // Apply the canonical schema with `IF NOT EXISTS` — fresh
        // databases get the latest shape directly; existing databases
        // see no DDL from this script (every CREATE is guarded). The
        // INSERT OR IGNORE on `schema_version` lays down v1's row on
        // a fresh DB and is a no-op on existing ones.
        conn.execute_batch(SCHEMA_V1_SQL)?;

        // Read the on-disk version. On a fresh DB this is 1 (laid
        // down by the INSERT OR IGNORE in SCHEMA_V1_SQL). On an
        // existing DB, this is whatever the last build wrote.
        let on_disk: i64 = conn.query_row(
            "SELECT version FROM schema_version WHERE id = 1",
            [],
            |row| row.get(0),
        )?;

        // Migration chain. Apply each `if v < N` block in order under
        // a single transaction so a partial migration rolls back
        // cleanly. The version row is bumped INSIDE the same
        // transaction so partial state isn't visible to a concurrent
        // reader.
        //
        // Special case: a FRESH database just laid down by
        // SCHEMA_V1_SQL already has the latest schema (v2 columns are
        // included in that script for fresh-DB convenience). The
        // migration ALTER TABLE would fail on "duplicate column".
        // To detect fresh-vs-existing, we check whether the
        // `attempt_count` column already exists; if so, skip the
        // v1→v2 ALTER and just bump the version row.
        if on_disk < 2 {
            let has_attempt_count = column_exists(&conn, "step_checkpoints", "attempt_count")?;
            let tx = conn.unchecked_transaction()?;
            if !has_attempt_count {
                tx.execute_batch(SCHEMA_V1_TO_V2_SQL)?;
            }
            tx.execute(
                "UPDATE schema_version SET version = ?1 WHERE id = 1",
                params![2_i64],
            )?;
            tx.commit()?;
        }

        // Final version check — refuses to open a database that
        // somehow ended up at a version we don't recognize (future
        // build wrote v3 to disk, current build only knows v2).
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

    /// Read the raw `metadata_json` column for a run. Returned verbatim
    /// — callers (the `WorkflowRunner`) deserialize into typed shapes.
    /// `Ok(None)` if the run doesn't exist.
    ///
    /// Introduced in `0.3-S2c` to support the approve / reject CLI paths
    /// that round-trip the metadata blob.
    pub fn get_run_metadata(&self, run_id: &str) -> Result<Option<String>, PersistenceError> {
        let mut stmt = self
            .conn
            .prepare("SELECT metadata_json FROM runs WHERE run_id = ?1")?;
        let mut rows = stmt.query(params![run_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// Update only the `metadata_json` column (and the operational
    /// `updated_at` timestamp). Returns `PersistenceError::NotFound` for
    /// an unknown `run_id` — same silent-no-op-rejection pattern as
    /// [`update_run_status`].
    ///
    /// Wrapped in `BEGIN IMMEDIATE` so a concurrent approve/reject
    /// operation against the same row serializes correctly. Read-modify-
    /// write callers (e.g. `record_approval_decision`) should hold their
    /// own outer atomicity if they need read+write coordinated.
    pub fn update_run_metadata(
        &self,
        run_id: &str,
        metadata_json: &str,
        updated_at_ms: i64,
    ) -> Result<(), PersistenceError> {
        with_busy_retry(|| {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
            let body = || -> Result<(), PersistenceError> {
                let rows_affected = self.conn.execute(
                    "UPDATE runs SET metadata_json = ?1, updated_at = ?2 WHERE run_id = ?3",
                    params![metadata_json, updated_at_ms, run_id],
                )?;
                if rows_affected == 0 {
                    return Err(PersistenceError::NotFound {
                        entity: "run",
                        key: run_id.to_string(),
                    });
                }
                Ok(())
            };
            match body() {
                Ok(()) => {
                    self.conn.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })
    }

    /// Compare-and-swap variant of [`update_run_metadata`]. Atomically
    /// updates `metadata_json` ONLY if the on-disk value still equals
    /// `expected_prior_json` byte-for-byte. Returns `Ok(true)` on a
    /// successful swap, `Ok(false)` if the metadata has drifted (a
    /// concurrent writer changed it). The unique-row UPDATE is wrapped
    /// in `BEGIN IMMEDIATE` + busy-retry like every other writer.
    ///
    /// Used by `record_approval_decision` to close the read-validate-
    /// write race that the prior 3-transaction implementation had: two
    /// concurrent `approve` calls could both pass the in-memory prior-
    /// decision check (each reading their own pre-write snapshot) and
    /// then race the UPDATE, with the loser silently overwriting the
    /// winner. With CAS, the loser's UPDATE matches 0 rows; the caller
    /// re-reads and surfaces the typed `StepAlreadyDecided` error.
    ///
    /// Returns `PersistenceError::NotFound` if the `run_id` does not
    /// exist (regardless of expected_prior_json).
    pub fn compare_and_swap_metadata(
        &self,
        run_id: &str,
        expected_prior_json: &str,
        new_metadata_json: &str,
        updated_at_ms: i64,
    ) -> Result<bool, PersistenceError> {
        with_busy_retry(|| {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
            let body = || -> Result<bool, PersistenceError> {
                // Verify the run exists at all.
                let exists: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM runs WHERE run_id = ?1",
                    params![run_id],
                    |row| row.get(0),
                )?;
                if exists == 0 {
                    return Err(PersistenceError::NotFound {
                        entity: "run",
                        key: run_id.to_string(),
                    });
                }
                let rows_affected = self.conn.execute(
                    "UPDATE runs SET metadata_json = ?1, updated_at = ?2 \
                     WHERE run_id = ?3 AND metadata_json = ?4",
                    params![
                        new_metadata_json,
                        updated_at_ms,
                        run_id,
                        expected_prior_json
                    ],
                )?;
                Ok(rows_affected > 0)
            };
            match body() {
                Ok(swapped) => {
                    self.conn.execute_batch("COMMIT")?;
                    Ok(swapped)
                }
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })
    }

    /// Atomic commit of an external-trigger event (sprint 0.3-S16):
    /// CAS-update the run's `metadata_json` AND transition the named
    /// step's checkpoint from `awaiting_external_event` to `completed`
    /// (with the synthesized output) in a single `BEGIN IMMEDIATE`
    /// transaction.
    ///
    /// **Closes the TOCTOU race** between [`compare_and_swap_metadata`]
    /// and a concurrent runner's `mark_step_running_clearing_output`.
    /// Without this method, a webhook-driven trigger could commit its
    /// metadata write at the same instant as a `boruna workflow resume`
    /// transitioned the same step to `running` — the next resume's
    /// trigger-sentinel pass would then see the checkpoint past the
    /// gate and silently discard the payload. SQLite's `BEGIN IMMEDIATE`
    /// acquires a write lock that blocks concurrent writers until this
    /// transaction commits or rolls back, so the checkpoint state
    /// observed inside this method is the authoritative state the
    /// metadata write commits against.
    ///
    /// Returns:
    /// - [`TriggerCommitOutcome::Committed`] — both writes committed.
    /// - [`TriggerCommitOutcome::MetadataChanged`] — the metadata_json
    ///   on disk did not match `expected_prior_metadata`. Caller should
    ///   re-read metadata, re-validate, and retry.
    /// - [`TriggerCommitOutcome::CheckpointStateMismatch`] — the
    ///   step's checkpoint is not in `awaiting_external_event` state.
    ///   Caller should surface a typed error to the operator.
    /// - `Err(NotFound)` — the run does not exist.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_external_trigger(
        &self,
        run_id: &str,
        step_id: &str,
        expected_prior_metadata: &str,
        new_metadata_json: &str,
        output_json: &str,
        output_hash: &str,
        triggered_at_ms: i64,
    ) -> Result<TriggerCommitOutcome, PersistenceError> {
        with_busy_retry(|| {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
            let body = || -> Result<TriggerCommitOutcome, PersistenceError> {
                // Verify the run exists.
                let exists: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM runs WHERE run_id = ?1",
                    params![run_id],
                    |row| row.get(0),
                )?;
                if exists == 0 {
                    return Err(PersistenceError::NotFound {
                        entity: "run",
                        key: run_id.to_string(),
                    });
                }

                // Read checkpoint status under write-lock. A concurrent
                // resume's `mark_step_running_clearing_output` is blocked
                // by BEGIN IMMEDIATE, so this snapshot is authoritative.
                let cp_status: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT status FROM step_checkpoints \
                         WHERE run_id = ?1 AND step_id = ?2",
                        params![run_id, step_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let current_status = match cp_status {
                    Some(s) => s,
                    None => {
                        return Ok(TriggerCommitOutcome::CheckpointStateMismatch {
                            current_status: "missing".to_string(),
                        });
                    }
                };
                if current_status != StepStatus::AwaitingExternalEvent.as_str() {
                    return Ok(TriggerCommitOutcome::CheckpointStateMismatch { current_status });
                }

                // CAS the metadata. If the on-disk metadata moved while
                // the caller was validating its read snapshot, surface
                // MetadataChanged so the caller's CAS-retry loop can
                // re-validate.
                let metadata_swapped = self.conn.execute(
                    "UPDATE runs SET metadata_json = ?1, updated_at = ?2 \
                     WHERE run_id = ?3 AND metadata_json = ?4",
                    params![
                        new_metadata_json,
                        triggered_at_ms,
                        run_id,
                        expected_prior_metadata
                    ],
                )?;
                if metadata_swapped == 0 {
                    return Ok(TriggerCommitOutcome::MetadataChanged);
                }

                // Transition the checkpoint to Completed with the
                // synthesized output. The `started_at` column is
                // preserved (we don't touch it). `attempt_count` stays
                // at whatever the gate-entry upsert wrote (typically 1).
                self.conn.execute(
                    "UPDATE step_checkpoints SET \
                       status      = 'completed', \
                       output_json = ?1, \
                       output_hash = ?2, \
                       ended_at    = ?3, \
                       error_msg   = NULL \
                     WHERE run_id = ?4 AND step_id = ?5",
                    params![output_json, output_hash, triggered_at_ms, run_id, step_id],
                )?;

                Ok(TriggerCommitOutcome::Committed)
            };
            match body() {
                Ok(TriggerCommitOutcome::Committed) => {
                    self.conn.execute_batch("COMMIT")?;
                    Ok(TriggerCommitOutcome::Committed)
                }
                Ok(other) => {
                    // No mutation happened (or only a CAS that didn't
                    // match): roll back so we don't accidentally commit
                    // a partial write.
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Ok(other)
                }
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })
    }

    /// List ALL runs, ordered by `(workflow_name, run_id)` —
    /// deterministic, not timestamp-keyed. Use [`list_runs_by_status`]
    /// for the filtered case.
    pub fn list_runs(&self) -> Result<Vec<RunRow>, PersistenceError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, workflow_name, workflow_hash, status, started_at, updated_at, policy_json, metadata_json \
             FROM runs ORDER BY workflow_name, run_id",
        )?;
        let rows = stmt
            .query_map([], parse_run_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
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

    /// List in-flight (Running or Paused) runs for a given
    /// `workflow_hash`, ordered by `(workflow_name, run_id)`. Empty
    /// when no run is currently active for that workflow.
    ///
    /// Used by the `--skip-if-running` CLI flag (sprint `0.3-S7`) to
    /// decide whether to skip a new invocation when a prior run is
    /// still active. Cron-driven scheduling pattern:
    ///
    /// ```text
    /// 0 2 * * * boruna workflow run /path/to/wf --skip-if-running --data-dir /var/lib/boruna
    /// ```
    pub fn list_in_flight_runs_for_workflow(
        &self,
        workflow_hash: &str,
    ) -> Result<Vec<RunRow>, PersistenceError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, workflow_name, workflow_hash, status, started_at, updated_at, policy_json, metadata_json \
             FROM runs WHERE workflow_hash = ?1 AND status IN ('running', 'paused') \
             ORDER BY workflow_name, run_id",
        )?;
        let rows = stmt
            .query_map(params![workflow_hash], parse_run_row)?
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
                 (run_id, step_id, status, output_json, output_hash, started_at, ended_at, error_msg, attempt_count) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT(run_id, step_id) DO UPDATE SET \
                   status        = excluded.status, \
                   output_json   = COALESCE(excluded.output_json, step_checkpoints.output_json), \
                   output_hash   = COALESCE(excluded.output_hash, step_checkpoints.output_hash), \
                   started_at    = COALESCE(excluded.started_at,  step_checkpoints.started_at), \
                   ended_at      = excluded.ended_at, \
                   error_msg     = excluded.error_msg, \
                   attempt_count = excluded.attempt_count",
                params![
                    cp.run_id,
                    cp.step_id,
                    cp.status.as_str(),
                    cp.output_json,
                    cp.output_hash,
                    cp.started_at_ms,
                    cp.ended_at_ms,
                    cp.error_msg,
                    cp.attempt_count,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Replay-verified view of a run row. See [`RunRecord`] for why this
    /// exists separately from [`get_run`].
    ///
    /// `terminal_status` is `Some` only when the persisted status is
    /// `Completed` or `Failed`; transient states (`Running`, `Paused`)
    /// return `None` so replay code cannot accidentally treat a still-
    /// running workflow as comparable.
    pub fn get_run_record(&self, run_id: &str) -> Result<Option<RunRecord>, PersistenceError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, workflow_name, workflow_hash, status, policy_json, metadata_json \
             FROM runs WHERE run_id = ?1",
        )?;
        let mut rows = stmt.query(params![run_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(parse_run_record(row)?)),
            None => Ok(None),
        }
    }

    /// Operational view of a run row. See [`RunOperational`].
    pub fn get_run_operational(
        &self,
        run_id: &str,
    ) -> Result<Option<RunOperational>, PersistenceError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, status, started_at, updated_at \
             FROM runs WHERE run_id = ?1",
        )?;
        let mut rows = stmt.query(params![run_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(parse_run_operational(row)?)),
            None => Ok(None),
        }
    }

    /// Count of existing runs for a given `workflow_hash`. Used by
    /// `0.3-S2b`'s deterministic `run_id` derivation as the per-workflow
    /// counter input. Inside a `BEGIN IMMEDIATE` transaction this read +
    /// the subsequent insert are atomic against concurrent writers.
    pub fn count_runs_for_workflow(&self, workflow_hash: &str) -> Result<i64, PersistenceError> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM runs WHERE workflow_hash = ?1")?;
        let count: i64 = stmt.query_row(params![workflow_hash], |row| row.get(0))?;
        Ok(count)
    }

    /// Derive a run_id and insert a new run atomically. The counter is
    /// read inside a `BEGIN IMMEDIATE` transaction so concurrent writers
    /// see distinct counter values (and therefore distinct `run_id`s)
    /// without a UNIQUE collision.
    ///
    /// Returns the freshly-derived `run_id`. The caller passes
    /// `workflow_hash` and `inputs_hash` as the deterministic inputs;
    /// `policy_json`, `metadata_json`, and timestamps round out the row.
    ///
    /// **Determinism contract:** given an empty database (or one with no
    /// prior runs of this `workflow_hash`), the returned `run_id` is
    /// bit-identical across machines for the same `(workflow_hash,
    /// inputs_hash)`. The wall-clock timestamps stored in the row are
    /// operational-only and do NOT feed the `run_id` derivation.
    pub fn insert_run_with_derived_id(
        &self,
        workflow_name: &str,
        workflow_hash: &str,
        inputs_hash: &str,
        policy_json: &str,
        metadata_json: &str,
        started_at_ms: i64,
    ) -> Result<String, PersistenceError> {
        with_busy_retry(|| {
            // `BEGIN IMMEDIATE` acquires the RESERVED writer lock at
            // transaction start so the `SELECT COUNT(*)` below is
            // serialized against any concurrent inserter.
            // `Connection::unchecked_transaction()` defaults to DEFERRED,
            // which would let two writers both observe the same counter
            // value, derive the same `run_id`, and race to INSERT —
            // producing a UNIQUE constraint violation (NOT a busy retry)
            // for the loser. Reviewed in 0.3-S2b. We can't use
            // `Connection::transaction_with_behavior` because that
            // requires `&mut self` and the store holds `&self` methods;
            // an explicit BEGIN IMMEDIATE / COMMIT / ROLLBACK pair works
            // on `&self`. The `with_busy_retry` wrapper retries this
            // whole closure on `SQLITE_BUSY`/`SQLITE_LOCKED` so an
            // immediate-lock contention surfaces correctly as a busy
            // retry, not a UNIQUE collision.
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
            let body = || -> Result<String, PersistenceError> {
                let counter: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM runs WHERE workflow_hash = ?1",
                    params![workflow_hash],
                    |row| row.get(0),
                )?;
                let run_id = derive_run_id(workflow_hash, inputs_hash, counter);
                self.conn.execute(
                    "INSERT INTO runs \
                     (run_id, workflow_name, workflow_hash, status, started_at, updated_at, policy_json, metadata_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        run_id,
                        workflow_name,
                        workflow_hash,
                        RunStatus::Running.as_str(),
                        started_at_ms,
                        started_at_ms,
                        policy_json,
                        metadata_json,
                    ],
                )?;
                Ok(run_id)
            };
            match body() {
                Ok(run_id) => {
                    self.conn.execute_batch("COMMIT")?;
                    Ok(run_id)
                }
                Err(e) => {
                    // Best-effort rollback. If ROLLBACK itself fails, the
                    // connection is in a degraded state but we still
                    // surface the underlying cause.
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })
    }

    /// Atomic check-then-insert for the `--skip-if-running` cron pattern.
    ///
    /// In one `BEGIN IMMEDIATE` transaction:
    /// 1. Query for any in-flight (`Running` or `Paused`) run with the
    ///    same `workflow_hash`. If any: ROLLBACK and return
    ///    `Ok(InsertOrSkip::Skipped(prior_row))`.
    /// 2. Else: derive a fresh `run_id` (per [`derive_run_id`]) and
    ///    INSERT a new run row. COMMIT. Return
    ///    `Ok(InsertOrSkip::Inserted(run_id))`.
    ///
    /// Closes the race window in the prior 0.3-S7 two-call flow
    /// ([`list_in_flight_runs_for_workflow`] then
    /// [`insert_run_with_derived_id`]): two concurrent processes
    /// could both pass the SELECT and both INSERT, producing
    /// overlapping in-flight runs the operator wanted skipped.
    ///
    /// Both sub-operations run under the writer lock acquired by
    /// `BEGIN IMMEDIATE`, so the second process either:
    /// - sees the first's just-inserted row (and gets Skipped), or
    /// - blocks on the writer lock (handled by [`with_busy_retry`])
    ///   until the first commits, then sees the row.
    ///
    /// Reviewed in 0.3-S10 (carried-forward debt from 0.3-S7).
    pub fn insert_run_with_derived_id_skip_if_in_flight(
        &self,
        workflow_name: &str,
        workflow_hash: &str,
        inputs_hash: &str,
        policy_json: &str,
        metadata_json: &str,
        started_at_ms: i64,
    ) -> Result<InsertOrSkip, PersistenceError> {
        with_busy_retry(|| {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
            let body = || -> Result<InsertOrSkip, PersistenceError> {
                // Step 1: check for in-flight prior. We fetch the
                // first matching row by deterministic order
                // (workflow_name, run_id) so the returned "prior" is
                // stable across processes.
                let mut stmt = self.conn.prepare(
                    "SELECT run_id, workflow_name, workflow_hash, status, started_at, updated_at, policy_json, metadata_json \
                     FROM runs WHERE workflow_hash = ?1 AND status IN ('running', 'paused') \
                     ORDER BY workflow_name, run_id LIMIT 1",
                )?;
                let prior: Option<RunRow> = stmt
                    .query_row(params![workflow_hash], parse_run_row)
                    .map(Some)
                    .or_else(|e| match e {
                        SqlError::QueryReturnedNoRows => Ok(None),
                        other => Err(other),
                    })?;
                if let Some(row) = prior {
                    return Ok(InsertOrSkip::Skipped(row));
                }
                // Step 2: derive run_id + INSERT — same logic as
                // `insert_run_with_derived_id`, but folded into the
                // outer transaction.
                let counter: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM runs WHERE workflow_hash = ?1",
                    params![workflow_hash],
                    |row| row.get(0),
                )?;
                let run_id = derive_run_id(workflow_hash, inputs_hash, counter);
                self.conn.execute(
                    "INSERT INTO runs \
                     (run_id, workflow_name, workflow_hash, status, started_at, updated_at, policy_json, metadata_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        run_id,
                        workflow_name,
                        workflow_hash,
                        RunStatus::Running.as_str(),
                        started_at_ms,
                        started_at_ms,
                        policy_json,
                        metadata_json,
                    ],
                )?;
                Ok(InsertOrSkip::Inserted(run_id))
            };
            match body() {
                Ok(outcome) => {
                    self.conn.execute_batch("COMMIT")?;
                    Ok(outcome)
                }
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })
    }

    /// Reset a step checkpoint's `output_json`/`output_hash` to NULL while
    /// flipping it to `Running`. Required by sprint `0.3-S2b` resume
    /// semantics: when a previously-Completed-but-now-being-re-executed
    /// step (which can't happen today) or a previously-Running step is
    /// re-attempted, COALESCE-on-conflict in `upsert_step_checkpoint`
    /// would preserve stale outputs. This explicit clear-on-running keeps
    /// the on-disk invariant "non-Completed rows have null output."
    /// Idempotent — safe to call on a row that doesn't exist.
    pub fn mark_step_running_clearing_output(
        &self,
        run_id: &str,
        step_id: &str,
        started_at_ms: i64,
    ) -> Result<(), PersistenceError> {
        with_busy_retry(|| {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
            let body = || -> Result<(), PersistenceError> {
                // attempt_count: leave UNCHANGED on conflict (the
                // running checkpoint is mid-attempt; the terminal
                // upsert later carries the final attempt count).
                // For a fresh insert, default to 1 — this is a NEW
                // attempt; the post-execution upsert overwrites with
                // the actual count if retries fired.
                self.conn.execute(
                    "INSERT INTO step_checkpoints \
                     (run_id, step_id, status, output_json, output_hash, started_at, ended_at, error_msg, attempt_count) \
                     VALUES (?1, ?2, 'running', NULL, NULL, ?3, NULL, NULL, 1) \
                     ON CONFLICT(run_id, step_id) DO UPDATE SET \
                       status      = 'running', \
                       output_json = NULL, \
                       output_hash = NULL, \
                       started_at  = COALESCE(step_checkpoints.started_at, excluded.started_at), \
                       ended_at    = NULL, \
                       error_msg   = NULL",
                    params![run_id, step_id, started_at_ms],
                )?;
                Ok(())
            };
            match body() {
                Ok(()) => {
                    self.conn.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
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
            "SELECT run_id, step_id, status, output_json, output_hash, started_at, ended_at, error_msg, attempt_count \
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
/// Return true if the named column exists on the given table.
/// Uses `PRAGMA table_info(<table>)` and checks for the column name.
/// Used by the v1→v2 migration to distinguish a fresh database
/// (where the canonical creation script already includes the v2
/// columns) from an actual v1 database that needs the ALTER TABLE.
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, PersistenceError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for r in rows {
        if r? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

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

/// Deterministic `run_id` derivation per ADR 001 + project-conventions §16.
///
/// `run_id = first_16_hex_chars(sha256(workflow_hash || ":" || inputs_hash || ":" || counter_LE_8))`.
///
/// **Domain separators (`:`) prevent collision** between e.g. a workflow
/// named `"foo:bar"` with no inputs and a workflow named `"foo"` with
/// inputs `"bar"`. The little-endian 8-byte counter encoding is fixed-width
/// so the input to the hash is a stable bit string.
///
/// Wall-clock is intentionally NOT an input. Two independent fresh
/// databases producing the same `(workflow_hash, inputs_hash, counter)`
/// triple yield the same `run_id` — the determinism property the platform
/// relies on for cross-machine replay.
pub fn derive_run_id(workflow_hash: &str, inputs_hash: &str, counter: i64) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(workflow_hash.as_bytes());
    hasher.update(b":");
    hasher.update(inputs_hash.as_bytes());
    hasher.update(b":");
    hasher.update(counter.to_le_bytes());
    let digest = hasher.finalize();
    // First 8 bytes → 16 hex chars. Plenty of entropy for collision
    // avoidance in a single-tenant store while staying short enough to
    // be human-pasteable on a CLI line.
    let mut out = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write;
        write!(out, "{b:02x}").expect("writing to String never fails");
    }
    out
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

fn parse_run_record(row: &rusqlite::Row<'_>) -> Result<RunRecord, SqlError> {
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
    let terminal_status = match status {
        RunStatus::Completed | RunStatus::Failed => Some(status),
        RunStatus::Running | RunStatus::Paused => None,
    };
    Ok(RunRecord {
        run_id: row.get(0)?,
        workflow_name: row.get(1)?,
        workflow_hash: row.get(2)?,
        terminal_status,
        policy_json: row.get(4)?,
        metadata_json: row.get(5)?,
    })
}

fn parse_run_operational(row: &rusqlite::Row<'_>) -> Result<RunOperational, SqlError> {
    let status_str: String = row.get(1)?;
    let transient_status = RunStatus::parse_str(&status_str).ok_or_else(|| {
        SqlError::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown run status '{status_str}'"),
            )),
        )
    })?;
    Ok(RunOperational {
        run_id: row.get(0)?,
        transient_status,
        started_at_ms: row.get(2)?,
        updated_at_ms: row.get(3)?,
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
        // Read as i64 then narrow — SQLite has no u32 affinity.
        // The default-1 in the schema and the migration ensures
        // every row has a value.
        attempt_count: row.get::<_, i64>(8)? as u32,
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
            attempt_count: 1,
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

    // ── view structs (RunRecord / RunOperational) ──

    #[test]
    fn get_run_record_returns_none_for_missing() {
        let store = fresh_store();
        assert!(store.get_run_record("nope").unwrap().is_none());
    }

    #[test]
    fn get_run_record_terminal_status_some_for_completed() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        store
            .update_run_status("R-1", RunStatus::Completed, 1_700_000_500_000)
            .unwrap();
        let rec = store.get_run_record("R-1").unwrap().expect("present");
        assert_eq!(rec.terminal_status, Some(RunStatus::Completed));
        assert_eq!(rec.workflow_hash, "sha256:dead");
    }

    #[test]
    fn get_run_record_terminal_status_some_for_failed() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        store
            .update_run_status("R-1", RunStatus::Failed, 1_700_000_500_000)
            .unwrap();
        let rec = store.get_run_record("R-1").unwrap().expect("present");
        assert_eq!(rec.terminal_status, Some(RunStatus::Failed));
    }

    #[test]
    fn get_run_record_terminal_status_none_for_running() {
        // The structural guarantee: replay code must not see a transient
        // status. terminal_status = None means "not finished, do not
        // compare outputs".
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        let rec = store.get_run_record("R-1").unwrap().expect("present");
        assert_eq!(rec.terminal_status, None);
    }

    #[test]
    fn get_run_record_terminal_status_none_for_paused() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        store
            .update_run_status("R-1", RunStatus::Paused, 1_700_000_500_000)
            .unwrap();
        let rec = store.get_run_record("R-1").unwrap().expect("present");
        assert_eq!(rec.terminal_status, None);
    }

    #[test]
    fn get_run_operational_carries_transient_status() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        store
            .update_run_status("R-1", RunStatus::Paused, 1_700_000_500_000)
            .unwrap();
        let op = store.get_run_operational("R-1").unwrap().expect("present");
        assert_eq!(op.transient_status, RunStatus::Paused);
        assert_eq!(op.updated_at_ms, 1_700_000_500_000);
    }

    // ── workflow counter + derived run_id ──

    #[test]
    fn count_runs_for_workflow_zero_on_empty() {
        let store = fresh_store();
        assert_eq!(store.count_runs_for_workflow("anything").unwrap(), 0);
    }

    #[test]
    fn count_runs_for_workflow_increments_per_insert() {
        let store = fresh_store();
        let mut a = sample_run("A-1");
        a.workflow_hash = "wh-A".into();
        store.insert_run(&a).unwrap();
        let mut a2 = sample_run("A-2");
        a2.workflow_hash = "wh-A".into();
        store.insert_run(&a2).unwrap();
        let mut b = sample_run("B-1");
        b.workflow_hash = "wh-B".into();
        store.insert_run(&b).unwrap();
        assert_eq!(store.count_runs_for_workflow("wh-A").unwrap(), 2);
        assert_eq!(store.count_runs_for_workflow("wh-B").unwrap(), 1);
        assert_eq!(store.count_runs_for_workflow("wh-C").unwrap(), 0);
    }

    #[test]
    fn derive_run_id_is_deterministic() {
        // D-1: same inputs → same output.
        let a = derive_run_id("wh", "ih", 0);
        let b = derive_run_id("wh", "ih", 0);
        assert_eq!(a, b);
    }

    #[test]
    fn derive_run_id_changes_with_counter() {
        // D-2: counter participates.
        let a = derive_run_id("wh", "ih", 0);
        let b = derive_run_id("wh", "ih", 1);
        assert_ne!(a, b);
    }

    #[test]
    fn derive_run_id_changes_with_workflow_hash() {
        // D-3
        let a = derive_run_id("wh-1", "ih", 0);
        let b = derive_run_id("wh-2", "ih", 0);
        assert_ne!(a, b);
    }

    #[test]
    fn derive_run_id_changes_with_inputs_hash() {
        // D-4
        let a = derive_run_id("wh", "ih-1", 0);
        let b = derive_run_id("wh", "ih-2", 0);
        assert_ne!(a, b);
    }

    #[test]
    fn derive_run_id_golden_vector() {
        // D-5: locks the algorithm against accidental change. The expected
        // values were computed externally with:
        //   printf 'dead:beef:\x00\x00\x00\x00\x00\x00\x00\x00' | shasum -a 256 | head -c 16
        // Bumping the algorithm requires re-deriving these — that is the
        // intended friction.
        assert_eq!(derive_run_id("dead", "beef", 0), "11d9dfd019b20391");
        assert_eq!(derive_run_id("dead", "beef", 1), "e13240cbb376903b");
    }

    #[test]
    fn derive_run_id_is_16_hex_chars() {
        let id = derive_run_id("wh", "ih", 42);
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn insert_run_with_derived_id_atomic_counter() {
        // The whole point of the BEGIN IMMEDIATE: counter read + insert
        // are atomic, so two sequential inserts at counter=0 and counter=1
        // produce different run_ids deterministically.
        let store = fresh_store();
        let r1 = store
            .insert_run_with_derived_id("wf", "wh-X", "ih-1", "{}", "{}", 1_700_000_000_000)
            .unwrap();
        let r2 = store
            .insert_run_with_derived_id("wf", "wh-X", "ih-1", "{}", "{}", 1_700_000_000_000)
            .unwrap();
        assert_ne!(r1, r2);
        // r1 is counter=0, r2 is counter=1 — locked against the golden algorithm.
        assert_eq!(r1, derive_run_id("wh-X", "ih-1", 0));
        assert_eq!(r2, derive_run_id("wh-X", "ih-1", 1));
        // Both rows are present.
        assert_eq!(store.count_runs_for_workflow("wh-X").unwrap(), 2);
    }

    #[test]
    fn insert_run_with_derived_id_starts_running() {
        // Newly-inserted run must be in transient `Running` state, not
        // accidentally inserted as a terminal one.
        let store = fresh_store();
        let id = store
            .insert_run_with_derived_id("wf", "wh", "ih", "{}", r#"{"k":"v"}"#, 1_700_000_000_000)
            .unwrap();
        let rec = store.get_run_record(&id).unwrap().expect("present");
        assert_eq!(rec.terminal_status, None);
        let op = store.get_run_operational(&id).unwrap().expect("present");
        assert_eq!(op.transient_status, RunStatus::Running);
        assert_eq!(op.started_at_ms, 1_700_000_000_000);
    }

    // ── metadata round-trip + list_runs (0.3-S2c) ──

    #[test]
    fn get_run_metadata_returns_none_for_missing() {
        let store = fresh_store();
        assert!(store.get_run_metadata("nope").unwrap().is_none());
    }

    #[test]
    fn get_run_metadata_round_trips_string() {
        let store = fresh_store();
        let mut r = sample_run("R-1");
        r.metadata_json = r#"{"k":"v","approvals":{}}"#.to_string();
        store.insert_run(&r).unwrap();
        let got = store.get_run_metadata("R-1").unwrap().unwrap();
        assert_eq!(got, r#"{"k":"v","approvals":{}}"#);
    }

    #[test]
    fn update_run_metadata_changes_metadata_and_updated_at() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        let new_meta = r#"{"approvals":{"S-1":{"decision":"approved","decided_at_ms":42}}}"#;
        store
            .update_run_metadata("R-1", new_meta, 1_700_000_999_000)
            .unwrap();
        let got = store.get_run_metadata("R-1").unwrap().unwrap();
        assert_eq!(got, new_meta);
        let op = store.get_run_operational("R-1").unwrap().unwrap();
        assert_eq!(op.updated_at_ms, 1_700_000_999_000);
    }

    #[test]
    fn update_run_metadata_returns_not_found_for_missing() {
        let store = fresh_store();
        let err = store
            .update_run_metadata("nope", "{}", 0)
            .expect_err("missing run must error");
        match err {
            PersistenceError::NotFound { entity, key } => {
                assert_eq!(entity, "run");
                assert_eq!(key, "nope");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn list_runs_empty_db() {
        let store = fresh_store();
        let runs = store.list_runs().unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn list_runs_returns_all_ordered_by_workflow_name_run_id() {
        let store = fresh_store();
        let mut a = sample_run("R-1");
        a.workflow_name = "z-late".into();
        a.status = RunStatus::Running;
        let mut b = sample_run("R-2");
        b.workflow_name = "a-early".into();
        b.status = RunStatus::Completed;
        let mut c = sample_run("R-3");
        c.workflow_name = "a-early".into();
        c.status = RunStatus::Failed;
        store.insert_run(&a).unwrap();
        store.insert_run(&b).unwrap();
        store.insert_run(&c).unwrap();
        let runs = store.list_runs().unwrap();
        assert_eq!(runs.len(), 3);
        // Sorted by (workflow_name, run_id) — deterministic, NOT by timestamps.
        assert_eq!(runs[0].run_id, "R-2"); // a-early, R-2
        assert_eq!(runs[1].run_id, "R-3"); // a-early, R-3
        assert_eq!(runs[2].run_id, "R-1"); // z-late, R-1
    }

    // ── 0.3-S7: in-flight runs filter ──

    #[test]
    fn list_in_flight_runs_for_workflow_empty_db() {
        let store = fresh_store();
        let runs = store.list_in_flight_runs_for_workflow("any").unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn list_in_flight_runs_for_workflow_returns_running_and_paused() {
        let store = fresh_store();
        // Two runs of workflow A: one Running, one Completed.
        // Two runs of workflow B: one Paused, one Failed.
        let mut a_running = sample_run("A-1");
        a_running.workflow_hash = "wh-A".into();
        a_running.status = RunStatus::Running;
        let mut a_completed = sample_run("A-2");
        a_completed.workflow_hash = "wh-A".into();
        a_completed.status = RunStatus::Completed;
        let mut b_paused = sample_run("B-1");
        b_paused.workflow_hash = "wh-B".into();
        b_paused.status = RunStatus::Paused;
        let mut b_failed = sample_run("B-2");
        b_failed.workflow_hash = "wh-B".into();
        b_failed.status = RunStatus::Failed;
        store.insert_run(&a_running).unwrap();
        store.insert_run(&a_completed).unwrap();
        store.insert_run(&b_paused).unwrap();
        store.insert_run(&b_failed).unwrap();

        // Workflow A: only A-1 (Running) is in-flight.
        let in_flight_a = store.list_in_flight_runs_for_workflow("wh-A").unwrap();
        assert_eq!(in_flight_a.len(), 1);
        assert_eq!(in_flight_a[0].run_id, "A-1");
        assert_eq!(in_flight_a[0].status, RunStatus::Running);

        // Workflow B: only B-1 (Paused) is in-flight.
        let in_flight_b = store.list_in_flight_runs_for_workflow("wh-B").unwrap();
        assert_eq!(in_flight_b.len(), 1);
        assert_eq!(in_flight_b[0].run_id, "B-1");
        assert_eq!(in_flight_b[0].status, RunStatus::Paused);

        // Workflow C: nothing.
        assert!(store
            .list_in_flight_runs_for_workflow("wh-C")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn list_in_flight_runs_for_workflow_orders_deterministically() {
        let store = fresh_store();
        let mut r1 = sample_run("R-1");
        r1.workflow_name = "z-late".into();
        r1.workflow_hash = "shared".into();
        r1.status = RunStatus::Running;
        let mut r2 = sample_run("R-2");
        r2.workflow_name = "a-early".into();
        r2.workflow_hash = "shared".into();
        r2.status = RunStatus::Paused;
        store.insert_run(&r1).unwrap();
        store.insert_run(&r2).unwrap();
        let runs = store.list_in_flight_runs_for_workflow("shared").unwrap();
        assert_eq!(runs.len(), 2);
        // Sorted by (workflow_name, run_id) — a-early before z-late.
        assert_eq!(runs[0].run_id, "R-2");
        assert_eq!(runs[1].run_id, "R-1");
    }

    // ── 0.3-S11: attempt_count column + schema v1→v2 migration ──

    #[test]
    fn attempt_count_column_exists_after_init() {
        // Locks the schema v2 contract: every freshly-opened DB has
        // the attempt_count column on step_checkpoints. Catches an
        // accidental removal of the column from schema_v1.sql or
        // a botched migration that fails to add it.
        let store = fresh_store();
        let exists =
            super::column_exists(&store.conn, "step_checkpoints", "attempt_count").unwrap();
        assert!(
            exists,
            "attempt_count column must exist on step_checkpoints"
        );
    }

    #[test]
    fn schema_version_is_2_after_init() {
        let store = fresh_store();
        let version: i64 = store
            .conn
            .query_row("SELECT version FROM schema_version WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, 2);
        assert_eq!(SCHEMA_VERSION, 2);
    }

    #[test]
    fn upsert_preserves_attempt_count_round_trip() {
        // Insert a checkpoint with attempt_count=3, list_step_checkpoints
        // must return the same value (parser correctly reads it).
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        let mut cp = sample_checkpoint("R-1", "S-1", StepStatus::Completed);
        cp.attempt_count = 3;
        store.upsert_step_checkpoint(&cp).unwrap();
        let listed = store.list_step_checkpoints("R-1").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].attempt_count, 3);
    }

    #[test]
    fn upsert_terminal_overwrites_attempt_count_from_running_default() {
        // Realistic flow: mark_step_running_clearing_output sets
        // attempt_count=1 on insert. Then the terminal upsert
        // overwrites with the actual final attempt count (e.g. 3
        // after retries).
        let store = fresh_store();
        store.insert_run(&sample_run("R-1")).unwrap();
        store
            .mark_step_running_clearing_output("R-1", "S-1", 1_700_000_000_000)
            .unwrap();
        // Verify Running checkpoint has attempt_count=1 (the default).
        let listed = store.list_step_checkpoints("R-1").unwrap();
        assert_eq!(listed[0].attempt_count, 1);
        // Terminal upsert with actual count. Pass started_at_ms=None
        // so COALESCE preserves the original (running) timestamp;
        // this matches the runner's actual call pattern.
        let mut completed = sample_checkpoint("R-1", "S-1", StepStatus::Completed);
        completed.attempt_count = 4;
        completed.output_json = Some(r#"{"ok":true}"#.into());
        completed.output_hash = Some("sha256:beef".into());
        completed.started_at_ms = None;
        completed.ended_at_ms = Some(1_700_000_002_000);
        store.upsert_step_checkpoint(&completed).unwrap();
        let listed = store.list_step_checkpoints("R-1").unwrap();
        assert_eq!(
            listed[0].attempt_count, 4,
            "terminal upsert overwrites attempt_count from Running default"
        );
        // started_at preserved via COALESCE (terminal upsert passed None).
        assert_eq!(listed[0].started_at_ms, Some(1_700_000_000_000));
    }

    // ── 0.3-S10: atomic skip-if-in-flight ──

    #[test]
    fn skip_if_in_flight_inserts_when_no_prior() {
        let store = fresh_store();
        let outcome = store
            .insert_run_with_derived_id_skip_if_in_flight(
                "wf",
                "wh-X",
                "ih-1",
                "{}",
                "{}",
                1_700_000_000_000,
            )
            .unwrap();
        match outcome {
            InsertOrSkip::Inserted(run_id) => {
                assert!(!run_id.is_empty());
                // Confirm the row landed.
                assert!(store.get_run(&run_id).unwrap().is_some());
            }
            InsertOrSkip::Skipped(_) => panic!("expected Inserted, got Skipped"),
        }
    }

    #[test]
    fn skip_if_in_flight_skips_when_running_prior_exists() {
        let store = fresh_store();
        let mut prior = sample_run("R-1");
        prior.workflow_hash = "wh-X".into();
        prior.status = RunStatus::Running;
        store.insert_run(&prior).unwrap();
        let outcome = store
            .insert_run_with_derived_id_skip_if_in_flight(
                "wf",
                "wh-X",
                "ih-1",
                "{}",
                "{}",
                1_700_000_000_000,
            )
            .unwrap();
        match outcome {
            InsertOrSkip::Skipped(row) => assert_eq!(row.run_id, "R-1"),
            InsertOrSkip::Inserted(_) => panic!("expected Skipped, got Inserted"),
        }
        // Verify only the original row exists.
        assert_eq!(store.count_runs_for_workflow("wh-X").unwrap(), 1);
    }

    #[test]
    fn skip_if_in_flight_skips_when_paused_prior_exists() {
        let store = fresh_store();
        let mut prior = sample_run("R-1");
        prior.workflow_hash = "wh-X".into();
        prior.status = RunStatus::Paused;
        store.insert_run(&prior).unwrap();
        let outcome = store
            .insert_run_with_derived_id_skip_if_in_flight(
                "wf",
                "wh-X",
                "ih-1",
                "{}",
                "{}",
                1_700_000_000_000,
            )
            .unwrap();
        assert!(matches!(outcome, InsertOrSkip::Skipped(_)));
    }

    #[test]
    fn skip_if_in_flight_inserts_when_only_terminal_priors_exist() {
        let store = fresh_store();
        let mut completed = sample_run("R-old1");
        completed.workflow_hash = "wh-X".into();
        completed.status = RunStatus::Completed;
        let mut failed = sample_run("R-old2");
        failed.workflow_hash = "wh-X".into();
        failed.status = RunStatus::Failed;
        store.insert_run(&completed).unwrap();
        store.insert_run(&failed).unwrap();
        let outcome = store
            .insert_run_with_derived_id_skip_if_in_flight(
                "wf",
                "wh-X",
                "ih-1",
                "{}",
                "{}",
                1_700_000_000_000,
            )
            .unwrap();
        assert!(matches!(outcome, InsertOrSkip::Inserted(_)));
        assert_eq!(store.count_runs_for_workflow("wh-X").unwrap(), 3);
    }

    #[test]
    fn skip_if_in_flight_concurrent_invocations_at_most_one_inserts() {
        // Headline regression: N concurrent threads racing the
        // check-then-insert. Without the atomic transaction (the
        // 0.3-S7 flow), multiple threads could both pass the check
        // and both insert. With the atomic transaction (0.3-S10),
        // at most ONE thread inserts; others see Skipped or each
        // other's Inserted result and Skip.
        //
        // Note: the FIRST thread to acquire the writer lock inserts;
        // subsequent threads see that insert and Skip. The expected
        // outcome distribution is 1 Inserted + (N-1) Skipped.
        use std::sync::Arc;
        use std::thread;
        let data_dir = tempfile::tempdir().unwrap();
        let db_path = data_dir.path().join("runs.db");
        let _ = RunCheckpointStore::open(&db_path).unwrap();
        let db_path = Arc::new(db_path);
        const N: usize = 8;
        let mut handles = Vec::new();
        for _ in 0..N {
            let path = Arc::clone(&db_path);
            handles.push(thread::spawn(move || {
                let store = RunCheckpointStore::open(&path).unwrap();
                store
                    .insert_run_with_derived_id_skip_if_in_flight(
                        "wf",
                        "shared-wh",
                        "ih-1",
                        "{}",
                        "{}",
                        1_700_000_000_000,
                    )
                    .expect("must not error")
            }));
        }
        let outcomes: Vec<InsertOrSkip> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let inserted_count = outcomes
            .iter()
            .filter(|o| matches!(o, InsertOrSkip::Inserted(_)))
            .count();
        let skipped_count = outcomes
            .iter()
            .filter(|o| matches!(o, InsertOrSkip::Skipped(_)))
            .count();
        assert_eq!(
            inserted_count, 1,
            "exactly one writer must insert; got {inserted_count} inserts and {skipped_count} skips"
        );
        assert_eq!(skipped_count, N - 1);
        // Confirm only one row landed in the store.
        let store = RunCheckpointStore::open(&db_path).unwrap();
        assert_eq!(store.count_runs_for_workflow("shared-wh").unwrap(), 1);
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

    // ── 0.3-S16: commit_external_trigger atomic commit ──

    fn paused_run_with_trigger_gate(store: &RunCheckpointStore, run_id: &str) {
        store.insert_run(&sample_run(run_id)).unwrap();
        store
            .upsert_step_checkpoint(&sample_checkpoint(
                run_id,
                "webhook",
                StepStatus::AwaitingExternalEvent,
            ))
            .unwrap();
    }

    #[test]
    fn commit_external_trigger_succeeds_under_normal_conditions() {
        let store = fresh_store();
        paused_run_with_trigger_gate(&store, "R-T1");
        let outcome = store
            .commit_external_trigger(
                "R-T1",
                "webhook",
                "{}",
                "{\"triggers\":{\"webhook\":\"x\"}}",
                "\"payload\"",
                "sha256:abc",
                1_700_000_002_000,
            )
            .unwrap();
        assert_eq!(outcome, TriggerCommitOutcome::Committed);
        // Checkpoint must be Completed with output and ended_at.
        let cps = store.list_step_checkpoints("R-T1").unwrap();
        let cp = cps.iter().find(|c| c.step_id == "webhook").unwrap();
        assert_eq!(cp.status, StepStatus::Completed);
        assert_eq!(cp.output_json.as_deref(), Some("\"payload\""));
        assert_eq!(cp.output_hash.as_deref(), Some("sha256:abc"));
        assert_eq!(cp.ended_at_ms, Some(1_700_000_002_000));
        // Metadata must reflect the new value.
        let metadata = store.get_run_metadata("R-T1").unwrap().unwrap();
        assert_eq!(metadata, "{\"triggers\":{\"webhook\":\"x\"}}");
    }

    #[test]
    fn commit_external_trigger_returns_metadata_changed_when_cas_loses() {
        let store = fresh_store();
        paused_run_with_trigger_gate(&store, "R-T2");
        // The expected_prior_metadata doesn't match what's on disk
        // ("{}"), so the CAS should fail.
        let outcome = store
            .commit_external_trigger(
                "R-T2",
                "webhook",
                "{\"different\":\"snapshot\"}",
                "{\"triggers\":{\"webhook\":\"x\"}}",
                "\"payload\"",
                "sha256:abc",
                1_700_000_002_000,
            )
            .unwrap();
        assert_eq!(outcome, TriggerCommitOutcome::MetadataChanged);
        // No mutations should have committed: checkpoint still
        // AwaitingExternalEvent, metadata still "{}".
        let cps = store.list_step_checkpoints("R-T2").unwrap();
        let cp = cps.iter().find(|c| c.step_id == "webhook").unwrap();
        assert_eq!(cp.status, StepStatus::AwaitingExternalEvent);
        let metadata = store.get_run_metadata("R-T2").unwrap().unwrap();
        assert_eq!(metadata, "{}");
    }

    #[test]
    fn commit_external_trigger_returns_state_mismatch_when_checkpoint_advanced() {
        let store = fresh_store();
        paused_run_with_trigger_gate(&store, "R-T3");
        // Simulate a concurrent resume marking the step Running. The
        // commit must surface CheckpointStateMismatch.
        store
            .upsert_step_checkpoint(&sample_checkpoint("R-T3", "webhook", StepStatus::Running))
            .unwrap();
        let outcome = store
            .commit_external_trigger(
                "R-T3",
                "webhook",
                "{}",
                "{\"triggers\":{\"webhook\":\"x\"}}",
                "\"payload\"",
                "sha256:abc",
                1_700_000_002_000,
            )
            .unwrap();
        assert_eq!(
            outcome,
            TriggerCommitOutcome::CheckpointStateMismatch {
                current_status: "running".to_string()
            }
        );
        // Both the metadata CAS and the checkpoint write must have
        // been rolled back.
        let metadata = store.get_run_metadata("R-T3").unwrap().unwrap();
        assert_eq!(metadata, "{}");
    }

    #[test]
    fn commit_external_trigger_state_mismatch_when_checkpoint_missing() {
        let store = fresh_store();
        store.insert_run(&sample_run("R-T4")).unwrap();
        // No step checkpoint at all.
        let outcome = store
            .commit_external_trigger(
                "R-T4",
                "webhook",
                "{}",
                "{}",
                "\"payload\"",
                "sha256:abc",
                1_700_000_002_000,
            )
            .unwrap();
        assert!(matches!(
            outcome,
            TriggerCommitOutcome::CheckpointStateMismatch { ref current_status }
                if current_status == "missing"
        ));
    }

    #[test]
    fn commit_external_trigger_run_not_found_surfaces_typed_error() {
        let store = fresh_store();
        let err = store
            .commit_external_trigger(
                "R-MISSING",
                "webhook",
                "{}",
                "{}",
                "\"payload\"",
                "sha256:abc",
                0,
            )
            .expect_err("missing run must surface typed error");
        assert!(matches!(
            err,
            PersistenceError::NotFound { entity: "run", .. }
        ));
    }
}
