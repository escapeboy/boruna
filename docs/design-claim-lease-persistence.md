# Design — Claim/lease persistence API (sprint 0.5-S2a)

## Premise

ADR 002 commits to a lease-based claim model with a monotonic
`claim_id` that the completion handler CASes against. The
correctness of the entire distributed-execution surface lives in
this state machine: if `claim_step` and `complete_step_cas` get
the atomicity wrong, the slow-but-not-dead worker race the ADR
calls out becomes a real double-completion bug.

This sprint locks the **persistence-layer half** of that contract:
schema columns, `RunCheckpointStore` API, and a comprehensive
test suite that exercises every documented race window.
**No HTTP code lands in this sprint.** The coordinator and
worker subcommands ship in 0.5-S2b on top of this stable base.

This is the same split that worked for ADR 001 → 0.3-S2a/b: the
persistence module gets its own sprint with focused tests, then
the integration sprint wires it into the runner.

## Who needs this

The next-sprint implementer of the coordinator HTTP layer (me,
or whoever picks up 0.5-S2b). They need:

- Atomic `claim_step` so the route handler can call it from any
  request without an ad-hoc transaction.
- Atomic `complete_step_cas` and `fail_step_cas` so the
  completion handlers reject late writes from expired-lease
  workers without inventing a second consistency surface.
- A sweep API (`expire_leases_and_requeue`) the coordinator can
  call on a timer or on coordinator startup.

Operators don't see this sprint directly — it's persistence
infrastructure. They benefit when 0.5-S2b ships.

## Narrowest MVP

**Five methods on `RunCheckpointStore`, three new columns,
no API changes to existing callers.**

```rust
impl RunCheckpointStore {
    /// Atomically claim the named step on behalf of `worker_id`.
    /// Caller must already know the step is the next one ready
    /// to run (the wave-loop scheduler decides this).
    pub fn claim_step(
        &self,
        run_id: &str,
        step_id: &str,
        worker_id: &str,
        lease_expires_at_ms: i64,
        now_ms: i64,
    ) -> Result<ClaimOutcome, PersistenceError>;

    /// CAS-protected completion. Atomic on (run_id, step_id, claim_id).
    pub fn complete_step_cas(
        &self,
        run_id: &str,
        step_id: &str,
        claim_id: u64,
        output_json: &str,
        output_hash: &str,
        attempt_count: u32,
        ended_at_ms: i64,
    ) -> Result<TerminalOutcome, PersistenceError>;

    /// CAS-protected failure. Atomic on (run_id, step_id, claim_id).
    pub fn fail_step_cas(
        &self,
        run_id: &str,
        step_id: &str,
        claim_id: u64,
        error_msg: &str,
        attempt_count: u32,
        ended_at_ms: i64,
    ) -> Result<TerminalOutcome, PersistenceError>;

    /// Sweep step_checkpoints for expired leases, transition them
    /// back to Pending so the next claim_step succeeds. Returns
    /// the count of expired-and-requeued leases.
    pub fn expire_leases_and_requeue(
        &self,
        now_ms: i64,
    ) -> Result<usize, PersistenceError>;

    /// Push out an existing claim's lease. CAS-protected against
    /// the original `claim_id` so an expired-and-reclaimed step
    /// rejects extension attempts from the original worker.
    pub fn extend_lease_cas(
        &self,
        run_id: &str,
        step_id: &str,
        claim_id: u64,
        new_lease_expires_at_ms: i64,
    ) -> Result<ExtendOutcome, PersistenceError>;
}
```

Outcome enums distinguish the three orthogonal failure modes per
project convention #1 (reject at parse / operation time, don't
silently no-op):

```rust
pub enum ClaimOutcome {
    /// Step claimed; carry this `claim_id` to the completion call.
    Claimed { claim_id: u64 },
    /// Step is not in `Pending` status (already running, completed,
    /// failed, or in a pause state). Caller should pick another
    /// step.
    NotClaimable { current_status: StepStatus },
    /// The (run_id, step_id) row doesn't exist. Caller bug.
    StepNotFound,
}

pub enum TerminalOutcome {
    /// Status transition committed.
    Committed,
    /// CAS failed because `claim_id` does not match the row's
    /// current `claim_id`. Carries the row's current state for
    /// observability.
    LeaseExpired {
        current_claim_id: u64,
        current_status: StepStatus,
    },
    /// The (run_id, step_id) row doesn't exist.
    StepNotFound,
}

pub enum ExtendOutcome {
    Extended { new_lease_expires_at_ms: i64 },
    LeaseExpired {
        current_claim_id: u64,
        current_status: StepStatus,
    },
    StepNotFound,
}
```

## What would make someone say "whoa"

- **`current_claim_id` in `LeaseExpired`** lets the coordinator's
  HTTP handler return a precise error shape: "you claimed at
  claim_id=3, the row is now at claim_id=5." Operators
  triaging a partition can correlate the worker's logs against
  the dashboard's lease history.
- **One transaction per call.** Every API method uses
  `BEGIN IMMEDIATE` + `with_busy_retry` (the existing pattern
  from project convention #13). No ad-hoc transactions in
  callers; the persistence layer is the contract.
- **Existing callers untouched.** `upsert_step_checkpoint` keeps
  working unchanged. The single-process runner doesn't need to
  use claim/lease — it owns the dispatch lifecycle directly.
  Distributed mode goes through the new methods exclusively.

## How this compounds

- Once `claim_id` is the CAS key, every future state-machine
  evolution (capability tagging, blob refs, priority queues)
  becomes additive — the lease guarantees stay correct.
- Future ADR sprints proposing alternative schedulers (priority
  queue, capability-aware dispatch) inherit the same
  state-machine guarantees by composing these methods.
- The dashboard (0.4-S16) auto-picks up the new columns via the
  `Serialize` derives on `StepCheckpoint` — no dashboard code
  change needed for "show which worker holds this lease."

## Scope (what this sprint changes)

- New: `orchestrator/src/persistence/schema_v2_to_v3.sql`
  migration adds three columns to `step_checkpoints`:
  - `worker_id TEXT` — operational only, nullable
  - `lease_expires_at INTEGER` — operational only, nullable, unix ms
  - `claim_id INTEGER NOT NULL DEFAULT 0` — operational only,
    monotonic per (run_id, step_id)
- New: `claim_step`, `complete_step_cas`, `fail_step_cas`,
  `expire_leases_and_requeue`, `extend_lease_cas` on
  `RunCheckpointStore`.
- New: `ClaimOutcome`, `TerminalOutcome`, `ExtendOutcome`
  result enums.
- New: extends `StepCheckpoint` struct with three additional
  fields (`worker_id: Option<String>`, `lease_expires_at_ms:
  Option<i64>`, `claim_id: u64`). Existing callers that
  construct `StepCheckpoint` directly use struct-update syntax;
  defaults are `None`/`0`.
- Wiring: bump `SCHEMA_VERSION` from `2` to `3`. `init()` adds
  a `if v < 3` block that runs the v2→v3 migration; the fresh-DB
  detection guard mirrors the v1→v2 pattern (check whether
  `claim_id` column exists already from `SCHEMA_V1_SQL`).
- `SCHEMA_V1_SQL` itself learns the three new columns so a fresh
  database arrives at v3 directly without going through the
  ALTER chain — same fresh-vs-existing pattern as v2.
- Tests: race-condition coverage. Every documented outcome of
  every new method has at least one test. The
  slow-but-not-dead worker race specifically gets a regression
  test that interleaves `claim_step` → `expire_leases_and_requeue`
  → `claim_step` (new claim_id) → `complete_step_cas` (with the
  OLD claim_id) and asserts `LeaseExpired` with the new claim_id.

## Non-goals (deferred)

- **No HTTP code.** The coordinator and worker subcommands ship
  in 0.5-S2b. This sprint produces the API; the next sprint
  consumes it.
- **No coordinator-startup lease cleanup.** The ADR commits to
  "scan runs.db for Running rows and re-enqueue as Pending on
  startup," but that's a coordinator-side concern. This sprint's
  `expire_leases_and_requeue` covers the timer-based sweep; the
  startup sweep is a thin wrapper the coordinator will add.
- **No worker tagging / capability metadata.** Workers in this
  sprint are opaque strings. Capability-aware claim is 0.5-S4+.
- **No retry-policy interaction.** `attempt_count` is a
  caller-supplied parameter to `complete_step_cas` and
  `fail_step_cas`. Retry-policy decisions stay in the runner.
- **No blob-output handling.** `output_json` is still a
  `&str` parameter. The 8 MB cap from ADR 002 is an HTTP-layer
  concern for 0.5-S2b.
- **No lease-extension auth.** `extend_lease_cas` accepts the
  caller's claim_id as the only authentication. Auth is
  deferred to the auth sprint.

## Stable surface

Locked at this sprint's ship:

- The three new column names (`worker_id`, `lease_expires_at`,
  `claim_id`).
- The five method names and their parameter shapes.
- The three outcome enum names and their variants.
- The semantic meaning of `claim_id == 0`: "never claimed."
  `claim_step` always allocates `claim_id >= 1`.

Future additions are additive (new columns, new outcome
variants — convention #2). Renaming or removing is breaking.

## Open question for next sprint

- Should `claim_step` be combined with "find next claimable
  step in this run/wave" into a single `claim_next_step(run_id)`
  method? Today the runner picks the step; for distributed mode
  the coordinator picks. Either pattern works on top of
  `claim_step(run_id, step_id, ...)` — the smaller method
  composes more flexibly. Defer the convenience wrapper to
  0.5-S2b when the coordinator's exact dispatch shape is clear.

## Stability tier

Per `docs/stability.md`: **stable**.
- Method signatures, outcome enums, schema columns are locked.
- Future additions (e.g., `claim_step_with_capability`) are
  additive; new columns get `#[serde(default)]` per convention
  #11 so old binaries reading new schemas don't break.
