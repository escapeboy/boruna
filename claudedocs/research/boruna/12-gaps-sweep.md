# Boruna — Cross-Cutting Gaps & Completeness Sweep

**Scope:** whole repo (crates/, libs/, orchestrator/, tooling/, packages/, examples/, docs/). Read-only. All claims cite `path:line`.
**Date:** 2026-07-16 · **Branch:** ci/reduce-artifact-storage · **Workspace version:** 1.9.0

## Executive Summary

The **code is in good shape** — markers are overwhelmingly cosmetic (compiler jump-offset placeholders, mock test handlers, template-substitution `{{placeholder}}` machinery). There are **no `todo!()`/`unimplemented!()` in shipping code paths** and only 6 `#[ignore]` tests (all external-service blob integration).

The real gaps are **documentation drift**, and it is significant enough to mislead a user or evaluator:

- **README.md still says "Boruna is at v1.4.0"** (`README.md:161`) while the workspace is **1.9.0** (`Cargo.toml:5`) and CHANGELOG's top entry is `[1.9.0]` (`CHANGELOG.md:9`). README is **5 minor releases stale** and describes 1.4.0 features as "new."
- **CLAUDE.md is materially stale**: "557+ tests across 9 crates" (`CLAUDE.md:15`), "Exposes 10 tools" (`CLAUDE.md:110`), and "std-llm and std-json (0.1.0, Experimental)" (`CLAUDE.md:126`) — actual: 11 workspace crates, **12 MCP tools**, and std-llm/std-json are now **1.0.0**.
- **README undercounts builtins**: "27 built-in functions" (`README.md:34`) vs **33** distinct `__builtin_*` symbols in code.
- **docs/stability.md self-contradiction**: lists "all 13 std-* packages are 1.0-stable" *under* the **"Experimental (may change in minor versions)"** heading (`docs/stability.md:42,50`), and dates graduation to "v1.3.0" while CLAUDE.md says "v1.2.0."

Individually verified numbers where README is now **correct**: 12 MCP tools (`README.md:37`), 13 stdlib packages (`README.md:35`).

---

## DOC ↔ CODE DRIFT (highest value)

| Claim | Source file:line | Actual | Verdict |
|---|---|---|---|
| "Boruna is at **v1.4.0** — fourth minor release" | README.md:161 | workspace `version = "1.9.0"`; CHANGELOG top `[1.9.0]` | **DRIFT — 5 versions stale**, describes 1.4.0 features as new |
| "**1175+ tests** across 11 workspace members" | README.md:132 | 1613 `#[test]`/`#[tokio::test]` annotations; 12 members (incl. benches) | STALE (undercount; `+` keeps it technically true) |
| "**27 built-in functions**" | README.md:34 | 33 distinct `__builtin_*` symbols | **DRIFT — undercount** |
| "all 13 stdlib packages are 1.0-stable" | README.md:35 | 13 libs, all `"version":"1.0.0"` in package.ax.json | MATCHES |
| "exposes **12 tools** for AI coding agent integration" | README.md:37 | 12 `#[tool]` methods in server.rs | MATCHES |
| "Run all tests (**557+ tests across 9 crates**)" | CLAUDE.md:15 | 1613 test annotations; 11 crates (+benches) | **DRIFT — very stale** |
| "Exposes **10 tools** over JSON-RPC" | CLAUDE.md:110 | 12 tools (adds `boruna_capability_list`, `boruna_policy_validate`) | **DRIFT** |
| MCP tool table lists **10 tools** | CLAUDE.md:113-124 | 12 tools registered (server.rs:172-460) | **DRIFT — missing 2** |
| "std-llm and std-json (**0.1.0, Experimental**)" | CLAUDE.md:126 | both `"version":"1.0.0"` (libs/std-llm, libs/std-json package.ax.json) | **DRIFT — graduated** |
| "std-* … 1.0-stable **as of v1.2.0**" | CLAUDE.md:126 | stability.md:50 says "as of v1.3.0" | INCONSISTENT (which minor graduated them) |
| "all 13 std-* packages are 1.0-stable" listed **under "Experimental (may change)"** header | docs/stability.md:42 + :50 | contradictory framing — stable content under Experimental tier | **DOC SELF-CONTRADICTION** |
| stability.md "Actor system", "Multi-agent orchestration", "Package system" = Experimental | docs/stability.md:44-47 | matches memory notes (actor system wired but Experimental) | plausible; not code-verified this pass ("not verified") |

**Note on test count:** 1613 is the count of `#[test]`/`#[tokio::test]` *attributes* across crates/orchestrator/tooling/packages (grep). Actual executed/passing count not run this pass ("not verified" — this is an annotation count, an upper-ish proxy, not a `cargo test` tally).

---

## MARKER TABLE

| file:line | marker | missing feature? | severity |
|---|---|---|---|
| crates/llmc/src/codegen.rs:196,550,556,592,598 | `// placeholder` | **Cosmetic** — jump-offset backpatching (`Op::Jmp(0)` then patched). Normal codegen. | none |
| tooling/src/diagnostics/suggest.rs:317,322,337 | `/* TODO */` in generated text | **Intended** — repair tool emits stub match arms *for the user's code*. Feature, not gap. | none |
| crates/llm-effect/src/gateway.rs:196-260 | `generate_mock_response`, `mock_patch_bundle`, `mock_json_object` | **By design** — deterministic mock LLM backend for record/replay + tests. Real handler is BYOH (examples/llm_handlers). | none |
| crates/llmvm/src/capability_gateway.rs:156-192 | "Default handler that returns mock values (for testing / sandbox)" | **By design** — mock is the default; `--live` / BYOH provides real effects. | low (see stub list) |
| crates/llmfw/src/executor.rs:16-101 | "Mock executor … returns deterministic stub results" | **By design** — test/framework executor. | none |
| crates/llmvm-cli/src/main.rs:2864; orchestrator/src/workflow/runner.rs:3137 | "warning: --live requires the `http` feature; falling back to mock handler" | **Intended fallback** — real behavior gated behind `http` feature (Alpha per stability.md). | low |
| crates/llmvm-cli/src/main.rs:2826-2829 | "Write an empty placeholder tape" | **Intended** — net record/replay bootstrap. | none |
| orchestrator/src/workflow/runner.rs:1239,1885,3611,3652,4175,9440 | pause-time / trigger "placeholder" `TriggerRecord` (empty payload) | **Intended** — sentinel-value protocol to discriminate paused vs triggered steps. Documented inline. | none |
| orchestrator/src/workflow/runner.rs:895 | "distributed mode does not yet support those step kinds" | **Real limitation** — some StepKinds unsupported under distributed execution. | **medium** |
| orchestrator/tests/s3_integration.rs:21 | "for now the unit tests …" (no real S3 round-trip gating release) | **Real test gap** — S3 path not covered by a live round-trip in CI. | **medium** |
| crates/llmc/src/parser.rs:750,757,764,988,995,1016 | `unreachable!()` | Parser invariants after token-kind checks. Not a gap. | none |
| orchestrator/src/workflow/runner.rs:2419,3867,10869; simulate/witness.rs:144 | `unreachable!()` | Exhaustiveness guards; `persist_one_pause` documents its invariant. | none |
| crates/llmvm-cli/src/serve.rs:274-275 | HTML `placeholder=` attrs | Cosmetic — form UI. | none |

**No occurrences** of `todo!()`, `unimplemented!()`, `panic!("not …")`, `HACK`, `XXX`, "coming soon", or "WIP" in shipping Rust code paths.

---

## STUB / DEMO LIST

| Item | Location | Status |
|---|---|---|
| Mock LLM gateway (deterministic canned responses) | crates/llm-effect/src/gateway.rs:196-260 | Intentional test/replay backend. Real = BYOH handler. |
| Default capability handler returns mock net/fs/llm values | crates/llmvm/src/capability_gateway.rs:156-192 | Intentional sandbox default; `--live` + `http` feature or custom handler for real effects. |
| MockExecutor (framework) | crates/llmfw/src/executor.rs:16-101 | Test harness executor; returns `"mock_result"`. |
| `examples/llm_handlers/{anthropic,openai,bedrock,ollama,vllm}` | examples/llm_handlers/ | **Reference skeletons, explicitly "not a production handler"** (openai/README.md:5, anthropic/README.md:5, bedrock/README.md:5). `router_setup.rs:3` "This is a SNIPPET, not a compiled module." Honest labeling — not misleading. |
| `orchestrator/src/adapters/mod.rs:365-413` `llm_mock_verify` gate | adapters/mod.rs | Verifies mock mode works; real backend via `LLM_BACKEND` env. |
| `#[allow(dead_code)]` sites (11) | boruna-mcp/server.rs:153; llmvm-cli/{worker,provider_registry,coordinator,scaffold}.rs; orchestrator/audit/storage.rs:314; runner.rs:41,61 (persist-sqlite-gated) | Mostly intentional: scaffold field-inspection, feature-gated persistence fields. Low concern; none indicate abandoned features. |
| `simulate/mod.rs:291` `placeholder_def()` | orchestrator/src/simulate/mod.rs | Test-only workflow fixture. |

---

## TEST-GAP LIST

| Subsystem | Gap | Severity |
|---|---|---|
| Blob storage integration (S3/Azure/GCS) | 6 `#[ignore]` tests in orchestrator/tests/blob_integration_tests.rs (:76,121,194,231,320,357) — require live cloud creds, not run in CI. | medium — cloud-store round-trips unverified in CI |
| S3 release gating | orchestrator/tests/s3_integration.rs:21 — "for now the unit tests" gate release, no real round-trip. | medium |
| `--live` HTTP handler | Alpha tier (stability.md:54-58); only compiles under `http` feature — not in default CI matrix (CI runs `--features boruna-cli/serve`, per README:132). Real network path exercised only when feature explicitly enabled. | medium |
| Distributed execution step-kind coverage | runner.rs:895 notes unsupported step kinds in distributed mode — coverage of the gap itself not asserted. | low-medium |

Overall test density is high (1613 annotations); gaps are concentrated in **external-service / feature-gated** paths, which is a reasonable place for them but should be stated in docs.

---

## HALF-MIGRATED LIST

| Migration | State | Evidence |
|---|---|---|
| `llmbc/llmc/llmvm/llmfw/llm-effect` → `boruna-*` crate rename | **Intentional split, ongoing**: crate *names* are `boruna-*` but *directories* keep original names. Documented as deliberate (CLAUDE.md Architecture note, MEMORY.md mapping). Not a defect, but a permanent source of confusion for new readers. | Cargo.toml:members lists `crates/llmbc`, `crates/llmvm`, … alongside `crates/boruna-mcp`, `crates/boruna-lsp` |
| Bytecode 1.0 → 1.1 | Shipped (git log `fc8ca9b` "debug builtins (bytecode 1.1)"). Not deep-verified for lingering 1.0-only paths this pass. | "not verified" beyond git log |
| stdlib graduation 0.1.0 → 1.0.0 (std-llm, std-json) | **Code done, docs lagging**: manifests are 1.0.0 but CLAUDE.md:126 still says "0.1.0, Experimental". | libs/std-llm/package.ax.json, libs/std-json/package.ax.json vs CLAUDE.md:126 |
| MCP tool set 10 → 12 | **Code done, CLAUDE.md lagging**: `boruna_capability_list` (server.rs:448) + `boruna_policy_validate` (server.rs:460) added; CLAUDE.md table still 10. README already updated to 12. | server.rs:448,460 vs CLAUDE.md:110-124 |

---

## Top 5 Gaps (evaluator-facing)

1. **README.md says "v1.4.0" — workspace is 1.9.0** (README.md:161 vs Cargo.toml:5 / CHANGELOG.md:9). Anyone reading the front-door README sees a version 5 minors stale, with 1.4.0 features labeled "new." Highest-impact drift.
2. **CLAUDE.md is broadly stale**: "557+ tests / 9 crates" (:15), "10 MCP tools" (:110, table :113-124), "std-llm/std-json 0.1.0 Experimental" (:126). Actual: 11 crates, 12 tools, both libs 1.0.0. Misleads AI agents that load CLAUDE.md as ground truth.
3. **README undercounts builtins: "27" vs 33** actual `__builtin_*` symbols (README.md:34). Evaluator counting features will find more than advertised (benign direction, still drift).
4. **docs/stability.md contradiction**: "all 13 std-* packages are 1.0-stable" placed under the **"Experimental (may change in minor versions)"** heading (:42/:50); graduation version disagrees with CLAUDE.md (v1.3.0 vs v1.2.0). A reader can't tell whether stdlib is stable or experimental.
5. **Feature-gated / external-service test gaps**: `--live` HTTP (Alpha), S3/Azure/GCS blob round-trips (6 `#[ignore]`, s3_integration.rs:21), and distributed-mode step-kind coverage (runner.rs:895) are not exercised in default CI — real but appropriately-scoped.

**Coverage line:** Marker sweep + doc-drift verification complete across all crates/libs/docs; every numeric claim in README/CLAUDE.md cross-checked against code (versions, test count, MCP tools, builtins, stdlib stability). Code-level markers are ~95% cosmetic/intentional; the substantive gaps are documentation drift (README + CLAUDE.md 5 minors stale) plus feature-gated external-service test coverage. Not verified this pass: actual `cargo test` pass count (annotation-count proxy used) and bytecode 1.0-only residual paths.
