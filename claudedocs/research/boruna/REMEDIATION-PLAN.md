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

### ✅ E-sign — ed25519 manifest signing — DONE + RECONCILED (2026-07-17, `01ca774`)
Reconciliation agent reset to the branch tip, cherry-picked the ed25519 work, and unified both
verify features into ONE `VerifyOptions { kek, expected_bundle_hash, require_encryption,
trusted_pubkey, require_signature }` + one verify body + all 4 CLI flags. 10 verify tests
(anchor + signature) green. The #1 High now has BOTH an external-anchor AND a signature path.

### ✅ G higher-order/indirect calls — DONE (2026-07-17, `9e7e25d`)
New `Op::CallIndirect` — a function passed as a value now dispatches correctly (was hardcoded to
fn #0). `apply(double, 21) == 42` test.

### ✅ Enum construction + per-variant match tags — DONE (2026-07-17, `e7a3fac`)
Root cause was bigger than "codegen one-liner": `EnumVariant` existed only as a *Pattern*, never as
an *Expr*, and the lexer had no `::` token — user enums could be DECLARED and MATCHED but never
CONSTRUCTED, so `pattern_to_tag`→-1 was dead code (every enum arm collapsed to the first). Built the
whole feature end to end: lexer `::` token; `Expr::EnumVariant { enum_name, variant, payload }`;
parser `Enum::Variant`/`Enum::Variant(payload)`; codegen `MakeEnum(type_id, variant_idx)` +
`resolve_enum_variant()`; `pattern_to_tag` now a method resolving variant name→declaration index (VM
`Op::Match` already dispatches on `Value::Enum.variant`, so NO vm change). Propagated the new Expr to
the exhaustive matches: tooling format printer, diagnostics walkers, `serve.rs` cap-tag collector,
typeck. Additive/non-breaking (`::` was never a valid token). +2 e2e tests. Known limit: duplicate
variant names across enums resolve to first-match in patterns (VM matches on index, ignores type_id).

### ✅ Arity checking — DONE (2026-07-17, `5110a0b`)
Type checker now rejects a direct call to a named function with the wrong argument count. Local/param
callees (first-class fn values) are skipped — `Op::CallIndirect` covers them. Non-breaking: verified
green across compiler/vm/tooling/framework/orchestrator — the corpus carries no arity mismatches.

### ⏳ REMAINING — full strict type inference (LTS-breaking, needs a user decision)
The additive, non-breaking checks are done. What's LEFT is the genuinely LTS-breaking part: type
inference, match exhaustiveness, record-field typing, `requires`/`ensures` typed to Bool. This REJECTS
existing loose `.ax` across the corpus (stdlib libs, examples, framework apps) → needs a corpus
migration + a strictness-policy decision (warn-only first? `--strict` flag? hard-break at 2.0?). Do
NOT fire-and-forget; surface the strictness decision to the user first.

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
