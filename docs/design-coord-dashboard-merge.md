# Design — Coordinator + dashboard listener-merge (sprint 0.5-S2d)

Combined design + architecture doc for a small focused sprint.

## Premise

Sprint `0.4-S16` shipped the dashboard. Sprint `0.5-S2b`
shipped the coordinator. Both bind their own listener. Today
operators running both must:

1. Run `boruna dashboard serve --port 8080 --data-dir ...` AND
2. Run `boruna coordinator serve --port 8090 --data-dir ...`

Two processes, two ports, two firewall rules, two sets of
`--data-dir` to keep in sync. Each opens its own
`RunCheckpointStore` connection to the same database.

ADR 002 explicitly anticipated this merge:

> The coordinator's worker routes will be `/api/workers/...`,
> `/api/work/...` — no overlap [with dashboard's read routes].
> The coordinator and dashboard share a listener.

This sprint delivers that merge: one binary, one process,
one port, one connection to runs.db. Operators get fleet
visibility AND distributed dispatch from a single
`boruna coordinator serve` invocation.

## Who needs this

- **Operators** running coordinator + workers in production.
  One service, one port to expose, one banner-warning surface.
- **The future 0.5-S2e implementer** (workflow runner
  integration). When the runner-side wave loop tails run
  status, it can hit the dashboard's read routes against the
  same coordinator listener.

## Narrowest MVP

When `boruna coordinator serve` is running:

- All coordinator routes (`/api/workers/...`, `/api/work/...`)
  available on the configured port.
- All dashboard read routes (`/`, `/runs/:id`, `/api/runs`,
  `/api/runs/:id`) available on the same port.
- Bind warning, if any, propagates to BOTH the dashboard's
  HTML banner AND the existing coordinator stderr warning.
- The standalone `boruna dashboard serve` subcommand stays as
  a backwards-compat alias for read-only-only deployments.
- No new flags. The merge is automatic — there's no way to
  run the coordinator WITHOUT the dashboard routes
  (intentionally; operators who want stricter isolation can
  run dashboard separately).

## What would make someone say "whoa"

- **Banner consistency.** The dashboard's existing red banner
  for non-loopback bind correctly fires when the COORDINATOR
  is bound to non-loopback. Operators can't accidentally
  expose the coordinator without the dashboard banner
  warning them.
- **Single SQLite connection.** Today two processes hold two
  connections to runs.db. Merging means one connection, less
  contention, simpler operator mental model.
- **No duplication.** The dashboard's route handlers move to
  a new `dashboard::routes(...)` builder that takes the
  shared store handle. Coordinator + standalone dashboard
  both use the same builder. Zero copy-paste.

## How this compounds

- Once merged, future routes are additive on the same listener.
  No question of "does this go on the coordinator or the
  dashboard?" — the answer is always "the same listener."
- The 0.5-S2e workflow-runner-integration sprint can use the
  dashboard routes to tail run status without standing up a
  separate process.
- Auth (when it lands) protects both surfaces with one
  middleware layer.

## Scope

- Refactor `dashboard.rs`:
  - `pub fn dashboard_routes(store: Arc<Mutex<RunCheckpointStore>>,
    bind_warning: Option<String>) -> Router` — exposes the route
    builder for external consumers.
  - The standalone `dashboard::run_serve` continues to call
    `dashboard_routes` internally; behavior unchanged.
- Refactor `coordinator.rs`:
  - `build_router` calls `dashboard::dashboard_routes(...)` and
    `.merge(...)` it onto the coordinator's router.
  - Coordinator's `bind_warning` is forwarded to the dashboard
    builder so the banner appears on coordinator-served HTML
    pages.
- Test changes:
  - New CLI integration test
    `coord_serve_includes_dashboard_routes` that hits `/`,
    `/api/runs`, `/runs/:id` against a coordinator and asserts
    they return the same shapes as the standalone dashboard.
- CHANGELOG `[Unreleased]` entry.

## Non-goals (deferred)

- **No new auth.** Both surfaces stay no-auth.
- **No standalone-dashboard deprecation.** `boruna dashboard
  serve` keeps working — useful for read-only deployments
  without the coordinator overhead.
- **No new dashboard features.** Pagination, CSP headers,
  log streaming etc. all stay deferred.
- **No new coordinator features.** Workflow runner
  integration stays 0.5-S2e.

## Test plan

| # | Test | Expectation |
|---|---|---|
| 1 | `coord_serve_responds_to_dashboard_index` | `GET /` against the coordinator returns `<html>` with "Boruna runs" |
| 2 | `coord_serve_responds_to_dashboard_api_runs` | `GET /api/runs` returns slim `RunSummary` list |
| 3 | `coord_serve_dashboard_route_404_for_unknown_run` | `GET /runs/no-such-id` returns 404 |
| 4 | `coord_bind_warning_appears_in_dashboard_banner` | Coordinator with `--bind 0.0.0.0` → `GET /` HTML contains the WARNING banner |
| 5 | (existing) `cli_dashboard_serve_responds_to_index` continues to pass — proves the standalone path still works |

Plus existing dashboard unit tests must keep passing. The
`build_router` rename (now `dashboard_routes`) requires
updating one test entry point.

## Adversarial review focus areas

When `ce-correctness-reviewer` runs:

1. **Route conflict** — confirm `/api/runs` (dashboard) and
   `/api/work/...` (coordinator) don't overlap. The
   `/api/runs/:id` and `/api/work/claim` paths are
   structurally distinct.
2. **Store handle sharing** — both routers use the same
   `Arc<Mutex<RunCheckpointStore>>`. Verify the dashboard's
   read paths use a brief lock (no `.await` while held), and
   that they compose cleanly with the coordinator's
   write-heavy paths under SQLite WAL.
3. **Bind warning propagation** — coordinator's
   `bind_warning` flows into the dashboard's
   `bind_warning`; the banner appears on coordinator-served
   HTML pages.
4. **State type coupling** — `dashboard_routes` takes
   primitive args (Arc, Option<String>), not the full
   `DashboardState`. This decouples the dashboard's internal
   state shape from the coordinator's.
5. **Test surface** — the standalone-dashboard tests must
   keep passing without changes. The route builder
   refactor should be transparent.

## Stable contract

- The merge is automatic and unavoidable: anyone running
  `boruna coordinator serve` gets the dashboard routes too.
- The standalone `boruna dashboard serve` keeps working with
  no behavior change.
- Route paths are stable.
- HTTP semantics are stable (200/404/etc).

## Stability tier

Per `docs/stability.md`: **experimental** (matches both
parents).
