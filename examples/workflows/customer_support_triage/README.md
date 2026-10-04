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

The `approve` step is `kind: approval_gate` in `workflow.json`. When the triage result has `severity >= 3`, the workflow pauses and waits for a user with `required_role: support_lead` to approve or reject routing.

This is not simulated — the gate is enforced by the workflow runner. The decision is recorded in the run's hash-chained audit log, with the name and reason given on the command line:

```bash
boruna workflow approve <run-id> approve --approver "Maria Petrova" --reason "Outage confirmed with the customer"
boruna workflow resume <run-id>
```

The name is self-declared; Boruna does not authenticate it.

## How to run

```bash
# Validate the workflow (including the approval gate definition)
cargo run --bin boruna -- workflow validate examples/workflows/customer_support_triage

# Run in demo mode
cargo run --bin boruna -- workflow run examples/workflows/customer_support_triage --policy allow-all

# Run and record evidence up to the pause (see below for the bundle with the decision)
cargo run --bin boruna -- workflow run examples/workflows/customer_support_triage --policy allow-all --record
```

## Evidence produced

Each `--record` run writes a bundle to `<evidence-dir>/<run-id>/` (default `evidence/` inside the workflow folder) containing:

- `audit_log.json` — hash-chained log of the run up to that point
- `workflow.json` — the workflow definition, including the gate's `required_role: support_lead`
- `policy.json` — the policy the run used
- `env_fingerprint.json` — environment fingerprint
- `bundle.json`, `manifest.json` — format version, checksums and bundle hash

`--record` writes the bundle when the run stops, which for this workflow is the pause at the gate, so that bundle does not contain the decision yet (`resume` has no `--record`). After approving and resuming, build the full bundle from the stored run:

```bash
boruna evidence create <run-id> --output-dir evidence/
boruna evidence verify evidence/<run-id>
```

This bundle also has `outputs/` (each step's result), and its `audit_log.json` includes the `ApprovalGranted` / `ApprovalDenied` entry with the approver's name and reason.

The approval gate record provides a compliance artifact: "this ticket was escalated by X, approved by Y at Z time, under policy P."

## Notes

- The demo ticket is a Severity 5 (critical) production outage from an enterprise customer — this exercises the approval gate path.
- To test the non-gate path, change the ticket in `receive.ax` to a lower severity so triage produces `severity < 3`.
