# Design — distributed approval-gate / external-trigger (sprint 0.5-S6)

## Problem

Sprint 0.5-S2/S4 made distributed-mode workflows usable end-to-end
for `Source`-kind steps. Approval-gate and external-trigger steps
— the two pause-and-wait-for-human/event step kinds shipped in
0.3-S2c and 0.3-S15 — still require:

- The operator to have filesystem access to the same `data-dir` as
  the cluster, and
- `boruna workflow advance_run_one_tick` to refuse non-first-wave
  approval / trigger steps with an explicit Validation error
  (current behavior).

Result: real-world workflows with human-in-the-loop or
webhook-driven steps cannot be driven from a CI runner that hits
the cluster over HTTP — the operator has to drop down to ssh or
shared volumes to use the local CLI.

## Goal

Two new HTTP routes + two new CLI flags so a CI runner with only
the coordinator URL + bearer token can drive an approval / trigger
gate to terminal status:

- `POST /api/runs/{run_id}/approve` — record approval / rejection
  decision (delegates to existing
  `record_approval_decision_in_store`).
- `POST /api/runs/{run_id}/trigger` — record external trigger
  (delegates to existing `record_external_trigger_in_store`).
- `boruna workflow approve <run_id> <step_id> --coordinator <url>`
  → POSTs to the approve route.
- `boruna workflow reject <run_id> <step_id> --coordinator <url>`
  → POSTs to the approve route with `{"decision":"rejected"}`.
- `boruna workflow trigger <run_id> <step_id> --coordinator <url>
  --token X --payload Y` → POSTs to the trigger route.

The wait driver inside `advance_run_one_tick` learns to:

1. **Open approval/trigger gates** when their dependencies complete
   — insert `AwaitingApproval` / `AwaitingExternalEvent`
   checkpoints. Currently these step kinds fail with a
   non-first-wave Validation error; this sprint replaces that
   error path with proper insertion logic.
2. **Close approval/trigger gates** when their sentinel arrives
   in `metadata.approvals` / `metadata.triggers` — synthesize a
   `Completed` (approved) or `Failed` (rejected) checkpoint so
   downstream steps unblock. The sentinel logic mirrors what
   `WorkflowRunner::resume` does for the in-process path.

## Non-goals

- **Approver identity / audit attribution.** Same posture as 0.4-S9:
  we capture `approver: ""` in the audit chain. Per-operator auth
  remains 0.6.x scope; the current bearer token is shared across
  the whole cluster.
- **Trigger token rotation / expiry.** The trigger token model is
  inherited from 0.3-S15 unchanged. mTLS / per-workflow tokens
  are 0.6.x.
- **Streaming approval prompts.** Operators still poll the existing
  `/api/runs/{id}/status` for the gate state — there's no
  push channel for "this gate just opened, please approve."
  Polling matches the rest of the operator-facing surface and
  keeps the wire format simple.
- **Compound payloads on trigger.** Same 4 MiB body limit as the
  rest of the operator-facing routes; large webhook payloads
  remain a 0.5-S7 (output-blob-refs) concern.
- **Graceful timeout / auto-reject.** No automatic gate timeout —
  if the operator never approves, the run stays paused. Same as
  the existing local CLI's posture.

## Required surface

### CLI

```
boruna workflow approve <run-id> <step-id> \
  --coordinator https://coord.example/ \
  --token $BORUNA_TOKEN

boruna workflow reject  <run-id> <step-id> \
  --coordinator https://coord.example/ \
  --token $BORUNA_TOKEN \
  [--reason "string"]

boruna workflow trigger <run-id> <step-id> \
  --coordinator https://coord.example/ \
  --token $BORUNA_TOKEN \
  --token-arg <trigger-token> \
  --payload <json-string>
```

`--coordinator` is mutually exclusive with `--data-dir` (different
operational model). Bearer-token discovery falls back to
`BORUNA_TOKEN` env var per 0.5-S3.

The `--token` here is the bearer token for the coordinator's auth
middleware; the trigger flow uses a separate `--token-arg` for
the per-step trigger token (the secret stashed in metadata at
gate-pause time). Two-secret world is awkward but matches the
existing security model.

### Coordinator HTTP

```
POST /api/runs/{run_id}/approve   → 200, no body / 4xx + ErrorBody
POST /api/runs/{run_id}/trigger   → 200, no body / 4xx + ErrorBody
```

`POST /api/runs/{run_id}/approve` body:

```json
{
  "step_id": "human_review",
  "decision": "approved" | "rejected",
  "reason": "optional string"
}
```

`POST /api/runs/{run_id}/trigger` body:

```json
{
  "step_id": "stripe_webhook",
  "token": "<32-hex-trigger-token>",
  "payload": "<JSON-payload-string>"
}
```

**New `error_kind` taxonomy entries (locked):**
- `coord.approve.invalid_state` — step not at approval gate / wrong
  step kind / already decided.
- `coord.approve.bad_payload` — body fails to parse / unknown
  decision string.
- `coord.trigger.invalid_state` — step not at external trigger
  gate / wrong step kind / already triggered.
- `coord.trigger.bad_token` — supplied trigger token doesn't match
  the stashed one.
- `coord.trigger.bad_payload` — empty / unparseable payload.

Existing `coord.runs.not_found` and `coord.unauthorized` cover the
remaining failure surfaces.

### Wait driver (`advance_run_one_tick`)

Replaces the current `Err(Validation)` path for non-first-wave
approval / trigger steps with two new behaviors, each idempotent:

1. **Gate open.** Ready (deps Completed) ApprovalGate / Trigger
   step has no checkpoint → insert AwaitingApproval /
   AwaitingExternalEvent. For trigger steps, also acquire the
   trigger token via the existing `acquire_trigger_token`
   helper.
2. **Gate close.** Existing checkpoint is AwaitingApproval +
   metadata.approvals has a sentinel for that step → synthesize
   Completed (approved) or Failed (rejected). Existing
   AwaitingExternalEvent + non-empty trigger payload in
   metadata.triggers → synthesize Completed with payload as
   `result` output (matching the in-process resume path).

Both transitions emit the matching audit events
(`ApprovalGranted` / `ApprovalDenied` for approval,
`ExternalTriggerReceived` for trigger) idempotently — the chain
either has the entry already or gets it appended.

## Why not rebuild on top of `boruna workflow resume`?

Resume reads metadata.approvals / metadata.triggers and synthesizes
checkpoints in-process — the same logic we want in the wait
driver. But resume also re-runs all incomplete waves locally,
which conflicts with distributed mode (workers do that).
Refactoring resume into a "scan + synthesize" pass that the wait
driver and the in-process resumer can share is the right
long-term move; for 0.5-S6 we ship a dedicated wait-driver pass
to keep the scope contained, and accept code-duplication that a
later refactor can collapse.

## Acceptance criteria

- `advance_run_one_tick` opens AwaitingApproval / AwaitingExternalEvent
  checkpoints for non-first-wave gate steps; closes them when
  sentinels appear; new unit tests cover each transition.
- `POST /api/runs/{id}/approve` and `/trigger` validate payload,
  delegate to the in-store record functions, return 200 on
  success / typed `coord.*` error_kind on failure; new handler
  unit tests cover happy path + each error class.
- `boruna workflow approve|reject|trigger --coordinator <url>`
  matches the local-CLI behavior over HTTP; new end-to-end
  integration test drives a full approval gate from submit
  through approval to completion via remote coord+worker.
- All existing tests remain green.
