# Architecture — Coordinator/worker HTTP MVP (sprint 0.5-S2b)

Companion to `docs/design-coordinator-worker-http.md`. Covers
*how* — module layout, JSON shapes, dispatch logic, error
mapping.

## Module placement

Two new modules in `crates/llmvm-cli/src/`:

- `coordinator.rs` — Axum router, in-memory worker registry,
  HTTP handlers. Behind `serve` feature flag.
- `worker.rs` — HTTP client loop, registration, heartbeat task,
  step execution. Behind `serve` feature flag.

Both reuse the `serve` feature dependencies (`axum 0.8`, `tokio`)
that were wired in 0.4-S16's dashboard sprint. No new
workspace deps.

A new dev-dep on `reqwest` (or similar) is required for the
worker's HTTP client. Use the workspace's existing minimal-deps
posture: `ureq` is already a `boruna-vm` optional dep for
`net.fetch`. We could reuse it, but `ureq` is blocking and
doesn't fit `tokio::main`. **Decision:** add `reqwest` as a CLI
dep (gated by `serve` feature), version-pinned with
`default-features = false` and only the `json` + `rustls-tls`
features. Avoids `openssl` system dep.

## CLI surface

Two new top-level subcommands:

```rust
#[cfg(feature = "serve")]
#[command(subcommand)]
Coordinator(CoordinatorCommand),

#[cfg(feature = "serve")]
#[command(subcommand)]
Worker(WorkerCommand),
```

```rust
enum CoordinatorCommand {
    Serve {
        data_dir: Option<PathBuf>,
        #[arg(long, default_value = "8090")]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Cap on lease TTL workers can request. Default 5 min.
        #[arg(long, default_value = "300000")]
        max_lease_ttl_ms: u64,
        /// Long-poll timeout (server-side cap on claim wait).
        #[arg(long, default_value = "30000")]
        poll_timeout_ms: u64,
    },
}

enum WorkerCommand {
    Run {
        #[arg(long)]
        coordinator: String,  // base URL e.g. http://127.0.0.1:8090
        #[arg(long)]
        worker_id: Option<String>,  // auto-allocated if missing
        #[arg(long, default_value = "300000")]
        lease_ttl_ms: u64,
        #[arg(long, default_value = "30000")]
        poll_timeout_ms: u64,
    },
}
```

## Wire format

All routes accept and return JSON. Every response (success and
failure) carries `protocol_version: 1`. Failure responses also
carry `error_kind: "<stable_string>"`.

### `POST /api/workers/register`

Request:
```json
{
  "worker_id": "host-12",         // optional; server allocates if absent
  "capability_set_hash": "sha256:abc..."
}
```

Response (200):
```json
{
  "protocol_version": 1,
  "worker_id": "host-12",
  "session_token": "wkr-abc..."   // opaque; required on subsequent calls
}
```

Response (409 — binary mismatch):
```json
{
  "protocol_version": 1,
  "error_kind": "coord.binary_mismatch",
  "message": "worker hash sha256:abc... does not match coordinator's sha256:def...",
  "expected_hash": "sha256:def..."
}
```

### `POST /api/workers/heartbeat`

Request:
```json
{ "worker_id": "host-12", "session_token": "wkr-abc..." }
```

Response (200): `{ "protocol_version": 1, "ok": true }`

Response (404 — unknown worker; e.g. coordinator restarted):
```json
{
  "protocol_version": 1,
  "error_kind": "coord.unknown_worker",
  "message": "worker host-12 not registered; re-register"
}
```

### `GET /api/work/claim?worker_id=<id>&session_token=<tok>&lease_ttl_ms=<ms>`

Long-polls up to `poll_timeout_ms` (server-side cap on
`max_lease_ttl_ms`). Returns:

- **200** with a work item if a claim succeeded.
- **204 No Content** if no work was claimable within the timeout
  (worker should retry).
- **404** if `worker_id` is unknown (worker re-registers).

Work item shape (200):
```json
{
  "protocol_version": 1,
  "run_id": "run-abc",
  "step_id": "extract",
  "claim_id": 7,
  "lease_expires_at_ms": 1_700_000_300_000,
  "source": "fn main() -> Int { 42 }",
  "policy_json": "{ \"default_allow\": true }",
  "inputs_json": null
}
```

The coordinator dispatch logic (this sprint, simplified):
1. Pick the next claimable Pending step from `runs.db` (any run).
2. Call `claim_step(run_id, step_id, worker_id,
   lease_expires_at_ms, now_ms)`.
3. Read the step's `.ax` source from the on-disk workflow
   directory (path stored in the run's `metadata_json`).
4. Return the work item.

If `claim_step` returns `NotClaimable` (race against another
worker), pick another step. If no Pending steps, long-poll
loop until timeout.

### `POST /api/work/complete`

Request:
```json
{
  "worker_id": "host-12",
  "session_token": "wkr-abc...",
  "run_id": "run-abc",
  "step_id": "extract",
  "claim_id": 7,
  "output_json": "42",
  "output_hash": "sha256:...",
  "attempt_count": 1
}
```

Response (200): `{ "protocol_version": 1, "ok": true }`

Response (409 — lease expired):
```json
{
  "protocol_version": 1,
  "error_kind": "coord.lease_expired",
  "message": "lease for step extract was claim_id=7; current is claim_id=8",
  "current_claim_id": 8,
  "current_status": "running"
}
```

Response (413 — output too large):
```json
{
  "protocol_version": 1,
  "error_kind": "coord.output_too_large",
  "message": "output_json size N bytes exceeds 8 MiB cap",
  "max_bytes": 8_388_608
}
```

The 8 MB cap is checked before parsing the JSON body (Axum body
limit). Workers receiving 413 should not retry with the same
body.

### `POST /api/work/fail`

Same shape as complete, but with `error_msg` instead of
`output_json` / `output_hash`. Same response shapes.

### `POST /api/work/extend-lease`

Request:
```json
{
  "worker_id": "host-12",
  "session_token": "wkr-abc...",
  "run_id": "run-abc",
  "step_id": "extract",
  "claim_id": 7,
  "extend_by_ms": 60_000
}
```

Response (200):
```json
{
  "protocol_version": 1,
  "new_lease_expires_at_ms": 1_700_000_400_000
}
```

Response (409): `coord.lease_expired` (same shape as complete).

The coordinator caps `extend_by_ms` at its `max_lease_ttl_ms`
config — workers asking for longer get the cap.

## Coordinator state

```rust
struct CoordinatorState {
    store: Arc<Mutex<RunCheckpointStore>>,
    workers: Arc<Mutex<HashMap<String, WorkerSession>>>,
    workflow_dirs: Arc<Mutex<HashMap<String, PathBuf>>>,  // run_id -> workflow_dir
    capability_set_hash: String,
    config: CoordinatorConfig,
}

struct WorkerSession {
    session_token: String,
    last_heartbeat_ms: i64,
    capability_set_hash: String,
}

struct CoordinatorConfig {
    max_lease_ttl_ms: u64,
    poll_timeout_ms: u64,
    bind_warning: Option<String>,  // for the dashboard banner integration
}
```

The `workflow_dirs` map is populated on startup by scanning
`runs.db` for non-terminal runs and reading their
`metadata_json.workflow_dir` field (existing convention from
0.3-S2). When 0.5-S2c lands `boruna workflow run --coordinator`,
runs get added on submit.

In-memory worker registry is **deliberately not persisted** —
per ADR 002 ("coordinator restart = all leases void"). On
coordinator restart, all workers must re-register, and on
startup the coordinator runs `expire_leases_and_requeue(now_ms)`
plus a sweep of `Running`-status rows back to `Pending` (since
their leases are dead with no active worker holding them).

## Worker state

```rust
struct WorkerState {
    coord_url: String,
    worker_id: String,
    session_token: String,
    client: reqwest::Client,
    config: WorkerConfig,
}

struct WorkerConfig {
    lease_ttl_ms: u64,
    poll_timeout_ms: u64,
}
```

Worker loop:

```rust
async fn run_worker_loop(state: WorkerState) -> Result<(), WorkerError> {
    // Spawn heartbeat task in background
    let hb_task = tokio::spawn(heartbeat_loop(state.clone()));
    let result = main_loop(state).await;
    hb_task.abort();
    result
}

async fn main_loop(state: WorkerState) -> Result<(), WorkerError> {
    loop {
        match claim_work(&state).await? {
            None => continue,  // 204; long-poll exhausted, retry
            Some(work) => {
                let result = execute_step(&work).await;
                match result {
                    Ok(output) => report_complete(&state, &work, output).await?,
                    Err(e) => report_fail(&state, &work, e).await?,
                }
            }
        }
        if shutdown_requested() { break; }
    }
    Ok(())
}
```

Step execution reuses the existing `Vm` + `CapabilityGateway`
plumbing from `boruna-vm`. Compile + run, capture the resulting
`Value`, serialize as JSON, hash via SHA-256.

## Output size enforcement

```rust
let app = Router::new()
    .route("/api/work/complete", post(handle_complete))
    .layer(DefaultBodyLimit::max(8 * 1024 * 1024))  // 8 MiB
    .with_state(state);
```

Bodies exceeding the limit return Axum's default `413 Payload
Too Large` with no body. We add a custom error handler to
inject the `coord.output_too_large` JSON shape.

## Error mapping

Outcome → HTTP response:

| `RunCheckpointStore` outcome | HTTP status | `error_kind` |
|---|---|---|
| `ClaimOutcome::Claimed` | 200 | (n/a) |
| `ClaimOutcome::NotClaimable` | (coordinator picks next; doesn't surface) |
| `ClaimOutcome::StepNotFound` | (coordinator picks next; doesn't surface) |
| `TerminalOutcome::Committed` | 200 | (n/a) |
| `TerminalOutcome::LeaseExpired` | 409 | `coord.lease_expired` |
| `TerminalOutcome::StepNotFound` | 404 | `coord.step_not_found` |
| `ExtendOutcome::Extended` | 200 | (n/a) |
| `ExtendOutcome::LeaseExpired` | 409 | `coord.lease_expired` |
| `ExtendOutcome::StepNotFound` | 404 | `coord.step_not_found` |

## File diff summary (estimated)

| File | Change |
|---|---|
| `crates/llmvm-cli/src/coordinator.rs` | NEW (~600 lines) |
| `crates/llmvm-cli/src/worker.rs` | NEW (~400 lines) |
| `crates/llmvm-cli/src/main.rs` | new subcommands + dispatch |
| `crates/llmvm-cli/Cargo.toml` | add `reqwest` to `serve` feature |
| `crates/llmvm-cli/tests/cli_coordinator_worker.rs` | NEW (~300 lines) |
| `docs/reference/coordinator-worker.md` | NEW |
| `CHANGELOG.md` | `[Unreleased]` entry |
