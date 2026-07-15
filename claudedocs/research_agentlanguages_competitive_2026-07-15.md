# Competitive Research — agentlanguages.dev catalogue

**What ideas can Boruna borrow from the other 34 "AI-agent languages"?**

- **Date:** 2026-07-15
- **Source of truth:** [agentlanguages.dev](https://agentlanguages.dev/#catalogue) — "35 languages tracked," snapshot dated 13 Jul 2026
- **Method:** Fetched the catalogue + Boruna's own analysis page, then fanned out 5 parallel research agents across all 34 non-Boruna entries. Each agent read the per-project analysis page (and repo/README where quick), extracted the single distinctive mechanism, and rated its borrow-value *for Boruna specifically* (given Boruna's profile and named gaps).
- **Confidence:** Medium-High on the field map and per-project distinctive ideas (primary source is the catalogue's own analysis, which is opinionated but well-researched). Lower on exact maturity/benchmark numbers (several projects are preprints, thought experiments, or single-author early builds; a few self-disclaim correctness). **This is a research report — no implementation, no code, no architectural commitment. Recommendations are for human decision.**

---

## Executive summary

The field splits into **three camps**: **Syntactic** (strip ambiguity from the surface — SSA, JSON ASTs, English-keyword operators), **Verification** (make the compiler catch semantic errors — contracts, refinement types, SMT/BMC proofs), and **Orchestration** (it's not a language problem — sequence, sandbox, audit, approve). Boruna sits in **Orchestration, tagged "also verification."** Its differentiator — capability-safe-by-construction execution + hash-chained tamper-evident evidence + deterministic replay for regulated industries — is **unique in the catalogue**. No other project owns the "prove exactly what ran to a regulator" position.

Boruna's own analysis is flattering ("more carefully thought through than several entries with two orders of magnitude more attention") but its catalogue data is **stale** — it lists v0.2.0 / 34 commits / 557 tests, whereas the project is at **v1.5.0** with distributed execution, REPL, literate workflows, ITF export, and property-based `simulate`. Worth refreshing the listing.

Across 34 projects, **5 rate High borrow-value, 2 Medium-High, 17 Medium, 4 Low, 6 None.** The High/Medium-High cluster is strikingly coherent: almost everything worth stealing pushes on Boruna's **two named gaps** — *no formal contracts/SMT* and *no uncertainty quantification* — or **deepens the auditability moat Boruna already owns**. Crucially, the best ideas slot into subsystems Boruna *already has* (typechecker, capability system, EventLog, evidence bundles, `simulate`/witness DSL, MCP server) rather than requiring a rewrite.

**The five highest-value borrows:**

| # | Idea | From | Slots into | Why it wins |
|---|------|------|-----------|-------------|
| 1 | **Optional contracts (`requires`/`ensures`) with graduated discharge** — typecheck → property `simulate` (already have) → SMT (Z3) for decidable fragments → runtime guard that logs to EventLog | Vera, Aver, Vow, Intent, MoonBit | typechecker + `simulate` + EventLog/evidence | Closes the #1 named gap, reuses the invariant/witness DSL, and a failed guard becomes a *replayable evidence event* — pure Boruna thesis |
| 2 | **`intent`/`explain` annotations bound to machine-checked facts** — prose intent per step, compiler-verified to reference real contracts/capabilities (build fails on dangling/drifted reference), flowing into the evidence bundle | Pact, Intent, Prove, Aver | syntax + typechecker + literate workflows + evidence | Regulator gets "what was this step *supposed* to do" next to "what it did"; cheap; extends v1.5 literate workflows from unchecked narration to verified claims |
| 3 | **LLM inference as a tracked typed effect + calibrated uncertainty** — promote `llm` from boolean capability to an effect the typechecker propagates up the call graph; wrap model outputs in conformal-prediction confidence sets with a policy-set target error rate, recorded in evidence | Vera, Quasar, Lumen | capability system + typechecker + evidence | Fills the uncertainty gap; "this step transitively calls a model, at this confidence" becomes a static, auditable fact |
| 4 | **Analysis-gated approvals + typed recovery restarts** — approval fires only when static analysis *cannot prove* an effect is non-sensitive (cuts approval fatigue); failed steps surface named, policy-gated "restart" recovery options logged in the EventLog | Quasar, Pel | typechecker + distributed approval-gate + EventLog | Fewer, smarter human gates; richer than binary approve/reject; every choice stays in the evidence chain |
| 5 | **Hardened agent-facing surface** — stable *versioned* diagnostic codes with `fix`+`retry` fields; deterministic **structural (JSON-Patch) quickfixes** recordable in evidence; a token-budgeted project-context MCP tool; an `llms.txt`; and a version-locked auto-generated agent skill | Codong, X07, Zero, Aver/Lume, NERD, Vow | diagnostics + MCP server + tooling | Cheap, self-contained wins that directly improve the agent-authoring loop Boruna already targets |

**Reinforced (do NOT change):** Multiple projects independently converge on Boruna's existing choices — capability-explicit semantics, canonical/deterministic output, JSON diagnostics, VM-enforced determinism, separate human-readable surface over a bytecode IR. The catalogue also documents a **cautionary lesson** (B-IR's three failed attempts; the TBIR model that spontaneously rewrote control-character opcodes back into English): **do not token-golf `.ax` into a dense/alien syntax.** LLMs handle readable keywords *better*, and it would sacrifice the human-auditor half of Boruna's audience for a benefit Boruna doesn't need. Boruna's "syntax not token-optimized" gap is **safe to leave open.**

---

## The field, in one map

```
SYNTACTIC (representational)        VERIFICATION (semantic)              ORCHESTRATION (coordination)
strip ambiguity from the surface   make the compiler catch errors       constrain how agents coordinate
─────────────────────────────      ──────────────────────────────       ────────────────────────────────
Magpie  (SSA-as-syntax)            MoonBit (real-time type sampler) ★    Boruna  (evidence + replay) ← us
X07     (JSON ASTs)                Vera    (mandatory contracts+Z3) ★    Pel     (grammar-as-capability)
NERD    (English-keyword ops)      Aver    (verify→prove ladder)         Quasar  (conformal prediction) ★
Axis    (constrained decoding)     Intent  (verified_by no-drift)        Marsha  (LLM-as-compiler)
Laze/Sever/Mog/Lume/Lumen…         Vow     (BMC + counterexamples)        Fabro   (Graphviz DOT workflows)
                                   AILANG  (capability-row inference)     Plasm   (dry-run plan gate)
                                   Prove   (verbs-as-effects; anti-AI)    Spec    (role-owned IR artefacts)
                                   NanoLang(mandatory self-tests; Coq)    Plumbing(category-theoretic wiring)
                                   Pact/Codex/Codong/Tacit/Zero/BHC…
```

The camps blur most around Boruna — it already spans Orchestration + Verification. That overlap is exactly where the borrowable ideas concentrate.

---

## Thematic synthesis — where the ideas cluster

Instead of 34 disconnected suggestions, the borrow-worthy ideas group into **seven themes**. Themes A–C are the strategic ones (close named gaps / deepen the moat); D–G are tactical hardening.

### Theme A — Formal contracts, discharged in tiers *(the #1 gap)*
**Converging projects:** Vera (High), Aver (High), Vow (Med-High), Intent (High), MoonBit (Med), Prove (Med).

The verification camp's whole thesis is Boruna's biggest missing piece. The consensus mechanism, adapted to Boruna:

- Add **optional** `requires`/`ensures` (and loop `invariant`) contracts to `.ax`. Optional, not mandatory — Vera makes them mandatory, but Boruna's workflow-step model doesn't need that severity.
- **Graduated discharge** (Vera's three-tier / Aver's ladder): decidable clauses → **SMT (Z3)** at compile time; the rest → **runtime guard**; and reuse Boruna's existing **property-based `simulate` + invariant/witness DSL** (v1.5) as the sampled middle tier. A clause's tier can be chosen automatically by which arithmetic fragment it lives in.
- **A failed guard is an evidence event.** This is the Boruna-native twist (Vow's "counterexample = concrete replayable input + structured blame"): a violated contract drops a **hash-chained, re-runnable counterexample record** into the evidence bundle, replayable via ReplayEngine. That turns verification failures into auditable artifacts, not log lines — and upgrades `simulate` witnesses into first-class audit objects.
- **Structured blame** (Vow): a `requires` violation faults the caller, an `ensures` violation the callee — useful in multi-step workflows.

*Effort:* SMT integration is the heavy part (feature-flagged Z3, per ADR-001 dep discipline). The `simulate`-as-middle-tier and counterexample-as-evidence pieces are cheap and reuse existing code.

### Theme B — Intent bound to machine-checked facts *(deepen the moat)*
**Converging projects:** Pact (High), Intent (High), Prove (Med), Aver (High).

Boruna proves *what ran*. These projects prove *what it was supposed to do* — and bind the two so they can't drift:

- **`intent "..."` in the signature** (Pact): a first-class, machine-read intent string per `fn`/workflow-step, captured into the evidence hash chain. Trivial cost, direct regulator payoff.
- **`verified_by` referential integrity** (Intent): the human-readable intent names the specific contracts/capabilities it relies on (`BankAccount.withdraw.requires`); the compiler resolves every reference in semantic analysis and **fails the build on a dangling one.** Prose provably cannot drift from the checked facts.
- **Verified `explain` blocks** (Prove): controlled-natural-language docs parsed and checked against the called functions' declared effects/contracts.
- **Net effect on v1.5 literate workflows:** upgrade markdown↔`.ax` literate prose from *unchecked narration* to *compiler-verified claims about a step's declared effects and intent.* This is the single most on-thesis, lowest-cost cluster.

### Theme C — LLM effect as a typed, tracked, budgeted, calibrated thing *(the #2 gap)*
**Converging projects:** Vera (High), Quasar (High), Lumen (Med).

Boruna gates `llm` as a boolean capability. The field goes three steps further:

- **Typed effect propagation** (Vera): make `llm` an effect the typechecker propagates *up the call graph*, so "this workflow step transitively invokes a model" is statically visible and surfaced in evidence — a small typechecker/capability change, large auditability payoff.
- **Parameterized grants** (Lumen): attach numeric budgets to the authorization, not just a boolean — `!{llm.chat(max_tokens=1024)}` — producing quantifiable evidence entries for cost/usage governance.
- **Calibrated uncertainty** (Quasar): wrap model outputs in **conformal-prediction sets** with a policy-declared target error rate, recorded in the evidence bundle so regulators see *calibrated uncertainty*, not a bare string. This directly fills Boruna's named "no uncertainty quantification" gap. (Quasar reports 42–56% faster execution and ~52% fewer approvals in its paper; treat numbers as unverified preprint claims.)

### Theme D — Least-privilege capability tightening
**Converging projects:** AILANG (Med), Mog (Med), Plumbing (Med), Lumen (Med).

- **Capability-row inference** (AILANG): the typechecker computes each function's *minimal* capability set from its callees and flags any `!{...}` annotation that **over-declares** authority. Makes "this step's declared effects are exactly what it can reach" a derived, auditable fact.
- **`optional` capabilities** (Mog): a step that degrades gracefully when a capability isn't granted, rather than hard-failing — useful for regulated "reduced-capability mode" runs, which Boruna's binary allow/deny Policy can't currently express.
- **Static information-flow / data-visibility typing** (Plumbing): the strongest *new* guarantee here. Boruna gates *effects* (can this step call the network) but not *data visibility* (can step B observe the raw document step A read). Encoding "this step may not observe field X" in the typechecker gives a **data-minimisation / PII-partitioning** guarantee that regulators care about — a genuine extension of the compliance edge, not a re-skin. (The category-theoretic framing itself is not needed.)

### Theme E — Agent-facing surface & structured, deterministic repair
**Converging projects:** Codong (Med), X07 (Med), Zero (Low), Aver/Lume (Med), NERD (Med), Vow (Med-High), MoonBit (Med), B-IR (lesson).

Boruna already has spans + suggested patches + a 10-tool MCP server. Cheap hardening the field has converged on:

- **Structured error contract** (Codong): every diagnostic *and* every VM/capability-denial/worker error carries a stable `code` + `fix` + `retry` boolean, so an agent remediates without parsing prose. Especially valuable for the `boruna_check`/`boruna_repair` loop and distributed-runtime failures.
- **Stable *versioned* diagnostic codes** (Zero, B-IR, Codong): codes contractually stable across compiler versions — agents match on the code, not the message. (Aligns with Boruna's existing `error_kind`/`protocol_version` convention — extend that discipline to all diagnostics.)
- **Deterministic structural quickfixes** (X07): express repair patches as a **stable structural format (RFC 6902 JSON Patch)** the toolchain applies mechanically, so auto-repair is reproducible and *evidence-recordable* — a natural fit for Boruna's determinism story.
- **Token-budgeted project-context MCP tool** (Aver's `aver context`, Lume's `kb pack`): a `boruna context --budget 10kb` MCP tool that packs types/effects/intents/diagnostics under a caller-set token cap, instead of dumping whole source/specs.
- **`llms.txt` teaching corpus** (NERD): a single-fetch syntax+capability primer for coding agents. Near-zero cost.
- **Self-generated, version-locked agent skill** (Vow): generate the coding-agent skill / `AGENTS.md` from the *same source* as the CLI, so guidance can't drift from behavior.
- **Incremental typecheck over MCP** (MoonBit): expose a partial/incremental check endpoint an agent (or grammar-constrained decoder) can query mid-generation to reject ill-typed continuations — a reachable adaptation of MoonBit's famous token-sampler without owning model serving.

### Theme F — Reproducibility & provenance, enforced not asserted
**Converging projects:** Codex (Med), Tacit (Med), NanoLang (Med-High), Plasm (Med), Spec (Low).

- **Reproducibility as a CI merge-gate** (Codex): require that recompiling an `.ax` module + re-running under replay yields **byte-identical bytecode and an identical evidence hash-chain**, failing the build otherwise. Turns "every run reproducible" from an ad-hoc-tested property into a continuously-enforced invariant.
- **Content-addressed definition identity** (Tacit): hash at *definition* granularity (BLAKE3 of canonical text), not just per-package. Lets an evidence bundle pin the exact function bodies that executed and makes replay drift detectable at the symbol level.
- **Mandatory inline self-tests** (NanoLang's `shadow` blocks): the compiler refuses to compile a function/step without a paired assertion block; results recorded in the hash chain → "every step shipped with a passing self-test." Boruna's `TestHarness` already exists to build on.
- **Dry-run plan-then-execute-identical-artefact gate** (Plasm): compile a workflow to its execution DAG + declared-effect manifest, present *that* for review/approval, then execute the identical artefact. An explicit reviewable "here is exactly the effect plan I'm about to run," distinct from post-hoc evidence.

### Theme G — Recovery & approval enrichment
**Converging projects:** Pel (Med), Quasar (High, see Theme C/D), Plasm (Med).

- **Condition/restart recovery model** (Pel, from Common Lisp restarts): a failed step surfaces a structured set of named, policy-gated resume options, chosen by a human approver or helper agent, each logged in the EventLog — resumable, typed recovery points as a richer alternative to binary approve/reject. Determinism preserved because the chosen restart is recorded.

---

## Full catalogue — per-project borrow ratings

Ordered by borrow-value for Boruna. "Idea" = the one thing worth extracting (not necessarily the project's headline feature).

| Project | Camp | Maturity (per catalogue) | Rating | Borrowable idea → Boruna subsystem |
|---|---|---|---|---|
| **Aver** | Verification | Rust, ★52, working | **High** | Graduated verify→adversarial→formal-proof ladder; token-budgeted `context` export → tooling + MCP |
| **Intent** | Verification | Go→Rust/JS/WASM, working | **High** | `verified_by` prose↔contract no-drift binding (build fails on dangling ref) → typechecker + literate |
| **Pact** | Verification | Rust, working | **High** | `intent` in signature → evidence bundle; declarative deterministic effect-swap in tests → syntax + replay |
| **Quasar** | Orch.+Verif. | UPenn preprint, no impl | **High** | Conformal-prediction confidence sets on LLM effects; analysis-gated approvals → capability + evidence + gates |
| **Vera** | Verification | Python→WASM, ★300+, working | **High** | LLM inference as tracked typed effect; three-tier contract discharge (Z3/guard/check) → typechecker + capability |
| **NanoLang** | Verif.+Synt. | C, multi-target, working | **Med-High** | Mandatory inline `shadow` self-tests recorded in evidence → compiler + TestHarness + evidence |
| **Vow** | Verification | Rust self-hosting, working | **Med-High** | Counterexample-as-replayable-evidence-artifact; self-generated version-locked toolchain skill → simulate + MCP |
| **AILANG** | Verification | Go, Apache-2.0, working | Medium | Capability-row inference (flag over-declared authority) → typechecker + capability |
| **Codex** | Verif.+Orch. | self-hosting, working | Medium | Byte-identical reproducibility as CI merge-gate → CI + evidence |
| **Codong** | Syntactic | Go, ★68, working | Medium | `code`/`fix`/`retry` structured error schema → diagnostics + MCP |
| **Fabro** | Orchestration | Rust, working | Medium | CSS-like selector+specificity overlay to assign model/effort/policy to nodes → orchestrator/DAG |
| **LLMLang** | Synt.+Verif. | Rust→LLVM+OpenCL, working | Medium | Marker-triggered compiler-injected instrumentation (OTel spans) → codegen + EventLog |
| **Lume** | Syntactic | Go transpiler, early | Medium | Token-budgeted KB retrieval (`kb pack --max-tokens`) as MCP tool → MCP + tooling |
| **Lumen** | Orchestration | Rust→bytecode VM+WASM, working | Medium | Parameterized capability grants w/ numeric caps (max_tokens, temp) → capability + evidence |
| **Marsha** | Orchestration | Python, abandoned 2023 | Medium | Examples→generated test suite→bounded corrective-repair loop → trace2tests + repair |
| **Mog** | Synt.+Verif. | Rust→QBE, working (embedded) | Medium | `optional` capabilities / graceful degradation; host capability manifest → CapabilityGateway + Policy |
| **MoonBit** | Verification | OCaml, ★2115+, most mature | Medium | Incremental/partial typecheck over MCP for constrained generation → compiler + MCP |
| **NERD** | Syntactic | C→LLVM, ★135, working | Medium | `llms.txt` teaching corpus; first-class capability-gated `llm`/`mcp` primitives → MCP + syntax |
| **Pel** | Orchestration | CMU preprint, no impl | Medium | Condition/restart typed recovery model for approval gates → distributed runtime + gates |
| **Plasm** | Orch.+Synt. | Rust, BSL-1.1, working | Medium | Dry-run plan-then-execute-identical-artefact gate → CLI + approval gate |
| **Plumbing** | Substrate | preprint, native, undisclosed | Medium | Static information-flow / data-visibility typing between steps (PII partitioning) → typechecker + PolicySet |
| **Prove** | Verification | Python→C, working | Medium | Verified `explain` blocks (CNL checked vs contracts) → literate + typechecker |
| **Tacit** | Synt.+Verif. | Rust→LLVM, working | Medium | Content-addressed definition-level hashing; typed `Hole` nodes → evidence + package + repair |
| **X07** | Syntactic | Rust→C+WASM, working | Medium | JSON-Patch deterministic quickfixes; versioned machine-facing portal → repair + MCP |
| **Axis** | Synt.+Verif. | Rust, ★3, working | Low | Grammar-as-state-machine for constrained decoding — off-thesis (Boruna is post-hoc) |
| **Magpie** | Syntactic | Rust→LLVM, early | Low | SSA-as-surface — philosophically opposite to readable-surface+separate-IR |
| **Spec** | Orch./unclass. | TS/React POC, proposal | Low | Role-owned typed artefacts w/ handoff ordering (provenance framing only) → orchestrator + evidence |
| **Zero** | Verif.+Synt. | C bootstrap, early (Vercel Labs) | Low | Stable versioned diagnostic codes (rest convergent) → diagnostics |
| **B-IR** | Syntactic | Python→Arm64, thought experiment | None | *Lesson:* don't token-golf; keep stable matchable error codes + pre/postconditions |
| **BHC/hx** | Verif.+Orch. | Rust wrapper, early | None | Thesis-agreement (purity/determinism) but no transferable mechanism |
| **Koru** | unclassified | Zig superset, pre-alpha | None | Mandatory event-branch exhaustiveness — already covered by Boruna's `match`/`UpdateResult` |
| **Laze** | Syntactic | Python→C, weekend experiment | None | Strip-surface-for-generation-speed — antithetical to auditability |
| **Sever** | Syntactic | Zig, self-disclaimed art | None | Single-char opcode density — unverified, conflicts with readability |
| **Valea** | unclassified | Rust MVP, "noise floor" marker | None | Nothing shipped Boruna doesn't already have |

Distribution: **High 5 · Med-High 2 · Medium 17 · Low 4 · None 6.**

---

## Recommended next steps (for human decision — not executed)

1. **Refresh the catalogue listing** — Boruna is shown at v0.2.0/★0; the real project is v1.5.0 with distributed execution, REPL, literate workflows, ITF export, and `simulate`. A quick PR/message to the catalogue maintainer (Negroni Venture Studios) corrects a materially stale entry.
2. **Pick the moat-deepening cluster first (Themes A+B+C).** They close both named gaps *and* reuse existing subsystems (`simulate`/witness DSL, typechecker, capability system, evidence bundles, literate workflows). Suggested spike order:
   - **B first** (cheapest, highest thesis-fit): `intent` in signatures → evidence; `verified_by`/`explain` prose↔contract binding checked by the compiler.
   - **A next**: optional `requires`/`ensures`, with `simulate` as the sampled tier and a runtime guard that logs a replayable counterexample to the EventLog; feature-flag Z3 for the decidable tier later.
   - **C alongside**: `llm` as a propagated typed effect + conformal-prediction confidence in evidence.
3. **Batch the tactical hardening (Theme E)** into one DX sprint — structured `code/fix/retry` diagnostics, versioned codes, JSON-Patch quickfixes, `boruna context` MCP tool, `llms.txt`, auto-generated version-locked agent skill. All small, all reinforce the agent-authoring loop.
4. **Consider Theme F's reproducibility merge-gate** as a CI addition — it enforces Boruna's loudest claim continuously and is cheap given the evidence machinery already exists.
5. **Explicitly decide NOT to** token-optimize `.ax` syntax, adopt JSON-AST-as-source, or add mandatory (vs optional) contracts. The catalogue's own evidence argues against the first two; the third doesn't fit a workflow-step model.

> Per `/sc:research` boundaries: this report stops at findings + recommendations. Use `/sc:design` to turn any theme into an architecture, or `/sc:implement` to build a spike.

---

## Sources

- agentlanguages.dev catalogue (index) — https://agentlanguages.dev/#catalogue
- Boruna analysis page — https://agentlanguages.dev/languages/boruna/
- Per-project analysis pages — https://agentlanguages.dev/languages/{ailang,aver,axis,b-ir,bhc-hx,codex,codong,fabro,intent,koru,laze,llmlang,lume,lumen,magpie,marsha,mog,moonbit,nanolang,nerd,pact,pel,plasm,plumbing,prove,quasar,sever,spec,tacit,valea,vera,vow,x07,zero}/
- Framing essay: "Three camps alike in dignity" — Negroni Venture Studios blog, 2026-05-20
- Boruna internal profile: repo `CLAUDE.md`, Serena memories (architecture, project-conventions-2026-04, release-1.5.0-status)

*Primary evidence throughout is the catalogue's own per-project analysis (opinionated but well-sourced). Maturity/benchmark figures for preprint and single-author projects are unverified and flagged as such.*
