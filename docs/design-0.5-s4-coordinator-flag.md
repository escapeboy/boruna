# Design — `boruna workflow run --coordinator <url>` (sprint 0.5-S4)

## Problem

`boruna workflow run` currently has two shapes:

1. **Local synchronous** — runs the workflow in-process against a
   local sqlite `data-dir`. No coordinator involved.
2. **Submit-only** — `workflow run --submit-only --data-dir <dir>`
   inserts the run into a sqlite that a coordinator + workers
   already share, then exits. Operator drives to terminal via
   `boruna coordinator wait <run_id> --data-dir <dir>`.

Both shapes assume the operator has filesystem access to the same
`data-dir` the cluster uses. In CI workflows that target a remote
Boruna cluster (different machine, no shared volume / NFS), there
is no such path:

- The CI runner has the workflow files locally.
- The cluster runs on a server in another network segment.
- Operator has the coordinator URL and a bearer token (0.5-S3).

Result: operators are forced into ssh-tunnels-and-shared-volumes
gymnastics for a deployment shape that the existing protocol
already mostly supports.

## Goal

One command — `boruna workflow run --coordinator <url> --token
<bearer> <workflow-dir>` — that submits a workflow to a remote
coordinator over HTTP, polls for terminal status, prints step
transitions to stdout, and exits with `0` (Completed), `1`
(Failed), or `2` (Timeout / submit-failed).

No new transport, no new auth scheme. Reuses the bearer token
established in 0.5-S3 and the same JSON wire format the CLI
already understands.

## Non-goals

- **Streaming events.** Polling matches `coordinator wait` and
  keeps the wire format simple. Streaming is `0.6.x` material.
- **Asset uploads.** Workflows reference `.ax` source files
  alongside their `workflow.json`; we ship those by inlining the
  source into the submit payload. Binary artifacts, large
  assets, and per-step pre-staged uploads are out of scope.
- **Multi-tenant auth.** Single shared secret, same as
  worker-facing endpoints. Per-operator tokens / RBAC is `0.6.x`
  or later.
- **Coordinator-side workflow validation beyond what the CLI
  already runs.** Operators get the full validator on submit
  (because the coordinator parses the workflow.json), but we do
  not duplicate per-step compile checks server-side; failures
  surface as Failed step status with a clear `error_msg`.
- **Resume after CI runner exit.** If the operator's CLI dies
  mid-poll, the workflow keeps running on the cluster, but the
  CLI loses its print-as-you-go feed. Operators wanting durable
  observation use the dashboard.

## Required surface

### CLI

```
boruna workflow run \
  --coordinator https://coord.example/ \
  --token $BORUNA_TOKEN \
  [--poll-interval-ms 1000] \
  [--max-wait-secs 0] \
  [--policy <path>] \
  <workflow-dir>
```

- `--coordinator <url>` triggers the new mode. Mutually
  exclusive with `--data-dir` (different model entirely) and
  with `--submit-only` (this *is* submit + wait).
- `--token <bearer>` honors the `BORUNA_TOKEN` env var fallback
  established in 0.5-S3.
- `--poll-interval-ms` defaults to `1000`, clamped to a `500`-ms
  floor (matches `coordinator wait`).
- `--max-wait-secs 0` (the default) means no timeout.

Output matches `coordinator wait`'s line-per-transition format
so downstream tooling that parses one parses the other.

### Coordinator HTTP

Two new operator-facing endpoints, both bearer-gated by the
same `boruna-vm/auth` middleware that fronts worker endpoints.

```
POST /api/runs/submit       → { run_id, workflow_hash }
GET  /api/runs/{run_id}/status → { status, step_statuses, error_msg }
```

`POST /api/runs/submit` body:

```json
{
  "workflow": { /* full workflow.json */ },
  "step_sources": { "step_id": "<contents of step.ax>", ... },
  "policy": { /* optional Policy object, omitted == default */ }
}
```

`GET /api/runs/{run_id}/status` body:

```json
{
  "status": "running" | "completed" | "failed" | "paused" | ...,
  "step_statuses": { "step_id": "pending" | "completed" | ... },
  "error_msg": "optional failure description"
}
```

Error envelopes follow the existing `{ "error_kind": "...",
"message": "..." }` taxonomy — new kinds:
`coord.submit.invalid_workflow`, `coord.submit.bad_payload`,
`coord.runs.not_found`. All other failures fall back to existing
auth / CAS / persistence kinds.

## Why not just expose `submit` + reuse `coordinator wait`?

Because `coordinator wait` polls sqlite directly. The flag's
whole point is "no shared sqlite." We need an HTTP poll path to
break the data-dir coupling.

Once `/api/runs/{run_id}/status` exists, `coordinator wait`
*could* be retrofitted with an HTTP mode in a later sprint
(`0.6.x` candidate). For 0.5-S4 we keep the new path embedded
in the `workflow run` flow because it gives us the full lifecycle
in one command and matches the stated CI use case.
