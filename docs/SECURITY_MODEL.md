# Security Model

Boruna is a local engine and CLI. Since v3.0.0 there is no HTTP server, coordinator, worker or dashboard, so there is no network-facing service to secure. This document covers what the engine itself enforces. For the evidence-specific threat model see [concepts/threat-model.md](./concepts/threat-model.md).

## Capability System

All side effects in Boruna are declared and enforced. Functions annotate their capabilities:

```
fn fetch(url: String) -> String !{net.fetch} { ... }
```

The compiler records each function's declared capabilities in the bytecode. At runtime the VM rejects a capability call from a function that did not declare it (`CapabilityDenied`), and the `CapabilityGateway` checks every remaining call against the active `Policy`.

### Built-in Capabilities
- `net.fetch` — HTTP requests
- `db.query` — Database queries
- `fs.read`, `fs.write` — File system access
- `llm.call` — LLM model invocation
- `time.now` — Current time
- `random` — Randomness
- `ui.render` — Framework view output
- `actor.spawn`, `actor.send` — Actor system operations
- `step.input` — Read a workflow step's resolved inputs

## Isolation

### Process Isolation
Each workflow step compiles to bytecode and runs in a fresh VM instance. Steps cannot share memory or state except through the explicit data flow system.

### Filesystem Isolation
- `PatchBundle` rejects `..`, absolute, drive-letter and rooted paths on every platform
- `canonicalize()` defense-in-depth prevents path traversal
- Evidence bundle outputs are written to controlled directories

### Data Validation
- LLM cache keys are hex-only validated
- Context store hashes are hex-only validated
- Package content uses SHA-256 integrity verification

## Policy Enforcement

Policies can restrict:
- Which capabilities may be used, with a per-capability call budget
- Which network domains and HTTP methods are reachable, response size, timeout and redirects (`net_policy`)

A denied call is a runtime error (`CapabilityDenied`, or `CapabilityBudgetExceeded` when the budget is used up). Check a policy file with `boruna policy validate` / `boruna policy show` (see [reference/policy-schema.md](./reference/policy-schema.md)).

The `boruna-effect` crate also has an LLM policy (allowed models, total token budget). It is a library API; the CLI policy file does not set it.

## Secrets Management (Gap)

There is no dedicated secrets management system. The bundle encryption key (`--bundle-encryption-key` / `BORUNA_BUNDLE_KEK`) and the ed25519 signing seed (`--signing-key` / `BORUNA_BUNDLE_SIGNING_KEY`) are passed as flags or environment variables. Recommended approach:
- Use external secret managers (Vault, AWS Secrets Manager) and inject keys at invocation time
- Pass secrets via capability handlers at runtime
- Never embed secrets in workflow definitions or source files

## Threat Model

### In Scope
- **Malicious workflow steps**: Capability gating prevents unauthorized IO
- **Tampered evidence**: Hash-chained audit log, per-file checksums and `bundle_hash` detect modification
- **Path traversal**: Validated at multiple layers
- **Non-determinism**: BTreeMap ordering, `time.now` and `random` gated as capabilities, replay verification

### Out of Scope (current)
- Host OS compromise
- Supply chain attacks on the Boruna binary itself (release archives are checked against `SHA256SUMS` by the installers, but binaries are not signed)
- Multi-tenant isolation (single-tenant model)
- A stolen signing key: a bundle signed with it verifies (see [concepts/threat-model.md](./concepts/threat-model.md))

## Audit Trail

Workflow runs recorded with `--record` produce an evidence bundle with a hash-chained audit log. It records workflow start and completion, step completion and failure, approval decisions, external triggers and confidence-gate decisions. Individual capability calls are recorded in the VM event log (`boruna run --record <file>`), not in the bundle's audit log.

The chain is verifiable:
- `content_sha256 = SHA-256(event_json)`; `entry_hash = SHA-256(sequence || prev_hash || content_sha256)` (bundle format 1.1; 1.0 bundles hash the event directly)
- Tampering with any entry breaks the chain from that point forward
- `boruna evidence verify <dir>` checks the chain, file checksums, required files, and the signature and encryption when present

## Digital Signatures

A manifest can carry an ed25519 signature over `bundle_hash`. Bundles are signed through the library API (`EvidenceBundleBuilder::with_signing_key`); `boruna workflow run` has no signing flag. `boruna evidence verify --verify-key <hex>` pins the trusted public key, and `--require-signature` rejects unsigned bundles. `boruna evidence attest` emits an in-toto + DSSE attestation signed with the same ed25519 seed.

## Encryption at Rest

`boruna workflow run --encrypt-bundle` encrypts bundle files with AES-256-GCM envelope encryption under a key-encryption key (KEK). `boruna evidence verify --bundle-encryption-key` decrypts in memory to verify, and `boruna evidence rotate-kek` re-wraps the data key under a new KEK without re-encrypting the files.

## Redaction

`boruna evidence redact <dir> --event <N> [--field <name>]` removes content (for example PII) from a sealed plaintext bundle. The entry keeps its `content_sha256`, so the chain and `audit_log_hash` stay valid and `evidence verify` still passes and reports the redacted entries. A prior manifest signature is dropped; re-sign or re-anchor afterward. Confidence-gate decisions cannot be redacted. See `orchestrator/docs/verifiable-redaction.md`.

## Transparency-Log Anchoring

`boruna evidence anchor <dir>` records a signed bundle's `bundle_hash` in a Sigstore Rekor log, which adds an external witness and timestamp. `--rekor-url` accepts a private Rekor instance, `--offline` emits the entry for out-of-band submission, and `--verify` checks a stored inclusion proof without network access. Live submission requires a build with the `rekor` cargo feature.
