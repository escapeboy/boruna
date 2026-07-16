# Boruna 2.0 Remediation — Sprint Plan & Status

Branch: `sprint/2.0-remediation`. Targets **2.0.0** because several fixes are intentionally
LTS-breaking (strict type checking, checked arithmetic as errors, fail-closed defaults,
coordinator protocol change). Every finding traces to [[00-index]] and its verify notes.

## Tranche status

| Tranche | Item | Finding | Status |
|---|---|---|---|
| A | Doc-drift (version/counts/stability) | doc-drift | ✅ done, committed |
| B | SSRF: resolve+check DNS, re-validate redirects, IPv6 brackets | F3/F4 | ✅ done (llmvm tests green) |
| B | Actor `.expect()` panic → fail the actor | F8/actor | ✅ done (llmvm tests green) |
| B | Checked i64 arithmetic → `ArithmeticOverflow` | F7 | ✅ done (llmvm tests green) |
| B | XSS: escape all bundle-derived HTML in `evidence serve` | CLI 4.2 | ✅ done (gating) |
| B | Content-addressing: verify `output_hash==SHA256(json)` at coordinator boundary | G2/S12 | ✅ done + regression test (gating) |
| B | Coordinator fail-closed on non-loopback bind w/o auth | F5/S1 | ✅ done (gating) |
| B | Template-name traversal guard (MCP) | tooling S3 | ✅ done (gating) |
| B | Storage `ref_to_run_id` `..`/`.` guard ×3 | audit 6.1 | ✅ done (gating) |
| C | Codegen `len() as u8` count truncation → error >255 | frontend S1 | 🔜 staged (low-risk, next pass) |
| C | Actor opcodes gated by ActorSpawn/ActorSend policy | F6 | 🔜 staged (bounded, next pass) |
| D | Framework fail-closed policy defaults | FW fail-open | 🔜 staged — policy-trust refactor, broad blast radius (example apps + TestHarness), needs the mandated dedicated self-review pass; NOT rushed here |
| G | Emit `ensures` postconditions (needs `result` binding) | frontend G1 | 🔜 staged (real feature, not a mirror of `requires`) |
| G | `while`-body trailing-expr stack leak | frontend G11 | 🔜 staged |
| B | DEK/KEK `Zeroize` on drop | F10 | 🔜 staged (adds `zeroize` dep; Low) |

**This pass consolidates the gated security + arithmetic tranche (A + B + C-arithmetic).**
Remaining C/D/G items are staged so this PR stays clean and green rather than carrying
half-finished behavior changes. E/F/G-language remain architectural follow-ups below.

## Staged for dedicated follow-up (architectural / LTS / protocol — not safely one-shot)

These are large, cross-cutting, and each carries contract implications that deserve a focused
sprint + review rather than being rushed:

### E — Evidence tamper-evidence (the #1 High)
- **Verify-side (tractable, high value):** make `verify_bundle` recompute+check `manifest.bundle_hash`,
  reject an `encryption`-block strip (downgrade), and add an `--expected-bundle-hash` anchor param +
  require-encryption flag. Closes the *detection* half without new crypto deps.
- **Sign-side (architectural):** ed25519 manifest signature under an operator key, verified with a
  trusted public key. New key-management surface, bundle-format version bump → `evidence-bundle 1.1`.

### F — Coordinator principal/ownership model (verify-B N2, root of S6+S9)
- Add `worker_id` to the terminal CAS predicate (kills cross-worker claim hijack S6).
- Add an approver identity + per-gate token to `/approve` (kills gate seizure S9/N1), record the
  run submitter. Wire-protocol change → `protocol_version: 2` + worker/coord version negotiation.

### G — Language buildout (the "statically typed" gap)
- **Type checker:** arity enforcement, type consistency, record-field validation, `requires`/`ensures`
  typed to `Bool`, match exhaustiveness. Strict mode rejects existing loose programs → LTS-breaking (2.0).
- **Higher-order/indirect calls:** real `FnRef` dispatch (currently hardcoded to fn #0).
- **Enum-variant match:** real per-variant tags (currently all collapse to `-1`).
- **Parser:** `for`-loop production; `Map<K,V>` / `Fn(..)->T` type expressions.

## LTS / 2.0 breaking-change ledger (for CHANGELOG on release)
- Integer overflow is now a runtime error (was: debug panic / release wrap).
- Coordinator refuses to start on a non-loopback bind without auth (was: warn + serve).
- Coordinator rejects `output_hash` that doesn't match `output_json` (was: trusted).
- (staged) Strict type checking rejects previously-accepted loose `.ax` programs.
- (staged) Coordinator wire protocol v2 (ownership fields).
