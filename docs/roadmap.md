# Roadmap

This roadmap describes what Boruna is working toward. It is realistic, not aspirational marketing. Items without a milestone are under consideration but not scheduled.

Last refreshed: 2026-04-25 (after v0.2.0 ship).

## Current: 0.2.0

Released 2026-04-25 — driven by [FleetQ implementer feedback](https://github.com/escapeboy/boruna/issues?q=label%3Aenhancement). Closes the two P0 adoption blockers; other P1/P2 asks tracked as issues #3–#9.

**What shipped:**
- **Fine-grained capability policy in MCP `boruna_run`** — accepts a structured `Policy` object (per-capability allow/budget rules, allowlist vs. denylist mode, `NetPolicy` with allowed_domains / methods / byte limits / timeout), in addition to the legacy `"allow-all"` / `"deny-all"` strings. Documented JSON Schema 2020-12 at `docs/reference/policy.schema.json`. **Breaking (MCP only):** unknown policy values now return `error_kind: "invalid_policy"` instead of silently treating them as `"allow-all"`.
- **Multi-target static binary releases** on every `v*` tag: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, plus combined `SHA256SUMS`. Linux builds are musl so they run on Alpine and other libc-minimal distros.
- `docs/releasing.md` — release process and verification.

**What did NOT ship from the original 0.2.0 plan** (deferred to 0.2.x or 0.3.0):
- `boruna new` interactive scaffold
- `boruna fmt` auto-formatter
- Watch mode (`boruna run --watch`)
- Improved error messages with suggested fixes for all common mistakes
- Better `lang repair` coverage
- Evidence bundle diff
- Workflow step output piping
- `std-llm`, `std-json` library expansion

These were displaced by the FleetQ adoption work. They are still on the path to v1 — see 0.2.x and 0.3.0 below.

## 0.2.x — Developer experience patch lane

Target: rolling, May–July 2026

The DX work originally scoped for 0.2.0 ships incrementally as point releases. Each is small, additive, and non-breaking.

- [ ] `boruna new` — scaffold a new workflow from a template interactively
- [ ] `boruna fmt` — auto-formatter for `.ax` files
- [ ] `boruna run --watch` — re-run on file change
- [ ] Improved error messages — actionable diagnostics with suggested fixes for common mistakes
- [ ] Better `lang repair` — handle more repair cases automatically
- [ ] Evidence bundle diff — compare two runs side-by-side
- [ ] Expanded stdlib — `std-llm`, `std-json` libraries

## 0.3.0 — Real-use durability

Target: Q3 2026

Focus: workflows that survive process restarts, handle long-running steps, and unblock production use cases. Combines original 0.3.0 plan with two FleetQ P1 asks that fit thematically.

- [ ] **Persistent workflow state** — checkpoint and resume across process restarts
- [ ] **Async step execution** — steps that wait for external events (webhooks, approvals)
- [ ] **Scheduled workflows** — trigger workflows on a cron schedule
- [ ] **Step retry policies** — configurable retry with backoff on transient failures
- [ ] **Workflow versioning** — run a workflow at a specific commit/version
- [ ] **Workflow step output piping** — pass step outputs as typed inputs to downstream steps (deferred from 0.2.0)
- [ ] **Structured resource limits with typed errors** ([#5](https://github.com/escapeboy/boruna/issues/5)) — `max_memory_mb`, `max_wall_ms`, `max_output_bytes`, returning `error_kind: "limit_exceeded"`. P1 from FleetQ.
- [ ] **Versioned capability identity** ([#3](https://github.com/escapeboy/boruna/issues/3)) — `boruna_capability_list` returns `capability_set_hash` so integrators can safely cache results across binary upgrades. P1 from FleetQ. Pairs with the `Policy.schema_version` already in 0.2.0.
- [x] **LLM live handler decision** — **DECIDED (sprint `0.3-S8`):** Bring Your Own Handler (BYOH) is the supported model. No default LLM handler ships in core; integrators wire their provider via the `CapabilityHandler` trait. Rationale + integration contract + reference OpenAI handler in [`docs/guides/llm-integration.md`](./guides/llm-integration.md).

## 0.4.0 — Operations

Target: Q4 2026

Focus: running Boruna in team environments. Combines original 0.4.0 plan with FleetQ observability + UX asks.

- [ ] **Distributed step execution** — run steps on separate worker processes
- [ ] **Workflow dashboard** — web UI for run history, step status, evidence inspection
- [ ] **Prometheus metrics endpoint**
- [ ] **OpenTelemetry observability** ([#9](https://github.com/escapeboy/boruna/issues/9)) — per-capability OTLP spans (`boruna.cap.<name>`) with timing, byte counts, error attributes. Activation via `OTEL_EXPORTER_OTLP_ENDPOINT`. P2 from FleetQ.
- [ ] **Streaming output from `boruna_run`** ([#4](https://github.com/escapeboy/boruna/issues/4)) — periodic `progress` events plus optional `EventLog` event streaming. P1 from FleetQ.
- [ ] **Policy management** — define and version capability policies as code
- [ ] **LLM provider registry** — configure and route between model providers
- [ ] **Multi-environment support** — dev/staging/production policy separation

## 0.5.0 — Spec freeze candidate

Target: Q1 2027

Focus: lock down what 1.0 will guarantee. No new capabilities; this is the version where the API surface stops moving. Anything that ships here is what 1.0 commits to forever.

- [ ] **Versioned `.ax` language specification** — formal grammar, type rules, capability semantics. Each future release publishes against a `language_version`.
- [ ] **Versioned workflow DAG schema** — JSON Schema for `workflow.json` with `schema_version` field; backwards-compatible parser.
- [ ] **Versioned evidence bundle format** — schema for the bundle directory contents, `format_version` field, forward-compat reader.
- [ ] **Stable, documented MCP tool response schemas** ([#6](https://github.com/escapeboy/boruna/issues/6)) — `protocol_version: 1` on every tool response (validate, run, check, repair, etc.). P1 from FleetQ.
- [ ] **Migration tooling beta** — `boruna migrate <from-version>` upgrade path for any pre-1.0 breaking change.
- [ ] **Output JSON Schema validation as first-class gate** ([#8](https://github.com/escapeboy/boruna/issues/8)) — declare schema, get typed validation errors. P2 from FleetQ.
- [ ] **Record/replay for `net.fetch`** ([#7](https://github.com/escapeboy/boruna/issues/7)) — distinctive selling point that lands in spec freeze for stable storage format. P2 from FleetQ.

## 1.0.0 — Production readiness

Target: Q2 2027

Milestone: the stable API surface is locked. 0.5+ programs compile and run unchanged. This is mostly a *commitment* release, not a feature release — the engineering between 0.5 and 1.0 is small but the durability promise is large.

- [ ] **Security audit** of the VM and capability enforcement (external auditor; bookable months in advance — must commit Q4 2026 to land Q2 2027)
- [ ] **Performance benchmarks** — published baseline for compile time, step throughput, evidence bundle write/verify time
- [ ] **Long-term support commitment for 1.x** — backports for security fixes, deprecation policy
- [ ] **Migration tooling stable** (graduated from 0.5 beta)
- [ ] All schemas (language, DAG, evidence) finalized and documented
- [ ] **Evidence bundle storage adapters** — pluggable shipping to S3 / object storage / document store, beyond local files
- [ ] **Evidence bundle encryption** — at-rest encryption for bundles containing sensitive data

## What we need to decide *now* (before 0.3.0 starts)

These decisions block downstream planning. None of them are urgent today, but each one becomes urgent within 1–2 quarters.

1. **Security audit booking** — pick auditor, scope, budget by Q4 2026. A real audit costs $30–100k and books months in advance. If this slips past Q4 2026, v1.0.0 slips with it.
2. ~~**LLM live handler shipping plan**~~ — **decided** (`0.3-S8`): Bring Your Own Handler. See [`docs/guides/llm-integration.md`](./guides/llm-integration.md).
3. ~~**Persistence storage backend**~~ — **decided** ([ADR 001](./adr/001-persistence-backend.md)): sqlite, no abstraction trait. Shipped via 0.3-S2a/S2b/S3/S6.
4. **Dashboard scope and tech** — full SSR Rust stack (Axum + askama, fits the project) vs. SPA (more work, more polish). 0.4.0 dashboard depends on this answer.

## Future / under consideration

These items are on the long-term radar but not scheduled:

- **Commercial platform**: hosted workflow execution, managed evidence storage, SSO, RBAC, compliance reporting — built on the open source core.
- **IDE integration**: language server (LSP) for `.ax` syntax, completion, and diagnostics in VS Code / Neovim.
- **Model evaluation framework**: run the same workflow against multiple LLM providers and compare evidence bundles.
- **Compliance templates**: pre-built workflow patterns for common regulated use cases (SOC 2, HIPAA, financial audit).
- **Cross-language FFI**: call into Rust/Python libraries from `.ax` through a typed capability interface.

## What is intentionally out of scope

Boruna will not become:

- A general-purpose programming language (use Rust, Python, etc. for that)
- An LLM framework (use LangChain, LCEL, etc. for that)
- A cloud provider (Boruna runs where you deploy it)
- A no-code tool (Boruna is for engineers)

## Tracking

- **Filed issues:** https://github.com/escapeboy/boruna/issues
- **FleetQ feedback issues:** [#3](https://github.com/escapeboy/boruna/issues/3) through [#9](https://github.com/escapeboy/boruna/issues/9)
- **Past sprint retros:** `retro/`

See also: [Stability](./stability.md), [Limitations](./limitations.md), [Releasing](./releasing.md)
