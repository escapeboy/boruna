# Design — 0.3-S3: Production Hardening

**Status:** 2026-04-25
**Predecessor:** `0.3-S2c` shipped the approval-gate operator UX. Two known gaps deferred from prior reviews remain:
1. `DataStore::store_output` writes via non-atomic `std::fs::write` — concurrent resumes can read torn JSON files (H4 from 0.3-S2c review).
2. Operators have no first-class CLI to inspect the full state of a single run — they query SQLite directly today.

## Scope

**In scope:**

1. **Atomic write in `DataStore::store_output`** — replace `std::fs::write` with `tempfile::NamedTempFile::persist`, eliminating torn-read risk for concurrent readers (or for a writer that crashes mid-write).
2. **`boruna workflow show <run-id> [--json]`** — operator inspection of a single run's full state: row, step checkpoints, approval sentinels, computed totals.
3. Per project-conventions §1: typed errors for the show path (run-not-found surfaces clearly).

**Deferred (NOT this sprint):**

- **Audit-log integration of approval decisions.** Sentinels are operational-only by design (project-conventions §15); whether and how to fold them into the hash chain is a deeper contract decision that needs its own ADR.
- **`list_runs` performance.** No real-world complaint yet. YAGNI until a dataset > 100k runs surfaces.
- **Cross-process flock on `data_dir`.** Single-writer-by-process is the documented assumption (ADR 001). Atomic-rename gives defense-in-depth without changing the deployment model.

## Forcing questions (Think)

**Who needs this?**
Operators running production workflows. Atomic-rename closes a real corruption window — even single-process resume can torn-write if the OS pages aren't flushed before a crash. `workflow show` replaces ad-hoc `sqlite3 runs.db 'SELECT …'` queries.

**Narrowest MVP?**
Atomic-rename alone is the must-have — it's a correctness fix. `workflow show` is a UX nice-to-have but high-leverage.

**What would make someone say "whoa"?**
`boruna workflow show <run-id> --json` returns a single structured document operators can pipe to `jq`, including the approval audit trail with timestamps. It's the inverse of `workflow run`: where run produces state, show consumes it.

**How does it compound?**
Closes the persistence story for 0.3.0. After this, async/parallel step execution (0.3-S4) lands on a non-racy foundation.

## Key invariants (must not regress)

1. **Determinism**: atomic-rename does not change `output_hash` (same bytes written, just safer). `workflow show --json` is reproducible — same run state → same JSON output.
2. **Replay-verified vs operational**: `workflow show` clearly labels operational-only fields (`started_at_ms`, sentinels' `decided_at_ms`).
3. **No silent footguns**: `workflow show <bad-id>` returns a typed `RunNotFound` (project-conventions §1); doesn't print empty.

## Open questions (resolved in Plan)

- **Q1: Atomic-rename across filesystems.** `NamedTempFile::persist` works only on the same filesystem. We always write to `data_dir/runs/<run_id>/outputs/<step>/result.json`; the temp file goes in the same parent directory. Same-FS guaranteed. **Decision: use `tempfile::NamedTempFile::new_in(parent_dir).persist(target)`.**
- **Q2: `workflow show` output shape (plain mode).** Stable, parsable-but-human. Sections: `=== Run ===`, `=== Steps ===`, `=== Approvals ===`. Each section is a fixed-width table. Mirror the existing `workflow list` aesthetic.
- **Q3: `workflow show --json` shape.** Top-level object: `{ "run": {...}, "steps": [...], "approvals": [...] }`. Steps array is sorted by `step_id` (deterministic). Approvals array is sorted by `step_id` (deterministic).
- **Q4: Should `workflow show` show the synthetic approved-gate output?** Yes — operators need to verify the gate was advanced. Show `output_hash` + a truncated preview of `output_json` (first 200 chars).

## Risks

- **Atomic-rename breaks existing tests** that read the output file before it's written by the test. Mitigation: tests construct the data store explicitly; atomic-rename is transparent at the API level. Run full suite.
- **`workflow show` `--json` output drift.** Lock with a regression test asserting the JSON shape.

## Acceptance criteria

- `cargo test --workspace` green including 2 new regression tests (atomic-rename + show).
- `cargo clippy -D warnings` clean.
- `cargo fmt --check` clean.
- Manual demo:
  1. `boruna workflow run examples/workflows/customer_support_triage --data-dir /tmp/d --policy allow-all` (paused).
  2. `boruna workflow show <run-id> --data-dir /tmp/d` — prints run + steps + approvals (empty initially).
  3. `boruna workflow approve <run-id> <gate> --data-dir /tmp/d`.
  4. `boruna workflow show <run-id> --data-dir /tmp/d` — now shows the approval sentinel.
  5. `boruna workflow show <run-id> --data-dir /tmp/d --json | jq '.approvals[0].decision'` returns `"approved"`.
