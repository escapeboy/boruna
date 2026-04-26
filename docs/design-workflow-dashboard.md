# Design — Workflow dashboard, read-only MVP (sprint 0.4-S16)

## Premise

`runs.db` is the system of record for every workflow run, every step
checkpoint, and (since 0.4-S11) every lifecycle audit event. Today
operators inspect it via:

- `boruna workflow show <run-id>` — single-run JSON dump
- `boruna evidence inspect <bundle>` — single-bundle inspection
- `boruna metrics export` — Prometheus aggregates
- `sqlite3 runs.db` — when the above don't fit

What's missing is the **fleet view** — "what's currently running, what
just paused, which step in run X is failing." Today that requires
either custom SQL or a Prom dashboard backed by metrics, neither of
which gives operators the run/step detail they need to triage.

The 0.4.0 cycle's premise is operations. A read-only HTTP dashboard
is the natural next step: it doesn't change behavior, doesn't add
runtime risk, and unlocks fleet visibility as a single subcommand.

## Who needs this

Operators / on-call staff with at least file-system access to a
running Boruna deployment.

Today they:
- SSH to the host, run `sqlite3 runs.db` against a deployed runs.db
  to figure out "is run X paused or failed?"
- Cross-reference run IDs from logs against `boruna workflow show`
- Read evidence bundles by hand when a run is in a strange state

After this sprint they should be able to:
- `boruna dashboard serve --data-dir /var/lib/boruna` on the host
- Open `http://127.0.0.1:8080/` and see all runs grouped by status
- Click a run and see its step list, statuses, attempt counts,
  errors

## Narrowest MVP

**One subcommand, four routes, two views.**

```
boruna dashboard serve --data-dir <path> [--port 8080] [--bind 127.0.0.1]
```

Routes:

| Method + path | Returns |
|---|---|
| `GET /` | HTML index — table of all runs, grouped by status |
| `GET /runs/:id` | HTML detail — run header + step list |
| `GET /api/runs` | JSON `{ runs: [RunRow, ...] }` |
| `GET /api/runs/:id` | JSON `{ run: RunRow, steps: [StepCheckpoint, ...] }` |

Behavior:
- Read-only. Zero `POST`, `PUT`, `DELETE`, `PATCH` routes. The only
  state mutation visible from the dashboard is the SQLite `WAL` /
  `shm` files growing as the orchestrator writes. The dashboard
  itself never mutates.
- **Loopback by default.** Binds `127.0.0.1`. Operators who want LAN
  access pass `--bind 0.0.0.0` explicitly and accept the
  consequences (no auth in this sprint — see non-goals).
- Behind the existing `serve` feature flag (already wired for the
  framework-app `serve` command). `boruna` builds without `--features
  serve` continue to work; the `dashboard` subcommand is gated.
- Multi-env aware: when `--env` is set on the parent CLI, the
  dashboard reads from `<data-dir>/<env>/runs.db` per the 0.4-S14
  contract.

That's the MVP. Anything else ships later.

## What would make someone say "whoa"

- **Same data, two formats.** `GET /api/runs` returns the same
  shape as the HTML view's table — the HTML is just a server-side
  render of the JSON. Operators can `curl http://127.0.0.1:8080/api/runs
  | jq` and get exactly what the page shows. No JS framework, no
  client-side state, no API drift.
- **Loud security stance.** The HTML index includes a banner
  whenever the bind address is not `127.0.0.1`, naming the bind
  address explicitly: "Listening on 0.0.0.0:8080 — anyone with
  network access to this port can read your run data."
- **Evidence-bundle determinism preserved.** The dashboard never
  hashes anything, never participates in audit chains, never serves
  evidence-bundle assets. It's purely a read view. Convention #15
  ("replay-verified vs. operational") is honored: every column the
  dashboard displays is operational state.

## How this compounds

- Once the dashboard exists, `boruna workflow show` becomes the
  CLI fallback (good for SSH-only environments) and the dashboard
  becomes the default for triage. Both share the same SQLite
  schema and read APIs.
- The route table is forward-compatible with later additions:
  `GET /runs/:id/steps/:step_id`, `GET /api/runs?status=paused`,
  `GET /api/triggers/pending`. Adding routes is additive; the
  read-only contract stays intact.
- Adding auth later (for non-loopback usage) is a contained
  change — middleware over the existing routes, no rework needed.
- The HTML output is intentionally simple (server-rendered tables,
  inline CSS, no JS dependency). When a future sprint adds an SPA
  the templates stay as a no-JS fallback.

## Scope (what this sprint changes)

- New: `boruna dashboard serve` subcommand behind `serve` feature.
- New: `crates/llmvm-cli/src/dashboard.rs` module — Axum router,
  handler functions, HTML templates (inline `format!` strings or
  small `&'static str` shells; no template engine).
- New: read-side use of `RunCheckpointStore::list_runs`,
  `get_run_record`, `list_step_checkpoints`,
  `get_run_operational`, `get_run_metadata`. All exist today.
- New: `--bind` flag (defaults to `127.0.0.1`). `--port` flag
  (defaults to `8080`).
- New: `docs/reference/dashboard.md` — operator-facing usage,
  security posture, route reference.
- Wiring: when `--env` is set, the dashboard's data-dir resolution
  uses the same path-namespacing as `boruna workflow run`
  (0.4-S14 contract).

## Non-goals (deferred)

- **No mutations.** No "pause this run", "approve this gate",
  "retry this step" buttons. Every operational mutation today
  goes through `boruna workflow approve|reject|run|resume`. The
  dashboard does not add a second mutation path.
- **No authentication / authorization.** The dashboard ships
  loopback-default and **must not be exposed to the internet
  without a reverse proxy doing auth**. This is documented and
  banner-warned.
- **No TLS.** Loopback HTTP is intentional. TLS termination
  belongs to the operator's reverse proxy if they expose it.
- **No real-time updates.** The page is a snapshot at request
  time. No WebSockets, no SSE, no polling-driven UI. Refresh the
  page.
- **No evidence-bundle inspection in-browser.** A future sprint
  can add a `GET /api/evidence/:bundle_id` route over a built-in
  bundle directory; this one stays scoped to runs.
- **No SPA / client-side state.** Plain HTML; no JS dependency.
- **No metric / chart embedding.** Operators run Prometheus +
  Grafana for that (sprint 0.4-S12). The dashboard is for
  per-run drill-down, not aggregates.
- **No log streaming.** Logs live in the host's stdout/file
  destination, not the dashboard.
- **No multi-tenancy.** One Boruna installation, one runs.db.
  Multi-env (0.4-S14) is the closest thing — operators can run
  the dashboard once per env if they need separate views.

## Stable contract surface

Locked in this sprint:

- The four route paths above (`/`, `/runs/:id`, `/api/runs`,
  `/api/runs/:id`).
- The `--bind`, `--port`, `--data-dir` flag names and semantics.
- The JSON shapes of `/api/runs` and `/api/runs/:id` — they
  serialize the existing `RunRow` and `StepCheckpoint` types
  unmodified, so they automatically inherit those types' stability
  guarantees (per convention #11, `#[serde(default)]` on every new
  metadata field, and convention #15 replay-verified vs. operational
  annotation).

Future routes can be added additively. Removing a documented route
is a breaking change; the dashboard does not version itself
otherwise (no `protocol_version`).

## Open questions for next sprint

- Should `--bind 0.0.0.0` require a confirmation flag like
  `--unsafe-public-bind` to avoid accidental exposure? (This
  sprint just emits a loud warning and binds; defer the harder
  policy to a future sprint when auth lands.)
- Should there be a `--ttl <seconds>` flag that auto-shuts-down
  after a window? Useful for "open dashboard during a triage
  session, walk away, doesn't stay open."
- Should the JSON include a `protocol_version` field (per
  conventions used in MCP)? CLI surfaces don't carry one today;
  defer to consistency-sprint when that question matters.

## Stability tier

Per `docs/stability.md` this lands as **experimental**:
- The route contract is stable but the rendered HTML is not.
- The `--bind`, `--port`, `--data-dir` flags are stable.
- Additive route changes (new routes, new fields in JSON) keep
  the surface stable. Shape changes to `RunRow` /
  `StepCheckpoint` cascade per their existing stability tiers
  (those are stable).
