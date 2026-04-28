# Evidence Bundle Format Specification — version 1.0

**Status:** stable
**Format version:** `1.0`
**Reader contract:** semver-like — `1.x` is forward-compatible for any `1.0` reader; `2.x` is breaking and MUST be rejected.

This document is the source of truth for the on-disk layout, integrity contract, and version semantics of evidence bundles emitted by `boruna workflow run --record` and validated by `boruna evidence verify`.

---

## 1. Format-version semantics

Every bundle declares its format in a top-level `bundle.json` file. Readers MUST gate on this field before reading any other content.

| Bundle `format_version` | 1.0 reader | 1.x reader (x ≥ 1) | 2.x reader |
|-------------------------|-----------|--------------------|------------|
| missing / pre-1.0       | reject (legacy) | reject (legacy) | reject |
| `"1.0"`                 | accept    | accept             | reject |
| `"1.5"`                 | accept (forward-compat) | accept | reject |
| `"2.0"`                 | reject (incompatible major) | reject | accept |

Compatibility rules:

- **Same major:** unknown fields are ignored. A `1.0` reader presented with a `1.5` bundle MUST treat it as valid and silently drop fields it doesn't recognize.
- **Different major:** the wire format is allowed to change in incompatible ways. Readers MUST refuse to interpret content from a major they don't know.
- **Missing `bundle.json`:** the bundle is pre-1.0 (legacy). Readers MUST reject and surface a hint pointing the user at `boruna migrate evidence-bundle` (planned, sprint W5-C).

This applies §1 of the project conventions: "reject at parse, don't silently override". A reader that silently accepts an unknown major would let a future bundle's content be misinterpreted as the format the reader expects.

## 2. `bundle.json` schema

```json
{
  "format_version": "1.0",
  "boruna_version": "0.6.0",
  "created_at": "2026-04-28T14:32:11.420Z",
  "run_id": "run-2026-04-28-abc123",
  "workflow_hash": "9c8a1f...",
  "components": [
    "audit_log.json",
    "env_fingerprint.json",
    "manifest.json",
    "outputs/",
    "policy.json",
    "workflow.json"
  ]
}
```

Field semantics:

| Field            | Type            | Required | Description |
|------------------|-----------------|----------|-------------|
| `format_version` | string          | yes      | Semver-like format version. The major component is the compat gate. |
| `boruna_version` | string          | yes      | Version of the Boruna binary that emitted the bundle (`CARGO_PKG_VERSION`). Diagnostic, not a compat gate. |
| `created_at`     | string (RFC3339, UTC) | yes | Wall-clock time the bundle was finalized. |
| `run_id`         | string          | yes      | Workflow run identifier, matches `manifest.run_id`. |
| `workflow_hash`  | string (hex)    | yes      | SHA-256 of the workflow definition JSON, matches `manifest.workflow_hash`. |
| `components`     | string[]        | yes      | Sorted list of component file/directory names actually present in this bundle. Trailing `/` denotes a directory. Diagnostic; readers MUST NOT rely on this list to drive parsing. |
| `encryption`     | object          | no       | **Additive in 1.x** (sprint `W6-B`). When present in `manifest.json`, the bundle's file contents are AES-256-GCM envelope-encrypted. See §9. Absent → plaintext bundle (original 1.0 behavior). |

Future minor versions (`1.x`) may add optional fields. Readers MUST tolerate them.

## 3. Bundle directory layout (1.0)

```
<bundle-dir>/
├── bundle.json             # version gate (this spec)
├── manifest.json           # cryptographic manifest with file checksums + bundle_hash
├── workflow.json           # snapshot of the workflow definition
├── policy.json             # snapshot of the active policy
├── audit_log.json          # hash-chained event log
├── env_fingerprint.json    # OS / arch / boruna_version captured at run time
└── outputs/
    └── <step_id>/
        └── <output_name>.json   # per-step JSON outputs (compact form)
```

`bundle.json` is the LAST file written during finalize. Every other component is written and (where applicable) parent-dir fsynced before `bundle.json` is committed via the same atomic-rename + parent-dir-fsync pattern used by `workflow::data_flow::DataStore::store_output`. Consequence: a reader observing `bundle.json` is guaranteed to observe a complete bundle.

### 3.1 Component contracts

| Component | Contract |
|-----------|----------|
| `manifest.json` | `BundleManifest` (see `orchestrator/src/audit/evidence.rs`). Carries `file_checksums: BTreeMap<filename, sha256>` for every other file (excluding `bundle.json` and `manifest.json` itself). |
| `workflow.json` | The workflow definition as submitted, byte-for-byte. `workflow_hash = sha256(workflow.json)`. |
| `policy.json`   | The policy snapshot. `policy_hash = sha256(policy.json)`. |
| `audit_log.json`| `AuditLog` JSON; chain integrity is independently verifiable via `AuditLog::verify`. |
| `env_fingerprint.json` | OS / arch / `CARGO_PKG_VERSION` of the recording binary. |
| `outputs/<step>/<name>.json` | Compact JSON; same bytes that `DataStore::hash_value` hashed and that the orchestrator's SQLite checkpoint persisted. `sha256sum` MUST match the `output_hash` recorded in the audit log. |

## 4. Hash-chain integrity contract

Independent of the format gate, `verify_bundle` enforces:

1. Every entry in `manifest.file_checksums` matches the SHA-256 of the on-disk file.
2. `audit_log.json` parses as a valid `AuditLog`, and every entry's `entry_hash` is `sha256(prev_hash || event_json)`. The chain is broken iff any entry fails this check.
3. `audit_log.hash()` (last entry's `entry_hash`) equals `manifest.audit_log_hash`.
4. All required components from §3 are present.

A bundle that fails any of (1)–(4) is INVALID. The `verify_bundle` reader emits the failing checks but does not attempt to "repair" — that is reserved for `boruna migrate`.

## 5. Reader compat matrix

| Feature                       | 1.0 reader | 1.x reader (x ≥ 1) |
|-------------------------------|------------|---------------------|
| Read `1.0` bundle             | yes        | yes                 |
| Read `1.x` bundle (x > 0)     | yes (drops unknown fields) | yes |
| Read pre-1.0 / legacy bundle  | no (reject + migration hint) | no |
| Read `2.x` bundle             | no         | no                  |

Producers MUST emit the lowest format version that contains every field they need; this maximizes the population of readers that can consume the bundle.

## 6. Future evolution (non-normative)

The 0.5-S7 retro flagged the **sidecar layout for output blob references** as the leading driver of a future `1.1` minor bump. Rationale: large LLM step outputs are stored content-addressed (`/api/runs/{id}/blobs/{hash}`) since 0.5-S7. Bundles currently inline-resolve the bytes into `outputs/<step>/result.json`. A `1.1` bundle would optionally carry a `blobs/<sha256>` sidecar directory and rewrite `outputs/<step>/result.json` to a `{ "$blob_ref": "<hash>" }` reference. A `1.0` reader would (correctly) reject the rewritten `outputs` JSON unless it understands `$blob_ref`; therefore the sidecar layout is a `1.x` field-level extension rather than a parse-time break — provided producers continue to emit the inline form when no blobs are referenced. The compat matrix is preserved.

A `2.0` break — for example, switching from JSON to a binary-framed format — is not currently planned and would require an ADR plus a migration path through `boruna migrate evidence-bundle`.

## 7. Migration

Bundles produced by Boruna v0.5.0 and earlier do NOT carry `bundle.json`. The reader rejects them with:

```
unsupported evidence bundle format_version: found `missing bundle.json (legacy bundle from pre-1.0 release; use `boruna migrate evidence-bundle` to upgrade)`, expected major `1`
```

The `boruna migrate evidence-bundle` tool is planned for sprint W5-C. Until it ships, legacy bundles must be re-recorded against a current binary.

## 8. Encryption (additive 1.x)

The optional `encryption` field in `manifest.json` carries the metadata
for AES-256-GCM envelope encryption (sprint `W6-B`). When absent, the
bundle is plaintext (the original 1.0 behavior). When present, file
contents in the bundle directory are AES-256-GCM ciphertext keyed off
a fresh per-bundle data-encryption key (DEK) which is itself wrapped
with an operator-supplied key-encryption-key (KEK).

Field shape:

| Field | Type | Required | Description |
|-------|------|:--------:|-------------|
| `algorithm` | string | yes | Locked to `"aes-256-gcm"` for 1.x |
| `kek_id` | string | yes | Operator-supplied identifier; bound as AAD into the wrapped DEK |
| `wrapped_dek` | base64 | yes | DEK encrypted with the KEK |
| `wrapped_dek_nonce` | base64 | yes | Nonce for the DEK wrap |
| `files` | string[] | no | Operational hint listing encrypted files; readers MUST NOT use this to drive parsing |

`algorithm`, `kek_id`, `wrapped_dek`, and `wrapped_dek_nonce` are
replay-verified — they live inside `manifest.json` and therefore
participate in `bundle_hash`. `files` is OPERATIONAL metadata (per
project convention §15): it is informational; the canonical set of
encrypted files is derivable from `manifest.file_checksums` and
tampering with `files` cannot bypass decryption (the verify loop
iterates `file_checksums`, not `encryption.files`).

Per-file nonces are deterministic `SHA-256(filename)[..12]`. This is
safe because the DEK is freshly generated per bundle and filenames are
unique within a bundle, so a (DEK, nonce) pair never repeats. Each
per-file ciphertext carries an AES-GCM authentication tag; tag failure
on read surfaces as `evidence.cipher_tag_invalid`.

### 8.1 Reader contract for encryption

A 1.x reader:

- MUST accept bundles WITHOUT `encryption` (plaintext path; original
  1.0 behavior).
- MUST accept bundles WITH `encryption` if a KEK is available.
- MUST reject `encryption.algorithm` values other than `"aes-256-gcm"`
  with `evidence.unsupported_algorithm`.
- MUST surface DEK-unwrap failure as `evidence.encryption_key_mismatch`
  and a missing KEK as `evidence.encryption_key_required`.
- MUST surface per-file tag mismatch as `evidence.cipher_tag_invalid`
  and refuse to return decrypted bytes from the failing file.
- MUST NOT log the unwrapped DEK at any severity.

### 8.2 Reject-at-parse contract (§1)

Per project convention §1 ("reject at parse, don't silently override"),
a reader MUST refuse to interpret a bundle when:

- `encryption` is present but `algorithm` is anything other than
  `"aes-256-gcm"` → `evidence.unsupported_algorithm`.
- `encryption` is present but a required field
  (`kek_id`, `wrapped_dek`, `wrapped_dek_nonce`) is missing or
  malformed (non-base64, wrong length) → reader-defined parse error.
- `encryption` is present but no KEK has been supplied to the reader
  → `evidence.encryption_key_required`.

A 1.0 reader (pre-W6-B) that does not know the `encryption` field will
ignore it as an unknown field per §1 of this spec, then fail integrity
verification because the on-disk bytes hash to ciphertext rather than
plaintext referenced by `manifest.file_checksums`. Operators using a
1.0 reader against an encrypted bundle MUST upgrade to a 1.x reader
that understands `encryption`. This is the documented compat story per
§B.3 of the LTS contract (`docs/lts.md`).

## 9. References

- Implementation: `orchestrator/src/audit/evidence.rs` (`BundleJson`, `EvidenceBundleBuilder::finalize`)
- Reader gate: `orchestrator/src/audit/verify.rs` (`check_bundle_format`, `verify_bundle`)
- Constant: `orchestrator/src/audit/mod.rs` (`BUNDLE_FORMAT_VERSION`)
- Concept doc: `docs/concepts/evidence-bundles.md`
- CLI surface: `boruna evidence verify`, `boruna evidence inspect [--json]`
