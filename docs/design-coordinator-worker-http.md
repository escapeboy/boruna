# Design — Coordinator/worker HTTP MVP (sprint 0.5-S2b)

## Premise

ADR 002 locked the architectural shape. Sprint 0.5-S2a hardened
the persistence-layer state machine with `claim_id` + atomic CAS.
This sprint wraps that state machine in an HTTP protocol so
remote workers can claim work over the wire.

**This is the protocol-only sprint.** It ships:

- A coordinator subcommand exposing the 6 routes from ADR 002.
- A worker subcommand that polls, claims, executes a step's `.ax`
  source, and reports.
- An end-to-end integration test that spawns coordinator + 1
  worker against a pre-populated `runs.db` and asserts a step
  flows through claim → execute → complete.
- A second integration test that exercises the kill-worker
  scenario: A claims, A is killed, lease expires, B reclaims, B
  completes; `output_hash` matches what B produced.

**Wave-loop runner integration is the next sprint (0.5-S2c).**
This sprint's MVP doesn't yet drive workflow progress through
the coordinator — operators populate `runs.db` via existing
mechanisms (`boruna workflow run --ephemeral=false` writes the
checkpoints, which workers can consume), or via a test helper.
The marquee feature (`boruna workflow run --coordinator <url>`)
follows.

## Who needs this

The 0.5-S2c implementer and any operator running a
multi-host POC. The persistence API is too low-level to invoke
across the network; the HTTP layer is the actual integration
surface for everything that follows. Plus, having the HTTP layer
in place lets us debug the protocol with `curl` against a real
coordinator, before building the workflow-runner glue on top.

## Narrowest MVP

Two subcommands behind the `serve` feature flag:

```
boruna coordinator serve --data-dir /var/lib/boruna [--port 8090] [--bind 127.0.0.1]
boruna worker run --coordinator http://coord.internal:8090 [--worker-id host-12]
```

Coordinator exposes:

| Method + path | Purpose | State change |
|---|---|---|
| `POST /api/workers/register` | Worker announces itself | None (in-memory only) |
| `POST /api/workers/heartbeat` | Liveness signal | None (extends in-memory worker registry) |
| `GET /api/work/claim?worker_id=X` | Long-poll for claimable step | `claim_step` if any Pending step exists |
| `POST /api/work/complete` | Worker reports success | `complete_step_cas` |
| `POST /api/work/fail` | Worker reports failure | `fail_step_cas` |
| `POST /api/work/extend-lease` | Push out lease | `extend_lease_cas` |

All routes carry `protocol_version: 1` on every response (project
convention #4).

Worker loop:
1. Register at startup. Receive worker_id (caller-supplied or
   server-allocated UUID-like).
2. Loop:
   1. `GET /api/work/claim?worker_id=...` (long-poll, 30s
      timeout). On 204, retry. On 200, decode the work item.
   2. Compile the step's `.ax` source (the work item carries it
      inline).
   3. Run the VM under the work item's policy.
   4. `POST /api/work/complete` with output JSON + hash, OR
      `POST /api/work/fail` with error message.
3. Heartbeat in a background task every 10s.
4. On SIGINT: stop accepting new claims, finish current step,
   exit cleanly.

That's the MVP. Anything else is deferred.

## What would make someone say "whoa"

- **One stable parser end-to-end.** Worker JSON shapes
  serialize/deserialize via the same types — there's no
  schema-drift surface between coordinator and worker. The
  outcome enums from 0.5-S2a serialize directly via `serde`.
- **`coord.lease_expired` actually works at the wire level.**
  An operator can `curl` the complete endpoint with a stale
  `claim_id` and see exactly the same `error_kind` they'd see
  from the persistence layer's `kind()` string.
- **Bit-identical `output_hash` regardless of which worker ran
  the step.** The integration test's kill-worker variant proves
  this: A and B run different `.ax` source bodies (test fixture
  forces this), the coordinator's complete CAS rejects A's
  late report, B's hash is what's persisted, and the row's
  `output_hash` matches B's hash exactly.

## How this compounds

- Once the protocol is stable, 0.5-S2c can wire the wave-loop
  runner to dispatch via the same routes — no protocol-design
  burden in that sprint, just integration plumbing.
- Future work (auth, capability tagging, blob storage) becomes
  additive at the route level. New optional fields,
  `#[serde(default)]`, locked stable.
- The dashboard (0.4-S16) and the coordinator share a listener;
  operators get fleet visibility AND distributed dispatch from a
  single binary.

## Scope (what this sprint changes)

- New: `crates/llmvm-cli/src/coordinator.rs` — Axum router for
  the 6 routes, in-memory worker registry, JSON wire types.
- New: `crates/llmvm-cli/src/worker.rs` — HTTP client loop,
  registration, heartbeat task, claim-execute-report cycle.
- New: `boruna coordinator serve` subcommand. Mirrors
  `dashboard serve`'s flag set: `--data-dir`, `--port`,
  `--bind`. Loopback default. Same security posture (no auth,
  loud bind warning).
- New: `boruna worker run` subcommand: `--coordinator <url>`,
  `--worker-id <name>` (optional), `--lease-ttl-ms <ms>`
  (default 300_000 = 5 min), `--poll-timeout-ms <ms>` (default
  30_000 = 30 s).
- New: `boruna_capability_set_hash` check at registration —
  workers whose hash mismatches the coordinator's are rejected
  with `coord.binary_mismatch` (per ADR 002 atomic-upgrade
  rule).
- New: `crates/llmvm-cli/tests/cli_coordinator_worker.rs`
  integration tests.
- New: `docs/reference/coordinator-worker.md` operator-facing
  reference.
- CHANGELOG `[Unreleased]` entry.

## Non-goals (deferred)

- **No `boruna workflow run --coordinator <url>` mode.** Sprint
  0.5-S2c. The runner-integration question (how the coordinator
  advances the wave) needs its own design pass.
- **No HTTP-server-side wave loop.** This sprint's coordinator
  does NOT mark next-wave steps as Pending after a completion.
  Workers consume whatever's already Pending; populating
  Pending steps is the runner's job (current single-process
  path or future `--coordinator` mode).
- **No authentication.** Loopback default, banner on non-
  loopback. Per ADR 002 open question 1.
- **No TLS.** Operator concern via reverse proxy.
- **No blob output.** 8 MB cap from ADR 002 enforced via
  `413 Payload Too Large` + `coord.output_too_large`. Reserved
  `output_blob_ref` field stays None.
- **No worker capability tagging.** Workers are opaque strings.
  Capability-aware claim is 0.5-S4+.
- **No observability beyond `tracing` logs.** OTel spans on the
  HTTP path are an additive future change.

## Stable contract (locked at this sprint's ship)

- The 6 route paths.
- The `coord.*` `error_kind` taxonomy:
  `coord.lease_expired`, `coord.unknown_worker`,
  `coord.workflow_completed`, `coord.binary_mismatch`,
  `coord.invalid_request`, `coord.output_too_large`,
  `coord.step_not_found`.
- `protocol_version: 1` on every response.
- The CLI flag names and defaults.
- The JSON shape of work items, completion reports, lease
  extensions.

Future fields are additive (per convention #11
`#[serde(default)]`). Renames or removals are breaking.

## Open questions for next sprint

- How does the coordinator decide a workflow is complete and
  fire the workflow-completed audit event? Today the runner
  does this; the coordinator MVP doesn't track workflow-level
  state. 0.5-S2c handles it.
- Should the worker's `--lease-ttl-ms` be coordinator-enforced
  (server overrides client) or worker-suggested (client
  proposes)? This sprint goes with worker-suggested capped at
  the coordinator's max. Reconsider if abuse cases emerge.
- Long-poll timeout: 30s default fits most environments but is
  a per-worker concern. Defer to operator config; document the
  tradeoff.

## Stability tier

Per `docs/stability.md`: **experimental**.

- Route paths and `error_kind` strings are stable.
- CLI flag names are stable.
- JSON shape evolves additively under `protocol_version: 1`.
- The internal scheduling behavior (which step claim returns,
  how leases interact) is locked at the persistence layer
  (0.5-S2a) — the HTTP layer is just a wrapper.
