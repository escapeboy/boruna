# SOC 2 Audit Workflow

A four-step linear workflow demonstrating how Boruna's hash-chained evidence bundle satisfies SOC 2 audit trail requirements.

## Steps

```
gather_system_state → classify_access_events → generate_audit_report → verify_evidence_chain
```

| Step | Purpose | SOC 2 Criteria |
|------|---------|----------------|
| `gather_system_state` | Collects uptime, access counts, and anomaly flags | A1 (Availability) |
| `classify_access_events` | Applies CC6.x access classification rules | CC6 (Security) |
| `generate_audit_report` | Formats a Section III narrative summary | All in-scope criteria |
| `verify_evidence_chain` | Confirms all prior outputs are non-empty | Internal consistency gate |

## Compliance narrative

Each step output is recorded in Boruna's hash-chained `AuditLog`. The chain is SHA-256 linked: tampering with any step's output breaks the hash chain and is detected immediately by `boruna evidence verify`.

The resulting evidence bundle can be shared with an external auditor as a tamper-evident record of the full audit cycle — no separate audit log tooling required.

## Running

```bash
# Demo mode (synthetic data — no capabilities required)
boruna workflow run examples/compliance/soc2_audit_workflow --policy allow-all --record

# Verify the bundle
boruna evidence verify <bundle-dir>
```

## Adapting for production

Replace `gather_system_state.ax` with a `net.fetch` call (or `db.query`) that pulls live system telemetry from your SIEM or metrics platform. Add `"net.fetch"` to that step's `capabilities` array in `workflow.json` and configure an appropriate `--policy` file with your allowed domains.
