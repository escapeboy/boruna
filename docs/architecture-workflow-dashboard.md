# Architecture — Workflow dashboard, read-only MVP (sprint 0.4-S16)

Companion to `docs/design-workflow-dashboard.md`. This doc covers
*how*, not *what* — module layout, route handlers, threading
model, error handling.

## Module placement

New module: `crates/llmvm-cli/src/dashboard.rs` (in `boruna-cli`),
gated behind the existing `serve` feature flag (already wired in
`Cargo.toml` for the framework-app `serve` command — same Axum +
Tokio dependencies).

We keep the dashboard in the CLI crate because:
1. It's a CLI subcommand, not a library API.
2. It directly composes `boruna-orchestrator::persistence` types
   (`RunCheckpointStore`, `RunRow`, `StepCheckpoint`, …) which are
   already a CLI dep.
3. Reusing the `serve` feature avoids a second Axum/Tokio
   dependency tree.

The existing `crates/llmvm-cli/src/serve.rs` module (framework-app
serve) is left untouched.

## CLI surface

```
boruna [--env <name>] dashboard serve --data-dir <path> [--port 8080] [--bind 127.0.0.1]
```

In `crates/llmvm-cli/src/main.rs`:

- New top-level `Command::Dashboard { command: DashboardCommand }`.
- `enum DashboardCommand { Serve { data_dir, port, bind } }` —
  one subcommand for now (matches the `MetricsCommand::Export` and
  `CapabilityCommand::List` precedents).
- Dispatch to `dashboard::run_serve(...)` (gated by
  `#[cfg(feature = "serve")]`).
- `--data-dir` defaults via the existing `resolve_data_dir` helper
  — same fallback chain as `boruna workflow run` / `metrics
  export`. Multi-env (`BORUNA_ENV` / `--env`) namespacing is
  inherited automatically.
- `--port` defaults to `8080`. `--bind` defaults to `127.0.0.1`.
- Any non-loopback bind emits a loud `eprintln!` warning naming
  the bind address before the server starts.

## State + threading

```rust
struct DashboardState {
    store: Arc<Mutex<RunCheckpointStore>>,
}
```

- `RunCheckpointStore` holds a `rusqlite::Connection` which is
  `!Sync` and the `Arc<Mutex<>>` is the standard way to share it
  across Axum handler tasks.
- The mutex is acquired briefly per request (one `list_runs` or
  `get_run_record + list_step_checkpoints` call). No long-lived
  locks, no deadlock risk because handlers don't call other
  handlers.
- We use `std::sync::Mutex`, not `tokio::sync::Mutex`. Database
  reads are quick and synchronous; holding a tokio mutex across
  `await` points isn't a benefit here, and `std::sync::Mutex`
  lets us write handlers that look like normal sync code.
- Connection-level reads are protected against writers by the
  existing `BEGIN IMMEDIATE` + busy_timeout combo (project
  conventions #11, #13). The dashboard reads do NOT use
  `BEGIN IMMEDIATE` — they're plain selects against WAL-mode
  SQLite, which permits concurrent reads alongside the writer.

## Routes

```
GET /              → dashboard::handle_index
GET /runs/:id      → dashboard::handle_run_detail
GET /api/runs      → dashboard::handle_api_runs
GET /api/runs/:id  → dashboard::handle_api_run_detail
```

No other routes. No `POST`, `PUT`, `DELETE`, `PATCH` — this is
enforced by the router, not just by handlers (a regression test
asserts non-GET methods on every route return 405).

### Handler shapes

```rust
async fn handle_index(State(s): State<DashboardState>) -> Html<String>
async fn handle_run_detail(State(s): State<DashboardState>, Path(id): Path<String>) -> Result<Html<String>, StatusCode>
async fn handle_api_runs(State(s): State<DashboardState>) -> Json<RunsListResponse>
async fn handle_api_run_detail(State(s): State<DashboardState>, Path(id): Path<String>) -> Result<Json<RunDetailResponse>, StatusCode>
```

Where:

```rust
#[derive(Serialize)]
struct RunsListResponse {
    runs: Vec<RunRow>,
}

#[derive(Serialize)]
struct RunDetailResponse {
    run: RunRecord,
    operational: Option<RunOperational>,
    steps: Vec<StepCheckpoint>,
}
```

`RunRow`, `RunRecord`, `RunOperational`, and `StepCheckpoint` are
already `Serialize` (used in CHANGELOG-locked surfaces); the
dashboard inherits their stability.

`Path<String>` accepts arbitrary string `:id` values; the SQL
query already binds parameters safely (parameterized queries) so
SQL injection is impossible at this layer. Missing IDs → 404.

## HTML rendering

No template engine. Each handler builds a `String` via `format!`
or by composing `&'static str` shells. Rationale:

- Template engines (askama, tera, minijinja) add a build
  dependency for ~3 pages of HTML.
- The pages are simple tables. `format!` is plenty.
- HTML escaping: every value rendered into HTML goes through a
  small `html_escape(s: &str) -> String` helper that escapes
  `<`, `>`, `&`, `"`, `'`. This is convention #15 — operator-
  controlled run_ids and workflow_names are operational state but
  could in principle contain XSS payloads if they came from
  external triggers; we escape always.
- All inline CSS in a single `<style>` block in the page header.
  No external assets, no CSP complications.

Inline CSS keeps the dashboard self-contained — no `assets/`
directory, no version-skew between binary and assets.

## Bind security stance

```rust
let listener = tokio::net::TcpListener::bind((bind_addr, port)).await?;
if !bind_addr.is_loopback() {
    eprintln!(
        "[WARNING] dashboard bound to non-loopback {}:{}, anyone with network access \
         can READ all run data; the dashboard has no auth.",
        bind_addr, port
    );
}
```

The HTML index also includes a banner div when bound non-loopback,
with the same message.

The bind address is parsed via `std::net::IpAddr::from_str` so
operators get a clean error if they pass garbage. `127.0.0.1` and
`::1` both count as loopback (the standard library's
`IpAddr::is_loopback`).

## Error handling

- 404 when the run_id doesn't exist (both HTML detail and JSON
  detail).
- 500 when the persistence layer returns an unexpected error —
  message includes the `PersistenceError` Display output and
  nothing else (no stack trace, no file paths beyond what the
  error itself carries).
- The CLI exits non-zero on bind failure (port in use, etc.).

## Observability

Each request logs at `info` level via `tracing`:

```
{
  event: "dashboard.request",
  method: "GET",
  path: "/api/runs",
  status: 200,
  duration_ms: 3
}
```

Tracing is already a dep on `boruna-vm` (always on); the dashboard
only emits these spans when `boruna --features serve` is built.
This is consistent with sprint 0.4-S5's observability story.

## Multi-env wiring

The CLI's existing `resolve_data_dir(data_dir.as_ref())` helper
already implements the 0.4-S14 namespacing (`<data-dir>/<env>/`
when `BORUNA_ENV` is set). The dashboard subcommand calls it
unchanged; no special multi-env logic in `dashboard.rs`.

## Feature gating

- `serve` feature (already exists) gates the entire `dashboard`
  module and its CLI subcommand.
- Building `--no-default-features` (no `serve`) leaves
  `boruna dashboard` undocumented — the subcommand simply doesn't
  appear in `--help`. This matches how `boruna serve` (framework
  app) behaves today.

## File diff summary (estimated)

| File | Change |
|---|---|
| `crates/llmvm-cli/src/dashboard.rs` | NEW (~400 lines) |
| `crates/llmvm-cli/src/main.rs` | new `Dashboard` subcommand + dispatch (~30 lines) |
| `crates/llmvm-cli/Cargo.toml` | no change (already has `serve` feature) |
| `docs/reference/dashboard.md` | NEW |
| `CHANGELOG.md` | `[Unreleased]` entries |
| `crates/llmvm-cli/tests/cli_dashboard.rs` | NEW (integration tests via spawned binary) |
