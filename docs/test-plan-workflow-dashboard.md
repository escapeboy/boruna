# Test plan — Workflow dashboard, read-only MVP (sprint 0.4-S16)

Companion to `docs/design-workflow-dashboard.md` and
`docs/architecture-workflow-dashboard.md`.

## Strategy

Two layers:

1. **Handler unit tests** (in `crates/llmvm-cli/src/dashboard.rs`
   under `#[cfg(test)]`) — call the handler functions directly
   with an in-memory `RunCheckpointStore`, assert the response
   shape. Cheap, parallel-safe, no port binding.

2. **CLI integration tests** (in
   `crates/llmvm-cli/tests/cli_dashboard.rs`) — spawn the binary
   bound to `127.0.0.1:0` (kernel-assigned ephemeral port), make
   HTTP requests with a small client, assert end-to-end behavior.
   Slower but catches wiring bugs (bind, dispatch, real Axum
   routing).

We rely on `RunCheckpointStore::open_in_memory()` for handler
unit tests so we don't touch the filesystem.

## Handler unit tests (in `dashboard.rs`)

| # | Test | Setup | Expectation |
|---|---|---|---|
| 1 | `handle_index_empty_store_renders_empty_table` | empty store | HTML contains "No runs" or empty `<tbody></tbody>` |
| 2 | `handle_index_renders_runs_grouped_by_status` | 3 runs across 2 statuses | HTML contains all 3 run_ids and both status labels |
| 3 | `handle_index_html_escapes_run_ids` | run with id `"<script>alert(1)</script>"` | rendered HTML has `&lt;script&gt;`, NOT raw `<script>` |
| 4 | `handle_index_warns_when_bound_non_loopback` | state with `bind_warning: Some("0.0.0.0")` | banner div present in HTML |
| 5 | `handle_index_no_warning_when_loopback` | state with `bind_warning: None` | banner div absent |
| 6 | `handle_run_detail_renders_run_and_steps` | run + 2 step checkpoints | HTML contains both step_ids and the run header |
| 7 | `handle_run_detail_404_for_unknown_id` | empty store | returns `StatusCode::NOT_FOUND` |
| 8 | `handle_run_detail_html_escapes_step_error_msg` | step with `error_msg: Some("<x")` | rendered HTML has `&lt;x` |
| 9 | `handle_api_runs_returns_run_list_json` | 2 runs | JSON has `runs[]` of length 2 |
| 10 | `handle_api_runs_empty_store_returns_empty_array` | empty store | JSON has `runs: []` |
| 11 | `handle_api_run_detail_returns_full_record` | run + 1 step | JSON has `run`, `operational`, `steps[]` |
| 12 | `handle_api_run_detail_404_for_unknown_id` | empty store | returns `StatusCode::NOT_FOUND` |

## CLI integration tests (`cli_dashboard.rs`)

These spawn the binary, hit the running server, and assert end-to-
end behavior. Pattern:

```rust
let port = pick_free_port();
let mut child = Command::new(boruna_bin())
    .args(["dashboard", "serve", "--data-dir", data_dir, "--port", &port.to_string()])
    .spawn()?;
wait_for_server(port);
// ... make requests, assert ...
child.kill();
```

| # | Test | Expectation |
|---|---|---|
| 13 | `cli_dashboard_serve_responds_to_index` | `GET /` → 200, body contains `<html` |
| 14 | `cli_dashboard_serve_responds_to_api_runs` | `GET /api/runs` → 200, JSON parseable, has `runs` field |
| 15 | `cli_dashboard_serve_404_for_unknown_run` | `GET /runs/no-such-id` → 404 |
| 16 | `cli_dashboard_serve_post_returns_405` | `POST /` → 405 (no mutation routes exist) |
| 17 | `cli_dashboard_serve_default_bind_is_loopback` | server only reachable via 127.0.0.1, not 0.0.0.0 (best-effort: check bind via netstat or just trust the documented default) |
| 18 | `cli_dashboard_serve_with_data_from_real_run` | populate runs.db via `boruna workflow run --ephemeral=false` (or directly via store), then verify dashboard shows it |

For tests 13-16, we use a minimal HTTP client (either
`std::net::TcpStream` with a hand-rolled HTTP/1.1 GET, or pull in
`ureq` which is already a dev-dep on `boruna-vm`). To avoid
adding a `boruna-cli` dev-dep we use the `std::net::TcpStream`
approach — simple GET requests, parse status line + body.

Test 17 is best-effort because verifying "not bound to 0.0.0.0"
deterministically is platform-specific. We document the default
in the design doc and trust it; the test asserts the help-text /
default value, not the actual socket state.

Test 18 is the most valuable end-to-end check. Approach:
- Spin up a tempdir as data-dir.
- Insert runs+steps directly via `RunCheckpointStore` (faster
  than running an actual workflow).
- Spawn dashboard.
- `GET /api/runs` and assert the inserted run appears.
- Cleanup.

## Adversarial review focus areas

When `ce-correctness-reviewer` and `ce-security-reviewer` run:

1. **Mutation surface** — confirm zero `POST`/`PUT`/`DELETE`/
   `PATCH` routes. Confirm there's no Axum middleware that
   exposes `axum::routing::any` or similar fallback.
2. **Bind default** — is `127.0.0.1` the documented AND the
   actual default in clap?
3. **HTML escaping** — every value rendered into HTML must go
   through the escape helper. Audit the handler code line by line.
   Specifically: `error_msg`, `run_id`, `workflow_name`,
   `policy_json` (if rendered).
4. **Connection mutex contention** — could a slow query block
   other requests indefinitely? (Mitigated by SQLite WAL +
   lightweight queries; but a sanity check.)
5. **Path traversal in `:id`** — Axum's `Path<String>` doesn't
   try to interpret the value as a filesystem path. The SQL is
   parameterized. So no risk, but confirm no callsite passes
   `:id` to `Path::new` or similar.
6. **Information disclosure** — the dashboard intentionally
   exposes `policy_json` and `metadata_json` in the JSON detail
   response. If those contain secrets (rare but possible — an
   integrator might embed an API key in metadata), the dashboard
   leaks them. Document this; defer redaction to a future sprint
   when auth lands.
7. **Sub-domain CORS** — by default Axum's `Json` does not set
   CORS headers. We don't add `tower_http::cors`. Cross-origin
   requests fail in browsers. Document.

## Out of scope for this sprint's tests

- Load testing / benchmarking — defer.
- Browser automation (Playwright, etc.) — defer.
- Visual regression — no design system to lock yet.
- TLS — not implemented.

## Regression tests carried forward

- The `RunRow` / `RunRecord` / `StepCheckpoint` JSON shapes are
  asserted by existing persistence-layer tests; the dashboard's
  JSON shapes inherit those. If those types add new fields (per
  convention #11 — `#[serde(default)]` on every new field), the
  dashboard automatically picks them up and the existing
  persistence regression tests catch any breakage.
- No new persisted columns; no new annotations needed.

## CI matrix

The `serve` feature is opt-in in CI per existing convention. To
keep the dashboard exercised, add `cargo test -p boruna-cli
--features serve` to `.github/workflows/ci.yml`. (Or batch under
the existing per-feature test job pattern from the http-feature
sprint.)
