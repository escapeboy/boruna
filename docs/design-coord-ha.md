# Coordinator HA / Failover (Sprint W2)

Status: design
Last revised: 2026-04-28

## Problem

The 0.5.0 distributed-execution stack has a single coordinator
process. If the coord crashes, dies on a host reboot, or is
restarted for a binary upgrade:

- Workers cannot reach the coord HTTP endpoint, so claim/complete/
  fail/extend-lease all 503 or hang.
- The background lease-expiry sweep stops, so any in-flight
  step's lease keeps ticking but no one will requeue when it
  expires (until a coord comes back).
- New runs cannot be submitted (`POST /api/runs/submit`).

Operators running Boruna in production cannot tolerate this SPOF.
W2 eliminates it.

## Constraints we already satisfy

The coord is a thin HTTP wrapper around the SQLite-backed
`RunCheckpointStore`. The state machine lives in the database:

- Claim: `claim_step` is a CAS-protected SQLite UPDATE.
- Complete / fail: `complete_step_cas` / `fail_step_cas` are
  CAS-protected.
- Extend lease: `extend_lease_cas` is CAS-protected.
- Sweep: `expire_leases_and_requeue` is idempotent — running it
  from multiple coords concurrently produces the same result
  as running it once (CAS rejects duplicate writes).

SQLite in WAL mode + `busy_timeout = 5000` + `BEGIN IMMEDIATE`
+ exponential-backoff retry on `SQLITE_BUSY` (per project
convention §13) already supports multiple concurrent writers
to the same file.

In other words: **the persistence layer is already HA-safe**.
The coord process is the SPOF, not the data.

## What W2 ships

A coord deployment can run as **N ≥ 1 stateless coord
processes** sharing a single SQLite data-dir, with workers
configured to fail over between them.

### 1. Premise audit — startup sweep is already HA-safe

Per project convention §40 (audit retro premises), I read the
actual code before committing to a "fix." The startup sweep at
`crates/llmvm-cli/src/coordinator.rs:127` calls
`expire_leases_and_requeue(now_ms + 1)`, which the persistence
layer implements as:

```sql
UPDATE step_checkpoints
SET status = 'pending', worker_id = NULL, lease_expires_at = NULL
WHERE status = 'running'
  AND lease_expires_at IS NOT NULL
  AND lease_expires_at < ?1
```

Only leases whose `lease_expires_at < now_ms + 1` are voided.
Healthy leases held by other workers are NOT touched. The
existing code is therefore **already HA-safe** under multiple
concurrent coords.

**Action**: no behavior change. Update the misleading comment
("any row in Running status is a leftover") to match the actual
threshold-based behavior.

### 2. Worker URL failover

Today the worker takes a single `--coordinator <url>`. Add
support for multiple URLs:

```
boruna worker \
  --coordinator http://coord-1:8080,http://coord-2:8080,http://coord-3:8080 \
  --shared-secret "$BORUNA_SECRET"
```

Worker behavior:
- Try URLs in order; on connection refused / 503 / timeout,
  move to the next URL.
- After cycling all URLs, sleep `--retry-backoff-ms` (default
  500ms, doubles up to 30s) and retry.
- Successful claim "sticks" to the URL until the next failure
  (so a long-poll doesn't hop URLs mid-poll).
- Log URL transitions so operators can correlate worker
  output with coord availability.

### 3. Health endpoint with readiness signal

Add `GET /api/health` returning:
```json
{
  "status": "ready",
  "protocol_version": 1,
  "boruna_version": "0.6.0",
  "capability_set_hash": "<hex>",
  "data_dir": "<absolute path>",
  "uptime_ms": 123456
}
```

Failure cases:
- `503 service_unavailable` if the SQLite store can't be opened
  or if the store mutex is poisoned.
- `4xx coord.binary_mismatch` is not used here — health is
  binary-agnostic; binary-mismatch is a per-claim concern.

Workers use this endpoint at startup to validate their list of
coord URLs. Load balancers (HAProxy, nginx, k8s) use it for
health checks.

### 4. Multi-coord deployment doc

`docs/guides/coord-ha.md`:

- Two recommended topologies:
  - **Active-active behind a load balancer**: 2-3 coord
    processes, all healthy, traffic spread by L4 LB. Worker
    URLs point at the LB only. Simplest.
  - **Active-active with worker-side failover**: workers know
    all coord URLs directly. Skips the LB. Useful when the
    operator doesn't want another moving piece.
- SQLite shared-storage caveat: NFS is risky (locking can
  diverge from POSIX); recommend local-disk shared via
  filesystem (e.g. EFS with `nolock` is NOT acceptable; only
  use storage that honors POSIX advisory locks). For
  production HA, document the requirement and link to the
  SQLite docs on shared storage.
- Rolling-upgrade procedure (drains workers from one coord
  via stopping it; remaining coords pick up).

### 5. Adversarial-review cases

H1. **Two coords on the same host, racing on lease expiry.**
The sweep is idempotent (CAS); both can run, one wins.
Verified by a unit test that runs two `expire_leases_and_requeue`
calls concurrently against a shared store.

H2. **Worker mid-claim when its current coord dies.** The
`/api/work/claim` is a long-poll (default 30s). If the coord
dies during the poll, the worker's `reqwest` returns an error
immediately (connection reset). The worker advances to the next
URL. No orphan claim — the claim transaction was never
committed.

H3. **Worker mid-complete when its current coord dies.** This
is the dangerous one: the work is done locally, but the report
home failed. The complete RPC is idempotent on the server side
(re-posting `complete` for a step already in Completed status
is a no-op via CAS). So the worker re-tries the next URL with
the same payload — coord-2 commits the result.

H4. **Two coords concurrently completing the same step.** Can't
happen by current claim semantics (only one worker holds the
claim_id; only that worker can complete). But: if a network
partition causes coord-1 to think the worker is gone, coord-1
might `expire_leases_and_requeue`, the step goes back to
Pending, a different worker claims it, and BOTH workers think
they own it. The second worker's complete will succeed first
(CAS by claim_id); the first worker's complete will fail with
`coord.lease_expired`. Worker treats this as a permanent error
and reports stderr; the run completes correctly with one
worker's output. Verified by an integration test.

H5. **Coord process crash mid-claim transaction.** SQLite WAL
mode + atomic-commit guarantees the claim either committed or
didn't. If didn't: step is still Pending; another claim picks
it up. If did: claim_id is in DB; the worker's HTTP call
returned, work proceeds. No corruption.

## What W2 explicitly does NOT ship (deferred to 0.6.x or 0.7.x)

- **Leader election among coords** — not needed. All coords
  are stateless wrappers around SQLite.
- **Coord-side replication** — out of scope. SQLite is the
  single source of truth; replicate at the storage layer
  (filesystem snapshots, Litestream, etc.).
- **Mid-run worker reassignment** — workers stick to a URL
  for a single claim/complete cycle. Cross-cycle reassignment
  is the URL-rotation behavior.
- **mTLS / per-coord keys** — deferred to W4-B. Today auth is
  shared-secret bearer; the same secret works for all coords
  in the cluster.
- **Worker-driven coord discovery** (DNS SRV, service mesh) —
  too operator-environment-specific. Operators wire URLs
  themselves or use a load balancer.

## Acceptance

- `cargo test --workspace` green.
- `cargo clippy --workspace --features boruna-cli/serve --all-targets -- -D warnings` clean.
- `cargo fmt --all -- --check` clean.
- 5 new tests covering H1–H5 above.
- New integration test: spin up 2 coord processes against the
  same data-dir, kill one mid-run, verify the run completes.
- Doc: `docs/guides/coord-ha.md` with 2 deployment topologies.

## Migration

This is fully additive. Operators running a single coord see
no behavior change. The `--coordinator` flag still accepts a
single URL; the comma-separated form is opt-in. No persistence
schema changes.

CHANGELOG entry: `### Added` (multi-URL coord failover, health
endpoint, multi-coord deployment topology guide).
