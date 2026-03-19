# Boruna

[![CI](https://github.com/escapeboy/boruna/actions/workflows/ci.yml/badge.svg)](https://github.com/escapeboy/boruna/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.1.0-orange.svg)](CHANGELOG.md)
[![Status: Pre-production](https://img.shields.io/badge/status-pre--production-yellow.svg)](docs/stability.md)

**Deterministic, policy-gated workflow execution for AI systems that must be auditable.**

---

## The problem

Most AI orchestration tools run workflows and return outputs. When something goes wrong — or when a regulator asks — there is no reliable way to answer: *What exactly ran? What did the model see? What did it return? Can you prove it?*

Boruna answers those questions by design.

Every Boruna workflow run produces a **tamper-evident evidence bundle**: a hash-chained audit log of every step executed, every capability invoked, every model response received. That bundle can be inspected, verified, and replayed — without network access, without a central server, without trusting anyone's word.

This makes Boruna suited for teams building AI workflows that touch regulated data, make consequential decisions, or need a defensible audit trail.

## What Boruna provides

- **DAG workflow execution** — steps are `.ax` source files; the workflow is a `workflow.json` DAG definition
- **Capability enforcement** — every side effect (LLM calls, HTTP, database, filesystem) is declared and policy-gated at the VM level
- **Evidence bundles** — hash-chained tamper-evident logs, written automatically with `--record`
- **Deterministic replay** — re-execute any recorded workflow with identical outputs, verified by the VM
- **Approval gates** — pause workflow execution for human review before continuing
- **Diagnostics and auto-repair** — `boruna lang check` and `boruna lang repair` for `.ax` files
- **MCP server** — exposes 10 tools for AI coding agent integration (Claude Code, Cursor, Codex)

## What Boruna is not

- Not a general-purpose language or runtime (use Rust, Python, Go for that)
- Not an LLM framework (use LangChain, LCEL, etc. for prompt engineering)
- Not a cloud service (Boruna runs wherever you deploy it)
- Not yet production-ready (v0.1.0 — core is complete and tested, API evolving)

## Quickstart

```bash
git clone https://github.com/escapeboy/boruna
cd boruna
cargo build --workspace

# Run a workflow
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review \
  --policy allow-all --record

# Verify the evidence bundle
cargo run --bin boruna -- evidence verify .boruna/runs/<run-id>/
```

**→ [Full Quickstart](docs/QUICKSTART.md)** — 10 minutes, ends with a verified evidence bundle.

## Example workflows

| Workflow | Pattern | What it shows |
|----------|---------|---------------|
| [LLM Code Review](examples/workflows/llm_code_review/) | Linear, 3 steps | LLM capability, data flow, evidence recording |
| [Document Processing](examples/workflows/document_processing/) | Fan-out, 5 steps | Parallel steps, multi-input merge |
| [Customer Support Triage](examples/workflows/customer_support_triage/) | Approval gate | Human-in-the-loop, conditional pause, audit trail |

Each example runs in demo mode (no external services) and produces a verifiable evidence bundle.

## How the evidence guarantee works

```
workflow.json  →  DAG Validator  →  Step Runner
                                        ↓
                                   .ax source
                                        ↓
                                   Compiler → Bytecode
                                        ↓
                                   VM (capability gateway)
                                        ↓
                                   EventLog entry (CapCall + CapResult)
                                        ↓
                              Hash-chained audit log

boruna evidence verify <bundle>
  → Chain integrity: VALID
  → All step hashes: MATCH
  → Verification: PASSED
```

Every `CapCall` (including LLM calls) is logged with its full response. The log is SHA-256 hash-chained from a genesis entry containing the workflow definition hash. Modification of any entry breaks the chain.

## Architecture

Boruna is a Rust workspace with 9 crates:

| Crate | Purpose |
|-------|---------|
| `boruna-orchestrator` | Workflow engine, DAG execution, evidence bundles |
| `boruna-vm` | Bytecode VM, capability gateway, actor system, replay |
| `boruna-compiler` | Lexer, parser, type checker, code generator |
| `boruna-bytecode` | Opcodes, Module, Value, Capability definitions |
| `boruna-framework` | Elm-architecture runtime, test harness |
| `boruna-effect` | LLM integration, prompt management, caching |
| `boruna-cli` | CLI binary (`boruna`) |
| `boruna-tooling` | Diagnostics, repair, trace-to-tests, templates |
| `boruna-pkg` | Package registry, resolver, lockfiles |

557+ tests. `cargo test --workspace` — all pass.

## Documentation

| | |
|---|---|
| [Quickstart](docs/QUICKSTART.md) | Build, run a workflow, inspect evidence |
| [Concepts: Determinism](docs/concepts/determinism.md) | Why and how determinism is enforced |
| [Concepts: Capabilities](docs/concepts/capabilities.md) | Side effect declaration and policy gating |
| [Concepts: Evidence Bundles](docs/concepts/evidence-bundles.md) | Hash-chained audit logs and replay |
| [Guide: First Workflow](docs/guides/first-workflow.md) | Build a workflow from scratch |
| [Reference: CLI](docs/reference/cli.md) | All `boruna` commands |
| [Reference: .ax Language](docs/reference/ax-language.md) | Syntax, types, capabilities |
| [Stability](docs/stability.md) | What is stable, experimental, and planned |
| [Roadmap](docs/roadmap.md) | 0.2.0 through 1.0.0 |
| [Limitations](docs/limitations.md) | Real constraints, stated honestly |
| [FAQ](docs/faq.md) | Common questions |
| [All docs →](docs/README.md) | Full documentation index |

## Status

Boruna is at **v0.1.0**. The core execution engine is complete and tested. The public API is not yet stable — breaking changes may occur before 1.0.

The project is appropriate for evaluation, proof-of-concept workflows, and teams who want to adopt the architecture early. It is not yet appropriate for production workloads requiring API stability guarantees.

See [docs/stability.md](docs/stability.md) for the full maturity assessment.

## For coding agents

Boruna exposes an MCP server for AI coding agent integration. See [AGENTS.md](AGENTS.md) for integration instructions and the tool reference.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version: open an issue, implement with tests, run `cargo test --workspace` + `cargo clippy` + `cargo fmt`, add a CHANGELOG entry, open a PR.

## License

[MIT](LICENSE) — Copyright 2026 Boruna Contributors
