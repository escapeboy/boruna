# Rename Audit: LLM-Lang → Axiom

## Current Names in Use

| Old Name | Context | Files Affected |
|----------|---------|----------------|
| **LLM-Lang** | Project/platform name | docs/*.md, CLAUDE.md, CLI about strings, host/web/* |
| **llmvm** | VM crate name + CLI binary | crates/llmvm/Cargo.toml, crates/llmvm-cli/Cargo.toml, all crates that depend on it |
| **llmvm-cli** | CLI crate name | crates/llmvm-cli/Cargo.toml |
| **llmc** | Compiler crate name | crates/llmc/Cargo.toml, all crates that depend on it |
| **llmbc** | Bytecode crate name | crates/llmbc/Cargo.toml, all crates that depend on it |
| **llmfw** | Framework crate name | crates/llmfw/Cargo.toml, all crates that depend on it |
| **llm-effect** | LLM integration crate name | crates/llm-effect/Cargo.toml |
| **llmpkg** | Package manager crate + binary | packages/Cargo.toml |
| **orch** | Orchestrator binary name | orchestrator/Cargo.toml |
| **.llm** | Source file extension | 28 .llm files in examples/ and libs/ |
| **.llm.template** | Template file extension | 5 files in templates/ |
| **package.llm.json** | Package manifest filename | 11 files in libs/, referenced in packages/src/*.rs |
| **policy.llm.json** | Policy manifest filename | referenced in packages/src/cli/mod.rs |
| **llm-lang-web-host** | npm package name | host/web/package.json |

## Detailed File Inventory

### Rust Crate Cargo.toml Files (name + description + binary names)
- `Cargo.toml` (workspace root) — members list references all crate paths
- `crates/llmbc/Cargo.toml` — name="llmbc", description="LLM-Lang bytecode..."
- `crates/llmc/Cargo.toml` — name="llmc", description="LLM-Lang compiler..."
- `crates/llmvm/Cargo.toml` — name="llmvm", description="LLM-Lang virtual machine..."
- `crates/llmvm-cli/Cargo.toml` — name="llmvm-cli", binary name="llmvm", description="LLM-Lang CLI tool"
- `crates/llmfw/Cargo.toml` — name="llmfw", description="LLM-Lang Application Framework..."
- `crates/llm-effect/Cargo.toml` — name="llm-effect"
- `orchestrator/Cargo.toml` — name="orchestrator", binary name="orch"
- `packages/Cargo.toml` — name="llmpkg", binary name="llmpkg"
- `tooling/Cargo.toml` — name="tooling"

### Rust Source Files (39 files with crate name references)
All `use llmbc::`, `use llmc::`, `use llmvm::`, `use llmfw::`, `use llm_effect::` imports across:
- tooling/src/*.rs (stdlib, templates, diagnostics, repair, trace2tests, tests)
- crates/llmvm-cli/src/main.rs (CLI entry point)
- crates/llmfw/src/*.rs (runtime, testing, policy, effect, validate, etc.)
- crates/llm-effect/src/*.rs (gateway, normalize, cache, tests)
- orchestrator/src/adapters/mod.rs (hardcoded `cargo run -p llmpkg`)
- packages/src/*.rs (references to package.llm.json, .llm files)

### Documentation Files (20 docs)
All docs/*.md files reference LLM-Lang, llmvm, or crate names:
- `docs/language-guide.md` — "LLM-Lang Language Guide"
- `docs/FRAMEWORK_SPEC.md` — "LLM-Lang Application Framework Specification"
- `docs/PACKAGE_SPEC.md` — llmpkg references throughout
- `docs/ORCHESTRATOR_SPEC.md` — LLM-Lang, llmpkg, llmvm references
- `docs/bytecode-spec.md` — "LLM-Lang Bytecode Specification"
- `docs/STD_LIBRARIES_SPEC.md` — LLM-Lang VM references
- `docs/DIAGNOSTICS_AND_REPAIR.md` — llmvm CLI commands
- `docs/TRACE_TO_TESTS.md` — llmvm CLI commands
- `docs/APP_TEMPLATE.md` — llmvm CLI commands, LLM-Lang
- `docs/FRAMEWORK_API.md` — llmvm references
- `docs/FRAMEWORK_STATUS.md` — llmvm references
- `docs/TESTING_GUIDE.md` — llmvm references
- `docs/CHANGELOG_DOGFIX.md` — llmvm references
- `docs/MILESTONE_DOGFIX_PLAN.md` — llmvm references
- `docs/LLM_EFFECT_SPEC.md` — llm-effect, LLM-Lang references
- `docs/LLM_EFFICIENCY_ROADMAP.md` — LLM references

### CLAUDE.md
- Multiple references to LLM-Lang, llmvm, llmc, llmbc, llmfw, llm-effect, llmpkg

### Examples (3 READMEs + 28 .llm source files)
- `examples/llm_patch_demo/README.md`
- `examples/repair_demo/README.md`
- `examples/trace_demo/README.md`
- `examples/todo.llm` — contains "Learn LLM-Lang" in string literal

### Host/Web
- `host/web/package.json` — name="llm-lang-web-host"
- `host/web/index.html` — title="LLM-Lang Web Host"
- `host/web/src/App.jsx` — "LLM-Lang Web Host Application", h1 text
- `host/web/src/renderer.jsx` — "LLM-Lang runtime" comment

### Templates
- 5 `app.llm.template` files
- 5 `template.json` manifests

### Standard Libraries
- 11 `package.llm.json` manifests
- 11 `src/core.llm` source files

## Naming Map (Old → New)

| Old | New | Notes |
|-----|-----|-------|
| LLM-Lang | Axiom | Project/platform name |
| llmvm (binary) | axiom | Primary CLI |
| llmvm (crate) | axiom-vm | VM library crate |
| llmvm-cli (crate) | axiom-cli | CLI crate |
| llmc (crate) | axiom-compiler | Compiler crate |
| llmbc (crate) | axiom-bytecode | Bytecode crate |
| llmfw (crate) | axiom-framework | Framework crate |
| llm-effect (crate) | axiom-effect | LLM integration crate |
| llmpkg (crate+binary) | axiom-pkg | Package manager |
| orch (binary) | axiom-orch | Orchestrator binary |
| tooling (crate) | axiom-tooling | Tooling crate |
| orchestrator (crate) | axiom-orchestrator | Orchestrator crate |
| .llm | .ax | Source file extension |
| .llm.template | .ax.template | Template file extension |
| package.llm.json | package.ax.json | Package manifest |
| policy.llm.json | policy.ax.json | Policy manifest |
