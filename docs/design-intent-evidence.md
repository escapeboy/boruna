# Design — Intent annotations → evidence bundles (Theme B, Sprint 1)

**Program context:** First of a sequenced multi-sprint program borrowing the 5 High-rated ideas from the agentlanguages.dev competitive research (`claudedocs/research_agentlanguages_competitive_2026-07-15.md`). Order: **B → A → C → D**, each its own minor release.

- **Sprint 1 (this doc):** Theme B — machine-read `intent` per function/step, captured into the evidence bundle; prose↔fact binding in literate workflows.
- Borrowed from: **Pact** (`intent` in signature → audit record), **Intent** (`verified_by` no-drift binding), **Prove** (verified `explain` blocks), **Aver** (`?` prose intent).

## Forcing questions (Think)

- **Who needs this?** Regulators/auditors reading a Boruna evidence bundle. Today they see *what ran* (inputs, outputs, model responses, approvals, hashes) but not *what each step was supposed to do* — they must cross-reference external design docs.
- **Narrowest MVP someone would pay for?** An `intent "..."` clause on an `.ax` function that is captured verbatim into the hash-chained evidence bundle for every executed step. "Declared purpose" sits next to "actual behaviour," both tamper-evident.
- **What makes someone say "whoa"?** The prose can't lie: a literate-workflow narrative that references a step's declared intent/capabilities is **compiler-verified** — a dangling or drifted reference fails the build. Documentation that is mechanically bound to the code it describes.
- **How does it compound?** Every later theme (contracts, typed LLM effects, capability inference) produces machine facts that intent/explain prose can cite via the same no-drift binding — the audit narrative grows richer without ever drifting.

## Scope

**In (Sprint 1):**
1. `intent "<string>"` clause on `fn` definitions (`.ax` syntax).
2. Thread intent through: lexer → AST → parser → codegen → bytecode `Function` (additive, serde-default; old modules load with `intent: None`).
3. Surface intent in `boruna ast` (automatic via serde) and `boruna compile` module info.
4. Capture the entry function's intent into the evidence bundle step record.
5. Tests: parse, serialize roundtrip, evidence capture, backward-compat load.

**Deferred to a Sprint 1b slice (still Theme B):**
- `verified_by` / `explain` prose↔fact binding in literate workflows (larger literate-tooling change; lands after the core `intent` primitive ships).

**Out (later themes):** contracts/SMT (A), typed LLM effects/uncertainty (C), capability inference/info-flow (D). Note: `FnDef` already carries `requires`/`ensures` Vec<Expr> fields (unused today) — Theme A will build on those; this sprint does not touch them.

## Data flow

```
fn transfer(x: Int) -> Int !{db.write} intent "Move funds between accounts"
        │
   lexer: TokenKind::Intent + StringLit
        │
   parser::parse_fn_def → FnDef { intent: Some("Move funds…"), .. }
        │
   codegen → bytecode Function { intent: Some(...), capabilities, .. }  (serde, #[serde(default)])
        │
   orchestrator step run → read entry fn intent → evidence bundle step record
        │
   evidence verify / inspect → intent shown, inside the hash chain
```

## Design decisions

- **`intent` is `Option<String>`, optional.** Unlike Pact/Vera (mandatory), Boruna steps don't all need prose; forcing it would be gold-plating and break existing `.ax`.
- **Grammar position:** after capability annotations, before `requires`/`ensures`. One `intent` per fn (a second is a parse error — keeps it a single declarative purpose, matching Pact).
- **Additive bytecode.** `Function.intent` gets `#[serde(default)]`. Determinism note (§15): intent is **replay-verified** input to the evidence hash (it's declared source, part of "what was authorized"), NOT operational-only — changing a step's intent changes its evidence identity, which is correct.
- **No new dep.** Pure additive language/serialization change.

## Acceptance criteria

- `fn f() -> Int intent "does a thing" { 0 }` parses; `intent` visible in `boruna ast --json`.
- A module compiled without `intent` loads unchanged; a pre-Sprint-1 serialized module deserializes with `intent: None`.
- Two `intent` clauses on one fn → parse error with a clear message.
- An executed workflow step's evidence record contains the declared intent; `evidence verify` still PASSES (hash chain intact) and `evidence inspect` shows it.
- Gates green: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`.

## Test plan

| Case | Layer | Assert |
|---|---|---|
| parse single intent | parser | `FnDef.intent == Some("...")` |
| parse no intent | parser | `FnDef.intent == None` |
| double intent | parser | `Err` with message naming duplicate intent |
| intent + capabilities + requires order | parser | all three parsed |
| codegen threads intent | codegen | `Function.intent == Some("...")` |
| serialize roundtrip | bytecode | intent survives serde round-trip |
| legacy module load | bytecode | JSON without `intent` key → `None` |
| evidence capture | orchestrator | step record carries intent; bundle verifies |
