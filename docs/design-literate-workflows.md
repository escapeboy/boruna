# Design — Literate workflow specs (`boruna literate extract`)

## Status

Planned for v1.5.0. **Implemented this sprint** (per `/sprint-orchestrate full планирай и
имплементирай`). Borrowed from Quint's Literate Specifications.
Source: research_quint_borrowable_ideas_2026-05-20.md, rec #3.

## Context

Boruna's 1.4.0 shipped compliance example workflows at `examples/compliance/` (SOC 2 audit,
HIPAA pipeline, financial review). The intent: workflows ARE auditable specifications. But today
a compliance officer reading a workflow has to flip between (a) the `.ax` source files inside
the workflow directory, (b) the `workflow.json` DAG manifest, and (c) a separately-maintained
README.

Quint's literate specs solve this exact split: one markdown document contains the audit narrative
inline with `quint filename.qnt +=` code fences that extract into the actual spec files. The
human reads one document. The compiler reads the same document and emits the runnable spec.

## Why

This is the single most audience-aligned borrow from Quint for Boruna. The compliance customer
is exactly the user who needs one document that legal can read and the engine can execute.
Boruna has *zero* equivalent today and the gap is widening as the compliance template library
grows.

The Quint FAQ specifically argues: *"Markdown accepts all kinds of errors: undefined names, type
mismatches, impossible claims and ambiguous interfaces, because it is not executable. A Quint
spec gives you name resolution, type checking, and the ability to catch contradictions and
undefined references automatically."* That's directly the Boruna pitch for executable workflows.

## Goals

1. `boruna literate extract <file.md> --out-dir <dir>` parses markdown, extracts every code
   fence labeled `ax <filename> +=`, appends each block to the named file in `--out-dir`.
2. Same input file run twice produces byte-identical output (idempotent / deterministic).
3. Round-trip: `boruna literate extract foo.md` followed by `boruna run` on the extracted output
   should compile and execute the embedded spec.
4. Diagnostic surface: bad fences (unknown filename syntax, mismatched `+=`) produce structured
   errors with line numbers matching the markdown source.
5. The extract command is a `boruna-tooling` library function — reusable from `boruna-mcp` so an
   agent can extract and validate literate specs without a shell.

## Non-goals

- Reverse direction (`.ax` → markdown). The markdown is the authoritative source.
- Markdown rendering as HTML/PDF. That's the dashboard's job (existing `evidence_serve.rs`
  shows it can serve HTML if needed).
- Watch mode (`--watch`). Deferred; the existing `boruna run --watch` deferred item covers it
  generically.
- Multi-format input (no AsciiDoc/RST/Org-mode). Markdown only.
- `lmt`-style "tangle and weave" mode — we only tangle (`extract`). Untangling is the
  out-of-scope direction.

## Forcing questions

**Who needs this? What are they doing today?**
A risk officer at a hypothetical Boruna customer writing the SOC 2 control-flow narrative. Today
they maintain (a) a Word doc explaining the workflow to auditors, (b) the actual `.ax` files,
and (c) hope the two stay in sync. They don't. With literate workflows: the audit narrative IS
the executable spec.

**What's the narrowest MVP someone would pay for?**
`boruna literate extract file.md` produces files in cwd. No options, no validation pass — just
extraction. If that works and the extracted output `boruna workflow validate`s, the rest is polish.

**What would make someone say "whoa"?**
Running `boruna literate extract examples/compliance/soc2_audit_workflow.md` and getting back
a complete, executable, `workflow validate`-passing workflow directory. The same document is
already what auditors review.

**How does this compound over time?**
Every compliance template in `examples/compliance/` can be rewritten as a single markdown file.
The audit narrative travels with the spec, immune to drift. Future: same machinery extracts
`.ax` library docs from markdown (treats Boruna docs like Quint treats specs).

## Scope (this sprint)

| In | Out |
|---|---|
| `boruna literate extract <file.md> --out-dir <dir>` | `boruna literate render` (markdown → HTML) |
| Code fences ` ```ax <filename> += ` (and ` ```rust ` etc. ignored) | Hot-reload / watch mode |
| Append semantics (multiple blocks merge into one file) | Replace / overwrite semantics |
| Idempotent runs (delete `--out-dir` first, then re-extract) | Incremental sync |
| Reuse from MCP via `boruna_literate_extract` tool | (no MCP tool exposed this sprint) |
| Structured diagnostics with markdown-line attribution | LSP integration for inline diagnostics |

## Decisions

1. **Fence syntax:** Match Quint exactly — ` ```<lang> <filename> += `. We accept `ax`,
   `boruna`, and `quint` as the language tag. Filename is a relative path resolved against
   `--out-dir`. The `+=` is required (no `=` overwrite mode).
2. **Append behavior:** Multiple fences with the same filename append in document order,
   separated by a single newline. No automatic blank line. Each block ends with a newline.
3. **Lang tag enforcement:** ONLY `ax`, `boruna`, `quint` are extracted. Other tags (`rust`,
   `sh`, etc.) are ignored. This matches Quint's behavior.
4. **Path safety:** Filenames may NOT contain `..` or be absolute paths. Per project conventions
   `path traversal prevention` section, rejected at parse time with `error_kind:
   "invalid_filename"`. Defense-in-depth: `canonicalize()` after path construction.
5. **Existing file behavior:** If `<out-dir>/<filename>` already exists, the FIRST `+=` block in
   the document REPLACES it; subsequent `+=` blocks append. (Quint's behavior is "all `+=`
   append", but we deliberately diverge to enable idempotent extraction: re-running the command
   produces a clean output rather than doubling existing content. Recorded as decision.)

## Risks

- **Decision #5 diverges from Quint.** May surprise users transferring from Quint who expect
  pure append. Mitigation: document the divergence prominently in the user-facing docs; add a
  `--quint-compat` flag if real users complain.
- **Markdown parsing edge cases.** Nested fences in markdown (rare but possible — a fence inside
  a blockquote, or a fence specifying ` ```` (four backticks) instead of ` ``` `). Use the
  `pulldown-cmark` crate (already widely adopted in Rust ecosystem) for robust parsing rather
  than line-by-line regex.
- **Cargo dep growth.** `pulldown-cmark` is ~5 transitive deps. Acceptable; it's pure-Rust and
  default-features = false.

## Open questions resolved during architecture phase

- Module location: `tooling/src/literate/` (parallel to `templates/`).
- CLI command structure: `boruna literate <extract>` (subcommand pattern, leaves room for
  future `render`, `lint`).
- Error type: existing `boruna_tooling::diagnostics::Diagnostic` (already has line/column spans).
