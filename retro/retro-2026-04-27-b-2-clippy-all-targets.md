# Retro — Sprint B-2 Clippy `--all-targets` Sweep (2026-04-27)

**Sprint:** B-2 — debt cleanup, no feature
**Branch:** sprint/b2-clippy-all-targets-sweep (merged + deleted)
**Commit:** `dc93046`
**Master state:** 8 commits ahead of origin; not pushed.

## Goal

Clear pre-existing `cargo clippy --all-targets -- -D warnings`
violations across the workspace so that pushing master to origin
does not red-CI the release pipeline. The lints had been emitted
locally by rustc 1.91 since at least early-Apr 2026 but never
swept; the project's standard CI gate runs `cargo clippy
--workspace -- -D warnings` (without `--all-targets`), so test-
code lints had been invisible to CI but were active on the
runner per `release-pipeline` memory.

## What shipped

10 files touched, 35 insertions / 32 deletions. Net code change
small; net lint surface zeroed.

| Lint | Crate(s) | Sites | Fix |
|------|----------|-------|-----|
| `needless_borrows_for_generic_args` | packages/tests, tooling | 2 | auto-fix |
| `manual_contains` | llmbc/tests | 1 | auto-fix → `Capability::ALL.contains(&cap)` |
| `clone_on_copy` | llmvm/capability_gateway | 1 | auto-fix → `*cap` |
| `for_kv_map` | orchestrator/workflow/runner | 1 | auto-fix |
| `manual_is_multiple_of` | llmvm-cli/main | 1 | auto-fix → `.is_multiple_of(2)` |
| `module_inception` | llmbc, llmc, llmfw, llmvm tests | 4 | `#[allow]` on inner mod |
| `type_complexity` | llmvm/capability_gateway test handler | 3 | `type RecordedCalls = ...` alias |
| `approx_constant` | tooling/trace2tests | 1 | replaced 3.14 with 2.5 in roundtrip fixture |
| `await_holding_lock` | llmvm-cli/coordinator (test) | 1 | wrapped lock in block scope so drop is automatic before await |

## What worked

### 1. `cargo clippy --fix` removed most of the work

`cargo clippy --workspace --features boruna-cli/serve --all-targets
--fix --allow-dirty` cleaned 6 of the 13 lints automatically. The
remainders were either structural (module_inception, type_complexity)
or had non-obvious value semantics (approx_constant on `3.14`
specifically).

**Lesson reinforced:** for any `-D warnings` debt sweep, run
`--fix` first and only handle the residue manually. Saved
~15 minutes versus editing each site by hand.

### 2. The `await_holding_lock` lint caught a real footgun shape

The pre-existing test had `let store = ...lock(); ...; drop(store);
.await`. Compiled cleanly, ran correctly. But the binding scope
spans the await, and clippy's analysis is conservative — it
doesn't track explicit `drop()` calls. The recommended pattern
(block-scope the lock) is also more robust because future code
edits can't accidentally extend the lock past the drop without
the lint complaining.

This isn't a sprint output but worth recording: even though
`drop(x)` is semantically equivalent to letting x leave scope, the
**block-scope pattern is more lint-friendly AND more
edit-resistant**. The block visually delineates the critical
section.

### 3. Resisted the urge to refactor the inner mods

Was tempted to flatten the `mod tests { ... }` blocks (move
contents to top of `tests.rs`). Resisted because:
- It's a stylistic lint, not a correctness issue.
- Flattening 4 files would touch hundreds of lines + risk
  test breakage.
- The pattern matches cargo's recommended `tests.rs` template.
- An `#[allow]` is one line per file and explicit about intent.

Per project convention §1 (reject at parse, don't silently
override) — analogously, here: take the explicit-allow path,
don't silently restructure code to satisfy a stylistic linter.

## What I'd do differently

### 1. This should have been a quick-pipeline sprint

The /sprint-orchestrate skill has `quick` mode (Build → Review →
Test → Ship, no Think/Plan). Mechanical lint sweeps are exactly
what `quick` was designed for. I went through the full lifecycle
out of habit; the Think/Plan/Reflect overhead probably exceeded
the actual debt-clearing time.

**Followup:** when the user says "next sprint" for a debt-cleanup
task, default to `quick` shape unless the task crosses architectural
surface.

### 2. Should sweep `--all-targets` clippy in CI, not just `--workspace`

The project's CI gate (`.github/workflows/ci.yml`) runs `cargo
clippy --workspace -- -D warnings` — without `--all-targets`. So
test-code lints have been invisible to CI for months. After
this sweep, master should pass with `--all-targets` too.

**Followup:** consider promoting the CI gate to `--all-targets`
in a separate small PR. Filed as new debt: "CI clippy gate uses
`--all-targets` so test-code lint regressions are caught at PR
time, not at release-runner time."

## Metrics

| | Pre-B-2 | Post-B-2 |
|---|---|---|
| Workspace tests passing | 100% | 100% |
| Lib clippy clean | yes | yes |
| `--all-targets` clippy clean | NO (13 errors) | yes |
| fmt clean | yes | yes |
| Lines added | — | 35 |
| Lines removed | — | 32 |
| Files touched | — | 10 |

## Carried-forward debts

**Resolved this sprint:**
- ~~`--all-targets` clippy sweep~~ → done.

**New debts introduced:**
- Promote CI clippy gate to `--all-targets` so test-code lint
  regressions surface at PR time, not at release-runner time.
  Tiny PR; worth ~5 lines in `.github/workflows/ci.yml`.

**Standing post-B-2:**
- `StepCheckpoint` builder/constructor refactor (S7 debt).
- Blob GC sweep (S7 accepted limitation).
- Evidence bundle sidecar layout (S7 future evolution).
- `boruna fmt` v2 (comment-preserving).
- mTLS / per-worker keys / OAuth (0.6.x).
- HTTP-based remote `coordinator wait`.
- `boruna policy lint` warnings layer (deferred 0.4-S15).
- Audit lifecycle opt-out flag (deferred 0.4-S11).
- Dashboard pagination + auth + CSP (full sprint).
- Coordinator HA / failover (0.6.x).

**Spec freeze (0.5.0 ship gating):**
- Versioned `.ax` language specification.
- Versioned workflow DAG schema.
- Versioned evidence bundle format.
- Migration tooling beta.

## Recommended next session

**Cut `v0.5.0` tag.** All gates green workspace-wide AND on
`--all-targets`. Distributed-execution surface feature-complete.
Read paths consistent across resume / bundle / dashboard /
step_input. Spec-freeze items can land in 0.5.x patches as
additive versioned schemas.

If you want one more small patch before tagging, promote the
CI clippy gate to `--all-targets` (5-line `.github/workflows/ci.yml`
change). That ensures the sweep stays clean as future PRs land.

Master is now 8 commits ahead of origin. User has not pushed yet.
