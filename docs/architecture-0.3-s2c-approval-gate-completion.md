# Architecture — 0.3-S2c: Approval-Gate Completion

**Companion:** [`design-0.3-s2c-approval-gate-completion.md`](design-0.3-s2c-approval-gate-completion.md)

## Decision summary

| Open Q | Decision |
|---|---|
| Q1 — Sentinel shape | **`metadata.approvals.<step_id>: { decision: "approved" \| "rejected", decided_at_ms: i64, reason?: String }`**. Captures audit-relevant fields without bloating the per-step payload. |
| Q2 — Approve writes sentinel only | **Sentinel-only.** `approve` writes `metadata.approvals.<step>` atomically; `resume` reads the sentinel and transitions the step. Single source of truth. The `approve` command's success message points operators at the next `resume` invocation. |
| Q3 — Wrong-state errors | **Distinct typed errors:** `step_not_found`, `step_not_at_approval_gate`, `step_already_decided`, `not_an_approval_gate_step` (the StepDef is not a `StepKind::ApprovalGate`). |
| Q4 — Approve on terminal run | **Typed error `run_not_resumable`** with `terminal_status` field for context. Refuses to mutate finished runs. |
| Q5 — `workflow list` shape | Columns: status, run_id, workflow_name, started_at_ms, updated_at_ms. Ordered by `(workflow_name, run_id)` — deterministic. Optional `--status <STATUS>` filter. Optional `--json` for machine-readable. |

## Component changes

### `orchestrator/src/persistence/mod.rs`

**No new types.** `metadata_json` continues to be opaque caller-defined JSON (the runner serializes a typed `PersistedRunMetadata`).

**New methods on `RunCheckpointStore`:**

```rust
/// Read the metadata_json column. Returns the raw string for the runner
/// to deserialize into its typed shape. Used by `approve`/`reject` paths
/// before round-tripping the modified metadata back to disk.
pub fn get_run_metadata(&self, run_id: &str) -> Result<Option<String>, PersistenceError>;

/// Update only the metadata_json column atomically (under BEGIN IMMEDIATE
/// + busy-retry). Touches updated_at_ms operationally. Returns
/// PersistenceError::NotFound for unknown run_id.
pub fn update_run_metadata(
    &self,
    run_id: &str,
    metadata_json: &str,
    updated_at_ms: i64,
) -> Result<(), PersistenceError>;

/// List ALL runs (no status filter) ordered by (workflow_name, run_id).
/// `workflow list --status` reuses the existing `list_runs_by_status`;
/// the unfiltered case needs this new method.
pub fn list_runs(&self) -> Result<Vec<RunRow>, PersistenceError>;
```

### `orchestrator/src/workflow/runner.rs`

**Extend `PersistedRunMetadata`:**

```rust
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct PersistedRunMetadata {
    workflow_dir: String,
    inputs_hash: String,
    boruna_version: String,
    /// **OPERATIONAL ONLY.** Map of step_id → ApprovalDecision. Captures
    /// who decided what and when for audit-trail purposes; does NOT feed
    /// any replay hash. Default empty for back-compat with 0.3-S2b
    /// databases that have no `approvals` key.
    #[serde(default)]
    approvals: BTreeMap<String, ApprovalDecision>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ApprovalDecision {
    decision: ApprovalKind,
    /// **OPERATIONAL ONLY** — Unix epoch ms of when the operator ran
    /// `boruna workflow approve`/`reject`. Wall-clock-keyed; not in any
    /// hash chain.
    decided_at_ms: i64,
    /// Optional human-supplied rejection reason. None for approvals.
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ApprovalKind {
    Approved,
    Rejected,
}
```

**New module-level functions:**

```rust
/// Public entry for `boruna workflow approve` / `reject` CLI handlers.
/// Validates the run + step, mutates metadata.approvals, writes back via
/// update_run_metadata. Returns the typed error for any wrong-state
/// scenario. Does NOT advance the run — the operator must run resume.
#[cfg(feature = "persist-sqlite")]
pub fn record_approval_decision(
    data_dir: &Path,
    run_id: &str,
    step_id: &str,
    decision: ApprovalKind,
    reason: Option<String>,
) -> Result<(), WorkflowRunError>;
```

`ApprovalKind` is exposed as a public enum on the workflow module so the CLI can pass it. `ApprovalDecision` and `PersistedRunMetadata` stay private (they're an internal serialization detail).

**Resume changes (in `WorkflowRunner::resume`):**

After loading checkpoints, BEFORE the `execute_steps` call, scan `metadata.approvals`. For each `(step_id, decision)`:

- If `decision = Approved` AND the step's persisted checkpoint is `awaiting_approval`: write a `Completed` checkpoint (with synthetic empty-record output `{}`), insert into `already_completed`, add a `prior_results` entry. Approval-gate steps don't have a "real" output; this synthetic shape matches what an approved gate would have produced if executed.
- If `decision = Rejected` AND the step's checkpoint is `awaiting_approval`: write a `Failed` checkpoint with `error_msg = decision.reason.unwrap_or("rejected by operator")`. Set `halt_with_failed_step` so the run terminates Failed.
- If the sentinel is set but the checkpoint is already terminal (Completed/Failed), no-op — sentinel was decoration after the fact.

This change happens entirely inside `WorkflowRunner::resume` before `execute_steps` runs, so the existing run-loop logic stays intact.

**New error variants on `WorkflowRunError` (cfg-gated):**

```rust
#[cfg(feature = "persist-sqlite")]
StepNotFound { run_id: String, step_id: String },
#[cfg(feature = "persist-sqlite")]
StepNotAtApprovalGate { run_id: String, step_id: String, current_status: String },
#[cfg(feature = "persist-sqlite")]
StepAlreadyDecided { run_id: String, step_id: String, prior_decision: String },
#[cfg(feature = "persist-sqlite")]
NotAnApprovalGateStep { run_id: String, step_id: String },
#[cfg(feature = "persist-sqlite")]
RunNotResumable { run_id: String, terminal_status: String },
```

### `crates/llmvm-cli/src/main.rs`

**New `WorkflowCommand` variants:**

```rust
WorkflowCommand::Approve {
    run_id: String,
    step_id: String,
    #[arg(long)]
    data_dir: Option<PathBuf>,
}
WorkflowCommand::Reject {
    run_id: String,
    step_id: String,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long)]
    data_dir: Option<PathBuf>,
}
WorkflowCommand::List {
    /// Filter by status: "running" | "paused" | "completed" | "failed".
    #[arg(long)]
    status: Option<String>,
    /// Output as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long)]
    data_dir: Option<PathBuf>,
}
```

Each handler:
- Resolves data_dir via the existing `resolve_data_dir` helper.
- Calls into the orchestrator's typed entry points.
- Surfaces typed errors with stable user-facing strings:
  - `step_not_found` → "step '<id>' not found in run '<id>'"
  - `step_not_at_approval_gate` → "step '<id>' is in state <state>, not awaiting_approval"
  - `step_already_decided` → "step '<id>' was already <prior>"
  - `not_an_approval_gate_step` → "step '<id>' is not an approval gate"
  - `run_not_resumable` → "run '<id>' is <state>; cannot mutate"
- On success, `approve`/`reject` print:
  ```
  approval recorded for step '<id>' in run '<id>'.
  Run `boruna workflow resume <run-id> --data-dir <path>` to advance.
  ```

`workflow list` prints:
- Plain mode: a fixed-width table of `STATUS | RUN_ID | WORKFLOW | STARTED | UPDATED`.
- `--json` mode: `[{"run_id":"...","workflow_name":"...","status":"...","started_at_ms":...,"updated_at_ms":...}, ...]`.

## Sequencing

```
1. Persistence: get_run_metadata, update_run_metadata, list_runs + tests
2. Runner: extend PersistedRunMetadata with approvals; ApprovalDecision/Kind types
3. Runner: record_approval_decision public entry + tests
4. Runner: resume() honors the sentinel + tests
5. CLI: Approve/Reject/List subcommands + smoke tests
6. CHANGELOG entry + docs update (mention the workflow approve/reject CLI in design-0.3-s2b-persistence-wiring's "deferred" section as resolved)
```

## What stays the same

- Schema v1 (no migration).
- All 0.3-S2b semantics for `run_persistent`, `resume`, `--data-dir`, `--ephemeral`.
- `protocol_version: 1` MCP envelope (untouched — this sprint is CLI-only).
- Determinism contract for replay-verified columns. Approval sentinel is operational only.

## Risks + mitigations

| Risk | Mitigation |
|---|---|
| Existing 0.3-S2b databases without `approvals` field | `#[serde(default)]` on the field; tests include re-opening a 0.3-S2b-shape metadata blob |
| Race: two `approve` calls for the same step | `update_run_metadata` is BEGIN IMMEDIATE + busy-retry. The second writer reads the just-updated metadata and fails with `step_already_decided` |
| `record_approval_decision` mutates metadata for a run that's been deleted between read + write | The transaction's INSERT has a FK to runs(run_id); FK violation surfaces correctly |
| `workflow list --json` output drifts | Lock with a regression test asserting the JSON shape of one row |
