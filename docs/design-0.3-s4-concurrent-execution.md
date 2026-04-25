# Design — 0.3-S4: Concurrent Step Execution

**Status:** 2026-04-25
**Predecessor:** `0.3-S3` shipped atomic-rename + workflow show, closing the persistence story for production. This sprint adds the parallelism that the persistence chassis enables.

## Scope

Steps with no `depends_on` between them execute concurrently. Operators opt in with `--concurrency <N>`; default stays `1` (sequential, today's behavior). Persistent-path only.

**In scope:**

1. **Wave-based execution.** Compute topological levels (BFS from sources): all steps at level N have all dependencies at levels < N. Within a wave, steps execute in parallel up to `--concurrency`.
2. `RunOptions::concurrency: usize` with default 1 = sequential (no behavior change for existing callers).
3. New CLI flag `--concurrency <N>` on `boruna workflow run`. `0` rejected at parse (project-conventions §1).
4. Worker threads each open their own `RunCheckpointStore` connection — SQLite WAL mode supports multiple writers serializing on `BEGIN IMMEDIATE` + busy-retry (already in place).
5. `DataStore` thread-safety: workers receive a snapshot of resolved inputs; outputs are merged back in the coordinator after each wave (no shared mutable DataStore).
6. Approval gates inside a wave: detected by the coordinator before dispatching to a worker — pause is sequential, gates don't go through the thread pool.
7. Step failures inside a wave: in-flight steps complete; downstream waves are skipped; run halts as Failed.

**Out of scope (deferred):**

- **Full DAG scheduler.** Wave-based loses some parallelism (a slow step at level N blocks fast steps at level N+1 even if they don't actually depend on it). Acceptable trade-off for sprint simplicity; a real DAG scheduler is a separate ADR.
- **Concurrent execution on the ephemeral path.** `WorkflowRunner::run` (no persistence) stays single-threaded — keeps tests + simple invocations unchanged.
- **Tokio.** The VM is synchronous; adding a runtime just to fan out blocking work isn't worth the dependency surface.
- **Cross-process or cross-host concurrency.** Single-writer-by-process holds.

## Forcing questions (Think)

**Who needs this?**
Operators running fan-out workflows — e.g. `document_processing` (1 ingest → 3 parallel branches → 1 merge). Today the 3 branches run serially: a 2s LLM call × 3 = 6s wall. With concurrency=3: 2s wall.

**Narrowest MVP?**
`--concurrency 4` on `boruna workflow run document_processing` produces the same step outputs as `--concurrency 1` (determinism preserved) but runs ~3× faster end-to-end. Wave granularity is enough — full DAG scheduling is sprint-S6+.

**What would make someone say "whoa"?**
Re-running an existing 30-step workflow with `--concurrency 8` and seeing it complete 5× faster while every persisted `output_hash` is bit-identical to the sequential run.

**How does this compound?**
Closes the parallel-execution gap that LLM-batch and document workflows really need. Future retry-policy (`0.3-S5`) and async-step (deferred) sprints can build on the wave abstraction without rearchitecting.

## Key invariants (must not regress)

1. **Determinism.** Same inputs → same step outputs and same `output_hash` per step regardless of concurrency level. The `WorkflowRunResult.step_results` map is BTreeMap-keyed (already deterministic). Per-step wall-clock fields are operational-only (project-conventions §15/§17).
2. **Approval gate semantics.** A run with an approval gate at level N pauses with the same step_results regardless of concurrency. Steps in level N before the gate complete normally; downstream levels (N+1+) don't run.
3. **Failure semantics.** A failed step halts the run. Other steps in the SAME wave that have already started complete; the run is Failed. Downstream waves don't run.
4. **`run_id` derivation unchanged.** Concurrency doesn't feed the hash chain.
5. **Crash-recovery via `resume`.** Mid-wave crashes leave Running checkpoints; resume re-executes those. Same as 0.3-S2b semantics; verified by an integration test.

## Architecture sketch

```
┌─ run_persistent (data_dir, options{concurrency: N}) ─┐
│ 1. Validate, topo-order, open store, insert run row  │
│ 2. compute_topological_levels(def) → Vec<Vec<step>>  │
│ 3. for wave in levels:                               │
│    a. partition wave: { gates: [...], sources: [...] }
│    b. for each gate: handle inline (pause if active) │
│    c. fan out sources via thread pool (bounded by N) │
│    d. merge worker outputs back into DataStore       │
│    e. write Completed checkpoints (worker's own conn)│
│    f. if any failed: halt + persist Failed status    │
│ 4. update run terminal status                        │
└──────────────────────────────────────────────────────┘
```

Worker thread (per step):
- Owns an `Arc<Path>` to data_dir (opens its own `RunCheckpointStore` connection)
- Receives `(step_id, step_def, resolved_inputs: BTreeMap<String, Value>, options snapshot)` via `move` closure
- Writes Running checkpoint via own connection
- Compiles + executes VM
- Writes Completed (or Failed) checkpoint
- Returns `(step_id, Result<Value, String>, output_hash, capabilities, duration_ms)` via `JoinHandle`

Coordinator (main thread):
- Holds the canonical `DataStore` (single-threaded mutation)
- Builds resolved-inputs snapshot for each step before dispatch
- Calls `data_store.store_output` for each successful worker after the wave completes (single-thread mutation; atomic-rename from 0.3-S3 means even though the disk file was already written by the worker via its own DataStore, re-writing the same value is idempotent — actually we should NOT have workers write to disk via DataStore; only the coordinator should). See architecture doc for the disk-write convention.

## Open questions (resolved in Plan)

- **Q1: Should workers write to `outputs/` directly or send Values back to the coordinator?**
  Workers writing leads to two writers per step on the same path (worker writes during execution; if we re-write in coordinator, we double-write). **Decision: workers do NOT call `DataStore::store_output`.** They run the VM, capture the Value, and return it. The coordinator alone calls `data_store.store_output` after the wave. Single-writer per output file. This also means workers don't need a DataStore at all — they need the resolved inputs map up front.

- **Q2: Concurrency bound enforcement.** Use `std::sync::Arc<std::sync::Semaphore>`-equivalent via `std::thread::available_parallelism` cap? Simplest: chunk the wave into `chunks(N)` and spawn N at a time. **Decision: chunk-and-join.** Each chunk spawns up to N threads, joins all, moves to next chunk. Loses some throughput vs a true semaphore (tail-of-chunk waiting) but keeps the code dead simple.

- **Q3: Error propagation across workers.** If worker A panics mid-step, what happens? **Decision:** `JoinHandle::join` returns `Err` on panic; the coordinator surfaces this as a typed `WorkflowRunError::Internal("step '<id>' panicked: ...")`. Other workers in the same chunk still complete (we always join all). Run is marked Failed.

- **Q4: How does this interact with `RunOptions.live` (real HTTP)?**
  Workers each create their own `CapabilityGateway` with the configured policy + (if live) `HttpHandler::new(net_policy)`. Workers don't share gateway state. Each VM instance is per-step, per-worker.

- **Q5: Concurrency = 0?** Reject at parse via the CLI; `RunOptions::concurrency = 0` falls through to sequential at the runner level (defensive). project-conventions §1 — typed CLI error.

## Risks

- **Step-execution panics** propagate to the coordinator's `join().unwrap()`. Fix: handle `join()` errors explicitly.
- **DataStore in-memory cache stale after a wave.** Coordinator writes via `store_output` after each wave — guaranteed sequential update.
- **SQLite WAL contention** if many workers race on `BEGIN IMMEDIATE`. The retry policy already handles this; add a regression test that confirms a 4-worker wave completes without `Busy` errors.
- **Test flakiness** from thread scheduling. Mitigate with deterministic assertions (same outputs regardless of order, not "thread A finishes before thread B").

## Audit contract for failed runs at concurrency > 1 (review-driven)

When a step fails inside a wave at `concurrency > 1`, sibling steps in
the same chunk that have already started keep running until they
complete (we always join all spawned threads — no detached threads,
no torn workflow_dir). The completed siblings' outputs are persisted
as Completed checkpoints with their real `output_hash`.

This means: at concurrency=1, a chunk-of-3 with a failure at step B
produces `step_results = {A: Completed, B: Failed}` (C never runs). At
concurrency=3, the same workflow may produce
`step_results = {A: Completed, B: Failed, C: Completed}` — C did
complete, and its output is real.

**Audit implication.** Evidence bundles for failed runs at
concurrency > 1 may include more `output_hash` entries than a
sequential run would. This is honest reporting of what actually
executed, not a bug. The replay-verified determinism contract still
holds: any single (workflow, inputs, concurrency) tuple replays
bit-identically across machines. Cross-concurrency replay of failed
runs is NOT a contract — operators comparing audit bundles across
concurrency levels should expect divergence on failure paths.

Successful runs are concurrency-invariant: per-step `output_hash` and
the set of completed step_ids are identical regardless of concurrency
level. Locked by the headline regression test.

## Acceptance criteria

- `cargo test --workspace` green including new concurrent-execution regression tests.
- `cargo clippy -D warnings` clean.
- `cargo fmt --check` clean.
- Manual demo: `boruna workflow run examples/workflows/document_processing --policy allow-all --concurrency 4 --data-dir /tmp/d` completes Successfully with the same `output_hash` per step as `--concurrency 1`.
- A regression test runs the same workflow at concurrency 1 and 4, asserts every step's `output_hash` is bit-identical between the two runs.
