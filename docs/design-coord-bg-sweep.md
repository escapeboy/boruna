# Design — Coordinator background lease-expiry sweep + multi-step DAG test (sprint 0.5-S2c)

## Premise

Sprint `0.5-S2b` shipped the HTTP coordinator + worker MVP.
Lease expiry today only fires at coordinator startup — once
running, expired leases sit indefinitely until a manual sweep
(via direct persistence-API call) or a coordinator restart.

This is operationally fragile. Worker A claims a step at 10:00
with a 5-minute lease, crashes at 10:01, and the step stays in
`Running` status with a stale `worker_id=A` until the
coordinator restarts. Workers B, C, D never see the step as
claimable.

This sprint adds the missing piece: a **periodic background
sweep** as a tokio interval task running on the coordinator.
Plus a **multi-step DAG integration test** that proves the
protocol's non-trivial-workflow story works (even though the
coordinator itself doesn't yet know about workflow DAGs —
that's 0.5-S2d).

## Architectural note: the coordinator is a dumb transport

In v0.5.x, the coordinator does NOT advance workflow waves. It
dispatches whatever's in `Pending` status, persists what
completes, and re-enqueues what expires. The "wave loop"
(deciding which step is Pending after a successful completion
based on DAG dependencies) lives in the **client** — for now,
that's whatever process pre-populated `runs.db` with the
initial wave's checkpoints. In 0.5-S2d the client becomes the
existing `boruna workflow run` runner extended with a
`--coordinator <url>` mode.

Splitting orchestration (client) from transport (coordinator)
is a deliberate choice for v0.5.x. It keeps the coordinator
small and stateless-ish, makes the protocol surface compact,
and means we can change orchestration strategy without
re-implementing the transport layer.

## Who needs this

- **Operators** running coordinator + workers in production.
  Without the background sweep, a worker crash silently
  strands its claimed step until coordinator restart.
- **The 0.5-S2d implementer** (next sprint). When they add the
  client-side wave loop, the coordinator must be able to
  re-dispatch expired claims without restart, otherwise long-
  running workflows stall on the first worker crash.
- **Anyone running the integration tests.** A multi-step DAG
  test with a kill-mid-step scenario needs the sweep to fire
  WITHOUT a restart.

## Narrowest MVP

Two changes:

1. A new tokio task spawned by `boruna coordinator serve` that
   wakes up every `--sweep-interval-ms` (default 30000 = 30s)
   and calls `expire_leases_and_requeue(now_ms)`. Logs the
   number of requeued steps when non-zero.

2. A new CLI integration test
   (`coord_bg_sweep_requeues_expired_lease`) that:
   - Spawns coordinator with `--sweep-interval-ms=200`.
   - Pre-populates a Pending step.
   - Calls `claim_step` directly with `lease_expires_at` in
     the past.
   - Waits ~500ms.
   - Asserts the row's status is back to Pending.

Plus a multi-step DAG integration test
(`worker_completes_two_step_linear_dag`) that:
- Pre-populates a 2-step linear DAG: step1 (Pending),
  step2 (Pending dependency on step1, but for the MVP
  pre-populated as Pending too since the coordinator doesn't
  do DAG advancement yet).
- Spawns 1 worker.
- Worker claims step1, completes; claims step2, completes.
- Asserts both rows are `Completed` with correct `output_hash`.

## What would make someone say "whoa"

- **The CLI integration test for the sweep is deterministic.**
  No race conditions. We use `--sweep-interval-ms=200` so the
  test runs in <1s. The flagship 0.5-S2b kill-mid-step test
  used a `simulated stale claim`; this one uses the real
  background-sweep mechanism, proving it actually works in
  production.
- **The architecture note documents what's deliberately
  deferred.** Operators know the v0.5.x coordinator is a
  "dumb transport" — they shouldn't expect it to track
  workflow DAG state. Setting expectations here avoids
  surprises in 0.5-S2d.

## How this compounds

- Once the background sweep ships, 0.5-S2d's runner-integration
  sprint can assume "expired leases get re-dispatched
  automatically" — no need for the client to drive the sweep.
- The multi-step DAG integration test is a regression for the
  protocol surface; future protocol-evolution work can
  re-run it.
- The `--sweep-interval-ms` flag is forward-compatible with
  per-environment tuning (faster sweeps in dev, slower in
  prod).

## Scope (what this sprint changes)

- New tokio task in `coordinator::run_serve`: wakes up every
  `sweep_interval_ms`, calls `store.expire_leases_and_requeue`.
- New `--sweep-interval-ms <ms>` CLI flag (default 30000).
- 1 new CLI integration test for the background sweep.
- 1 new CLI integration test for a 2-step DAG.
- CHANGELOG `[Unreleased]` entry.

## Non-goals (deferred to 0.5-S2d or later)

- **`boruna workflow run --coordinator <url>` client mode.**
  The marquee feature for distributed workflows. Substantial
  enough to deserve its own sprint.
- **Coordinator-side DAG advancement.** Today the
  coordinator dispatches whatever's in Pending; it doesn't
  read workflow.json or know which step's completion unlocks
  which downstream step. The client owns the wave loop.
- **`POST /api/runs/submit`** route for remote workflow
  submission over HTTP. Defer until shared-filesystem
  deployment becomes constraining.
- **Dashboard listener-merge.** Coordinator and dashboard run
  on different ports today. Merging is its own small sprint.
- **Sweep observability** (Prometheus counter, OTel span).
  Deferred — `eprintln!` logging is sufficient for v0.5.x.
- **Worker crash detection via missing heartbeats.** The
  registry's `last_heartbeat_ms` field is captured but not
  yet acted on; future sweep evolution.

## Stable contract

- `--sweep-interval-ms` flag name and default (30000).
- The sweep is best-effort — failures log + continue, never
  panic the coordinator.
- The sweep fires the same `expire_leases_and_requeue` API
  from 0.5-S2a; behavior is identical to a manual call.

## Stability tier

Per `docs/stability.md`: **experimental**.

- The flag name is stable.
- The interval default is stable (30000 ms).
- Failure semantics (best-effort, log + continue) are stable.
