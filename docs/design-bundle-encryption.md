# Evidence Bundle Encryption (sprint W6-B)

## Status

Shipped in 1.0.0-rc1 follow-up. Additive to the W1-C bundle format
(`schema_version: 1`). Plaintext bundles continue to work unchanged.

## Goal

Compliance-sensitive operators can encrypt evidence bundles at rest
without trusting filesystem permissions. A snapshot of the disk, a
backup tape leak, or casual inspection by an unprivileged user no
longer reveals workflow inputs, outputs, audit trails, or policy
snapshots.

## Non-goals (explicit)

- **Key management.** Boruna does not store, rotate, or derive
  KEKs. Operators supply the KEK out-of-band — HSM, KMS, sealed
  config, password manager, whatever fits their threat model.
- **Key rotation tooling.** Operators rotate by re-encrypting old
  bundles via a future migration tool (out of scope for this
  sprint).
- **KMS / cloud-KMS integration.** Out of scope.
- **Streaming encryption** for very large bundles. Bundles fit in
  memory; AES-GCM is applied per-file in one shot.
- **Client-side decryption tooling for non-Rust readers.** The
  format is documented here; a Python or shell decoder is left to
  downstream consumers.

## Threat model

### What this protects against

- **Filesystem snapshots / backup tape leaks.** A snapshot of
  `<data-dir>/evidence/<run-id>/` reveals only ciphertexts and the
  manifest. Manifest reveals workflow name, hashes, timestamps, and
  the `kek_id` — not plaintext payloads.
- **Casual inspection** by an operator without the KEK. The
  manifest shows `encryption.algorithm = "aes-256-gcm"` and the
  filenames (operational metadata), but the contents are
  unrecoverable without the KEK.
- **Tamper detection.** AES-GCM's authentication tag fails if any
  byte of any encrypted file is changed; verify surfaces this as
  `evidence.cipher_tag_invalid`. Existing SHA-256 checksums in
  `manifest.json` provide a second layer (and detect tampering of
  the manifest itself).

### What this does NOT protect against

- **RAM dumps during workflow execution.** The KEK and DEK live in
  process memory while the workflow runs. A core dump or a
  privileged debugger sees them.
- **KEK exfiltration from the operator's environment.** If the
  attacker has the KEK, they have the bundles. Boruna assumes the
  operator's KEK lifecycle is sound.
- **Manifest leakage of workflow names, timestamps, hashes,
  kek_id.** These are intentionally plaintext — the verifier needs
  them before it can decide whether to attempt decryption.
- **Side-channel timing leaks** on the operator's box. AES-GCM is
  constant-time on hardware that supports AES-NI; we rely on the
  `aes-gcm` crate's invariants.

## Format

Each encrypted bundle has the same on-disk shape as a plaintext
bundle, plus an additive `encryption` block in `manifest.json`:

```json
{
  "schema_version": 1,
  "...": "...",
  "file_checksums": {
    "audit_log.json": "<sha256 of PLAINTEXT>",
    "workflow.json":  "<sha256 of PLAINTEXT>",
    "...": "..."
  },
  "encryption": {
    "algorithm": "aes-256-gcm",
    "kek_id": "k-prod-2026-q2",
    "wrapped_dek": "<base64 of DEK encrypted under KEK>",
    "wrapped_dek_nonce": "<base64 of 12 bytes>",
    "files": ["audit_log.json", "outputs/s1/result.json", "..."]
  }
}
```

- **DEK** (data-encryption key): 32 random bytes, fresh per bundle.
- **KEK** (key-encryption-key): 32 bytes supplied by the operator
  via `--bundle-encryption-key <hex>` or the `BORUNA_BUNDLE_KEK`
  env var.
- **DEK wrapping**: AES-256-GCM with a fresh 12-byte nonce; AAD is
  the `kek_id` so a swapped manifest still fails authentication.
- **File encryption**: AES-256-GCM with a deterministic per-file
  nonce: `nonce = SHA-256(filename)[..12]`. This avoids storing
  per-file nonces in the manifest. The DEK is fresh per bundle, so
  per-file nonces never repeat across bundles. Filenames are
  unique within a bundle (operations differ across files), so the
  (key, nonce) pair is unique. AAD is empty.
- **Checksums** in `file_checksums` are SHA-256 over the
  **plaintext**, NOT the ciphertext. Verify decrypts then hashes,
  matching the existing W1-C verify path.
- **`manifest.json` itself is plaintext.** The verifier needs to
  read it before it can find the wrapped DEK.

## Replay-verified vs operational classification

Per project-conventions §15:

| Field | Class |
|-------|-------|
| `encryption.algorithm` | REPLAY-VERIFIED — feeds `bundle_hash` |
| `encryption.kek_id` | REPLAY-VERIFIED — feeds `bundle_hash` |
| `encryption.wrapped_dek` | REPLAY-VERIFIED — feeds `bundle_hash` |
| `encryption.wrapped_dek_nonce` | REPLAY-VERIFIED — feeds `bundle_hash` |
| `encryption.files` | OPERATIONAL — informational |

`encryption.files` is informational because it's derivable from the
`file_checksums` map already in the manifest, and we don't want a
non-substantive metadata field to invalidate the bundle hash.

## CLI surface

```sh
# Generate a 32-byte KEK as 64 hex chars (operator's responsibility):
KEK=$(openssl rand -hex 32)
export BORUNA_BUNDLE_KEK="$KEK"

# Encrypt a bundle on the way out.
boruna workflow run ./wf --policy allow-all --record \
    --encrypt-bundle --bundle-kek-id k-prod-2026-q2

# Verify (env var) — auto-detects encryption.
boruna evidence verify ./wf/evidence/<run-id>

# Verify (explicit flag).
boruna evidence verify ./wf/evidence/<run-id> \
    --bundle-encryption-key "$KEK"

# Inspect — refuses to print decrypted bodies unless --decrypt is
# passed.
boruna evidence inspect ./wf/evidence/<run-id>
```

## Errors

| Error kind | When | Caller-actionable |
|------------|------|------|
| `evidence.encryption_key_required` | Bundle has `encryption`, no KEK supplied | Set env or pass flag |
| `evidence.encryption_key_mismatch` | Wrong KEK for the wrapped DEK | Operator looked up wrong key |
| `evidence.cipher_tag_invalid` | AES-GCM tag failed on a file | Bundle tampered or wrong key |

## KEK lifecycle (operator's responsibility)

Boruna does NOT manage keys. Recommended patterns:

- **HSM-backed KEK**: store the 32-byte KEK in a hardware security
  module; export only when needed; pass via env var that's wiped
  after the call.
- **KMS**: pull the KEK from your cloud KMS at run time, hand to
  Boruna via `--bundle-encryption-key`, then drop.
- **Sealed config**: `sops`, `age`, `vault` — decrypt the KEK at
  process start, hand to Boruna via env, never write to disk.

## Key rotation (out of scope for W6-B)

Operators rotate by:

1. Decrypt old bundle with old KEK (via a future
   `boruna evidence rotate-key` command — not in this sprint).
2. Re-encrypt with new KEK.
3. Update `kek_id` in the manifest, recompute `bundle_hash`.

The migration tool will be additive when shipped; the manifest
shape supports it today.

## Why not encrypt the manifest too?

The manifest carries the wrapped DEK and the algorithm
identifier — both are needed to decide whether to attempt
decryption and which KEK to look up. Encrypting the manifest would
require a chicken-and-egg pre-manifest header. The manifest's
plaintext fields (workflow name, timestamps, hashes) are not
sensitive in compliance contexts (they prove a workflow ran;
that's the point of an evidence bundle). If an operator needs to
hide even the workflow name, they should encrypt at the
filesystem layer (LUKS, EncFS) on top of bundle encryption.

## Test coverage

Unit (`orchestrator/src/audit/encryption.rs` mod tests):

- Round-trip encrypt/decrypt with a fixed KEK
- Unwrap with wrong KEK fails with `EncryptionKeyMismatch`
- Tampered ciphertext fails with `CipherTagInvalid`
- Wrong-filename decrypt fails (per-file nonce binds the file)
- KEK hex parsing length / non-hex rejection

Integration (`orchestrator/tests/bundle_encryption.rs`):

- `bundle_encrypts_and_verifies_round_trip`
- `bundle_verify_fails_with_wrong_kek`
- `bundle_verify_fails_with_no_kek`
- `bundle_tampered_ciphertext_fails_at_verify`
- `bundle_without_encryption_field_still_works` (W1-C backwards-compat)
- `bundle_inspect_refuses_to_print_decrypted_without_flag`
- `encrypted_bundle_format_version_is_1_0`
