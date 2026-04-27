# Design — `boruna coordinator wait` (sprint 0.5-S2f)

## Premise

Sprints 0.5-S1 through 0.5-S2e shipped the entire single-wave
distributed-execution stack. `workflow run --submit-only`
inserts a run row + the **first wave's** Pending checkpoints,
then exits; coord+workers drive that wave to completion. The
last gap is **multi-wave advancement**: when wave 1 completes,
something has to write Pending checkpoints for wave 2's
downstream-ready successors, and so on until the run is
terminal.

Two architectural shapes are possible (see
`distributed-execution-architecture` memory for the framing):
client-side `coordinator wait` driver vs. coordinator-side
DAG advancement. **This sprint takes the client-side path** to
preserve the "coordinator is dumb transport" invariant locked
in 0.5-S2c. The coordinator gains zero new
workflow-DAG logic; a new pure-read+write client driver does
the advancement work.

## Who needs this

- **Operators running multi-wave workflows in production.**
  Today they get only the first wave executed by remote
  workers. After this sprint they get the entire DAG.
- **The 0.5-S3 implementer.** Once `coordinator wait` exists,
  layering on `--coordinator <url>` (truly remote submit) and
  authentication is incremental, not architectural.

## Narrowest MVP

```sh
# Same coord+workers cluster as 0.5-S2e.
boruna coordinator serve --data-dir /var/lib/boruna &
boruna worker run --coordinator http://127.0.0.1:8090 &

# Submit, then drive to terminal.
boruna workflow run examples/workflows/document_processing \
    --submit-only --data-dir /var/lib/boruna
# stdout: submitted run_id=<id>

boruna coordinator wait <id> --data-dir /var/lib/boruna
# Polls runs.db every 500 ms.
# As each step completes, computes next-ready successors,
# writes Pending checkpoints (with step_sources).
# Exits 0 when run is Completed; non-zero on Failed.
```

The wait client:
1. Opens `RunCheckpointStore` directly via `--data-dir`.
2. Reads `metadata_json.workflow_def` (new field) once at
   start.
3. Polls `list_step_checkpoints(run_id)` at a fixed interval.
4. On each poll: derives the set of ready-but-not-started
   steps using the topological-level structure of the
   `WorkflowDef` and the current step status map.
5. For each newly-ready step: writes its Pending checkpoint
   (with `step_source` from the metadata's `step_sources` map)
   via the same `RunCheckpointStore` API the submit-only path
   uses.
6. When the run reaches a terminal status (all steps
   Completed → run Completed; any step Failed → run Failed),
   exits with code 0 or non-zero accordingly.
7. **Idempotent restart**: re-invoking `coordinator wait`
   against an in-progress run picks up state from runs.db and
   continues from there. No state lives in the wait client.

## What would make someone say "whoa"

- **The coordinator gains exactly zero new logic.** All
  multi-wave advancement is client-side. The same coord
  binary that shipped 0.5-S2e drives 0.5-S2f workflows.
- **Restart-safe.** Kill the wait client at any point; restart
  it; the run continues. No "in-flight" state on the client.
- **Multi-host friendly within the coordinator's host.** The
  wait client only needs `--data-dir` (filesystem access to
  runs.db). It can run from the same host as the coordinator
  but a different process / shell / cron job.
- **Symmetric with `--submit-only`.** Submit writes initial-
  wave Pending; wait writes downstream Pending. Same
  persistence API, same audit-event emission pattern.

## How this compounds

- **0.5-S3 — auth + remote submit.** Once `coordinator wait`
  is the standard advancement mechanism, adding HTTP-based
  remote submit is a surface-level change. The advance loop
  doesn't move.
- **0.5-S4 — `boruna workflow run --coordinator <url>`** can
  internally invoke a `wait`-equivalent loop, giving the "one
  command" experience while preserving the dumb-transport
  invariant.
- **Future coordinator-side advancement (0.6.x+)** is now an
  optimization, not a redesign. If operators want the client-
  doesn't-stay-running property, the coordinator can adopt
  the same advance loop without breaking client-side users.

## Scope (what this sprint changes)

- New: `PersistedRunMetadata::workflow_def: Option<WorkflowDef>`
  field with `#[serde(default)]` for back-compat.
- New: at submit_only time, `prepare_persistent_run` embeds
  the full `WorkflowDef` into metadata alongside
  `step_sources`. Capped at 1 MiB serialized JSON.
- New: `WorkflowRunner::compute_ready_steps(def,
  step_status_map) -> Vec<String>` — pure function that
  returns next-ready step IDs given a DAG + completion state.
- New: `WorkflowRunner::advance_run_one_tick(run_id) ->
  AdvanceResult` — one polling tick: read state, derive
  ready set, write Pending checkpoints.
- New: `boruna coordinator wait <run-id>` CLI subcommand.
- Test: 3 unit tests on `compute_ready_steps`; 1 unit test
  on `advance_run_one_tick` idempotency; 1 CLI integration
  test for end-to-end multi-wave flow; 1 CLI integration
  test for crash-recovery (kill mid-run, re-invoke, all
  steps complete).

## Non-goals (deferred)

- **No coordinator-side advancement.** Strictly client-side
  this sprint.
- **No `--coordinator <url>` mode.** Wait client uses
  `--data-dir` only; HTTP-based remote wait is a future
  sprint.
- **No `--follow` / streaming progress display.** Wait prints
  state changes (line per step transition) but no fancy TUI.
- **No approval gates / external triggers in non-first
  waves.** The submit-only path already rejects them; if a
  wave-2+ step is approval-kind, the wait client surfaces a
  typed error and exits non-zero. Future sprint plumbs them
  through.
- **No retry policy in advance loop.** A failed step → run
  Failed → wait exits. Sibling steps in the same wave are
  allowed to finish; downstream steps stay Pending forever
  (operator's call to investigate).
- **No auth, no capability tagging, no rolling upgrades, no
  output blob refs.** All deferred to 0.5-S3+.

## Documented limitations

These were validated by adversarial review (`ce-correctness-reviewer`)
and accepted for v0.5.x. Each will be revisited in 0.5-S3+ or
0.6.x as appropriate.

1. ~~**Distributed retry policies are not honored.**~~
   **Resolved by sprint 0.5-S5.** The wait driver reads each
   Failed step's `RetryPolicy` and requeues (Failed → Pending)
   when budget remains and the error class matches the policy's
   `retry_on` list (or `on_transient` fallback). Run-status
   declares `Failed` only when a step is `Failed` AND has no
   retry budget left. See `WorkflowRunner::advance_run_one_tick`
   retry pass and `RunCheckpointStore::requeue_failed_step_for_retry`.

2. **Audit chain has no wait-driven terminating event.**
   Submit-only emits `WorkflowStarted`. The wait driver does
   NOT append `WorkflowCompleted` / `WorkflowFailed` on
   terminal transition. Auditors should query
   `runs.status` directly for the terminal-status truth.

3. **Concurrent wait clients are safe but untested.** Two
   `coordinator wait` processes against the same `run_id` will
   each read state, compute the same ready set, and race to
   call `insert_pending_step_if_absent`. The
   `INSERT ... ON CONFLICT DO NOTHING` primitive guarantees
   only one win, so both processes converge to the same
   terminal exit. No integration test exercises this scenario.

4. **HTTP-based remote wait deferred.** The wait client opens
   `runs.db` directly via `--data-dir`. Remote operators must
   submit and wait on the coordinator's host. A future
   `coordinator wait --coordinator <url>` mode would relax this.

## Stable contract

- `coordinator wait <run-id> --data-dir <path>` CLI surface.
- `metadata_json.workflow_def` field name and shape (matches
  `orchestrator::workflow::WorkflowDef` serde shape).
- Exit codes: 0 = run Completed; non-zero = run Failed,
  unknown run, or persistent error.
- Polling interval default 500 ms; configurable via
  `--poll-interval-ms` (minimum 100 ms with auto-clamp +
  warning, mirroring the coord background sweep pattern from
  0.5-S2c).
- Idempotency: writing a Pending checkpoint for a step that
  is already in any non-Pending status is a no-op (handled
  by existing `upsert_step_checkpoint` semantics).

## Stability tier

Per `docs/stability.md`: **experimental**. The
`workflow_def` field is `#[serde(default)]`-additive; the
`coordinator wait` CLI surface is new and may evolve. The
underlying persistence APIs (claim/lease, CAS) are unchanged.

## Test plan

See `docs/test-plan-coord-wait.md` for the full matrix.
Marquee tests:
1. `compute_ready_steps_returns_topological_next_set`.
2. `advance_run_one_tick_is_idempotent`.
3. `cli_coordinator_wait_drives_multi_wave_to_completion`.
4. `cli_coordinator_wait_resumes_after_kill`.

## Adversarial review focus areas

1. **Race between coordinator's claim/dispatch and wait
   client's checkpoint write.** Both write to runs.db
   concurrently. SQLite WAL + `BEGIN IMMEDIATE` retry
   (convention #13) covers row-level conflicts, but: can
   the wait client write a Pending checkpoint for a step
   that the coordinator is simultaneously transitioning
   from Pending → Running on a different worker? The
   `upsert_step_checkpoint` semantics + COALESCE pattern
   (convention #14) should preserve the in-flight Running
   state; verify with a regression test.

2. **`workflow_def` field size cap.** Real workflows can have
   100s of steps. Serialized JSON for a 100-step workflow is
   roughly 50–80 KiB; 1 MiB cap is generous but check that a
   pathological case (huge prompt strings in `with_input`
   blocks) doesn't blow it. Reject with typed `Validation`
   error at submit time, mirroring the 0.5-S2e step_sources
   pattern (convention #1, #5).

3. **Determinism of next-ready ordering.** `compute_ready_steps`
   must return a deterministic order — sort by step_id
   ascending so replay-equivalence tests pass and audit logs
   are stable. BTreeMap iteration over `WorkflowDef::steps`
   is already sorted; preserve that.

4. **Lease-expiry interaction.** A step that gets re-claimed
   after lease expiry transitions Running → Pending → Running
   with a higher claim_id. The wait client must not write a
   Pending checkpoint that overwrites the in-flight Running
   row. The slow-worker-race CAS test in persistence/mod.rs
   is the load-bearing invariant; verify the wait client
   never bypasses CAS.

5. **Failed-run handling.** When a step fails, downstream
   steps stay Pending forever from the wait client's
   perspective. The wait client should detect overall run-
   level Failed status and exit non-zero promptly, not poll
   indefinitely.

6. **Approval-gate / external-trigger steps in non-first
   waves.** The submit-only path doesn't reject them — it
   just doesn't see them at submit time (they're in wave 2+).
   The wait client encounters them when they become ready.
   Surface a typed error and exit; don't silently skip.

7. **Audit lifecycle.** Each new Pending checkpoint should
   emit an audit event so the chain has continuity, mirroring
   the `WorkflowStarted` event emitted by submit-only.
   Confirm `upsert_step_checkpoint` already does this, or
   add it explicitly.
