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

# Build/test with real HTTP handler (optional feature)
cargo build --workspace --features boruna-vm/http
cargo test -p boruna-vm --features http
cargo clippy --workspace --features boruna-vm/http -- -D warnings

# Run a .ax source file
cargo run --bin boruna -- run examples/hello.ax

# Run with capability policy
cargo run --bin boruna -- run app.ax --policy allow-all

# Run with real HTTP (requires http feature)
cargo run --features boruna-cli/http --bin boruna -- run app.ax --policy allow-all --live

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

# MCP Server (AI agent integration)
cargo run --bin boruna-mcp
cargo run --bin boruna-mcp -- --templates-dir templates --libs-dir libs

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
       boruna-mcp (MCP server for AI coding agents)
```

Note: directory paths still use original names (crates/llmbc, crates/llmc, etc.). The full mapping is in the memory file and in the Architecture section below.

### Core Crates (crates/)

- **boruna-bytecode** (dir: crates/llmbc) — Bytecode format. Defines `Op`, `Module`, `Function`, `Value`, `Capability`. The `Value` enum covers: `Int`, `Float`, `String`, `Bool`, `Unit`, `None`, `Some`, `Ok`, `Err`, `Record`, `Enum`, `List`, `Map`, `ActorId`, `FnRef`.
- **boruna-compiler** (dir: crates/llmc) — Compiler pipeline: `lexer::lex()` → `parser::parse()` → `typeck::check()` → `codegen::emit()`. Entry point: `boruna_compiler::compile(name, source) -> Result<Module, CompileError>`.
- **boruna-vm** (dir: crates/llmvm) — Virtual machine. `Vm::new(module, gateway)`, `vm.run() -> Result<Value, VmError>`. Includes `CapabilityGateway` (with `Policy::allow_all()`, `Policy::deny_all()`, `Policy::default()`), `ActorSystem`, `ReplayEngine`, `EventLog`.
- **boruna-framework** (dir: crates/llmfw) — Framework layer enforcing the App protocol (Elm architecture: init/update/view). `AppValidator`, `AppRuntime`, `TestHarness`, `PolicySet`, state machine diffing.
- **boruna-effect** (dir: crates/llm-effect) — Token-optimized LLM integration: prompt building, context management, caching, normalization, capability gating for LLM calls.
- **boruna-cli** (dir: crates/llmvm-cli) — CLI binary (`boruna`). Subcommands: compile, run, trace, replay, inspect, ast, framework, lang, trace2tests, template, workflow, evidence.
- **boruna-mcp** (dir: crates/boruna-mcp) — MCP server binary (`boruna-mcp`). Exposes 10 tools over JSON-RPC stdio for AI coding agents. Built on rmcp v0.16.

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

## MCP Server (boruna-mcp)

MCP (Model Context Protocol) server that exposes Boruna's toolchain to AI coding agents (Claude Code, Cursor, Codex, etc.) over JSON-RPC stdio transport. Binary: `boruna-mcp`. Crate: `crates/boruna-mcp/`.

### Available Tools

| Tool | Description |
|------|-------------|
| `boruna_compile` | Compile `.ax` source → module info or structured errors |
| `boruna_ast` | Parse `.ax` source → AST JSON (truncated at 100KB) |
| `boruna_run` | Compile + execute `.ax` source with policy/step-limit/trace |
| `boruna_check` | Run diagnostics → severity, spans, suggested patches |
| `boruna_repair` | Auto-repair `.ax` source using diagnostic suggestions |
| `boruna_validate_app` | Validate App protocol conformance (init/update/view) |
| `boruna_framework_test` | Run a framework app through a message sequence |
| `boruna_workflow_validate` | Validate workflow DAG structure + topological order |
| `boruna_template_list` | List available app templates |
| `boruna_template_apply` | Apply a template with variable substitution |

### IDE Configuration

Add to `.mcp.json` (Claude Code) or equivalent:

```json
{
  "mcpServers": {
    "boruna": {
      "command": "cargo",
      "args": ["run", "--bin", "boruna-mcp", "--manifest-path", "/path/to/ai-lang/Cargo.toml"],
      "env": {}
    }
  }
}
```

### Design Conventions

- All tools return structured JSON with `"success": true|false`.
- Domain errors (compile failures, runtime errors) are returned as successful tool responses with `success: false`, not as MCP errors.
- Source code is passed as strings, not file paths (1MB limit enforced).
- Synchronous Boruna APIs run inside `tokio::task::spawn_blocking`.

## Language Key Facts

- Statically typed. Types: `Int`, `Float`, `String`, `Bool`, `Unit`, `Option<T>`, `Result<T,E>`, `List<T>`, `Map<K,V>`, records, enums.
- Record spread: `State { ..old_state, field: new_value }`.
- Pattern matching: `match expr { "a" => ..., _ => ... }`.
- Capability annotations on functions: `fn fetch(url: String) -> String !{net.fetch}`.
- Framework apps must define: `init() -> State`, `update(State, Msg) -> UpdateResult`, `view(State) -> UINode`. Types: `State`, `Msg`, `Effect`, `UpdateResult`, `UINode`, `PolicySet`.
- Every `.ax` file needs a `fn main() -> Int` for standalone execution.

## Repository Layout (key files)

```
README.md                     Portal README — start here
AGENTS.md                     AI coding agent integration guide (MCP server, rules)
CHANGELOG.md                  User-facing release history (keep-a-changelog format)
CONTRIBUTING.md               Contribution guide
SECURITY.md                   Vulnerability reporting policy
LICENSE                       MIT

docs/
  README.md                   Documentation index
  QUICKSTART.md               10-minute onboarding (ends with evidence verify)
  concepts/
    determinism.md            Why and how determinism is enforced
    capabilities.md           Side effect declaration and policy gating
    evidence-bundles.md       Hash-chained audit logs and replay
  guides/
    first-workflow.md         Build a workflow from scratch
  reference/
    cli.md                    All boruna CLI commands and flags
    ax-language.md            .ax syntax, types, capabilities reference
  stability.md                Stability tiers (stable/experimental/alpha/planned)
  roadmap.md                  0.2.0 through 1.0.0
  faq.md                      Common questions
  limitations.md              Real constraints, stated honestly
  archive/                    Internal development docs (not for external readers)

examples/workflows/
  llm_code_review/            Linear 3-step LLM workflow + README
  document_processing/        Fan-out 5-step workflow + README
  customer_support_triage/    Approval-gate workflow + README
```

## Critical Invariants

- **Determinism**: All execution must be deterministic. Same input → same output, always. No randomness, no time-dependent behavior in pure code. Use `BTreeMap` (not `HashMap`) for ordered iteration.
- **Capability gating**: Side effects (network, db, fs) are declared and enforced. The VM's `CapabilityGateway` checks every capability call against the active `Policy`.
- **Replay compatibility**: Execution can be recorded and replayed. `EventLog` captures capability results; `ReplayEngine` verifies determinism.
- **Package content hashing**: Packages use SHA-256 content hashes for integrity verification.
- **Path traversal prevention**: PatchBundle validates against `..` and absolute paths, with `canonicalize()` defense-in-depth. LLM cache and context store validate hex-only keys/hashes.
