# Operator guide — mTLS + per-worker client certificates

Sprint `W6-A` (post 1.0-rc1) adds an opt-in mutual-TLS auth surface
to the coord+worker HTTP protocol. This guide covers when to use
it, how to provision certificates, and how to configure both
sides.

mTLS is **additive**. The shared-secret bearer auth shipped in
0.5-S3 keeps working unchanged. Operators choose between three
modes:

| Mode | When to use | What you get |
|------|-------------|--------------|
| Bearer only (current default) | Loopback or behind a TLS-terminating proxy | Authenticated requests, no transport encryption from Boruna itself |
| mTLS only | Compliance environments where every cluster connection must carry an X.509 identity | TLS encryption + per-worker identity (cert subject CN) — no shared secret to rotate |
| mTLS + bearer | Defense-in-depth: cert proves identity, bearer is a second factor | Both gates checked on every request |

Operators opt in by passing flags. There is no global config file.
**The default behavior is unchanged** — running `boruna coordinator
serve` with no TLS flags binds plain TCP, exactly as in 1.0-rc1.

## CLI flags

### Coordinator

```bash
boruna coordinator serve \
  --data-dir /var/lib/boruna \
  --bind 0.0.0.0 \
  --tls-cert /etc/boruna/server.pem \
  --tls-key  /etc/boruna/server.key \
  --tls-client-ca /etc/boruna/clients-ca.pem
```

| Flag | Meaning |
|------|---------|
| `--tls-cert` | PEM file with the coord's server certificate chain |
| `--tls-key` | PEM file with the coord's server private key |
| `--tls-client-ca` | Trust root for verifying CLIENT certificates. Workers must present a cert chained to this root. |

All three flags are required together. Passing only some is a
startup error: `--tls-cert, --tls-key, --tls-client-ca must all be
provided together`.

### Worker

```bash
boruna worker run \
  --coordinator https://coord.internal:8090 \
  --worker-id   workshard-3 \
  --tls-cert    /etc/boruna/workshard-3.pem \
  --tls-key     /etc/boruna/workshard-3.key \
  --tls-server-ca /etc/boruna/server-ca.pem
```

| Flag | Meaning |
|------|---------|
| `--tls-cert` | PEM file with the worker's client certificate chain |
| `--tls-key` | PEM file with the worker's client private key |
| `--tls-server-ca` | Trust root for verifying the COORD's server certificate |

Same all-or-nothing rule: partial flag sets fail at startup.

## Identity model

When mTLS is on, the coord extracts the **subject CN** from each
worker's client certificate during the TLS handshake. That CN
drives the worker's identity:

- The CN is logged at registration time (`coordinator: registering
  worker 'workshard-3' via mTLS cert`).
- If the body of `POST /api/workers/register` also contains a
  `worker_id` field, it MUST match the CN
  (case-insensitive). Mismatch returns 401 with
  `error_kind: "coord.identity_mismatch"`.
- If the body does NOT contain a `worker_id`, the CN becomes the
  worker_id automatically.

This is a real security boundary. Without the CN check, any worker
holding a valid cert chained to `--tls-client-ca` could impersonate
any other worker by passing a different `worker_id` in the body.

## Auth precedence

The coord's auth middleware composes both gates:

```
mTLS_required?    bearer_set?    behavior
────────────────  ────────────  ─────────────────────────────
no                no             pass through (loopback default)
no                yes            require Authorization: Bearer
yes               no             require valid client cert
yes               yes            require BOTH cert and bearer
```

If TLS is configured but a request reaches the middleware without
a `ClientIdentity` extension (a defense-in-depth check against
plumbing bugs), the middleware rejects with 401
`coord.unauthorized`.

## Cert provisioning

Boruna does not ship a CA tool. Use any X.509 toolchain you
already have:

- [`step-ca`](https://smallstep.com/docs/step-ca/) — recommended
  for production. Issue short-lived certs, automate renewal.
- [`cfssl`](https://github.com/cloudflare/cfssl) — Cloudflare's
  PKI toolchain.
- `openssl req` — fine for self-signed development setups (see
  recipe below).

### Self-signed development recipe (FOR DEV ONLY)

This recipe produces a self-signed CA + server cert + client cert
with `openssl`. **DO NOT use these certs in production** —
self-signed roots have no revocation story and the keys live on
your laptop.

```bash
# 1. Self-signed CA.
openssl genrsa -out ca.key 4096
openssl req -x509 -new -key ca.key -out ca.pem -days 3650 \
  -subj "/CN=boruna-dev-CA"

# 2. Server cert for the coord (CN=coord hostname or IP).
openssl genrsa -out server.key 4096
openssl req -new -key server.key -out server.csr \
  -subj "/CN=coord.internal"
# SAN extension file (required by modern clients):
cat > server.ext <<EOF
subjectAltName = DNS:coord.internal,IP:127.0.0.1
EOF
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key \
  -CAcreateserial -out server.pem -days 365 -extfile server.ext

# 3. Client cert per worker (CN=worker_id).
openssl genrsa -out workshard-3.key 4096
openssl req -new -key workshard-3.key -out workshard-3.csr \
  -subj "/CN=workshard-3"
openssl x509 -req -in workshard-3.csr -CA ca.pem -CAkey ca.key \
  -CAcreateserial -out workshard-3.pem -days 365
```

Then run the coord and worker with the file paths shown above.

### Production checklist

- [ ] Use a real CA — step-ca, cfssl, or your org's existing PKI.
- [ ] Issue **per-worker** client certs with the worker_id as CN.
      Sharing one client cert across workers defeats the identity
      check.
- [ ] Set short cert lifetimes (24h–7d) and automate renewal.
- [ ] Restrict access to private keys — `chmod 600`, owned by the
      Boruna service user.
- [ ] Rotate the client CA when adding/removing workers from the
      fleet — old worker certs should fail to verify after their
      cert is revoked or expires.
- [ ] If you also use shared-secret bearer (mTLS+bearer mode),
      rotate the secret quarterly — same hygiene as before.

## Migration from bearer-only

mTLS is additive — bearer-only deployments continue to work
unchanged on 1.0+. To migrate:

1. Provision certs (CA, server cert, per-worker certs).
2. Roll the workers FIRST — start them with both
   `--shared-secret` AND the three TLS flags. They'll keep using
   bearer until the coord enables mTLS.
3. Restart the coord with the three TLS flags. From this point
   the coord requires both cert and bearer.
4. Optionally: drop `--shared-secret` from coord+workers to go
   mTLS-only.

There is no way to silently downgrade from mTLS to plain — once
the coord is started with TLS flags, every connection MUST present
a valid client cert. This is by design.

## Adversarial properties

The integration test in
`crates/llmvm-cli/tests/cli_coordinator_mtls.rs` locks four
behaviors:

1. Connection without a client cert → TLS handshake fails. No
   HTTP response is produced.
2. Connection with a client cert from a DIFFERENT CA → handshake
   fails. The server's `WebPkiClientVerifier` only trusts certs
   chained to `--tls-client-ca`.
3. Valid cert with no body `worker_id` → registration succeeds;
   the recorded worker_id is the cert's CN.
4. Valid cert with body `worker_id` that doesn't match the CN →
   401 `coord.identity_mismatch`.

If any of these regress, the test suite fails — the property
isn't on a "we hope it still works" footing, it's on a CI gate.
