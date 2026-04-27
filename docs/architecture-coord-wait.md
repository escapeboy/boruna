# Architecture — `boruna coordinator wait` (sprint 0.5-S2f)

Companion to `docs/design-coord-wait.md`. Covers *how* —
module layout, advance-loop algorithm, error mapping,
concurrency.

## Module placement

Three layers, from bottom up:

1. **`orchestrator/src/workflow/definition.rs`** — no change.
   `WorkflowDef` already implements Serialize/Deserialize.
2. **`orchestrator/src/workflow/runner.rs`** — additions:
   - `PersistedRunMetadata::workflow_def: Option<WorkflowDef>`
     field (after `step_sources`, line ~172).
   - `WorkflowRunner::compute_ready_steps(def, status_map)`
     — pure associated function (no `&self`), takes the
     workflow def and a `BTreeMap<String, StepStatus>` map of
     step_id → current status. Returns deterministic
     `Vec<String>` of step IDs whose upstream deps are all
     Completed and whose own status is `Unknown` (i.e. no
     checkpoint row yet).
   - `WorkflowRunner::advance_run_one_tick(store, run_id)`
     — one polling tick. Reads metadata + checkpoints, calls
     `compute_ready_steps`, inserts Pending checkpoints for
     each newly-ready step using the existing
     `insert_initial_wave_pending_checkpoints` helper (or a
     refactored sibling that takes a list of step IDs).
     Returns `AdvanceResult { newly_pending: Vec<String>,
     run_status: RunStatus, all_step_statuses:
     BTreeMap<String, StepStatus> }`.
3. **`crates/llmvm-cli/src/coordinator.rs`** — adds a `wait`
   handler:
   - New `WaitArgs` struct with `run_id`, `data_dir`,
     `poll_interval_ms`.
   - New `pub async fn run_wait(args: WaitArgs)` entry point
     (called from `main.rs` matcher).
   - The handler opens `RunCheckpointStore`, loops calling
     `WorkflowRunner::advance_run_one_tick`, prints state
     changes line-by-line to stdout, exits when terminal.

## CLI surface

`crates/llmvm-cli/src/main.rs:166-198` (`CoordinatorCommand`
enum) gains a `Wait` variant:

```rust
enum CoordinatorCommand {
    Serve { /* unchanged */ },
    Wait {
        /// Run ID to drive to terminal status.
        run_id: String,

        /// SQLite data directory (must match the coordinator's
        /// --data-dir).
        #[arg(long)]
        data_dir: Option<PathBuf>,

        /// Polling interval in milliseconds (clamped to >=100,
        /// default 500).
        #[arg(long, default_value = "500")]
        poll_interval_ms: u64,

        /// Maximum total wait duration in seconds. 0 = unlimited.
        /// Default 0. Surface to support CI test timeouts.
        #[arg(long, default_value = "0")]
        max_wait_secs: u64,
    },
}
```

Dispatched in `main.rs::main` to
`crates/llmvm-cli/src/coordinator.rs::run_wait(args)`.

## Wire format / data shapes

### `PersistedRunMetadata` (after this sprint)

```rust
pub struct PersistedRunMetadata {
    pub workflow_dir: String,
    pub inputs_hash: String,
    pub boruna_version: String,
    #[serde(default)]
    pub approvals: BTreeMap<String, ApprovalRecord>,
    #[serde(default)]
    pub triggers: BTreeMap<String, TriggerRecord>,
    #[serde(default)]
    pub audit_log: Vec<AuditEvent>,
    #[serde(default)]
    pub step_sources: BTreeMap<String, String>,
    /// NEW: Full workflow DAG for client-side advancement.
    /// Operational only — `workflow_hash` remains the
    /// replay-verified record of source content.
    /// Capped at 1 MiB serialized JSON.
    #[serde(default)]
    pub workflow_def: Option<WorkflowDef>,
}
```

### `AdvanceResult`

```rust
pub struct AdvanceResult {
    /// Step IDs that transitioned Unknown → Pending this tick.
    pub newly_pending: Vec<String>,
    /// Overall run status: Running, Completed, or Failed.
    pub run_status: RunStatus,
    /// All step statuses after this tick (for diff against
    /// previous tick to detect new transitions).
    pub all_step_statuses: BTreeMap<String, StepStatus>,
}
```

`RunStatus` reuse the existing enum (Pending, Running,
Completed, Failed). The wait client maps:
- `Completed` → exit 0
- `Failed` → exit 1
- `Pending` / `Running` → keep polling

### Size cap on `workflow_def`

Embedded after `serde_json::to_string(&workflow_def)` in
`prepare_persistent_run`. `MAX_WORKFLOW_DEF_BYTES = 1 MiB`
(matches the per-step source body cap pattern from 0.5-S2e
but generous since there's only one). Exceeding the cap
returns `WorkflowError::Validation { message:
"workflow_def serialized to N bytes, exceeds MAX_WORKFLOW_DEF_BYTES (1048576)" }`
at submit time. Locked by a regression test that crafts a
synthetic 1.1 MiB WorkflowDef.

## Advance-loop algorithm

```text
loop:
  tick_start = now()
  load metadata_json from runs.db
  parse → PersistedRunMetadata { workflow_def, step_sources, ... }
  if workflow_def is None:
    error("run was submitted before 0.5-S2f; cannot wait")
  load all step checkpoints for run_id
  build status_map: BTreeMap<step_id, StepStatus>
    (Unknown for any step in workflow_def.steps not present in checkpoints)
  ready = compute_ready_steps(workflow_def, status_map)
    // returns step_ids where:
    //   - status is Unknown
    //   - all upstream edges target a Completed step
    // sorted ascending by step_id (deterministic)
  for step_id in ready:
    source = step_sources[step_id]
    upsert_step_checkpoint(run_id, step_id, Pending, source, ...)
  run_status = derive_run_status(workflow_def, status_map)
    // Completed if all steps Completed
    // Failed if any step Failed
    // Running otherwise
  print transitions (status_map diff vs. previous tick)
  if run_status terminal: break
  sleep(poll_interval_ms - (now() - tick_start))
```

## Concurrency / race analysis

### Wait client + coordinator concurrent writes

Both processes hold `Arc<Mutex<RunCheckpointStore>>`-equivalent
handles to runs.db. SQLite WAL + `busy_timeout = 5000ms` +
`with_busy_retry` (convention #13) covers transactional
conflicts.

Specific race: wait client computes `ready = [step_5]` based
on checkpoints at time T. Between then and the upsert at T+ε,
the coordinator might have already... actually NO: a step can
only become Pending via the wait client (or submit-only).
The coordinator never inserts new Pending rows; it only
transitions Pending → Running → Completed/Failed. So:

- **Wait writes Pending.** Coordinator hasn't seen it yet,
  but next claim cycle picks it up. ✓
- **Wait re-tick reads status.** Sees the row as Pending or
  (if a worker has already claimed) Running. Either way,
  `compute_ready_steps` returns `Unknown` only, so the row
  is not in the ready set on the next tick. ✓
- **upsert_step_checkpoint with status=Pending against an
  existing Running/Completed row**: the COALESCE pattern
  (convention #14) and the `with_busy_retry` envelope mean
  the upsert sees the existing non-Pending status and the
  CAS-style update preserves it. **Verify** with a regression
  test that explicitly issues a wait-tick when one of the
  ready steps has been concurrently claimed.

### Lease-expiry / re-claim

A step that's Running with a stale lease may transition
back to Pending (via the coordinator's background sweep) and
then Running again with a higher claim_id. The wait client
should NOT write a fresh Pending checkpoint during the brief
Pending-after-expiry window; that step is not in the ready
set because its status is Pending, not Unknown. So the
sweep+wait interaction is safe by construction.

### Stale-claim CAS

The wait client never calls `complete_step_cas` or
`fail_step_cas`. Those are coordinator-only. So the slow-
worker-race CAS guarantee is unaffected by this sprint.

## Error mapping (typed)

| Path | Error | error_kind |
|---|---|---|
| `workflow_def` missing in metadata | `WorkflowError::Validation` | (CLI surfaces "run pre-dates 0.5-S2f, cannot wait" exit 2) |
| `workflow_def` exceeds 1 MiB at submit time | `WorkflowError::Validation` | wait.workflow_def_too_large (additive new kind) |
| Run not found | `PersistenceError::NotFound` | wait.run_not_found |
| `--data-dir` not provided + no env var | clap error | exit 2 |
| Approval-gate step becomes ready | `WorkflowError::ApprovalNotSupportedInWait` | wait.approval_step_unsupported |
| External-trigger step becomes ready | analogous | wait.trigger_step_unsupported |
| `--max-wait-secs` exceeded | timeout | wait.timeout |

All emitted error_kinds are stable per convention #2; new
kinds prefixed `wait.*` are additive.

## Audit lifecycle

Submit-only emits `WorkflowStarted` audit event. The wait
client should NOT re-emit `WorkflowStarted` (the run already
started at submit time). New Pending checkpoints in
subsequent waves are tracked by per-step `StepCheckpointed`
events (already emitted by `upsert_step_checkpoint` per
existing behavior — confirm in code). At terminal status,
the wait client emits a single `WorkflowFinished` event. The
hash chain stays intact across submit/wait/coord boundaries.

## Test integration

- **Unit (`cargo test -p boruna-orchestrator`):**
  3 tests for `compute_ready_steps`, 1 idempotency test for
  `advance_run_one_tick`, 1 size-cap test for
  `workflow_def`, 1 race test for concurrent claim+wait.

- **CLI integration (`cargo test -p boruna-cli --features
  serve --test cli_coordinator_worker`):**
  1 marquee test (multi-wave end-to-end), 1 crash-recovery
  test (kill wait mid-run, re-invoke, complete).

## Files touched (estimate)

- `orchestrator/src/workflow/runner.rs` (~+200 lines: 3
  fns + struct field + tests)
- `orchestrator/src/workflow/definition.rs` (no change)
- `orchestrator/src/persistence/mod.rs` (no schema change;
  possibly minor helper)
- `crates/llmvm-cli/src/main.rs` (CLI variant + dispatch)
- `crates/llmvm-cli/src/coordinator.rs` (~+150 lines: wait
  handler)
- `crates/llmvm-cli/tests/cli_coordinator_worker.rs`
  (~+200 lines: 2 integration tests)
- `docs/design-coord-wait.md` (this sprint's design)
- `docs/architecture-coord-wait.md` (this doc)
- `docs/test-plan-coord-wait.md` (test plan)
- `CHANGELOG.md` (Unreleased section)

Total estimated diff: ~600-800 lines (in line with prior
sub-sprints).
