# Architecture — `boruna simulate`

Companion to `docs/design-boruna-simulate.md`. Implementation deferred past this sprint.

## Component map

| Component | Location | Role |
|---|---|---|
| `Simulator` | `orchestrator/src/simulate/mod.rs` (new) | Multi-trace driver, seeded RNG |
| `TraceRunner` | `orchestrator/src/simulate/runner.rs` (new) | One-trace executor on top of `WorkflowRunner` |
| `InputFuzzer` | `orchestrator/src/simulate/fuzz.rs` (new) | JSON Schema → random `Value` |
| `InvariantEvaluator` | `orchestrator/src/simulate/invariant.rs` (new) | Evaluates `.ax` expr against data store |
| `SimulationReport` | same | Aggregate stats + per-violation evidence bundle paths |
| CLI handler | `crates/llmvm-cli/src/simulate.rs` (new) | clap subcommand, flag parsing, report rendering |

## Data flow

```
boruna simulate <dir> --invariant=x --max-samples=N --seed=S
  ↓
Simulator::new(workflow_dir, seed=S, samples=N, max_steps=K)
  ↓
parallel via rayon::par_iter over (0..N):
  for each sample i:
    ChaCha20Rng::seed_from_u64(seed ⊕ i)
    InputFuzzer::generate_inputs(schema, &mut rng)
    TraceRunner::run_one(workflow, inputs, &mut rng):
      sandboxed Store, fresh data_dir under /tmp
      step loop:
        WorkflowRunner::advance_one_tick()
        InvariantEvaluator::check(invariant_expr, store)
        if violated:
          finalize evidence bundle to <out-dir>/violation_<i>/
          return Violation { seed, step_no, last_state }
        if completed: return Ok(steps_taken)
  ↓
SimulationReport.aggregate()
  ↓
human report (or --json) to stdout
```

## File map (new files)

- `orchestrator/src/simulate/mod.rs`
- `orchestrator/src/simulate/runner.rs`
- `orchestrator/src/simulate/fuzz.rs`
- `orchestrator/src/simulate/invariant.rs`
- `crates/llmvm-cli/src/simulate.rs`
- `orchestrator/Cargo.toml` — add `rand = "0.8"`, `rand_chacha = "0.3"`, `rayon = "1"`

## CLI surface

```
boruna simulate <DIR>
  Run guided random simulation of a workflow.

Options:
  --invariant <EXPR>       .ax boolean expression to check on every state
  --witnesses <NAME=EXPR,...>  Witness predicates (see witnesses doc)
  --max-samples <N>        Number of traces to run [default: 10000]
  --max-steps <K>          Step ceiling per trace [default: 20]
  --n-traces <M>           Buffer M longest OK traces or shortest violations [default: 1]
  --seed <HEX>             Deterministic RNG seed (else random + printed)
  --out-dir <PATH>         Where violation bundles go [default: ./simulate-out/]
  --parallel <T>           Worker threads [default: num_cpus]
  --json                   Emit JSON report to stdout
```

## Determinism contract

Per `project-conventions-2026-04` §15:

- Simulator evidence bundles tagged `simulator: true` in manifest.
- Simulator traces NEVER live in `data-dir/evidence/` of the production install. CLI ensures
  `--out-dir` is outside the standard data dir; rejected with `error_kind: "invalid_out_dir"`
  if it isn't.
- Drift test: `simulate_bundle_does_not_pollute_production_data_dir`.

## Per-violation evidence

Each violation produces a real evidence bundle in `<out-dir>/violation_<seed>/`. It's a real
bundle (passes `boruna evidence verify`) but the manifest's `kind` field is `"simulator"`
not `"production"`. Audit consumers filter on `kind`.

## Witnesses integration

When `--witnesses=name=expr,...` is provided, `InvariantEvaluator` evaluates each witness
predicate on every state in addition to the invariant. Per-witness counters accumulate;
`SimulationReport` includes a `witnesses: [{name, trace_count, total, frequency}]` array.

## Open architecture decisions

- **Invariant grammar:** standalone `--invariant=<expr>` flag accepts an inline `.ax` boolean
  expression. The expression references workflow step outputs via the existing
  `step_output_value(<step>, <field>)` builtin.
- **Fault injection:** post-MVP. v0 simulator only randomizes inputs. Fault injection (worker
  failure, approval-gate decision randomization) deferred to a follow-up architecture iteration.
