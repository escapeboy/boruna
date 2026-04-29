# Financial Review Pipeline

A five-step workflow with dual-control approval gates, demonstrating SOX and internal-control requirements for financial change management.

## Steps

```
extract_changes → [approval: finance_manager] → [approval: finance_controller] → reconcile → finalize_record
```

| Step | Kind | Purpose | Control |
|------|------|---------|---------|
| `extract_changes` | source | Load pending financial change record | — |
| `first_approver` | approval_gate | Finance manager sign-off | Segregation of duties |
| `second_approver` | approval_gate | Finance controller sign-off | Dual control |
| `reconcile` | source | Validate amount against policy limits | Policy enforcement |
| `finalize_record` | source | Format final approved record for posting | Audit trail |

## Compliance narrative

Dual-control approval is a core internal control under SOX (Sarbanes-Oxley) Section 302/404 and COSO framework. No single person can approve and post a financial change. The two `approval_gate` steps map directly to this requirement: both the `finance_manager` and `finance_controller` roles must provide explicit sign-off before `reconcile` runs.

Boruna's evidence bundle records which approver approved at what point in the execution, creating an immutable, cryptographically-linked audit trail. The bundle cannot be altered without breaking the SHA-256 hash chain, making it a verifiable record suitable for external auditors.

## Running

```bash
# Demo mode (synthetic data, async approval gates skipped in non-interactive mode)
boruna workflow run examples/compliance/financial_review_pipeline --policy allow-all --record

# Verify the bundle
boruna evidence verify <bundle-dir>
```

## Triggering approval gates

In production, approval gates wait for an external trigger from an authorized approver. Use the async trigger mechanism:

```bash
# Finance manager approves
boruna workflow trigger <run-id> --step first_approver --role finance_manager

# Finance controller approves
boruna workflow trigger <run-id> --step second_approver --role finance_controller
```

## Adapting for production

Replace `extract_changes.ax` with a `db.query` call to your financial system. Update the `amount_cents` policy limit in `reconcile.ax` to match your internal control thresholds. The approval gates and evidence bundle require no changes.
