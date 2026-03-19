# Boruna for AI Coding Agents

Boruna ships an MCP (Model Context Protocol) server that exposes its toolchain to AI coding agents. This enables agents to compile, run, validate, and inspect `.ax` programs and workflows directly.

## MCP server setup

Add to your MCP configuration (`.mcp.json` for Claude Code, or equivalent):

```json
{
  "mcpServers": {
    "boruna": {
      "command": "cargo",
      "args": [
        "run",
        "--bin", "boruna-mcp",
        "--manifest-path", "/path/to/boruna/Cargo.toml"
      ],
      "env": {}
    }
  }
}
```

Start the server manually:

```bash
cargo run --bin boruna-mcp
```

The server communicates over JSON-RPC stdio. All tools return structured JSON with `"success": true|false`.

## Available tools

| Tool | Description |
|------|-------------|
| `boruna_compile` | Compile `.ax` source → module info or structured errors |
| `boruna_ast` | Parse `.ax` source → AST JSON |
| `boruna_run` | Compile + execute `.ax` source with policy, step limit, trace |
| `boruna_check` | Run diagnostics → severity, spans, suggested patches |
| `boruna_repair` | Auto-repair `.ax` source using diagnostic suggestions |
| `boruna_validate_app` | Validate App protocol conformance (init/update/view) |
| `boruna_framework_test` | Run a framework app through a message sequence |
| `boruna_workflow_validate` | Validate workflow DAG structure + topological order |
| `boruna_template_list` | List available app templates |
| `boruna_template_apply` | Apply a template with variable substitution |

## Usage patterns

**Compile and check for errors:**

```json
{ "tool": "boruna_check", "source": "fn main() -> Int { 42 }" }
```

**Run a program:**

```json
{
  "tool": "boruna_run",
  "source": "fn main() -> Int { 42 }",
  "policy": "allow-all",
  "step_limit": 10000
}
```

**Validate a workflow (pass the workflow.json content):**

```json
{
  "tool": "boruna_workflow_validate",
  "workflow_json": "{ \"id\": \"my-workflow\", ... }"
}
```

## Rules for agents working on Boruna

When writing or modifying Boruna code:

1. **Never break determinism** — use `BTreeMap`, never `HashMap` for ordered iteration. No randomness or time reads in pure code.
2. **Declare all capabilities** — functions with side effects require `!{capability}` annotations. The VM enforces this; tests will fail if missing.
3. **Run `cargo test --workspace`** after every change — 557+ tests must pass.
4. **Run `cargo clippy --workspace -- -D warnings`** — zero warnings are allowed. CI enforces this.
5. **Run `cargo fmt --all`** — formatting is enforced by CI.
6. **No semicolons in `.ax` files** — `.ax` has no statement terminators.
7. **Type annotations required** — every `let` binding needs an explicit type: `let x: Int = 42`.

## The ten capabilities

| Capability | Purpose |
|------------|---------|
| `net.fetch` | HTTP requests |
| `llm.call` | LLM inference |
| `time.now` | Current timestamp |
| `rand.next` | Random numbers |
| `fs.read` | File system reads |
| `fs.write` | File system writes |
| `db.query` | Database reads |
| `db.mutate` | Database writes |
| `actor.spawn` | Spawn actor processes |
| `actor.send` | Send actor messages |

## Key entry points for exploration

| Task | File |
|------|------|
| Understand the project | [`CLAUDE.md`](CLAUDE.md) — build commands, architecture, invariants |
| Learn the language | [`docs/reference/ax-language.md`](docs/reference/ax-language.md) |
| Understand determinism | [`docs/concepts/determinism.md`](docs/concepts/determinism.md) |
| Understand capabilities | [`docs/concepts/capabilities.md`](docs/concepts/capabilities.md) |
| Framework app protocol | [`docs/FRAMEWORK_SPEC.md`](docs/FRAMEWORK_SPEC.md) |
| Effects / LLM integration | [`docs/EFFECTS_GUIDE.md`](docs/EFFECTS_GUIDE.md) |
| Actor system | [`docs/ACTORS_GUIDE.md`](docs/ACTORS_GUIDE.md) |
| Evidence bundles | [`docs/concepts/evidence-bundles.md`](docs/concepts/evidence-bundles.md) |
| All CLI commands | [`docs/reference/cli.md`](docs/reference/cli.md) |

## Directory structure

```
crates/llmbc/        boruna-bytecode    (opcodes, Value, Capability)
crates/llmc/         boruna-compiler    (lexer, parser, typeck, codegen)
crates/llmvm/        boruna-vm          (VM, capability gateway, actors, replay)
crates/llmvm-cli/    boruna-cli         (CLI binary)
crates/llmfw/        boruna-framework   (Elm-architecture runtime)
crates/llm-effect/   boruna-effect      (LLM integration)
crates/boruna-mcp/   boruna-mcp         (MCP server)
orchestrator/        boruna-orchestrator (workflow engine, audit, evidence)
packages/            boruna-pkg         (package registry, resolver)
tooling/             boruna-tooling     (diagnostics, repair, templates)
libs/                standard libraries (std-ui, std-http, std-db, ...)
templates/           app templates      (crud-admin, form-basic, ...)
examples/            runnable examples  (hello.ax, workflows/, ...)
```
