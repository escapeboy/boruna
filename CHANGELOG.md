# Changelog

All notable changes to Boruna are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Versioned capability identity** ([#3](https://github.com/escapeboy/boruna/issues/3),
  sprint `0.3-S11`). New `boruna capability list [--json]` CLI subcommand and
  `boruna_capability_list` MCP tool report a stable `capability_set_hash` over
  the binary's capability surface. Integrators use it as part of a cache key —
  `(source_hash, policy_hash, capability_set_hash, policy.schema_version)` — to
  safely memoize deterministic run results across binary upgrades. Algorithm,
  caching recipe, and per-capability versioning rules documented in
  `docs/reference/capability-identity.md`. All 10 shipped capabilities start at
  contract version `"1"`.
- New library API in `boruna-bytecode`:
  `Capability::ALL` (canonical sorted iteration), `Capability::version()`,
  `CapabilityIdentity`, `CapabilitySetReport`,
  `compute_capability_set_hash()`, `capability_set_report()`.
- **`protocol_version: 1` field on every `boruna-mcp` tool response**
  ([#6](https://github.com/escapeboy/boruna/issues/6), sprint `0.5-S4`,
  pulled forward from 0.5.0 because FleetQ blocked on it for their
  validate-on-save UX). Wire-format version of the response envelope; bumps
  only on breaking shape changes (additive changes keep the version).
  Locked by `crates/boruna-mcp/src/tools/mod.rs::TOOL_RESPONSE_PROTOCOL_VERSION`
  and a 16-case regression test asserting every tool's success and failure
  path carries it. Versioning policy and bump rules documented in
  `docs/reference/mcp-server.md` under "Stability". Pairs with
  `Policy.schema_version` shipped in 0.2.0.
- **MCP Server Tool Reference** documentation at `docs/reference/mcp-server.md` —
  wire contract for all 10 `boruna-mcp` tools: parameter names and types,
  return shapes, `error_kind` values, encoding rules, and limits. Driven by
  FleetQ implementer feedback (post-v0.2.0 follow-up): integrators previously
  had to read `crates/boruna-mcp/src/server.rs` to learn that `boruna_run`'s
  parameter is `source` (not `script`) and that there is no `input` parameter.
  Linked from `docs/README.md`.


## [0.2.0] - 2026-04-25

Driven by [implementer feedback from FleetQ](https://github.com/escapeboy/boruna/issues?q=label%3Aenhancement) (production integrator). This release closes the two P0 adoption blockers; remaining P1/P2 asks are tracked as issues #3–#9.

### Added

- MCP `boruna_run` tool now accepts a structured `Policy` object for the `policy`
  parameter, in addition to the existing `"allow-all"` / `"deny-all"` string
  shorthands. This exposes the per-capability rules (`allow`, `budget`),
  `default_allow` mode (allowlist vs. denylist), and `net_policy` (allowed
  domains, methods, byte limits, timeout) that the VM has always supported.
  See `docs/reference/policy-schema.md` and `docs/reference/policy.schema.json`.
- New documentation: `docs/reference/policy-schema.md` (prose + examples) and
  `docs/reference/policy.schema.json` (machine-readable JSON Schema 2020-12)
  for integrators rendering capability matrices in their own UIs.
- The `boruna_run` MCP tool description now advertises the structured-policy
  capability so AI agents discover it from the tool list directly.
- Multi-target release workflow (`.github/workflows/release.yml`) that publishes
  static binaries on every `v*` tag for `x86_64-unknown-linux-musl`,
  `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`,
  plus a combined `SHA256SUMS` checksum file. Linux builds use musl so the
  binaries run on Alpine and other libc-minimal distributions.
- `docs/releasing.md` — release process, verification, and rationale for using
  GitHub-hosted runners (vs. the self-hosted runner used by `ci.yml`).
- README install section showing curl-and-verify install.

### Changed

- **Breaking (MCP only):** `boruna_run` now rejects unknown `policy` values
  (e.g. typo'd strings, numbers, arrays) with `success: false,
  error_kind: "invalid_policy"` instead of silently treating them as
  `"allow-all"`. The legacy strings `"allow-all"` and `"deny-all"` continue
  to behave identically.

## [0.1.0] - 2026-02-21

### Added

- Deterministic workflow execution engine with DAG validation and topological ordering
- Hash-chained audit logs (SHA-256) and self-contained evidence bundles for compliance
- Policy-gated capability system — 10 capabilities: `net.fetch`, `db.query`, `fs.read`,
  `fs.write`, `time.now`, `random`, `ui.render`, `llm.call`, `actor.spawn`, `actor.send`
- Replay engine for determinism verification via `EventLog` comparison
- Three reference workflow examples:
  - `llm_code_review` — linear 3-step pipeline demonstrating LLM capability and evidence recording
  - `document_processing` — fan-out/merge 5-step pipeline demonstrating parallel steps and DAG scheduling
  - `customer_support_triage` — approval-gate 4-step pipeline demonstrating human-in-the-loop and conditional pause
- MCP server (`boruna-mcp`) exposing 10 tools over JSON-RPC stdio for AI coding agent integration
- Actor system with `OneForOne` supervision and bounded execution scheduling (`Vm::execute_bounded`)
- `boruna-tooling`: diagnostics with source spans, auto-repair, trace-to-tests, stdlib test runner, 5 app templates
- `boruna-pkg`: deterministic package system with SHA-256 content hashing, dependency resolution, and lockfiles
- Real HTTP handler (feature-gated via `boruna-vm/http`) with SSRF protection for `net.fetch` capability
- CLI binary (`boruna`) with subcommands: `compile`, `run`, `trace`, `replay`, `inspect`, `ast`,
  `workflow`, `evidence`, `framework`, `lang`, `trace2tests`, `template`
- Standard library: 11 deterministic libraries — `std-ui`, `std-forms`, `std-authz`, `std-http`,
  `std-db`, `std-sync`, `std-validation`, `std-routing`, `std-storage`, `std-notifications`, `std-testing`
- 557+ tests across 9 crates

[Unreleased]: https://github.com/escapeboy/boruna/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/escapeboy/boruna/releases/tag/v0.2.0
[0.1.0]: https://github.com/escapeboy/boruna/releases/tag/v0.1.0
