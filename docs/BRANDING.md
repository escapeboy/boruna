# Boruna Branding Guide

## Project Name

**Boruna** — a deterministic, capability-safe programming language and framework for building LLM-native applications.

**Tagline**: Boruna — Deterministic Agent-Native Execution Platform

## CLI Commands

| Command | Purpose |
|---------|---------|
| `boruna` | Primary CLI (compile, run, trace, replay, inspect, framework, lang, template) |
| `boruna-pkg` | Package manager (init, add, resolve, install, verify, publish, list) |
| `boruna-orch` | Multi-agent orchestration (run, status, resolve, adapters) |

## Component Names

| Component | Full Name | Crate Name |
|-----------|-----------|------------|
| Bytecode | Boruna Bytecode | `boruna-bytecode` |
| Compiler | Boruna Compiler | `boruna-compiler` |
| VM | Boruna VM | `boruna-vm` |
| Framework | Boruna Framework | `boruna-framework` |
| Effect System | Boruna Effect | `boruna-effect` |
| CLI | Boruna CLI | `boruna-cli` |
| Tooling | Boruna Tooling | `boruna-tooling` |
| Package Manager | Boruna Packages | `boruna-pkg` |
| Orchestrator | Boruna Orchestrator | `boruna-orchestrator` |

## File Extensions

| Extension | Purpose |
|-----------|---------|
| `.ax` | Boruna source files |
| `.ax.template` | Boruna template files |
| `package.ax.json` | Package manifest |
| `policy.ax.json` | Policy manifest |

## Style Rules

- **Capitalization**: "Boruna" is always capitalized when referring to the platform/project. Crate names and CLI commands are lowercase.
- **In prose**: "Boruna" (not "the Boruna language" or "Boruna Lang"). E.g., "Boruna enforces capability safety."
- **In code**: Rust crates use `boruna_*` (underscored) for module paths. E.g., `use boruna_vm::Vm;`
- **File naming**: Source files use `.ax` extension. Package manifests use `package.ax.json`.
- **CLI references**: Use `boruna` in documentation. E.g., `boruna run app.ax`, `boruna-pkg install`.
