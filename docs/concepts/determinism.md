# Determinism in Boruna

Determinism is Boruna's foundational guarantee: given the same workflow definition, the same inputs, and the same capability responses, execution produces identical outputs every time, on every machine.

This is not a convention or a best-effort property. It is structurally enforced by the runtime.

## Why determinism matters

AI systems are inherently probabilistic. LLMs return different outputs on repeated calls. Network responses vary. Clocks differ between machines. In ad hoc orchestration this is accepted as normal. In regulated environments, it creates a compliance problem: you cannot audit what you cannot reproduce.

Boruna's answer is to treat non-determinism as a capability, not a default. Every source of external state must be explicitly declared, gated by policy, and logged in the evidence bundle. The pure `.ax` computation in between is deterministic by construction.

This makes it possible to:
- Reproduce any workflow execution exactly from its evidence bundle
- Prove that a recorded run matches a given workflow definition
- Detect if model behavior has drifted between runs
- Satisfy audit requirements without manual log correlation

## The determinism boundary

Boruna draws a hard boundary between pure computation and effects.

**Pure (deterministic):**
- All `.ax` expressions and functions
- Record and enum operations
- Pattern matching, conditionals, loops
- Function calls (including recursive)
- Actor message passing (scheduling order is recorded)

**Effects (non-deterministic, capability-gated):**
- Network requests (`net.fetch`)
- LLM calls (`llm.call`)
- Time reads (`time.now`)
- File system access (`fs.read`, `fs.write`)
- Database queries (`db.query`)
- Random number generation (`rand.next`)

Effects are declared on functions using capability annotations:

```ax
fn fetch_data(url: String) -> String !{net.fetch} {
    // implementation (live mode)
}
```

Without the `!{net.fetch}` annotation, a function cannot perform network I/O — the VM enforces this at the bytecode level.

## How the EventLog works

Every capability call is intercepted by the `CapabilityGateway` and written to the `EventLog` as a `CapCall`/`CapResult` pair:

```
CapCall  { capability: "net.fetch", args: ["https://api.example.com/data"] }
CapResult{ capability: "net.fetch", value: "{\"status\": 200, ...}" }
```

The EventLog also captures actor lifecycle events (`ActorSpawn`, `MessageSend`, `MessageReceive`, `SchedulerTick`) so multi-actor scheduling is fully reproducible.

When `--record` is passed to `workflow run`, the EventLog is written into the evidence bundle.

## Replay verification

Replay works by substituting recorded `CapResult` values instead of making real calls:

1. **Record**: Run the workflow with live capabilities. Save the EventLog.
2. **Replay**: Run the same bytecode. When a `CapCall` is encountered, return the recorded result instead of executing the effect.
3. **Verify**: The `CapCall` sequence must match exactly — same capability, same arguments, same order.

If verification fails, either the workflow is non-deterministic (a bug) or an external value leaked into the pure core.

```bash
# Run and record
boruna workflow run examples/workflows/llm_code_review --policy allow-all --record

# Verify the evidence bundle
boruna evidence verify .boruna/runs/<run-id>/
```

## BTreeMap, not HashMap

One concrete implication of the determinism guarantee: all ordered iteration in Boruna's Rust implementation uses `BTreeMap` (sorted by key) rather than `HashMap` (random iteration order). This ensures that serialized outputs and logged values are identical across runs and platforms.

When writing `.ax` code, `Map` literals also use deterministic ordering.

## Determinism boundaries and guarantees

| Guarantee | Scope |
|-----------|-------|
| Same bytecode + same EventLog → identical output | VM, framework |
| Actor scheduling order is reproducible | VM actor system |
| Package resolution is locked (SHA-256 content hashes) | boruna-pkg |
| Workflow step execution order follows DAG topology | boruna-orchestrator |
| Evidence bundle integrity (hash-chained log) | audit module |

**Not guaranteed:**
- LLM output reproducibility (LLMs are probabilistic; Boruna records responses but cannot force models to repeat them)
- Wall-clock timing of individual steps
- External service behavior between record and replay

See also: [Limitations](../limitations.md), [Evidence Bundles](../COMPLIANCE_EVIDENCE.md)
