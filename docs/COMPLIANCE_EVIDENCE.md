# Compliance Evidence

## Evidence Bundles

Every workflow run with `--record` produces a self-contained evidence bundle — a directory of artifacts sufficient for compliance audit.

### Bundle Contents

| File | Purpose |
|---|---|
| `manifest.json` | Bundle metadata, checksums, environment info |
| `workflow.json` | Exact workflow definition used |
| `policy.json` | Exact policy applied |
| `audit_log.json` | Hash-chained log of all decisions and events |
| `env_fingerprint.json` | Runtime environment (Boruna version, OS, arch) |
| `outputs/<step>/<name>.json` | Per-step output data |

### Manifest Fields

```json
{
  "schema_version": 1,
  "run_id": "run-my-workflow-20260221T120000",
  "workflow_name": "my-workflow",
  "workflow_hash": "<sha256>",
  "policy_hash": "<sha256>",
  "audit_log_hash": "<sha256>",
  "file_checksums": {
    "workflow.json": "<sha256>",
    "policy.json": "<sha256>",
    "audit_log.json": "<sha256>"
  },
  "env_fingerprint": {
    "boruna_version": "0.1.0",
    "rust_version": "...",
    "os": "linux",
    "arch": "x86_64",
    "hostname": "worker-01"
  },
  "started_at": "2026-02-21T12:00:00Z",
  "completed_at": "2026-02-21T12:00:01Z",
  "bundle_hash": "<sha256>"
}
```

## Verification

```bash
boruna evidence verify <bundle-dir>
```

Verification checks:
1. All files listed in `file_checksums` exist and match their SHA-256 hashes
2. Audit log hash-chain is intact (each entry's hash includes the previous)
3. Audit log hash matches the manifest's `audit_log_hash`
4. Required files are present (manifest, workflow, policy, audit log, fingerprint)

## Audit Log

The audit log is a JSON array of hash-chained entries:

```json
[
  {
    "sequence": 0,
    "prev_hash": "0000000000000000000000000000000000000000000000000000000000000000",
    "event": { "WorkflowStarted": { "workflow_hash": "...", "policy_hash": "..." } },
    "entry_hash": "<sha256>"
  },
  {
    "sequence": 1,
    "prev_hash": "<hash of entry 0>",
    "event": { "StepCompleted": { "step_id": "fetch", "output_hash": "...", "duration_ms": 42 } },
    "entry_hash": "<sha256>"
  }
]
```

### Tamper Detection

Each entry's hash = SHA-256(sequence + prev_hash + event_json). Modifying any entry invalidates all subsequent hashes.

## Determinism Proof

To prove determinism:
1. Run the same workflow twice with `--record`
2. Compare evidence bundles: file checksums should be identical (excluding timestamps)
3. Audit log event hashes should match for the same events

## What This Proves

For compliance auditors, an evidence bundle demonstrates:
- **What ran**: Exact workflow definition and policy
- **When it ran**: Timestamps and run ID
- **Where it ran**: Environment fingerprint
- **What happened**: Complete audit trail of every step, decision, and output
- **Integrity**: SHA-256 hash chain prevents undetected modification
