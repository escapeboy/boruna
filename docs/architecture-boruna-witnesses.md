# Architecture — `boruna simulate --witnesses`

Companion to `docs/design-boruna-witnesses.md`. Depends on simulate (see
`docs/architecture-boruna-simulate.md`). Implementation deferred past this sprint.

## Component additions to simulate

| Component | Location | Role |
|---|---|---|
| `WitnessSpec` | `orchestrator/src/simulate/witness.rs` (new) | `{ name, expr }` parsed from CLI |
| `WitnessTracker` | same | Per-trace `HashMap<witness_name, bool>` and aggregate counts |
| Witness aggregation in `SimulationReport` | `orchestrator/src/simulate/mod.rs` | Add `witnesses` field |

## Data flow extension (only the additions)

```
Simulator init:
  parse --witnesses=N1=E1,N2=E2 into Vec<WitnessSpec>
  compile each expr like invariant
  ↓
Per-trace runner:
  WitnessTracker::new()
  on each state:
    for each witness in witnesses:
      if InvariantEvaluator::run(witness.expr) returns true:
        tracker.mark_hit(witness.name)
  on trace end:
    for each hit name: aggregate.trace_count[name] += 1
    aggregate.total += 1
```

## CLI surface additions

```
--witnesses <NAME=EXPR,...>            Witness predicates (any-state checks)
--require-witness <NAME=THRESHOLD,...> Fail simulator if a witness is below threshold
```

## Output format additions
Human-readable mirror of Quint:
```
Witnesses:
  ack_path was witnessed in 7094 trace(s) out of 10000 explored (70.94%)
  reject_path was witnessed in 312 trace(s) out of 10000 explored (3.12%)
```

JSON (`--json`):
```json
{
  "witnesses": [
    { "name": "ack_path", "trace_count": 7094, "total": 10000, "frequency": 0.7094 },
    { "name": "reject_path", "trace_count": 312, "total": 10000, "frequency": 0.0312 }
  ]
}
```

## Exit code semantics

- Default: witnesses never fail the simulator. Exit code unchanged.
- `--require-witness=NAME=T`: if `frequency < T`, exit 2 with stderr listing the under-threshold
  witnesses. Reuses the existing exit-2 convention but with `error_kind: "witness_threshold"`.

## File map (new files)

- `orchestrator/src/simulate/witness.rs` (~100 lines)
- Extension to `crates/llmvm-cli/src/simulate.rs` (~40 lines)
- Tests in `orchestrator/tests/witness_*.rs` (~150 lines)

Total marginal cost over simulate: ~290 lines.
