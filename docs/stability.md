# Stability and Maturity

Boruna is at version **1.0.0-rc1**. The first 1.0 release candidate. This document is explicit about what is stable, what is experimental, and what is planned.

> **LTS contract for 1.x:** see [`lts.md`](./lts.md). The **Stable** tier
> below is what becomes LTS-protected at 1.0 GA — the surfaces listed there
> are the same surfaces the LTS document commits to preserving across the
> 1.x line. Experimental and Alpha tiers are explicitly NOT LTS-protected
> and may break in 1.x minor releases.

## Current status

Boruna is feature-complete for the 1.0 surface and currently soak-testing as a release candidate. The core execution engine, distributed-execution stack, three formal versioned specifications (`.ax` language, workflow DAG, evidence bundle), HA coordinator, mTLS, bundle encryption, capability-tagged worker placement, blob GC, migration tooling, and performance baselines are all shipped and tested (1175+ tests, all passing).

Boruna is appropriate for:
- Evaluation and proof-of-concept workflows
- Internal tooling and audit-sensitive AI pipelines
- Teams that want to adopt the architecture before 1.0 GA

Boruna is not yet appropriate for:
- Regulated environments requiring vendor certification (book external security audit per [`lts.md`](./lts.md))
- Workloads exceeding the [`PERFORMANCE.md`](./PERFORMANCE.md) budget without your own benchmarking
- Storage layouts beyond local filesystem (cloud-storage adapters are 0.7.x territory)

## Stability tiers

### Stable (LTS-protected at 1.0 GA — see [`lts.md`](./lts.md) §B)

These components are complete, tested, and behave as documented. Every 1.0 program continues to work on every 1.y release:

- **`.ax` language 1.0** — syntax, type system, pattern matching, records, enums; formal spec at [`spec/ax-language-1.0.md`](./spec/ax-language-1.0.md)
- **VM execution** — bytecode format, capability enforcement, determinism guarantees
- **Workflow DAG 1.0** — `workflow.json` format with `schema_version: 1`, topological execution, step isolation; spec at [`spec/workflow-dag-1.0.md`](./spec/workflow-dag-1.0.md)
- **Evidence bundle 1.0** — hash-chained log + `bundle.json` manifest with `format_version: "1.0"`, optional AES-256-GCM envelope encryption; spec at [`spec/evidence-bundle-1.0.md`](./spec/evidence-bundle-1.0.md)
- **Capability system** — the capability set is frozen at 1.0; any additions in 1.x are additive
- **CLI commands** — `run`, `compile`, `workflow validate/run/approve`, `evidence inspect/verify/gc-blobs`, `coordinator serve/wait`, `worker run`, `migrate`, `new`, `lang check/repair`, `template list/apply`
- **Coord/worker HTTP protocol** — `protocol_version: 1` responses, locked `coord.*` and `evidence.*` `error_kind` taxonomy
- **MCP tool response shapes** — `protocol_version: 1` carried on every response (success and failure)
- **HA + mTLS surfaces** — multi-coord deployments, worker URL failover, X.509 client certs

### Experimental (may change in minor versions)

These components work but may change based on usage feedback:

- **Actor system** — spawning, message passing, supervision semantics
- **Multi-agent orchestration** — `boruna-orch` binary and its API
- **Package system** — `boruna-pkg` manifest format and registry protocol
- **Standard libraries** — `std-*` library APIs (interfaces stabilizing across 1.x minors)
- **App templates** — template variable names and generated code structure
- **`trace2tests`** — test generation format and minimization behavior
- **Migration tooling** — `boruna migrate` is currently in beta; covered migrators are stable, additional migrators may ship in 1.x

### Alpha (expect breaking changes)

These components are available but under active development:

- **`--live` HTTP handler** — real network calls, SSRF policy, response handling
- **Replay verification** — semantics of `--verify` with partial replays
- **Framework app testing** — `framework test` message protocol

### Planned (post-1.0 — see [roadmap.md](./roadmap.md))

These capabilities are on the roadmap but do not yet exist:

- Evidence bundle storage adapters (S3 / object storage / document store) — 0.7.x or 1.x minor
- Rolling-upgrade per-capability version negotiation — 0.7.x
- Streaming output from `boruna_run` — 1.x minor (FleetQ P1)
- LLM provider registry and model routing
- `boruna fmt` v2 (comment-preserving formatter) and `boruna run --watch`
- Web-based evidence inspector
- Commercial platform features (SSO, RBAC, policy management UI)

## What "stable" means

For stable components (LTS-protected at 1.0 GA): a `.ax` file, workflow.json, or evidence bundle that compiles, validates, or verifies on 1.0 will continue to do so on every 1.y release. Per [`lts.md`](./lts.md): `language_version: "1.x"`, workflow DAG `schema_version: 1`, and evidence bundle `format_version: "1.x"` are forward-compat-readable across the entire 1.x line.

For experimental and alpha components: best-effort compatibility, with breakage documented in CHANGELOG `### Changed` or `### Deprecated`.

## Versioning policy

Boruna follows [Semantic Versioning](https://semver.org):

- **Patch** (1.0.x): Bug fixes, security patches, no API changes
- **Minor** (1.x.0): New capabilities, experimental components may change, stable components preserved per LTS contract
- **Major** (x.0.0): Breaking changes to stable API surface; deprecation announced ≥6 months prior in a minor release; full migration tooling provided

## Dependency on nightly Rust

Boruna builds on stable Rust. No nightly features are required. Minimum supported Rust version (MSRV): **1.75.0**.

## Security

See [SECURITY.md](../SECURITY.md) for the vulnerability disclosure policy, supported version matrix, and CVSS-based backport SLAs (CRITICAL/HIGH within 7 days of disclosure).
