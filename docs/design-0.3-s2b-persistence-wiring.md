# Design — 0.3-S2b: Wire RunCheckpointStore into WorkflowRunner

**Status:** Drafted 2026-04-25 (Think phase)
**Predecessor:** `0.3-S2a` shipped `RunCheckpointStore` API; `WorkflowRunner` still uses `tempfile::tempdir()`.
**ADR:** [`docs/adr/001-persistence-backend.md`](adr/001-persistence-backend.md) — binding for all decisions below.

## Scope

Workflow runs survive process restarts. Operators can list paused runs, resume the one they care about, and trust that the resumed run produces output bit-identical to the would-have-been single-process run.

**In scope:**

1. `WorkflowRunner` opens a `RunCheckpointStore` (new `--data-dir` flag, defaulted) and writes a checkpoint row at every step transition.
2. Replace the wall-clock `run_id` derivation (`runner.rs:49-53`) with the deterministic pattern from project-conventions §16.
3. Add `boruna workflow resume <run-id>` CLI subcommand.
4. Split `RunRow` into replay-verified vs. operational subsets per H1 review finding.
5. SIGKILL crash-recovery integration test (kill mid-step → resume → expect identical final state).

**Out of scope (deferred to later 0.3 sprints):**

- Async/concurrent step execution (`0.3-S4`).
- Retry policy refinements beyond what `runner.rs` already does (`0.3-S5`).
- Multi-process / scheduler-daemon writer contention (`0.3-S7`).
- Cross-machine resume (resumes are bound to the host that holds `--data-dir`).

## Forcing questions (Think)

**Who needs this? What are they doing today?**
Compliance-driven Boruna integrators running long workflows (LLM batch grading, document fan-outs) on hosts that get restarted. Today: a process restart loses the run; they re-run from scratch and burn LLM budget twice.

**What's the narrowest MVP someone would pay for?**
SIGKILL the runner mid-step, restart with `boruna workflow resume <run-id>`, observe identical output and zero re-execution of completed steps. That's it. Not multi-host, not async, not retry-policy magic.

**What would make someone say "whoa"?**
Demonstrating that the resumed `evidence verify` bundle hash-equates the unkilled-run bundle. Determinism through a process boundary is the differentiator vs. every other workflow engine (which all rebuild caches from scratch on resume).

**How does this compound?**
It unlocks `0.3-S4` (async steps): once the durable boundary is there, parallel step execution can flush completion events to the same store without rewriting the resume contract. Persistence is the chassis; everything 0.3+ bolts onto it.

## Key invariants (must not regress)

1. **Determinism:** `run_id`, replay-verified columns, terminal statuses are bit-identical across runs given identical inputs. Wall-clock columns (`started_at_ms`, `updated_at_ms`, `ended_at_ms`) are operational-only and never feed a hash or a query's `ORDER BY`.
2. **Crash safety:** Every `BEGIN IMMEDIATE` retry path stays intact; `foreign_keys = ON` PRAGMA stays mandatory.
3. **Workflow integrity at resume:** Refuse to resume against a workflow definition whose `workflow_hash` no longer matches what was persisted.
4. **No silent footguns:** Typo'd `run_id` on `resume` returns `PersistenceError::NotFound` (per project-conventions §1).
5. **`protocol_version: 1` envelope** stays on every MCP response touched (none directly mutated this sprint, but resume-related metadata may surface via `boruna_run` results — verify nothing drifts).

## Open questions (to resolve in Plan)

- **Q1: `--data-dir` default.** Options: (a) `./.boruna/data` (relative to CWD — discoverable, but ambiguous for daemons), (b) `$BORUNA_DATA_DIR` with no default (operator must opt in — safest, less ergonomic), (c) `~/.boruna/data` (cross-CWD, surprising in CI). **Lean toward (a) with explicit `--data-dir` override and `BORUNA_DATA_DIR` env var taking precedence.**
- **Q2: `run_id` counter source.** ADR pattern is `sha256(workflow_hash + serialized_inputs + counter)`. Counter must be persisted (otherwise re-running same inputs produces same `run_id` → UNIQUE violation). Lean toward `MAX(counter) + 1` queried at insert time, scoped by `(workflow_hash, inputs_hash)`. New schema column or sidecar table required — additive, won't bump `SCHEMA_VERSION`.
- **Q3: Resume against partial writes.** If the runner crashed mid-`upsert_step_checkpoint`, the row may show `Running` with no output. Resume must re-execute that step, not skip it. Requires a "transient → re-execute" rule documented in code + test.
- **Q4: Approval-gate resume.** Existing approval-gate code prints `boruna workflow approve <run-id> <step-id>` (which doesn't exist yet either — separate sprint). For 0.3-S2b: ensure resume picks up an `awaiting_approval` step correctly even though the `approve` subcommand is still absent. Out-of-band approval (manual SQL) should suffice for the test.
- **Q5: `RunRow` split shape.** Single struct with `#[doc]` annotations, OR two structs (`RunRecord` + `RunOperationalState`)? Two structs forbid the bug at compile time (you can't accidentally `ORDER BY started_at`); single struct is simpler. **Lean toward two structs returned as a tuple from `get_run`**, with a thin `Run { record, operational }` wrapper for ergonomic call sites.

## Risks

- **Schema additions** (counter column or sidecar) are technically additive but reviewers will scrutinize per ADR 001's "schema_version stays at 1 only for additive changes." Document the additive rationale up front.
- **Crash-recovery test flakiness on CI.** SIGKILL + subprocess + filesystem timing is a recipe for intermittent failures on slow runners. Mitigate with deterministic wall-clock-free assertions and a generous post-resume settle window (or use a synchronous in-process simulation if subprocess proves flaky on the self-hosted runner).
- **`--data-dir` default ambiguity** if a daemon is started from `/`. Make the default explicit + early-warning if the resolved path is a system root.

## Acceptance criteria (Build → Test gate)

- `cargo test --workspace` passes with the new crash-recovery integration test.
- `cargo clippy --workspace -- -D warnings` clean.
- `cargo fmt --all -- --check` clean.
- A manual end-to-end demo (documented in CHANGELOG):
  1. `boruna workflow run examples/workflows/document_processing --data-dir /tmp/d --policy allow-all` (kill at step 2)
  2. `ls /tmp/d/runs.db` exists
  3. `boruna workflow resume <run-id> --data-dir /tmp/d` completes
  4. `boruna evidence verify <bundle-dir>` PASSED
  5. Hash of final output equals an unkilled control run.

Plan phase converts these questions into structural decisions and the test plan locks the assertions.
