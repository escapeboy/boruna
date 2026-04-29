# mTLS Certificate Revocation Lists

Sprint reference: post1-T-4.2 (0.7.x speculative branch).

`boruna coordinator serve` accepts `--tls-client-crl <FILE>` to
revoke previously-trusted client certificates without redistributing
the CA bundle. The CRL is loaded at startup and consulted on every
TLS handshake.

> **0.7.x branch:** this feature lives on the parallel
> `0.7.x` line and is **not** part of the 1.x LTS surface. Operators
> running 1.x in production should pin to a 1.x tag; CRL support
> arrives on `master` only after the 0.7.x design has stabilized
> and a back-compat additive design is merged.

## Configuration

```sh
boruna coordinator serve \
  --tls-cert      /etc/boruna/tls/server.pem \
  --tls-key       /etc/boruna/tls/server.key \
  --tls-client-ca /etc/boruna/tls/clients-ca.pem \
  --tls-client-crl /etc/boruna/tls/intermediate-a.crl.pem \
  --tls-client-crl /etc/boruna/tls/intermediate-b.crl.pem
```

Pass `--tls-client-crl` repeatedly for multiple CRLs (one per
intermediate CA). All listed files are loaded; their entries are
merged into a single revocation set.

CRL files MUST be PEM-encoded `X509 CRL`. If your CA emits DER:

```sh
openssl crl -inform DER -in cert.crl -out cert.crl.pem
```

## Failure modes

- **CRL parse failure on startup** → fatal startup error. The
  coordinator refuses to come up rather than silently fall through
  to a CRL-less verifier.
- **Empty CRL file** → fatal startup error (`no X509 CRL PEM blocks
  found`). An empty file usually indicates a configuration mistake
  rather than an intentional revocation reset.
- **`--tls-client-crl` without `--tls-cert`/`--tls-key`/`--tls-client-ca`**
  → fatal startup error. CRLs are meaningless on a plaintext server.
- **Connection from a revoked cert** → TLS handshake rejected.
  Clients see the standard `tls.cert_revoked` failure (rustls's
  `BadCertificate(CertRevoked)`).

## Reload on SIGHUP

The mTLS handshake configuration is built from disk at process
startup. To pick up CRL updates without a full restart, send the
running process a `SIGHUP`. The signal handler re-reads every
`--tls-client-crl` file and rebuilds the verifier.

CRL parse failure on reload **does not** crash the process — the
previous CRL set stays in effect, and the failure is logged with
the offending path. This matches the standard "fail-static" model:
a malformed CRL during reload should not blow away revocation
that was already in force.

> **Implementation status:** the SIGHUP reload path is part of the
> 0.7.x design but is not in this PR. The current implementation
> requires a process restart to pick up CRL changes. Tracked as a
> follow-up; reload semantics above describe the intended
> behavior.

## Verification recipe (manual)

After deploying, exercise the revocation path with a smoke test:

1. Issue a client cert from your CA, register the worker, run a job.
2. Add the cert's serial to the CRL, regenerate the PEM CRL.
3. (Restart or SIGHUP the coord; depends on implementation status.)
4. Re-attempt to register from the same client. The TLS handshake
   should be rejected.
5. Issue a fresh cert from the same CA. Register. Job runs.

The first three steps cover the revocation path; the last two cover
that revocation didn't accidentally lock out the CA itself.
