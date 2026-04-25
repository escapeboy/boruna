# Test Plan — 0.3-S2b

**Reads:** `architecture-0.3-s2b-persistence-wiring.md`. Each scenario maps to one or more `#[test]` functions. The Test phase asserts every row passes.

## Layer 1 — Persistence (orchestrator/src/persistence/mod.rs)

| ID | Scenario | Pass criteria |
|---|---|---|
| P-1 | `count_runs_for_workflow` returns 0 on empty DB | `count == 0` for any `workflow_hash` |
| P-2 | `count_runs_for_workflow` increments per insert | After 3 inserts with the same `workflow_hash`, count == 3; with a different hash, count == 0 |
| P-3 | `get_run_record` returns `None` for missing run | `Ok(None)` |
| P-4 | `get_run_record` returns `Some(RunRecord)` with `terminal_status = Some(Completed)` for completed run | Insert run, update to Completed, fetch record, assert `terminal_status == Some(RunStatus::Completed)` |
| P-5 | `get_run_record` returns `terminal_status = None` for `Running` | Audit code must branch on Some/None, not on Running |
| P-6 | `get_run_operational` returns `transient_status` reflecting current state | After `update_run_status(_, Paused, _)`, fetch operational, assert `transient_status == Paused` |
| P-7 | `RunRecord` does NOT expose `started_at_ms` field | Compile-time check (omitted field) |
| P-8 | `RunOperational` does NOT expose `policy_json` or `workflow_hash` | Compile-time check |
| P-9 | All existing `RunRow` tests still pass | No regression in `0.3-S2a`'s 18 persistence tests |

## Layer 2 — `derive_run_id` determinism

| ID | Scenario | Pass criteria |
|---|---|---|
| D-1 | Same inputs + counter ⇒ same `run_id` | Two calls with identical args return equal strings |
| D-2 | Different counter ⇒ different `run_id` | counter=0 vs counter=1 return different strings |
| D-3 | Different `workflow_hash` ⇒ different `run_id` | Self-evident |
| D-4 | Different `inputs_hash` ⇒ different `run_id` | Self-evident |
| D-5 | Golden `run_id` for known triple | Hard-coded vector: `workflow_hash="dead"`, `inputs_hash="beef"`, `counter=0` ⇒ specific 16-char hex (computed externally with `printf "...:...:..." \| shasum -a 256` and pasted in) |

D-5 is the canonical golden — locks the algorithm against accidental change.

## Layer 3 — `WorkflowRunner::run_persistent`

| ID | Scenario | Pass criteria |
|---|---|---|
| RP-1 | Running a 3-step workflow writes 1 run row + 3 step checkpoint rows | After completion, store has the expected rows; all step statuses == Completed |
| RP-2 | Run with a failing step transitions run status to Failed | `RunRecord.terminal_status == Some(Failed)`; failing step has `status=Failed` and `error_msg` populated |
| RP-3 | Run with an approval gate persists `awaiting_approval` checkpoint | Run status persisted as `Paused`; gate step persisted as `awaiting_approval` |
| RP-4 | Re-running the same workflow definition + inputs creates a 2nd run with a different `run_id` | First run counter=0, second counter=1 |
| RP-5 | Concurrent inserts (2 threads, same workflow_hash) produce distinct run_ids | Use `Arc<Mutex<RunCheckpointStore>>` → 2 threads call `run_persistent` → assert run_ids differ AND no FK violation |
| RP-6 | Terminal state's `output_hash` equals the in-memory step result's hash | Bit-identical replay-verified column |

## Layer 4 — `WorkflowRunner::resume` (the headline feature)

| ID | Scenario | Pass criteria |
|---|---|---|
| R-1 | Resume of a never-existed `run_id` returns `WorkflowRunError::RunNotFound` | Verifies typed error path (project-conventions §1) |
| R-2 | Resume of a `Completed` run is a no-op (returns the existing result without re-execution) | Step results returned without re-running |
| R-3 | Resume of a `Paused` (approval) run with no approval sentinel re-pauses with no progress | Same as before-resume state |
| R-4 | Resume of a `Paused` run **with** approval sentinel proceeds past the gate | Steps after the gate execute; final status Completed |
| R-5 | Resume of a `Failed` run does NOT silently re-run | Returns the existing failed result; explicit "re-run from scratch" requires a new `run` invocation |
| R-6 | Resume against a workflow file whose hash differs from the persisted one returns `WorkflowHashMismatch` | Modify a step's source file between run + resume; verify the typed error |
| R-7 | Mid-run crash simulation (in-process): truncate execution after step 1 completes; reopen store; resume; assert step 2 + step 3 execute and step 1 is NOT re-executed | Test uses a sentinel file written by step 1; assert the file is written exactly once across the run + resume |
| R-8 | Resume with a `running`-status step at resume-time RE-EXECUTES that step | Crash-during-step semantics: trust only `completed` |
| R-9 | Resume produces output bit-identical to an unkilled control run | Compare `output_hash` of every step + final `RunRecord.terminal_status` |

R-7 and R-9 together are the headline determinism-through-process-boundary proof.

## Layer 5 — Subprocess SIGKILL (opt-in, locally-run)

| ID | Scenario | Pass criteria |
|---|---|---|
| SK-1 | `cargo run -- workflow run …` spawned as subprocess; SIGKILL after 200ms; `boruna workflow resume <run-id>` completes | Final state `Completed`; intermediate evidence bundle hashes match a control run |

`#[ignore]`-gated; runs locally with `cargo test -p boruna-orchestrator --test crash_recovery -- --ignored`. Not on CI by default (subprocess + filesystem timing on the self-hosted runner risks flakiness).

## Layer 6 — CLI surface

| ID | Scenario | Pass criteria |
|---|---|---|
| C-1 | `boruna workflow run dir --data-dir /tmp/x` creates `/tmp/x/runs.db` | File exists post-run |
| C-2 | `boruna workflow run dir --ephemeral` creates no DB file | No `*.db` in CWD or any `.boruna` dir |
| C-3 | `boruna workflow resume <run-id> --data-dir /tmp/x` succeeds for a paused run | Stdout shows progression past the gate (when sentinel is set) |
| C-4 | `boruna workflow resume <bad-id>` exits non-zero with `run not found` | Exit code != 0; error message matches |
| C-5 | `BORUNA_DATA_DIR` env var is honored when `--data-dir` is omitted | Works in a clean shell |
| C-6 | `--data-dir /` is rejected with a clear error | Defense against accidental system-root writes |

## Layer 7 — Gates

- `cargo test --workspace` — green
- `cargo test --workspace --features boruna-orchestrator/persist-sqlite` — green (default already on, but verify explicitly)
- `cargo test --workspace --features boruna-vm/http` — green
- `cargo clippy --workspace -- -D warnings` — clean
- `cargo fmt --all -- --check` — clean
- `cargo run --bin boruna -- workflow run examples/workflows/llm_code_review --policy allow-all --data-dir /tmp/test` — green end-to-end smoke
- Hand-verify the demo from `design-0.3-s2b-persistence-wiring.md` § Acceptance criteria

## What we explicitly do NOT test this sprint

- Cross-host resume (data_dir on NFS / network FS).
- Subprocess SIGKILL on CI (only locally).
- Async / parallel step persistence (deferred to `0.3-S4`).
- The `approve` CLI workflow (deferred to `0.3-S2c`).
