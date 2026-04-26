# Design: Audit-log integration of approval/trigger decisions

**Sprint**: `0.4-S9`. Closes a 0.3.0 carried-forward debt.

## Problem

Today's audit module (`orchestrator/src/audit/`) defines a hash-chained
`AuditLog` with event variants for `ApprovalGranted`, `ApprovalDenied`,
plus the lifecycle events. But **the runner never constructs an
`AuditLog`** — the variants are dead code from a runtime perspective.

This is a real gap for compliance-driven use cases. The operator's
approval / trigger action determines workflow flow (Approved →
advance, Rejected → halt with reason). Without an audit-chain entry
recording WHO authorized WHAT, a malicious operator could replay a
run with substituted decisions and the chain would not detect it.

## Decision

Wire `record_approval_decision` and `record_external_trigger` to
append to a per-run hash-chained audit log. The log is persisted in
the run's `metadata.audit_log` field. Both decision entry points
include the new audit entry in their compare-and-swap write so the
audit log is updated atomically with the operator-facing metadata.

## Replay-verification classification

The audit chain captures decisions, not operator identity or
timestamps:

| Field                      | In hash chain? | Rationale                                      |
| -------------------------- | -------------- | ---------------------------------------------- |
| step_id                    | YES            | Identifies which step the decision applies to. |
| approval kind (granted/denied) | YES        | Determines workflow flow; replay-relevant.     |
| rejection reason           | YES            | Surfaces in step `error_msg` of failed gate.   |
| approver username          | YES (in `approver` field) | Captured for accountability; replays must include the same string. |
| trigger payload_hash       | YES            | Already in step `output_hash`; redundant but distinguishes "trigger advanced" from "source completed". |
| trigger payload itself     | NO (only hash) | Operator's webhook body; large; hash suffices. |
| decided_at_ms / triggered_at_ms | NO        | Wall-clock-keyed; operational only.            |
| operator session info      | NO             | Not captured today; future identity sprint.    |

## What this sprint ships

1. New `AuditEvent::ExternalTriggerReceived { step_id, payload_hash }`
   variant.
2. `PersistedRunMetadata.audit_log: AuditLog` field, defaulted via
   `#[serde(default)]` for back-compat with 0.3.x databases (no
   `audit_log` field → empty log).
3. `record_approval_decision` appends `ApprovalGranted` or
   `ApprovalDenied` to the run's audit log inside the existing CAS
   loop.
4. `record_external_trigger` appends `ExternalTriggerReceived` inside
   the existing atomic commit (the new metadata already includes the
   updated log).
5. CLI: `boruna workflow show --json` exposes `audit_log_hash` and
   `audit_log_entry_count` fields.
6. Tests covering: events appended on decision, hash chain verifies,
   multiple decisions chain correctly, legacy DBs (no audit_log
   field) deserialize cleanly.

## What this sprint does NOT ship

- **Full lifecycle audit events** (`WorkflowStarted`, `StepStarted`,
  `StepCompleted`, etc.) — separately scheduled. This sprint
  surgically closes the 0.3.0 debt without touching the per-step
  hot path.
- **Audit log in evidence bundles** — `EvidenceBundleBuilder::finalize`
  already accepts an `AuditLog` parameter; wiring the in-metadata log
  into bundle construction is a small follow-on sprint.
- **Operator identity capture** — no auth subsystem yet. The
  `approver` field is operator-supplied via the CLI today (currently
  hardcoded; future sprint will add proper identity).

## Concurrency

Same CAS-retry loop as the existing decision flows. If two
operators race to decide on different steps in the same run, the
losing CAS re-reads the on-disk metadata (now containing the other
writer's audit entry) and appends its own entry on top. The chain
is preserved.

For triggers, the atomic commit (`commit_external_trigger`) already
serializes metadata + checkpoint writes via `BEGIN IMMEDIATE`; the
audit append happens before serializing the metadata, so the
on-disk log is always consistent with the on-disk checkpoint state.
