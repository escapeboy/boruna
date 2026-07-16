# Boruna — Cross-Cutting Security Sweep (read-only)

**Scope:** whole-repo security pass focused on the product's existential claims —
capability safety and auditability. Every claim below cites `path:line` and is
tagged CONFIRMED (read the code, exploit path is real) or NEEDS-REVIEW (real code
behavior, but exploitability depends on deployment/threat-model assumptions I
could not fully close this pass). Read-only: no code was modified.

**Date:** 2026-07-16 · **Branch:** `ci/reduce-artifact-storage`

---

## Executive Risk Summary

Boruna's two load-bearing security stories are **(1) every side effect passes the
VM CapabilityGateway/Policy** and **(2) evidence bundles are tamper-evident**. The
sweep found the capability gateway itself is a clean, single choke point for
`CapCall`, and the encrypted-bundle path (AES-256-GCM under an operator KEK) is
cryptographically sound. But three findings cut at the core value proposition:

1. **Evidence bundles are not cryptographically signed.** The audit log is an
   *unkeyed* SHA-256 hash chain and the manifest's `bundle_hash` is a plain
   self-SHA-256 that `verify_bundle` never even checks. For **plaintext** bundles
   (the default), anyone with filesystem write access can rewrite outputs, the
   audit log, and every checksum, and `evidence verify` still returns PASS.
   Tamper-*evidence* only actually holds for **encrypted** bundles (attacker
   lacks the KEK) or if the operator anchors `bundle_hash` out-of-band — neither
   is on by default.

2. **SSRF in the live HTTP handler** via DNS-resolved hostnames and via
   redirects. `validate_url_safety` only blocks *literal* private IPs; a public
   hostname whose DNS points at `169.254.169.254` / `127.0.0.1`, or a public URL
   that 302-redirects there, is fetched. Cloud-metadata credential theft is the
   direct impact when `--live` + `http` feature are used with the default
   (empty = allow-all) domain allowlist.

3. **The distributed coordinator fails open.** With no `--shared-secret` and no
   mTLS it runs with auth entirely disabled on *every* route — including
   `/api/runs/{run_id}/approve`, `/api/runs/submit`, and `/api/work/*` — emitting
   only a startup warning on a non-loopback bind. The human-in-the-loop approval
   control (a compliance feature) is then bypassable by any network peer.

Mitigating facts that materially lower blast radius: **workers execute
coordinator-supplied `.ax` under a `MockHandler`**, so remote code from a rogue
coordinator cannot do real network/fs/db I/O (DoS/compute only, not RCE); the VM
is step-bounded (10M); div/mod-by-zero is guarded; PatchBundle, BlobStore,
LlmCache, and ContextStore path handling are all properly hardened; and
worker↔coord mTLS pins the CA with no `danger_accept_invalid_certs`.

---

## Findings Table

| id | area | sev | file:line | tag | one-line scenario |
|----|------|-----|-----------|-----|-------------------|
| F1 | Evidence tamper-evidence | **High** | orchestrator/src/audit/log.rs:189; evidence.rs:224 | CONFIRMED | Unkeyed SHA-256 chain + unsigned manifest → plaintext bundle fully forgeable by anyone with FS write; verify still PASSes |
| F2 | Evidence tamper-evidence | Med | orchestrator/src/audit/verify.rs:89-284 | CONFIRMED | `verify_bundle` never recomputes/checks `manifest.bundle_hash`; the field is decorative on the read path |
| F3 | SSRF | **High** | crates/llmvm/src/http_handler.rs:178-201 | CONFIRMED | Hostname resolving to a private IP (DNS rebinding / `metadata.google.internal`) bypasses the literal-IP-only check → metadata credential theft |
| F4 | SSRF | **High** | crates/llmvm/src/http_handler.rs:30-33,52; capability_gateway.rs:40-42,64 | CONFIRMED | `allow_redirects` defaults true; redirect targets are NOT re-validated → public URL 302→169.254.169.254 |
| F5 | Distributed auth | **High** | crates/llmvm-cli/src/coordinator.rs:265-291,737-774 | CONFIRMED | No secret + no mTLS = auth disabled on all routes (approve/submit/work); warn-only, fails open on non-loopback |
| F6 | Capability completeness | Med | crates/llmvm/src/vm.rs:526-546; capability_gateway.rs:184-187 | CONFIRMED | `SpawnActor`/`SendMsg` opcodes bypass `gateway.call`; `ActorSpawn`/`ActorSend` capabilities are unenforceable by Policy |
| F7 | Panic / DoS | Med | crates/llmvm/src/vm.rs:621,635 (Add/Sub/Mul) | CONFIRMED | Unchecked i64 arithmetic from crafted `.ax`: debug build panics on overflow, release wraps (silent, breaks determinism); `i64::MIN / -1` panics despite zero-guard |
| F8 | Panic / DoS | Med | crates/llmvm/src/vm.rs:580,603 | NEEDS-REVIEW | `module.functions[func_idx]` / constants indexing panics on a crafted bytecode `Module` — real only if bytecode is loaded from an untrusted source |
| F9 | Evidence tamper-evidence | Low | orchestrator/src/audit/verify.rs:192-224 | CONFIRMED | verify only iterates `manifest.file_checksums`; extra/unregistered files added to the bundle dir are never detected |
| F10 | Secrets / key hygiene | Low | orchestrator/src/audit/encryption.rs:105-108 | CONFIRMED | DEK `[u8;32]` and parsed KEK are never zeroized on drop (linger in process memory); Debug is redacted, but no `Zeroize` |
| F11 | Actor DoS | Low | crates/llmvm/src/vm.rs:526-530 | NEEDS-REVIEW | `SpawnActor` has no per-run actor-count cap visible; step budget bounds it but memory from a spawn-loop may balloon before the 10M limit |

---

## Per-Finding Detail

### F1 — Evidence bundles are not signed; the audit chain is an unkeyed hash chain (High, CONFIRMED)

The audit log hash is computed with a bare SHA-256 of public inputs, no secret/HMAC:

```rust
// orchestrator/src/audit/log.rs:189
fn compute_hash(sequence: u64, prev_hash: &str, event: &AuditEvent) -> String {
    let event_json = serde_json::to_string(event).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(sequence.to_le_bytes());
    hasher.update(prev_hash.as_bytes());
    hasher.update(event_json.as_bytes());
    format!("{:x}", hasher.finalize())
}
```

`bundle_hash` is likewise just `SHA-256(manifest-with-bundle_hash-blanked)`
(evidence.rs:224-227), and a grep for `signature|ed25519|hmac|sign_` across
`crates/` + `orchestrator/` returns **nothing** in the bundle path (only AWS
SigV4 in the S3 storage backend). 

**Exploit path (plaintext bundle — the default):** an attacker with write access
to the bundle directory edits `workflow.json` / `outputs/.../result.json`, rebuilds
the audit chain entry-by-entry (all inputs are in the file), recomputes every
`file_checksums` entry and `audit_log_hash`, and rewrites `manifest.json`.
`verify_bundle` recomputes those same hashes from the same manifest and returns
`valid: true`. There is no external root of trust to contradict the forged
manifest.

**Why it matters:** the product is sold on auditability/compliance evidence. A
hash chain with no signature and no externally-anchored root detects only
*accidental* corruption or tampering by a party who cannot recompute — i.e. nobody
who has the bundle. The genuinely tamper-*resistant* mode is envelope encryption
(encryption.rs): file bytes are AES-256-GCM under a DEK wrapped by an operator KEK,
so an attacker lacking the KEK cannot produce ciphertext that decrypts to chosen
plaintext (GCM tag fails at verify.rs:204). That path is sound — but it is
opt-in (`--bundle-encryption-key` / `BORUNA_BUNDLE_KEK`), and the manifest itself
(checksums, `audit_log_hash`) is still stored in plaintext. Recommendation:
sign the manifest (or at least `bundle_hash`) with an operator key, or document
loudly that plaintext bundles are integrity-checked but **not** tamper-evident.

### F2 — `verify_bundle` never checks `bundle_hash` (Med, CONFIRMED)

`verify_bundle`/`verify_bundle_with_kek` (verify.rs:89-284) does five things:
format gate, per-file checksums, audit-chain + `audit_log_hash`, required-files
existence. It **never** recomputes `bundle_hash` from the manifest and compares it
to `manifest.bundle_hash`. `compute_bundle_hash` exists (audit/rotate.rs:210) and
is used at build/rotate time, but not on the verify path. Even the manifest's own
self-consistency check is absent. Low standalone impact (it's an unsigned
self-hash, see F1), but it means a hand-edited manifest with a stale/blank
`bundle_hash` verifies fine.

### F3 — SSRF via DNS-resolved hostname (High, CONFIRMED)

```rust
// crates/llmvm/src/http_handler.rs:187
if let Ok(ip) = host.parse::<IpAddr>() {   // only fires for a LITERAL ip host
    if is_private_ip(&ip) { return Err(...); }
}
```

Validation runs on the URL *string*. When `host` is a domain name it does not
parse as `IpAddr`, so `is_private_ip` is never consulted; ureq then resolves the
name and connects to whatever IP DNS returns. An attacker supplies
`http://metadata.google.internal/…`, `http://169-254-169-254.sslip.io/…`, or an
attacker-owned domain with an A record of `169.254.169.254` / `127.0.0.1` /
`10.x`. Default `NetPolicy.allowed_domains` is empty = allow-all
(capability_gateway.rs:59-66; check at http_handler.rs:225-228 returns Ok on empty
allowlist), so nothing else stops it. Impact: cloud metadata credential theft,
internal service access. Only reachable on the `--live` + `http`-feature path
(the workflow runner's real handler); the default `MockHandler` is inert.
(Note: the decimal-integer-IP case the module comment at line 194 claims to handle
is actually covered incidentally by the `url` crate's WHATWG host normalization —
`http://2130706433/` normalizes to `127.0.0.1` and is caught — but hostname
resolution is the real, uncovered gap.) Fix: resolve the host and check every
resolved `IpAddr` against `is_private_ip` before connecting (and re-check on each
redirect hop, see F4).

### F4 — SSRF via unvalidated redirects (High, CONFIRMED)

`NetPolicy.allow_redirects` defaults `true` (capability_gateway.rs:53-55,64), and
`HttpHandler::new` only disables redirects when it is false (http_handler.rs:30-33).
`validate_url_safety` runs once, on the initial URL only (line 52). ureq then
follows 3xx responses to any `Location`, including `http://169.254.169.254/…` or
`http://127.0.0.1/…`, with no re-validation. A fully-public, allowlist-passing
initial URL can thus pivot into the internal network. Fix: set redirects to 0 and
re-run `validate_url_safety` + allowlist per hop manually, or disable redirects by
default.

### F5 — Coordinator auth fails open (High, CONFIRMED)

```rust
// crates/llmvm-cli/src/coordinator.rs:273
if shared_secret.is_none() && !mtls_required && !bind.is_loopback() {
    eprintln!("[WARNING] coordinator is bound to a non-loopback address with NO --shared-secret ...");
}
```

The startup only *warns*; it does not refuse to bind. `auth_middleware`
(coordinator.rs:737-774) is a pass-through when both `mtls_required` is false and
`shared_secret` is `None` — the final `next.run(request).await` is reached with no
check. The merged router exposes `/api/runs/{run_id}/approve`,
`/api/runs/{run_id}/trigger`, `/api/runs/submit`, and all `/api/work/*`
(coordinator.rs:799-816) under that same middleware. So an operator who binds
`0.0.0.0` without auth flags lets any network peer approve human-in-the-loop gates,
submit runs, and claim/complete/fail work — poisoning run state and evidence. It is
documented as "front with a reverse proxy" (module docs line 15-16), but failing
*open* (with a bypassable approval endpoint) is a dangerous default for a
compliance product. The auth primitives themselves are good: constant-time bearer
compare (coordinator.rs:700-709), mTLS via `WebPkiClientVerifier`
(coordinator.rs:360-367), and the two gates compose correctly. Recommendation:
refuse to start on a non-loopback bind without auth, or require an explicit
`--insecure-no-auth` acknowledgement.

### F6 — Actor opcodes bypass the capability gateway (Med, CONFIRMED)

`Op::SpawnActor` and `Op::SendMsg` (vm.rs:526-546) mutate `spawn_requests` /
`outgoing_messages` directly; only `Op::CapCall` (vm.rs:598-619) routes through
`self.gateway.call(...)`. `MockHandler` confirms the intent: "Actor ops are handled
at the opcode level, not through the gateway" (capability_gateway.rs:184-187). So
the `ActorSpawn`/`ActorSend` capabilities exist in the bytecode/framework but are
**never checked against `Policy`**. A `deny(ActorSpawn)` policy has no effect —
the VM spawns anyway. This is an over-declared/unenforced capability: the policy
surface claims coverage it does not deliver. Blast radius is limited (actors are
in-VM concurrency, not an external side effect), but it undercuts the "every
capability is policy-gated" invariant and enables F11.

### F7 — Unchecked integer arithmetic (Med, CONFIRMED)

`Op::Add`/`Op::Sub`/`Op::Mul` use bare `x + y` / `x - y` on `i64`
(vm.rs:621,635, and the Mul arm). From crafted `.ax`
(`9223372036854775807 + 1`): **debug** builds panic (overflow check) → VM process
aborts (DoS); **release** builds wrap silently → a "deterministic execution
platform" produces a platform/profile-dependent result, weakening the determinism
guarantee. `Op::Div`/`Op::Mod` correctly guard divide-by-zero (vm.rs:664-666,
685-687), but `i64::MIN / -1` still overflows past the zero-guard and panics.
Fix: `checked_add`/`checked_sub`/`checked_mul`/`checked_div` → `VmError`.

### F8 — Crafted bytecode Module panics via unchecked indexing (Med, NEEDS-REVIEW)

`self.module.functions[func_idx as usize]` (vm.rs:580 in Assert, 603 in CapCall)
and `self.module.constants.get(...)` patterns index directly. A hand-crafted
`Module` (deserialized bytecode) with an out-of-range `func_idx` triggers an
index-out-of-bounds panic. Severity hinges on whether a serialized bytecode
`Module` is ever loaded from an untrusted source — the worker path compiles `.ax`
*source* (worker.rs:420), not raw bytecode, and I did not find a
network/file bytecode-load path this pass. If one exists (or is added), this is a
DoS. Flagged for the bytecode/VM owner to confirm the trust boundary.

### F9 — Extra bundle files are invisible to verify (Low, CONFIRMED)

`verify_bundle` iterates `manifest.file_checksums` (verify.rs:192) and the fixed
required-file list (verify.rs:267-278); it never enumerates the directory. Files
added to the bundle dir that are not in the manifest are neither hashed nor
flagged — a channel to smuggle content into an "evidence" bundle that still
verifies. Low impact (extra files aren't part of the attested evidence set) but
worth an allowlist/exact-set check.

### F10 — Key material not zeroized (Low, CONFIRMED)

`Envelope.dek: [u8; KEY_LEN]` (encryption.rs:106) and the KEK returned by
`parse_kek_hex`/`resolve_kek` (encryption.rs:301-329) are plain arrays with no
`Zeroize`/`ZeroizeOnDrop`; they remain in freed memory after use. The Debug impl is
correctly redacted (encryption.rs:112-119) and the KEK is never logged, so this is
defense-in-depth hardening, not an active leak.

---

## VERIFIED-SAFE (checked, confirmed defended)

- **PatchBundle path traversal** — `apply` canonicalizes the joined path and
  enforces `canonical.starts_with(canonical_base)` (patch/mod.rs:139-150), which
  also resolves symlinks; `validate` independently rejects `..`/absolute
  (patch/mod.rs:91-104). Solid.
- **BlobStore** — every method (`write`/`read_bytes`/`read_string`/`exists`/
  `delete`) calls `validate_hash` (64 lowercase hex) before any filesystem access
  (blob_store.rs:118-190,314-325). No traversal reachable.
- **LlmCache / ContextStore** — hex-only key/hash validation
  (cache.rs:36-53, context.rs:19-47) before `join`, blocking `/`, `.`, `\`.
  (Minor: `is_ascii_hexdigit()` also accepts uppercase and length isn't pinned —
  cosmetic, not a traversal vector.)
- **Worker → Coordinator mTLS** — reqwest with pinned server CA
  (`reqwest::Certificate::from_pem` + `.use_rustls_tls()`, worker.rs:181-202) and a
  client identity; **no** `danger_accept_invalid_certs` anywhere. Coordinator side
  uses `WebPkiClientVerifier` requiring + validating client certs
  (coordinator.rs:360-367); failed handshakes drop the connection
  (coordinator.rs:446-456). The hand-rolled CN DER parser (coordinator.rs:536+)
  runs only on already-webpki-verified certs.
- **Bearer-token comparison** — constant-time (coordinator.rs:700-709).
- **Worker RCE containment** — `execute_step` runs coordinator-supplied `.ax`
  under `CapabilityGateway::new(policy)` = `MockHandler` (worker.rs:417-423), so
  net/fs/db effects are mocked. Rogue-coordinator code is DoS/compute, not real I/O.
- **VM run bounding** — `max_steps` default 10_000_000 (vm.rs:115), so an infinite
  `.ax` loop terminates with an error rather than hanging forever.
- **Div/Mod by zero** — guarded → `VmError::DivisionByZero` (vm.rs:664-666,685-687).
- **AES-GCM nonce derivation** — deterministic per-file nonce = `SHA-256(name)[..12]`
  is safe because the DEK is fresh-random per bundle (encryption.rs:125-126,291-298);
  no cross-bundle nonce+key reuse. DEK-wrap and rewrap use fresh random nonces
  (encryption.rs:128-129,245-246). Algorithm is pinned to `aes-256-gcm` and rejected
  at parse otherwise (encryption.rs:164-169).
- **Cloud storage backends** — S3/GCS/Azure endpoints come from operator config
  (builder `.with_endpoint` / env), not from workflow/user data (storage_azure.rs:
  253-278, storage_gcs.rs:44-48). No user-data-driven SSRF; the SigV4 HMAC path is
  library-standard.
- **Command execution** — the two `Command::new` sites take no shell
  (`main.rs:2308` splits on whitespace and execs argv directly for an
  operator-supplied trace2tests predicate; `evidence_serve.rs:449-459` is
  `open`/`xdg-open <url>` for a locally-served dashboard). `adapters/mod.rs` shells
  `cargo` with fixed subcommands. No injection from remote input.
- **SQL** — no `format!`-into-SQL found in `orchestrator/` (grep for
  `format!.*(SELECT|INSERT|UPDATE|DELETE)` empty); persistence appears
  parameterized (see caveat below).
- **Secrets** — no hardcoded credentials and no secret/token logging found; shared
  secret and KEK arrive via flag/env only.

---

## NOT able to verify this pass

- **Coordinator handler bodies** beyond the auth middleware — did not read the full
  `handle_submit_run` / `handle_approve_run` / blob-fetch (`/api/runs/{id}/blobs`)
  logic, so the claimed run-scoping/IDOR protection on the blob route and any
  authorization semantics on `approve` (beyond bearer possession) are unconfirmed.
- **persistence/mod.rs SQL** — inferred parameterized from a negative grep; did not
  read the actual query construction to confirm no dynamic identifier interpolation.
- **Actor scheduler spawn/memory limits** (F11) — did not read `actor.rs`; the
  fork-bomb memory ceiling before the step budget bites is unquantified.
- **Crafted-bytecode trust boundary** (F8) — did not confirm whether a serialized
  `Module` is ever deserialized from file/network (vs. always compiler-produced).
- **Untraced surfaces**: `net_record_replay.rs` / `replay.rs`, the LSP server
  (`crates/boruna-lsp`), the MCP server input limits (`crates/boruna-mcp`), the
  template engine substitution (`tooling/templates`), the package resolver
  (`packages/`), `audit/rotate.rs` KEK-rotation correctness, and telemetry data
  classification (`telemetry.rs`).
- **Prompt injection** into LLM system prompts — the LLM path is BYOH (no shipped
  provider handler), so there is no in-repo system-prompt construction to audit;
  the router (capability_gateway.rs:322-407) forwards args unchanged.
