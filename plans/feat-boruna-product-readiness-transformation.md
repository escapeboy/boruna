# feat: Boruna Product Readiness Transformation

> **Status**: Draft — 2026-03-19
> **Scope**: Repository-wide product, documentation, and positioning transformation
> **Effort**: Large (5–7 focused sessions)
> **Goal**: Make Boruna feel like the early version of a real product, not a research prototype

---

## Overview

Boruna is technically solid: a deterministic execution platform with a custom bytecode VM, policy-gated capability system, hash-chained audit logs, and a working workflow engine. The core is genuinely differentiated.

The problem is the gap between technical substance and how the project presents itself. Critical trust signals are missing (no LICENSE, no CHANGELOG, no CONTRIBUTING, no SECURITY). Internal development artifacts are committed publicly. The workflow examples are hollow stubs. The docs are a flat list of 33 files mixing public and internal content. The positioning has competing taglines across documents.

This plan closes that gap. Every action is anchored to adoption readiness — what a technical architect, enterprise buyer, or first-time contributor needs to see to go from "interesting project" to "I trust this enough to evaluate seriously."

---

## Problem Statement

### What a first-time visitor sees today

A developer who discovers Boruna on GitHub and spends 10 minutes evaluating it will encounter:

1. **A README that buries its own value.** The positioning line is correct but the "For LLMs and Coding Agents" section (40 lines) dominates the lower half, signaling internal tooling rather than user value.
2. **No LICENSE file.** MIT is claimed on line 216 but there is no `LICENSE` file. For enterprise legal teams, this is a hard stop.
3. **Hollow workflow examples.** Running `boruna workflow run examples/workflows/llm_code_review` "succeeds" — but all step `.ax` files return `fn main() -> Int { 42 }`. The workflow does nothing. This actively misleads evaluators.
4. **33 internal docs at one flat depth.** Files like `RENAME_AUDIT.md`, `MILESTONE_DOGFIX_PLAN.md`, `IDE_GAPS.md` (which references a non-existent `boruna-ide` repo) are publicly committed alongside legitimate docs, creating noise and confusion.
5. **No standard community files.** No `CHANGELOG.md`, `CONTRIBUTING.md`, `SECURITY.md`, or `CODE_OF_CONDUCT.md`. These are enterprise adoption prerequisites.
6. **Competing taglines.** README uses "enterprise AI workflows", BRANDING.md uses "Agent-Native Execution Platform", language-guide uses "LLM-native applications." These describe different products.
7. **Naming inconsistency.** Directory names (`crates/llmc/`, `crates/llmvm/`) bear no relation to crate names (`boruna-compiler`, `boruna-vm`). The bytecode magic bytes are `"LLMB"` — a rename artifact baked into the binary format.

### Why it matters

The technical work is real. The governance, audit, and determinism guarantees are genuine differentiators. But first impressions block adoption before those differentiators can be evaluated. An engineering team doing technical due diligence on a potential dependency will stop at the missing LICENSE. An enterprise architect evaluating Boruna for a compliance workflow will find the hollow examples and lose confidence.

---

## Proposed Solution

Transform the repository in six sequential phases, each with a clear done-criteria. The phases are ordered to address the hardest blockers first and build on each completed phase.

**Phase 1 — Trust Foundation**: The legal and community files that block enterprise evaluation.
**Phase 2 — Repo Hygiene**: Remove committed artifacts, hide internal docs, fix naming artifacts.
**Phase 3 — Workflow Examples**: Make the three reference workflows demonstrate real behavior.
**Phase 4 — Docs Restructure**: Reorganize 33 flat files into a navigable product documentation system.
**Phase 5 — README & Positioning Rewrite**: Rewrite the README as a portal, establish a single tagline.
**Phase 6 — Onboarding & Maturity Signals**: Quickstart, stability tiers, roadmap, commercial path.

---

## Technical Approach

### Phase 1 — Trust Foundation

**What to create:**

#### `LICENSE`
```
MIT License

Copyright (c) 2026 Boruna Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy...
[standard MIT text]
```

#### `CHANGELOG.md` (root)
Format: keep-a-changelog.com v1.1.0 spec. Structure:
```markdown
# Changelog

All notable changes to Boruna are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-02-21

### Added
- Deterministic workflow execution engine with DAG validation and topological ordering
- Hash-chained audit logs (SHA-256) and self-contained evidence bundles for compliance
- Policy-gated capability system (10 capabilities: net.fetch, db.query, fs.read, fs.write,
  time.now, random, ui.render, llm.call, actor.spawn, actor.send)
- Replay engine for determinism verification via EventLog comparison
- Three reference workflow examples: linear (llm_code_review), fan-out (document_processing),
  approval gate (customer_support_triage)
- MCP server exposing 10 tools over JSON-RPC stdio for AI coding agent integration
- Actor system with OneForOne supervision and bounded execution scheduling
- boruna-tooling: diagnostics, auto-repair, trace-to-tests, stdlib test runner, templates
- boruna-pkg: deterministic package system with SHA-256 content hashing and lockfiles
- Real HTTP handler (feature-gated, SSRF-protected) for net.fetch capability
- CLI subcommands: compile, run, trace, replay, inspect, ast, workflow, evidence,
  framework, lang, trace2tests, template
- Standard library: 11 deterministic libraries (std-ui, std-forms, std-authz, std-http,
  std-db, std-sync, std-validation, std-routing, std-storage, std-notifications, std-testing)

[Unreleased]: https://github.com/escapeboy/boruna/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/escapeboy/boruna/releases/tag/v0.1.0
```

#### `SECURITY.md` (root)
```markdown
# Security Policy

## Supported Versions

Only the current release receives security patches.

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✓         |

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Report using [GitHub Security Advisories](https://github.com/escapeboy/boruna/security/advisories/new)
or email `security@boruna.dev` if a public advisory is not appropriate.

Include: description, reproduction steps, potential impact, affected versions.

## Response Timeline

- Acknowledgment: within 48 hours
- Initial triage: within 5 business days
- Status updates: every 7 days until resolved
- Target resolution: within 90 days for critical issues

## Scope

In scope: boruna-vm (capability gateway, replay engine), boruna-compiler, boruna-orchestrator
(workflow runner, evidence bundle verification), boruna-mcp server.

Out of scope: example files, documentation, third-party dependencies.

## Safe Harbor

Good-faith security research conducted in accordance with this policy constitutes
authorized testing. We will not pursue legal action for responsible disclosure.
```

#### `CONTRIBUTING.md` (root)
Sections: welcome → code of conduct → what to work on → prerequisites → dev workflow → testing → PR checklist → changelog requirement → license.

Key content:
- Prerequisites: Rust stable toolchain (edition 2021), `cargo test --workspace`
- Build: `cargo build --workspace`
- Test: `cargo test --workspace` (557+ tests, all must pass)
- Lint: `cargo clippy --workspace -- -D warnings` (zero warnings)
- Format: `cargo fmt --all` (must be clean)
- Critical invariant: never use `HashMap` where `BTreeMap` is needed (determinism)
- Issue labels: `good-first-issue` (bug fixes, doc improvements), `help-wanted` (new features)
- PR checklist: tests pass, clippy clean, fmt clean, CHANGELOG entry under `[Unreleased]`

#### `CODE_OF_CONDUCT.md` (root)
Reference the Contributor Covenant v2.1. One file, no customization needed.

---

### Phase 2 — Repo Hygiene

**Files to remove from repo (or move to .gitignore):**

| Action | Target | Reason |
|--------|--------|--------|
| Delete | `repomix-output.xml` | Local tooling artifact, 1.2MB, no user value |
| Delete | `repomix.config.json` | Local tooling config, no user value |
| Add to `.gitignore` | `context_store/` | Runtime-generated artifact directory |
| Add to `.gitignore` | `llm_cache/` | Runtime-generated artifact directory |
| Delete | `examples/framework/counter_app.axbc` | Compiled binary should not be in source control |
| Move or archive | `docs/RENAME_AUDIT.md` | Internal rename plan, Phase 1 history |
| Move or archive | `docs/RENAME_TO_BORUNA_AUDIT.md` | Internal rename plan, Phase 2 history |
| Move or archive | `docs/RENAMING.md` | Internal rename history |
| Move or archive | `docs/MILESTONE_DOGFIX_PLAN.md` | Internal implementation plan |
| Move or archive | `docs/CHANGELOG_DOGFIX.md` | Superseded by root CHANGELOG.md |
| Move or archive | `docs/LLM_EFFICIENCY_ROADMAP.md` | Internal roadmap with stale statuses |
| Move or archive | `docs/FRAMEWORK_STATUS.md` | Internal completion checklist (stale numbers) |
| Delete | `docs/IDE_GAPS.md` | References non-existent `boruna-ide` repo and `src-tauri` paths |
| Move to `.github/` | `plans/` directory | Internal engineering design docs — move to `.github/internal/plans/` or add a header |

**Naming artifacts to fix:**

| File | Fix |
|------|-----|
| `docs/DOGFOOD_FINDINGS.md` lines 89, 111 | Change ` ```llm ` code fence tags to ` ```ax ` |
| README.md line 197 | Fix "541+ tests" → "557+ tests" (consistent with line 36) |

**Note on crate directory naming**: The mismatch between directory names (`crates/llmc/`) and crate names (`boruna-compiler`) is a known artifact documented in `docs/RENAMING.md`. The bytecode magic bytes `"LLMB"` are embedded in the binary format. Renaming directories would require a significant refactor (update all internal paths, CI, documentation) with risk of breakage. Defer this to a separate targeted PR rather than including it here. The `docs/RENAMING.md` (which explains the mapping) should be kept but moved to `docs/reference/` as part of Phase 4.

---

### Phase 3 — Workflow Examples

This is the highest-impact content change. The current workflow step files all return `fn main() -> Int { 42 }`. They must be replaced with implementations that demonstrate real behavior.

**Constraint**: The VM does not execute real LLM calls or HTTP requests unless the `--live` flag is used with the `http` feature enabled. This means examples need to work in two modes:
- **Demo mode (default)**: Realistic `.ax` code that demonstrates the language patterns and capability declarations, with representative hardcoded data that makes the output meaningful.
- **Live mode (documented)**: The same code, but with `net.fetch` or `llm.call` that execute against real endpoints when `--policy allow-all --live` is passed.

**For each workflow:**

#### `examples/workflows/llm_code_review/`

Add `README.md`:
```markdown
# LLM Code Review Workflow

## Use case
Your team reviews 50+ PRs per day. This workflow automates pre-screening:
fetching the diff, running it through an LLM analyzer, and producing a
structured review report — with a full audit trail showing which model
analyzed which diff under which policy.

## Steps
- `fetch_diff` → fetches the PR diff (demo: representative diff string; live: net.fetch)
- `analyze` → calls LLM with security and style review prompt (demo: structured mock output; live: llm.call)
- `report` → formats findings as a structured review report

## Run it
cargo run --bin boruna -- workflow validate examples/workflows/llm_code_review
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review --policy allow-all --record

## Evidence produced
An evidence bundle at `evidence/run-llm-code-review-<timestamp>/` containing:
- Hash-chained audit log of all 3 steps
- Per-step inputs and outputs
- Policy snapshot
- Environment fingerprint
```

Replace `steps/fetch_diff.ax`:
```ax
// Fetches a PR diff for LLM review.
// In live mode: makes a real API call via net.fetch capability.
// In demo mode: returns a representative diff showing the capability declaration.

fn fetch_diff() -> String !{} {
  "--- a/src/auth.rs\n+++ b/src/auth.rs\n@@ -42,7 +42,6 @@ impl AuthService {\n-        let token = generate_token();\n+        let token = Token::new(user_id, expires_at);\n         self.sessions.insert(token.clone(), user_id);\n         token\n     }\n"
}

fn main() -> Int {
  let diff = fetch_diff();
  0
}
```

Replace `steps/analyze.ax`:
```ax
// Analyzes a code diff for security and style issues.
// Capability: llm.call — declared but not invoked in demo mode.

type Finding {
  severity: String,
  message: String,
  line: Int,
}

fn analyze_diff(diff: String) -> String !{} {
  // Demo: returns structured findings representative of real LLM output
  "{\"findings\": [{\"severity\": \"medium\", \"message\": \"Token generation moved to Token::new — verify expiry is set correctly\", \"line\": 43}, {\"severity\": \"low\", \"message\": \"Consider adding test coverage for session insertion\", \"line\": 45}], \"model\": \"gpt-4\", \"policy\": \"allow-all\"}"
}

fn main() -> Int {
  let diff = "--- a/src/auth.rs\n+++ b/src/auth.rs\n@@ -42,7 +42,6 @@\n-        let token = generate_token();\n+        let token = Token::new(user_id, expires_at);";
  let result = analyze_diff(diff);
  0
}
```

Replace `steps/report.ax`:
```ax
// Formats analysis findings as a structured code review report.

fn format_report(analysis: String) -> String !{} {
  "## Automated Code Review Report\n\nAnalysis completed.\n\n### Findings\n- [MEDIUM] Line 43: Token generation moved to Token::new — verify expiry is set correctly\n- [LOW] Line 45: Consider adding test coverage for session insertion\n\n### Summary\n2 findings (1 medium, 1 low). No blocking issues.\n\n---\nGenerated by Boruna LLM Code Review Workflow v1.0.0\nEvidence bundle: recorded for audit trail"
}

fn main() -> Int {
  let report = format_report("analysis-output");
  0
}
```

Apply the same pattern to `document_processing` and `customer_support_triage` — realistic code with real `.ax` types, records, pattern matching, and capability declarations even if not live-executed.

**Also add README.md to `document_processing/` and `customer_support_triage/`** following the same template:
- When to use this
- What it does (each step)
- Pattern demonstrated (fan-out, approval gate)
- Evidence produced
- How to run it

---

### Phase 4 — Docs Restructure

Reorganize `docs/` from a flat list into a hierarchy following the Divio four-quadrant model (Tutorials, How-to, Reference, Explanation).

**New structure:**

```
docs/
  concepts/                     — Why things work the way they do
    determinism.md              — (from DETERMINISM_CONTRACT.md)
    capability-model.md         — (new, synthesized from EFFECTS_GUIDE.md + language-guide.md)
    workflow-lifecycle.md       — (from ENTERPRISE_PLATFORM_OVERVIEW.md concepts)
    policy-model.md             — (from PLATFORM_GOVERNANCE.md)
    audit-and-evidence.md       — (from COMPLIANCE_EVIDENCE.md)
    actor-model.md              — (from ACTORS_GUIDE.md)

  guides/                       — How to accomplish specific tasks
    first-workflow.md           — Build and run a 3-step workflow from scratch
    add-approval-gate.md        — Human-in-the-loop pattern
    writing-policies.md         — (from PLATFORM_GOVERNANCE.md practical sections)
    replay-verification.md      — (from TRACE_TO_TESTS.md and EFFECTS_GUIDE.md)
    embed-in-rust.md            — (from INTEGRATION_GUIDE.md)
    ci-integration.md           — (from OPERATIONS.md)
    mcp-integration.md          — (new, from boruna-mcp README content in CLAUDE.md)

  reference/                    — Complete, accurate descriptions
    cli.md                      — All CLI subcommands with flags and exit codes
    workflow-json.md            — workflow.json schema
    ax-language.md              — (from language-guide.md)
    capabilities.md             — Capability registry (IDs, semantics, gating)
    bytecode-spec.md            — (moved from docs/)
    mcp-tools.md                — 10 MCP tool schemas (from boruna-mcp plans)
    standard-libraries.md       — 11 stdlib entries (from STD_LIBRARIES_SPEC.md)

  stability.md                  — Stability tiers for all components
  roadmap.md                    — Narrative roadmap (themes, not dates)
  faq.md                        — (new)
  limitations.md                — (new — what Boruna does not currently do)

  archive/                      — Internal documents not deleted (historical record)
    RENAME_AUDIT.md
    RENAME_TO_BORUNA_AUDIT.md
    RENAMING.md
    MILESTONE_DOGFIX_PLAN.md
    CHANGELOG_DOGFIX.md
    LLM_EFFICIENCY_ROADMAP.md
    FRAMEWORK_STATUS.md
    DOGFOOD_FINDINGS.md
    ENTERPRISE_GAPS.md          — Keep for internal reference, not linked from README
```

Files to keep at `docs/` root (still product-facing, just not restructured yet):
- `QUICKSTART.md` — (replaced entirely in Phase 6)
- `OPERATIONS.md` → eventually moves to `guides/`
- `FRAMEWORK_API.md` → eventually moves to `reference/`
- `PACKAGE_SPEC.md` → eventually moves to `reference/`
- `TESTING_GUIDE.md` → eventually moves to `guides/`

**New files to write:**

#### `docs/stability.md`

```markdown
# Component Stability

Boruna uses a three-tier stability model. Breaking changes in Beta components
require a CHANGELOG entry and deprecation notice. Alpha components may change
without prior notice.

| Component | Tier | What's stable |
|-----------|------|---------------|
| Workflow execution engine | **Beta** | workflow.json schema, step execution, DAG ordering |
| Evidence bundle format | **Beta** | SHA-256 chain format; new fields are additive-only |
| CLI: `workflow`, `evidence` | **Beta** | Current flags and output format |
| `.ax` language syntax | Alpha | May change; every change gets a CHANGELOG entry |
| Actor system | Alpha | API subject to change |
| MCP server (boruna-mcp) | Alpha | Tool schemas subject to change |
| Framework (Elm architecture) | Alpha | AppRuntime API evolving |
| Package system (boruna-pkg) | Alpha | Lockfile format may change |
| HTTP handler (`--live` flag) | Alpha | Interface stable; behavior under test |
```

#### `docs/roadmap.md`

```markdown
# Roadmap

Boruna is at v0.1.0. This is a direction statement, not a commitment to dates or features.

## Current focus: Hardening

- Persistent workflow state (survive process restart mid-run)
- Structured retry policies (jitter, backoff, max-attempts per step)
- Richer step output types (beyond the current single "result" key)
- Distributed step execution (steps on separate processes/machines)

## Developer experience

- Language server protocol (LSP) support for `.ax` files
- Watch mode for rapid iteration
- Improved error messages with repair suggestions for common mistakes
- Integration test framework for workflow authors

## Ecosystem

- Python SDK for writing workflow steps without `.ax`
- Docker-based workflow step execution
- GitHub Actions integration package

## Not currently planned

- Managed cloud service (self-hosted focus in this phase)
- General-purpose language competing with Python/Rust/Go
- IDE or editor (separate project)
- Plugin marketplace

Changes to this roadmap are noted in CHANGELOG.md under `[Unreleased]`.
```

#### `docs/faq.md`

Key questions to answer:
1. How is Boruna different from Temporal?
2. Does `.ax` replace Python/Rust for AI code?
3. What does "deterministic" actually guarantee?
4. Can I use Boruna with OpenAI / Anthropic APIs?
5. Is Boruna production-ready?
6. What is the evidence bundle for?
7. Can I write workflow steps in a language other than `.ax`?
8. What happens when a step fails mid-workflow?

#### `docs/limitations.md`

Honest, specific list:
- No persistence across process restarts (workflow state is in-memory)
- No distributed step execution (single process)
- No native Python/TypeScript step support (`.ax` only for now)
- Actor system is experimental
- Package registry is local only (no remote packages yet)
- Real HTTP handler requires the `http` cargo feature and `--live` flag
- Evidence bundle verification does not support cryptographic signing (hash integrity only)

---

### Phase 5 — README & Positioning Rewrite

**Single canonical tagline** (apply everywhere):
> Boruna — deterministic, policy-gated execution for enterprise AI workflows

**README rewrite target**: 180–220 lines. A portal, not documentation.

**New structure:**

```markdown
# Boruna

[CI badge] [License badge] [Version badge]

**Deterministic, policy-gated execution for enterprise AI workflows.**

Run AI workflows where every execution is reproducible, every side effect is
declared and enforced, and every run can produce a verifiable compliance bundle.

## Why Boruna

[3-paragraph "why determinism matters for enterprise AI" section]
[Use the Restate replay metaphor: same inputs + same policy = identical outputs, every time]

## What Boruna Does

[5 bullets, current "What Boruna Does" section — keep this]

## What Boruna Is Not

[Current section — keep this, it's good]

## Quick Start

[Rewrite to: prerequisites → build → run first workflow → see evidence bundle]
[End with the evidence bundle output as the visible payoff]

## Reference Workflows

[Current table — add business use case description column]

## Component Stability

[Link to docs/stability.md + inline the table]

## Documentation

[Navigation links to docs/ structure: Concepts → Guides → Reference → FAQ]

## Roadmap

[3-line summary + link to docs/roadmap.md]

## Contributing

[Link to CONTRIBUTING.md + one-sentence invitation]

## License

MIT — see LICENSE
```

**Remove from README:**
- The "For LLMs and Coding Agents" section (40+ lines) → move to `AGENTS.md` at root
  This section is valuable for LLM-based contributors but creates the wrong first impression for human evaluators.

**Move CLI reference table** from README to `docs/reference/cli.md` — link from README with "See the [CLI reference](docs/reference/cli.md)" one-liner.

**Resolve tagline inconsistency:**
- `docs/BRANDING.md` → update tagline to match README
- `docs/language-guide.md` line 5 → rephrase "a statically-typed, capability-safe language for writing deterministic workflow steps" (not "LLM-native applications")

---

### Phase 6 — Onboarding & Maturity Signals

#### Rewrite `docs/QUICKSTART.md`

Target: 400–600 words. Structure:

```markdown
# Quickstart

## What you'll build
A 3-step LLM code review workflow that validates, runs, and produces a
verifiable evidence bundle.

## Prerequisites
- Rust stable (1.75+): https://rustup.rs
- Git, 5 minutes

## 1. Clone and build
[commands]

## 2. Validate the example workflow
[boruna workflow validate command + expected output]

## 3. Run it and record evidence
[boruna workflow run command + expected output showing completion + bundle path]

## 4. Verify the evidence bundle
[boruna evidence verify command + expected output showing integrity check]

## 5. Inspect what happened
[boruna evidence inspect command + expected JSON output]

## What just happened
[3 sentences: what the workflow did, what the evidence bundle is, what replay is]

## Next steps
- [Write your first step] → guides/first-workflow.md
- [Understand determinism] → concepts/determinism.md
- [Configure a policy] → guides/writing-policies.md
- [Browse examples] → examples/
```

**Key design principle**: The quickstart ends with visible evidence bundle output. This makes the compliance value concrete before any explanation.

#### Add `AGENTS.md` (new file at root)

Move the "For LLMs and Coding Agents" section from README here. Add:
- The entry point table (already in README)
- Critical invariants for agents
- Capability list
- A note that this project is also used by AI agents via the MCP server (`crates/boruna-mcp/`)

#### Badge row additions

Current README has one badge (CI). Add:
- License badge: `[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)`
- Version badge: `[![Version](https://img.shields.io/badge/version-0.1.0-blue)](CHANGELOG.md)`
- (Optional) Test count badge via CI artifact

#### Commercial path signal (one-liner only)

Add to README, one line under the Contributing section:
> Building on Boruna for an enterprise use case? [Open a discussion](https://github.com/escapeboy/boruna/discussions) — we want to hear about it.

This signals interest in tracking adoption without implying a commercial product.

---

## Alternative Approaches Considered

### Alternative: Docs site (MkDocs/Docusaurus)
Generate a hosted docs site alongside the GitHub docs. **Rejected for this phase** because:
- It requires a domain, hosting decision, and CI integration not yet in place
- The docs content must be right before the infrastructure matters
- A well-structured `docs/` in the repo serves the same purpose for early evaluators

### Alternative: Replace `.ax` workflow steps with Python/shell stubs
Use shell scripts or Python to make example outputs feel more "real." **Rejected** because:
- It undermines the core value proposition (the whole point is `.ax` steps on the deterministic VM)
- Misleads about the actual product capability
- The right solution is better `.ax` code with realistic hardcoded demo data

### Alternative: Move all internal docs to a separate private repo
Move `plans/`, `docs/archive/`, and internal docs to a private companion repo. **Partially accepted**:
- Internal docs that create confusion (`IDE_GAPS.md`) should be removed
- Rename history should be archived in `docs/archive/` rather than deleted (useful for contributors)
- `plans/` should be prefixed with a header note that these are internal engineering documents

---

## Acceptance Criteria

### Phase 1 — Trust Foundation

- [ ] `LICENSE` file exists at repo root with full MIT text
- [ ] `CHANGELOG.md` exists at repo root following keep-a-changelog format with v0.1.0 entry
- [ ] `SECURITY.md` exists at repo root with vulnerability reporting instructions, 48h commitment, safe harbor statement
- [ ] `CONTRIBUTING.md` exists at repo root with prerequisites, workflow, PR checklist
- [ ] `CODE_OF_CONDUCT.md` exists at repo root linking to Contributor Covenant v2.1

### Phase 2 — Repo Hygiene

- [ ] `repomix-output.xml` and `repomix.config.json` deleted from root
- [ ] `context_store/` and `llm_cache/` added to `.gitignore` (remove from tracking if present)
- [ ] `examples/framework/counter_app.axbc` deleted
- [ ] Internal docs (`RENAME_AUDIT.md`, `IDE_GAPS.md`, etc.) moved to `docs/archive/` or deleted
- [ ] `docs/DOGFOOD_FINDINGS.md` code fence tags corrected (`llm` → `ax`)
- [ ] README line 197 corrected to "557+ tests"

### Phase 3 — Workflow Examples

- [ ] All 11 step `.ax` files have meaningful code (types, records, function signatures) that demonstrates real `.ax` patterns
- [ ] No step file returns a hardcoded `42` as its only output
- [ ] All capability annotations are accurate (`!{}` for pure steps, `!{net.fetch}` or `!{llm.call}` for live steps)
- [ ] Each workflow directory contains a `README.md` explaining: use case, steps, evidence produced, how to run
- [ ] `cargo test --workspace` still passes with 557+ tests
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `boruna workflow validate examples/workflows/llm_code_review` outputs a sensible validation result

### Phase 4 — Docs Restructure

- [ ] `docs/concepts/` directory exists with at least 3 files
- [ ] `docs/guides/` directory exists with at least 3 files
- [ ] `docs/reference/cli.md` exists and covers all CLI subcommands
- [ ] `docs/stability.md` exists with stability tier table
- [ ] `docs/roadmap.md` exists (narrative themes, no dates)
- [ ] `docs/faq.md` exists with at least 6 questions answered
- [ ] `docs/limitations.md` exists with an honest, specific list
- [ ] Internal docs moved to `docs/archive/` (not linked from README)
- [ ] No broken links in any newly created or modified doc

### Phase 5 — README & Positioning Rewrite

- [ ] README is 180–220 lines
- [ ] Single canonical tagline used in README, BRANDING.md, language-guide.md
- [ ] "For LLMs and Coding Agents" section removed from README; content present in `AGENTS.md`
- [ ] CLI reference table removed from README; link to `docs/reference/cli.md` present
- [ ] Three badges present: CI, License, Version
- [ ] "What Boruna Is Not" section retained
- [ ] Evidence bundle output is visible in the Quick Start section

### Phase 6 — Onboarding & Maturity Signals

- [ ] `docs/QUICKSTART.md` is 400–600 words and covers: prerequisites, build, validate, run, verify, inspect
- [ ] Quickstart ends with expected output for `boruna evidence verify`
- [ ] `AGENTS.md` exists at root with the LLM/agent documentation
- [ ] README has license and version badges in addition to CI badge
- [ ] A one-line "open a discussion" invitation for enterprise use cases is present in README

---

## Success Metrics

A competent engineer visiting the repository after this transformation should be able to answer these questions without opening a single Rust source file:

1. **What problem does this solve?** — Within 30 seconds of reading the README
2. **Is it safe to use legally?** — Yes: LICENSE file is present, MIT
3. **Is this project serious?** — Yes: CHANGELOG, CONTRIBUTING, SECURITY all present
4. **What changed between versions?** — CHANGELOG.md at root, v0.1.0 entry complete
5. **How do I report a vulnerability?** — SECURITY.md, clear instructions
6. **What is the evidence bundle?** — Explained in README Quick Start, visible output shown
7. **What is alpha vs stable?** — `docs/stability.md`, inline table in README
8. **Where is this going?** — `docs/roadmap.md`, themes without date commitments
9. **How do I run a real example?** — QUICKSTART.md, single workflow to completion
10. **How do I contribute?** — CONTRIBUTING.md, 8 steps

---

## Dependencies & Prerequisites

- Phase 1 has no dependencies. Start here.
- Phase 2 depends on Phase 1 (CHANGELOG must exist before archiving internal changelog).
- Phase 3 depends on nothing but benefits from Phase 2 (cleaner working state).
- Phase 4 depends on Phase 2 (need to know which docs are internal before restructuring).
- Phase 5 depends on Phases 2, 3, 4 (README content references cleaned examples and docs structure).
- Phase 6 depends on Phase 5 (quickstart links must match the new README structure).

**Cargo.toml version**: Remains at `0.1.0`. Do not bump version as part of this transformation — the version bump to `0.2.0` should happen after the CHANGELOG and tagging workflow is in place.

**Tests must pass after every phase.** Run `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` after Phase 3 (the only phase that touches `.ax` source files).

---

## Risk Analysis

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Workflow step `.ax` code breaks VM execution | Medium | High | Keep steps minimal and test `workflow validate` + `workflow run` after changes |
| Broken internal links after docs restructure | Medium | Medium | Run a link checker after Phase 4 |
| Positioning change alienates current evaluators | Low | Low | The positioning clarification doesn't change the product, only removes ambiguity |
| Removing internal docs causes confusion for maintainers | Low | Low | Archive in `docs/archive/` rather than delete; add header note |
| Missing something that's still publicly indexed | Low | Low | Check Google Search Console / GitHub search after phase completion |

---

## Future Considerations

**Not in this plan, but natural next steps:**

1. **GitHub Releases + version tag**: After the CHANGELOG workflow is established, create a `v0.1.0` GitHub Release with the CHANGELOG entry as the body. This adds the version badge link target.

2. **Pre-built binary distribution**: Ship `boruna` binary via GitHub Releases for macOS/Linux. `cargo binstall` support costs ~2 hours and removes the biggest onboarding friction (the `cargo build --workspace` step).

3. **Docs site**: MkDocs Material or Docusaurus on GitHub Pages. The restructured `docs/` from Phase 4 is a direct input.

4. **Additional example workflows**: Contract analysis, data quality validation, LLM prompt regression testing. Each adds a new enterprise use case to the portfolio.

5. **`v0.2.0` milestone**: The first version bump after this transformation. Entry point for contributors to see what shipping looks like with the new CHANGELOG workflow.

---

## Documentation Plan

Files created or modified by this plan:

**New files at root:**
- `LICENSE` — MIT
- `CHANGELOG.md` — keep-a-changelog format
- `SECURITY.md` — vulnerability reporting policy
- `CONTRIBUTING.md` — contributor guide
- `CODE_OF_CONDUCT.md` — Contributor Covenant v2.1
- `AGENTS.md` — LLM/agent integration guide (content moved from README)

**Modified files:**
- `README.md` — complete rewrite to portal format
- `docs/QUICKSTART.md` — complete rewrite
- `docs/BRANDING.md` — tagline updated
- `docs/language-guide.md` — description line updated
- `docs/DOGFOOD_FINDINGS.md` — code fence tags fixed
- `.gitignore` — add context_store/, llm_cache/

**New docs files:**
- `docs/stability.md`
- `docs/roadmap.md`
- `docs/faq.md`
- `docs/limitations.md`
- `docs/concepts/determinism.md`
- `docs/guides/first-workflow.md`
- `docs/reference/cli.md`
- `examples/workflows/llm_code_review/README.md`
- `examples/workflows/document_processing/README.md`
- `examples/workflows/customer_support_triage/README.md`

**Modified workflow steps (all `.ax` files in `examples/workflows/*/steps/`):**
- `llm_code_review/steps/fetch_diff.ax`
- `llm_code_review/steps/analyze.ax`
- `llm_code_review/steps/report.ax`
- `document_processing/steps/*.ax` (all)
- `customer_support_triage/steps/*.ax` (all)

---

## References

### Internal References
- Current README: `README.md` (217 lines, CI badge only, hollow examples)
- Current docs structure: `docs/` (33 files, flat, mixed internal/external)
- Workflow examples: `examples/workflows/*/steps/*.ax` (all return `fn main() -> Int { 42 }`)
- Quickstart: `docs/QUICKSTART.md` (69 lines, outdated)
- Determinism contract: `docs/DETERMINISM_CONTRACT.md` — excellent, move to `docs/concepts/`
- Security model: `docs/SECURITY_MODEL.md` — keep, distinct from `SECURITY.md` (this is architectural, that is policy)
- Enterprise gaps: `docs/ENTERPRISE_GAPS.md` — useful internally, archive from public nav
- Stability: `docs/FRAMEWORK_STATUS.md` — stale, superseded by `docs/stability.md`

### External References
- Keep a Changelog spec: https://keepachangelog.com/en/1.1.0/
- Semantic Versioning: https://semver.org/spec/v2.0.0.html
- Divio documentation system (four-quadrant model): https://docs.divio.com/documentation-system/
- Temporal README (portal pattern): https://github.com/temporalio/temporal/blob/main/README.md
- Dagger CHANGELOG (breaking changes pattern): https://github.com/dagger/dagger/blob/main/CHANGELOG.md
- Restate docs (deterministic execution messaging): https://docs.restate.dev
- OpenTelemetry stability tiers: https://github.com/open-telemetry/opentelemetry-collector/blob/main/docs/component-stability.md
- Temporal samples (example quality benchmark): https://github.com/temporalio/samples-go
- Contributor Covenant v2.1: https://www.contributor-covenant.org/version/2/1/code_of_conduct/

### Competitive Context
- **Restate** (closest analog — deterministic execution, similar value prop): https://restate.dev
- **Temporal** (durable execution, general-purpose): https://temporal.io
- **Prefect** (Python-native workflow orchestration): https://prefect.io
- **LangGraph** (AI agent orchestration): https://github.com/langchain-ai/langgraph
