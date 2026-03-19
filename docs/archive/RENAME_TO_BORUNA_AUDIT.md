# Rename Audit: Axiom → Boruna

**Date**: 2026-02-21

## Summary

Renaming the entire platform from "Axiom" to "Boruna". This is the second rename:
- Phase 1 (2026-02-20): LLM-Lang → Axiom (completed)
- Phase 2 (2026-02-21): Axiom → Boruna (this rename)

Directory paths remain unchanged (they still use the original `llm*` names from Phase 1).

## Naming Map

| Old Name | New Name | Context |
|----------|----------|---------|
| Axiom | Boruna | Project/platform name |
| axiom (binary) | boruna | Primary CLI |
| axiom-cli (crate) | boruna-cli | CLI crate |
| axiom-bytecode (crate) | boruna-bytecode | Bytecode crate |
| axiom-compiler (crate) | boruna-compiler | Compiler crate |
| axiom-vm (crate) | boruna-vm | VM crate |
| axiom-framework (crate) | boruna-framework | Framework crate |
| axiom-effect (crate) | boruna-effect | Effect/LLM integration crate |
| axiom-tooling (crate) | boruna-tooling | Tooling crate |
| axiom-pkg (crate+binary) | boruna-pkg | Package manager |
| axiom-orchestrator (crate) | boruna-orchestrator | Orchestrator crate |
| axiom-orch (binary) | boruna-orch | Orchestrator binary |
| axiom-web-host (npm) | boruna-web-host | Web host package |

## Files Requiring Changes

### Cargo.toml Files (10 files)

| File | Changes |
|------|---------|
| `Cargo.toml` | workspace comment |
| `crates/llmbc/Cargo.toml` | name, description |
| `crates/llmc/Cargo.toml` | name, description, deps |
| `crates/llmvm/Cargo.toml` | name, description, deps |
| `crates/llmvm-cli/Cargo.toml` | name, description, bin name, deps |
| `crates/llmfw/Cargo.toml` | name, description, deps |
| `crates/llm-effect/Cargo.toml` | name, description, deps |
| `orchestrator/Cargo.toml` | name, bin name, deps |
| `packages/Cargo.toml` | name, bin name, deps |
| `tooling/Cargo.toml` | name, deps |

### Rust Source Files (~40 files)

All `use axiom_*::` imports → `use boruna_*::`:
- `crates/llmvm-cli/src/main.rs` — imports + user-facing strings
- `crates/llmbc/src/module.rs` — comments
- `crates/llmbc/src/opcode.rs` — comments
- `crates/llmbc/src/value.rs` — comments
- `crates/llmc/src/lib.rs` — imports
- `crates/llmc/src/codegen.rs` — imports
- `crates/llmc/src/tests.rs` — imports
- `crates/llmvm/src/vm.rs` — imports + comments
- `crates/llmvm/src/error.rs` — imports
- `crates/llmvm/src/replay.rs` — imports
- `crates/llmvm/src/actor.rs` — imports
- `crates/llmvm/src/capability_gateway.rs` — imports
- `crates/llmvm/src/tests.rs` — imports
- `crates/llmfw/src/runtime.rs` — imports
- `crates/llmfw/src/testing.rs` — imports
- `crates/llmfw/src/validate.rs` — imports
- `crates/llmfw/src/policy.rs` — imports
- `crates/llmfw/src/effect.rs` — imports
- `crates/llmfw/src/state.rs` — imports
- `crates/llmfw/src/ui.rs` — imports
- `crates/llmfw/src/error.rs` — imports
- `crates/llmfw/src/tests.rs` — imports
- `crates/llm-effect/src/gateway.rs` — imports
- `crates/llm-effect/src/normalize.rs` — imports
- `crates/llm-effect/src/cache.rs` — imports
- `crates/llm-effect/src/tests.rs` — imports
- `orchestrator/src/main.rs` — imports + CLI strings
- `orchestrator/src/adapters/mod.rs` — hardcoded crate names
- `orchestrator/src/storage/mod.rs` — comments
- `orchestrator/src/conflict/mod.rs` — test data strings
- `orchestrator/tests/integration.rs` — imports + test data strings
- `packages/src/main.rs` — imports + CLI strings
- `packages/src/spec/mod.rs` — imports
- `packages/src/storage/mod.rs` — imports + comments
- `packages/tests/integration.rs` — imports
- `tooling/src/stdlib/mod.rs` — imports
- `tooling/src/templates/mod.rs` — imports
- `tooling/src/diagnostics/collector.rs` — imports
- `tooling/src/diagnostics/analyzer.rs` — imports
- `tooling/src/diagnostics/suggest.rs` — imports
- `tooling/src/trace2tests/mod.rs` — imports + comments
- `tooling/src/tests.rs` — imports

### Documentation Files (16 files)

| File | Action |
|------|--------|
| `docs/BRANDING.md` | Full rewrite for Boruna |
| `docs/RENAMING.md` | Update with Axiom → Boruna phase |
| `docs/RENAME_AUDIT.md` | Historical — keep as-is, add note |
| `docs/QUICKSTART.md` | Update CLI names |
| `docs/language-guide.md` | Update project name |
| `docs/bytecode-spec.md` | Update project name |
| `docs/FRAMEWORK_SPEC.md` | Update project name |
| `docs/FRAMEWORK_STATUS.md` | Update CLI names |
| `docs/FRAMEWORK_API.md` | Update CLI names |
| `docs/TESTING_GUIDE.md` | Update CLI names |
| `docs/DIAGNOSTICS_AND_REPAIR.md` | Update CLI names |
| `docs/TRACE_TO_TESTS.md` | Update CLI names |
| `docs/APP_TEMPLATE.md` | Update CLI names |
| `docs/ORCHESTRATOR_SPEC.md` | Update project/CLI names |
| `docs/STD_LIBRARIES_SPEC.md` | Update project name |
| `docs/LLM_EFFECT_SPEC.md` | Update project name |
| `docs/PACKAGE_SPEC.md` | Update CLI names |

### Other Files

| File | Action |
|------|--------|
| `CLAUDE.md` | Update all Axiom → Boruna references |
| `examples/todo.ax` | "Learn Axiom" string → "Learn Boruna" |
| `examples/trace_demo/README.md` | Update references |
| `examples/repair_demo/README.md` | Update references |
| `examples/llm_patch_demo/README.md` | Update references |
| `host/web/package.json` | name field |
| `host/web/index.html` | title |
| `host/web/src/App.jsx` | heading + comments |
| `host/web/src/renderer.jsx` | comments |
| `orchestrator/spec/examples/*.json` | module name references |

### Files NOT Changed

- `target/` — generated build artifacts, will be rebuilt
- `Cargo.lock` — will be regenerated by `cargo build`
- Directory paths — remain unchanged (`crates/llmbc/`, etc.)
- `.ax` file extension — unchanged
- `package.ax.json` / `policy.ax.json` — unchanged
