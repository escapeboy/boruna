# Keyless signing (Fulcio) — design note

**Status:** design sketch only. NOT implemented in this slice. The
`boruna evidence anchor` command and `audit/anchor.rs` implement Rekor
transparency-log anchoring for a bundle that is *already* signed with a
self-managed ed25519 key (`EvidenceBundleBuilder::with_signing_key`).
This note records how Sigstore **keyless** signing would plug into
`evidence attest` / `evidence anchor` if we take it on later.

## Why keyless

Today the operator manages a long-lived ed25519 key: they must generate,
store, rotate, and guard it, and a verifier has to be told out-of-band
which public key to trust. Sigstore **keyless** removes the long-lived
key entirely:

1. The signer authenticates to an OIDC identity provider (Google,
   GitHub Actions, corporate SSO, …) and obtains an **ID token** whose
   `sub`/`email` identifies the workload or human.
2. It generates an **ephemeral keypair** in memory.
3. It sends the public key + a proof-of-possession + the OIDC token to
   **Fulcio**, Sigstore's CA. Fulcio validates the token and issues a
   **short-lived X.509 certificate** (~10 min validity) binding the
   ephemeral public key to the OIDC identity (the identity is recorded in
   a SAN / OID extension).
4. The signer signs the artifact with the ephemeral **private** key,
   then **throws the private key away**.
5. The signature, the certificate, and the artifact digest are logged in
   **Rekor**. Because the cert is short-lived, the Rekor entry's
   `integratedTime` is what proves the signature was made *while the cert
   was valid* — the transparency log is load-bearing, not optional.

Net effect: no key to store or rotate; trust roots in "identity X signed
this at time T", verifiable against Fulcio's + Rekor's public roots.

## How it plugs into Boruna

The current flow, unchanged:

```
finalize bundle → manifest.bundle_hash
  → ManifestSignature (ed25519 over bundle_hash)      [self-managed key]
  → evidence attest  → DSSE envelope (in-toto Statement)
  → evidence anchor  → Rekor hashedrekord entry        [this slice]
```

Keyless would introduce an alternative *signing identity provider* behind
the same seams, leaving the bundle format and the anchor path intact:

- **New: a `SigningIdentity` abstraction.** Two implementations:
  - `LocalKey(ed25519 seed)` — today's behavior.
  - `Keyless { oidc_token, fulcio_url }` — runs steps 1–3 above and
    yields `(ephemeral_signing_key, cert_chain_pem)`.
- **`evidence attest` change.** When signing keyless, the DSSE signature
  is produced with the ephemeral key, and the envelope gains a
  certificate: DSSE has no cert field, so we attach the Fulcio cert chain
  the way cosign does — either as an unauthenticated
  `signatures[].cert` extension or, preferably, by emitting a Sigstore
  **bundle** (`bundle.sigstore.json`, protobuf/JSON) that carries
  `{ dsse envelope, verificationMaterial.x509CertificateChain,
  tlogEntries[] }`. That bundle is what `cosign verify-blob-attestation`
  and `sigstore-python` consume.
- **`evidence anchor` change.** For keyless, the Rekor entry is a
  `hashedrekord`/`dsse`/`intoto` type whose `publicKey.content` is the
  **Fulcio leaf certificate** (PEM), not a bare ed25519 SPKI key. The
  inclusion-proof verification in `audit/anchor.rs` is *unchanged* — it is
  cert-agnostic (it hashes the Rekor `body` and checks the Merkle proof +
  data-hash binding). The extra keyless verification steps layer on top:
  - verify the Fulcio cert chains to the Fulcio root,
  - check the OIDC identity (SAN) against an allowed-identity policy,
  - check `integratedTime` falls within the cert's validity window,
  - verify the signature with the cert's public key.

## What it would cost / open questions

- **An OIDC token source.** Interactive (browser device-flow) is fine for
  humans; CI needs ambient tokens (GitHub Actions OIDC, GCP/AWS workload
  identity). This is the bulk of the work and is inherently *non-local* —
  it breaks Boruna's "runs fully offline / air-gapped" default, so keyless
  must stay strictly opt-in, parallel to the self-managed-key path, never
  replacing it.
- **X.509 + protobuf deps.** Fulcio cert parsing/validation and the
  Sigstore bundle format pull in `x509-cert` / `der` (and possibly the
  sigstore protobufs). None are needed for the current anchor slice, so
  they belong behind a `keyless` cargo feature, matching how `rekor`
  gates the network client.
- **Private deployments.** Air-gapped users can run a private Fulcio +
  Rekor + OIDC (Dex); the `fulcio_url` / `rekor_url` must stay
  configurable exactly like `--rekor-url` already is.

## Recommended increment order

1. (done) Rekor anchoring for self-managed-key bundles — this slice.
2. `SigningIdentity` abstraction + emit a Sigstore `bundle.sigstore.json`
   for the *local key* case (no OIDC yet) to prove the bundle format /
   cosign-verify path end-to-end.
3. Fulcio client + OIDC device-flow (behind `keyless`), then ambient CI
   token sources.
4. Full keyless verification (cert-chain + identity policy + time-window)
   in `evidence anchor --verify`.
