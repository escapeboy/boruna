# Evidence Bundle Threat Model

This document states, honestly and without marketing, what an evidence
bundle proves and — just as important — what it does **not** prove. It
is written in the SLSA spirit: enumerate the threats, name the concrete
mitigation, and name the residual gap that remains after the mitigation.

If you take one sentence away, take this:

> An evidence bundle lets you prove that **the record was not altered
> after it was sealed**, and that the record is **internally consistent
> under replay**. It does not, and cannot, prove that the record was
> **true when it was written**.

Everything below is an elaboration of that single distinction.

---

## 1. Three properties people conflate

Compliance conversations routinely blur three different guarantees.
Boruna delivers the first, delivers the second only under stated
conditions, and deliberately does not claim the third.

| Property | Plain-English meaning | Does Boruna provide it? |
|----------|-----------------------|-------------------------|
| **Tamper-evidence** | If someone changes the sealed record, a verifier can *detect* the change. | Yes — this is the core guarantee. |
| **Tamper-proofing** | Changing the sealed record is *impossible*. | No. Nothing on a general-purpose filesystem is tamper-proof; a holder of the bytes can always rewrite them. Boruna makes tampering *detectable*, not *impossible*. |
| **Non-repudiation** | The party who produced the record cannot later deny producing it. | Partial, and only with a signed bundle under a pinned key (see §4). Even then it attests *who sealed the bytes*, not *whether the bytes are true*. |

Keep these separate. Most overclaiming comes from quietly upgrading
"tamper-evident" to "tamper-proof", or from treating a signature as
proof of *truth* rather than proof of *origin*.

---

## 2. What a bundle actually contains

A sealed bundle (`orchestrator/src/audit/evidence.rs`, `BundleManifest`)
carries, at minimum:

- `run_id`, `workflow_name`, `workflow_hash`, `policy_hash`
- `audit_log_hash` — the head of a hash-chained event log
- `file_checksums` — SHA-256 of every component file (workflow, policy,
  audit log, env fingerprint, per-step outputs, and optional
  `intents.json` / `model_invoking_steps.json`)
- `env_fingerprint` — OS / arch / Boruna version, **self-reported**
- `bundle_hash` — SHA-256 over the manifest itself (excluding
  `bundle_hash` and `signature`), which therefore commits to every
  `file_checksums` entry and the `audit_log_hash`
- optional `encryption` — AES-256-GCM envelope metadata
- optional `signature` — an ed25519 signature over `bundle_hash`

The integrity contract enforced by `verify_bundle`
(`orchestrator/src/audit/verify.rs`): every file's on-disk SHA-256 must
match `file_checksums`; the audit-log chain must be unbroken
(`entry_hash = SHA-256(prev_hash || event_json)`); the chain head must
equal `audit_log_hash`; and all required components must be present. For
the full on-disk contract see
[Evidence Bundles](./evidence-bundles.md) and the format spec at
`docs/spec/evidence-bundle-1.0.md`.

---

## 3. Threats and mitigations

| Threat | What Boruna does | Residual gap |
|--------|------------------|--------------|
| **A third party edits a bundle file after it was sealed.** | The hash chain plus `file_checksums` plus `bundle_hash` make a naive edit detectable: any changed byte fails its SHA-256 check, and any spliced/removed audit entry breaks the chain. `verify_bundle` reports the failing check. | A *naive* edit is caught by plain `verify`. A *motivated* attacker who holds the whole bundle can rewrite the file **and** recompute every checksum **and** recompute `bundle_hash` so the bundle is internally self-consistent — this defeats plain `verify` (documented in-code as the "F1 weakness"). Closing it requires an **external anchor** or a **signature under a pinned key** — see §4. Tamper-*evidence*, not tamper-*proofing*. |
| **The recorder/producer is malicious and seals a FALSE record at write time.** | Nothing. The bundle faithfully seals whatever the producer fed it. A signature (if present) attests *which key sealed these bytes* — the producer's identity — not that the sealed facts are true. | **Not prevented, by construction.** Garbage-in is sealed as faithfully as truth-in. Evidence bundles are a *tamper-evidence* mechanism, not a *truth oracle*. Detecting a lying producer requires controls outside the bundle (independent corroboration, dual control over the recorder, a trusted execution environment — see §5). |
| **The signing key is compromised.** | With a valid key an attacker can forge or backdate the entire bundle, sign it, and it will verify under that key — hash-chaining is single-writer and provides no defense once the writer's key is held. Pinning a `trusted_pubkey` at verify time limits acceptance to a specific key, so a *different* attacker key is rejected. | If the *legitimate* key itself is stolen, pinning does not help — the forged bundle carries the pinned key. There is no revocation, no key rotation history, and no witnessed record of *when* a signature was made. Mitigation direction (not yet implemented): anchoring signatures in an append-only **transparency log** and/or **keyless, identity-bound signing**, so a signature is bound to a witnessed moment and a verifiable identity rather than to a long-lived secret. |
| **Backdating — sealing a record now but claiming it was produced earlier.** | The manifest carries `started_at` / `completed_at` / `created_at` timestamps, but these are **self-reported wall-clock values written by the producer**. Nothing external witnesses them. | **No trusted timestamp today.** A producer (or a key holder) can set these fields to any value. Mitigation direction (not yet implemented): anchoring the `bundle_hash` in an external append-only log (e.g. a Rekor-style transparency log) at seal time, so the *earliest-existence* time of the bundle is witnessed by a third party rather than asserted by the producer. |
| **Non-determinism, especially LLM calls, undermines "reproducibility".** | Replay re-executes the workflow against the **recorded** capability results: LLM calls, HTTP fetches, and other effects return their captured responses instead of hitting live services, and `--verify` checks that the replay reproduces the same output hashes. This proves the recorded run is *internally consistent* — the recorded inputs deterministically produce the recorded outputs. | Replay proves reproducibility **given the recorded capability results** — it does **not** prove that the model (or any external service) would return the same thing if called again live. A non-deterministic model is captured, not tamed: the bundle pins *what the model said this time*, not *what the model will say next time*. Do not read a passing replay as "the model is deterministic." |
| **The environment fingerprint is forged.** | `env_fingerprint.json` records OS, architecture, and Boruna version, and it is checksummed and covered by `bundle_hash` like every other file — so it cannot be changed *after* sealing without detection. | The fingerprint is **self-reported by the recording process, not hardware-attested.** A malicious or misconfigured producer can write any values it likes *at seal time*; the integrity check only proves those values were not altered afterward, not that they were true. Mitigation direction (not yet implemented): **TEE remote attestation**, binding the fingerprint to a hardware root of trust that attests the actual code image and platform that ran. |

---

## 4. Why "detects tampering" needs a footnote

Plain `boruna evidence verify` gives you **internal** consistency: it
recomputes every checksum and the chain and confirms they agree with the
manifest. That catches accidental corruption and unsophisticated edits.

It does **not**, by itself, catch an attacker who controls the whole
bundle, because that attacker can make the manifest agree with their
forgery. Two independent, composable checks close this gap; neither is on
by default, and each roots trust in something the attacker does not
control:

1. **External anchor** (`--expected-bundle-hash` /
   `expected_bundle_hash`). You record the `bundle_hash` out-of-band at
   seal time — in a separate system the attacker cannot rewrite — and
   supply it at verify time. Verification then requires the recomputed
   hash to equal *your* anchor, not the manifest's self-reported one. A
   forged-but-self-consistent bundle fails because its recomputed hash no
   longer matches the anchor you kept. This is what makes a plaintext
   bundle genuinely tamper-evident against a motivated attacker.

2. **ed25519 signature under a pinned key** (`--verify-key` /
   `trusted_pubkey`, optionally `require_signature`). The producer signs
   `bundle_hash` with an ed25519 key; the verifier pins the *expected*
   public key. Trust is rooted in the pinned key: a bundle re-signed with
   any other key is rejected as `signature_untrusted_key`. Without
   pinning, a signature proves only that *some* key signed — an attacker
   can substitute their own.

The signature's meaning is precise: it attests **which key sealed these
bytes**. That is an origin/authenticity claim about the producer, not a
truth claim about the content (contrast the malicious-producer row in
§3). Non-repudiation follows only to the extent that the key is bound to
an accountable identity and is not shared — conditions the bundle format
cannot enforce on its own.

---

## 5. Mitigation directions (not yet implemented)

The residual gaps in §3 are real. The honest position is that they are
*known* and have *known* remedies on the roadmap, none of which ship
today:

- **Transparency-log anchoring** (Rekor-style): witness the `bundle_hash`
  in an external append-only log at seal time, giving a third-party-
  attested earliest-existence timestamp and defeating silent backdating.
- **Keyless / identity-bound signing**: bind a signature to a verifiable
  workload identity for a short-lived credential, reducing the blast
  radius of a stolen long-lived key.
- **TEE remote attestation**: replace the self-reported environment
  fingerprint with a hardware-attested measurement of the code image and
  platform that actually executed.

Until these land, treat the corresponding claims conservatively: a bundle
proves post-seal integrity and internal replay-consistency, anchored or
signed bundles additionally prove origin against a chosen root of trust,
and *nothing in the bundle* proves the producer was honest or that the
timestamps are true.

---

## 6. See also

- [Evidence Bundles](./evidence-bundles.md) — on-disk layout, hash chain,
  and the `verify` / `inspect` / replay workflow.
- [Runtime Execution Provenance](./runtime-execution-provenance.md) — the
  provenance category Boruna occupies, and how it relates to SLSA,
  in-toto, Sigstore, C2PA, and TEE attestation.
- `docs/spec/evidence-bundle-1.0.md` — the normative format and integrity
  contract.
