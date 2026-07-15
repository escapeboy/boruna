# Design — LLM effect propagates up the call graph (Theme C-lite, Sprint 3)

Branch: `feat/llm-typed-effect` off master (v1.7.0). Borrowed from **Vera** (LLM inference as a
tracked typed effect). User chose the lite core — **no conformal prediction / uncertainty
quantification** (research-grade statistics; a poor fit like Z3 was for Theme A). Deferred as a
documented follow-up.

## Idea
Boruna gates the `llm.call` capability per function at the VM. But "does this workflow step
*transitively* invoke a model?" was not surfaced — a step whose `main` calls a helper that calls a
model is invisible at the step level. This is exactly what a regulator/auditor wants to know
("which steps touched an LLM"). We make the `llm` effect propagate up the call graph and record it
in the evidence bundle.

## Design
- **`Module::transitively_invokes(func_idx, cap) -> bool`** (bytecode): a function reaches `cap`
  if it declares it directly OR any function it `Call`s / `SpawnActor`s (transitively) does.
  Cycle-safe via a visited set; result is order-independent → deterministic. General over any
  `Capability`, used here for `Capability::LlmCall`.
- **Evidence capture** (orchestrator): the bundle builder already recompiles each source-kind
  step (for Sprint 1's intent capture). In that same loop, compute
  `module.transitively_invokes(module.entry, LlmCall)` and collect the sorted list of
  model-invoking step ids into `model_invoking_steps.json` — a new bundle component, checksummed
  and hash-covered like `intents.json` (so `evidence verify` fails on tamper). No-op when empty.

## Why no Function-metadata field
Storing the flag on bytecode `Function` would work but forces updating ~35 struct literals every
sprint (known churn debt). Computing it as a call-graph analysis where it's consumed (evidence
build) is smaller, avoids churn, and keeps the fact derivable/auditable from the module.

## Determinism
The analysis is a pure function of the module. `step_sources` is a BTreeMap → the collected list is
sorted → deterministic bytes. Replay-verified (feeds the evidence bundle).

## Scope
In: transitive `llm.call` reachability per step → `model_invoking_steps.json`.
Out (deferred): conformal-prediction confidence sets; numeric grant caps (Lumen); a first-class
`Function.transitive_capabilities` field (overlaps Theme D capability-row inference).

## Tests
- bytecode: transitive reach through a call; cycle-safety; a pure function is false.
- evidence: `model_invoking_steps.json` written + checksummed; empty → no file.
