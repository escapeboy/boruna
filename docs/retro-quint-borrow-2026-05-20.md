# Sprint Retro — Quint borrow bundle (2026-05-20)

## Sprint shape

Triggered by `/sc:research https://quint.sh/` followed by `/sprint-orchestrate full
планирай и имплементирай всички 5 препоръки`. The user chose the **"Plan all 5, build
smallest bundle"** option from the scope-clarification prompt — meaning full Think+Plan
for every recommendation but only the lowest-risk ones built this session.

## What shipped

| Item | Status | Tests | LoC |
|---|---|---|---|
| Research report (`research_quint_borrowable_ideas_2026-05-20.md`) | shipped | n/a | 380 |
| Design doc — `boruna repl` | shipped (plan only) | n/a | ~150 |
| Design doc — `boruna simulate` | shipped (plan only) | n/a | ~150 |
| Design doc — `--witnesses` | shipped (plan only) | n/a | ~70 |
| Design doc — literate workflows | shipped | n/a | ~150 |
| Design doc — ITF traces | shipped | n/a | ~120 |
| Design doc — `debug` builtin | shipped (deferred build) | n/a | ~120 |
| Architecture docs (5) | shipped | n/a | ~700 total |
| Test plans for build bundle (3) | shipped | n/a | ~250 total |
| ITF v0.15 emit + `evidence inspect --itf` | shipped | 25 unit + 7 audit-adapter | ~570 |
| Literate workflow extract (`boruna literate extract`) | shipped | 16 unit + e2e | ~470 |
| `debug` / `debug_msg` builtin | **deferred** | n/a | n/a |

**Code totals:** ~1,040 lines of new Rust + tests across `tooling/src/{trace,literate}/`,
`crates/llmvm-cli/src/main.rs`, `tooling/Cargo.toml` (+ `pulldown-cmark` dep).
**Doc totals:** ~2,300 lines of new design / architecture / test-plan / research markdown.

## Gates status

- `cargo test --workspace --no-fail-fast`: **1262 passing**, 1 failing
  (`cli_watch::watch_reruns_on_file_change` — pre-existing macOS flake documented in
  memory `known-flaky-tests`).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.

## What worked

1. **Scope clarification before scope contamination.** The `/sprint-orchestrate full
   планирай и имплементирай всички 5` request was ambiguous about how much was actually
   build vs. plan. Using `AskUserQuestion` up-front cost one round-trip and saved an
   estimated 4× the implementation budget by reducing the build set from 5 features to 3,
   then 2.
2. **Inventory before design.** The `ctx_batch_execute` survey of tooling/src/lib.rs,
   trace2tests, evidence inspect, and the Value enum surfaced the bytecode-opcode-dispatch
   reality of `__builtin_*` BEFORE committing to a `debug` build. That's the kind of
   architecture-phase discovery the "plan all 5, build subset" workflow was designed to
   produce.
3. **Conventions §33 paid off.** The design docs explicitly referenced the schema-drift
   pattern; the resulting ITF code includes `ITF_FORMAT_VERSION = "0.15"` as a drift fence
   without pulling in a `jsonschema` runtime dependency.
4. **Idempotency-first literate semantics.** The deliberate divergence from Quint
   (truncate-on-first vs. pure append) was documented as Decision #5 in the design doc
   and validated by a dedicated unit test. Re-running extraction on the same input gives
   byte-identical output — exactly the property a compliance pipeline needs.

## What we'd do differently

1. **Architecture-phase verification of opcode dispatch.** The `debug` architecture doc
   claimed a "string-dispatched intrinsic table at runtime" mechanism that does not exist
   in Boruna. The dispatch is at COMPILE time (each builtin name lowers to a dedicated
   opcode). A 10-line `grep` would have caught this before the architecture doc was
   accepted. Action item: when an architecture doc claims a runtime mechanism, the build
   phase MUST spot-check the claim with `grep -rn '<mechanism>' crates/`.
2. **Boruna parser quirks deserve a smoke-test fixture.** The literate end-to-end test
   tripped over `let _ = greeting();` and `let g = greeting();` patterns the parser
   doesn't fully accept yet. The fixture had to be reduced to `fn main() -> Int { 0 }`
   for the round-trip test to pass. The literate extractor itself was correct; the
   Boruna lang surface has rough edges for ergonomic test fixtures.
3. **Test command default fails-fast.** `cargo test --workspace` (without `--no-fail-fast`)
   stopped after the known pre-existing `cli_watch` flake, hiding ~1180 other passing
   suites. Should standardize on `--no-fail-fast` in `scripts/ci.sh` so flake-in-one-suite
   never masks the rest. Filed as small followup.

## Items to follow up

1. **`debug` builtin sprint** — bytecode evolution from 1.0 → 1.1. Scope:
   - 2 new opcodes in `crates/llmbc/src/opcode.rs`
   - VM dispatch arms in `crates/llmvm/src/vm.rs`
   - Codegen arms in `crates/llmc/src/codegen.rs`
   - §4 entries in `docs/spec/bytecode-1.0.md` (likely split as `bytecode-1.1.md`)
   - `### Decided` CHANGELOG entry referencing this retro
   - Drift test that a `1.0` reader rejects a `1.1` opcode with the right typed error
2. **`boruna repl` spike** — 1-2 day timeboxed validation that the VM's
   `execute_bounded` primitive supports stateful between-thunk evaluation without
   stack/heap-state invariants breaking. If yes: real sprint. If no: rethink.
3. **`boruna simulate` and `--witnesses` together** — single sprint, witnesses are
   100 LoC marginal cost over simulate. Architecture doc already nails the per-violation
   evidence-bundle convention (`simulator: true` manifest flag, separate `--out-dir`).
4. **CI `--no-fail-fast` adoption** — small followup PR; see §3 above.
5. **Literate fixtures in `examples/compliance/`** — convert one of the three v1.4.0
   compliance workflows (likely `soc2_audit_workflow`) to its literate form as a worked
   example. Adds the "auditor-facing single document" demonstration the design doc was
   pitched around.

## Memory updates

Project memories that should reflect this sprint (added in this commit's session):

- `release-pipeline` — note that v1.5.0 will carry the literate + ITF features as
  headline additions; the deferred items move to v1.6.0+ scoping.
- (no new project conventions surfaced — the existing 41 conventions covered every
  decision needed.)
