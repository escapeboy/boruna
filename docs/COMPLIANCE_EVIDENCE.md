# Compliance Evidence

## Evidence Bundles

A workflow run with `boruna workflow run <dir> --record` produces a self-contained evidence bundle — a directory of artifacts for compliance audit. `boruna evidence create <run-id>` builds a bundle from a run in the persistent store and also includes the per-step outputs.

### Bundle Contents

| File | Purpose |
|---|---|
| `manifest.json` | Bundle metadata, checksums, environment info, optional signature and encryption info |
| `bundle.json` | Bundle format version (`1.1`), Boruna version, component list |
| `workflow.json` | Exact workflow definition used |
| `policy.json` | Exact policy applied |
| `audit_log.json` | Hash-chained log of workflow, step and approval events |
| `env_fingerprint.json` | Runtime environment (Boruna version, OS, arch) |
| `outputs/<step>/result.json` | Per-step output data (bundles built with `evidence create`) |
| `confidence_gates.json`, `confidence/<step>.calibration.json` | Confidence-gate decisions and calibration files (only for workflows that use `confidence_gate`) |

### Manifest Fields

```json
{
  "schema_version": 1,
  "run_id": "805554905d6624c4",
  "workflow_name": "my-workflow",
  "workflow_hash": "<sha256>",
  "policy_hash": "<sha256>",
  "audit_log_hash": "<sha256>",
  "file_checksums": {
    "audit_log.json": "<sha256>",
    "env_fingerprint.json": "<sha256>",
    "policy.json": "<sha256>",
    "workflow.json": "<sha256>"
  },
  "env_fingerprint": {
    "boruna_version": "3.5.0",
    "rust_version": "unknown",
    "os": "linux",
    "arch": "x86_64",
    "hostname": "unknown"
  },
  "started_at": "2026-10-03T12:00:00.000000+00:00",
  "completed_at": "2026-10-03T12:00:01.000000+00:00",
  "bundle_hash": "<sha256>"
}
```

Two optional fields appear only when used: `encryption` (bundle encrypted with `--encrypt-bundle`) and `signature` (ed25519 signature over `bundle_hash`).

## Verification

```bash
boruna evidence verify <bundle-dir>
```

Verification checks:
1. The bundle format version is supported
2. If present, the ed25519 signature over `bundle_hash` is valid (`--verify-key <hex>` pins the key, `--require-signature` rejects unsigned bundles)
3. Encrypted bundles are decrypted in memory (`--bundle-encryption-key`); `--require-encryption` rejects plaintext bundles
4. All files listed in `file_checksums` exist and match their SHA-256 hashes
5. Audit log hash-chain is intact and its hash matches the manifest's `audit_log_hash`; redacted entries are reported
6. Confidence-gate decisions are recomputed from the recorded calibration
7. Required files are present (`bundle.json`, manifest, workflow, policy, audit log, fingerprint)

`--expected-bundle-hash <hex>` additionally checks the bundle against a hash recorded elsewhere.

## Audit Log

The audit log is a JSON array of hash-chained entries:

```json
[
  {
    "sequence": 0,
    "prev_hash": "0000000000000000000000000000000000000000000000000000000000000000",
    "content_sha256": "<sha256 of event_json>",
    "event": { "WorkflowStarted": { "workflow_hash": "...", "policy_hash": "..." } },
    "entry_hash": "<sha256>"
  },
  {
    "sequence": 1,
    "prev_hash": "<hash of entry 0>",
    "content_sha256": "<sha256 of event_json>",
    "event": { "StepCompleted": { "step_id": "fetch", "output_hash": "...", "duration_ms": 42 } },
    "entry_hash": "<sha256>"
  }
]
```

### Tamper Detection

`content_sha256 = SHA-256(event_json)` and `entry_hash = SHA-256(sequence + prev_hash + content_sha256)`. Modifying any entry invalidates all subsequent hashes. Format 1.0 bundles, which have no `content_sha256`, hash `sequence + prev_hash + event_json` directly and still verify.

Because the chain commits to `content_sha256` rather than the event bytes, `boruna evidence redact <dir> --event <N> [--field <name>]` can remove content (for example PII) from a sealed bundle while `evidence verify` still passes. `audit_log_hash` stays the same under redaction and changes under tampering. Confidence-gate decisions cannot be redacted.

## Determinism Proof

To check that two runs produced the same results:
1. Run the same workflow twice with `--record`
2. Compare the bundles with `boruna evidence diff <bundle-a> <bundle-b>`, or compare them by hand: `workflow.json` and `policy.json` checksums, and each step's `output_hash` in the audit log, should be identical

The audit log hash itself differs between runs, because `StepCompleted` events record `duration_ms` and the manifest records timestamps.

## Compliance Mapping Report

```bash
boruna evidence report <bundle-dir> --framework eu-ai-act|nist|iso42001 [--format md|html]
```

Verifies the bundle, states the verdict at the top, and maps the bundle's contents to the obligations they help satisfy (EU AI Act, NIST AI RMF 1.0, ISO/IEC 42001), flagging gaps. It is a technical mapping, not a certificate of compliance.

## Interoperability and External Witnessing

- `boruna evidence attest <dir>` — in-toto Statement in a DSSE envelope, signed with an ed25519 seed, for `cosign` / `in-toto-verify`
- `boruna evidence anchor <dir>` — records a signed bundle's `bundle_hash` in a Sigstore Rekor transparency log for an external witness and timestamp
- `boruna evidence otel <dir>` — exports the run as OTLP/JSON spans

## What This Proves

For compliance auditors, an evidence bundle demonstrates:
- **What ran**: Exact workflow definition and policy
- **When it ran**: Timestamps and run ID (independently witnessed only if anchored)
- **Where it ran**: Environment fingerprint
- **What happened**: Hash-chained record of every step, approval decision and output hash
- **Integrity**: SHA-256 hash chain and file checksums detect modification; an ed25519 signature ties the bundle to a key
