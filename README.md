# Boruna

[![CI](https://github.com/escapeboy/boruna/actions/workflows/ci.yml/badge.svg)](https://github.com/escapeboy/boruna/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-1.0.0--rc1-orange.svg)](CHANGELOG.md)
[![Status: Release Candidate](https://img.shields.io/badge/status-1.0--rc-yellow.svg)](docs/stability.md)

> **LTS:** 1.x will be the long-term-support line. See [`docs/lts.md`](./docs/lts.md) for the support-window, deprecation, and security-backport policy that takes effect at 1.0 GA.

**Deterministic, policy-gated workflow execution for AI systems that must be auditable.**

---

## The problem

Most AI orchestration tools run workflows and return outputs. When something goes wrong — or when a regulator asks — there is no reliable way to answer: *What exactly ran? What did the model see? What did it return? Can you prove it?*

Boruna answers those questions by design.

Every Boruna workflow run produces a **tamper-evident evidence bundle**: a hash-chained audit log of every step executed, every capability invoked, every model response received. That bundle can be inspected, verified, and replayed — without network access, without a central server, without trusting anyone's word.

This makes Boruna suited for teams building AI workflows that touch regulated data, make consequential decisions, or need a defensible audit trail.

## What Boruna provides

- **DAG workflow execution** — steps are `.ax` source files; the workflow is a `workflow.json` DAG definition (`schema_version: 1` frozen at 1.0)
- **Capability enforcement** — every side effect (LLM calls, HTTP, database, filesystem) is declared and policy-gated at the VM level
- **Evidence bundles** — hash-chained tamper-evident logs, written automatically with `--record`. Optional **AES-256-GCM envelope encryption** for compliance-sensitive deployments
- **Deterministic replay** — re-execute any recorded workflow with identical outputs, verified by the VM
- **Distributed execution** — coord+workers HTTP cluster with active-active **HA**, worker URL failover, capability-tagged placement, and optional **mTLS** with per-worker client certs
- **Approval gates** — pause workflow execution for human review or external triggers before continuing
- **Diagnostics, auto-repair, and migration** — `boruna lang check`, `boruna lang repair`, `boruna migrate` for `.ax` files and bundle/workflow upgrades
- **`boruna new`** — interactive scaffold for new workflows from templates
- **Three formal versioned specifications** — `.ax` language 1.0, evidence bundle format 1.0, workflow DAG schema 1.0 (all under [`docs/spec/`](./docs/spec/))
- **MCP server** — exposes 10 tools for AI coding agent integration (Claude Code, Cursor, Codex)

## What Boruna is not

- Not a general-purpose language or runtime (use Rust, Python, Go for that)
- Not an LLM framework (use LangChain, LCEL, etc. for prompt engineering)
- Not a cloud service (Boruna runs wherever you deploy it)
- Not a key-management system (operators wire HSM / KMS integration themselves; bundle-encryption KEK lifecycle is operator-owned)

## Install

Pre-built static binaries are published on every tagged release:

```bash
# Linux x86_64 (musl — works on Alpine, Ubuntu, Debian, ...)
curl -fsSL https://github.com/escapeboy/boruna/releases/latest/download/SHA256SUMS -o SHA256SUMS
TARGET=x86_64-unknown-linux-musl
TAR=$(grep "$TARGET" SHA256SUMS | awk '{print $2}')
curl -fsSLO "https://github.com/escapeboy/boruna/releases/latest/download/$TAR"
grep "$TAR" SHA256SUMS | sha256sum -c -
tar -xzf "$TAR"
./boruna-*-${TARGET}/boruna --version
```

Other targets: `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`. See [`docs/releasing.md`](docs/releasing.md) for details.

Or build from source:

```bash
git clone https://github.com/escapeboy/boruna
cd boruna
cargo build --workspace --release
```

## Quickstart

```bash
# Run a workflow
boruna workflow run examples/workflows/llm_code_review --policy allow-all --record

# Verify the evidence bundle
boruna evidence verify .boruna/runs/<run-id>/
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

1175+ tests across 11 workspace members. `cargo test --workspace --features boruna-cli/serve` — all pass.

## Documentation

| | |
|---|---|
| [Quickstart](docs/QUICKSTART.md) | Build, run a workflow, inspect evidence |
| [Concepts: Determinism](docs/concepts/determinism.md) | Why and how determinism is enforced |
| [Concepts: Capabilities](docs/concepts/capabilities.md) | Side effect declaration and policy gating |
| [Concepts: Evidence Bundles](docs/concepts/evidence-bundles.md) | Hash-chained audit logs and replay |
| [Guide: First Workflow](docs/guides/first-workflow.md) | Build a workflow from scratch |
| [Guide: Coord HA](docs/guides/coord-ha.md) | Multi-coord deployment topologies |
| [Guide: Coord mTLS](docs/guides/coord-mtls.md) | X.509 client certs + cert generation |
| [Guide: Worker Capability Tagging](docs/guides/worker-capability-tagging.md) | Heterogeneous fleet placement |
| [Guide: Migration](docs/guides/migration.md) | Upgrade legacy bundles and workflow files |
| [Spec: `.ax` Language 1.0](docs/spec/ax-language-1.0.md) | Formal language specification |
| [Spec: Workflow DAG 1.0](docs/spec/workflow-dag-1.0.md) | `workflow.json` schema |
| [Spec: Evidence Bundle 1.0](docs/spec/evidence-bundle-1.0.md) | Bundle format + encryption envelope |
| [Reference: CLI](docs/reference/cli.md) | All `boruna` commands |
| [LTS contract](docs/lts.md) | Support windows + deprecation policy for 1.x |
| [Performance](docs/PERFORMANCE.md) | Baseline numbers + 1.x performance budget |
| [Stability](docs/stability.md) | What is stable, experimental, and planned |
| [Roadmap](docs/roadmap.md) | 0.2.0 → 1.0.0 → 1.x |
| [Limitations](docs/limitations.md) | Real constraints, stated honestly |
| [FAQ](docs/faq.md) | Common questions |
| [All docs →](docs/README.md) | Full documentation index |

## Status

Boruna is at **v1.0.0-rc1**. The core execution engine, distributed-execution stack, evidence bundles, and three formal versioned specifications are feature-complete. The 1.x line is committed to long-term-support per [`docs/lts.md`](docs/lts.md), which takes effect at 1.0 GA.

The project is appropriate for evaluation, internal tooling, and audit-sensitive AI pipelines. **Operator action before production GA**: book external security audit (Q4 2026 commitment), validate the [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) budget against your workload, and review [`docs/limitations.md`](docs/limitations.md) for known constraints.

See [docs/stability.md](docs/stability.md) for the full maturity assessment.

## For coding agents

Boruna exposes an MCP server for AI coding agent integration. See [AGENTS.md](AGENTS.md) for integration instructions and the tool reference.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version: open an issue, implement with tests, run `cargo test --workspace` + `cargo clippy` + `cargo fmt`, add a CHANGELOG entry, open a PR.

## License

[MIT](LICENSE) — Copyright 2026 Boruna Contributors
