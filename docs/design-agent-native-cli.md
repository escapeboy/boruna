# Design — Agent-Native CLI surfaces

**Sprint:** agent-native-cli · **Branch:** `feature/agent-native-cli` (from `master`, v1.2.0-line)
**Date:** 2026-05-17
**Origin:** Research of `vercel-labs/zero` (`claudedocs/research_zerolang_vs_boruna_2026-05-17.md`) surfaced 5 agent-DX ideas worth borrowing.

## Why

Zero (Vercel Labs) validated the "agent-native language" category: a toolchain where every
surface emits structured, machine-readable facts that AI agents consume directly. Boruna
already has the foundation (capability gating, JSON diagnostics, `boruna-mcp`). This sprint
closes 5 specific gaps so agents can *inspect* Boruna projects as fluently as humans.

## Forcing questions

- **Who needs this?** AI coding agents (and humans) operating on `.ax` projects and workflows.
  Today they must read source, guess diagnostic-code meanings, and have no way to query
  artifact cost or workflow shape without writing a script.
- **Narrowest MVP?** Five read-only CLI surfaces, each with `--json`. No new runtime behavior,
  no schema changes to persisted data.
- **What makes someone say "whoa"?** `boruna skills get` — the binary self-describes how to
  write `.ax` and drive the toolchain, so a fresh agent is productive with zero repo access.
- **How does it compound?** Every future diagnostic code, CLI command, and workflow feature
  plugs into a registry/skill doc that agents already know how to query. The agent's
  understanding of Boruna scales with the toolchain instead of lagging it.

## Scope — 5 surfaces

Idea #1 from the research ("stable diagnostic codes") is **already implemented** — `E001`–`E009`
exist as stable `pub const` in `tooling/src/diagnostics/mod.rs`. Per the user decision it is
**replaced** with a machine-readable *registry* surface (the agent-facing piece that was missing).

| # | Surface | What it does |
|---|---------|--------------|
| 1 | `boruna lang codes [--json]` | Emit the registry of all diagnostic codes (id, name, summary, category). |
| 2 | `boruna doctor [--json]` | Environment/toolchain health: version, compiled features, data-dir writability, project dirs. |
| 3 | `boruna workflow graph <dir> [--json]` | Emit DAG facts: nodes, edges, topological order, roots, leaves. |
| 4 | `boruna size <file.ax> [--json]` | Bytecode artifact cost: per-function op counts, totals, serialized byte size. |
| 5 | `boruna skills list` / `boruna skills get <name> [--json]` | Embedded, agent-curated docs the binary serves with no repo access. |

## Non-goals

- No native compilation, no GC removal, no C ABI — those are Zero's niche, not Boruna's.
- No changes to determinism, replay, evidence bundles, or any persisted schema.
- No DOT/Graphviz export for `workflow graph` (JSON facts only; visualization is out of scope).
- No new MCP tools this sprint (CLI-first; MCP exposure is a possible follow-up).

## Decisions

- **`lang codes`, not a new top-level `diagnostics` command.** `Lang` is already
  "Language tooling commands (diagnostics, repair)" — `codes` belongs there. Avoids surface bloat.
- **All five are read-only.** No mutation, no persistence writes. Safe by construction.
- **`--json` everywhere; human output is the default.** Matches every existing Boruna CLI surface.
- **Skill docs embedded via `include_str!`.** The binary must self-describe without the repo
  checked out (an agent may only have the installed binary).
- **Registry is the single source of truth for codes.** A drift test (convention §33) asserts
  every `E0xx` const has exactly one registry entry and vice versa.

## Risks

- `doctor` runs `rustc --version` (external process) — acceptable for a diagnostic command;
  failure is reported as a `warn` check, never aborts.
- `size`'s "serialized bytes" depends on Boruna's bytecode serialization format; the number is
  labeled with the format used so it is not mistaken for a native-artifact size.
