# Renaming History

## Phase 2: Axiom → Boruna

**Date**: 2026-02-21
**Version**: 0.1.0

### Name Changes

| Old Name | New Name |
|----------|----------|
| Axiom | Boruna |
| axiom (CLI) | boruna |
| axiom-cli (crate) | boruna-cli |
| axiom-bytecode (crate) | boruna-bytecode |
| axiom-compiler (crate) | boruna-compiler |
| axiom-vm (crate) | boruna-vm |
| axiom-framework (crate) | boruna-framework |
| axiom-effect (crate) | boruna-effect |
| axiom-tooling (crate) | boruna-tooling |
| axiom-pkg (crate+binary) | boruna-pkg |
| axiom-orchestrator (crate) | boruna-orchestrator |
| axiom-orch (binary) | boruna-orch |
| axiom-web-host (npm) | boruna-web-host |

### Compatibility

- Rust imports use underscored forms: `use boruna_vm::Vm`, `use boruna_bytecode::Value`, etc.
- CLI binaries: `boruna`, `boruna-pkg`, `boruna-orch`
- Source files: `.ax` extension (unchanged)
- Package manifests: `package.ax.json` (unchanged)

---

## Phase 1: LLM-Lang → Axiom

**Date**: 2026-02-20
**Version**: 0.1.0

### Name Changes

| Old Name | New Name |
|----------|----------|
| LLM-Lang | Axiom |
| llmvm (CLI) | axiom |
| llmvm (crate) | axiom-vm |
| llmvm-cli (crate) | axiom-cli |
| llmc | axiom-compiler |
| llmbc | axiom-bytecode |
| llmfw | axiom-framework |
| llm-effect | axiom-effect |
| llmpkg | axiom-pkg |
| orch | axiom-orch |
| orchestrator (crate) | axiom-orchestrator |
| tooling (crate) | axiom-tooling |
| .llm (source) | .ax |
| .llm.template | .ax.template |
| package.llm.json | package.ax.json |
| policy.llm.json | policy.ax.json |

### Directory Structure

Directory paths are **unchanged** — only crate names, binary names, and file extensions changed:

```
crates/llmbc/       → still crates/llmbc/       (crate name: boruna-bytecode)
crates/llmc/        → still crates/llmc/        (crate name: boruna-compiler)
crates/llmvm/       → still crates/llmvm/       (crate name: boruna-vm)
crates/llmfw/       → still crates/llmfw/       (crate name: boruna-framework)
crates/llm-effect/  → still crates/llm-effect/  (crate name: boruna-effect)
crates/llmvm-cli/   → still crates/llmvm-cli/   (crate name: boruna-cli)
orchestrator/       → still orchestrator/       (crate name: boruna-orchestrator)
packages/           → still packages/           (crate name: boruna-pkg)
tooling/            → still tooling/            (crate name: boruna-tooling)
```
