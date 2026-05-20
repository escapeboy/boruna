# Quint (quint.sh) — Borrowable Ideas for Boruna

**Date:** 2026-05-20
**Source under review:** https://quint.sh and the Informal Systems Quint repo
**Boruna baseline:** v1.4.0 (shipped 2026-05-17) — agent-native CLI, boruna-lsp, compliance example workflows
**Output type:** research report only; no implementation

---

## Executive summary

Quint is an executable specification language built by Informal Systems on the same logical foundation as TLA+ (Temporal Logic of Actions), but with a TypeScript-flavored surface syntax and modern tooling: REPL, simulator, symbolic model checker (Apalache), LSP, VS Code extension, and a separate Rust library (Quint Connect) that drives implementations from validated specs. Its design audience is engineers who write distributed-systems specs and want fast feedback.

Boruna and Quint occupy adjacent but distinct slots. Boruna is a *runtime* with hash-chained evidence bundles for compliance. Quint is a *specification environment* whose verifier output is intended to *guide* implementation. They share more than is obvious: both have a typed language compiled to bytecode-or-IR, both target deterministic execution, both treat traces as artifacts, both serve auditable systems.

Five Quint ideas would genuinely strengthen Boruna without conflicting with its philosophy. The single biggest miss is a **REPL** (`boruna repl`) — Quint's REPL is its most-praised feature and Boruna has nothing comparable. Three more are mid-effort wins: **random property-based simulation** (`boruna simulate`), **literate workflows** (compliance audits as markdown + `.ax` blocks), and adopting the **ITF (Informal Trace Format)** so Boruna's trace2tests and evidence bundles interoperate with the broader formal-methods ecosystem. A fifth (the **Quint Connect** spec-as-oracle pattern for testing Rust impls against a spec) is more speculative but maps cleanly onto Boruna's existing `trace2tests` and `evidence verify` machinery.

The Apalache-style **bounded model checker** is *not* recommended — Boruna's design philosophy is concrete-trace + replay, and pulling in an SMT-backed symbolic checker would require a fundamental architectural shift that the current customer base does not need.

**TL;DR rank order:** REPL > random simulation w/ seeds > literate specs > ITF trace format > Connect-style impl testing.

---

## What Quint actually is (1-paragraph framing)

Quint is published by Informal Systems (the team behind the Cosmos SDK and Apalache) as a frontend to the temporal logic of actions. A `.qnt` file declares state variables, an `init` action, a `step` action, and optionally `invariant` / `temporal` properties. The toolchain is one `npm install -g @informalsystems/quint`. Six CLI subcommands matter: `repl`, `run` (random simulation), `test`, `verify` (bounded model checking via Apalache+Z3), `compile`, `parse/typecheck`. Quint's distinguishing semantic choices: explicit **modes** for each definition (stateless / state / non-determinism / action / run / temporal), an **effect system** that classifies state reads/writes for coherence checking, and **runs as first-class** values (concrete execution paths that can be named, stored, and replayed as tests).

Selected verbatim from quint.sh: *"Quint extends the standard programming paradigm with non-determinism and temporal formulas, which allow concise specification of protocol environments such as networks, faults, and time."*

---

## Side-by-side: Quint surface vs. Boruna surface

| Capability | Quint | Boruna (1.4.0) | Gap? |
|---|---|---|---|
| Surface syntax | TS-inspired, ASCII, minimal | TS-inspired, ASCII, minimal | none |
| Type system | Static, inferred, records/sets/maps | Static, records/enums/lists/maps | none |
| Effects on language | Effect system (reads/writes of state) | Capability annotations `!{net.fetch}` | partial — different axis |
| Modes | Explicit (stateless/state/action/run/temporal) | Implicit — capability annotations only | medium gap |
| REPL | Yes (`quint repl`), interactive eval | None | **HIGH gap** |
| Random simulation | `quint run` w/ `--seed`, `--max-samples`, `--max-steps`, `--n-traces` | None (workflows are concrete) | **HIGH gap** |
| Bounded model checker | `quint verify` via Apalache + Z3 | None (concrete-replay only) | gap, but NOT recommended to close |
| Witness queries | `quint run --witnesses=...` reports trace-hit % | None | medium gap |
| Trace format | ITF (Informal Trace Format) JSON | Custom evidence bundle | gap — interop |
| Unit tests in spec | `quint test`, `run`-blocks inside spec | Rust integration tests external to `.ax` | medium gap |
| Compilation targets | `tlaplus`, `json` IR | bytecode (`.bb`) | none — Boruna ships bytecode |
| VS Code extension | Yes | LSP shipped 1.4.0; VS Code wrapper TBD | low gap |
| Literate spec | `quint`-blocks in `.md` → extracted `.qnt` | Not supported | medium gap |
| Spec-driven impl testing | Quint Connect (Rust lib) | `trace2tests` + replay (different shape) | medium gap, complementary |
| `q::debug` print-passthrough | Built-in operator | None | trivial |
| Bytecode/IR for downstream tooling | JSON IR (`compile --target=json`) | `.bb` + JSON via `ast` | parity |

Boruna's surface that Quint does *not* have: capability gateway with policy enforcement, hash-chained evidence bundles, blob store for large outputs, distributed coordinator/worker, approval gates, MCP server with 10 typed tools, mTLS auth.

---

## Borrow recommendations (ranked)

### 1. `boruna repl` — interactive evaluation of `.ax` modules (HIGH priority)

**What Quint does:** `quint repl` boots a REPL with the spec in scope. You can call individual actions, evaluate expressions, step the state machine forward (`init`, `step`, `step`, `step`), inspect the current state, and exercise non-deterministic choices. Tutorial 3 ("Coin") shows random runs from the REPL.

**Why borrow:** Boruna's iteration loop today is *edit → `cargo run -- run file.ax`* — a full re-compile cycle per question. A REPL collapses that to seconds. This is the single most quoted Quint feature ("near-instant feedback") and the largest DX gap in Boruna.

**How it could fit:** A `boruna repl` subcommand in `crates/llmvm-cli/src/` loading the same compile pipeline as `boruna run`, but keeping the VM hot between inputs. The Boruna VM already supports `Vm::execute_bounded` (actor-system sprint, Feb 2026) which gives the right primitive for stepwise REPL evaluation. State recovery between inputs is straightforward — the VM's `Value` enum is fully serializable.

**Effort:** medium (probably 2–3 sprints). The actor-system stepwise execution already exists; the work is line editing, prompt rendering, error recovery, and persistent VM state.

**Cite:** [Quint REPL tutorial](https://quint.sh/docs/repl).

### 2. Random property-based simulation: `boruna simulate` (HIGH priority)

**What Quint does:** `quint run spec.qnt --invariant=safety --max-samples=10000 --max-steps=20 --seed=0xabc123 --n-traces=10 --out-itf=traces/`. The simulator does a guided random walk through the state machine, keeps the N longest traces (or shortest counterexamples), checks invariants on every state, and emits reproducible failures with a printable seed.

**Why borrow:** Boruna's workflow story today is **"run it once with the inputs the customer specified, archive the evidence."** That's correct for *deterministic compliance*, but it tells you nothing about *what could happen* if a different message arrives or a step fails non-deterministically. For workflows involving approval gates, external triggers, retries, and worker failure (all shipped in 0.5.x), random property simulation is the natural next layer. It would tell a customer "we ran 10,000 simulated runs of your approval pipeline with random worker-failure injection; invariant X held in 9,997 of them; the 3 violations are reproducible with seeds A/B/C."

**How it could fit:** A `boruna simulate <workflow-dir>` subcommand built on the existing orchestrator runner. The random source is a seedable RNG (already present in capability gateway tests). Invariants are declared in `workflow.json` as `.ax` expressions evaluated against the data store after each step. Traces serialize via the existing evidence-bundle machinery, optionally to ITF (see #4).

**Effort:** medium-high (orchestrator hook + invariant evaluation + seed plumbing). Adversarial-review caution: §15 of project-conventions says wall-clock keying is non-deterministic on failure — random simulation must NOT touch operational columns or it will produce flaky bundles.

**Cite:** [Quint simulator docs](https://quint.sh/docs/simulator), [run command](https://quint.sh/docs/quint).

### 3. Literate workflow specs: `.md` + `.ax` blocks (HIGH priority, audience-aligned)

**What Quint does:** A literate spec is a markdown file with `quint myspec.qnt += ...` code fences. The `lmt` tool extracts code blocks into `.qnt` files. The same document is both human-readable explanation and machine-checkable spec. The Quint FAQ specifically recommends replacing markdown design docs with literate Quint, because "Markdown accepts all kinds of errors: undefined names, type mismatches, impossible claims and ambiguous interfaces."

**Why borrow:** Boruna's 1.4.0 shipped compliance example workflows (`examples/compliance/{soc2_audit_workflow,hipaa_data_pipeline,financial_review_pipeline}`). The whole *point* of those workflows is they are auditable specifications. A compliance officer reading `soc2_audit_workflow` today has to flip between the `.ax` source and a separately-maintained README. Literate workflows fuse those — the audit narrative *is* the executable spec, with no risk of doc drift.

This maps directly onto Boruna's positioning ("auditable evidence bundles for AI workflows"). The compliance customer is *exactly* the user who needs a single document that legal can read and the engine can execute.

**How it could fit:** A `boruna literate extract <file.md>` command (or as part of `boruna workflow validate`) that pulls code fences into `.ax`/workflow files. Conventions reuse Quint's `<filename> +=` syntax. Optionally publish a `boruna literate render` that produces audit-friendly HTML or PDF for legal review (Boruna already has `dashboard.rs` + `evidence_serve.rs`, so the HTML render path exists).

**Effort:** small-medium. Pure tooling layer, no language changes. Could ship as part of `tooling/src/literate/` next to the existing `templates/`.

**Cite:** [Literate Specifications](https://quint.sh/docs/literate), [Quint FAQ on markdown replacement](https://quint.sh/faq).

### 4. Adopt the Informal Trace Format (ITF) (MEDIUM priority)

**What Quint does:** When the simulator or Apalache produces a trace, it serializes as **ITF JSON** — a small, documented schema that the [ITF VS Code Trace Viewer](https://marketplace.visualstudio.com/items?itemName=informal.itf-trace-viewer) consumes. ITF is also output by Apalache directly, by Quint's `compile --target=tlaplus`, and increasingly by other formal-methods tools.

**Why borrow:** Boruna's evidence bundles use a custom JSON schema (`orchestrator/src/audit/`) tied to its execution model. ITF is a *narrower* schema (it doesn't carry hash chains or env fingerprints) but it's an industry-standard envelope for "sequence of typed states." If Boruna emitted an `evidence inspect --format=itf` view (or `trace2tests --out-itf`), every Quint/Apalache visualization tool would work on Boruna traces out of the box. This is roughly the same play Boruna already made with the agent-native `--json` outputs in v1.4.0 — make Boruna interoperable with the surrounding ecosystem instead of insisting on a bespoke format.

**How it could fit:** Add an `--out-itf` flag to `boruna trace2tests` (already exists at `tooling/src/trace2tests/`) and to `boruna evidence inspect`. ITF schema is ~50 lines; no new dependencies needed. Evidence bundles stay in their current format; ITF is an *export*, not a replacement.

**Effort:** small. Pure additive serializer.

**Cite:** [Quint CLI verify, `--out-itf`](https://quint.sh/docs/quint), [Quint MBT docs](https://quint.sh/docs/model-based-testing).

### 5. Witness queries: `--witnesses=<expr>` (MEDIUM priority)

**What Quint does:** A *witness* is a predicate that should be true in *some* state (the opposite of an invariant, which must be true in *every* state). `quint run --witnesses=alice_more_than_bob` reports that the expression held in 7094 of 10000 explored traces (70.94%). Quint markets this as "a practical alternative to liveness properties."

**Why borrow:** Once `boruna simulate` (#2) exists, witnesses are a 100-line addition that makes the simulator dramatically more useful. Boruna customers want to ask things like *"in our SOC2 workflow, do we ever actually hit the rejection path?"* or *"under random worker failure, does any approval gate stay pending longer than 24h?"* — those are witness questions, not invariant questions.

**Effort:** small (assuming #2 ships first).

**Cite:** [Checking Properties → witnesses](https://quint.sh/docs/checking-properties).

### 6. Mode discipline at the type-check layer (MEDIUM priority, conceptual)

**What Quint does:** Every definition is tagged with one of six **modes**: Stateless, State, Non-determinism, Action, Run, Temporal. Modes form a partial order (`<m`); operators declare what mode their arguments must be in. The type checker rejects mode violations. The Quint design rationale explicitly says: *"We have found that the lack of such a clear separation [in TLA+] causes lots of confusion."*

**Why borrow:** Boruna's `boruna-framework` crate already separates `init / update / view` (Elm architecture) and capability annotations gate side effects. But there's no first-class distinction inside the *pure* layer between "this expression touches state" vs. "this is stateless." `view` functions, for instance, *should* be stateless-on-records — but nothing in the type checker enforces it. Adding a coarser mode lattice (probably just `Pure / State / Action`) at the framework boundary would tighten the contract without bloating the surface.

**How it could fit:** Annotation on functions (already supported via capability tags) extended to mode tags; or, more elegantly, inferred. The `update(State, Msg) -> UpdateResult` signature implicitly already has the right shape — just needs to be enforced.

**Effort:** medium. Touches the type checker (`crates/llmc/src/typeck.rs`). Risk: backward-compat with existing `.ax` files in std libs.

**Cite:** [Summary of Quint → Modes](https://quint.sh/docs/lang).

### 7. Spec-as-oracle impl testing (Connect-style) (LOW-MEDIUM priority, speculative)

**What Quint does:** **Quint Connect** is a separate Rust library that lets you drive a Rust system from a verified Quint spec — the spec acts as an oracle. You provide two adapters: (1) how to extract relevant state from your impl, (2) how to translate spec actions into impl operations. The simulator generates random traces; for each trace, both sides execute and state is compared. The Quint FAQ cites the **AWS DynamoDB Oct-2025 incident** as the canonical example — original design was correct, impl drifted, race emerged in production. Connect catches that drift in CI.

**Why borrow:** Boruna's `trace2tests` (already shipped) generates regression tests from execution traces and minimizes failing ones via delta debugging. The mental model is *similar* — replay a trace, assert outputs match — but it's directed at Boruna's *own* VM, not at an external production service. Extending the pattern to "the spec is the workflow, the SUT is the customer's downstream microservice" is a natural product extension and aligns with Boruna's compliance angle: prove the customer's actual production system still matches the audited workflow.

**Why low-medium and not high:** This is a *product feature*, not just tooling. It needs an adapter library, customer integration, and the marketing positioning (which the Boruna positioning docs would need to absorb). It's also adjacent to FleetQ integration territory (per the `fleetq-integration` memory). Probably not a v1.5.0 item; closer to a 2.x play.

**Cite:** [Quint Connect](https://github.com/informalsystems/quint-connect), [Quint FAQ on MBT](https://quint.sh/faq).

### 8. `q::debug` builtin (TRIVIAL)

**What Quint does:** `q::debug(msg, value)` prints `msg value` to stderr and returns `value` unchanged. Works inline inside any expression: `(x' = q::debug("new x:", x + 1))`.

**Why borrow:** Boruna's `.ax` has no equivalent. Today debugging means adding a `let _ = ` binding and a trace event. `dbg!` macro-equivalent inline ergonomics are valuable for `.ax` developer experience.

**Effort:** trivial (1 builtin function + 1 VM intrinsic).

**Cite:** [Quint builtin → q::debug](https://quint.sh/docs/builtin).

---

## What NOT to borrow

### Apalache-style bounded symbolic model checking

Quint's flagship verification command (`quint verify`) is implemented by translating the spec to SMT constraints and discharging them via Z3, integrated through Apalache. This is a multi-engineer-year integration. The Quint team works on this *because their customers (Cosmos chains, etc.) prove protocol correctness*. Boruna's customers buy *evidence that this specific run conformed to policy* — concrete trace, hash chained, replayable. Adding bounded model checking would be a massive architectural shift (drag in Java, ship a Z3 distribution, build an IR translator) for a benefit that doesn't appear on any of the open roadmap items (mTLS hardening, spec freeze, DX). Strong recommendation: **don't**.

### Choreo (paved-path library for distributed protocols)

Quint Choreo provides pre-built abstractions for message-soup-based distributed protocol specs (Tendermint, two-phase commit). Boruna already has `examples/workflows/` and `examples/compliance/` playing a similar role. The Choreo idea is good but the audience is "people writing new consensus protocols," which is not Boruna's audience.

### Quint's compile-to-TLA+ target

The `compile --target=tlaplus` flag exists so Quint specs can flow into the TLA+ ecosystem (TLC, classic Apalache, the LearnTla.com curriculum). Boruna has no equivalent compile target wishlist and no audience for one.

### Wholesale syntax changes

Boruna's surface syntax already follows the same "TypeScript-inspired, ASCII-clean" design principles Quint advocates. There's nothing to copy at the lexer level — both projects independently arrived at the same conclusions about ergonomics.

---

## What Quint borrowed *from Boruna* (incidentally)

Worth noting for context, though not actionable: Quint's marketing post-Oct-2025 has shifted toward exactly the auditable-AI positioning Boruna has occupied since v1.0. The Quint FAQ now has a dedicated "Quint and AI/LLMs" section arguing the spec is the verification artifact AI cannot hallucinate around. The Quint LLM Kit even ships Claude Code agents that drive the toolchain. This is convergent evolution, not derivative — both projects independently realized the same market is forming. It does mean Boruna's competitive framing in any future investor or sales conversation will benefit from being explicit about *what auditability means at runtime* (Boruna) vs. *what it means at spec time* (Quint). They are complements, not substitutes.

---

## Recommended next steps (for human decision)

This is a research report. No commitments made. Possible paths if you want to proceed:

1. **Spike the REPL.** A 1–2-day spike on `boruna repl` to validate the VM-stateful-between-inputs approach is the lowest-risk way to test whether the borrow lands.
2. **Audit `boruna simulate` against the design philosophy.** Before scoping it, do a Think-phase pass to confirm random simulation belongs in the Boruna runtime story or whether it lives in a sibling tool. The non-determinism conflicts with §15 of `project-conventions-2026-04` ("replay-verified vs. operational state — annotated"); the simulator's traces would need to be marked operational-only.
3. **Make ITF the export format for trace2tests.** Smallest, safest borrow; ships in any patch release.
4. **Literate workflow specs as the v1.5.0 headline feature.** Aligns with the compliance customer base, ships on top of existing template machinery, and gives a concrete answer to "why Boruna over Quint" for the audit market (because it executes the policy, not just specifies it).

---

## Sources

- [Quint home](https://quint.sh/)
- [Quint Design Principles](https://quint.sh/docs/design-principles)
- [Quint Language Summary](https://quint.sh/docs/lang) — modes table, runs, syntax
- [Quint CLI Reference](https://quint.sh/docs/quint) — `repl`, `run`, `verify`, `compile`, `test`, `--out-itf`, `--witnesses`, `--seed`, `--n-traces`
- [Quint REPL Tutorial](https://quint.sh/docs/repl)
- [Quint Simulator Docs](https://quint.sh/docs/simulator)
- [Quint Model-Based Testing](https://quint.sh/docs/model-based-testing)
- [Quint Model Checkers](https://quint.sh/docs/model-checkers)
- [Quint Checking Properties](https://quint.sh/docs/checking-properties) — witnesses
- [Quint Literate Specifications](https://quint.sh/docs/literate)
- [Quint Choreo](https://quint.sh/docs/choreo) and [Choreo Tutorial: 2PC](https://quint.sh/docs/choreo/tutorial)
- [Quint Builtins → q::debug](https://quint.sh/docs/builtin)
- [Quint FAQ](https://quint.sh/faq) — TLA+ comparison, Apalache, Connect, AI/LLMs
- [ADR 001: Transpiler Architecture](https://quint.sh/docs/development-docs/architecture-decision-records/adr001-transpiler-architecture)
- [informalsystems/quint GitHub](https://github.com/informalsystems/quint)
- [informalsystems/quint-connect GitHub](https://github.com/informalsystems/quint-connect)
- [informalsystems/quint-llm-kit GitHub](https://github.com/informalsystems/quint-llm-kit)

**Internal cross-references:**
- `claudedocs/research_zerolang_vs_boruna_2026-05-17.md` (prior competitive review, motivated v1.4.0 agent-native CLI)
- Boruna memory: `architecture`, `project-conventions-2026-04`, `release-1.4.0-status`, `post-1.0-execution-status`

---

*Stop here. Per `/sc:research` boundary: this is a report. Use `/sc:design` if you want to architect a borrow, or `/sc:implement` to ship one.*
