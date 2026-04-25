# Design: Persistent Workflow State — Module (0.3-S2a)

**Sprint:** `0.3-S2a` (split from `0.3-S2`) · **Implements:** ADR 001 step 1–5 · **Status:** Think

## Scope split rationale

Per the [Persistence Backend ADR](./adr/001-persistence-backend.md) "Sizing flag" section, `0.3-S2` is realistically L+ to 2× L. Splitting into:

- **`0.3-S2a` (this sprint):** schema + `RunCheckpointStore` + connection setup + retry policy + tests. **No runner integration, no `boruna workflow resume`.**
- **`0.3-S2b` (next sprint):** replace `tempfile::tempdir()` in `WorkflowRunner::run`, replace deterministic-violating `run_id` derivation, add `boruna workflow resume <run-id>` CLI, add `--data-dir` flag, crash-recovery integration test.

Split keeps each PR reviewable in one pass. The module landing first means `0.3-S2b`'s integration is mechanical — wire an already-tested API into the runner.

## Who needs this

The orchestrator's workflow runner (post-`0.3-S2b`). Today `WorkflowRunner::run` puts state in `tempfile::tempdir()` — workflow runs survive zero process restarts. Persistent checkpoints unblock `boruna workflow resume`, async steps (`0.3-S4`), retry policies (`0.3-S5`), scheduled workflows (`0.3-S7`), and the dashboard backend (`0.4-S7`).

This sprint ships only the bottom half: the `RunCheckpointStore` API. The integration is the next sprint.

## What this sprint ships

### `boruna_orchestrator::persistence::RunCheckpointStore`

A struct owning a `rusqlite::Connection`. Public API:

```rust
pub struct RunCheckpointStore { /* opaque */ }

impl RunCheckpointStore {
    /// Open or create a tape file at the given path. Runs migrations on first
    /// open. Idempotent: re-opening an existing DB is a no-op for the schema.
    pub fn open(path: &Path) -> Result<Self, PersistenceError>;

    /// Open in memory (for tests).
    pub fn open_in_memory() -> Result<Self, PersistenceError>;

    /// Insert a new run row. Returns Err if run_id already exists.
    pub fn insert_run(&self, run: &RunRow) -> Result<(), PersistenceError>;

    /// Update an existing run's status + updated_at.
    pub fn update_run_status(
        &self,
        run_id: &str,
        status: RunStatus,
        updated_at_ms: i64,
    ) -> Result<(), PersistenceError>;

    /// Fetch one run by id.
    pub fn get_run(&self, run_id: &str) -> Result<Option<RunRow>, PersistenceError>;

    /// List runs by status (uses the idx_runs_status index).
    pub fn list_runs_by_status(&self, status: RunStatus) -> Result<Vec<RunRow>, PersistenceError>;

    /// Insert OR replace (upsert) a step checkpoint. Caller writes once at
    /// each step transition (running → completed | failed). Wraps in a
    /// BEGIN IMMEDIATE / COMMIT transaction with automatic SQLITE_BUSY
    /// retry per the ADR's retry policy.
    pub fn upsert_step_checkpoint(
        &self,
        cp: &StepCheckpoint,
    ) -> Result<(), PersistenceError>;

    /// Fetch all step checkpoints for a run, ordered deterministically by
    /// (run_id, step_id). Useful for resume to find the next pending step.
    pub fn list_step_checkpoints(
        &self,
        run_id: &str,
    ) -> Result<Vec<StepCheckpoint>, PersistenceError>;
}
```

### Schema (per ADR, with the review-driven additions)

```sql
CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
INSERT INTO schema_version VALUES (1);

CREATE TABLE runs (
    run_id        TEXT PRIMARY KEY,
    workflow_name TEXT NOT NULL,
    workflow_hash TEXT NOT NULL,
    status        TEXT NOT NULL,
    started_at    INTEGER NOT NULL,    -- OPERATIONAL ONLY (per ADR)
    updated_at    INTEGER NOT NULL,    -- OPERATIONAL ONLY
    policy_json   TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE step_checkpoints (
    run_id      TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    step_id     TEXT NOT NULL,
    status      TEXT NOT NULL,
    output_json TEXT,                  -- REPLAY-VERIFIED
    output_hash TEXT,                  -- REPLAY-VERIFIED
    started_at  INTEGER,               -- OPERATIONAL ONLY
    ended_at    INTEGER,               -- OPERATIONAL ONLY
    error_msg   TEXT,
    PRIMARY KEY (run_id, step_id)
);

CREATE INDEX idx_runs_status ON runs(status);

-- Partial index for "what's blocked / running across all runs?" queries
-- the dashboard (0.4-S7) and the scheduler (0.3-S7) will need. Keeps the
-- index small (most rows have terminal status).
CREATE INDEX idx_step_checkpoints_active
    ON step_checkpoints(status)
    WHERE status IN ('awaiting_approval', 'running');
```

### Mandatory PRAGMAs (per ADR — all four on every Connection open)

```rust
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;        -- safe under WAL
PRAGMA foreign_keys = ON;           -- ⚠️ DEFAULT IS OFF — without this,
                                    --   ON DELETE CASCADE is silently inert
PRAGMA busy_timeout = 5000;         -- 5s — pairs with retry policy below
```

A unit test asserts the cascade behavior so that a future regression on the
`foreign_keys = ON` line is loud.

### Writer policy (per ADR)

All mutating operations wrap in `BEGIN IMMEDIATE; ...; COMMIT;`. On `SQLITE_BUSY`, retry with exponential backoff (10 ms → 50 ms → 250 ms → 1.25 s) before failing with `PersistenceError::Busy`. The `busy_timeout = 5000` PRAGMA covers reads transparently; the explicit retry loop covers immediate-acquire writes.

### Determinism contract (per ADR 001)

Reviewers of code in this module + downstream sprints must enforce:
- **`output_json`, `output_hash`, terminal `status` values** are replay-verified.
- **`started_at`, `updated_at`, `ended_at`, transient `status` values** are OPERATIONAL ONLY — never feed an audit hash, never ordered-on for replay-relevant queries.
- **`run_id`** is **caller-provided**. The store does NOT generate it. The runner sprint (`0.3-S2b`) is responsible for derivation per ADR (sha256 of workflow_hash + serialized inputs + counter, NOT `chrono::Utc::now()`).

### Out of scope for this sprint (defers to 0.3-S2b)

- `WorkflowRunner` integration (replace `tempfile::tempdir()`)
- `boruna workflow resume <run-id>` CLI subcommand
- `--data-dir` CLI flag plumbing
- Workflow-hash check on resume (refuse to resume against a modified workflow)
- Replace `runner.rs:49-53` deterministic-violating `run_id` derivation
- Crash-recovery integration test (SIGKILL mid-step)

These all sit on top of `RunCheckpointStore` mechanically.

### Out of scope entirely (per ADR)

- `max_memory_mb` enforcement (separate sprint)
- Legacy `orchestrator::storage::Store` consolidation (defers to `0.4-S7` per ADR)

## Library deps

```toml
# orchestrator/Cargo.toml
[features]
default = ["persist-sqlite"]
persist-sqlite = ["dep:rusqlite"]

[dependencies]
rusqlite = { version = "0.32", features = ["bundled"], optional = true }
```

The `bundled` feature compiles SQLite from source — confirmed in the ADR's host-build probe (`/tmp/rusqlite-musl-probe/`). Adds ~1.5 MB to the binary; acceptable per ADR's "Negative consequences" section.

`persist-sqlite` is on by default for `orchestrator`. The ADR's recommendation to gate it OFF in `boruna-mcp` and `boruna-pkg` (binaries that don't need persistence) lands when those binaries actually consume the orchestrator's persistence (next sprint at earliest).

## Acceptance criteria

1. New module `orchestrator/src/persistence/mod.rs` exporting `RunCheckpointStore`, `RunRow`, `StepCheckpoint`, `RunStatus`, `StepStatus`, `PersistenceError`.
2. `Cargo.toml` has `persist-sqlite` feature; `rusqlite/bundled` is the only new transitive dep.
3. All four PRAGMAs set on every connection open.
4. Schema migrated on first open (`schema_version` table populated); idempotent on re-open.
5. CRUD operations: `insert_run`, `update_run_status`, `get_run`, `list_runs_by_status`, `upsert_step_checkpoint`, `list_step_checkpoints`.
6. `BEGIN IMMEDIATE` + exponential backoff retry on `SQLITE_BUSY`.
7. **`PRAGMA foreign_keys = ON` is verified by a unit test** — insert run + child checkpoint, delete run, assert checkpoint is also gone.
8. **Schema version mismatch** on open returns `PersistenceError::SchemaVersionMismatch { expected, actual }` rather than silent corruption.
9. CHANGELOG entry in the `### Added` section.
10. Workspace tests still pass (`cargo test --workspace` and `cargo test --workspace --features boruna-orchestrator/persist-sqlite`).
11. clippy `-D warnings` clean both feature configurations.
12. Sprint design doc (this file) checked in.
