# Design — Runtime-checked contracts + counterexample evidence (Theme A-lite, Sprint 2)

Branch (create AFTER v1.6.0 merges): `feat/contracts-runtime` off updated master.
Borrowed from Vera/Aver (contracts) + **Vow** (counterexample = concrete replayable input + structured blame). NO SMT/Z3 — aligns with the recorded 1.5.0 "Decided" ruling against symbolic model checking; stays in Boruna's concrete-trace + replay philosophy. User chose this over full-Z3.

## Grounding (verified)
- `FnDef.requires: Vec<Expr>`, `FnDef.ensures: Vec<Expr>` already parsed, currently UNUSED (dead fields).
- `Op::Assert(err_const)` exists in bytecode + VM: pops value, if falsy → `VmError::AssertionFailed(msg=const)`. **Never emitted by codegen; no `assert` builtin.** → free to repurpose exclusively for contracts.
- codegen `emit_function`: registers params as locals 0..n, emits body, appends implicit `Ret`.
- VM frame has locals; current function arity is `module.functions[idx].arity` → args = locals[0..arity].

## MVP scope (Sprint 2)
`requires` preconditions → runtime guards with a concrete counterexample captured into evidence.
`ensures` (needs `result` binding + injection at every return point) → deferred to Sprint 2b, documented.

## Design
1. **codegen**: in `emit_function`, after params registered and BEFORE body, for each `requires` expr `e_i`:
   emit code for `e_i` (pushes bool) → `Op::Assert(const_i)` where `const_i` is a String constant
   `"precondition failed in <fn>"` (or numbered). Reuses the dormant Assert path.
2. **VM**: replace the `AssertionFailed` outcome of `Op::Assert` with a richer
   `VmError::ContractViolation { message, counterexample }` where `counterexample` = the current
   frame's args (`locals[0..arity]`) rendered as `Vec<String>` (positional — no param names needed
   in bytecode). Structured blame: `requires` violation ⇒ caller-fault (message says so).
   (Assert is contract-only, so changing its error shape is safe.)
3. **orchestrator**: when a workflow step fails with a ContractViolation, capture
   `{step_id, message, counterexample}` into the evidence bundle as `contract_violations.json`
   (via a new `EvidenceBundleBuilder::add_contract_violations`, same write_file → checksummed →
   hash-covered → verify pattern as Sprint 1's intents.json). The offending inputs become a
   replayable, tamper-evident audit record.

## Determinism
Contract checks are pure boolean evals over args → deterministic. A violation is a deterministic
function of the inputs; the counterexample IS the replay input (Vow). Replay-verified (feeds the
evidence bundle). No new nondeterminism.

## Bytecode version
No NEW opcode (reusing Assert) → no bytecode bump needed. If ensures (2b) needs a `result`-dup
opcode, that would be the 1.1→1.2 additive bump.

## Tests
- parse+codegen: `fn f(x: Int) -> Int requires x > 0 { x }` emits Assert at entry.
- VM: calling with x=-1 → ContractViolation, counterexample ["-1"]; x=5 → ok.
- evidence: a workflow step whose precondition fails records contract_violations.json, bundle verifies, tamper breaks verify.
- backward-compat: functions without requires emit no Assert (unchanged bytecode).

## Acceptance
requires preconditions enforced at runtime; violation yields a concrete counterexample in a
tamper-evident evidence component; gates green (test/clippy-1.97/fmt).
