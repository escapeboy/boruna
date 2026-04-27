# Test Plan — `boruna coordinator wait` (sprint 0.5-S2f)

Companion to `docs/design-coord-wait.md` and
`docs/architecture-coord-wait.md`.

## Acceptance criteria

A sprint is shippable when ALL of the following pass:

- [ ] `cargo test --workspace` — green
- [ ] `cargo test -p boruna-orchestrator` — green, with the
      new advance-loop tests counted
- [ ] `cargo test -p boruna-cli --features serve` — green,
      with the 2 new integration tests counted
- [ ] `cargo clippy --workspace -- -D warnings` — clean
- [ ] `cargo clippy -p boruna-cli --features serve -- -D warnings`
      — clean
- [ ] `cargo fmt --all -- --check` — clean
- [ ] Marquee end-to-end test
      (`cli_coordinator_wait_drives_multi_wave_to_completion`)
      runs a 3-wave example workflow through coord+worker
      and asserts ALL steps Completed via the wait driver
- [ ] Crash-recovery test
      (`cli_coordinator_wait_resumes_after_kill`) kills the
      wait process between waves and asserts the run
      completes when wait is re-invoked

## Test matrix

### Unit tests in `orchestrator`

| # | Test | File | Expectation |
|---|---|---|---|
| 1 | `compute_ready_steps_initial_state_returns_source_steps` | `workflow/runner.rs` (mod tests) | A workflow with 3 source steps and 2 downstream returns `[s1,s2,s3]` sorted ascending given an empty status map. |
| 2 | `compute_ready_steps_after_first_wave_returns_downstream` | same | Same workflow, status_map = {s1:Completed, s2:Completed, s3:Completed} returns `[s4, s5]` sorted ascending. |
| 3 | `compute_ready_steps_partial_completion_returns_empty` | same | Status_map = {s1:Completed, s2:Pending, s3:Completed} returns `[]` (s4, s5 depend on s2 still). |
| 4 | `compute_ready_steps_skips_already_pending_steps` | same | Status_map = {s1:Completed, s2:Completed, s3:Completed, s4:Pending} returns `[s5]` (s4 already pending, not Unknown). |
| 5 | `compute_ready_steps_sort_is_deterministic` | same | A workflow with non-alphabetic step IDs returns ascending-sorted output regardless of insertion order. |
| 6 | `advance_run_one_tick_inserts_pending_checkpoints` | same | Given a run with completed wave-1 checkpoints, one tick inserts wave-2 Pending checkpoints + populates step_source from metadata.step_sources. |
| 7 | `advance_run_one_tick_is_idempotent` | same | Running the tick twice with no completion changes between ticks produces zero new Pending checkpoints. |
| 8 | `advance_run_one_tick_preserves_running_status_on_concurrent_claim` | same | Race scenario: between read and write of the tick, a step transitions Pending→Running. The tick's upsert must NOT clobber Running back to Pending. |
| 9 | `advance_run_one_tick_returns_completed_when_all_done` | same | All steps Completed → AdvanceResult.run_status = Completed. |
| 10 | `advance_run_one_tick_returns_failed_when_any_failed` | same | Any step Failed → AdvanceResult.run_status = Failed (even with siblings still Running). |
| 11 | `prepare_persistent_run_embeds_workflow_def_when_submit_only` | same | After submit-only, `metadata_json.workflow_def` round-trips equal to the input WorkflowDef. |
| 12 | `prepare_persistent_run_rejects_oversized_workflow_def` | same | A WorkflowDef serializing to >1 MiB returns a typed Validation error at submit time with `error_kind: "wait.workflow_def_too_large"`. |
| 13 | `prepare_persistent_run_does_not_embed_workflow_def_in_in_process_mode` | same | When `submit_only=false` (current default), workflow_def is None to keep metadata small. |

### CLI integration tests in `boruna-cli` (feature serve)

Existing harness: `crates/llmvm-cli/tests/cli_coordinator_worker.rs`.
Use the same coord+worker spawn helpers as the marquee
0.5-S2e test.

| # | Test | Expectation |
|---|---|---|
| 14 | `cli_coordinator_wait_drives_multi_wave_to_completion` (marquee) | Spawn coord + 1 worker. Run `boruna workflow run --submit-only` against a 3-wave workflow (3 source steps → 1 fan-in step → 1 final step). Run `boruna coordinator wait <run-id>` in a child process. Assert: process exits 0 within 60s; runs.db shows all 5 steps Completed; wait stdout contains progress lines for each transition. |
| 15 | `cli_coordinator_wait_resumes_after_kill` | Same setup. Spawn `coordinator wait`. After the run reaches "wave 1 in flight" status, SIGKILL the wait process. Assert: 1 worker is still running; some wave-1 steps may already be Completed. Re-invoke `coordinator wait <run-id>` in a fresh child. Assert: run reaches Completed and exit 0. |
| 16 | `cli_coordinator_wait_exits_nonzero_on_failed_run` | Workflow with a step that fails (uses a deliberately broken `.ax` source). Wait should exit non-zero (1) when the run reaches Failed status, with stderr describing which step failed. |
| 17 | `cli_coordinator_wait_rejects_run_pre_05s2f` | Manually craft a runs.db row with `workflow_def: None` in metadata (older format). `coordinator wait <id>` exits 2 with a clear error: "run pre-dates 0.5-S2f, workflow_def not embedded; use in-process workflow run instead". |
| 18 | `cli_coordinator_wait_max_wait_secs_timeout` | A workflow where the worker is intentionally slow (sleep 30s); `--max-wait-secs 5`. Wait exits non-zero with a timeout error. |

### Adversarial edge cases (covered by tests above)

- Concurrent coord-claim vs. wait-checkpoint write → test #8.
- Lease expiry during wait poll → covered implicitly by #14
  (worker is healthy; sweep doesn't fire) and re-tested
  manually if needed.
- Pathological workflow_def size → test #12.
- Approval / external trigger as a non-first-wave step →
  surfaces error in `compute_ready_steps` (or in
  `advance_run_one_tick` when reading the StepDef kind);
  covered by a manual smoke test rather than a CI test
  (since the approval-gate path is itself experimental).

## Non-tested / acknowledged gaps

- **No long-soak test.** A 24-hour run with multiple waves
  spread across hours is not in CI. Acceptable; the marquee
  test runs ~10 seconds and covers the state-machine.
- **No multi-host test.** The wait client and coordinator
  share a filesystem in v0.5.x by design; multi-host wait
  is 0.5-S3+.
- **No fuzzing of WorkflowDef shape.** The advance-loop
  trusts the WorkflowDef parsed from runs.db; if metadata
  was hand-edited to corruption, advance-loop returns a
  parse error. We don't test corrupted metadata.

## Running the tests

```sh
# Unit
cargo test -p boruna-orchestrator workflow::runner::tests

# Specific advance-loop unit tests
cargo test -p boruna-orchestrator compute_ready_steps
cargo test -p boruna-orchestrator advance_run_one_tick

# CLI integration (require serve feature)
cargo test -p boruna-cli --features serve --test cli_coordinator_worker

# Marquee only
cargo test -p boruna-cli --features serve cli_coordinator_wait_drives_multi_wave_to_completion -- --nocapture
```

## Decision gate to ship

All boxes in "Acceptance criteria" checked + adversarial
review (`ce-correctness-reviewer`) returns no unaddressed
HIGH findings. If any HIGH finding is documented as
deferred-with-rationale, surface it in CHANGELOG `### Known
issues`.
