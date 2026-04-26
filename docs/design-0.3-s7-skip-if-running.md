# Design — 0.3-S7: `--skip-if-running` (idempotent invocation)

**Status:** 2026-04-26
**Theme:** Roadmap — "Scheduled workflows" (cron triggers).

## Scope

Add a `--skip-if-running` flag on `boruna workflow run` that checks the persistent store for in-flight runs of the same workflow before launching a new one. If a `Running` or `Paused` run exists, exit 0 cleanly with a clear message. This makes cron-driven scheduled execution safe without an in-binary scheduler daemon.

**In scope:**

1. New `--skip-if-running` flag on `boruna workflow run`. Implies `--data-dir` (or its fallback chain — uses persistent store).
2. New library API `WorkflowRunner::find_in_flight_runs(data_dir, workflow_hash) -> Vec<RunRow>` — returns Running/Paused runs for the given workflow.
3. CLI behavior: skip → print `"skipped: <prior_run_id> is <status>"` to stderr + exit 0 (cron-friendly, distinguishable from "ran successfully" via stdout content but exit-code identical).
4. Determinism: skip-decision is operational only.

**Out of scope:**

- A `boruna scheduler` daemon. Operators wire up `cron` or `systemd timers` themselves.
- A cron-expression-in-workflow.json. The schedule lives outside the workflow def — different operators may want different schedules for the same workflow.
- Cancellation of in-flight runs. The flag only DECIDES whether to start a new run; it doesn't kill existing ones.

## Forcing questions (Think)

**Who needs this?** Operators running scheduled batch workflows (LLM grading, document ingest, report generation). Today: a job that runs longer than its cron interval can produce overlapping runs that race on the same outputs/, double-bill LLM API calls, etc.

**Narrowest MVP?** A cron entry like `0 2 * * * boruna workflow run /path/to/wf --policy allow-all --data-dir /var/lib/boruna --skip-if-running` runs at 2am daily. If yesterday's run is still in progress, today's invocation skips cleanly.

**Whoa moment?** The same one-line cron entry that would have caused overlapping runs (and a midnight pager) now safely no-ops with a clear log line.

**Compounds?** Unblocks the "scheduled workflows" roadmap theme without committing to a full daemon design. Future sprint can layer a `boruna scheduler tick` that adds `--skip-if-running` automatically.

## Implementation

### `RunCheckpointStore::list_runs_by_workflow_hash` (new)

The existing `list_runs_by_status` filters by status; we need to filter by workflow_hash AND status-in-(Running, Paused). Add:

```rust
pub fn list_in_flight_runs_for_workflow(
    &self,
    workflow_hash: &str,
) -> Result<Vec<RunRow>, PersistenceError>
```

Queries: `SELECT ... FROM runs WHERE workflow_hash = ?1 AND status IN ('running', 'paused') ORDER BY workflow_name, run_id`.

### `WorkflowRunner::find_in_flight_runs` (new)

Public CLI entry. Opens the store, computes `workflow_hash_from_def`, calls `list_in_flight_runs_for_workflow`.

### CLI behavior

```rust
WorkflowCommand::Run {
    ...
    #[arg(long)]
    skip_if_running: bool,
}
```

In the handler, BEFORE calling `run_persistent`:
```rust
if skip_if_running && !ephemeral {
    let in_flight = WorkflowRunner::find_in_flight_runs(&data_dir, &def)?;
    if let Some(prior) = in_flight.first() {
        eprintln!(
            "skipped: workflow '{}' has {} run '{}' from {}",
            prior.workflow_name, prior.status.as_str(), prior.run_id, ...
        );
        return Ok(());  // exit 0
    }
}
```

`--skip-if-running` is incompatible with `--ephemeral` (no store to check). Reject at parse via clap `conflicts_with`.

### Determinism

The skip decision depends on the store's current state — not deterministic-given-inputs (different runs at different wall-clock times see different stores). But the skip is operational metadata only. The actual workflow execution that happens (when not skipped) is bit-identical to a non-skip-flag invocation.

## Acceptance criteria

- `cargo test --workspace` green including:
  - `find_in_flight_runs_empty_db` (no prior runs)
  - `find_in_flight_runs_with_running` (returns the row)
  - `find_in_flight_runs_skips_terminal` (Completed/Failed don't surface)
  - CLI test or integration test: `--skip-if-running` exits 0 without running when prior is in-flight
- `cargo clippy -D warnings` clean.
- `cargo fmt --check` clean.
- Smoke: kick off a paused-at-approval-gate run, then re-invoke `boruna workflow run --skip-if-running` and verify it skips cleanly.
