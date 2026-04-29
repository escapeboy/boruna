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

## Reloading CRLs at runtime

To pick up CRL updates without restarting the coordinator process,
send it `SIGHUP`:

```sh
kill -HUP $(cat /run/boruna-coordinator.pid)
# or, if you know the PID:
kill -HUP <pid>
```

The signal handler re-reads every `--tls-client-crl` file from disk
and rebuilds the TLS verifier. New connections will use the updated
revocation list immediately; in-flight connections complete with the
previous config.

**What is and is not reloaded on SIGHUP:**

| Component | Reloaded on SIGHUP? |
|-----------|---------------------|
| `--tls-client-crl` files | Yes |
| `--tls-cert` / `--tls-key` | No — requires restart |
| `--tls-client-ca` | No — requires restart |
| `--tls-ocsp-staple` | No — requires restart |

CRL parse failure on reload **does not** crash the process — the
previous CRL set stays in effect, and the failure is logged with
the offending path. This matches the standard "fail-static" model:
a malformed CRL during reload should not blow away revocation
that was already in force.

SIGHUP support is UNIX-only. On Windows, a process restart is
required to pick up CRL changes.

## OCSP Stapling (post1-T-4.3)

`boruna coordinator serve` also accepts `--tls-ocsp-staple <FILE>` to
embed a pre-fetched OCSP response in every TLS handshake. This lets
connecting clients verify that the server certificate is not revoked
without making a separate round-trip to an OCSP responder.

The file must be **DER-encoded** (binary) — this is the format rustls
expects internally. Generate it with OpenSSL:

```sh
openssl ocsp \
  -issuer issuer.pem \
  -cert   server.pem \
  -url    http://ocsp.your-ca.example.com \
  -respout server.ocsp
```

Pass it to the coordinator:

```sh
boruna coordinator serve \
  --tls-cert         /etc/boruna/tls/server.pem \
  --tls-key          /etc/boruna/tls/server.key \
  --tls-client-ca    /etc/boruna/tls/clients-ca.pem \
  --tls-ocsp-staple  /etc/boruna/tls/server.ocsp
```

The flag requires the full `--tls-cert/--tls-key/--tls-client-ca`
trio; passing it on a plaintext server is a fatal startup error.
Refresh the staple file (OCSP responses typically expire within a few
hours to days) and restart the process to pick up the new response.

## Verification recipe (manual)

After deploying, exercise the revocation path with a smoke test:

1. Issue a client cert from your CA, register the worker, run a job.
2. Add the cert's serial to the CRL, regenerate the PEM CRL.
3. Send `SIGHUP` to the coordinator (`kill -HUP <pid>`) to reload the CRL.
4. Re-attempt to register from the same client. The TLS handshake
   should be rejected.
5. Issue a fresh cert from the same CA. Register. Job runs.

The first three steps cover the revocation path; the last two cover
that revocation didn't accidentally lock out the CA itself.
