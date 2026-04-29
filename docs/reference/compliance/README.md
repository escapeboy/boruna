# Compliance Workflow Templates

Pre-built workflow patterns for common regulated use cases. Each template
demonstrates how Boruna's evidence bundle, capability policy, and approval
gate features satisfy specific compliance requirements.

| Template | Standard | Key Feature |
|----------|----------|-------------|
| [`soc2_audit_workflow`](../../examples/compliance/soc2_audit_workflow/) | SOC 2 | Hash-chained evidence bundle as tamper-evident audit trail |
| [`hipaa_data_pipeline`](../../examples/compliance/hipaa_data_pipeline/) | HIPAA | PHI redaction before evidence bundle write |
| [`financial_review_pipeline`](../../examples/compliance/financial_review_pipeline/) | SOX / dual-control | Multi-approver approval gates with immutable sign-off record |

## How to use

1. Copy the template to your project
2. Replace synthetic data with real capability calls (e.g., `net.fetch` for live system data)
3. Run with `--record` to produce a verifiable evidence bundle:
   ```bash
   boruna workflow run examples/compliance/soc2_audit_workflow --policy allow-all --record
   boruna evidence verify <bundle-dir>
   ```
4. The verified bundle is your compliance artifact

## Customisation

Each template README describes which steps to modify for your environment.
Real integrations typically replace the `gather_*` or `receive_*` first step
with a capability call to fetch live data.
