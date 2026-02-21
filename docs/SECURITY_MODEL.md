# Security Model

## Capability System

All side effects in Boruna are declared and enforced. Functions annotate their capabilities:

```
fn fetch(url: String) -> String !{net.fetch} { ... }
```

The VM's `CapabilityGateway` checks every capability call against the active `Policy`. Undeclared capabilities are blocked at compile time; unauthorized capabilities are blocked at runtime.

### Built-in Capabilities
- `net.fetch` — HTTP requests
- `db.query` — Database queries
- `fs.read`, `fs.write` — File system access
- `llm.prompt` — LLM model invocation
- `actor.spawn`, `actor.send` — Actor system operations

## Isolation

### Process Isolation
Each workflow step compiles to bytecode and runs in a fresh VM instance. Steps cannot share memory or state except through the explicit data flow system.

### Filesystem Isolation
- `PatchBundle` validates against `..` and absolute paths
- `canonicalize()` defense-in-depth prevents path traversal
- Evidence bundle outputs are written to controlled directories

### Data Validation
- LLM cache keys are hex-only validated
- Context store hashes are hex-only validated
- Package content uses SHA-256 integrity verification

## Policy Enforcement

Policies can restrict:
- Which capabilities a step may use
- Which LLM models may be invoked
- Which network endpoints are reachable
- Token and call budgets per step

Policy violations are recorded in the audit log with deny decisions.

## Secrets Management (Gap)

Currently there is no dedicated secrets management system. Secrets should NOT be passed as environment variables in production. This is documented as a P1 gap. The recommended interim approach:
- Use external secret managers (Vault, AWS Secrets Manager)
- Pass secrets via capability handlers at runtime
- Never embed secrets in workflow definitions or source files

## Threat Model

### In Scope
- **Malicious workflow steps**: Capability gating prevents unauthorized IO
- **Tampered evidence**: Hash-chained audit log and bundle checksums detect modification
- **Path traversal**: Validated at multiple layers
- **Non-determinism**: BTreeMap ordering, controlled randomness, replay verification

### Out of Scope (current)
- Network-level attacks (TLS termination is external)
- Host OS compromise
- Supply chain attacks on the Boruna binary itself
- Multi-tenant isolation (single-tenant model currently)

## Audit Trail

Every policy decision, capability invocation, and approval action is recorded in the hash-chained audit log. The chain is cryptographically verifiable:
- Each entry hashes: sequence number + previous hash + event data
- Tampering with any entry breaks the chain from that point forward
- `boruna evidence verify` checks chain integrity

## Digital Signatures (Gap)

Evidence bundles use SHA-256 checksums but do not yet support Ed25519 digital signatures. This is documented as a P2 gap.
