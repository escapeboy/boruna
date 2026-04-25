# Test Plan — 0.3-S2c

## Layer 1 — Persistence

| ID | Scenario | Pass criteria |
|---|---|---|
| P-1 | `get_run_metadata` returns `None` for missing run | Ok(None) |
| P-2 | `get_run_metadata` round-trips a string | Insert run → fetch metadata → bytes-identical |
| P-3 | `update_run_metadata` updates the row + `updated_at_ms` | After update, fetch shows new metadata + new timestamp |
| P-4 | `update_run_metadata` returns `NotFound` for missing run | Typed error |
| P-5 | `list_runs` returns rows ordered by (workflow_name, run_id) | Determinism check |
| P-6 | `list_runs` on empty DB returns empty Vec | |

## Layer 2 — Approval-decision recording

| ID | Scenario | Pass criteria |
|---|---|---|
| A-1 | `record_approval_decision(approved)` against a paused approval-gate step writes `metadata.approvals.<step>` | Re-fetched metadata has the decision |
| A-2 | `record_approval_decision` against a missing run → `RunNotFound` | Typed error |
| A-3 | `record_approval_decision` against a non-existent step → `StepNotFound` | Typed error |
| A-4 | `record_approval_decision` against a step whose checkpoint is `Running` (impossible-but-defensive) → `StepNotAtApprovalGate` | Typed error w/ `current_status` |
| A-5 | `record_approval_decision` against a step that's not an `ApprovalGate` kind in the workflow def → `NotAnApprovalGateStep` | Typed error |
| A-6 | `record_approval_decision` against a step that already has a sentinel → `StepAlreadyDecided { prior_decision }` | Typed error w/ prior decision |
| A-7 | `record_approval_decision` against a Completed/Failed run → `RunNotResumable { terminal_status }` | Typed error |
| A-8 | `record_approval_decision(rejected, Some(reason))` round-trips the reason | Re-fetched metadata.approvals carries `reason: Some("...")` |
| A-9 | Two threads racing `record_approval_decision` for the same step: one wins, the other gets `StepAlreadyDecided` | Atomic via BEGIN IMMEDIATE |

## Layer 3 — Resume honors the sentinel

| ID | Scenario | Pass criteria |
|---|---|---|
| R-1 | Approve a paused gate, run resume → workflow completes | `WorkflowStatus::Completed`; gate step shows `Completed` in result; downstream steps execute |
| R-2 | Reject a paused gate, run resume → workflow halts Failed | `WorkflowStatus::Failed`; gate step `Failed` with `error_msg` matching reason; no downstream steps run |
| R-3 | No sentinel → resume re-pauses (regression of 0.3-S2b behavior) | Re-pause as today |
| R-4 | Sentinel for a step that's already terminal → no-op | Resume produces same result as without the sentinel |
| R-5 | Approved gate's persisted step checkpoint after resume = `Completed` with output_json `{}` (synthetic empty record) | Locked by checkpoint inspection |
| R-6 | Approve idempotency: 2nd resume after a successful approved-resume is a no-op (terminal Completed) | Same as the 0.3-S2b "resume Completed run" path |

## Layer 4 — CLI surface

| ID | Scenario | Pass criteria |
|---|---|---|
| C-1 | `boruna workflow approve <run-id> <step-id>` succeeds for a paused gate | Exit 0; success message points at resume |
| C-2 | `boruna workflow approve <bad-run-id> <step>` exits non-zero | Exit 1; typed message |
| C-3 | `boruna workflow reject <run-id> <step-id> --reason "X"` records the rejection | Exit 0; metadata has `decision: rejected, reason: Some("X")` |
| C-4 | `boruna workflow list` prints all runs, ordered | Output contains run_ids |
| C-5 | `boruna workflow list --status paused` filters | Only paused runs shown |
| C-6 | `boruna workflow list --json` emits valid JSON array | `serde_json::from_str::<Vec<Value>>` parses |
| C-7 | End-to-end demo from architecture § Acceptance | All 5 steps green |

## Layer 5 — Gates

- `cargo test --workspace` — green
- `cargo test --workspace --features boruna-vm/http` — green
- `cargo clippy --workspace -- -D warnings` — clean
- `cargo fmt --all -- --check` — clean
- Smoke: customer_support_triage workflow paused → approve → resume → Completed
