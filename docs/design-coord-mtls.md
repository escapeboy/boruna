# Design — coord+worker mTLS auth surface (sprint W6-A)

## Context

v1.0.0-rc1 ships shared-secret bearer auth on the coord HTTP
surface (sprint 0.5-S3). That's adequate for the common case
where Boruna runs on a private network or behind a TLS-terminating
reverse proxy. It's NOT adequate for compliance-sensitive
deployments where:

- Every connection must carry a per-worker X.509 identity (no
  more "any cluster node holding the secret can impersonate any
  worker").
- Transport encryption is required end-to-end, not just on the
  edge proxy.
- The audit story needs cryptographic proof that "worker
  workshard-3" submitted a particular `complete` message.

The 1.0 LTS contract (`docs/lts.md` — to be added by post-1.0
work) commits to NOT breaking the shared-secret bearer path.
mTLS therefore lands as ADDITIVE: operators opt in via new
flags, the existing bearer path keeps working unchanged.

## Goals

- Add `--tls-cert / --tls-key / --tls-client-ca` to
  `coordinator serve` and `--tls-cert / --tls-key /
  --tls-server-ca` to `worker run`.
- When all three flags are set, the coord requires a verified
  client cert on every connection. The cert subject CN drives
  worker identity.
- Compose with bearer auth: an operator with both flags AND a
  shared-secret gets a two-factor gate.
- Fail at startup if flags are partially configured (project
  §1: half-configured TLS rejected at parse, not silently
  fallback to plaintext).
- Document the operator workflow including a self-signed dev
  recipe.

## Non-goals

- **Built-in CA tooling.** Out of scope for W6-A. Operators use
  step-ca, cfssl, or `openssl req` (the dev recipe in
  `docs/guides/coord-mtls.md` is a copy-paste demo, not a
  product).
- **Cert renewal automation.** Operators run their own renewal
  agents (cert-manager, step renewal hooks, etc.).
- **OAuth / OIDC.** Tracked separately in the 0.6.x roadmap.
- **Encrypted-at-rest audit logs.** Sprint W6-B.

## Critical constraints

- **Bearer auth path unchanged.** `--shared-secret` continues to
  work on its own. Adding `--tls-cert` does NOT change bearer
  semantics.
- **Default behavior unchanged.** `boruna coordinator serve` with
  no TLS flags binds plain TCP, exactly as in 1.0-rc1.
- **CN drives identity.** When mTLS is on and the request body
  carries a `worker_id`, it MUST match the cert CN
  (case-insensitive). Mismatch → 401 `coord.identity_mismatch`.
  Without this check, any worker holding a valid cert could
  impersonate any other worker_id.

## Architecture

### Dependencies

Three new workspace deps:

- `rustls = { version = "0.23", default-features = false, features = ["std", "tls12", "aws_lc_rs"] }`
- `rustls-pemfile = "2"`
- `tokio-rustls = { version = "0.26", default-features = false, features = ["aws_lc_rs", "tls12"] }`

Plus a dev-dep: `rcgen = "0.13"` for the integration test's
on-the-fly cert generation.

`default-features = false` per ADR 001 (drop `prefer-post-quantum`
and `logging`). We DO enable `aws_lc_rs` — rustls's default
crypto provider with stronger LTS guarantees than `ring`.
`ring` is still in the dep tree (transitively via reqwest's
`rustls-tls` feature) — co-existing crypto providers in rustls
0.23 work because each TLS connection picks one explicitly via
the configured `ServerConfig` / `ClientConfig`.

### Coordinator listener path

`crates/llmvm-cli/src/coordinator.rs`:

1. `ServerTlsPaths::from_optional` validates the three Option<PathBuf>
   inputs at startup and returns `Result<Option<Self>>`. None ⇒
   plain TCP. All three Some ⇒ mTLS. Anything else ⇒ typed error.
2. `build_server_tls` reads the PEM files, builds a
   `WebPkiClientVerifier` from `--tls-client-ca`, and constructs
   a `rustls::ServerConfig` that requires a client cert on every
   connection.
3. `run_serve` chooses one of two paths after `TcpListener::bind`:
   - No TLS: existing `axum::serve(listener, app).await`.
   - mTLS: `serve_with_tls(listener, app, tls)` — a per-connection
     loop that wraps each accepted TCP stream in a
     `tokio_rustls::TlsAcceptor`, extracts the client cert
     subject CN from the completed handshake, and stuffs a
     `ClientIdentity { common_name }` extension into every request
     on that connection. The extension is wired in via
     `tower::ServiceBuilder::map_request` so it survives axum's
     extractor pipeline unchanged.

The Mutex-protected store and existing handlers stay byte-for-byte
identical between the two paths.

### CN extraction

`cn_from_cert_der` is a hand-rolled minimal X.509 DN parser that
walks the TBSCertificate structure to the Subject field
specifically (NOT a global OID scan — the issuer DN appears
BEFORE the subject DN in TBSCertificate, so a naive scan would
return the CA's CN instead of the worker's). The parser supports
both short-form and definite long-form ASN.1 lengths so 256+-byte
issuer DNs from large CAs don't trip it.

We deliberately avoided pulling `x509-parser` (a full DN parser):
the surface here is small, the failure mode is well-bounded
(`WebPkiClientVerifier` already validated the cert before we
touch it), and the dep weight savings matter for 1.0's binary
size.

### Auth middleware

`auth_middleware` composes both gates:

```rust
if state.config.mtls_required
    && request.extensions().get::<ClientIdentity>().is_none() {
    return unauthorized_response();  // defense-in-depth
}
if let Some(expected) = state.config.shared_secret.as_deref() {
    // existing bearer check
}
```

When both flags are set on the coord, BOTH checks must pass.

### Identity reconciliation in handle_register

`handle_register` extracts the optional `ClientIdentity` extension
and reconciles it against the body `worker_id`:

| cert_cn | body.worker_id | result |
|---------|----------------|--------|
| Some(cn) | None | worker_id := cn (CN drives identity) |
| Some(cn) | Some(id), id matches cn | worker_id := cn |
| Some(cn) | Some(id), id does NOT match cn | 401 `coord.identity_mismatch` |
| None | Some(id) | worker_id := id (no mTLS path) |
| None | None | auto-generate `wkr-<uuid>` |

Case-insensitive match per X.509 conventions (CNs are typically
ASCII so no special Unicode normalization is needed).

### Worker client config

`crates/llmvm-cli/src/worker.rs`:

`ClientTlsPaths::from_optional` mirrors the coord-side struct.
When all three set, the worker's `reqwest::Client` is built with
`use_rustls_tls()` + `Identity::from_pem(cert+key)` +
`add_root_certificate(server_ca)`. The shared-secret bearer path
is independent — operators can run mTLS-only, bearer-only, both,
or neither.

## Auth precedence (operator-facing)

| mtls_required | shared_secret set | behavior |
|---------------|-------------------|----------|
| no | no | pass through (loopback default — pre-W6 behavior) |
| no | yes | require `Authorization: Bearer …` |
| yes | no | require valid client cert (CN drives identity) |
| yes | yes | require BOTH cert AND bearer |

The startup banner reflects the active mode: `auth: enabled
(mTLS + shared-secret bearer)`.

## Test coverage

**Unit (in `coordinator.rs`):**

- `parse_tls_flags_requires_all_three_or_none` — validates
  `ServerTlsPaths::from_optional`'s typed-error semantics.
- `auth_middleware_rejects_when_tls_required_but_no_cert` —
  defense-in-depth: when mTLS is on and the extension is missing,
  middleware returns 401.
- `auth_middleware_accepts_when_tls_required_and_identity_present`
  — positive case: synthesized identity passes through.
- `handle_register_rejects_cn_worker_id_mismatch` — locks the
  identity-reconciliation rule. CN=worker-A but body says
  worker-B → `coord.identity_mismatch`.
- `cn_extraction_pulls_subject_correctly` — generates a real
  rcgen cert, runs `cn_from_cert_der`, asserts the CN is
  recovered. This locks the parser against the same DER shape
  rcgen / openssl produce.

**Integration (`tests/cli_coordinator_mtls.rs`):**

- `mtls_no_client_cert_handshake_rejected` — bare rustls client
  with `with_no_client_auth()` cannot complete the handshake.
- `mtls_client_cert_from_different_ca_handshake_rejected` —
  reqwest client with a foreign-CA cert fails at the transport
  layer (no HTTP response).
- `mtls_valid_client_cert_succeeds_and_cn_drives_worker_id` —
  end-to-end: spawn coord, register with no body worker_id,
  assert the recorded worker_id matches the cert CN.
- `mtls_cn_mismatch_returns_identity_mismatch` — end-to-end:
  cert CN=worker-A, body says worker-B, assert 401 +
  `coord.identity_mismatch`.
- `mtls_partial_flags_are_rejected_at_startup` — process exits
  non-zero with the typed error message when only `--tls-cert`
  is passed.

All five integration tests use rcgen to generate the certificate
hierarchy in a tempdir — no fixture files committed to the repo.

## New error_kind

- `coord.identity_mismatch` — 401, returned by `handle_register`
  when the cert subject CN does not match the body
  `worker_id` (case-insensitive).

No other new error_kind values. Existing
`coord.unauthorized` is reused for missing-bearer AND
missing-cert (both are "you didn't authenticate
correctly").

## Migration

Existing 1.0-rc1 deployments are unaffected. Operators who want
mTLS:

1. Provision certs out-of-band (the operator guide includes a
   step-by-step openssl recipe for dev environments).
2. Restart workers with the new flags FIRST. They keep using
   bearer until the coord adds TLS.
3. Restart the coord with `--tls-cert / --tls-key /
   --tls-client-ca`. From this point, every connection requires
   a verified cert.
4. Optionally drop `--shared-secret` once mTLS is fully rolled
   out.

## Open questions / future work

- **Cert chain depth limits.** The current `WebPkiClientVerifier`
  uses webpki defaults. If we hit operators with deep
  intermediate-CA hierarchies (>5 levels) we'd need to tune.
- **OCSP / CRL.** webpki doesn't do online revocation. Operators
  who need real-time revocation should pair short cert lifetimes
  with automated renewal (the recommended approach anyway).
- **Per-route auth scopes.** Today every coord route is gated by
  the same auth surface. A future sprint could split read-only
  dashboard routes from mutation routes — but that's an operator-
  ergonomics improvement, not a security gap.
- **Client cert pinning.** We trust the CA, not specific certs.
  An attacker who compromises the CA can mint new client certs.
  This is the standard X.509 trust model and matches the bearer
  model's "anyone who steals the secret wins" property.
