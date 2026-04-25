# Sprints to v1.0.0

Breakdown of remaining roadmap items into discrete, shippable sprints. Each sprint is sized to merge one focused PR with clear exit criteria.

**Sizing convention:**
- **S** — 1–3 days (mostly one file or small module)
- **M** — 1 week (multi-crate, includes tests + docs)
- **L** — 2 weeks (architectural change, design doc warranted)

**Naming:** `<version>-S<n>` (e.g. `0.3-S1`). Sprint numbers reset per version. Dependencies use the same IDs.

**Execution principle:** sprints with no shared `Deps` can run in parallel if team capacity allows. The "Critical path" diagrams at the end show the longest dependency chain per version.

Last updated: 2026-04-25.

---

## 0.2.x — Developer experience patch lane

Rolling, May–July 2026. Interleaved with 0.3.0 work (no shared code paths). Each ships as a `0.2.x` patch release.

| ID | Goal | Scope | Deps | Size | Issue |
|---|---|---|---|---|---|
| 0.2.x-S1 | `boruna new` interactive scaffold | Pick template, prompt for variables, write to disk, run `compile` to verify | — | S | — |
| 0.2.x-S2 | `boruna fmt` auto-formatter | AST → canonical formatter; `--check` flag for CI; format `examples/` in CI | — | M | — |
| 0.2.x-S3 | `boruna run --watch` | File watcher (notify-rs) + debounced re-run; Ctrl-C clean exit | — | S | — |
| 0.2.x-S4 | Improved error messages | Audit top 10 most common diagnostics; add suggested-fix hints to each; align with `lang repair` | — | M | — |
| 0.2.x-S5 | `lang repair` coverage expansion | Identify next 5 high-frequency diagnostics without auto-repair; add patches | 0.2.x-S4 | M | — |
| 0.2.x-S6 | Evidence bundle diff | `boruna evidence diff <a> <b>` — side-by-side step-by-step comparison; JSON + human output | — | M | — |
| 0.2.x-S7 | `std-json` library | Parse, stringify, basic JSON path; fully pure | — | S | — |
| 0.2.x-S8 | `std-llm` library | Prompt template helpers, response parsing; capability `llm.call` declarations only | 0.3-S11 (LLM handler decision) | M | — |

Total: ~7–9 weeks of solo engineer work, ships as 8 incremental patch releases.

---

## 0.3.0 — Real-use durability

Q3 2026. The release that makes Boruna usable for production workflows that survive process restarts.

| ID | Goal | Scope | Deps | Size | Issue |
|---|---|---|---|---|---|
| 0.3-S1 | Persistence backend decision + ADR | Compare sqlite vs. postgres vs. pluggable; write `docs/adr/001-persistence-backend.md`; pick one | — | S | — |
| 0.3-S2 | Persistent workflow state — MVP | Schema, write checkpoint after each step, `boruna workflow resume <run-id>` | 0.3-S1 | L | — |
| 0.3-S3 | Persistent state — production hardening | Crash recovery, partial-step rollback, transactional checkpoint, eviction policy | 0.3-S2 | M | — |
| 0.3-S4 | Async step execution | Step types: `webhook` (resume on POST), `external_event` (named); `boruna workflow inject-event` CLI | 0.3-S3 | L | — |
| 0.3-S5 | Step retry policies | Schema: `retry: { max: 3, backoff: "exponential" }`; integration with timeout | 0.3-S3 | M | — |
| 0.3-S6 | Per-step + per-workflow timeout enforcement | `timeout_ms` in step definition; SIGKILL on exceed; typed `error_kind: "timeout"` | 0.3-S3 | S | — |
| 0.3-S7 | Scheduled workflows | `boruna workflow schedule <dag> --cron "..."` ; daemon mode `boruna scheduler run` | 0.3-S3 | M | — |
| 0.3-S8 | Workflow versioning | `workflow_version` field; resolve to git commit; `boruna workflow run --version v1.2.0` | 0.3-S3 | M | — |
| 0.3-S9 | Workflow step output piping (typed) | Schema: `inputs: [{ from: "step1.output.field" }]`; type-check at validate time | 0.3-S3 | M | — |
| 0.3-S10 | Structured resource limits | `max_wall_ms`, `max_output_bytes` first; `max_memory_mb` as best-effort; typed `error_kind: "limit_exceeded"` | — | M | [#5](https://github.com/escapeboy/boruna/issues/5) |
| 0.3-S11 | Versioned capability identity | `boruna --capability-list --json` returns `capability_set_hash`; document caching contract | — | S | [#3](https://github.com/escapeboy/boruna/issues/3) |
| 0.3-S12 | LLM live handler — ship or document BYO | Decision sprint: implement default Anthropic/OpenAI handler OR formally document BYO interface in `docs/effects-guide.md` | — | M (either path) | — |

**Critical path (longest chain):** S1 → S2 → S3 → S4 → S7 → S8 → S9 = ~9 weeks sequential.
**Parallelizable:** S10, S11, S12 can each start day 1 (no deps on persistence). S5, S6 unlock as soon as S3 lands.

Recommended team size: **2 engineers in parallel** = ~7 weeks calendar.

---

## 0.4.0 — Operations

Q4 2026. The release that lets a team operate Boruna instead of just developing on it.

| ID | Goal | Scope | Deps | Size | Issue |
|---|---|---|---|---|---|
| 0.4-S1 | Distributed step execution — worker design ADR | IPC choice (gRPC / NATS / direct TCP), worker discovery, failure detection; `docs/adr/002-distributed-workers.md` | — | S | — |
| 0.4-S2 | Distributed step execution — implementation | Worker binary `boruna-worker`; orchestrator dispatches; capability gateway runs in worker; evidence streamed back | 0.4-S1, 0.3-S3 | L | — |
| 0.4-S3 | Streaming output from `boruna_run` | MCP incremental tool result chunks; periodic progress event every 100k steps; `--stream` CLI flag | — | M | [#4](https://github.com/escapeboy/boruna/issues/4) |
| 0.4-S4 | Prometheus metrics endpoint | `boruna --metrics-addr :9090`; counter per cap, histogram for step duration, gauge for queue depth | — | M | — |
| 0.4-S5 | OpenTelemetry observability | OTLP spans per `CapabilityGateway::call`; env-var activation; `telemetry` Cargo feature | — | M | [#9](https://github.com/escapeboy/boruna/issues/9) |
| 0.4-S6 | Workflow dashboard — tech ADR | SSR (axum + askama) vs. SPA decision; `docs/adr/003-dashboard-stack.md` | — | S | — |
| 0.4-S7 | Workflow dashboard — backend | Read-only API: list runs, view run, view step, fetch evidence | 0.4-S6, 0.3-S3 | M | — |
| 0.4-S8 | Workflow dashboard — UI | Run list, run detail, step timeline, evidence viewer; no-JS first | 0.4-S7 | M | — |
| 0.4-S9 | Policy management — code-versioned policies | `policies/*.json` checked in; `boruna policy lint`; reference policies by name in workflow | — | M | — |
| 0.4-S10 | LLM provider registry | `providers.json` config; routing rules; failover; integrate with `0.3-S12` outcome | 0.3-S12 | M | — |
| 0.4-S11 | Multi-environment support | `--env dev/staging/prod`; per-env policy + provider overrides; documented promotion flow | 0.4-S9 | S | — |

**Critical path:** S1 → S2 = 2.5 weeks; S6 → S7 → S8 = 2.5 weeks (parallel to distributed).
**Parallelizable:** S3, S4, S5 are all independent observability work. S10, S11 chain off S9.

Recommended team size: **2–3 engineers** = ~6–8 weeks calendar.

---

## 0.5.0 — Spec freeze candidate

Q1 2027. No new capabilities — this is where the API surface stops moving. Whatever ships here is what 1.0 commits to forever.

| ID | Goal | Scope | Deps | Size | Issue |
|---|---|---|---|---|---|
| 0.5-S1 | `.ax` language specification | Formal grammar (EBNF), type rules (inference + checking), capability semantics; published as `docs/spec/ax-language-1.0.md`; `language_version` field on every module | — | L | — |
| 0.5-S2 | Workflow DAG schema versioning | JSON Schema for `workflow.json` with `schema_version`; backwards-compat parser tested with corpus of 0.2/0.3/0.4 workflows | — | M | — |
| 0.5-S3 | Evidence bundle format versioning | Schema for bundle directory contents; `format_version` field; forward-compat reader | — | M | — |
| 0.5-S4 | Stable MCP tool response schemas | `protocol_version: 1` on every tool response (compile, run, check, repair, validate, framework, workflow, template); document each in `docs/reference/mcp-tools.md` | — | M | [#6](https://github.com/escapeboy/boruna/issues/6) |
| 0.5-S5 | Migration tooling beta | `boruna migrate <from-version>` framework; one example migration (0.4→0.5); written so 1.0 can plug in trivially | 0.5-S1, 0.5-S2, 0.5-S3 | L | — |
| 0.5-S6 | Output JSON Schema validation gate | `boruna_run` `output_schema` parameter; `jsonschema` crate; `error_kind: "validation_failed"` | 0.5-S4 | M | [#8](https://github.com/escapeboy/boruna/issues/8) |
| 0.5-S7 | Record/replay for `net.fetch` | `RecordingHandler` wrapper around HTTP handler; sidecar JSON file format (locked under 0.5-S3 versioning); `--record-net-to` / `--replay-net-from` flags | 0.5-S3 | M | [#7](https://github.com/escapeboy/boruna/issues/7) |

**Critical path:** S1 → S5 = 4 weeks; S2/S3 in parallel.
**Parallelizable:** S1, S2, S3, S4 are largely independent (different files / surfaces).

Recommended team size: **2 engineers** = ~5 weeks calendar.

---

## 1.0.0 — Production readiness

Q2 2027. Mostly a *commitment* release. Engineering is small; the durability promise is large.

| ID | Goal | Scope | Deps | Size | Issue |
|---|---|---|---|---|---|
| 1.0-S0 | Security audit booking | **Must happen Q4 2026.** Pick auditor (e.g. Trail of Bits / Cure53 / NCC), define scope (VM + capability enforcement + SSRF protection + evidence chain), sign contract, schedule. | — | S (calendar work) | — |
| 1.0-S1 | Performance benchmarks + baseline | Compile time per LOC, step throughput, evidence bundle write/verify; criterion-based; published as `docs/performance.md` and re-run in CI | — | M | — |
| 1.0-S2 | Security audit execution + fix sprint | Auditor runs scope, files findings; team triages and fixes; re-test | 1.0-S0 | L (variable based on findings) | — |
| 1.0-S3 | Migration tooling stable | Graduate `boruna migrate` from 0.5 beta; cover every breaking change committed to between 0.5 and 1.0; CI test against snapshot corpus | 0.5-S5 | M | — |
| 1.0-S4 | Evidence bundle storage adapters | Pluggable `EvidenceStore` trait; ship S3 + filesystem (default) + null (testing); document writing your own | 0.3-S3 | M | — |
| 1.0-S5 | Evidence bundle encryption | `--encrypt-with <key-file>` flag; AES-GCM; key derivation documented; verify works on encrypted bundles | 1.0-S4 | M | — |
| 1.0-S6 | LTS process documentation | Backport policy, deprecation policy, EOL timeline; published as `docs/lts.md` | — | S | — |
| 1.0-S7 | 1.0 release | Final CHANGELOG curation, stability tier promotions, migration guide from 0.5, retire `[Unreleased]`, tag `v1.0.0` | All | S | — |

**Critical path:** S0 (Q4 2026 calendar work) → S2 (Q2 2027) → S7. Audit findings can blow up timeline by 2–4 weeks; budget for it.

**Parallelizable:** S1, S3, S4, S5, S6 are all independent of the audit and can run while audit is ongoing.

Recommended team size: **2 engineers + auditor + 1 product** = ~6 weeks calendar.

---

## Decisions blocking sprint starts

These four decisions from the roadmap each block a specific sprint. **None are urgent today, but each becomes urgent within 1–2 quarters.**

| Decision | Blocks | Must decide by |
|---|---|---|
| Persistence storage backend (sqlite vs. postgres vs. pluggable) | 0.3-S1 → S2 | Q2 2026 end |
| LLM live handler shipping plan (ship vs. BYO) | 0.3-S12, 0.4-S10, 0.2.x-S8 | Q3 2026 start |
| Dashboard tech (SSR vs. SPA) | 0.4-S6 → S7 → S8 | Q3 2026 end |
| Security auditor + scope + budget | 1.0-S0 → S2 | Q4 2026 (calendar binding) |

---

## Total picture

| Version | Sprints | Min calendar (2 engineers) | Quarter |
|---|---|---|---|
| 0.2.x | 8 | rolling, ~2 months part-time | May–Jul 2026 |
| 0.3.0 | 12 | ~7 weeks | Q3 2026 |
| 0.4.0 | 11 | ~6–8 weeks | Q4 2026 |
| 0.5.0 | 7 | ~5 weeks | Q1 2027 |
| 1.0.0 | 8 | ~6 weeks (+ audit calendar) | Q2 2027 |
| **Total** | **46** | **~7 months engineering + 6 months calendar** | **2026-Q3 → 2027-Q2** |

Numbers assume 2 engineers working in parallel where dependencies allow. With 1 engineer, multiply by ~1.7 (no parallel work but less coordination overhead).

---

## How to use this document

1. **Plan a quarter:** pick the version, look at the sprint table, sequence the critical path.
2. **Pick the next sprint to start:** find the first row whose `Deps` are all done.
3. **Spawn a `feat/` branch** named after the sprint ID (e.g. `feat/0.3-s1-persistence-adr`).
4. **Run `/sprint-orchestrate quick <sprint-id>`** for sprints sized S; `/sprint-orchestrate full <sprint-id>` for L sprints that warrant Think+Plan docs.
5. **At end of each version, update the roadmap status** and refresh this doc to reflect what actually shipped vs. plan.
