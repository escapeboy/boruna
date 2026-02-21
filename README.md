# Boruna

[![CI](https://github.com/escapeboy/boruna/actions/workflows/ci.yml/badge.svg)](https://github.com/escapeboy/boruna/actions/workflows/ci.yml)

Boruna is a **deterministic execution platform for enterprise AI workflows**. It provides policy-gated, auditable workflow execution with built-in governance, replay, and compliance evidence generation. Written entirely in Rust.

## What Boruna Does

- **Executes DAG-based workflows** with typed data flow between steps, approval gates, and retry policies
- **Enforces policies** — capability allowlists, token budgets, model restrictions, and network controls
- **Produces audit trails** — hash-chained logs and self-contained evidence bundles for compliance
- **Guarantees determinism** — same inputs + same workflow + same policy = identical outputs, every time
- **Supports replay** — re-execute from recorded event logs to verify deterministic behavior
- **Compiles and runs .ax programs** on a custom bytecode VM with capability gating

## What Boruna Is Not

- Not a general-purpose programming language competing with Rust/Go/Python
- Not an IDE or editor
- Not a marketplace or plugin system
- Not production-ready yet (v0.1.0 — functional but evolving)

## Quick Start

```bash
# Build
cargo build --workspace

# Validate and run a workflow
cargo run --bin boruna -- workflow validate examples/workflows/llm_code_review
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review --policy allow-all --record

# Verify evidence bundle
cargo run --bin boruna -- evidence verify examples/workflows/llm_code_review/evidence/<run-id>

# Run all 557+ tests
cargo test --workspace
```

## Example: LLM Code Review Workflow

```json
{
  "name": "llm-code-review",
  "version": "1.0.0",
  "steps": {
    "fetch_diff": { "kind": "source", "source": "steps/fetch_diff.ax" },
    "analyze":    { "kind": "source", "source": "steps/analyze.ax",
                    "inputs": { "diff": "fetch_diff.result" } },
    "report":     { "kind": "source", "source": "steps/report.ax",
                    "inputs": { "analysis": "analyze.result" } }
  },
  "edges": [["fetch_diff", "analyze"], ["analyze", "report"]]
}
```

```bash
boruna workflow validate examples/workflows/llm_code_review
# workflow 'llm-code-review' v1.0.0 is valid
#   steps: 3
#   execution order: fetch_diff -> analyze -> report

boruna workflow run examples/workflows/llm_code_review --record
# workflow 'llm-code-review' run: Completed
#   evidence bundle: evidence/run-llm-code-review-...
```

## Example Workflows

| Workflow | Pattern | Demonstrates |
|----------|---------|-------------|
| [`llm_code_review`](examples/workflows/llm_code_review/) | Linear (3 steps) | LLM capability, data flow, evidence recording |
| [`document_processing`](examples/workflows/document_processing/) | Fan-out/merge (5 steps) | Parallel steps, multi-input merge, DAG scheduling |
| [`customer_support_triage`](examples/workflows/customer_support_triage/) | Approval gate (4 steps) | Human-in-the-loop, conditional pause, audit trail |

## Key Guarantees

### Determinism
Same bytecode + same inputs = identical outputs. `BTreeMap` ordering throughout, no `HashMap` non-determinism. Time and IO virtualized through capability gateway.

### Policy Enforcement
Every side effect is declared with `!{capability}` annotations and checked against the active policy. Budget limits, model allowlists, and network restrictions are enforced per step.

### Auditability
Every workflow run can produce an evidence bundle: hash-chained audit log, workflow/policy snapshots, per-step outputs, environment fingerprint. `boruna evidence verify` checks integrity.

### Replay
Execution can be recorded and replayed. The `EventLog` captures capability results; `ReplayEngine` verifies that replay produces identical outcomes.

## Architecture

```
workflow.json → Validator → Runner → Evidence Bundle
                              ↓
                          Per step:
                          .ax source → Compiler → VM → Output
                                       Policy check
                                       Audit log entry
```

### Crates

| Crate | Directory | Purpose |
|-------|-----------|---------|
| `boruna-orchestrator` | `orchestrator` | Workflow engine, audit system, evidence bundles |
| `boruna-compiler` | `crates/llmc` | Lexer, parser, type checker, code generator |
| `boruna-vm` | `crates/llmvm` | Virtual machine, capability gateway, actor system, replay |
| `boruna-bytecode` | `crates/llmbc` | Opcodes, Module, Value, Capability definitions |
| `boruna-effect` | `crates/llm-effect` | LLM integration, prompt management, caching |
| `boruna-framework` | `crates/llmfw` | Elm-architecture runtime, validation, test harness |
| `boruna-cli` | `crates/llmvm-cli` | CLI binary with all subcommands |
| `boruna-tooling` | `tooling` | Diagnostics, auto-repair, trace-to-tests, templates |
| `boruna-pkg` | `packages` | Package registry, resolver, lockfiles |

## CLI Reference

```bash
# Workflow commands
boruna workflow validate <dir>         # Validate workflow definition
boruna workflow run <dir> --policy <p> # Execute workflow
boruna workflow run <dir> --record     # Execute with evidence recording

# Evidence commands
boruna evidence verify <dir>           # Verify bundle integrity
boruna evidence inspect <dir>          # Show bundle manifest
boruna evidence inspect <dir> --json   # Machine-readable manifest

# Single-file execution
boruna compile app.ax                  # Compile to bytecode
boruna run app.ax --policy allow-all   # Run a program
boruna trace app.ax                    # Run with tracing
boruna replay app.axbc trace.json      # Replay from trace

# Framework commands
boruna framework validate app.ax       # Validate app structure
boruna framework test app.ax -m "msg:val"  # Test with messages

# Developer tools
boruna lang check app.ax --json        # Structured diagnostics
boruna lang repair app.ax              # Auto-repair
boruna template list                   # List app templates
boruna template apply <name> --args "k=v"  # Generate from template
```

## Enterprise Documentation

| Document | Contents |
|----------|----------|
| [`ENTERPRISE_PLATFORM_OVERVIEW.md`](docs/ENTERPRISE_PLATFORM_OVERVIEW.md) | Platform vision, architecture, workflow lifecycle |
| [`PLATFORM_GOVERNANCE.md`](docs/PLATFORM_GOVERNANCE.md) | Policies, RBAC, budgets, approval gates, audit log |
| [`OPERATIONS.md`](docs/OPERATIONS.md) | Deploy, run, observe, replay, CI integration |
| [`SECURITY_MODEL.md`](docs/SECURITY_MODEL.md) | Capabilities, isolation, secrets, threat model |
| [`COMPLIANCE_EVIDENCE.md`](docs/COMPLIANCE_EVIDENCE.md) | Evidence bundles, audit logs, verification |
| [`ENTERPRISE_GAPS.md`](docs/ENTERPRISE_GAPS.md) | Known gaps with priority and proposed direction |

## Language Features

Boruna includes a statically typed language for writing workflow steps:

| Feature | Example |
|---------|---------|
| Static types | `Int`, `Float`, `String`, `Bool`, `Unit` |
| Option / Result | `Option<T>`, `Result<T, E>` |
| Records | `User { name: "Ada", age: 30 }` |
| Record spread | `User { ..old_user, age: 31 }` |
| Enums | `enum Color { Red, Green, Custom(String) }` |
| Pattern matching | `match val { "a" => 1, _ => 0 }` |
| Capability annotations | `fn f() -> T !{net.fetch}` |
| Actors | `spawn`, `send`, `receive` |

---

## For LLMs and Coding Agents

Boruna is designed to be understood and operated by LLMs and autonomous coding agents.

### Entry Points

| Task | Where to Start |
|------|---------------|
| Understand the project | [`CLAUDE.md`](CLAUDE.md) — build commands, architecture, invariants |
| Run enterprise workflows | [`docs/OPERATIONS.md`](docs/OPERATIONS.md) — validate, run, verify |
| Understand governance | [`docs/PLATFORM_GOVERNANCE.md`](docs/PLATFORM_GOVERNANCE.md) — policies, budgets, audit |
| Learn the language | [`docs/language-guide.md`](docs/language-guide.md) — types, syntax, capabilities |
| Build framework apps | [`docs/FRAMEWORK_API.md`](docs/FRAMEWORK_API.md) — AppRuntime, TestHarness |
| Integrate into apps | [`docs/INTEGRATION_GUIDE.md`](docs/INTEGRATION_GUIDE.md) — embedding, plugins |
| Work with effects | [`docs/EFFECTS_GUIDE.md`](docs/EFFECTS_GUIDE.md) — effect lifecycle, capabilities |
| Use actors | [`docs/ACTORS_GUIDE.md`](docs/ACTORS_GUIDE.md) — spawn, send, supervision |
| Understand determinism | [`docs/DETERMINISM_CONTRACT.md`](docs/DETERMINISM_CONTRACT.md) — invariants |
| Manage packages | [`docs/PACKAGE_SPEC.md`](docs/PACKAGE_SPEC.md) — manifests, lockfiles |

### Critical Rules for Agents

1. **Never break determinism** — use `BTreeMap`, never `HashMap`. No randomness in pure code.
2. **Declare all capabilities** — functions with side effects need `!{capability}` annotations.
3. **Run `cargo test --workspace`** after every change — 541+ tests must pass.
4. **Run `cargo clippy --workspace -- -D warnings`** — zero warnings allowed.

### Capability List

| Capability | ID | Gate |
|------------|-----|------|
| `net.fetch` | 0 | HTTP requests |
| `db.query` | 1 | Database queries |
| `fs.read` | 2 | File reads |
| `fs.write` | 3 | File writes |
| `time.now` | 4 | Current time |
| `random` | 5 | Random values |
| `ui.render` | 6 | UI emission |
| `llm.call` | 7 | LLM API calls |
| `actor.spawn` | 8 | Spawn child actors |
| `actor.send` | 9 | Send actor messages |

## License

MIT
