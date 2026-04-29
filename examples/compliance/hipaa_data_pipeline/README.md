# HIPAA Data Pipeline

A four-step linear workflow demonstrating HIPAA-compliant PHI handling: redact before output so the evidence bundle never contains Protected Health Information.

## Steps

```
receive_record → identify_phi_fields → redact_for_output → verify_redaction
```

| Step | Purpose | HIPAA Requirement |
|------|---------|-------------------|
| `receive_record` | Loads a patient record (synthetic placeholder) | Record receipt audit trail |
| `identify_phi_fields` | Classifies fields per Safe Harbor method | Minimum-necessary analysis |
| `redact_for_output` | Replaces PHI fields with `[REDACTED]` | Minimum-necessary (§164.502(b)) |
| `verify_redaction` | Confirms no PHI values appear in output | Policy enforcement gate |

## Compliance narrative

HIPAA requires that audit logs do not themselves expose PHI. Boruna's evidence bundle records each step's output — by redacting in the step before output, the bundle retains the audit trail without containing identifiable information. The `verify_redaction` step acts as a deterministic policy enforcement gate: if PHI leaked into the redacted output, the workflow returns a non-zero exit and the bundle is flagged.

The Safe Harbor classification in `identify_phi_fields.ax` follows 45 CFR §164.514(b): `name` and `dob` are direct identifiers; `diagnosis_code` (ICD code alone) and `provider_id` are not PHI under Safe Harbor.

## Running

```bash
# Demo mode (synthetic data — no PHI, no capabilities required)
boruna workflow run examples/compliance/hipaa_data_pipeline --policy allow-all --record

# Verify the bundle
boruna evidence verify <bundle-dir>
```

## Adapting for production

Replace `receive_record.ax` with a `db.query` capability call to your EHR system. Add `"db.query"` to that step's `capabilities` array in `workflow.json`. The redaction and verification steps operate on the string output and require no changes.

Update the known PHI values in `verify_redaction.ax` to match the actual field values from your record schema — or replace the simple string-search with a schema-driven check.
