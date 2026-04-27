# Test plan — Claim/lease persistence (sprint 0.5-S2a)

Companion to design + architecture docs.

## Strategy

All tests run against `RunCheckpointStore::open_in_memory()` —
fast, parallel-safe, no filesystem. The test cases focus on the
state machine and race conditions, NOT on filesystem-durability
properties (those are covered by the existing 0.3-S2a/b tests).

## Schema migration

| # | Test | Setup | Expectation |
|---|---|---|---|
| 1 | `schema_version_is_3_after_init` | open in-memory store | `SELECT version FROM schema_version` returns 3 |
| 2 | `fresh_db_has_claim_columns` | open in-memory store | `column_exists("step_checkpoints", "claim_id")` true; same for `worker_id` and `lease_expires_at` |
| 3 | `claim_id_defaults_to_zero_for_existing_rows` | open store, insert step via `upsert_step_checkpoint` (no claim), read back | `claim_id == 0`, `worker_id` and `lease_expires_at` are `None` |
| 4 | `migration_v2_to_v3_idempotent_on_reopen` | open + close + reopen | second open succeeds, version still 3 |

## `claim_step` happy path

| # | Test | Expectation |
|---|---|---|
| 5 | `claim_step_first_claim_returns_one` | row has status=Pending, claim_step → `Claimed { claim_id: 1 }` |
| 6 | `claim_step_writes_worker_and_lease` | after claim_step, the row has worker_id, lease_expires_at, status=Running, claim_id=1 |
| 7 | `claim_step_preserves_started_at_on_reclaim` | claim, expire, claim again — started_at remains the first-attempt's value |
| 8 | `claim_step_increments_claim_id_on_reclaim` | after expire+reclaim, claim_id=2 |

## `claim_step` rejection paths

| # | Test | Expectation |
|---|---|---|
| 9 | `claim_step_step_not_found` | unknown (run_id, step_id) → `StepNotFound` |
| 10 | `claim_step_already_running` | row in status=Running → `NotClaimable { current_status: Running }` |
| 11 | `claim_step_already_completed` | row in status=Completed → `NotClaimable { current_status: Completed }` |
| 12 | `claim_step_awaiting_approval` | row in status=AwaitingApproval → `NotClaimable` |

## `complete_step_cas` happy path

| # | Test | Expectation |
|---|---|---|
| 13 | `complete_step_cas_committed` | claim → complete with matching claim_id → `Committed`. Row status=Completed, output_json/hash set, worker_id/lease_expires_at cleared |
| 14 | `complete_step_cas_clears_error_msg` | failed step's error_msg from prior attempt is cleared on successful completion |

## `complete_step_cas` rejection paths (the load-bearing tests)

| # | Test | Expectation |
|---|---|---|
| 15 | `complete_step_cas_lease_expired_after_requeue` | claim (claim_id=1) → expire+requeue → reclaim (claim_id=2) → original worker calls complete with claim_id=1 → `LeaseExpired { current_claim_id: 2, current_status: Running }`. **Row state UNCHANGED** by the rejected call. |
| 16 | `complete_step_cas_step_not_found` | unknown (run_id, step_id) → `StepNotFound` |
| 17 | `complete_step_cas_after_step_already_completed` | claim → complete (succeeds) → second complete with same claim_id → `LeaseExpired` (status=Completed, claim_id=1). Row UNCHANGED. |
| 18 | `complete_step_cas_zero_claim_id_rejected` | step never claimed → caller bug-tests with claim_id=0 → `LeaseExpired { current_claim_id: 0, current_status: Pending }` |

## `fail_step_cas`

| # | Test | Expectation |
|---|---|---|
| 19 | `fail_step_cas_committed` | claim → fail → status=Failed, error_msg set, worker_id/lease_expires_at cleared |
| 20 | `fail_step_cas_lease_expired` | claim → expire+requeue → reclaim → fail with old claim_id → `LeaseExpired`, row unchanged |

## `expire_leases_and_requeue`

| # | Test | Expectation |
|---|---|---|
| 21 | `expire_leases_zero_when_no_expired` | no leases past now_ms → returns 0 |
| 22 | `expire_leases_finds_only_running_with_expired_lease` | mix of pending, completed, running-with-fresh-lease, running-with-expired-lease — only the last is touched |
| 23 | `expire_leases_clears_worker_and_lease` | after sweep, the requeued row has status=Pending, worker_id=None, lease_expires_at=None, claim_id=unchanged |
| 24 | `expire_leases_idempotent` | second sweep on same data returns 0 |
| 25 | `expire_leases_does_not_touch_pending_rows` | rows in Pending without leases are untouched (their lease_expires_at is NULL) |

## `extend_lease_cas`

| # | Test | Expectation |
|---|---|---|
| 26 | `extend_lease_cas_extended` | claim → extend with matching claim_id → `Extended`, lease_expires_at updated |
| 27 | `extend_lease_cas_lease_expired_after_requeue` | claim → expire+requeue → reclaim (new claim_id) → original worker extends with old claim_id → `LeaseExpired { current_claim_id: 2 }` |
| 28 | `extend_lease_cas_step_not_found` | unknown row → `StepNotFound` |
| 29 | `extend_lease_cas_status_pending_rejected` | row was requeued → caller tries to extend → `LeaseExpired { current_status: Pending }` |

## End-to-end race regression

The flagship test from the ADR's adversarial review (the
slow-but-not-dead worker race):

| # | Test | Sequence |
|---|---|---|
| 30 | `slow_worker_race_late_completion_rejected` | (1) Insert step Pending. (2) `claim_step(worker=A)` → claim_id=1. (3) Time passes; lease expires. (4) `expire_leases_and_requeue(now_after_lease)` returns 1, row → Pending. (5) `claim_step(worker=B)` → claim_id=2. (6) Worker A's POST arrives: `complete_step_cas(claim_id=1, output="A's output")` → `LeaseExpired { current_claim_id: 2, current_status: Running }`. (7) Verify row's output_json is still NULL (worker A's output rejected). (8) Worker B completes: `complete_step_cas(claim_id=2, output="B's output")` → `Committed`. (9) Verify row's output_json = "B's output". |

This single test is the load-bearing assertion of the entire
sprint. If it ever fails, the state machine is broken.

## Concurrency stress (best-effort)

These are best-effort tests using `std::thread::spawn` against an
in-memory store. `BEGIN IMMEDIATE` should serialize concurrent
writers; the test asserts that exactly one of N concurrent
`claim_step` calls on the same row succeeds.

| # | Test | Expectation |
|---|---|---|
| 31 | `concurrent_claim_step_exactly_one_wins` | spawn 8 threads, each calls `claim_step` on the same row — exactly 1 returns `Claimed`, 7 return `NotClaimable { current_status: Running }` |
| 32 | `concurrent_complete_with_same_claim_id_exactly_one_wins` | spawn 8 threads calling `complete_step_cas` with the same claim_id — exactly 1 returns `Committed`, 7 return `LeaseExpired` |

These tests are skipped if SQLite WAL serialization makes them
flaky on the test runner; the core CAS guarantee is already
covered by the deterministic happy-path + rejection tests.

## Non-tests

- **No filesystem durability tests.** The 0.3-S2a tests already
  cover WAL/sync semantics; this sprint adds columns, not
  durability behavior.
- **No HTTP tests.** No HTTP code in this sprint.
- **No replay-engine integration.** `worker_id` /
  `lease_expires_at` / `claim_id` are operational only and never
  enter the audit hash chain. The existing replay tests are
  unaffected; no new replay tests needed.
- **No dashboard-rendering tests.** The dashboard's
  `Serialize`-based JSON output picks up the new fields
  automatically. A future dashboard test may want to assert the
  new fields' presence; defer to 0.5-S2b or a docs sprint.

## Adversarial review focus areas

When `ce-correctness-reviewer` and `ce-data-integrity-guardian`
run on the implementation:

1. **CAS atomicity** — does every state-transitioning method
   wrap in `BEGIN IMMEDIATE`? Any path that mutates without it
   is a footgun.
2. **`claim_id` overflow** — `INTEGER` in SQLite is i64. At a
   billion claims per second a step's claim_id overflows in
   ~292 years. Document the bound or pick `u64` with explicit
   wrap-detection. (We pick i64 storage, u64 in Rust;
   wraparound is theoretical.)
3. **`upsert_step_checkpoint` interaction with claim_id** — does
   the existing upsert path silently clobber claim_id? It
   doesn't write claim_id, so it's preserved by SQLite's
   not-mentioned-column-is-untouched rule. Verify.
4. **Sweep races** — what if `expire_leases_and_requeue` runs
   concurrently with `extend_lease_cas`? `BEGIN IMMEDIATE` on
   both serializes the writes; the test should confirm that
   the row ends in one of two consistent states (extended or
   requeued, not split).
5. **Outcome enum exhaustiveness** — is there a real-world race
   where the outcome doesn't fit any documented variant?
   Specifically: if the runs row is deleted between the
   coordinator's claim and the worker's complete, is that
   `StepNotFound`? (The `ON DELETE CASCADE` from runs to
   step_checkpoints means yes; verify.)
