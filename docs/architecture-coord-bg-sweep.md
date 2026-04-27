# Architecture + test plan — Coordinator background sweep (sprint 0.5-S2c)

Companion to `docs/design-coord-bg-sweep.md`. Combined doc
for a small sprint.

## Code shape

```rust
// In coordinator::run_serve, after building `state`:

let sweep_state = state.clone();
let sweep_interval = Duration::from_millis(sweep_interval_ms);
let sweep_task = tokio::spawn(async move {
    let mut tick = tokio::time::interval(sweep_interval);
    // First tick fires immediately; skip it (the startup
    // sweep already ran).
    tick.tick().await;
    loop {
        tick.tick().await;
        let now_ms = now_unix_ms();
        let result = {
            let store = match sweep_state.store.lock() {
                Ok(g) => g,
                Err(_) => continue,
            };
            store.expire_leases_and_requeue(now_ms)
        };
        match result {
            Ok(0) => {}  // no-op tick; quiet.
            Ok(n) => eprintln!("coordinator sweep: requeued {n} expired-lease step(s)"),
            Err(e) => eprintln!("coordinator sweep: error {e} — retrying next tick"),
        }
    }
});

let app = build_router(state);
let addr = std::net::SocketAddr::new(bind, port);
let listener = tokio::net::TcpListener::bind(addr).await?;
let result = axum::serve(listener, app).await;
sweep_task.abort();
result?;
```

The sweep task is spawned BEFORE `axum::serve` so it lives for
the lifetime of the server. On axum's exit (SIGINT cancels the
future), `sweep_task.abort()` cleans up the interval task.

## Failure semantics

- **Lock poisoned**: log nothing (avoid log spam), `continue` to
  next tick. The next tick re-acquires.
- **`expire_leases_and_requeue` returns Err**: log via
  `eprintln!`, continue to next tick. Common cause: SQLite
  busy timeout under heavy concurrent writers, which the
  `with_busy_retry` wrapper already handles internally — so
  reaching this branch means a real disk/IO failure.
- **Tokio task panic**: `tokio::spawn`'d tasks have their own
  panic handler; a panic logs to stderr and the task ends.
  The HTTP server keeps running but loses background sweep.
  Operators should monitor stderr for unrecovered panics. (We
  don't auto-respawn; that's a 0.6.x reliability concern.)

## CLI flag

```rust
// In CoordinatorCommand::Serve:
/// Background lease-expiry sweep interval in milliseconds.
/// Default 30000 (30 s). Lower = faster recovery from worker
/// crashes; higher = less DB churn under steady-state.
#[arg(long, default_value = "30000")]
sweep_interval_ms: u64,
```

## Tests

### Coordinator unit test (handler-level)

No new handler-level test needed; the sweep is a tokio task,
not an HTTP handler. Tested at the integration level.

### CLI integration tests

| # | Test | Approach |
|---|---|---|
| 1 | `coord_bg_sweep_requeues_expired_lease` | Spawn coord with `--sweep-interval-ms=200`. Insert run + Pending step. Direct-call `claim_step` with `lease_expires_at=1` (in the past). Wait 500ms. Verify row is back to `Pending`. |
| 2 | `worker_completes_two_step_linear_dag` | Spawn coord + 1 worker. Pre-populate run with step1 + step2 both `Pending`. Worker claims and completes them in some order. Assert both `Completed` with correct `output_json`. |

Test 1 proves the sweep fires periodically. Test 2 proves the
protocol scales beyond a single step. Together they cover the
sprint's scope.

## Concurrency: sweep vs. claim

The sweep wraps in `BEGIN IMMEDIATE` (via
`expire_leases_and_requeue`'s wrapper) just like
`claim_step`. SQLite serializes writers, so:

- Sweep starts, acquires writer lock.
- Concurrent `claim_step` waits (or hits busy_timeout retry).
- Sweep finishes within microseconds (single bulk UPDATE).
- `claim_step` proceeds.

No deadlock risk. The two operations CAN'T overlap in time on
the same connection because they both go through the same
`Arc<Mutex<RunCheckpointStore>>` — the std::sync::Mutex
serializes them at the application layer too.

## Adversarial review focus areas

When `ce-correctness-reviewer` runs:

1. **Tokio task lifetime**: does `sweep_task.abort()` actually
   stop the task on coordinator exit? What if the task is
   inside `expire_leases_and_requeue` when abort fires?
2. **Log spam**: 30s sweep × 0 expired leases = quiet; verify
   we don't log the empty case.
3. **`now_ms` skew**: the sweep uses wall-clock now; if the
   system clock jumps backward, leases that should have
   expired won't. Documented as a known limitation.
4. **Sweep interval lower bound**: `--sweep-interval-ms=0`
   would busy-loop. Add a minimum (e.g., 100 ms)? Or accept
   operator responsibility?
5. **Multi-step DAG test determinism**: order of step claims
   isn't deterministic; the test must accept any order as long
   as both reach Completed.
