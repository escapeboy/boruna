# Coordinator HA / Failover

This guide covers running Boruna's distributed-execution
coordinator in a high-availability configuration. Sprint W2
(v0.6.0) introduced the supporting primitives; the design doc
is at [`docs/design-coord-ha.md`](../design-coord-ha.md).

## When to use HA

The single-coord deployment (one `boruna coordinator serve`
process) is fine for development, CI, and small production
setups. Move to HA when:

- A coord process restart (binary upgrade, host reboot)
  blocks running workflows for unacceptable durations.
- The coord host has unacceptably low MTBF (frequent crashes,
  hardware issues, kernel panics).
- Compliance / SLO requires no single point of failure in the
  workflow execution path.

**HA does NOT replicate the SQLite data file.** It replicates
the *coord process*. The SQLite file remains the single source
of truth — back it up at the storage layer (filesystem
snapshots, [Litestream](https://litestream.io/), etc.).

## What HA actually buys you

| Failure | Single coord | Multi-coord |
|---------|-------------|-------------|
| Coord process crash | Workers stall until restart. | Workers register against a peer; fleet stays up. |
| Coord host reboot | Workers stall ~minutes. | Workers register against a peer; fleet stays up. |
| Binary upgrade | Workers stall during rolling restart. | Drain one coord, others serve. |
| SQLite file corruption | Cluster down. | **Cluster down** — no protection here. |
| Network partition between coord and SQLite host | Cluster down. | **Cluster down** — same. |

HA addresses **process-level** SPOF. Storage-level redundancy
is a separate concern.

## Architecture

The coord is a thin HTTP wrapper around the SQLite-backed
`RunCheckpointStore`. The state machine lives in the database:

- All claim/complete/fail/extend-lease operations are CAS-
  protected SQLite transactions.
- The lease-expiry sweep is idempotent — running it from
  multiple coords concurrently produces the same result as
  running it once (CAS rejects duplicate writes).
- SQLite WAL mode + `busy_timeout = 5000` + `BEGIN IMMEDIATE`
  with exponential backoff (per project convention §13)
  supports multiple concurrent writers to the same file.

So **multiple coord processes can run against the same data-dir
with no extra coordination**. The architectural change from
v0.5.0 is purely operational: how workers and load balancers
discover and route to coords.

## Topologies

### Topology A — Active-active behind a load balancer (recommended)

```
  ┌────────┐  ┌────────┐  ┌────────┐
  │worker-1│  │worker-2│  │worker-3│
  └───┬────┘  └───┬────┘  └────┬───┘
      │           │            │
      └───────────┼────────────┘
                  │
            ┌─────▼──────┐
            │  L4 LB     │  health: GET /api/health
            └──┬─────┬───┘
               │     │
        ┌──────┴┐  ┌─┴──────┐
        │coord-1│  │coord-2 │
        └───┬───┘  └────┬───┘
            │           │
            └─────┬─────┘
                  │
           ┌──────▼─────┐
           │  SQLite    │  (shared data-dir on
           │  data-dir  │   POSIX-compliant FS)
           └────────────┘
```

- 2-3 coord processes; all healthy; traffic spread by L4 LB
  (HAProxy, nginx stream, k8s Service, AWS NLB, etc.).
- Worker config points at the LB only:
  `--coordinator http://coord.internal:8090`.
- LB health-checks each coord at `GET /api/health`. Coords
  returning 503 (or non-200) are pulled from the rotation.

This is the simplest topology and the recommended default.

### Topology B — Active-active with worker-side failover

```
  ┌────────┐  ┌────────┐
  │worker-1│  │worker-2│
  └───┬────┘  └───┬────┘
      │           │
      └─────┬─────┘
       ┌────┼────┐
       │    │    │
  ┌────▼─┐ │ ┌──▼──┐
  │coord1│ │ │coord2
  └──────┘ │ └─────┘
           │
   shared SQLite
   data-dir
```

- Workers know all coord URLs directly:
  `--coordinator http://coord-1:8090,http://coord-2:8090`.
- No L4 LB needed.
- Worker tries URLs in order at startup; sticks to the first
  reachable one for the lifetime of the worker process.
- Operators recover from a sticky-coord crash by restarting
  the worker (k8s liveness probe handles this automatically).

Use this topology when you don't want another moving piece
(LB) or when your environment doesn't have a convenient L4 LB.

## Health endpoint

`GET /api/health` returns:

```json
{
  "protocol_version": 1,
  "status": "ready",
  "boruna_version": "0.6.0",
  "capability_set_hash": "fb16…d3a7",
  "uptime_ms": 123456
}
```

- **Authentication bypass:** `/api/health` is the only route
  that does NOT require the bearer secret. Load balancers and
  external probes can hit it without credentials.
- **Failure mode:** `503 service_unavailable` with
  `error_kind: "coord.unavailable"` if the coord's SQLite
  store mutex is poisoned. TCP-level coord failure (process
  dead) surfaces as a connect error to the probe.
- **Sensitivity:** the body deliberately excludes runtime
  state (no run counts, no worker counts, no secret
  fingerprints) so the bypass doesn't leak operational
  intel.

## SQLite shared storage caveat

SQLite's WAL mode requires POSIX-compliant advisory locks. Not
every networked filesystem honors them correctly — a divergent
lock can lead to silent data corruption. Tested-OK and
known-bad combinations:

| Storage | Status |
|---------|--------|
| Local disk (ext4, xfs, apfs) | ✅ Recommended |
| AWS EBS (single-AZ, multi-attach disabled) | ✅ Single-host only |
| Local NVMe shared by multiple containers via bind mount | ✅ Single-host |
| AWS EFS with `nolock` mount option | ❌ Will corrupt |
| NFSv3 without strong locking | ⚠️ Risky; avoid |
| GlusterFS, CephFS | ⚠️ Risky; benchmark first |

**Recommended deployment**: run coord processes on the same
host (or VM) as the SQLite file. Use HA at the host level
(VM live migration, container orchestration) rather than
sharing the SQLite file across hosts. If you need
cross-host HA at the storage layer, consider Litestream
(streaming replication to S3) or run a periodic data-dir
backup with a tested restore procedure.

## Rolling upgrades

To upgrade coord binaries without dropping in-flight workflows:

1. Verify all coords run the same `boruna_version` and report
   the same `capability_set_hash` at `/api/health`.
2. Take one coord out of LB rotation (or stop sending workers
   to its URL).
3. Wait for in-flight requests to drain
   (`uptime_ms` continuing to grow with no claims).
4. Stop the old coord process, install the new binary, start
   it.
5. Verify `/api/health` returns the new `boruna_version`.
6. Add it back to rotation.
7. Repeat for the remaining coords.

**Worker compatibility**: workers and coords must share the
same `capability_set_hash`. A capability-set change requires
restarting workers against the new coord version. This is the
"atomic upgrade" rule from ADR 002. Sprint W4-A will relax
this with per-capability version negotiation.

## Failure scenarios — what actually happens

### Coord-1 dies mid-claim long-poll

The worker's `reqwest` long-poll returns immediately with a
connection error. Worker logs the error and retries on the
next URL. No orphan claim — the claim transaction never
committed. **Net effect:** ~1 second worker delay; no
correctness impact.

### Coord-1 dies after the worker received the work but before complete posts back

The worker has finished local execution. `complete` POST
fails (connection error). Worker retries against the next
URL. The complete RPC is idempotent on the server side
(re-posting `complete` for a step already in Completed
status is a no-op via CAS). Coord-2 commits the result.
**Net effect:** the work is reported correctly; minor delay.

### Network partition isolates coord-1 from SQLite host

Coord-1's SQLite operations time out. Health probe returns
503 (mutex held by stuck operation). LB pulls coord-1 from
rotation. Workers route to coord-2. **Net effect:** transient
errors during partition; healthy operation resumes once
coord-1 reconnects or is taken out of service.

### Two coords sweep expired leases concurrently

Both call `expire_leases_and_requeue(now_ms)`. SQLite WAL +
`BEGIN IMMEDIATE` serializes the writes. The `WHERE
status = 'running' AND lease_expires_at < now_ms` predicate
filters out anything the first coord already updated. **Net
effect:** correct state; no duplicate sweeps.

## Observability

When running multi-coord, log aggregation (e.g. via the OTel
exporter from sprint 0.4-S5) becomes important. Key signals to
collect from each coord:

- `/api/health` 200 vs 503 rate.
- `coord_active_workers` metric (existing in Prometheus
  `/metrics` — sprint 0.4-S?).
- Stderr lines starting with `coordinator sweep:` (lease
  reclaim activity).
- Stderr lines starting with `coordinator startup:` (cold-
  start lease reclaim — should be 0 in steady-state HA).

If two coords are reporting wildly different
`coord_active_workers` numbers, your worker URL distribution
is uneven; investigate the load balancer config.

## What HA does NOT do (yet)

- **Coord-side replication** — out of scope. SQLite is the
  source of truth.
- **Mid-session worker failover** — workers stick to one
  coord URL after registration. Cross-cycle reassignment
  happens via worker restart. Sprint 0.7.x may add
  mid-session failover if the demand justifies the protocol
  complexity.
- **Per-coord secrets** — all coords in a cluster share the
  same `--shared-secret`. Sprint W4-B (mTLS) introduces
  per-coord cryptographic identity.
- **Auto-discovery of coord URLs** — operators wire the URLs
  themselves or use a load balancer. DNS SRV / service-mesh
  integration is environment-specific and out of scope.

## See also

- [`docs/design-coord-ha.md`](../design-coord-ha.md) — design
  doc and adversarial-review cases.
- [`docs/architecture-coordinator-worker-http.md`](../architecture-coordinator-worker-http.md)
  — the underlying coord/worker protocol.
- [ADR 002](../adr/002-distributed-step-execution.md) —
  original distributed-execution architecture decision.
