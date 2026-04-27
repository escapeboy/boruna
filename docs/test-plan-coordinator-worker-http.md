# Test plan — Coordinator/worker HTTP MVP (sprint 0.5-S2b)

Companion to design + architecture docs.

## Strategy

Three layers:

1. **Coordinator handler unit tests** — call handler functions
   directly with a synthetic `CoordinatorState` over an
   in-memory `RunCheckpointStore`. Asserts JSON shapes,
   error_kind strings, status codes.

2. **Worker unit tests** — test the parsing of work-item JSON
   responses and the claim/complete error flows. Mock the HTTP
   client at the response level.

3. **CLI integration tests** — spawn the binary, hit the
   running coordinator with a `reqwest`-based test client AND
   spawn an actual worker subprocess. Asserts end-to-end
   behavior including the kill-worker scenario.

## Coordinator handler unit tests

| # | Test | Expectation |
|---|---|---|
| 1 | `register_allocates_worker_id_when_absent` | server allocates a UUID-like id |
| 2 | `register_accepts_caller_supplied_worker_id` | echoes back the supplied id |
| 3 | `register_rejects_binary_mismatch` | 409, `error_kind: coord.binary_mismatch`, includes both hashes |
| 4 | `heartbeat_unknown_worker_returns_404` | `coord.unknown_worker` |
| 5 | `claim_returns_204_when_no_pending_work` | within poll timeout |
| 6 | `claim_returns_work_item_when_pending_step_exists` | shape per architecture doc |
| 7 | `claim_writes_lease_to_runs_db` | `worker_id`, `lease_expires_at_ms`, `claim_id` updated |
| 8 | `complete_committed_writes_terminal_state` | row → Completed |
| 9 | `complete_with_stale_claim_id_returns_409_lease_expired` | row state unchanged |
| 10 | `complete_step_not_found_returns_404` | `coord.step_not_found` |
| 11 | `fail_committed_writes_failed_state` | row → Failed |
| 12 | `fail_with_stale_claim_id_returns_409` | row state unchanged |
| 13 | `extend_lease_pushes_deadline` | new lease in row |
| 14 | `extend_lease_with_stale_claim_id_returns_409` | row state unchanged |
| 15 | `extend_lease_caps_at_max_lease_ttl_ms` | server-side cap enforced |
| 16 | `protocol_version_present_on_every_response` | regression suite |
| 17 | `output_too_large_returns_413_with_error_kind` | reject before parsing JSON |

## Worker unit tests

| # | Test | Expectation |
|---|---|---|
| 18 | `parse_work_item_json` | round-trip from coordinator's response shape |
| 19 | `worker_executes_pure_function` | `fn main() -> Int { 42 }` → output_json `"42"`, deterministic hash |
| 20 | `worker_handles_compile_error_via_fail` | bad source → `report_fail` with error message |
| 21 | `worker_handles_runtime_error_via_fail` | divide-by-zero or capability-denied → fail |

## CLI integration tests

`crates/llmvm-cli/tests/cli_coordinator_worker.rs`. Each test:
- Spins up a tempdir as `data-dir`.
- Pre-populates `runs.db` with one or more Pending steps using
  `RunCheckpointStore` directly.
- Spawns the coordinator on `127.0.0.1:0` (kernel-assigned port).
- Either spawns a worker subprocess OR uses an in-process
  `reqwest` client to exercise specific routes.
- Asserts end-to-end behavior.
- Cleanup.

| # | Test | Expectation |
|---|---|---|
| 22 | `cli_register_via_curl_equivalent` | `reqwest` POST → 200 with worker_id + session_token |
| 23 | `cli_claim_returns_204_on_empty_db` | long-poll completes, returns 204 |
| 24 | `cli_complete_with_stale_claim_returns_409` | error_kind in response body |
| 25 | `cli_post_oversize_body_returns_413` | 9 MB body → `coord.output_too_large` |
| 26 | `cli_worker_subprocess_completes_step_end_to_end` | spawn coord + worker, pre-populate one Pending step, wait for the worker to claim+execute+complete, assert the row's `output_json`/`output_hash` match expected values |
| 27 | `cli_worker_kill_mid_step_lease_expires_then_reclaim` | spawn coord + worker A; worker A claims a step that takes ≥ lease TTL; kill worker A; expire_leases_and_requeue runs; spawn worker B; B reclaims and completes; assert row's output_json is from B |

Test #26 is the **MVP smoke test**. Test #27 is the **flagship
regression** — if it ever fails, the lease-expiry-and-re-dispatch
guarantee is broken at the wire level.

For test #27 to be deterministic, the `.ax` source for the
"slow" step uses a wall-clock-keyed busy-wait that we can
control via test fixture timing. The lease TTL is set very low
(e.g., 500ms) so the test runs quickly.

Worker A's step source: `fn main() -> Int { busy_wait(2000); 1 }`
(or similar) — but Boruna doesn't have `busy_wait`. Alternative:
spawn worker A with `--lease-ttl-ms 200` and have worker B's
fixture compute a different output to make the assertion
distinguishable. Worker A holds the claim while we send SIGKILL;
A never reports; lease expires after 200ms; sweep runs; B claims
and completes with its (deterministic) output.

For the test to be deterministic and not time-flaky:
- After spawning worker A, wait for the row to flip to
  `status=Running` (poll runs.db).
- Send SIGKILL to worker A.
- Drive the coordinator's expire_leases sweep manually (e.g.,
  `cargo test` test-only HTTP route or wait the lease TTL).
- Spawn worker B; it claims and completes.
- Assert row's `worker_id == "B"`, `output_hash` matches B's
  source.

## Adversarial review focus areas

When `ce-correctness-reviewer` and `ce-security-reviewer` run:

1. **Mutation surface** — the only mutating routes are POST
   under `/api/work/*` and `/api/workers/*`. Confirm no
   accidental catch-all routes; non-GET on dashboard's read
   routes still returns 405.

2. **Long-poll graceful shutdown** — when the coordinator
   process gets SIGINT, in-flight long-polls should close
   cleanly (return 503 or close the connection). No orphan
   tokio tasks.

3. **Worker session-token validation** — every mutating route
   checks the worker's session token against the registry. A
   stale token (e.g., from before coordinator restart) gets
   `coord.unknown_worker`.

4. **Output payload size enforcement** — Axum's
   `DefaultBodyLimit::max(8 MiB)` returns 413 BEFORE the JSON
   parser runs. Crucially, this prevents memory exhaustion on
   a malicious worker sending a 1 GB body.

5. **Bind security** — same default as dashboard
   (`127.0.0.1`); `--bind 0.0.0.0` warning. The coordinator's
   no-auth posture is even more security-relevant than the
   dashboard's because mutations are possible.

6. **`capability_set_hash` enforcement** — workers with
   mismatched hashes are rejected at registration. Confirm a
   worker can't bypass this by skipping registration and going
   directly to `/api/work/claim` (claim should reject unknown
   `worker_id` with `coord.unknown_worker`).

7. **`protocol_version` regression suite** — every success and
   failure path on every route carries `protocol_version: 1`.
   New regression tests in the existing
   `protocol_version_tests` module pattern.

8. **Lease-extension cap** — `extend_lease_cas` doesn't cap on
   the persistence layer; the HTTP layer enforces `extend_by_ms
   <= max_lease_ttl_ms`. Test that a worker requesting longer
   gets the cap.

## Out of scope for this sprint's tests

- Multi-coordinator failover (HA is 0.6.x+).
- Worker capability tagging (0.5-S4+).
- Auth (deferred).
- Performance / load testing.
- TLS.
- Rolling upgrades.
- The wave-loop runner integration (0.5-S2c sprint will own
  end-to-end DAG tests).

## CI integration

The existing `--features serve` build/test/clippy steps from
0.4-S16 cover this sprint's code. No CI workflow change needed.
