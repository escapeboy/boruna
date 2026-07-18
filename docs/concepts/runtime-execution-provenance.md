# Runtime Execution Provenance

Boruna occupies a provenance category that the established supply-chain
and content-authenticity standards leave largely unoccupied:
**runtime execution provenance** — an attested, tamper-evident record of
what a *specific execution* actually did.

The claim this category makes is narrow and concrete:

> *This specific run executed these steps, made these policy-gated
> capability calls, under this policy, transforming these recorded inputs
> into these recorded outputs.*

That is a statement about an **execution trace**, not about a build, an
artifact, a signing event, a media file, or a booted code image. The
distinction matters because the standards people reach for by reflex all
answer a *different* question.

---

## 1. The provenance landscape

Each of these standards is good at what it targets. None of them targets
the execution trace of a particular run.

| Standard / mechanism | What it attests | What it leaves unoccupied |
|----------------------|-----------------|---------------------------|
| **SLSA** | **Build** provenance: that an artifact was produced by a particular build system from particular sources, following a particular process. | Says nothing about what happens when the built thing later *runs*. A SLSA-attested binary can still do anything at runtime. |
| **in-toto** | **Supply-chain step metadata**: that each link in a defined software supply chain was performed by an authorized party on declared materials/products. | Models the *pipeline that assembles software*, not the *runtime behavior of a workflow execution*. The "steps" are build/release steps, not gated capability calls made during one run. |
| **Sigstore / Rekor** | **Signing events over artifacts**: that a given artifact digest was signed by a given identity, witnessed in an append-only transparency log. | Attests the *existence of a signature* over a blob at a time — not *what an execution did*. Rekor witnesses that something was signed, not that a run made these calls under this policy. |
| **C2PA** | **Content provenance**: the capture/edit history and origin of a media asset (image, audio, video). | Concerned with the lineage of *content*, not with the *execution* of a program or workflow. |
| **TEE remote attestation** | **Code identity**: which enclave/image was measured and booted on attested hardware. | Attests *what code was loaded and the platform it ran on* — not the *trace of what that code then did* (which steps, which capability calls, which inputs→outputs). |

Read down the right-hand column: build provenance, supply-chain step
metadata, signing events, content lineage, and code identity are all
covered — and the *runtime execution trace* falls through the gap between
them. Boruna's evidence bundle is aimed squarely at that gap.

These standards are complementary, not competitors. A mature deployment
might use SLSA for the binary, TEE attestation for the platform, and
Boruna for the execution trace — each answering the question it is built
to answer.

---

## 2. What Boruna attests

The evidence bundle records the run itself. The load-bearing components
(see [Evidence Bundles](./evidence-bundles.md) and
`orchestrator/src/audit/evidence.rs`) map directly onto the category
claim:

- **These steps executed** — the hash-chained `audit_log`, whose entries
  record step start/completion and capability calls in order, with each
  entry chained to the previous (`entry_hash = SHA-256(prev_hash ||
  event_json)`).
- **Under this policy** — `policy.json` and its `policy_hash`; the policy
  snapshot that gated the run is sealed alongside the trace, so a verifier
  sees the exact rules in force.
- **Made these gated capability calls** — capability calls appear in the
  audit log / event stream; the optional `model_invoking_steps.json`
  additionally records which steps transitively reached an LLM
  capability, so an auditor can see which steps touched a model without
  re-analyzing sources.
- **Transforming these inputs into these outputs** — per-step outputs
  under `outputs/<step>/<name>.json`, each SHA-256-checksummed in
  `file_checksums` and thereby committed to by `bundle_hash`.
- **With declared purpose** — the optional `intents.json` records the
  per-step declared intent (what each step was *authorized* to do),
  captured as replay-verified evidence alongside what it actually did.

All of these are covered by the same integrity contract: on-disk
checksums, an unbroken audit-log chain, and a `bundle_hash` over the
manifest. The record is therefore **tamper-evident** in exactly the sense
defined in the [Evidence Bundle Threat Model](./threat-model.md) — and,
as that document is careful to state, tamper-evidence is not
tamper-proofing, and a sealed trace attests *what was recorded*, not that
the producer recorded honestly.

---

## 3. Interoperability with the standards

Occupying a distinct category does not mean living apart from the
ecosystem. The intent is for a Boruna execution record to slot into
existing supply-chain and attestation tooling rather than replace it.

To that end, an **in-toto / DSSE emission** is available via
`boruna evidence attest <bundle-dir>`: it exports the bundle's core
attestation (run identity, `workflow_hash`, `policy_hash`,
`audit_log_hash`, output digests) as an in-toto Statement
(`predicateType: https://boruna.dev/runtime-provenance/v1`) wrapped in a
DSSE envelope signed with the bundle's ed25519 key. That makes the
execution-provenance claim consumable by the same tooling that already
ingests SLSA and in-toto attestations — for example, a transparency log
or a policy engine that gates on attestations. The predicate schema is
specified in `docs/spec/runtime-provenance-predicate-1.0.md`.

> **Status note.** The in-toto/DSSE emitter is implemented
> (`boruna evidence attest`, `--verify` to check the envelope). It is
> **additive** — the native, authoritative format remains the evidence
> bundle described in `docs/spec/evidence-bundle-1.0.md`. Live
> interoperability with a specific `cosign`/`in-toto-verify` binary is
> spec-conformant but not yet end-to-end verified (the DSSE `keyid` is a
> raw-hex ed25519 key that a consumer must bridge to PEM SPKI); see the
> predicate spec's compatibility notes.

The relationship is layered, not overlapping:

- **SLSA / in-toto** attest how the software (and, via DSSE, other
  attestations) was produced and assembled.
- **Sigstore / Rekor** can witness signatures — including, once emitted,
  a DSSE-wrapped Boruna attestation — in an append-only log.
- **TEE attestation** can vouch for the platform and code image that ran
  the Boruna engine.
- **Boruna** attests the execution trace that happened on top of all of
  the above.

Each layer roots a different claim; together they compose into a story
that no single standard tells alone.

---

## 4. See also

- [Evidence Bundles](./evidence-bundles.md) — the concrete artifact that
  carries the execution-provenance record.
- [Evidence Bundle Threat Model](./threat-model.md) — precisely what the
  record proves and does not prove (tamper-evidence vs. tamper-proofing
  vs. non-repudiation).
- `docs/spec/evidence-bundle-1.0.md` — the normative on-disk format and
  integrity contract.
