# Architecture — Agent-Native CLI surfaces

Companion to `docs/design-agent-native-cli.md`. Component-level build plan.

## Component map

```
tooling/src/diagnostics/
  registry.rs        NEW — DiagnosticCodeInfo + REGISTRY const + registry()
  mod.rs             EDIT — `pub mod registry;`

crates/llmvm-cli/src/
  main.rs            EDIT — Command/LangCommand/WorkflowCommand enums + dispatch arms
  doctor.rs          NEW — environment health checks
  size.rs            NEW — bytecode artifact cost analysis
  skills.rs          NEW — embedded skill-doc registry + lookup
  skills/            NEW dir — ax-language.md, cli.md, workflows.md, diagnostics.md

docs/reference/
  diagnostic-codes.md  NEW — human reference for the code registry
  cli.md               EDIT — document the 5 new surfaces

CHANGELOG.md         EDIT — ### Added entries
AGENTS.md            EDIT — mention the agent-facing surfaces
```

## 1. `lang codes`

- `tooling/src/diagnostics/registry.rs`:
  - `pub struct DiagnosticCodeInfo { code, name, summary, category }` (all `&'static str`).
  - `pub const REGISTRY: &[DiagnosticCodeInfo]` — one entry per `E001`–`E009`.
  - `pub fn registry() -> &'static [DiagnosticCodeInfo]`.
- `LangCommand::Codes { json: bool }`. Handler in `run_lang`:
  - JSON: `{ "version": 1, "codes": [ {code,name,summary,category}, ... ] }`.
  - Human: aligned table.
- Drift test in `tooling` tests: collect every `E0NN` const, assert 1:1 with registry codes.

## 2. `doctor`

- `Command::Doctor { json: bool }` → `doctor::run(json)`.
- `doctor.rs`: `Check { name: String, status: Status, detail: String }`, `Status = Ok|Warn|Error`.
- Checks: boruna version; compiled features (`cfg!(feature=...)` for http/serve/telemetry);
  `rustc --version` (Warn if absent); default data-dir resolution + writability;
  presence of `templates/`, `libs/`, `examples/` relative to cwd.
- JSON: `{ "ok": bool, "boruna_version": "...", "checks": [...] }`. Exit 1 if any `Error`.

## 3. `workflow graph`

- `WorkflowCommand::Graph { dir: PathBuf, json: bool }`.
- Load `WorkflowDef` via the same loader `WorkflowCommand::Validate` uses.
- Reuse the validator's topological sort for `topological_order` (Err on cycle → reported).
- Facts: `nodes` (id, kind, capabilities, depends_on), `edges`, `topological_order`,
  `roots` (no deps), `leaves` (no dependents).
- JSON object; human = summary line + adjacency listing.

## 4. `size`

- `Command::Size { file: PathBuf, json: bool }` → `size::run(&file, json)`.
- Compile via `boruna_compiler::compile(name, source)`. On compile error: emit the error,
  exit 1 (consistent with `compile`).
- Per-function: `name, arity, locals, op_count, capabilities`.
- Totals: function count, total ops, constants, types, globals.
- `bytecode_bytes`: serialize `Module` with Boruna's existing serializer; label the format.
- JSON `{ module, functions, totals, bytecode_bytes, bytecode_format }`; human = table.

## 5. `skills`

- `SkillsCommand::List { json }` and `SkillsCommand::Get { name, json }`.
- `skills.rs`: `struct Skill { name, summary, body }`, bodies via `include_str!("skills/*.md")`.
  `SKILLS: &[Skill]` static slice; `fn lookup(name) -> Option<&Skill>`.
- `list`: names + summaries (JSON array or table).
- `get <name>`: prints `body`; `--json` → `{ name, summary, content }`; unknown name → exit 1
  with available-names hint.
- Skill docs (curated, concise, agent-focused): `ax-language.md`, `cli.md`, `workflows.md`,
  `diagnostics.md`.

## Build order (sequential — all touch `main.rs`)

1. Diagnostics registry (`tooling`) + `lang codes` + drift test + `diagnostic-codes.md`.
2. `doctor`.
3. `workflow graph`.
4. `size`.
5. `skills` + embedded docs.
6. Docs sweep: `cli.md`, `CHANGELOG.md`, `AGENTS.md`.
7. Gates: `cargo test --workspace`, `clippy -D warnings`, `fmt --check`.

Parallel sub-agents are **not** used: every surface edits `crates/llmvm-cli/src/main.rs`
(shared enum + dispatch) — convention §31 mandates sequential for same-crate work.
