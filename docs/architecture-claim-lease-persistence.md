# Architecture — Claim/lease persistence (sprint 0.5-S2a)

Companion to `docs/design-claim-lease-persistence.md`. Covers
*how*, not *what* — SQL shape, transaction boundaries, the
`StepCheckpoint` struct evolution, and the migration mechanics.

## Schema v2 → v3 migration

New file: `orchestrator/src/persistence/schema_v2_to_v3.sql`.

```sql
-- v2 → v3 migration (sprint 0.5-S2a): adds the lease/claim columns
-- to step_checkpoints. These columns power the distributed-execution
-- claim CAS state machine from ADR 002.
--
-- All three columns are OPERATIONAL ONLY — never feed audit hashes,
-- never order replay-relevant queries.
--
-- ALTER TABLE ADD COLUMN with a constant DEFAULT is fast in SQLite
-- (no table rewrite). worker_id and lease_expires_at default to NULL
-- which the application code reads as "no current lease". claim_id
-- defaults to 0 which the application reads as "never claimed".

ALTER TABLE step_checkpoints ADD COLUMN worker_id         TEXT;
ALTER TABLE step_checkpoints ADD COLUMN lease_expires_at  INTEGER;
ALTER TABLE step_checkpoints ADD COLUMN claim_id          INTEGER NOT NULL DEFAULT 0;
```

The same three columns get baked into `schema_v1.sql` so a fresh
DB arrives at v3 directly. The migration runner's existing
fresh-DB-detection guard checks for column presence; we extend
that to also check for `claim_id`.

In `mod.rs`:

```rust
pub const SCHEMA_VERSION: i64 = 3;

const SCHEMA_V2_TO_V3_SQL: &str = include_str!("schema_v2_to_v3.sql");

// In init() after the v1→v2 block:
if on_disk < 3 {
    let has_claim_id = column_exists(&conn, "step_checkpoints", "claim_id")?;
    let tx = conn.unchecked_transaction()?;
    if !has_claim_id {
        tx.execute_batch(SCHEMA_V2_TO_V3_SQL)?;
    }
    tx.execute(
        "UPDATE schema_version SET version = ?1 WHERE id = 1",
        params![3_i64],
    )?;
    tx.commit()?;
}
```

## `StepCheckpoint` struct evolution

```rust
pub struct StepCheckpoint {
    // ... existing fields ...
    pub attempt_count: u32,

    /// Operational only. Set when a worker holds the lease;
    /// `None` when the step is not currently claimed.
    pub worker_id: Option<String>,

    /// Operational only. Unix epoch ms when the current lease
    /// expires; `None` when no lease is held.
    pub lease_expires_at_ms: Option<i64>,

    /// Operational only. Monotonic counter of how many times
    /// this step has been claimed. `0` = never claimed.
    /// Increments by 1 on every successful `claim_step`.
    /// CAS key for `complete_step_cas` / `fail_step_cas` /
    /// `extend_lease_cas`.
    pub claim_id: u64,
}
```

The struct's `Serialize` derive (added in 0.4-S16) automatically
exposes the new fields in dashboard JSON responses. Old binaries
reading newer step_checkpoints rows see the columns as ignored
extras (the orchestrator-internal struct deserialization isn't
used cross-version — workers and coordinator must run matching
binaries per ADR 002's atomic-upgrade rule).

`upsert_step_checkpoint` is unchanged. It does NOT write the new
columns — they're managed only by the new claim/lease methods.

## SQL for each method

### `claim_step`

```sql
-- BEGIN IMMEDIATE
SELECT status, claim_id FROM step_checkpoints
 WHERE run_id = ?1 AND step_id = ?2;

-- If no row: return StepNotFound.
-- If status != 'pending': return NotClaimable { current_status }.
-- Otherwise:

UPDATE step_checkpoints
   SET status            = 'running',
       worker_id         = ?3,
       lease_expires_at  = ?4,
       claim_id          = claim_id + 1,
       started_at        = COALESCE(started_at, ?5)
 WHERE run_id = ?1 AND step_id = ?2;

-- Return Claimed { claim_id: <new_value> }
-- COMMIT
```

Note the `started_at = COALESCE(started_at, ?5)` — preserves the
first-attempt's started_at across reclaims, matching the existing
runner's semantics where `started_at` is operational and
first-write-wins.

### `complete_step_cas`

```sql
-- BEGIN IMMEDIATE
SELECT status, claim_id FROM step_checkpoints
 WHERE run_id = ?1 AND step_id = ?2;

-- If no row: return StepNotFound.
-- If claim_id != ?3 OR status != 'running':
--   return LeaseExpired { current_claim_id, current_status }.
-- Otherwise:

UPDATE step_checkpoints
   SET status            = 'completed',
       output_json       = ?4,
       output_hash       = ?5,
       attempt_count     = ?6,
       ended_at          = ?7,
       error_msg         = NULL,
       worker_id         = NULL,
       lease_expires_at  = NULL
 WHERE run_id = ?1 AND step_id = ?2 AND claim_id = ?3;

-- Verify rowcount == 1; otherwise return LeaseExpired
-- (defensive — protects against a concurrent transaction
-- that mutated the row between the SELECT and UPDATE; in
-- BEGIN IMMEDIATE this shouldn't happen, but the WHERE clause
-- on claim_id is the structural guard).
-- Return Committed
-- COMMIT
```

`fail_step_cas` is structurally identical with status='failed'
and error_msg=?4 instead of output_json/output_hash. On terminal
failure we also clear `worker_id` and `lease_expires_at` —
nobody owns the row anymore.

### `expire_leases_and_requeue`

```sql
-- BEGIN IMMEDIATE
UPDATE step_checkpoints
   SET status            = 'pending',
       worker_id         = NULL,
       lease_expires_at  = NULL
       -- claim_id stays at its current value; the next
       -- claim_step bumps it.
 WHERE status = 'running'
   AND lease_expires_at IS NOT NULL
   AND lease_expires_at < ?1;

-- Return rowcount as usize
-- COMMIT
```

This is a single bulk UPDATE protected by `BEGIN IMMEDIATE`. The
sweep is idempotent — running it twice on the same expired set
just selects zero rows the second time.

### `extend_lease_cas`

```sql
-- BEGIN IMMEDIATE
SELECT status, claim_id, lease_expires_at FROM step_checkpoints
 WHERE run_id = ?1 AND step_id = ?2;

-- If no row: StepNotFound.
-- If claim_id != ?3 OR status != 'running':
--   LeaseExpired { current_claim_id, current_status }.
-- Otherwise:

UPDATE step_checkpoints
   SET lease_expires_at = ?4
 WHERE run_id = ?1 AND step_id = ?2 AND claim_id = ?3;

-- Return Extended { new_lease_expires_at_ms: ?4 }
-- COMMIT
```

## Transaction boundaries

All five methods wrap their SQL in `BEGIN IMMEDIATE` via the
existing `with_busy_retry` helper (project conventions #11, #13).
This:

- Acquires the writer lock immediately, blocking other writers
  until commit.
- Lets the SELECT-then-UPDATE pattern be atomic — no
  read-then-write race within the same call.
- Composes with SQLite's `busy_timeout = 5000` PRAGMA so
  concurrent readers don't fail; only conflicting writers
  back off.

The `BEGIN IMMEDIATE` + claim_id WHERE clause is the
defense-in-depth pattern. Even if some future caller bypasses
the BEGIN IMMEDIATE wrapper, the structural CAS on `claim_id`
keeps the state machine correct.

## Outcome enums — Display + error_kind

Each outcome enum implements `Display` and a stable `kind() ->
&'static str` returning a project-convention-#2 string. These
strings are inputs to the future HTTP error_kind taxonomy that
0.5-S2b will lock at the wire level:

| Outcome | `kind()` | Future HTTP error_kind |
|---|---|---|
| `ClaimOutcome::NotClaimable` | `claim.not_claimable` | (not exposed) |
| `ClaimOutcome::StepNotFound` | `claim.step_not_found` | (not exposed) |
| `TerminalOutcome::LeaseExpired` | `terminal.lease_expired` | `coord.lease_expired` |
| `TerminalOutcome::StepNotFound` | `terminal.step_not_found` | `coord.step_not_found` |
| `ExtendOutcome::LeaseExpired` | `extend.lease_expired` | `coord.lease_expired` |
| `ExtendOutcome::StepNotFound` | `extend.step_not_found` | `coord.step_not_found` |

The persistence-layer kinds are stable but **not on the public
HTTP wire** until 0.5-S2b maps them. Locking them now keeps the
mapping mechanical.

## File diff summary (estimated)

| File | Change |
|---|---|
| `orchestrator/src/persistence/schema_v2_to_v3.sql` | NEW |
| `orchestrator/src/persistence/schema_v1.sql` | append three new columns to step_checkpoints |
| `orchestrator/src/persistence/mod.rs` | bump `SCHEMA_VERSION` to 3, new migration block, new methods, new outcome enums, extend `StepCheckpoint` struct (~400 lines) |
| `CHANGELOG.md` | `[Unreleased]` entry under `### Added` |
