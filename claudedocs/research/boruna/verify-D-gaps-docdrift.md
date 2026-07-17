# Verify-D — Gaps & Doc-Drift Ground-Truth Pass (READ-ONLY)

Repo: `/Users/katsarov/htdocs/ai-lang` @ branch `ci/reduce-artifact-storage`.
Method: counted/read actual source, not prior docs. Every row proven by a `path:line` or a command.

## Ground-Truth Table

| # | Claim (source) | ACTUAL value (proof) | Verdict |
|---|----------------|----------------------|---------|
| 1 | README.md:161 "Boruna is at **v1.4.0**" | `Cargo.toml:19` → `version = "1.9.0"`. README:37 also says "12 tools"; CHANGELOG top entry is `## [1.4.0]`. Latest release commit = `d01ad8e chore(release): v1.5.0`. Workspace is **5 minors ahead** of the README/CHANGELOG. | **CONFIRMED-DRIFT** (README+CHANGELOG stale; real = 1.9.0) |
| 2 | README.md:34 "**27 built-in functions**" | Distinct `__builtin_*` symbols in `crates/` = **33** (`grep -rhoE '__builtin_[a-z0-9_]+' \| sort -u \| wc -l`). Breakdown: string 12, list 7, map 7 (= 26 in the `string/list/map` families README enumerates) + 5 conversion (`bool_to_string`, `int_parse`, `int_to_string`, `float_parse`, `float_to_string`) + 2 debug (`debug`, `debug_msg`). Registered in `crates/llmc/src/codegen.rs:309+`. 27 matches neither 26 nor 33. | **CONFIRMED-DRIFT** (real = 33 distinct; 26 in the enumerated families) |
| 3 | README.md:37 "exposes **12 tools**" | `crates/boruna-mcp/src/server.rs` — 12 `async fn boruna_*` tool handlers: `compile`(172), `ast`(189), `run`(206), `check`(321), `repair`(338), `validate_app`(360), `framework_test`(375), `workflow_validate`(394), `template_list`(411), `template_apply`(422), `capability_list`(448), `policy_validate`(460). Count = **12**. | **NO-DRIFT** (README correct) |
| 3b | CLAUDE.md:110 (+ tool table) "Exposes **10 tools**" | Actual = **12** (see 3). Missing from CLAUDE.md: `boruna_capability_list`, `boruna_policy_validate`. | **CONFIRMED-DRIFT** (CLAUDE.md stale at 10) |
| 4 | "**13 stdlib packages**"; CLAUDE.md: std-llm & std-json are "0.1.0, Experimental" | `libs/` = **13** dirs. All 13 `package.ax.json` → `"version": "1.0.0"` (incl. `std-llm` and `std-json`). CHANGELOG [1.3.0]:92-93 marks both 1.0-stable. | 13 count **NO-DRIFT**; std-llm/std-json 0.1.0-experimental claim **CONFIRMED-DRIFT** (both are 1.0.0 — the gaps agent was right, CLAUDE.md wrong) |
| 5 | CLAUDE.md:15 "**557+ tests across 9 crates**" | `#[test]`/`#[tokio::test]` annotations = **1613** (grep over crates/orchestrator/tooling/packages). Workspace members = **12** (`Cargo.toml:3-16`): 11 crates + `benches`. "557+" is literally true but ~3× understated; "9 crates" is stale (missing `boruna-mcp`, `boruna-lsp`). | **CONFIRMED-DRIFT** (real ≈ 1613 test fns; 11 crates / 12 members, not 9) |
| 6 | docs/stability.md: "all 13 std-* are 1.0-stable" under an **Experimental** heading | `stability.md:42` = `### Experimental (may change in minor versions)`; `:49` = "**Standard libraries** — all 13 `std-*` packages are 1.0-stable as of v1.3.0". Self-contradiction is REAL: "1.0-stable" bullet sits inside the Experimental section. Graduation version agrees at v1.3.0 (`:13`, `:49`, CHANGELOG [1.3.0]) — no v1.2/v1.3 disagreement for llm/json (those two graduated in 1.3.0; the other 11 in 1.2.0). | **CONFIRMED-DRIFT** (heading/content contradiction) / version-graduation = NO-DRIFT |
| 7 | std-forms pins `std.validation` "0.1.0" while std-validation is 1.0.0 | `libs/std-forms/package.ax.json` → `"std.validation": "0.1.0"`; `libs/std-validation/package.ax.json` → `"version": "1.0.0"`. std-forms is the ONLY lib with a dependency, and the pin is unsatisfiable against the real 1.0.0. | **CONFIRMED-DRIFT** (stale/unsatisfiable pin) |
| 8 | "Code is clean" (no stubs) | `todo!(` = **0**, `unimplemented!(` = **0** across all shipping dirs. `unreachable!(` = **10**, `panic!(` = 154 (mostly test assertions). See breakdown below. | **HOLDS** (no stub/gap markers) — with nuance below |

## Item 8 detail — the "clean" claim

- **`todo!()` = 0, `unimplemented!()` = 0** — confirmed zero everywhere (crates/, orchestrator/src, tooling/src, packages/src). No feature-stub markers exist. This is the core of the "clean" claim and it **holds**.
- **`unreachable!()` = 10, all in shipping code but all idiomatic exhaustiveness guards** (not gaps):
  - `crates/llmc/src/parser.rs:750,757,764,988,995,1016` (6× — exhaustive token-match arms)
  - `orchestrator/src/simulate/witness.rs:144`
  - `orchestrator/src/workflow/runner.rs:2419`, `:3867` (`"persist_one_pause called with non-pause StepKind"`), `:10869`
- **`panic!()` = 154** — the overwhelming majority are test assertions (`panic!("wrong variant: {e:?}")` in `policy_validate.rs`, `panic!("{other:?}")` in `coordinator.rs`, etc.). Genuinely non-test occurrences are dev/self-test helpers, NOT runtime feature gaps:
  - `crates/boruna-mcp/src/tools/run.rs:938` — `panic!("example {} failed to parse")` (example-validation self-check)
  - `crates/boruna-mcp/src/tools/mod.rs:36` — `panic!("response was not valid JSON")` (tool-response test helper)
  - `crates/llmvm/src/policy_validate.rs:730-731` — panic on reading/parsing the bundled JSON schema fixture (dev/test loader)
  - `orchestrator/src/simulate/invariant.rs:655` — `.unwrap_or_else(|e| panic!(...))` inside a test
  - (runner.rs:2334 & :2599 are *comments* mentioning `panic!`, not calls)

**Verdict on "clean": HOLDS.** Zero `todo!`/`unimplemented!`; all `unreachable!`/`panic!` are either test assertions or defensive/idiomatic guards. The gaps agent did not miss a hidden stub — but note the claim is "no stub markers," not "no panics at all."

## Cross-check: who was right, gaps agent vs CLAUDE.md
- std-llm/std-json version: **gaps agent right (1.0.0)**, CLAUDE.md wrong (0.1.0-experimental).
- All numeric drift is in README/CLAUDE.md/CHANGELOG lagging behind a workspace that has moved to 1.9.0.
