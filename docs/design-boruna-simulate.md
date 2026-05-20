# Design — `boruna simulate` (random property-based workflow simulation)

## Status

Planned. Borrowed from Quint (`quint run`). Source: research_quint_borrowable_ideas_2026-05-20.md, rec #2.

## Context

Boruna's workflow story today is **"run with the customer's inputs once, archive the evidence."**
Correct for *deterministic compliance*, but it says nothing about *what could happen* if a
different message arrives, a worker fails, or an approval gate stays open longer than budget.

Quint's `quint run spec.qnt --invariant=safety --max-samples=10000 --max-steps=20 --seed=0xabc` is
a guided random walk through the state machine that checks invariants on every state and emits
reproducible failures with a printable seed. The pattern is *exactly* what compliance customers
need to ask before signing off: "in 10,000 simulated runs of this approval pipeline, does the
invariant 'no payment without approval' hold?"

## Why

Boruna already has:
- The execution engine (`orchestrator::WorkflowRunner`)
- A capability gateway with policy enforcement (so simulated runs can scope/inject faults)
- Hash-chained evidence bundles (which any one of 10K simulated runs can produce)
- Determinism contract (so a `--seed` reproduces exactly)

Random simulation closes a real gap: it lets us claim that *under all explored runs* an invariant
holds, not just *under this single specific input*.

## Goals

1. `boruna simulate <workflow-dir> --invariant=<expr> --max-samples=N --max-steps=K --seed=H` runs
   guided random simulation, reports invariant violations with seed for reproduction.
2. Each violation produces a full evidence bundle (so the existing audit/replay tooling works
   on simulator failures).
3. Random sources: input values from `inputs.schema.json`, message-injection order, worker-failure
   injection (per a fault-injection policy), approval-decision direction.
4. The simulator never modifies the customer's production data store; runs always go into a
   sandboxed temp directory (CLI `--data-dir`).
5. Performance target: 1000 traces × 10 steps in ≤ 30 seconds on a developer laptop.

## Non-goals

- Symbolic / SMT-backed verification. (Rec #5 in research report — explicitly NOT recommended.)
- Distributed simulation across coord+workers. The simulator runs the workflow runner directly
  in-process for speed.
- Replacing `boruna workflow run`. They coexist.
- Liveness properties. Invariants only. (Quint's `--witnesses` covers the "good things happen"
  half; that's a separate design — see design-boruna-witnesses.md.)

## Forcing questions

**Who needs this? What are they doing today?**
A compliance engineer at Boruna's hypothetical insurance customer who wants to certify that the
"refund_approval" workflow can never disburse funds without two approvals — across all possible
message orderings and worker-failure patterns. Today: they run the workflow with handcrafted
inputs (maybe 5-10 paths), file the evidence, and *hope* coverage is enough. With `simulate`:
they get a number ("0 violations in 10,000 runs") and a paper trail.

**What's the narrowest MVP someone would pay for?**
`simulate` that randomizes only inputs (per `inputs.schema.json`) and checks one invariant. Worker
failure / message reordering can be added later.

**What would make someone say "whoa"?**
A failing seed that, when fed back to `boruna workflow run --seed=...`, reproduces the failure
exactly. Per Quint's UX: "Use --seed=0x12954db3ea62c1 to reproduce."

**How does this compound over time?**
Pairs naturally with witnesses (see design-boruna-witnesses.md) and feeds the same trace format
that `trace2tests` consumes today. Eventually composes into a CI gate: "every PR must pass
1000-run simulate on workflows X/Y/Z."

## Scope

| In | Out |
|---|---|
| `boruna simulate <dir>` CLI command | Distributed coord+worker simulation |
| Input randomization driven by JSON Schema bounds | Live HTTP fault injection (use `--policy` net rules instead) |
| Invariant evaluation as a `.ax` boolean expression over the workflow data store | Temporal/liveness properties |
| Seeded RNG with reproducibility | Persistent simulator state across runs |
| `--n-traces` longest-runs-or-shortest-violations buffer (mirrors Quint) | Multi-machine swarm runs |
| Per-violation evidence bundle in `--out-dir` | Custom RNG injection sites in user `.ax` code |

## Decisions

1. **RNG:** `rand_chacha::ChaCha20Rng` seeded with the user-supplied or generated `u64` seed.
   Deterministic across platforms (matches Quint's reproducibility contract).
2. **Schema-driven input fuzzing:** JSON Schema's `minimum`/`maximum`/`enum`/`pattern` drives the
   randomizer. For unconstrained fields, fall back to bounded-random per type (1-byte string, etc.).
3. **Invariant evaluation:** invariant expression is compiled the same way as a workflow step
   (`.ax` → bytecode → VM eval) but with a special "read-only data store view" capability that
   forbids writes. Failure = VM returns `false` or panics.
4. **Operational vs. replay-verified state (§15):** simulator traces are operational-only. They
   never feed into a production evidence bundle's hash chain. Per-violation bundles are real
   evidence bundles, BUT marked with `simulator: true` in their manifest so audit consumers
   can filter them out.
5. **Output directory:** `--out-dir traces/` mirrors `quint run --out-itf=traces/`. Each violation
   produces `violation_<seed>_<n>/` with full bundle. OK-paths get summary stats only.

## Risks

- **Determinism contract conflict.** Random simulation, by definition, explores non-deterministic
  paths. Must be *very* careful that the simulator's evidence-bundle writes do NOT corrupt the
  hash-chain semantics of production bundles. Mitigation: `simulator: true` manifest flag,
  separate output dir convention, drift test that asserts simulator bundles never live in the
  production evidence path.
- **Performance.** 10K runs × 20 steps = 200K step evaluations. Today's runner is ~5ms/step
  (rough estimate). 200K × 5ms = 1000s = 17 minutes. Way over budget. Mitigation: parallel-runs
  via `rayon::par_iter` over independent seeds (each run is purely sequential within itself, but
  inter-run is embarrassingly parallel).
- **Invariant expression scope.** `.ax` expressions over the data store — what's the API? Decision:
  the existing `step_input` builtin can be reused, with a new `simulate_invariant_value(key)`
  variant that reads from any prior step output. Architecture doc nails this.

## Open questions for the architecture phase

- Concrete grammar for `--invariant=<expr>` — file path? Inline expression? Module reference?
- Per-step fault injection policy: where declared (workflow.json? policy.json? new file?).
- Storage layout for 10K trace summaries — SQLite? Append-only NDJSON?
