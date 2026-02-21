# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

Boruna: a deterministic execution platform for enterprise AI workflows. Everything is written in Rust. Workflows are DAG-based, policy-gated, and auditable. Each step compiles to bytecode and runs on a custom VM with capability enforcement. Every run can produce a hash-chained evidence bundle for compliance.

## Build & Test Commands

```bash
# Build everything
cargo build --workspace

# Run all tests (557+ tests across 9 crates)
cargo test --workspace

# Run tests for a single crate
cargo test -p boruna-compiler      # compiler tests
cargo test -p boruna-vm            # VM tests
cargo test -p boruna-framework     # framework tests
cargo test -p boruna-effect        # LLM integration tests
cargo test -p boruna-pkg           # package system tests
cargo test -p boruna-orchestrator  # multi-agent orchestration tests
cargo test -p boruna-tooling       # diagnostics, repair, stdlib, templates tests

# Run a single test by name
cargo test -p boruna-compiler test_record_spread
cargo test -p boruna-tooling test_std_ui_runs

# Run a .ax source file
cargo run --bin boruna -- run examples/hello.ax

# Run with capability policy
cargo run --bin boruna -- run app.ax --policy allow-all

# Framework commands
cargo run --bin boruna -- framework validate examples/framework/counter_app.ax
cargo run --bin boruna -- framework test examples/framework/counter_app.ax -m "increment:1,increment:1,reset:0"

# Diagnostics & repair
cargo run --bin boruna -- lang check file.ax --json
cargo run --bin boruna -- lang repair file.ax

# Templates
cargo run --bin boruna -- template list
cargo run --bin boruna -- template apply crud-admin --args "entity_name=products,fields=name|price" --validate

# Workflow commands
cargo run --bin boruna -- workflow validate examples/workflows/llm_code_review
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review --policy allow-all --record

# Evidence commands
cargo run --bin boruna -- evidence verify <bundle-dir>
cargo run --bin boruna -- evidence inspect <bundle-dir> --json

# CI/CD — runs on every push/PR (GitHub Actions)
# Clippy (zero warnings policy)
cargo clippy --workspace -- -D warnings

# Format check
cargo fmt --all -- --check
```

## Repository

GitHub: https://github.com/escapeboy/boruna

CI/CD runs three jobs on every push: `cargo test --workspace`, `cargo clippy -- -D warnings`, `cargo fmt --check`.

## Architecture

### Crate Dependency Graph

```
boruna-bytecode  ←  boruna-compiler  ←  boruna-framework
       ↑                    ↑                    ↑
       boruna-vm ───────────┘                    │
       ↑                                         │
       boruna-effect (LLM integration)           │
                                                 │
       boruna-tooling (diagnostics, repair, stdlib, templates)
       boruna-pkg (package registry, resolver, lockfiles)
       boruna-orchestrator (multi-agent coordination)
```

Note: directory paths still use original names (crates/llmbc, crates/llmc, etc.). See docs/RENAMING.md for the full mapping.

### Core Crates (crates/)

- **boruna-bytecode** (dir: crates/llmbc) — Bytecode format. Defines `Op`, `Module`, `Function`, `Value`, `Capability`. The `Value` enum covers: `Int`, `Float`, `String`, `Bool`, `Unit`, `None`, `Some`, `Ok`, `Err`, `Record`, `Enum`, `List`, `Map`, `ActorId`, `FnRef`.
- **boruna-compiler** (dir: crates/llmc) — Compiler pipeline: `lexer::lex()` → `parser::parse()` → `typeck::check()` → `codegen::emit()`. Entry point: `boruna_compiler::compile(name, source) -> Result<Module, CompileError>`.
- **boruna-vm** (dir: crates/llmvm) — Virtual machine. `Vm::new(module, gateway)`, `vm.run() -> Result<Value, VmError>`. Includes `CapabilityGateway` (with `Policy::allow_all()`, `Policy::deny_all()`, `Policy::default()`), `ActorSystem`, `ReplayEngine`, `EventLog`.
- **boruna-framework** (dir: crates/llmfw) — Framework layer enforcing the App protocol (Elm architecture: init/update/view). `AppValidator`, `AppRuntime`, `TestHarness`, `PolicySet`, state machine diffing.
- **boruna-effect** (dir: crates/llm-effect) — Token-optimized LLM integration: prompt building, context management, caching, normalization, capability gating for LLM calls.
- **boruna-cli** (dir: crates/llmvm-cli) — CLI binary (`boruna`). Subcommands: compile, run, trace, replay, inspect, ast, framework, lang, trace2tests, template, workflow, evidence.

### Supporting Crates

- **boruna-pkg** (dir: packages/, binary: `boruna-pkg`) — Deterministic package ecosystem: `PackageManifest` (package.ax.json), dependency resolution with topological sort, SHA-256 content hashing, lockfile generation, local registry.
- **boruna-orchestrator** (dir: orchestrator/, binary: `boruna-orch`) — Enterprise workflow execution: `workflow/` (WorkflowDef, Validator, Runner, DataStore), `audit/` (hash-chained AuditLog, EvidenceBundle, verify), plus multi-agent orchestration (engine, patch management, conflict resolution, storage, adapters).
- **boruna-tooling** (dir: tooling/) — Developer tooling library:
  - `diagnostics/` — Structured diagnostics with source spans, severity levels, suggested patches
  - `repair/` — Auto-repair tool applying diagnostic suggestions (strategies: best, all, by-id)
  - `trace2tests/` — Record execution traces → generate regression tests → minimize failing traces (delta debugging)
  - `stdlib/` — Standard library test runner (`run_library`, `verify_compiles`, `verify_determinism`)
  - `templates/` — Template engine with `{{variable}}` substitution, manifest validation

### Standard Libraries (libs/)

11 deterministic libraries, each with `package.ax.json` and `src/core.ax`:
std-ui, std-forms, std-authz, std-http, std-db, std-sync, std-validation, std-routing, std-storage, std-notifications, std-testing.

All are pure-functional (no hidden side effects). Libraries needing capabilities declare them in their manifest (e.g., std-http requires `net.fetch`, std-db requires `db.query`).

### Templates (templates/)

5 app templates (crud-admin, form-basic, auth-app, realtime-feed, offline-sync). Each has `template.json` manifest and `app.ax.template` with `{{variable}}` placeholders.

## Language Key Facts

- Statically typed. Types: `Int`, `Float`, `String`, `Bool`, `Unit`, `Option<T>`, `Result<T,E>`, `List<T>`, `Map<K,V>`, records, enums.
- Record spread: `State { ..old_state, field: new_value }`.
- Pattern matching: `match expr { "a" => ..., _ => ... }`.
- Capability annotations on functions: `fn fetch(url: String) -> String !{net.fetch}`.
- Framework apps must define: `init() -> State`, `update(State, Msg) -> UpdateResult`, `view(State) -> UINode`. Types: `State`, `Msg`, `Effect`, `UpdateResult`, `UINode`, `PolicySet`.
- Every `.ax` file needs a `fn main() -> Int` for standalone execution.

## Critical Invariants

- **Determinism**: All execution must be deterministic. Same input → same output, always. No randomness, no time-dependent behavior in pure code. Use `BTreeMap` (not `HashMap`) for ordered iteration.
- **Capability gating**: Side effects (network, db, fs) are declared and enforced. The VM's `CapabilityGateway` checks every capability call against the active `Policy`.
- **Replay compatibility**: Execution can be recorded and replayed. `EventLog` captures capability results; `ReplayEngine` verifies determinism.
- **Package content hashing**: Packages use SHA-256 content hashes for integrity verification.
- **Path traversal prevention**: PatchBundle validates against `..` and absolute paths, with `canonicalize()` defense-in-depth. LLM cache and context store validate hex-only keys/hashes.
