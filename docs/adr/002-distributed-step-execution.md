# ADR 002: Distributed step execution

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-04-27 |
| **Sprint** | `0.5-S1` (unblocks `0.5-S2` and the rest of the 0.5.0 cycle) |
| **Deciders** | Boruna maintainers |
| **Supersedes** | — |
| **Implements** | The "Distributed step execution" theme deferred from
| | 0.4.0; the only outstanding 0.4.x roadmap item.

## Context

Today (post-`v0.4.0`), `WorkflowRunner::run_persistent` runs the
entire workflow inside a single Boruna process. Wave-level
parallelism is real but bounded: `--concurrency N` spawns
`std::thread::spawn` workers within the same process, and the
coordinator (the calling thread) owns all SQLite + DataStore
mutation. From `orchestrator/src/workflow/runner.rs:1187-1196`:

> Each topological level (a "wave") is processed in parallel up
> to `options.concurrency` workers. Within a wave, source steps
> are dispatched to short-lived `std::thread::spawn`'d workers
> that compile + run a fresh VM and return the resulting `Value`.
> The coordinator (this function, on the calling thread) owns
> all SQLite + DataStore mutation.

This is sufficient for the operations cycle that 0.4.0 closed
out: one host, one coordinator process, one `runs.db`. It is
**not** sufficient for the 0.5.0 ("Scale") theme, where:

1. A single workflow's steps may exceed one host's CPU / memory
   / network budget. LLM-call-heavy and net.fetch-heavy
   workflows already exhibit this in real deployments.
2. Operators want to colocate workers near the data they
   process (e.g., a step that fetches from a private VPC must
   run on a host with VPC access; a step that calls an LLM
   provider must run from a host whose egress is allowlisted).
3. Worker crashes today fail the entire run (the coordinator
   thread sees the `JoinHandle` error and aborts the wave).
   Operators want a single bad host to fail-over rather than
   tank the run.

This ADR locks the architectural shape for distributed
execution. **Implementation lives in subsequent sprints** —
per project convention #27, ADR sprints capture decisions, not
code.

The existing single-process path **must keep working unchanged**.
Operators on small deployments (developer laptops, CI runners,
single-VPS shops) shouldn't need to learn a worker-pool model
to run a workflow.

### Constraints (non-negotiable)

Inherited from ADR 001 plus the shipping 0.4.x posture:

1. **Single-binary distribution.** Same `boruna-X.Y.Z-<target>.tar.gz`
   tarball; no second binary, no sidecar daemon. Distributed
   mode is a `boruna` subcommand, not a separate component.
2. **Single-process mode unchanged.** `boruna workflow run` and
   `boruna workflow resume` continue to work without any
   coordinator/worker setup. Distributed mode is opt-in. This
   protects every existing integrator (FleetQ included).
3. **Determinism preserved.** A step's `output_hash` and
   `output_json` MUST be bit-identical regardless of which
   worker ran it. Distribution is a parallelization detail; it
   does NOT enter the replay/audit pipeline. (Per project
   convention #15: which-worker-ran-this is operational state,
   never replay-verified.)
4. **No new mandatory external service.** No Redis, no RabbitMQ,
   no etcd, no Postgres, no Kubernetes. The distributed mode
   must work with only Boruna binaries + a network. Adding such
   a backend later is permitted but cannot be required for
   day-1 distribution.
5. **Local-first default.** When the user runs
   `boruna workflow run` with no coordinator present, the
   single-process path runs. Distributed mode requires explicit
   subcommand selection.
6. **Capability gating preserved.** Steps execute under the same
   `Policy` they would on a single-process run. The policy
   travels with the work claim; the worker enforces it
   identically to the in-process gateway.
7. **Per-step-stateless policy enforcement.** Today's `Policy`
   model — and this ADR — assumes capability budgets are
   per-step-stateless. Each step's NetPolicy / capability
   limits apply within that step's execution, and there are
   **no cross-step cumulative budgets** (e.g., "this run may
   make at most 100 net.fetch calls total"). If cross-step
   budgets ever ship, distribution would silently break them
   (each worker sees its own slice). Adding cross-step budget
   semantics is its own ADR; until then the property is
   explicit, not assumed.

### Decision criteria (weighted)

| Weight | Criterion |
|---|---|
| **Critical** | Backwards-compatible: existing single-process path keeps working with no changes |
| **Critical** | Determinism: output hashes byte-identical regardless of which worker ran the step |
| **Critical** | Worker crash recovery: a dead worker does not strand a step; the coordinator re-dispatches |
| **Critical** | Single-binary distribution unchanged |
| High | Wire protocol is HTTP/JSON (debuggable with curl, no special tooling) |
| High | No SQLite-on-NFS dependency (well-known footgun; mmap consistency is not portable) |
| High | Coordinator is the only writer of `runs.db` (preserves the 0.3.x persistence contract) |
| High | Operationally simple: `boruna coordinator serve` + N×`boruna worker run` |
| Medium | Scales to 10s of workers per coordinator (not 1000s — that's a different sprint) |
| Medium | Coexists with the read-only dashboard (0.4-S16) over the same listener |
| Low | Mutual auth between coordinator and worker — deferred (see Open questions) |

## Considered alternatives

### A. Status quo: horizontal scaling via separate runs

What FleetQ does today: spawn N independent `boruna workflow run`
processes, each with its own workflow input. Distributes
**runs**, not **steps within a run**.

- ✅ Zero code change.
- ❌ A single workflow's steps can't span hosts. The motivating
  use cases (a wave of 50 LLM-call steps; a step that needs VPC
  access) all want intra-run distribution.

**Rejected:** doesn't solve the actual problem.

### B. Shared-filesystem SQLite as the work queue

Add `worker_id`, `lease_expires_at`, `claim_count` columns to
`step_checkpoints`. Workers run as separate `boruna worker`
processes, all connecting to the SAME `runs.db` over a shared
filesystem (NFS, EFS, etc.). Workers atomically claim via:

```sql
UPDATE step_checkpoints
SET worker_id = ?, lease_expires_at = now + ttl
WHERE run_id = ? AND step_id = ?
  AND status = 'pending'
  AND (worker_id IS NULL OR lease_expires_at < now);
```

- ✅ Zero new networking.
- ✅ Single-binary, no new mandatory service.
- ❌ **SQLite over network filesystems is a documented footgun.**
  The SQLite project's own docs say: "SQLite database files
  should not be located on network filesystems unless the
  filesystem provides POSIX-compliant locking AND mmap
  consistency." Real-world NFS / EFS / SMB don't. WAL mode
  helps but doesn't eliminate the risk; the `.shm` shared-memory
  file expects the same mmap'd page across writers, and remote
  filesystems serve different pages.
- ❌ Adds a per-step row mutation by every worker — breaks the
  "coordinator is the only writer" property that's load-bearing
  for the audit/evidence pipeline.

**Rejected:** the "shared filesystem" assumption is too fragile
to bet the distributed-execution surface on.

### C. External queue (Redis / RabbitMQ / SQS / etc.)

Standard pattern: workers consume from a queue, ack/nack on
completion. Battle-tested.

- ✅ Mature ecosystem, well-understood operational properties.
- ❌ Breaks the single-binary constraint. Boruna currently runs
  with zero external dependencies; adding one as the
  distribution mechanism contradicts the BYOH (bring-your-own-
  handler) model that's been load-bearing since `0.3-S8`.
- ❌ Forces every operator who wants distribution to also run
  Redis/RMQ/etc. — a non-trivial operational tax for a small
  team.

**Rejected:** breaks the deployment-simplicity promise. May
re-emerge as an *optional* backend in 0.6.x once the core
protocol is stable, but cannot be the foundation.

### D. Embedded HTTP coordinator + lightweight HTTP workers (selected)

The coordinator runs `boruna coordinator serve` — an Axum-based
HTTP server (reusing the `serve` feature flag from 0.4-S16) that
owns `runs.db` and exposes a small wire protocol. Workers run
`boruna worker run --coordinator http://coord:8080`, register,
long-poll for claimable steps, execute under the coordinator's
policy, and POST results back.

- ✅ Single binary, single-process unchanged, no new mandatory
  external service.
- ✅ Coordinator remains the only writer of `runs.db` — the
  audit / evidence pipeline's invariants from 0.3.x and 0.4.x
  hold unchanged.
- ✅ HTTP/JSON is debuggable with `curl` and `gh api`-style
  tooling; no Protobuf or message-broker tooling needed.
- ✅ Reuses existing infrastructure: `serve` feature flag,
  `axum 0.8`, the dashboard's HTTP scaffolding, the dashboard's
  loopback-default security posture.
- ✅ Coordinator and dashboard can share the same listener (the
  dashboard's read routes are `/`, `/runs/:id`, `/api/runs/...`;
  the coordinator's worker routes will be `/api/workers/...`,
  `/api/work/...` — no overlap).
- ⚠️  Coordinator becomes a single point of failure. Acceptable
  for v0.5.0 — operators with HA needs run multiple workflows
  with different coordinators, the same way you run multiple
  Postgres clusters today. HA is a 0.6.x+ concern.

**Selected.** This is the architecture this ADR locks in.

### E. gRPC / streaming protocol

Same shape as D, but with gRPC instead of HTTP/JSON.

- ✅ Stronger typing, server-streaming for long-poll alternatives.
- ❌ Heavier dep tree (`tonic` + `prost` + the protoc compiler
  for build-time codegen).
- ❌ Not curl-debuggable; new tooling for operators.
- ❌ No compelling correctness or performance argument over
  HTTP/JSON at the scales we target (10s of workers, hundreds
  of in-flight steps).

**Rejected:** marginal benefits, real complexity costs.

## Decision

**Embedded HTTP coordinator + lightweight HTTP workers (alternative D).**

Two new subcommands ship behind the existing `serve` feature flag:

```sh
# On the coordinator host:
boruna coordinator serve --data-dir /var/lib/boruna [--port 8090] [--bind 127.0.0.1]

# On each worker host:
boruna worker run --coordinator http://coord.internal:8090 [--worker-id host-12]
```

The single-process path (`boruna workflow run` / `resume`)
**stays unchanged**. Operators who don't run `boruna coordinator
serve` see no behavior change.

### Wire-protocol shape (locked at the conceptual level)

The implementation sprint (`0.5-S2`) will fix the exact JSON
shapes; this ADR locks the **route surface** and the **state
machine**, not the field-level schemas.

| Method + path | Purpose |
|---|---|
| `POST /api/workers/register` | Worker announces itself; gets a worker_id (or echoes back a caller-supplied one) and a session token. |
| `POST /api/workers/heartbeat` | Worker periodically signals liveness; coordinator may extend leases on its claimed steps. |
| `GET /api/work/claim?worker_id=X` | Long-poll for the next claimable step. Returns the step descriptor + a lease deadline, or 204 No Content if nothing's claimable within the long-poll window. |
| `POST /api/work/complete` | Worker reports a successful step execution: output JSON, output hash, attempt count, capabilities used. Coordinator transitions the step from Running → Completed and re-evaluates the wave. |
| `POST /api/work/fail` | Worker reports a step failure (transient or terminal). Coordinator runs the existing retry-policy logic and either re-enqueues (Pending) or marks Failed. |
| `POST /api/work/extend-lease` | Worker explicitly requests more time on a long-running step (LLM call that's still streaming, e.g.). |

All new routes live under `/api/`; the dashboard's existing
read-only routes (`/`, `/runs/:id`, `/api/runs`, `/api/runs/:id`)
are untouched. The coordinator and dashboard share a listener
when both are enabled.

### Lease-based claim with re-dispatch on expiry

When a worker claims a step, the coordinator stamps the step's
row with three operational fields: `worker_id`,
`lease_expires_at`, and a **monotonic `claim_id`** that
increments per claim attempt against that step. (None of these
are replay-verified.) If the lease expires before the worker
reports completion, the coordinator re-enqueues the step
(status back to Pending) and the next claim allocates a fresh
`claim_id`. On re-dispatch, `attempt_count` increments — the
existing retry-budget logic from `0.3-S11` handles the
"too many attempts" terminal case unchanged.

`POST /api/work/complete` and `POST /api/work/fail` both carry
the `claim_id` they received from the original
`GET /api/work/claim` response. The coordinator does an atomic
CAS on `(step_id, claim_id)` — accept only if the step's
current `claim_id` matches, transition to terminal status in
the same transaction, and reject otherwise with `409 Conflict`,
`error_kind: "coord.lease_expired"`. **The CAS protects the
state machine from the slow-but-not-dead worker A landing its
POST after the coordinator has already re-enqueued and worker B
has claimed.** Without the `claim_id` guard, the
`status == 'Running'` check alone would have a millisecond
window where the row is back to Pending but unclaimed, and
worker A's late POST could falsely succeed.

### Coordinator restart semantics

When the coordinator process restarts, **all in-flight leases
are void.** Workers polling `claim` against a restarted
coordinator that doesn't recognize their `claim_id` get
`409 Conflict`, `error_kind: "coord.lease_expired"`, and
re-claim from scratch. The implementation does NOT persist
lease state across coordinator restarts — `worker_id` and
`lease_expires_at` are written to `runs.db` for observability
(the dashboard surfaces "currently held by worker host-12"),
but the coordinator's authoritative lease state lives in
memory. On startup, the coordinator scans `runs.db` for steps
in `Running` status and re-enqueues them as `Pending`.

This is the simpler choice. Persisting in-memory lease state
across restarts buys very little: a coordinator crash is
rare-ish, the worker pool is small enough that re-claim
overhead is in the milliseconds, and the alternative
introduces a second consistency surface (memory vs. disk) that
must be kept in sync. Treat coordinator restart as a partition
event.

### Output payload size

`POST /api/work/complete` carries `output_json` inline. The
**maximum body size is 8 MB.** Workers that produced a larger
output get `413 Payload Too Large`, `error_kind:
"coord.output_too_large"`. The route reserves a
`output_blob_ref: Option<String>` field (currently always None)
for a future content-addressed-blob backend; out of scope this
sprint, additive when it lands. 8 MB matches common defaults
across HTTP stacks and aligns with the existing 1 MB MCP
`source` cap × 8 to give workflow steps comfortable
headroom for normal LLM-output / structured-data sizes.

### Determinism guarantee

The output of a step is `(workflow_hash, step_id, resolved_inputs,
policy_hash, capability_set_hash, source_hash) → output_hash`.
**No worker identity in the input set.** Whether host A or host
B ran the step is operational. This holds the existing
determinism contract from project convention #15.

The audit log records `worker_id` per attempt as operational
metadata — useful for triage, never replay-verified.

### Atomic upgrade required for v0.5.0

Workers connecting to a coordinator must run binaries with the
**same `capability_set_hash`** (the SHA-256-of-(name, version)
pairs locked in 0.3-S15). Heterogeneous binary versions are
**rejected at registration time** with `error_kind:
"coord.binary_mismatch"`.

The operational consequence: **upgrading a deployed Boruna
cluster requires stopping all workers, upgrading the
coordinator and worker binaries, then restarting all workers**
in a coordinated window. There is no rolling-upgrade story for
v0.5.0. Operators planning to upgrade must schedule a
maintenance window that drains in-flight runs OR accept that
an upgrade voids in-flight leases (per the restart semantics
above). Heterogeneous-version support and rolling upgrades are
a 0.6.x+ concern.

This is a deliberate decision: distributed determinism over
heterogeneous binaries adds substantial protocol complexity
(per-capability version negotiation, per-step compatibility
checks). The simpler atomic-upgrade rule is operationally
acceptable for the v0.5.0 target deployment scale (10s of
workers per coordinator).

### Concurrency-flag semantics

The existing `--concurrency N` flag on `boruna workflow run`
**keeps its current meaning unchanged**: in-process thread
workers within a single host. When the run is submitted to a
remote coordinator (the future `--coordinator <url>` flag,
sprint 0.5-S2+), `--concurrency` no longer applies — the
coordinator dispatches one step per worker long-poll, and
worker-pool size is the implicit concurrency.

This split is deliberate. The local-first single-process path
keeps every operator's existing mental model. Distributed
mode introduces a second concurrency dimension (worker count),
which is set by how many `boruna worker run` processes are
running, not by the workflow client.

### Backwards compatibility

- `boruna workflow run` continues to dispatch via in-process
  `std::thread::spawn` workers when no coordinator is involved.
- `boruna workflow run --coordinator <url>` (new flag, separate
  sprint) submits the run to a remote coordinator and tails its
  status. The local process becomes a thin client; workers on
  remote hosts execute the steps. Behavior is otherwise
  identical from the operator's perspective.
- `runs.db` schema gains a small additive set of columns
  (`worker_id`, `lease_expires_at`, `claim_count` on
  `step_checkpoints`) per the `0.3-S2a` schema-migration
  pattern. Old databases auto-migrate at open time. Old binaries
  reading new databases see the columns as ignored extras (per
  convention #11, `#[serde(default)]`).

## Consequences

### Positive

- A single workflow's wave can fan out across hosts. LLM-call-
  heavy and net-bound workflows scale without changing the
  workflow definition.
- Worker crashes no longer fail the run — lease expiry +
  re-dispatch handles host-level fault tolerance for free.
- The protocol is debuggable with `curl` (consistent with the
  dashboard's debugging story).
- Capability colocation: operators tag worker hosts with
  capability flags (future sprint), and the coordinator routes
  steps requiring those capabilities to those workers.

### Negative

- Coordinator is a single point of failure. v0.5.0 ships with
  no HA story; operators run one coordinator per workflow
  cluster, identical to running one Postgres cluster.
- The HTTP-poll model has a small floor latency (one round-trip
  + the long-poll wait time). Workflows with many
  millisecond-scale steps will see overhead. Acceptable —
  Boruna's typical step is an LLM call, an HTTP fetch, or a DB
  query; sub-second step times are not the design target.
- Coordinator + workers must talk over a network. Tunneling and
  mTLS are operator concerns until the auth sprint (see Open
  questions).
- Two-process operation increases failure modes (coordinator
  crash, network partition, lease-expiry edge cases). The
  implementation sprint must include integration tests for each.

### Neutral

- The `0.4-S16` dashboard now has a sibling: dashboard is the
  read view, coordinator is the write/control surface. Both
  share the `serve` feature flag.
- The `0.4-S15` policy validator's locked taxonomy applies
  unchanged — workers parse policies via the same
  `boruna_vm::policy_validate::parse` entry point.

## Open questions

These are explicitly out of scope for this ADR and for the next
implementation sprint. They get their own ADRs / sprints when
they become load-bearing.

1. **Authentication between coordinator and worker.** v0.5.0
   ships with **no authentication** — the coordinator binds
   loopback by default, identically to the dashboard. Operators
   exposing the coordinator on a network must front it with an
   auth-enforcing reverse proxy. A native shared-secret or mTLS
   sprint follows. Documented in 4 places (per convention #18)
   when the implementation lands.

2. **Coordinator HA / failover.** Out of scope. Operators run
   one coordinator per workflow cluster. HA is a 0.6.x+ concern.

3. **Worker capability tagging / placement.** Steps requiring
   `net.fetch` to a VPC private endpoint should land on workers
   in that VPC. The protocol leaves room for it (`POST /api/workers/register`
   accepts a future `capabilities: [...]` field), but the
   matching/scheduling layer ships without it in 0.5-S2.

4. **External queue backend (Redis / RMQ / SQS).** Punted to
   0.6.x. The HTTP protocol is the v1 surface; alternative
   backends become possible once the protocol is stable.

5. **Pull vs push dispatch.** This ADR commits to **pull**
   (workers long-poll the claim endpoint). Push (coordinator
   sends work to registered workers) is operationally heavier
   (workers must run a server too) and offers no compelling
   correctness or latency win at our scale. Reconsider only if
   the long-poll overhead becomes load-bearing.

6. **Distributed run-id derivation.** The 0.3-S2b deterministic
   `run_id` derivation (sha256(workflow_hash + inputs +
   counter)) holds unchanged — only the coordinator computes
   it.

7. **Step-input streaming.** Today step outputs are
   JSON-encoded in `output_json` and consumed via `step_input`.
   For very large outputs (multi-MB), a content-addressed blob
   store would help. Out of scope; the protocol leaves room for
   `output_blob_ref: String` as a future additive field.

## Implementation notes (for `0.5-S2`)

These are non-binding guidance per project convention #28
("the reviewer enforces no implementation snuck into the ADR").
Treat as a hint to the next sprint's Plan phase, not a contract.

1. New crate? **No.** The coordinator HTTP layer lives in
   `crates/llmvm-cli/src/coordinator.rs` next to `dashboard.rs`,
   under the same `serve` feature flag. The worker HTTP client
   lives in `crates/llmvm-cli/src/worker.rs`. Both reuse
   `boruna-orchestrator` for the actual workflow logic.
2. Reuse the dashboard's `Arc<Mutex<RunCheckpointStore>>`
   pattern. The coordinator's HTTP handlers acquire the mutex
   briefly per request, identically to the dashboard. No
   long-lived locks across `await`.
3. Schema migration to add `worker_id`, `lease_expires_at`,
   `claim_count` columns goes through the same v1→vN migration
   runner the persistence module already has (`init()`).
4. The wire format starts as JSON. Reuse the `protocol_version`
   pattern from MCP responses (project convention #4) — every
   coordinator response carries `protocol_version: 1`. Locks
   the wire shape early.
5. Long-poll wait window: 30 seconds default, configurable via
   query string. Workers retry the claim endpoint after each
   timeout.
6. Lease TTL: 5 minutes default, extendable via
   `POST /api/work/extend-lease`. Configurable per workflow.
7. Worker process compiles `.ax` source freshly on each step
   (no shared compile cache across workers). The
   `capability_set_hash` from 0.3-S15 ensures workers running
   different binary versions are detectable at registration
   time — registration includes the worker's `capability_set_hash`,
   coordinator rejects workers whose hash doesn't match its own.
   Heterogeneous binary versions are a 0.6.x concern.
8. Integration tests: spawn a coordinator + 1-3 workers in a
   tempdir, run a small DAG end-to-end, kill a worker mid-step,
   assert lease expiry + re-dispatch + correct final
   `output_hash`.
9. Per project convention #2 — every new HTTP `error_kind`
   string is locked at first ship. Anticipated taxonomy for
   `0.5-S2`: `coord.lease_expired`, `coord.unknown_worker`,
   `coord.workflow_completed`, `coord.binary_mismatch`,
   `coord.invalid_request`, `coord.output_too_large`. Subject to
   revision in the implementation sprint, but lock at first ship.

The next sprint's Plan phase should size the implementation —
my rough estimate is **medium** (1 sprint for the protocol +
schema columns + happy-path integration test; subsequent
sprints layer on auth, HA, capability tagging).
