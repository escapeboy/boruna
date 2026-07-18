# Agent-facing corpus

This directory and a few root-level files exist so an AI coding agent can ingest
Boruna in a single fetch and then write correct `.ax` without prior training data.

## What's here

| File | Purpose |
|------|---------|
| [`../../llms.txt`](../../llms.txt) | Short, link-first index (the [llms.txt](https://llmstxt.org) convention). Point an agent here first. |
| [`../../llms-full.txt`](../../llms-full.txt) | The full `.ax` teaching primer inlined into one document: types, records/enums, pattern matching, capabilities, the framework App protocol, and correct snippets. |
| [`portal.json`](portal.json) | Machine-readable manifest: Boruna version, the 11-capability taxonomy, the 13 stdlib libraries, the 5 templates, and the MCP tool list. |

## How to use it

- **Humans / agents learning the language:** read `llms-full.txt`.
- **Tools that need structured facts** (capabilities, stdlib, templates, MCP tools):
  parse `portal.json`.
- **Deeper reference:** `docs/reference/ax-language.md` (narrative) and
  `docs/reference/cli.md`. When a snippet is in doubt, verify with
  `boruna lang check <file>.ax --json`.

## Drift caveat (important)

These files are **static and hand-curated**. They are not yet generated from the
compiled toolchain, so they can drift from the source of truth:

- The **compiler** (`crates/llmc`) defines the real grammar. Records use the `type`
  keyword; enum variants are unit or single-payload (`Enum::Variant(payload)`).
  The narrative reference's `record` keyword and struct-style enum variants are
  known drift — this corpus follows the compiler.
- `portal.json` mirrors data that lives in `crates/llmbc` (capabilities), `libs/`
  (stdlib), `templates/`, and the `boruna-mcp` tool registry.

Per the `"note"` field in `portal.json`, a future `boruna` subcommand should
**generate** this manifest from the compiled toolchain so it can never drift. Until
then, update these files whenever capabilities, stdlib packages, templates, MCP
tools, or language syntax change.
