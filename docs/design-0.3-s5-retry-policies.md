# Design — 0.3-S5: Retry Policies

**Status:** 2026-04-25
**Predecessor:** `0.3-S4` shipped concurrent execution. Steps can fail; today they fail once and the run halts. This sprint makes failure recoverable by honoring the `RetryPolicy` already on the step definition.

## Scope

**In scope:**

1. **Honor `RetryPolicy.max_attempts`**: actually loop up to `max_attempts` total attempts (was: "retry once" regardless of `max_attempts`). When `on_transient=false`: no retries.
2. **Exponential backoff**: 100ms → 200ms → 400ms → 800ms → ... capped at 5s.
3. **Single shared helper** `retry_with_backoff` used by both sequential and concurrent paths.
4. **`error_msg` carries attempt count**: "Failed after N attempts: <reason>" so `workflow show` shows it.
5. The retry happens INSIDE the step execution (within a chunk worker, or within `execute_source_step`). The persistent checkpoint is written ONCE at the final state — Completed (if any attempt succeeded) or Failed (if all attempts exhausted).

**Out of scope:**

- Per-error-class classification ("retry on net.fetch but not on capability.denied"). The current `on_transient` flag is a simple boolean — refining it requires a typed error taxonomy across the runner + VM, which is its own ADR.
- Per-step retry-count column in the schema. Operationally tracked in `error_msg` for now; future sprint can promote it to a column.
- Resume + retry interaction. A crash mid-retry leaves the step at status=Running; resume re-executes from scratch with fresh attempts. Acceptable; documented.

## Forcing questions (Think)

**Who needs this?** Operators running LLM workflows where transient failures (rate limits, network blips) are normal. Today: one transient blip kills the run. With proper retry: the run survives.

**Narrowest MVP?** A 3-step workflow with `RetryPolicy { max_attempts: 3, on_transient: true }` on the network-fetching step survives 2 transient errors. The third success makes the run Completed.

**Whoa moment?** The exact same transient failure that previously killed a run now logs "step 'fetch_doc' attempt 1 failed (transient): connection reset; retrying in 100ms" and the run completes.

**Compounds?** Reliability primitive that downstream sprints (async, distributed) can build on. Retry semantics are usually re-derived multiple times in a workflow engine's life; getting them right once and sharing the helper avoids drift.

## Key invariants

1. **Determinism**: a successful retry produces the same `output_hash` as a successful first-attempt. The number of attempts varies; the result doesn't.
2. **Backoff is wall-clock keyed (operational only)**. `total_duration_ms` and `started_at_ms`/`ended_at_ms` legitimately differ for retry vs. first-attempt success. Never feeds an audit hash.
3. **Retry budget is bounded.** No infinite-loop-on-permanent-error. After `max_attempts` failures, the step is Failed.
4. **`on_transient = false`** means **no retries** (matches today's "should_retry" gate).
5. **`max_attempts <= 1`** means no retries (one attempt total).

## Architecture

### `retry_with_backoff` helper

```rust
/// Run `attempt_fn` up to `policy.max_attempts` times with
/// exponential backoff between attempts. Returns the last result —
/// `Ok` from the first successful attempt, OR `Err` from the final
/// attempt if all exhausted. Backoff schedule: 100ms × 2^N capped
/// at 5s. The schedule is wall-clock; operational only per
/// project-conventions §17.
///
/// `policy = None` OR `policy.on_transient = false` OR
/// `policy.max_attempts <= 1` → single attempt (no retry).
fn retry_with_backoff<T, F>(
    policy: Option<&RetryPolicy>,
    step_id: &str,
    mut attempt_fn: F,
) -> Result<T, WorkflowRunError>
where
    F: FnMut(u32) -> Result<T, WorkflowRunError>,
```

Called from:
- `WorkflowRunner::execute_source_step` (sequential)
- The worker closure inside `execute_steps_concurrent` (parallel)

### Backoff schedule

```rust
const BASE_BACKOFF_MS: u64 = 100;
const MAX_BACKOFF_MS: u64 = 5000;
fn backoff_ms(attempt: u32) -> u64 {
    BASE_BACKOFF_MS
        .saturating_mul(2u64.saturating_pow(attempt))
        .min(MAX_BACKOFF_MS)
}
// attempt=0 → 100ms (sleep BEFORE attempt 1, not 0)
// attempt=1 → 200ms
// attempt=2 → 400ms
// attempt=3 → 800ms
// attempt=4 → 1600ms
// attempt=5 → 3200ms
// attempt=6 → 5000ms (capped)
```

The helper sleeps `backoff_ms(prev_attempt)` BEFORE each retry (not before attempt 1).

### `error_msg` shape on terminal failure

```text
"failed after 3 attempts: <last attempt's error>"
```

Single attempt failures keep their existing format (no attempt prefix) so we don't churn existing test fixtures and operator scripts.

## Risks

- **Test slowness.** A 3-attempt retry test with backoff = 100ms+200ms = 300ms+ between attempts. Mitigation: an `#[cfg(test)]` knob to override the schedule with `0ms` for unit tests. (Or: tests use `max_attempts: 1` to skip backoff entirely; the helper itself is tested directly with a counting closure.)
- **Retry inside concurrent worker** lengthens that worker's wall-clock time, blocking the wave from progressing. Acceptable — if a step is genuinely retryable, we want it retried. Operators control this with `max_attempts` per step.
- **Hidden interaction with `--ephemeral`**. Sequential path goes through `execute_source_step`; we add retry there. Both ephemeral and persistent paths get the new behavior. That's correct.

## Acceptance criteria

- `cargo test --workspace` green including:
  - `retry_with_backoff_succeeds_on_first_attempt` (no retry, no backoff)
  - `retry_with_backoff_succeeds_after_failures` (closure fails 2× then succeeds; helper returns Ok)
  - `retry_with_backoff_exhausts_attempts` (closure always fails; helper returns Err with attempt count in message)
  - `retry_disabled_when_on_transient_false` (no retry even with max_attempts > 1)
  - `retry_disabled_when_max_attempts_le_1` (no retry)
  - `retry_disabled_when_no_policy` (no retry)
  - `compile_error_step_with_retry_eventually_fails` (existing-test-style, but via the new helper)
- `cargo clippy -D warnings` clean.
- `cargo fmt --check` clean.
