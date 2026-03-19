# Roadmap

This roadmap describes what Boruna is working toward. It is realistic, not aspirational marketing. Items without a milestone are under consideration but not scheduled.

## Current: 0.1.x (working prototype)

The core execution engine is complete and tested. The goal for 0.1.x patch releases is stability and polish — no new features, only bug fixes and documentation.

**What exists today:**
- `.ax` language: types, pattern matching, records, enums, capability annotations
- VM: bytecode execution, capability enforcement, actor system, replay engine
- Workflow engine: DAG execution, topological sort, step isolation, approval gates
- Evidence bundles: hash-chained audit logs, `evidence verify/inspect`
- CLI: compile, run, trace, replay, workflow, evidence, lang, template, framework
- MCP server: 10 tools for AI coding agent integration
- Standard libraries: 11 pure-functional stdlib modules
- HTTP handler: real network calls (feature-gated, SSRF-protected)
- Package system: manifest format, dependency resolution, SHA-256 integrity
- 557+ tests covering all crates

## 0.2.0 — Developer experience

Target: Q2 2026

Focus: making Boruna usable without friction for teams evaluating it.

- [ ] `boruna new` command — scaffold a new workflow from a template interactively
- [ ] Improved error messages — actionable diagnostics with suggested fixes for all common mistakes
- [ ] `boruna fmt` — auto-formatter for `.ax` files
- [ ] Watch mode — `boruna run --watch` re-runs on file change
- [ ] Better `lang repair` — handle more repair cases automatically
- [ ] Evidence bundle diff — compare two runs side-by-side
- [ ] Workflow step output piping — pass step outputs as typed inputs to downstream steps
- [ ] Expanded stdlib — `std-llm`, `std-json` libraries

## 0.3.0 — Persistence and durability

Target: Q3 2026

Focus: workflows that survive process restarts and handle long-running steps.

- [ ] Persistent workflow state — checkpoint and resume across process restarts
- [ ] Async step execution — steps that wait for external events (webhooks, approvals)
- [ ] Scheduled workflows — trigger workflows on a cron schedule
- [ ] Workflow versioning — run a workflow at a specific commit/version
- [ ] Step retry policies — configurable retry with backoff on transient failures
- [ ] Timeout enforcement — per-step and per-workflow execution limits

## 0.4.0 — Scale and operations

Target: Q4 2026

Focus: running Boruna in team environments.

- [ ] Distributed step execution — run steps on separate worker processes
- [ ] Workflow dashboard — web UI for run history, step status, evidence inspection
- [ ] Metrics and observability — Prometheus-compatible metrics endpoint
- [ ] Policy management — define and version capability policies as code
- [ ] LLM provider registry — configure and route between model providers
- [ ] Multi-environment support — dev/staging/production policy separation

## 1.0.0 — Production readiness

Target: 2027

Milestone: the stable API surface is locked. 0.x programs compile and run unchanged.

- [ ] Stable `.ax` language specification (versioned)
- [ ] Stable workflow DAG schema (versioned)
- [ ] Stable evidence bundle format (versioned)
- [ ] Migration tooling for any pre-1.0 breaking changes
- [ ] Long-term support commitment for 1.x
- [ ] Security audit of the VM and capability enforcement
- [ ] Performance benchmarks and published baseline

## Future / under consideration

These items are on the long-term radar but not scheduled:

- **Commercial platform**: hosted workflow execution, managed evidence storage, SSO, RBAC, compliance reporting — built on the open source core
- **IDE integration**: language server (LSP) for `.ax` syntax, completion, and diagnostics in VS Code / Neovim
- **Model evaluation framework**: run the same workflow against multiple LLM providers and compare evidence bundles
- **Compliance templates**: pre-built workflow patterns for common regulated use cases (SOC 2, HIPAA, financial audit)
- **Cross-language FFI**: call into Rust/Python libraries from `.ax` through a typed capability interface

## What is intentionally out of scope

Boruna will not become:
- A general-purpose programming language (use Rust, Python, etc. for that)
- An LLM framework (use LangChain, LCEL, etc. for that)
- A cloud provider (Boruna runs where you deploy it)
- A no-code tool (Boruna is for engineers)

See also: [Stability](./stability.md), [Limitations](./limitations.md)
