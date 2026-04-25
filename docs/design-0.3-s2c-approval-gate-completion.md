# Design — 0.3-S2c: Approval-Gate Completion

**Status:** 2026-04-25 (Think phase)
**Predecessor:** `0.3-S2b` shipped persistent runs + resume but explicitly deferred the approval-gate completion mechanism. The CLI prints `boruna workflow approve <run-id> <step-id>` — pointing at a command that doesn't yet exist. This sprint closes that loop.

## Scope

An operator with a paused approval-gate run can advance it to completion (or reject it to a failed terminal state) via a CLI command. Resume after approval picks up past the gate.

**In scope:**

1. `boruna workflow approve <run-id> <step-id>` — record an approval sentinel in the run's `metadata_json.approvals.<step_id>`, set the step checkpoint to `Completed`, allow resume to proceed.
2. `boruna workflow reject <run-id> <step-id> [--reason <STR>]` — record a rejection, set the step + run to `Failed`, terminal.
3. `WorkflowRunner::resume` honors the sentinel: an `awaiting_approval` step with an approved sentinel transitions to `Completed` (output = empty record `{}`) and execution proceeds; with a rejected sentinel the run halts as Failed; with no sentinel re-pauses (the existing 0.3-S2b behavior).
4. **Stretch:** `boruna workflow list [--status <STATUS>]` — list runs, optionally filtered by status. Builds on `RunCheckpointStore::list_runs_by_status`.
5. Per project-conventions §1: typed errors for `approve`/`reject` against missing run, wrong step, already-decided step, non-approval-gate step.

**Out of scope:**

- Multi-approver / quorum semantics. (One approver per gate; future sprint.)
- Time-bounded approvals (auto-reject after N hours).
- Webhook / external-system approvals (this sprint stays CLI-only).
- Audit-log entries for the approval action — sketched but deferred unless review demands it.

## Forcing questions (Think)

**Who needs this? What are they doing today?**
The operators of compliance-driven workflows (LLM batch grading, document fan-outs) who paused at a human-review gate. Today: paused runs sit indefinitely; the only way to advance is to manually patch the SQLite database, which is not a documented operator interface.

**What's the narrowest MVP someone would pay for?**
`boruna workflow approve <run-id> <step-id>` plus a resume that honors the sentinel. Reject is a freebie (same code path with a different terminal-state). List is a stretch.

**What would make someone say "whoa"?**
The same operator running `boruna workflow list --status paused` to see all pending approvals, then running `boruna workflow approve <id> <step>` to resolve each one. The pause→approve→resume→complete cycle is the operator UX that makes the platform usable.

**How does this compound?**
Closes the persistence story for 0.3.0. Without approve/reject, the persistence work in 0.3-S2a/S2b is half-built — operators have a database they can write to but no documented interface for the most common operation against it. With this, the audit trail is complete: every state transition (run start, step complete, approval, rejection, run complete) is in the persisted record.

## Key invariants (must not regress)

1. **Determinism**: `run_id`, replay-verified columns, terminal statuses bit-identical given identical inputs. Approval sentinels are operational metadata — they record WHO approved WHEN, but the audit hash chain only sees that the gate completed (`Completed` status).
2. **Approval is a one-way transition**: an approved gate cannot be revoked; a rejected gate cannot be approved. Subsequent `approve`/`reject` calls against a decided gate return a typed error.
3. **No silent footguns**: `approve` against a non-approval-gate step → typed error; against a missing run → typed error; against a step that's not in `awaiting_approval` state → typed error.
4. **Resume of an already-approved gate is idempotent**: running `resume` after `approve` produces the same final state regardless of how many times it's invoked.
5. **`workflow_hash` mismatch still refuses to resume** (carried from 0.3-S2b). Approve doesn't bypass the hash check.

## Open questions (to resolve in Plan)

- **Q1: Sentinel shape in `metadata_json`.** Options:
  - (a) `metadata.approvals.<step_id>: "approved" | "rejected"` — minimal, opaque.
  - (b) `metadata.approvals.<step_id>: { decision, decided_at_ms, reason? }` — captures audit information.
  - **Lean toward (b)** so the operational metadata is dashboard-ready.
- **Q2: Should `approve` write a step checkpoint update directly, or only the metadata sentinel?**
  - If only the sentinel, resume mutates the step checkpoint when it sees the sentinel. Cleaner state machine — sentinel is the source of truth, checkpoint follows.
  - If both, `approve` does both writes atomically. Faster operator UX (no need to run `resume` for the checkpoint to update), but couples the two write paths.
  - **Lean toward sentinel-only**. Resume is the canonical state transition.
- **Q3: `approve` of a step that isn't currently in `awaiting_approval` — what error?**
  - Cases: step is `Pending` (workflow hasn't reached the gate yet), `Running` (impossible for an approval gate), `Completed` (already decided), `Failed` (terminal), no checkpoint (workflow doesn't have this step at all).
  - **Lean toward** distinct typed errors: `step_not_at_approval_gate`, `step_already_decided`, `step_not_found`. Avoid one catch-all.
- **Q4: `approve` against a Completed/Failed run.**
  - **Lean toward**: typed error `run_not_resumable` regardless of step state. The run is terminal; mutations are not allowed.
- **Q5: `workflow list` — what columns + ordering?**
  - Status, run_id, workflow_name, started_at, updated_at; ordered by `(workflow_name, run_id)` to match the existing deterministic sort in `list_runs_by_status`. Not by timestamps (operational-only).

## Risks

- **Sentinel-only approach forces operator to run `resume` after `approve`.** Mitigation: the `approve` command's success message tells them: "Run `boruna workflow resume <run-id> --data-dir <PATH>` to advance past the gate." Plus: design `approve` to call into resume internally if a `--resume` flag is passed. Keeps the two states clean while enabling one-shot operator UX.
- **`workflow list` performance** on large databases — `list_runs_by_status` does a full scan. Acceptable for 0.3.x; revisit if datasets grow.
- **Backwards compat for existing `runs.db` files** from 0.3-S2b that have no `approvals` field in metadata. The `PersistedRunMetadata` struct adds `approvals` with `#[serde(default)]` so old databases parse cleanly.

## Acceptance criteria

- `cargo test --workspace` passes including new `approve`/`reject`/`list` regression tests.
- `cargo clippy -D warnings` clean.
- `cargo fmt --check` clean.
- Manual demo:
  1. `boruna workflow run examples/workflows/customer_support_triage --data-dir /tmp/d --policy allow-all` (pauses at approval gate)
  2. `boruna workflow list --data-dir /tmp/d` shows the paused run.
  3. `boruna workflow approve <run-id> <step-id> --data-dir /tmp/d`
  4. `boruna workflow resume <run-id> --data-dir /tmp/d` completes the run.
  5. `boruna workflow list --data-dir /tmp/d --status completed` includes it.
