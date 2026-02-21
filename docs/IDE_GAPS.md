# IDE Gaps — Boruna Features Needed by Boruna Studio

These are features that the Boruna Studio IDE needs but the Boruna toolchain
does not yet provide. Each gap describes the symptom, what's minimally needed,
and a suggested approach.

## GAP-001: LSP Server (P1)

**Symptom**: IDE relies on CLI subprocess calls for every operation. No real-time
completions, hover types, or jump-to-definition.

**Minimal need**: A JSON-RPC or stdio-based language server implementing at least:
- textDocument/diagnostic (push diagnostics)
- textDocument/completion (keyword + symbol completions)
- textDocument/hover (type info)
- textDocument/definition (go-to-definition)

**Suggested approach**: Wrap the existing compiler front-end in an LSP adapter
crate. Use `tower-lsp` or a minimal custom stdio loop.

## GAP-002: Structured AST with Source Spans (P1)

**Symptom**: `boruna ast <file>` output may not include precise source locations
(line/col ranges) for all named symbols.

**Minimal need**: Every AST node that represents a named symbol (function, type,
enum variant, let binding, parameter) should include `start_line`, `start_col`,
`end_line`, `end_col` in the JSON output.

**Suggested approach**: Extend the AST serializer in `boruna-compiler` to emit
span info from the parser's token positions.

## GAP-003: Incremental Checking (P2)

**Symptom**: Full re-check on every save is slow for large files/projects.

**Minimal need**: Cache parse/type-check results and only re-process changed
portions.

**Suggested approach**: Hash-based invalidation at the module level. If a file's
content hash hasn't changed, skip re-checking.

## GAP-004: Trace Diff Command (P2)

**Symptom**: No way to structurally compare two execution traces.

**Minimal need**: `boruna trace diff <file1.trace.json> <file2.trace.json>` that
outputs a JSON diff of cycles (added, removed, changed state hashes).

**Suggested approach**: Implement in `boruna-cli` using the existing trace schema;
compare cycle-by-cycle and emit deltas.

## GAP-005: Orchestrator JSON Stability (P1)

**Symptom**: `boruna-orch` commands may not produce stable JSON output suitable
for automated parsing by the IDE.

**Minimal need**: All `boruna-orch` subcommands should accept `--json` flag and
output structured JSON. Schema should be versioned.

**Suggested approach**: Add `--json` flag to plan/next/apply/review/status commands
in `boruna-orchestrator`.

## GAP-006: Rename/Refactor Commands (P2)

**Symptom**: No CLI support for rename-symbol or structural refactoring.

**Minimal need**: `boruna refactor rename --file <f> --line <l> --col <c> --new-name <n>`
that outputs a PatchBundle with the rename edits.

**Suggested approach**: Use AST + reference finder to locate all usages, generate
a PatchBundle with the text edits.

## GAP-007: PatchBundle CLI Commands (P1)

**Symptom**: No CLI to create, validate, or apply PatchBundles.

**Minimal need**:
- `boruna patch validate <bundle.json>` — check well-formedness
- `boruna patch apply <bundle.json>` — apply to working directory
- `boruna patch rollback <bundle.json>` — reverse application

**Suggested approach**: The `boruna-orchestrator` crate already has `PatchBundle`
load/save/apply/rollback methods. Expose them via `boruna-cli` subcommands.

## GAP-008: Package Tree JSON (P2)

**Symptom**: No way to get a structured dependency tree from CLI.

**Minimal need**: `boruna-pkg tree --json` outputting resolved dependency graph
with capability requirements per package.

**Suggested approach**: Extend `boruna-pkg` CLI with `tree --json` subcommand.

## GAP-009: Capability Aggregation (P2)

**Symptom**: No command to show total capabilities required by an app + all dependencies.

**Minimal need**: `boruna capabilities <entry.ax> --json` that aggregates all
`!{...}` annotations from the app and its resolved dependencies.

**Suggested approach**: Walk the resolved dependency graph, collect capability
annotations, and merge/deduplicate.

## GAP-010: Context Bundle Export (P1)

**Symptom**: IDE needs to assemble structured context bundles for LLM agent tasks
(file contents, AST excerpts, diagnostics, constraints) but no Boruna command
supports this.

**Minimal need**: A library function or CLI command that, given a task intent and
file set, produces a JSON context bundle with:
- Selected file contents (possibly truncated)
- AST structure of relevant symbols
- Active diagnostics
- Policy constraints

**Suggested approach**: New subcommand `boruna context-bundle --files <...> --intent <desc>`
or a library API in `boruna-tooling`.

**IDE workaround**: IDE builds context bundles locally using SHA-256 hashing compatible
with Boruna's `ContextStore` format. See `src-tauri/src/commands/agent.rs`.

## GAP-011: Agent Protocol Schema (P0)

**Symptom**: No formal agent request/response JSON schema in the Boruna repo.

**Minimal need**: `schemas/agent-protocol.schema.json` defining:
- AgentRequest (intent, constraints, criteria, budgets, context_bundle)
- AgentResponse (patchbundle_path, evidence, risk_report, summary)
- GateEvidence (gate, passed, output, duration_ms)

**Suggested approach**: Extract the protocol types from the IDE repo
(`docs/AGENT_PROTOCOL.md`) into a JSON Schema file.

**IDE workaround**: Protocol defined in IDE code and documented in
`boruna-ide/docs/AGENT_PROTOCOL.md`.

## GAP-012: Go-to Definition / Find References (P1)

**Symptom**: No `boruna lang goto-def` or `boruna lang references` commands.

**Minimal need**:
- `boruna lang goto-def <file> <line> <col>` → `{ file, line, col }`
- `boruna lang references <file> <line> <col>` → `[{ file, line, col }]`

**Suggested approach**: Use the AST + module resolver to find symbol definitions
and cross-file references.

**IDE workaround**: AST dump parsing + regex-based text search across project files.
