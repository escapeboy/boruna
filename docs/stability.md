# Stability and Maturity

Boruna is at version **0.1.0**. It is an early-stage project. This document is explicit about what is stable, what is experimental, and what is planned.

## Current status

Boruna is a **working prototype** with a clear production trajectory. The core execution engine is functional and fully tested (557+ tests). The public API surface is not yet stable — breaking changes may occur in minor versions until 1.0.

Boruna is appropriate for:
- Evaluation and proof-of-concept workflows
- Internal tooling where you control the version
- Teams building audit-sensitive AI pipelines who want to evaluate the architecture early

Boruna is not yet appropriate for:
- Production workloads requiring guaranteed API stability
- Regulated environments requiring vendor certification
- Large-scale deployments without an in-house Rust team

## Stability tiers

### Stable (breaking changes only in major versions)

These components are complete, tested, and behave as documented:

- **`.ax` language core** — syntax, type system, pattern matching, records, enums
- **VM execution** — bytecode format, capability enforcement, determinism guarantees
- **Workflow DAG** — `workflow.json` format, topological execution, step isolation
- **Evidence bundles** — hash-chained log format, `evidence verify` output
- **Capability system** — the 10 capabilities and their enforcement semantics
- **CLI commands** — `run`, `compile`, `workflow validate/run`, `evidence inspect/verify`

### Experimental (may change in minor versions)

These components work but may change based on usage feedback:

- **Actor system** — spawning, message passing, supervision semantics
- **Multi-agent orchestration** — `boruna-orch` binary and its API
- **Package system** — `boruna-pkg` manifest format and registry protocol
- **MCP server** — tool schemas and JSON-RPC protocol
- **Standard libraries** — `std-*` library APIs
- **App templates** — template variable names and generated code structure
- **`trace2tests`** — test generation format and minimization behavior

### Alpha (expect breaking changes)

These components are available but under active development:

- **`--live` HTTP handler** — real network calls, SSRF policy, response handling
- **Replay verification** — semantics of `--verify` with partial replays
- **Framework app testing** — `framework test` message protocol

### Planned (not yet implemented)

These capabilities are on the roadmap but do not exist yet:

- Persistent workflow state (survives process restart)
- Distributed step execution across machines
- LLM provider registry and model routing
- Web-based evidence inspector
- Commercial platform features (SSO, RBAC, policy management UI)

See [roadmap.md](./roadmap.md) for the full timeline.

## What "stable" means

For stable components: a `.ax` file that compiles and runs correctly on 0.1.x will continue to compile and run correctly on 0.2.x and 0.3.x. If a breaking change becomes necessary, it will be documented in `CHANGELOG.md` with a migration path.

For experimental and alpha components: best-effort compatibility, with breakage documented in CHANGELOG.

## Versioning policy

Boruna follows [Semantic Versioning](https://semver.org):

- **Patch** (0.1.x): Bug fixes, security patches, no API changes
- **Minor** (0.x.0): New capabilities, experimental components may change, stable components preserved
- **Major** (x.0.0): Breaking changes to stable API surface; full migration guide provided

Until 1.0.0, the minor version increment may include breaking changes to experimental and alpha components without a major bump.

## Dependency on nightly Rust

Boruna currently builds on stable Rust. No nightly features are required. Minimum supported Rust version (MSRV): **1.75.0**.

## Security

See [SECURITY.md](../SECURITY.md) for the vulnerability disclosure policy and supported version matrix.
