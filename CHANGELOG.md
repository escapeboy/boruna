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
- **Structured resource limits in `boruna_run`** ([#5](https://github.com/escapeboy/boruna/issues/5),
  sprint `0.3-S10`, FleetQ P1). New optional `limits` parameter on the MCP
  `boruna_run` tool accepting `max_wall_ms`, `max_output_bytes`, and
  `max_memory_mb`. Overruns return a typed
  `error_kind: "limit_exceeded"` with a `limit_kind` discriminator
  (`"wall_ms"` or `"output_bytes"`), the configured `limit`, and a
  human-readable `message` — so callers can surface clean per-limit UX
  instead of parsing error strings. `max_memory_mb` is accepted in the
  schema but **not enforced** in 0.3.x (documented as platform-best-effort
  pending Linux `setrlimit` work in a future sprint).
- New `boruna-vm::error::VmError::WallTimeExceeded(u64)` variant and
  `Vm::set_max_wall_ms(Option<u64>)` setter. Wall-clock checked every 1024
  steps inside the execute loop; uses `std::time::Instant` (not
  `chrono::Utc::now()` per ADR 001 determinism contract). Wall-time
  enforcement is wall-clock-keyed and therefore non-deterministic on
  overrun by construction — `max_steps` remains the deterministic
  ceiling; `max_wall_ms` is the operational guardrail.
- **Output JSON Schema validation gate in `boruna_run`**
  ([#8](https://github.com/escapeboy/boruna/issues/8), sprint `0.5-S6`,
  pulled forward from 0.5.0 because FleetQ wanted it in their pipeline).
  New optional `output_schema` parameter on the MCP `boruna_run` tool
  accepting any JSON Schema 2020-12 object. The script's `result` is
  validated post-execution; mismatches return
  `error_kind: "validation_failed", phase: "output_validation"` with
  per-path JSON Pointer errors. Malformed or oversized schemas (>256 KB)
  return `error_kind: "invalid_output_schema"`. Schemas declaring a
  non-2020-12 `$schema` are rejected (same "reject at parse, don't
  silently override" pattern as `0.3-S10`'s `unsupported_limit`). Error
  array capped at 100 entries with `truncated` and `total_errors`
  fields. **Known limitation:** records/enums emit as wrapper objects;
  schemas for the natural shape will fail. Best for primitive returns.
  See `docs/design-output-schema.md`.
- New `jsonschema = "0.30"` dependency in `boruna-mcp` (default features
  off — no `resolve-http` or `resolve-file`, so `$ref` to remote URLs
  cannot trigger SSRF or arbitrary file reads).
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
- **Per-call OpenTelemetry observability** ([#9](https://github.com/escapeboy/boruna/issues/9),
  sprint `0.4-S5`, the LAST FleetQ ask). Always-on `tracing` instrumentation
  in `CapabilityGateway::call` emits `boruna.cap` spans with attributes
  `cap.name`, `bytes_in`, `bytes_out`, `cap.budget_remaining`, `error.kind`
  (set on the failure path: `denied` / `budget_exceeded` / `runtime_error`).
  When no subscriber is installed (the default), span macros are essentially
  no-ops — zero runtime cost.
- **`telemetry` Cargo feature** on `boruna-vm` (and mirror feature on
  `boruna-cli`) adds an OpenTelemetry OTLP-over-HTTP exporter
  (`opentelemetry 0.27` + `opentelemetry-otlp 0.27` + `tracing-opentelemetry
  0.28`). New helper `boruna_vm::init_telemetry()` reads
  `OTEL_EXPORTER_OTLP_ENDPOINT` (and optional `OTEL_SERVICE_NAME`,
  defaulting to `"boruna"`); returns a `Disabled` no-op handle when the
  endpoint is unset (Boruna behaves identically to a non-telemetry build),
  installs the exporter when set. Returns a `TelemetryHandle` whose `Drop`
  flushes pending spans.
- **CLI integration:** `boruna-cli` built with `--features telemetry` starts
  a tokio runtime in `main`, calls `init_telemetry()` BEFORE parsing CLI
  args, holds the handle for the binary lifetime, and on shutdown drops
  the handle THEN drains the runtime with a 5-second timeout (so
  in-flight OTel HTTP POSTs complete instead of being killed by
  `process::exit`).
- New documentation: `docs/design-otel.md` (span shape, attribute table,
  determinism contract, library-version pin set, BYO-subscriber fallback
  path).
- **`boruna_orchestrator::persistence::RunCheckpointStore`** — SQLite-backed
  workflow checkpoint store (sprint `0.3-S2a`). Implements ADR 001 step
  1–5: schema, Connection setup with mandatory PRAGMAs (`journal_mode=WAL`,
  `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`), CRUD
  operations (`insert_run`, `update_run_status`, `get_run`,
  `list_runs_by_status`, `upsert_step_checkpoint`,
  `list_step_checkpoints`), and a `BEGIN IMMEDIATE` retry policy that
  handles both `SQLITE_BUSY` and `SQLITE_LOCKED` with exponential
  backoff (10ms→50ms→250ms→1.25s) before failing with
  `PersistenceError::Busy`. **Not yet wired into `WorkflowRunner` —
  that integration lands in `0.3-S2b`** (along with `boruna workflow
  resume <run-id>` and `--data-dir`).
- New `persist-sqlite` Cargo feature on `boruna-orchestrator` (default-on).
  Adds `rusqlite = "0.32"` with the `bundled` feature so SQLite compiles
  from C source — preserves the musl-static-binary story per ADR 001.
- Schema embedded via `include_str!("schema_v1.sql")`. Single-row
  `schema_version` table with `CHECK (id = 1)` constraint structurally
  prevents stale-row accumulation across migration attempts.
- `PersistenceError::NotFound { entity, key }` returned by
  `update_run_status` when the target `run_id` does not exist (review-
  driven; silent-no-op was rejected as a footgun for the resume path).
- `upsert_step_checkpoint` uses `COALESCE(excluded.X, existing.X)` for
  `started_at`, `output_json`, `output_hash` so a partial upsert (e.g.
  step transition from Running to Completed without re-supplying
  started_at) preserves the original value rather than clobbering to
  NULL (review-driven; locked by two regression tests).
- `docs/design-persistence-store.md` — sprint scope split rationale,
  acceptance criteria, schema annotation conventions.

### Decided

- **ADR 001 — Persistence Backend** (`docs/adr/001-persistence-backend.md`).
  SQLite via `rusqlite/bundled` chosen as the workflow-checkpoint backend.
  No persistence-trait abstraction in v1 — direct concrete dependency.
  Includes a determinism contract for persisted state (operational vs.
  replay-verified columns), the writer serialization model, mandatory
  connection PRAGMAs (`journal_mode=WAL`, `foreign_keys=ON`,
  `busy_timeout=5000`), and an illustrative schema. Unblocks `0.3-S2`
  through `0.3-S9` — the entire 0.3.0 critical path. Sprint `0.3-S1`.
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
