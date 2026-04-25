# Project Index: Boruna (ai-lang)

Generated: 2026-04-25

Deterministic, policy-gated workflow execution platform for auditable AI systems. Rust workspace, 9 crates, 4 binaries, 557+ tests.

## Project Structure

```
ai-lang/
├── crates/                    # 7 core Rust crates (workspace members)
│   ├── llmbc/                 → boruna-bytecode    (6 .rs)  bytecode + Value types
│   ├── llmc/                  → boruna-compiler    (8 .rs)  lexer→parser→typeck→codegen
│   ├── llmvm/                 → boruna-vm          (8 .rs)  VM + capability gateway
│   ├── llmvm-cli/             → boruna-cli         (2 .rs)  `boruna` binary
│   ├── llmfw/                 → boruna-framework  (11 .rs)  Elm-style App protocol
│   ├── llm-effect/            → boruna-effect      (8 .rs)  LLM caching/context
│   └── boruna-mcp/            → boruna-mcp         (9 .rs)  MCP server for AI agents
├── orchestrator/              → boruna-orchestrator (19 .rs) workflow + audit + agents
├── packages/                  → boruna-pkg          (6 .rs) registry, resolver, lockfiles
├── tooling/                   → boruna-tooling     (10 .rs) diagnostics, repair, stdlib runner
├── libs/                      # 11 deterministic .ax standard libraries
├── templates/                 # 5 app templates (.ax.template)
├── examples/                  # .ax example programs + 3 workflow examples
├── docs/                      # User-facing + archived internal docs
├── plans/                     # Implementation plans
├── scripts/                   # ci.sh
└── .github/workflows/         # ci.yml (test, clippy, fmt)
```

## Entry Points

| Binary | Crate | Path | Purpose |
|--------|-------|------|---------|
| `boruna` | boruna-cli | crates/llmvm-cli/src/main.rs | Main CLI: compile, run, framework, workflow, evidence, template, lang |
| `boruna-mcp` | boruna-mcp | crates/boruna-mcp/src/main.rs | MCP JSON-RPC server (10 tools for AI agents) |
| `boruna-orch` | boruna-orchestrator | orchestrator/src/main.rs | Standalone orchestrator runner |
| `boruna-pkg` | boruna-pkg | packages/src/main.rs | Package management CLI |

## Core Modules (Crate Dependency Graph)

```
boruna-bytecode  ←  boruna-compiler  ←  boruna-framework  ←  boruna-mcp
       ↑                    ↑                    ↑                  ↑
       boruna-vm  ──────────┘                    │                  │
       ↑                                         │                  │
       boruna-effect (LLM)                       │                  │
                                                                    │
       boruna-tooling, boruna-orchestrator ──────────────────────────┘
```

| Module | Purpose | Key Exports |
|--------|---------|-------------|
| boruna-bytecode | Bytecode IR + Value enum | `Op`, `Module`, `Function`, `Value`, `Capability` |
| boruna-compiler | Compilation pipeline | `compile(name, source) -> Module` (lexer, parser, typeck, codegen) |
| boruna-vm | VM + capability enforcement | `Vm::new`, `Vm::run`, `CapabilityGateway`, `Policy`, `ActorSystem`, `ReplayEngine`, `EventLog` |
| boruna-vm (feature `http`) | Real HTTP capability handler | SSRF-safe `ureq` handler, `NetPolicy` |
| boruna-framework | Elm App protocol | `AppValidator`, `AppRuntime`, `TestHarness`, `PolicySet` |
| boruna-effect | LLM integration | prompt builder, context store, hex-validated cache |
| boruna-orchestrator | Workflow + audit + multi-agent | `WorkflowDef`, `Validator`, `Runner`, `AuditLog`, `EvidenceBundle`, agent engine |
| boruna-tooling | Dev tools | `diagnostics`, `repair`, `trace2tests`, `stdlib`, `templates` |
| boruna-pkg | Package system | `PackageManifest`, dependency resolver, SHA-256 hashing, lockfiles |
| boruna-mcp | MCP server | 10 JSON-RPC tools: compile, ast, run, check, repair, validate_app, framework_test, workflow_validate, template_list/apply |

## Configuration

| File | Purpose |
|------|---------|
| `Cargo.toml` (root) | Workspace: 10 members, edition 2021, version 0.1.0 |
| `.github/workflows/ci.yml` | CI: test, clippy `-D warnings`, fmt --check |
| `scripts/ci.sh` | Local CI mirror (+ workflow validation, evidence verify) |
| `.mcp.json` (user) | Wires `boruna-mcp` into Claude Code / Cursor |
| Cargo features | `boruna-vm/http` enables real HTTP (ureq + url, opt-in via `--live`) |

## Documentation

**Trust files (root)**: README.md, AGENTS.md, CHANGELOG.md, CONTRIBUTING.md, CODE_OF_CONDUCT.md, SECURITY.md, LICENSE (MIT), CLAUDE.md

**Docs index**: `docs/README.md`

| Path | Topic |
|------|-------|
| docs/QUICKSTART.md | 10-minute onboarding (ends with `evidence verify`) |
| docs/concepts/{determinism,capabilities,evidence-bundles}.md | Core concepts |
| docs/guides/first-workflow.md | Build a workflow from scratch |
| docs/reference/{cli,ax-language}.md | CLI flags + `.ax` syntax reference |
| docs/{stability,roadmap,faq,limitations}.md | Project status |
| docs/{FRAMEWORK_SPEC,LLM_EFFECT_SPEC,ORCHESTRATOR_SPEC,PACKAGE_SPEC,STD_LIBRARIES_SPEC}.md | Subsystem specs |
| docs/{ACTORS_GUIDE,EFFECTS_GUIDE,DIAGNOSTICS_AND_REPAIR,TRACE_TO_TESTS,TESTING_GUIDE,OPERATIONS,INTEGRATION_GUIDE}.md | Operational guides |
| docs/{SECURITY_MODEL,DETERMINISM_CONTRACT,COMPLIANCE_EVIDENCE,PLATFORM_GOVERNANCE,ENTERPRISE_PLATFORM_OVERVIEW}.md | Compliance & governance |
| docs/{language-guide,bytecode-spec,FRAMEWORK_API,APP_TEMPLATE,BRANDING}.md | Reference |
| docs/archive/ | Internal/historical docs (not for external readers) |

## Test Coverage

- **Total**: 557+ tests across 9 crates (per memory; verify with `cargo test --workspace`)
- **Integration tests dir**: `tests/` (4 files), plus per-crate `tests/` folders (e.g., `crates/llmvm/tests/http_integration.rs`)
- **Run all**: `cargo test --workspace`
- **Per crate**: `cargo test -p boruna-{compiler,vm,framework,effect,pkg,orchestrator,tooling}`
- **HTTP feature tests**: `cargo test -p boruna-vm --features http`

## Examples & Libraries

**Example .ax programs** (examples/): hello, fibonacci, counter, todo, pattern_matching, while_loop, capabilities + framework subdirs (admin_crud, offline_sync_todo, realtime_notifications, repair_demo, stdlib_demo, trace_demo, llm_patch_demo).

**Workflow examples** (examples/workflows/): `llm_code_review` (linear 3-step), `document_processing` (fan-out 5-step), `customer_support_triage` (approval gate). Each has README + `.ax` step files.

**Standard libraries** (libs/, 11 total): std-ui, std-forms, std-authz, std-http, std-db, std-sync, std-validation, std-routing, std-storage, std-notifications, std-testing. Each ships `package.ax.json` + `src/core.ax`. Side-effecting libs declare capabilities (e.g., std-http needs `net.fetch`).

**App templates** (templates/, 5 total): crud-admin, form-basic, auth-app, realtime-feed, offline-sync. Each has `template.json` + `app.ax.template` with `{{variable}}` placeholders.

## Key Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| serde / serde_json | 1.x | Serialization (workspace) |
| thiserror | 2.x | Error derivation (workspace) |
| clap | 4.x | CLI argument parsing |
| logos | 0.14 | Lexer for compiler |
| tokio | 1.x | Async runtime (full features) |
| axum | 0.8 | HTTP server (CLI `serve` feature) |
| sha2 | 0.10 | Content/audit hashing |
| chrono | 0.4 | Timestamps in CLI/orchestrator |
| ureq | 2.x (optional) | Real HTTP handler (`http` feature) |
| url | 2.x (optional) | URL parsing for SSRF guard |
| rmcp | 0.16 | MCP server protocol (boruna-mcp) |
| schemars | 1.0 | JSON schema for MCP tools |
| tempfile | 3.x | Safe temp files (CLI + tests) |

## Quick Start

```bash
# 1. Build workspace
cargo build --workspace

# 2. Run all tests
cargo test --workspace

# 3. Run an example
cargo run --bin boruna -- run examples/hello.ax

# 4. Validate + run a workflow
cargo run --bin boruna -- workflow validate examples/workflows/llm_code_review
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review --policy allow-all --record

# 5. Verify the evidence bundle
cargo run --bin boruna -- evidence verify <bundle-dir>

# 6. Start MCP server for AI agents
cargo run --bin boruna-mcp
```

## Critical Invariants

1. **Determinism**: Same input → same output. Use `BTreeMap`, never `HashMap` where order matters.
2. **Capability gating**: Side effects (net, db, fs) declared on `.ax` functions; enforced by `CapabilityGateway` at runtime.
3. **Replay compatibility**: `EventLog` + `ReplayEngine` verify reruns produce identical event streams.
4. **Path traversal defense**: `PatchBundle` rejects `..`/absolute, then `canonicalize()`s. Cache/context-store keys must be hex-only.
5. **CI gate**: `cargo clippy -- -D warnings` and `cargo fmt --check` block merges.
