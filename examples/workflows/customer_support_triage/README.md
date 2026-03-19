# Customer Support Triage Workflow

**Pattern**: Approval gate (4 steps, 1 gate)
**Demonstrates**: Human-in-the-loop, conditional pause, high-severity escalation, audit trail

## Use case

Your support team receives tickets ranging from routine questions to production outages. High-severity tickets require a support lead to approve routing before an engineer is paged, ensuring accountability and preventing false escalations. This workflow automates the classification and gate — then routes once approved.

The approval gate is the key compliance feature: every high-severity escalation has an auditable record showing who approved it, when, and under what policy.

## Steps

```
receive ──► triage ──► [approval gate: severity >= 3] ──► route
```

| Step | Role | Capability |
|------|------|-----------|
| `receive` | Parse incoming ticket from queue or webhook | `net.fetch` (live mode) |
| `triage` | LLM analysis: assign severity (1-5) and routing team | `llm.call` (live mode) |
| `approve` | Human approval gate — pauses if severity ≥ 3 | (policy-enforced gate) |
| `route` | Send routing notification and create incident record | `net.fetch` (live mode) |

## The approval gate

The `approve` step is `kind: approval_gate` in `workflow.json`. When the triage result has `severity >= 3`, the workflow pauses and waits for a user with `required_role: support_lead` to approve or reject routing. The approval decision is recorded in the evidence bundle.

This is not simulated — the gate is enforced by the workflow runner. In the current CLI implementation, approval is recorded as a workflow event.

## How to run

```bash
# Validate the workflow (including the approval gate definition)
cargo run --bin boruna -- workflow validate examples/workflows/customer_support_triage

# Run in demo mode
cargo run --bin boruna -- workflow run examples/workflows/customer_support_triage --policy allow-all

# Run and record evidence (includes approval gate event)
cargo run --bin boruna -- workflow run examples/workflows/customer_support_triage --policy allow-all --record
```

## Evidence produced

Each `--record` run writes a bundle to `evidence/run-customer-support-triage-<timestamp>/` containing:

- `audit_log.json` — hash-chained log including the approval gate event and approver identity
- `policy.json` — policy snapshot (shows that the gate required `support_lead` role)
- `outputs/` — triage result, routing confirmation
- `env.json` — environment fingerprint

The approval gate record provides a compliance artifact: "this ticket was escalated by X, approved by Y at Z time, under policy P."

## Notes

- The demo ticket is a Severity 5 (critical) production outage from an enterprise customer — this exercises the approval gate path.
- To test the non-gate path, change the ticket in `receive.ax` to a lower severity so triage produces `severity < 3`.
