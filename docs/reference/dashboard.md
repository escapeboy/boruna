# Workflow dashboard reference

Sprint `0.4-S16` introduced a read-only HTTP dashboard over `runs.db`.
Use it for fleet visibility — "what's running, what's paused, which
step in run X is failing" — when SQL or `boruna workflow show`
isn't enough.

## Building

The dashboard is gated behind the `serve` feature (already in use
for the framework-app `boruna serve` command). It is **not**
included in default builds.

```sh
cargo build --release -p boruna-cli --features serve
```

Operator-distributed Boruna binaries: pass `--features serve` (and
`--features persist-sqlite` if not building the default feature
set). The `dashboard serve` subcommand returns an error if the
binary was built without `persist-sqlite`.

## Running

```sh
boruna dashboard serve --data-dir /var/lib/boruna [--port 8080] [--bind 127.0.0.1]
boruna --env staging dashboard serve --data-dir /var/lib/boruna
```

When `--env` is set on the parent CLI, the dashboard reads from
`<data-dir>/<env>/runs.db` per the 0.4-S14 multi-env contract.

### Flags

| Flag | Default | Notes |
|---|---|---|
| `--data-dir` | `BORUNA_DATA_DIR` then `./.boruna/data` | Same fallback chain as `boruna workflow run`. Must exist; `runs.db` must exist inside it. |
| `--port` | `8080` | TCP port to listen on. |
| `--bind` | `127.0.0.1` | Bind address. Pass `0.0.0.0` to expose on all interfaces (see security below). |

## Security

The dashboard ships with **no authentication.** It loops back to
`127.0.0.1` by default, so out of the box it is reachable only by
local processes on the host.

If you bind to a non-loopback address (`--bind 0.0.0.0`, etc.) the
dashboard:

1. Emits a loud `[WARNING]` message on stderr at startup.
2. Renders a red banner at the top of every HTML page (index AND
   run-detail).

**Anyone with network access to that port can read every run's
metadata, every step's status, every step's `error_msg`, and (on
the per-run detail endpoint) the `policy_json` and `metadata_json`
blobs.** This is intentional — the dashboard is read-only — but
it's still data you may not want public.

The list endpoint (`GET /api/runs`) returns a **slim summary**
that excludes `policy_json` and `metadata_json` — listing all
runs would otherwise multiply the disclosure surface. The full
record is only returned for the per-run detail endpoint
(`GET /api/runs/:id`), which an operator must request
explicitly.

If you need to expose the dashboard:

- Front it with an auth-enforcing reverse proxy (nginx + Basic
  auth, oauth2-proxy, etc.).
- Terminate TLS at the proxy.
- Restrict by IP at the firewall layer if possible.

## Routes

All routes are `GET`. There are no mutation routes — `POST`, `PUT`,
`DELETE`, `PATCH` to any path return `405 Method Not Allowed`.

| Method + path | Returns |
|---|---|
| `GET /` | HTML index — table of all runs |
| `GET /runs/:id` | HTML detail — run header + step list |
| `GET /api/runs` | JSON `{ "runs": [RunSummary, ...] }` — slim view; no `policy_json` or `metadata_json` |
| `GET /api/runs/:id` | JSON `{ "run": RunRecord, "operational": RunOperational?, "steps": [StepCheckpoint, ...] }` |

The HTML rendering is intentionally simple — server-rendered
tables, inline CSS, no JS dependency, no `assets/` directory. The
JSON shapes are the source of truth; the HTML is just a
server-side render of the same data.

### JSON shapes

- `RunSummary` (list endpoint only) — `run_id`, `workflow_name`,
  `workflow_hash`, `status`, `started_at_ms`, `updated_at_ms`.
  Deliberately omits `policy_json` and `metadata_json`.
- `RunRecord` — replay-verified subset (no timestamps, no
  transient status). Used when correctness matters more than UI
  labels.
- `RunOperational` — operational subset (timestamps, transient
  status). Used by the UI for "started 5 minutes ago"-style
  display.
- `StepCheckpoint` — full step row.

These types use `#[serde(default)]` on additive fields, so JSON
clients written today will continue to parse responses from future
binaries that add new fields.

## What this is NOT

- **Not a control plane.** No "approve gate", "retry step", or
  "pause run" buttons. Mutations remain on the CLI:
  `boruna workflow approve|reject|run|resume`.
- **Not a metrics dashboard.** For aggregated graphs use the
  Prometheus exporter (sprint `0.4-S12`) and Grafana.
- **Not real-time.** The page is a snapshot at request time —
  refresh to update. No WebSockets, no SSE.
- **Not multi-tenant.** One installation, one runs.db. Multi-env
  is the closest thing — operators run one dashboard per `--env`
  if they need separate views.
- **Not authenticated.** See the security section above.

## Stability

Per `docs/stability.md`: **experimental**.

- Route paths are stable.
- CLI flag names are stable.
- JSON shapes inherit the stability of `RunRow`, `RunRecord`,
  `RunOperational`, and `StepCheckpoint` (currently stable).
- Rendered HTML is **not** stable — it may change layout,
  inline-CSS, or wording between minor releases. Don't scrape it;
  use the JSON.
