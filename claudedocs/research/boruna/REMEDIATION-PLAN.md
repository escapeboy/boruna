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
- **Verify-side — ✅ DONE this pass.** `verify_bundle_with_opts` recomputes + checks
  `manifest.bundle_hash` (internal consistency), adds `evidence verify --expected-bundle-hash <hex>`
  (the out-of-band ANCHOR that gives real tamper-evidence — proven by
  `test_verify_anchor_detects_forged_manifest`, which forges a fully self-consistent manifest that
  plain verify accepts but the anchor rejects), and `--require-encryption` to block a
  downgrade-to-plaintext strip. No new crypto deps.
- **Sign-side (architectural, STILL STAGED):** ed25519 manifest signature under an operator key,
  verified with a trusted public key — removes the need for operators to carry the anchor
  out-of-band. New key-management surface, bundle-format version bump → `evidence-bundle 1.1`.

### ✅ Landed after the checkpoint (2026-07-17, gated green)
- **S6 (cross-worker claim hijack)** — DONE, commit `2a8d2bb`. `RunCheckpointStore::step_claimed_by`
  + coordinator `reject_if_not_claim_owner` (403 `coord.claim_not_owned`) on complete/fail/extend, at
  the trust boundary under the store lock. Boundary approach (like S12) — no CAS/test churn.
- **F10 (key zeroize)** — DONE (parallel agent), commit `6d67670`.
- **S1 (codegen count truncation) + G11 (while-body stack leak)** — DONE (parallel agent), commit `c784dc9`.

### ✅ F COMPLETE (2026-07-17)
- **S6** — done (`2a8d2bb`).
- **S9** — DONE (`f0826ff`). The ApprovalGate open path now mints a per-gate token
  (`acquire_trigger_token`, previously only ExternalTrigger did); `handle_approve_run` requires it
  (403 `coord.approval_token_invalid`); CLI `workflow approve|reject --token`. Boundary approach.

### ✅ D — framework fail-closed (2026-07-17, parallel agent, cherry-picked `bcfb5f7`)
Empty/malformed `policies()` now DENIES (was allow-all). No-`policies()` allow_all convenience
preserved. Account-takeover self-review done. 96 framework tests green.

### ✅ G-completeness — for-loops + Map/Fn types + ensures (2026-07-17, parallel agent, `e984cf8`)
Additive `.ax` features. STILL OPEN in G: type checker (arity/type/exhaustiveness), higher-order/
indirect calls (dispatch to fn #0), enum-variant match tags (all -1) — serial on codegen/vm, LTS-breaking.

### ⏳ E-sign — ed25519 manifest signing — DONE BUT NEEDS RECONCILIATION
Parallel agent built it on branch `worktree-agent-acbdc161d1119c689` (`f214b7c`): optional ed25519
signature, `evidence verify --verify-key/--require-signature`, 6 tests green. BUT it branched from
master (pre-E-verify) and re-created `verify_bundle_with_opts`/`VerifyOptions` with a DIFFERENT
signature than this branch's `--expected-bundle-hash` version. **NEXT SESSION: hand-merge into one
`VerifyOptions` { kek, expected_bundle_hash, require_encryption, trusted_pubkey, require_signature }**
and one verify body + one set of CLI flags. The worktree is preserved for this.

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
