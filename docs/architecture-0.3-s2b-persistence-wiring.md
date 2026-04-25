# Architecture — 0.3-S2b: Persistence Wiring

**Companion:** [`design-0.3-s2b-persistence-wiring.md`](design-0.3-s2b-persistence-wiring.md)
**Anchored by:** ADR 001, project-conventions §11–17 (persistence + determinism).

## Decision summary

| Open Q | Decision | Why |
|---|---|---|
| Q1 — `--data-dir` default | `./.boruna/data` (CWD-relative). `BORUNA_DATA_DIR` env var overrides. Explicit `--data-dir` flag overrides both. | Discoverable for interactive use, overridable for daemons + CI. Refuse silently-using-system-root if resolved path is `/`, `/root`, or empty. |
| Q2 — `run_id` counter | Derived inside `BEGIN IMMEDIATE`: `counter = SELECT COUNT(*) FROM runs WHERE workflow_hash = ?`. `run_id = sha256(workflow_hash || ":" || inputs_hash || ":" || counter_LE_8bytes)[..16]` (hex). | No schema change. Deterministic given DB state. Re-running same workflow at counter=0 in two fresh DBs produces identical `run_id` (provable). |
| Q3 — Resume against partial writes | Steps with `status = running` at resume-time are re-executed (treated as "never completed"). Output column may be NULL or stale; ignored. Documented in `WorkflowRunner::resume` rustdoc + a regression test. | Crashes mid-step can leave any state; the only safe rule is "trust only terminal `completed`." Re-execution is idempotent because steps are pure functions of resolved inputs. |
| Q4 — Approval-gate resume | **Deferred to `0.3-S2c`** alongside `boruna workflow approve`. For 0.3-S2b: a paused approval-gate run resumes by re-encountering the gate and re-pausing — the run remains advancable only via the still-to-come approve CLI. The earlier sentinel-in-metadata sketch was rejected during review (C1/H6) because the runner has no read path for it; promising it without implementing it would create dead code. | Honesty: the current `eprintln!("Run: boruna workflow approve …")` message points at a command the next sprint owns. Architecture doc should not promise behavior the diff doesn't ship. |
| Q5 — `RunRow` split | Keep `RunRow` (catch-all) for back-compat. Add two **view structs**: `RunRecord` (replay-verified subset) and `RunOperational` (operational subset). New methods `get_run_record` + `get_run_operational`. Audit code paths MUST use `RunRecord`. | Compile-time enforcement that no audit query touches `started_at`. Cheaper than a full type-state refactor. |

## Component changes

### `orchestrator/src/persistence/mod.rs`

**Additions (no breaking removals):**

```rust
/// Replay-verified subset of a run row. Audit + replay code paths consume
/// this — never the full RunRow — so it is structurally impossible to
/// accidentally hash an operational timestamp.
pub struct RunRecord {
    pub run_id: String,
    pub workflow_name: String,
    pub workflow_hash: String,
    pub terminal_status: Option<RunStatus>, // Some only when status ∈ {Completed, Failed}
    pub policy_json: String,
    pub metadata_json: String,
}

/// Operational subset of a run row. Status dashboards, progress tracking,
/// alerting consume this. Never feeds a hash.
pub struct RunOperational {
    pub run_id: String,
    pub transient_status: RunStatus, // any value, including transient
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
}

impl RunCheckpointStore {
    pub fn get_run_record(&self, run_id: &str) -> Result<Option<RunRecord>, PersistenceError>;
    pub fn get_run_operational(&self, run_id: &str) -> Result<Option<RunOperational>, PersistenceError>;
    pub fn count_runs_for_workflow(&self, workflow_hash: &str) -> Result<i64, PersistenceError>;
}
```

`RunRecord::terminal_status` is `Some` only when the persisted status is `Completed` or `Failed`. For transient states it is `None` — replay code that needs to "verify the run completed identically" must therefore branch on `Some(Completed)` / `Some(Failed)`, not pattern-match a `RunStatus` that includes `Running`.

### `orchestrator/src/workflow/runner.rs`

**Replace** `WorkflowRunner` (unit struct) with a stateful struct that owns an optional store. Old static `WorkflowRunner::run(&def, &options)` API is preserved by delegating internally.

```rust
pub struct WorkflowRunner {
    store: Option<RunCheckpointStore>,    // None ⇒ ephemeral, no checkpointing
    inputs_hash: String,                   // sha256 of resolved-inputs JSON
}

impl WorkflowRunner {
    /// Backward-compat entry point — no persistence.
    pub fn run(def: &WorkflowDef, options: &RunOptions) -> Result<WorkflowRunResult, WorkflowRunError>;

    /// New entry point — opens (or reuses) a store at `data_dir/runs.db`.
    pub fn run_persistent(
        def: &WorkflowDef,
        options: &RunOptions,
        data_dir: &Path,
    ) -> Result<WorkflowRunResult, WorkflowRunError>;

    /// Resume a previously-paused or crashed run. Verifies workflow_hash
    /// matches the on-disk definition. Refuses with WorkflowRunError::HashMismatch
    /// when the workflow file changed since the original run.
    pub fn resume(
        run_id: &str,
        data_dir: &Path,
        options: &ResumeOptions,
    ) -> Result<WorkflowRunResult, WorkflowRunError>;
}
```

**New error variants** (additive on `WorkflowRunError`):

- `RunNotFound(String)` — for resume against a typo'd run_id
- `WorkflowHashMismatch { run_id, expected, actual }` — workflow file changed
- `Persistence(PersistenceError)` — wrapped; preserves typed error_kind for CLI output

**Determinism in `run_id` derivation:**

```rust
fn derive_run_id(
    workflow_hash: &str,
    inputs_hash: &str,
    counter: i64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workflow_hash.as_bytes());
    hasher.update(b":");
    hasher.update(inputs_hash.as_bytes());
    hasher.update(b":");
    hasher.update(&counter.to_le_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..8]) // first 16 hex chars
}
```

The `counter` lookup + run insertion happen in a single `BEGIN IMMEDIATE` transaction in `RunCheckpointStore`, atomic against concurrent inserts.

**`metadata_json` shape (runner-owned):**

```json
{
  "workflow_dir": "<absolute path>",
  "inputs_hash": "<sha256 hex>",
  "boruna_version": "0.2.0",
  "run_index": <i64>,
  "approvals": {}
}
```

`workflow_dir` is operational-only (different machine paths produce different metadata but identical replay output). Annotated as such in code comments.

### `crates/llmvm-cli/src/main.rs`

**New top-level CLI flag:** `--data-dir <PATH>` (parsed into a `PathBuf`, defaults to env var or `./.boruna/data`).

**New subcommand variant:**

```rust
WorkflowCommand::Resume {
    run_id: String,
    /// Override the workflow definition directory (defaults to the path stored
    /// in metadata at the original run).
    #[arg(long)]
    workflow_dir: Option<PathBuf>,
    #[arg(short, long, default_value = "allow-all")]
    policy: String,
    #[cfg(feature = "http")]
    #[arg(long)]
    live: bool,
}
```

**Run path changes:** `WorkflowCommand::Run` now opens a store at the resolved `--data-dir/runs.db` and calls `WorkflowRunner::run_persistent`. The old non-persistent path is kept behind a `--ephemeral` flag for tests / one-off runs.

### `boruna-orchestrator` `Cargo.toml`

`sha2` is already in the dep tree (used by audit). Add `hex` to the orchestrator deps if not already pulled transitively. No new external deps.

## Sequencing

```
[Build phase ordering]
  1. Persistence: add RunRecord, RunOperational, count_runs_for_workflow + tests
  2. RunRow split fanout: ensure existing audit code paths use RunRecord
  3. Runner: derive_run_id helper + 0-counter test
  4. Runner: run_persistent path (keeps run() backward-compat)
  5. Runner: resume() path
  6. CLI: --data-dir flag + Resume subcommand wiring
  7. Crash-recovery integration test (in-process simulation)
  8. Optional subprocess SIGKILL test gated by #[ignore]
  9. CHANGELOG + docs/concepts/persistence.md update
```

Each numbered step is one commit on the branch. Reviewer can bisect.

## What stays the same

- Schema v1 (no migration needed).
- Existing `RunRow`, `StepCheckpoint`, all CRUD methods.
- All existing `boruna workflow run` semantics for the ephemeral path.
- `protocol_version: 1` envelope on MCP responses (untouched).
- Approval-gate UX prints the same message; the `boruna workflow approve` subcommand is `0.3-S2c`, not this sprint.

## Out-of-scope (deferred)

- Async / parallel step execution.
- Multi-process writer contention (still single-writer-by-process).
- Cross-host resume (`--data-dir` is a local path).
- `boruna workflow approve <run-id> <step-id>` CLI — `0.3-S2c`.
- `boruna workflow list` for paused runs — would build on `list_runs_by_status`. Could squeeze in if review goes fast; otherwise `0.3-S2c`.

## Risks + mitigations

| Risk | Mitigation |
|---|---|
| Subprocess SIGKILL test is flaky on the self-hosted runner | Default test is in-process simulation; subprocess test is `#[ignore]`-gated and only runs locally. |
| Operators surprised by `./.boruna/data` writes in CWD | Print the resolved data_dir path at every run start; refuse `/` and empty. |
| `inputs_hash` semantic drift if input shape changes between sprints | Document the inputs hash algorithm in `metadata_json.inputs_hash` doc; lock with a regression test asserting a known input → known hash. |
| `run_id` collision on race (two writers, same DB, same workflow_hash, same inputs, same counter) | `BEGIN IMMEDIATE` serializes; second writer sees `count` = 1 not 0. Locked by a multi-thread test (in-process via `Arc<Mutex<Connection>>`-wrapped store). |
| Audit code accidentally typed against full `RunRow` after the split | New `clippy::deprecated`-style hint + a doc-test in `RunRecord` showing the canonical use. |
