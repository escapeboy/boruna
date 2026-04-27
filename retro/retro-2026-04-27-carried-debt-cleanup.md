# Sprint Retro — 2026-04-27 (carried-debt cleanup + parallel-agent prevention)

**Sprint:** carried-debt cleanup + parallel-agent path-resolution prevention
**Pipeline:** sequential (no parallel agents)
**Outcome:** Merged to `master` at `b2da53c`, pushed to origin
**Driver:** Three small adversarial-review findings from earlier
sprints had been accepted as documented limitations rather than
fixed. Plus the parallel-agent path-resolution issue from the
multi-sprint session needed prevention infrastructure (since the
Agent tool runtime can't be patched here).

## What shipped

### Carried-debt fixes

1. **Audit chain wait-driven terminating event.**
   `WorkflowRunner::append_wait_terminal_audit_event` emits
   `WorkflowCompleted` when the wait driver reaches Completed
   or Failed. Idempotent — re-invoked waits don't double-emit.
   Wired into `coordinator wait`'s terminal exit paths.
   3 new orchestrator unit tests (append + idempotency +
   zero-hash fallback).

2. **Two-concurrent-waits integration test.** CORR-6 from
   0.5-S2f adversarial review. Spawns two `coordinator wait`
   processes against the same run_id; verifies both exit
   cleanly because the underlying race-safe persistence
   primitives (`INSERT ... ON CONFLICT DO NOTHING`) collapse
   concurrent inserts. 1 new CLI integration test.

3. **Submit-only `--concurrency` warning.** F3 from 0.5-S2e
   adversarial review. Submit-only silently ignored
   `--concurrency`; now emits a clear stderr warning at
   submit time. 1 new orchestrator unit test (regression:
   the warning doesn't crash the run).

### Path-resolution prevention

- **`docs/AGENT-PROMPT-TEMPLATE.md`** — reusable skeleton for
  parallel worktree-agent prompts. Bakes in:
  - Worktree-verification block (`pwd; git rev-parse
    --show-toplevel; git branch --show-current`) that agents
    run before any file edit.
  - Relative-path discipline ("DO NOT use absolute paths").
  - Pre-launch + post-completion checklists.
  - "When NOT to use parallel worktree agents" — sometimes
    sequential is better.

- **`CLAUDE.md` "Parallel-Agent Best Practices" section** —
  documents the failure mode (absolute paths bypass cwd) and
  the required prevention. Adjacent to existing "Critical
  Invariants" so it surfaces during any future
  parallel-agent attempt.

- **Project conventions #31 + #32** added to the
  `project-conventions-2026-04` Serena memory.

## What worked

1. **Three small fixes shipped sequentially in ~30 minutes.**
   Each was a well-defined adversarial-review finding from
   prior sprints. No design phase needed — just implementation
   + tests + docs. This is the right tempo for cleanup-style
   work; resist the urge to wrap small fixes in a full
   sprint-orchestrate pipeline.

2. **Idempotency was the design pattern.** Both the audit
   terminating event and the concurrent-waits test rely on
   idempotent operations: the audit append checks whether
   `WorkflowCompleted` is already in the chain; the
   persistence primitives (`insert_pending_step_if_absent`,
   `requeue_failed_step_for_retry`) use `ON CONFLICT DO
   NOTHING`. Convention §14 (race semantics) was directly
   applicable.

3. **Path-resolution prevention is non-code work that pays
   back.** The Agent tool runtime can't be patched from this
   repo, but the workflow can be hardened so the failure
   mode doesn't recur. Convention + template + CLAUDE.md
   section is the minimum viable prevention; future
   parallel-agent prompts start from the template.

4. **Sequential was the right call this session.** After the
   multi-sprint parallelism failure, sequential gave me
   tight control over each fix without contamination risk.

## Adversarial-review findings triaged

No new adversarial review run (this is a cleanup pass, not a
substantive sprint per convention §29). The three fixes
themselves resolve prior adversarial findings. Self-review
caught one edge case during implementation:

- **Audit terminating event idempotency.** First draft would
  have emitted a second `WorkflowCompleted` if `coordinator
  wait` was re-invoked against a Completed run. Added an
  upfront chain scan that returns Ok(()) when an entry
  already exists. Locked by
  `append_wait_terminal_audit_event_is_idempotent`.

## What's next

The three carried debts that DIDN'T get addressed this
session (and were correctly skipped):

- **`boruna fmt` v2 with comment preservation** — token-aware
  formatter; full sprint, ~800-1200 LOC.
- **mTLS / per-worker keys / OAuth** — full sprint, possibly
  larger; gating for high-security deployments.
- **HTTP-based remote wait** — architectural change to the
  wait driver protocol; full sprint.

Plus the unchanged v1 work:
- 0.5-S4 (`workflow run --coordinator <url>`)
- 0.5-S6 (distributed approval-gate / external-trigger)
- 0.5-S7 (output blob refs)
- Versioned `.ax` spec freeze
- 1.0.0 commitment work (security audit, performance
  baselines, LTS)

## Tests / gates

- `cargo test --workspace --features boruna-cli/serve` — all
  green. Test count delta this session: +4 orchestrator unit
  + 1 cli_coordinator_worker integration. 314 orchestrator
  total; 22 cli_coord_worker integration.
- `cargo clippy --workspace --features boruna-cli/serve -- -D
  warnings` — clean.
- `cargo fmt --all -- --check` — clean.

## Conventions reinforced

- §14 (race semantics → right primitive) — applied to audit
  idempotency: chain scan before append, instead of relying
  on dedup at read time.
- §18 (document known limitations in 4 places) — design doc,
  CHANGELOG, retro, and the Serena memory all updated when
  the carried debt was resolved.
- §29 (adversarial review pays for itself) — cleanup of three
  prior-sprint findings closes the loop on convention §29's
  full lifecycle (find → triage → fix).
- §30 (cargo test + clippy + fmt before commit) — caught one
  fmt issue introduced during edits.

## Conventions added

- **§31 — Parallel worktree-agent prompts use RELATIVE paths
  only.** Documented and locked in the convention memory +
  CLAUDE.md + reusable template.
- **§32 — Strong gates absorb tooling failures.** Don't skip
  gates to save time; they're load-bearing for failure
  recovery, not just baseline correctness.

## Session summary (cumulative across all sprints today)

This is sprint number ~9 in this session. Total session output:

- 0.5-S2a–S2f (six distributed-execution sprints)
- 0.5-S3 (auth) + 0.5-S5 (retry) + DX `boruna fmt`
  (multi-sprint reconciliation after parallel-agent failure)
- This carried-debt cleanup

The 0.5.0 cycle's distributed-execution work is functionally
complete + production-ready (auth gates non-loopback bind,
retry honors RetryPolicy, multi-wave advancement works,
audit chain is closed). v1 still needs spec freeze and
1.0.0 commitment work.

**Recommended next session:** start fresh. The remaining v1
work (0.5-S4, S6, S7, spec freeze, 1.0 commitment) is best
tackled with new context.
