# LLM-Efficiency Roadmap

Implementation plan for safe, deterministic LLM-efficiency improvements.

## Core Invariants

- Deterministic execution (at least in debug/test mode)
- Replay compatibility (record/replay reproduces identical outcomes)
- Capability gating (no side effects outside Effects; update/view remain pure)
- Runtime does not depend on framework
- Orchestrator remains a separate layer

---

## Implementation Order

### A) Structured Diagnostics + Suggested Patches (HIGH ROI)

**Status:** DONE
**Crate:** `tooling`

Machine-readable diagnostics with auto-fix suggestions.

- Stable JSON output (`diagnostics.json`)
- `suggested_patches` per diagnostic (patchbundle-compatible hunks)
- Repair tool: apply best suggestion, re-verify
- CLI: `lang check --json`, `lang repair --from <file> --apply <best|id>`
- Error codes: E001-E009 (lexer, parse, undefined, non-exhaustive match, wrong field, capability violation, codegen)
- Analyzers: match exhaustiveness, record field validation, capability purity, name resolution with suggestions

### B) Intent Blocks (Metadata)

**Status:** Planned
**Dir:** `tooling/intent/`

Sidecar `*.intent.json` files bound to module/type/function IDs.
Schema: goal, non_goals, invariants, examples, perf_budget, security_notes.
CLI: `lang intent validate`, `lang intent scaffold-tests`.

### C) State Machines / Protocol Types

**Status:** Planned
**Dir:** `tooling/statemachine/`

StateMachine definition format (states, events, transitions, guards, pure actions).
Validator: unreachable states, missing transitions, illegal transitions.
CLI: `framework sm validate`, `framework sm generate-update`.

### D) Trace -> Regression Tests

**Status:** DONE
**Dir:** `tooling/src/trace2tests/`

Turn any runtime trace into a deterministic regression test.
Stable trace schema (version 1, SHA-256 hashed). Test spec generation.
Delta debugging minimizer (1-minimal). Built-in predicates (panic, state mismatch).
CLI: `trace2tests record`, `trace2tests generate`, `trace2tests run`, `trace2tests minimize`.
Example: `examples/trace_demo/`.

### E) Effect Schemas + Typed Adapters

**Status:** Planned
**Dir:** `tooling/effectschemas/`

Per-effect input/output schemas (JSON schema IDs).
Framework auto-generates callback message types.
Runtime validates payload at boundary.
CLI: `framework effects schema validate`, `framework effects schema dump`.

### F) Built-in Refactor Operations

**Status:** Planned
**Dir:** `tooling/refactor/`

AST-level deterministic refactoring: rename symbol, add missing match case, extract function.
Operates on canonical AST, produces patchbundle, validates via check gates.
CLI: `refactor rename`, `refactor extract-fn`, `refactor add-match-case`.

### G) Capability Minimization Pass

**Status:** Planned
**Dir:** `tooling/capmin/`

Static analysis to compute minimal required capabilities.
Suggests tighter policy. Integrates with package metadata.
CLI: `capmin analyze`, `capmin suggest-policy`.

### H) Context Lens API

**Status:** Planned
**Dir:** `tooling/contextlens/`

Deterministic, minimal context slices for LLM consumption.
Content-addressed references + stable ordering.
CLI: `contextlens for-diagnostic`, `contextlens for-trace`, `contextlens for-symbol`.

### I) Deterministic Fuzzing

**Status:** Planned
**Dir:** `tooling/fuzz/`

Seedable PRNG message-sequence generator.
Invariant checking (no panic, policy respected, state invariants).
Shrinker produces minimal failing trace. Exports via trace2tests.
CLI: `fuzz run --seed N --steps N`, `fuzz shrink --trace <file>`.

### J) Verified Templates Library

**Status:** Planned
**Dir:** `stdlib/templates/`

CRUD view-model, authz policy, retry/backoff, offline sync, pagination templates.
Versioned artifacts. `template apply` produces patchbundle and runs checks.
CLI: `template list`, `template apply <name> --args ...`.

---

## Quality Bar

- Every tool output: stable, machine-readable JSON
- Every transformation: outputs patchbundles
- All changes preserve determinism and replay
- Integration test loop: diagnostic -> contextlens -> suggested patch -> apply -> tests -> trace2tests
