# Changelog

All notable changes to Boruna are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Record/replay for `net.fetch`** ([#7](https://github.com/escapeboy/boruna/issues/7),
  sprint `0.5-S7`, pulled forward from 0.5.0). Boruna scripts are
  deterministic by design; external HTTP is not. New CLI flags on
  `boruna run`:
  - `--record-net-to <FILE>` (requires `--live`) makes real HTTP calls and
    persists each `(method, url, request_body) → response_body`
    transaction to a sidecar JSON tape file.
  - `--replay-net-from <FILE>` serves responses from a loaded tape with
    no real network access. Strict ordered match on
    `(method, url, request_body)`; mismatch returns a typed error
    naming the position and differing field; tape exhaustion returns a
    typed error; under-consumption is silently OK.
  - Mutually exclusive (clap `conflicts_with`). If `--live` is set
    alongside `--replay-net-from`, replay wins (no real calls happen).
- New module `boruna_vm::net_record_replay` (feature-gated under
  `http`) exposing `NetTransaction`, `NetTape`, `RecordingHttpHandler`,
  `ReplayingHttpHandler`, and `TAPE_FORMAT_VERSION`.
- `RecordingHttpHandler::with_save_path()` arms save-on-drop; the CLI
  also probes write access on the tape path **before** the run starts
  so a CI pipeline like `record-net-to fixtures/x.tape && verify x.tape`
  fails fast on disk errors instead of silently producing a stale
  fixture (review-driven hardening).
- New shared parser `boruna_vm::http_handler::parse_net_fetch_args()`
  used by both the real handler and the recording layer so they can't
  silently drift in arg interpretation.
- Documentation: `docs/design-net-record-replay.md` (tape format, match
  strategy, CLI surface, known limitations).
- **Known limitations** (documented in design doc + doc comments):
  request headers are NOT in the match key (auth tokens change between
  sessions); response status/headers are not recorded (handler returns
  body only); failed transactions and non-UTF-8 bodies are not taped;
  tape file size is unbounded (~2× JSON-pretty multiplier); a panic
  during recording may lose the tape if Drop doesn't run.

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
