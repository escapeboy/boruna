# Boruna Runtime-Provenance Predicate 1.0

`predicateType: https://boruna.dev/runtime-provenance/v1`

This spec defines an **interop** view of a Boruna evidence bundle: the same
provenance the native bundle already records (`manifest.json`), re-emitted as a
standard [in-toto Statement](https://github.com/in-toto/attestation) wrapped in a
[DSSE](https://github.com/secure-systems-lab/dsse) envelope so that off-the-shelf
supply-chain tooling (`cosign verify-blob`, `in-toto-verify`) can consume Boruna
evidence.

It is **additive and non-breaking**. Producing an attestation does not modify the
bundle, its `manifest.json`, its `bundle_hash`, or the existing
`evidence verify` path. See `docs/spec/evidence-bundle-1.0.md` for the native
format that remains the source of truth.

The producer/verifier lives in `orchestrator/src/audit/attestation.rs`; the CLI
surface is `boruna evidence attest`.

---

## 1. Artifacts

Two nested artifacts are produced from a finalized `BundleManifest`:

1. an **in-toto Statement v1**, and
2. a **DSSE envelope** wrapping that Statement, signed with the SAME ed25519 key
   used for manifest signing (`EvidenceBundleBuilder::with_signing_key`). No new
   keypair is introduced; the DSSE `keyid` is the lowercase-hex ed25519 public
   key, identical to `manifest.signature.public_key`.

The envelope is written to `<bundle-dir>/attestation.intoto.dsse.json`.

---

## 2. Statement shape

```jsonc
{
  "_type": "https://in-toto.io/Statement/v1",
  "subject": [
    { "name": "<component-file-path>", "digest": { "sha256": "<hex>" } },
    // ... one per manifest.file_checksums entry ...
    { "name": "boruna-bundle:<run_id>", "digest": { "sha256": "<bundle_hash>" } }
  ],
  "predicateType": "https://boruna.dev/runtime-provenance/v1",
  "predicate": { /* see §3 */ }
}
```

### Subjects

`subject[]` is the set of artifacts this attestation makes claims about:

- **Every component file** the manifest checksums — reusing
  `manifest.file_checksums` verbatim (name → `sha256`). These include
  `workflow.json`, `policy.json`, `audit_log.json`, `env_fingerprint.json`,
  per-step `outputs/<step>/<name>.json`, and any optional components
  (`intents.json`, `model_invoking_steps.json`, `event_log.json`).
- **The bundle itself**, as a synthetic subject `boruna-bundle:<run_id>` whose
  `sha256` is the manifest's `bundle_hash`. (`bundle_hash` is the SHA-256 of the
  canonicalized manifest, so it is a digest, not a file — but it lets a verifier
  bind the whole bundle by a single value.)

`file_checksums` is a `BTreeMap`, so subjects are emitted in sorted-name order —
the Statement bytes are deterministic for a given manifest.

---

## 3. Predicate schema

The predicate borrows the shape of
[SLSA Provenance v1](https://slsa.dev/provenance/v1)'s `buildDefinition` /
`runDetails`, mapping the existing manifest fields:

```jsonc
{
  "buildDefinition": {
    "buildType": "https://boruna.dev/workflow-run/v1",
    "externalParameters": {
      "workflowName": "<manifest.workflow_name>",
      "workflowHash": "<manifest.workflow_hash>",
      "policyHash":   "<manifest.policy_hash>"
    },
    "internalParameters": {
      "borunaVersion": "<producing boruna version>",
      "envFingerprint": { /* manifest.env_fingerprint, verbatim */ }
    }
  },
  "runDetails": {
    "builder": { "id": "https://boruna.dev/boruna@<version>" },
    "metadata": {
      "invocationId": "<manifest.run_id>",
      "startedOn":    "<manifest.started_at>",
      "finishedOn":   "<manifest.completed_at>"
    },
    "byproducts": {
      "auditLogHash": "<manifest.audit_log_hash>",
      "bundleHash":   "<manifest.bundle_hash>"
    }
  }
}
```

### Field mapping (manifest → predicate)

| Manifest field        | Predicate location                                   |
|-----------------------|------------------------------------------------------|
| `workflow_name`       | `buildDefinition.externalParameters.workflowName`    |
| `workflow_hash`       | `buildDefinition.externalParameters.workflowHash`    |
| `policy_hash`         | `buildDefinition.externalParameters.policyHash`      |
| `env_fingerprint`     | `buildDefinition.internalParameters.envFingerprint`  |
| (build version)       | `buildDefinition.internalParameters.borunaVersion`   |
| `run_id`              | `runDetails.metadata.invocationId`                   |
| `started_at`          | `runDetails.metadata.startedOn`                      |
| `completed_at`        | `runDetails.metadata.finishedOn`                     |
| `audit_log_hash`      | `runDetails.byproducts.auditLogHash`                 |
| `bundle_hash`         | `runDetails.byproducts.bundleHash`                   |

**Capability set and contract-check results** are not manifest struct fields —
they live in bundle *components* (`event_log.json` carries `ContractCheck`
events; `model_invoking_steps.json` lists steps that reached an `llm.*`
capability). Those components appear as **subjects** (by SHA-256) rather than
being inlined into the predicate, so the attestation still binds them without
duplicating or re-parsing their contents. A consumer that wants contract-check
detail dereferences the `event_log.json` subject and reads it from the bundle.

All maps are `BTreeMap` and struct fields serialize in declaration order, so the
Statement is byte-stable: same run → same manifest → same Statement bytes → same
signature.

---

## 4. DSSE envelope

```jsonc
{
  "payload": "<base64(canonical Statement JSON bytes)>",
  "payloadType": "application/vnd.in-toto+json",
  "signatures": [
    { "sig": "<base64(ed25519 sig over PAE)>", "keyid": "<hex ed25519 pubkey>" }
  ]
}
```

The signature is an ed25519 signature over the DSSE **Pre-Authentication
Encoding (PAE)** of `(payloadType, payload)`:

```
PAE(type, body) = "DSSEv1" SP LEN(type) SP type SP LEN(body) SP body
```

where `SP` is a single ASCII space (`0x20`) and `LEN` is the ASCII-decimal byte
length. `type` is `application/vnd.in-toto+json` and `body` is the **raw**
Statement JSON bytes (the pre-base64 bytes), not the base64 text.

**Known vector** (from the DSSE spec's worked example, asserted in
`attestation.rs` tests):

```
PAE("http://example.com/HelloWorld", "hello world")
  = "DSSEv1 29 http://example.com/HelloWorld 11 hello world"
```

The `payloadType` is bound into the signed bytes, so a signature over one payload
type cannot be replayed under another.

---

## 5. CLI

```bash
# Produce: sign the manifest's provenance into a DSSE envelope.
#   Reuses the SAME ed25519 seed you signed the bundle with.
boruna evidence attest <bundle-dir> --signing-key <64-hex-seed>
#   (or set BORUNA_BUNDLE_SIGNING_KEY instead of --signing-key)
# → writes <bundle-dir>/attestation.intoto.dsse.json

# Verify: check the DSSE signature over the PAE.
boruna evidence attest <bundle-dir> --verify
# Optionally pin the trusted signer key:
boruna evidence attest <bundle-dir> --verify --verify-key <64-hex-pubkey>
```

`--verify` exits non-zero on any failure (bad signature, mutated payload, wrong
`payloadType`, or a pinned key that made no valid signature).

---

## 6. Ecosystem compatibility

The envelope is a standard DSSE envelope with `payloadType`
`application/vnd.in-toto+json` and a standard in-toto Statement payload, so it is
structurally consumable by the wider ecosystem:

- **`cosign verify-blob-attestation`** consumes a DSSE envelope + a public key and
  verifies the ed25519 signature over the PAE. Export the signer's public key in
  PEM form (the DSSE `keyid` here is the raw 32-byte ed25519 public key as hex;
  cosign expects a PEM `PUBLIC KEY`, so wrap the key in SubjectPublicKeyInfo DER →
  PEM before handing it to cosign). The signature algorithm (ed25519 over PAE)
  and envelope layout match what cosign verifies.
- **`in-toto-verify` / the in-toto attestation validators** parse the Statement
  (`_type`, `subject`, `predicateType`, `predicate`) directly; the predicate is a
  custom type, so policy is expressed against `predicateType ==
  https://boruna.dev/runtime-provenance/v1` and the fields in §3.

**Honest caveat.** The bytes and algorithms follow the DSSE and in-toto specs, and
Boruna verifies its own envelopes end-to-end (round-trip, PAE known-vector, and
tamper tests). Full black-box interop with a specific `cosign` / `in-toto`
*release* — including the exact public-key PEM/DER encoding each tool wants and
any tool-specific envelope expectations — has **not** been exercised against those
binaries in this change; treat cross-tool verification as "spec-conformant,
pending a live `cosign`/`in-toto-verify` integration check." The key-encoding
bridge (raw hex ed25519 → SPKI PEM) is the most likely point of friction.
