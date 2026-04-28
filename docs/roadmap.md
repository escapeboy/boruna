# Roadmap

This roadmap describes what Boruna is working toward. It is realistic, not aspirational marketing. Items without a milestone are under consideration but not scheduled.

Last refreshed: 2026-04-27 (after the 0.5-S2 distributed-execution sprint cycle).

## Current: 0.4.0 (pending tag)

Workspace version is `0.4.0` in `Cargo.toml`. The distributed-execution stack from the 0.5.0 plan landed early via the 0.5-S2 sub-sprint cycle (S2a → S2f) on master without a release tag. The next release tag will likely be cut after the 0.5-S3 (auth) sprint lands, since exposing the coordinator publicly without auth is a footgun.

**On master since v0.3.0 tag:**
- 0.4-S1 → 0.4-S16 — Operations cycle (workflow dashboard, OpenTelemetry observability, Prometheus metrics, multi-environment namespacing, policy-as-code, fine-grained policy, output schema validation, net record/replay, capability identity hash, audit decision events, parent-dir fsync). Each sprint has a corresponding `docs/design-*.md` + retro under `retro/`.
- 0.5-S1 — ADR 002 (distributed step execution architecture).
- 0.5-S2a → 0.5-S2f — distributed-execution stack: claim/lease persistence, coordinator/worker HTTP MVP, background lease-expiry sweep, coord+dashboard listener-merge, `workflow run --submit-only`, `coordinator wait <run-id>` for multi-wave advancement.

**The 0.5.0 cycle is now end-to-end functional for arbitrary multi-wave workflows.** Operators can submit a real workflow.json + .ax files via `--submit-only` and drive to terminal status via `coordinator wait`, with a coord+workers cluster doing the actual execution.

## Previous: 0.3.0

Released 2026-04-26 — closes every big-rock theme on the original 0.3.0 plan: persistent workflow state (crash-resumable), concurrent step execution within waves, step retry policies, idempotent invocation, workflow versioning for CI/CD safety, the LLM-handler decision (BYOH), per-step attempt tracking with the project's first schema migration, workflow step output piping via the `step_input` builtin, and async step execution via the external-trigger CLI for webhook-driven workflows. Plus review-driven safety work (atomic trigger-commit closing a TOCTOU race; SSRF-hardened real HTTP handler).

See [`CHANGELOG.md`](../CHANGELOG.md#030--2026-04-26) for the full 0.3-S2a → 0.3-S16 sprint stack.

## Previous: 0.2.0

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

## 0.3.0 — Real-use durability — SHIPPED (2026-04-26)

Focus: workflows that survive process restarts, handle long-running steps, and unblock production use cases. Combined original 0.3.0 plan with two FleetQ P1 asks that fit thematically.

- [x] **Persistent workflow state** — checkpoint and resume across process restarts (`0.3-S2a`/`S2b`/`S3`/`S6`)
- [x] **Async step execution** — steps that wait for external events via webhook-driven CLI trigger (`0.3-S15`); approval gates (`0.3-S2c`)
- [x] **Step retry policies** — configurable retry with backoff on transient failures (`0.3-S5`)
- [x] **Workflow versioning** — `--expect-workflow-hash` for CI/CD safety (`0.3-S9`)
- [x] **Workflow step output piping** — `step_input` builtin (`0.3-S14`)
- [x] **Structured resource limits with typed errors** ([#5](https://github.com/escapeboy/boruna/issues/5)) — `max_memory_mb`, `max_wall_ms`, `max_output_bytes` (`0.3-S10`)
- [x] **Versioned capability identity** ([#3](https://github.com/escapeboy/boruna/issues/3)) — `boruna_capability_list` returns `capability_set_hash` for safe caching
- [x] **LLM live handler decision** — **DECIDED (sprint `0.3-S8`):** Bring Your Own Handler (BYOH). No default LLM handler ships in core; integrators wire their provider via the `CapabilityHandler` trait. Rationale + integration contract + reference OpenAI handler in [`docs/guides/llm-integration.md`](./guides/llm-integration.md).
- [x] **Concurrent step execution within waves** — `--concurrency N` (`0.3-S4`)
- [x] **Idempotent invocation** — `--skip-if-running` for cron-driven scheduling (`0.3-S7`/`S10`)
- [x] **Per-step attempt tracking** with first schema migration v1→v2 (`0.3-S11`/`S12`/`S13`)
- [x] **Atomic trigger commit** closing TOCTOU race (`0.3-S16`, review-driven)
- [ ] **Scheduled workflows** — trigger workflows on a cron schedule (deferred to 0.3.x; partially addressed by `--skip-if-running` for safe cron invocation)

## 0.4.0 — Operations (mostly shipped on master, tag pending)

Originally targeted Q4 2026; landed early as 0.4-S1 → 0.4-S16 + 0.5-S1 → 0.5-S2f. Tag will be cut after auth (0.5-S3) lands.

- [x] **Distributed step execution** — coord+workers HTTP cluster + multi-wave advancement (0.5-S2a → 0.5-S2f)
- [x] **Workflow dashboard** — Axum + askama SSR (0.4-S16); merged into the coordinator listener (0.5-S2d)
- [x] **Prometheus metrics endpoint** — `/metrics` route + per-run-status counters (0.4-S?)
- [x] **OpenTelemetry observability** ([#9](https://github.com/escapeboy/boruna/issues/9)) — per-capability OTLP spans (0.4-S5)
- [x] **Policy management as code** — `Policy` JSON files + `boruna policy validate` (0.4-S?)
- [x] **Multi-environment support** — `--env` flag + namespaced data-dir + Prometheus `env=` label (0.4-S14)
- [ ] **Streaming output from `boruna_run`** ([#4](https://github.com/escapeboy/boruna/issues/4)) — periodic `progress` events. P1 from FleetQ.
- [ ] **LLM provider registry** — configure and route between model providers
- [ ] **Scheduled workflows** (carried over from 0.3.x) — full cron daemon vs. external-scheduler-friendly mode

## 0.5.0 — Distributed execution + spec freeze

Target: ~Q3-Q4 2026 (accelerated from original Q1 2027 target).

Two sub-themes: (a) finish what `0.5-S2*` started so distributed mode is production-grade, (b) lock the API surface for 1.0.

### (a) Distributed-execution closure

- [x] **`workflow run --submit-only`** + **`coordinator wait`** — end-to-end multi-wave (0.5-S2e/f)
- [x] **0.5-S3 — Authentication** — shared-secret bearer token (mTLS deferred to 0.6.x). MUST land before any non-loopback bind is recommended. Gating for production deployments.
- [x] **0.5-S4 — `workflow run --coordinator <url>`** — combines submit + wait in one command for CI workflows
- [x] **0.5-S5 — Distributed retry policies** — wires `RetryPolicy` through the wait driver so failed steps with retry budget transition Failed → Pending instead of permanent Failed
- [x] **0.5-S6 — Distributed approval-gate / external-trigger** — generalizes the operator-bridge protocol from 0.3-S15 to work in distributed mode
- [ ] **0.5-S7 — Output blob references** — large step outputs (LLM responses) via content-addressed blob store; metadata carries refs only
- [ ] **Coordinator HA / failover** — eliminates the single-coord SPOF; worker leases survive coord failover
- [x] **Worker capability tagging / placement** — workers advertise capabilities; coord routes work to compatible workers (gates `coord.binary_mismatch` relaxation) (sprint `W3-A`)
- [ ] **Rolling upgrades** — heterogeneous worker versions via per-capability version negotiation

### (b) Spec freeze

- [x] **Stable, documented MCP tool response schemas** ([#6](https://github.com/escapeboy/boruna/issues/6)) — `protocol_version: 1` (0.5-S4 of FleetQ track)
- [x] **Output JSON Schema validation as first-class gate** ([#8](https://github.com/escapeboy/boruna/issues/8)) (0.5-S6 of FleetQ track)
- [x] **Record/replay for `net.fetch`** ([#7](https://github.com/escapeboy/boruna/issues/7)) (0.5-S7 of FleetQ track)
- [ ] **Versioned `.ax` language specification** — formal grammar, type rules, capability semantics. Each future release publishes against a `language_version`.
- [ ] **Versioned workflow DAG schema** — JSON Schema for `workflow.json` with `schema_version` field; backwards-compatible parser.
- [ ] **Versioned evidence bundle format** — schema for the bundle directory contents, `format_version` field, forward-compat reader.
- [ ] **Migration tooling beta** — `boruna migrate <from-version>` upgrade path for any pre-1.0 breaking change.

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
