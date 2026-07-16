# Boruna — Distributed Execution + Persistence (Coordinator/Worker HTTP Cluster)

Research slice: distributed execution + persistence. READ-ONLY audit. Every claim cites `path:line`.

> **Key locator correction:** the coordinator/worker HTTP cluster does **not** live in
> `orchestrator/`. It lives in the CLI crate **`crates/llmvm-cli/`** (`coordinator.rs`,
> `worker.rs`, `dashboard.rs`). `orchestrator/` provides the persistence layer
> (`RunCheckpointStore`, `BlobStore`) + workflow runner the coordinator wraps.
> `orchestrator/src/main.rs` + `orchestrator/src/cli/mod.rs` are the **multi-agent patch-bundle**
> CLI (`boruna-orch`) — unrelated to distributed execution (`orchestrator/src/main.rs:9-51`,
> `orchestrator/src/cli/mod.rs` dispatches `cmd_plan/next/apply/review/status/report`). Noted so the
> reader does not expect the coord code where the brief pointed.

---

## 1. Purpose & Architecture

A coordinator process wraps the persistence-layer claim/lease state machine in an HTTP protocol so
remote workers can claim/execute/report `.ax` workflow steps over the wire
(`crates/llmvm-cli/src/coordinator.rs:1-27`).

- **Coordinator** (`coordinator.rs`): axum HTTP server. Owns a single
  `Arc<Mutex<RunCheckpointStore>>` (SQLite) + in-memory `HashMap<worker_id, WorkerSession>`
  (`coordinator.rs:58-75`). Routes: worker lifecycle (`/api/workers/*`, `/api/work/*`) and
  operator lifecycle (`/api/runs/*`), plus a merged read-only dashboard and an unauthenticated
  `/api/health` (`coordinator.rs:799-841`).
- **Worker** (`worker.rs`): reqwest client. Registers, long-polls `/api/work/claim`, compiles+runs
  the step in-process on the VM under the step's policy, reports `complete`/`fail`
  (`worker.rs:344-435`).
- **Persistence** (`orchestrator/src/persistence/mod.rs`): `RunCheckpointStore` over SQLite (WAL,
  `foreign_keys=ON`, `busy_timeout=5000`) — the durable claim/lease CAS state machine
  (`mod.rs:615-623`). Schema v1→v4, forward-only additive migrations
  (`mod.rs:630-712`).
- **Blob store** (`orchestrator/src/persistence/blob_store.rs`): sharded content-addressed store for
  step outputs > `BLOB_THRESHOLD`; hash-validated before any FS access (`blob_store.rs:118-190`).
- **HA model**: multiple coordinators can share one `runs.db`; lease expiry uses a strict `<`
  threshold so peer coords' healthy leases are untouched (`coordinator.rs:213-233`,
  `mod.rs:2013-2040`). Workers fail over across a comma-separated `--coordinator` URL list **at
  registration time only** (sticky thereafter) (`worker.rs:216-302`).
- **Security posture (self-declared)**: loopback by default, **no auth unless configured**; optional
  shared-secret bearer OR mTLS client-cert (`coordinator.rs:10-21, 108-129`).

---

## 2. Component Inventory

| File | Responsibility | Key types / fns | Status |
|---|---|---|---|
| `crates/llmvm-cli/src/coordinator.rs` | Coord HTTP server, TLS listener, auth mw, handlers | `run_serve`, `build_router`, `auth_middleware`, `handle_claim/complete/fail/extend_lease/register/heartbeat/submit_run/approve_run/trigger_run/get_blob/health`, `serve_with_tls`, `cn_from_cert_der` | Complete, tested |
| `crates/llmvm-cli/src/worker.rs` | Worker client: register/claim/execute/report, mTLS client, HA failover | `run_worker`, `main_loop`, `claim_one`, `execute_step`, `report_complete/fail`, `parse_coordinator_urls` | Complete, tested |
| `crates/llmvm-cli/src/dashboard.rs` | Read-only fleet dashboard routes merged into coord | `dashboard_routes`, `run_serve` | Present (not deep-audited; merged under same auth) |
| `orchestrator/src/persistence/mod.rs` | SQLite checkpoint store; claim/lease CAS; migrations | `RunCheckpointStore::{open,init,claim_step,complete_step_cas,fail_step_cas,extend_lease_cas,expire_leases_and_requeue,run_owns_blob_ref}` | Complete, tested |
| `orchestrator/src/persistence/blob_store.rs` | Content-addressed blob store + GC | `BlobStore::{open,write,read_bytes,delete,find_orphans}`, `validate_hash` | Complete, well-hardened |
| `orchestrator/src/persistence/schema_v1.sql` + `_to_v2/_v3/_v4` | Canonical schema + forward migrations | `runs`, `step_checkpoints`, `schema_version` | Complete |
| `orchestrator/src/main.rs`, `cli/mod.rs` | **Unrelated** — multi-agent patch-bundle CLI (`boruna-orch`) | `cmd_plan/apply/review/...` | Out of slice |
| `crates/llmvm-cli/tests/cli_coordinator_mtls.rs` | mTLS handshake + identity tests | 5 tests | Green intent |
| `crates/llmvm-cli/tests/cli_coordinator_worker.rs` | Worker lifecycle, lease, bearer-auth tests | ~lease/sweep/secret tests | Green intent |

---

## 3. GAPS (functional / robustness)

- **G1 — `find_one_pending_step` is O(runs × steps) linear scan per claim poll**
  (`coordinator.rs:1499-1554`). Every claim (each worker, every 250 ms poll interval,
  `coordinator.rs:1379`) lists all Running runs and all their step checkpoints under the store mutex.
  At fleet scale this serializes all workers behind one scan + one global `Mutex`. **Severity: Medium**
  (scalability; the whole store is a single `Mutex<Connection>`, `coordinator.rs:60`).

- **G2 — Blob store `write()` does NOT verify `hash == SHA256(bytes)`** (`blob_store.rs:104-118`,
  documented "taken as-is and is NOT verified"). The coordinator persists a worker-supplied
  `output_json` under a worker-supplied `output_hash` with no server-side recomputation in
  `complete_step_cas` (`mod.rs:1665-1701` — passes `output_hash` straight through). Content-addressing
  integrity therefore depends entirely on an honest worker. **Severity: Medium** (see S6).

- **G3 — Concurrent cold-start migration race under HA.** `column_exists` is read *outside* the
  migration transaction (`mod.rs:655,671,687`), then the `ALTER TABLE` runs inside it. Two coords
  cold-starting against the same brand-new/old `runs.db` can both observe "column absent", one
  applies the `ALTER`, the second's `ALTER` fails ("duplicate column") → its `open()` errors.
  **Severity: Low** (narrow window; in practice `runs.db` is created + migrated by a prior local run
  before coords attach).

- **G4 — Worker failover is registration-time only (sticky coord).** After registration the worker
  pins one coord for its lifetime; a mid-session coord crash strands the worker until process restart
  (`worker.rs:216-318`, comment 217-223). Documented, but "HA active-active worker failover" is
  therefore **partial**, not continuous. **Severity: Low** (by design; k8s-liveness recovery model).

- **G5 — `handle_run_status` performs writes (advance tick + audit append) on a `GET`**
  (`coordinator.rs:1835-1921`). Any authenticated caller polling status drives run state forward and
  appends audit events. Semantically load-bearing (poll = wait driver) but violates GET-idempotency;
  an unaware monitoring probe mutates run state. **Severity: Low**.

- **G6 — `value_to_json` is lossy for composite outputs** (`worker.rs:437-449`): records/lists/maps
  fall through to `format!("{v:?}")` (Rust debug), not structured JSON. Distributed step outputs for
  non-scalar values are debug strings. **Severity: Low** (MVP limitation, self-noted line 438).

---

## 4. SECURITY

### 4.1 Endpoint authentication — `auth_middleware` (`coordinator.rs:737-774`)

- **S1 — Default-open; auth is opt-in. [CONFIRMED]** With neither `--shared-secret`/`BORUNA_COORD_SECRET`
  nor mTLS flags, `auth_middleware` is a pass-through (`coordinator.rs:759-772` — the `Some(expected)`
  and `mtls_required` guards are both skipped). Binding `--bind 0.0.0.0` in that state only prints a
  stderr WARNING (`coordinator.rs:236-246, 273-280`) — the listener still serves **all mutating routes
  unauthenticated**: submit, approve, trigger, claim, complete, fail, extend-lease. Operator must front
  with a reverse proxy. Correct-by-design but a sharp edge; the warning is not a control.

- **S2 — `mtls_required` is derived, not independent. [CONFIRMED / by-design]**
  `mtls_required = compiled_tls.is_some()` (`coordinator.rs:263`). mTLS enforcement exists only when TLS
  material is supplied; there is no "require auth" master switch that fails closed on a non-loopback bind.
  A misconfigured public deployment stays open.

- **S3 — mTLS client-cert verification is genuinely enforced (not bypassable). [SAFE]**
  `build_server_tls` builds a `WebPkiClientVerifier` and `with_client_cert_verifier`
  (`coordinator.rs:344-372`); the accept loop drops connections whose handshake fails
  (`serve_with_tls`, `coordinator.rs:445-456`). Tests confirm: no-cert handshake rejected, foreign-CA
  cert rejected (`tests/cli_coordinator_mtls.rs:7-14, 293-347`). Defense-in-depth: middleware also 401s
  if a request somehow reaches it without a `ClientIdentity` (`coordinator.rs:756-758`).

- **S4 — `/api/health` intentionally bypasses auth. [SAFE]** (`coordinator.rs:748-750`). Leaks only
  version, capability-set hash, uptime (`coordinator.rs:2126-2160`) — low sensitivity, documented.

- **S5 — Bearer secret compared in constant time. [SAFE]** `constant_time_bytes_eq`
  (`coordinator.rs:700-709`, used at `769`). Length-leak is documented + acceptable
  (`coordinator.rs:692-699`). Note: secret may be passed as a CLI flag `--shared-secret`
  (`crates/llmvm-cli/src/main.rs:359-360, 450-451`) → visible in `ps`/shell history; env var
  `BORUNA_COORD_SECRET` is the safer path. **[NEEDS-REVIEW — operational]**

### 4.2 Cross-worker authorization / claim ownership

- **S6 — Any authenticated worker can complete/fail/extend ANOTHER worker's in-flight step.
  [CONFIRMED — highest-value finding]**
  - `handle_complete/fail/extend_lease` gate only on `validate_session` — i.e. "is the *caller* a
    registered worker" (`coordinator.rs:1571-1573, 1599-1601, 1626-1628`; `validate_session`
    `coordinator.rs:1724-1743` checks `worker_id`+`session_token` of the **caller**, using the caller's
    own session).
  - The CAS then keys **only** on `(run_id, step_id, claim_id)` + `status=Running` — there is **no
    `worker_id` in the CAS** (`terminal_cas_inner` `mod.rs:1954, 1977`; `extend_lease_cas`
    `mod.rs:2079, 2088`).
  - `claim_id` is a **per-step monotonic counter starting at 1** (`claim_step` `mod.rs:1607`;
    "first claim returns one" test `mod.rs:3136-3142`) — highly predictable.
  - `run_id` is deterministically derived (`derive_run_id(workflow_hash, inputs_hash, counter)`,
    `mod.rs:2205`) and `step_id` comes from the workflow def — both enumerable by any submitter.
  - **Result:** worker A, using **its own valid session** (and its own valid client cert under mTLS),
    can submit `CompleteRequest{run_id, step_id, claim_id: 1}` for a step legitimately claimed by
    worker B and either commit a forged output or `fail`/sabotage it. mTLS raises the bar to "must be an
    authorized worker" but provides **no worker-to-worker isolation**. Combined with **S6/G2** (no
    server-side hash verification) an authorized worker can commit arbitrary output for another worker's
    step. **No test covers cross-worker claim ownership** (worker test suite covers lease expiry, sweep,
    bearer auth — `tests/cli_coordinator_worker.rs:408, 1043, 1337-1421` — none assert claim ownership).

- **S7 — mTLS CN is reconciled against `worker_id` ONLY at register, not on subsequent calls.
  [NEEDS-REVIEW]** `handle_register` checks cert-CN == body `worker_id` (`coordinator.rs:1265-1291`).
  `handle_claim/complete/fail/extend/heartbeat` do **not** take the `ClientIdentity` extension at all —
  they trust `session_token`. So the per-connection cert identity is not bound to the per-request
  `worker_id` after registration; the session token is the only guard, and (per S6) the token only
  proves caller-is-a-worker, not caller-owns-the-claim.

- **S8 — `session_token` transmitted in the URL query string on claim. [NEEDS-REVIEW]**
  `GET /api/work/claim?worker_id=...&session_token=...` (`worker.rs:377-384`; `ClaimQuery`
  `coordinator.rs:930-935`). Secrets in query strings are routinely captured by proxy/access logs and
  `Referer`. Should be a header. (Over mTLS the channel is encrypted, but intermediary/log exposure
  remains for the plaintext-bearer and reverse-proxy deployments.)

### 4.3 Approval-gate authorization

- **S9 — `/api/runs/{run_id}/approve` has no approver/operator role separation. [CONFIRMED]**
  `handle_approve_run` is gated by the *same* `auth_middleware` as worker routes
  (`coordinator.rs:814, 1941-1982`) and carries **no per-step secret** — decision is just
  `"approved"|"rejected"` in the body. Any principal holding the shared bearer **or any valid worker
  client cert** can approve/reject any human-in-the-loop gate on any run. There is no distinction
  between "worker" and "approver" identities. Contrast `/trigger`, which does require a per-step
  `token` second factor (`coordinator.rs:1025-1030, 1987-2013`). **Severity: High** for a platform whose
  value proposition is policy-gated, auditable approval workflows.

### 4.4 Blob path traversal & content addressing

- **S10 — Blob hash path traversal: hardened. [SAFE]** Hash validated as exactly 64 lowercase hex at
  the HTTP handler *before* any FS/store access (`coordinator.rs:2041-2053`) **and** again in every
  `BlobStore` method via `validate_hash` (`blob_store.rs:118-119,169,185,211,314-325`). `../`, slashes,
  uppercase, wrong length all rejected pre-disk (tests `blob_store.rs:377-427`). Shard derived from the
  first two already-validated hex chars (`blob_store.rs:96-100`). No traversal reachable.

- **S11 — Blob route is run-scoped. [SAFE]** `handle_get_blob` returns bytes only if
  `run_owns_blob_ref(run_id, hash)` (`coordinator.rs:2062-2079`; parameterized query
  `mod.rs:1708-1720`); 404 does not disambiguate run existence (`coordinator.rs:2067-2079`). Prevents the
  endpoint acting as a generic blob server.

- **S12 — Content-addressing integrity is caller-trusted. [NEEDS-REVIEW]** See G2/S6: the stored blob's
  hash is not verified against its bytes on write (`blob_store.rs:104-118`, `mod.rs:1683-1685`). A
  dishonest worker can store content whose `output_hash` (which feeds the audit chain,
  `schema_v3_to_v4.sql` REPLAY-VERIFIED note) does not match the bytes. Replay re-hash would later catch
  it, but the live audit record is corrupt until replay.

### 4.5 SQL injection & concurrency

- **S13 — No SQL injection anywhere in the slice. [SAFE]** Every query uses `params![...]` bound
  parameters (`mod.rs` throughout: `1584, 1608, 1713, 1927, 1966, 2017, 2058, 2085`, etc.). The only
  `format!`-built SQL is `column_exists` interpolating `PRAGMA table_info({table})` with **hardcoded
  string literals** (`"step_checkpoints"`), never user input (`mod.rs:2156`).

- **S14 — Claim/complete/fail concurrency is correct. [SAFE]** All mutators use
  `BEGIN IMMEDIATE` (acquire writer lock upfront) + SELECT-then-CAS-UPDATE under `with_busy_retry`
  (`claim_step` `mod.rs:1572-1639`, `terminal_cas_inner` `mod.rs:1922-2004`, `extend_lease_cas`
  `mod.rs:2052-2100`). Double-claim is prevented: `claim_step` only transitions `Pending→Running`
  (`mod.rs:1604-1607`); the coordinator's SELECT→`claim_step` race is handled by treating
  `NotClaimable`/`StepNotFound` as "retry" (`coordinator.rs:1450-1454`). `complete`/`fail`/`extend`
  reject `claim_id` mismatch or non-Running with `LeaseExpired` (`mod.rs:1954-1958, 2079-2083`) →
  idempotent/late-write-safe. Lease expiry sweep uses strict `<` (`mod.rs:2022-2024`) so it is HA-safe
  under concurrent coords (`coordinator.rs:213-233`).

- **S15 — Store mutex poisoning degrades safely but silently stalls the sweep. [NEEDS-REVIEW]** A handler
  panic while holding `store` poisons the `Mutex`; the background sweep then logs once and skips every
  subsequent tick forever (`coordinator.rs:662-677`) → expired leases stop being requeued (steps
  stranded) while `/api/health` starts returning 503 (`coordinator.rs:2142-2150`). Fails visible, not
  silent-wrong, but there is no auto-recovery short of coord restart.

---

## 5. COVERAGE

**Fully read (line-by-line):** `crates/llmvm-cli/src/coordinator.rs:1-2200` (server, TLS listener,
cert-CN parser, auth middleware, router, all 12 handlers, wire shapes, error mappers — remainder
2200-4136 is `coord.*`-taxonomy error mapping + `#[cfg(test)]`); `crates/llmvm-cli/src/worker.rs`
(full 1-722); `orchestrator/src/persistence/blob_store.rs` (full 1-628);
`orchestrator/src/persistence/mod.rs:560-720` (open/init/migration) and `1499-2100` (claim/CAS/expire/
extend/run_owns_blob_ref); all four `schema_v*.sql`; `orchestrator/src/main.rs` (full).
**Grep-verified (not full read):** parameterized-SQL sweep across `mod.rs`; coord serve/worker flag +
env wiring in `crates/llmvm-cli/src/main.rs:335-1799`; test intent in
`tests/cli_coordinator_mtls.rs` (fn names + asserts) and `tests/cli_coordinator_worker.rs` (lease/sweep/
bearer fn names + asserts).
**Not read (out of slice / not verified):** `orchestrator/src/cli/mod.rs` interior (patch-bundle CLI,
unrelated); `crates/llmvm-cli/src/dashboard.rs` interior (merged read-only routes — inherits coord auth,
not independently audited); `orchestrator/tests/{integration.rs,workflow_integration.rs,retry_timing.rs,
blob_integration_tests.rs}` interiors — **not verified** (time; behavior inferred from the CLI-crate
coordinator/worker tests instead). `mod.rs` regions 720-1560 and 2100-4608 skimmed via grep only.
```
```
