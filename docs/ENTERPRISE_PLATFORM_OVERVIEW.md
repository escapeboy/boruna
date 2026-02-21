# Boruna Enterprise Platform Overview

Boruna is a deterministic execution platform for enterprise AI workflows. It provides policy-gated, auditable workflow execution with built-in governance, replay, and compliance evidence generation.

## Core Capabilities

### Workflow Execution
- DAG-based workflow definitions with typed data flow between steps
- Topological execution ordering with dependency resolution
- Approval gates for human-in-the-loop review
- Retry policies for transient failures
- Budget enforcement per step and per workflow

### Determinism Guarantees
- Same inputs + same workflow + same policy = identical outputs
- BTreeMap-based ordering throughout (no HashMap non-determinism)
- Capability-gated side effects — all IO is declared and controlled
- Record/replay support via EventLog

### Policy Enforcement
- Capability allowlists: declare which side effects each step may use
- Budget limits: token and call count budgets per step
- Model allowlists: restrict which LLM models may be invoked
- Network allowlists: restrict outbound HTTP destinations

### Audit Trail
- Hash-chained audit log: tamper-evident, append-only record of all decisions
- Evidence bundles: self-contained compliance artifacts per workflow run
- Bundle verification: cryptographic integrity checking of all artifacts

## Architecture

```
workflow.json → Validator → Runner → Evidence Bundle
                  ↓           ↓
              Topological   Per-step:
              Sort          Compile .ax → VM → Output
                            Policy check
                            Audit log entry
```

### Crate Map
- `boruna-orchestrator` — Workflow engine, audit system, evidence bundles
- `boruna-compiler` — Compiles .ax source to bytecode
- `boruna-vm` — Executes bytecode with capability gating
- `boruna-bytecode` — Bytecode format and Value types
- `boruna-effect` — LLM integration with budget tracking
- `boruna-framework` — App protocol (Elm architecture)
- `boruna-tooling` — Diagnostics, repair, trace-to-tests, templates
- `boruna-pkg` — Package system with integrity verification

## Workflow Lifecycle

1. **Define** — Write `workflow.json` with steps, edges, policies
2. **Validate** — `boruna workflow validate <dir>` checks DAG structure
3. **Run** — `boruna workflow run <dir> --policy <policy>` executes steps
4. **Record** — `--record` flag generates evidence bundle
5. **Verify** — `boruna evidence verify <dir>` checks bundle integrity
6. **Replay** — Re-execute from recorded event log for determinism verification

## Schema Versioning

All serializable formats include `schema_version` for forward compatibility:
- Workflow definition: v1
- Policy: v1
- Audit log: v1
- Evidence bundle manifest: v1
