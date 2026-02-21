# feat: Boruna MCP Server Plugin for AI Coding Agents

## Overview

Build an MCP (Model Context Protocol) server that exposes Boruna's compiler, VM, diagnostics, framework, workflow, and template capabilities as structured tools for AI coding agents (Claude Code, Cursor, Windsurf, VS Code Copilot).

The server uses the official Rust MCP SDK (`rmcp` v0.16) and communicates via stdio transport. Agents can compile `.ax` code, run programs, get diagnostics, auto-repair, validate apps, execute workflows, and apply templates — all without parsing CLI text output.

## Problem Statement

Currently an AI agent working with Boruna must:
1. Shell out to `boruna` CLI and parse text output
2. Guess command flags and argument formats
3. Lose structured error information (spans, severities, suggestions)
4. Cannot discover available capabilities programmatically

An MCP server solves all of these: agents get typed JSON schemas, structured responses, and tool discovery via `tools/list`.

## Technical Approach

### SDK Choice: `rmcp` v0.16.0

- Official Rust SDK: `modelcontextprotocol/rust-sdk` (3,000+ stars)
- Proc macros: `#[tool_router]`, `#[tool]`, `#[tool_handler]`
- Stdio transport via `rmcp::transport::stdio`
- JSON Schema generation via `schemars`
- Async with Tokio

### Architecture

```
┌─────────────────────────────────────┐
│  AI Agent (Claude Code / Cursor)    │
│  stdin/stdout JSON-RPC              │
└──────────────┬──────────────────────┘
               │
┌──────────────▼──────────────────────┐
│  boruna-mcp (new crate)             │
│                                     │
│  BorumaMcpServer {                  │
│    tool_router: ToolRouter<Self>    │
│  }                                  │
│                                     │
│  Tools:                             │
│    boruna_compile                   │
│    boruna_run                       │
│    boruna_check                     │
│    boruna_repair                    │
│    boruna_validate_app              │
│    boruna_ast                       │
│    boruna_inspect                   │
│    boruna_framework_test            │
│    boruna_workflow_validate          │
│    boruna_workflow_run              │
│    boruna_template_list             │
│    boruna_template_apply            │
│                                     │
│  Resources:                         │
│    boruna://language-reference      │
│    boruna://stdlib/{lib_name}       │
│                                     │
│  Prompts:                           │
│    new_app                          │
│    debug_error                      │
└──────────────┬──────────────────────┘
               │ direct Rust calls (no subprocess)
┌──────────────▼──────────────────────┐
│  boruna-compiler                    │
│  boruna-vm                          │
│  boruna-tooling                     │
│  boruna-framework                   │
│  boruna-orchestrator                │
│  boruna-bytecode                    │
└─────────────────────────────────────┘
```

### New Crate: `crates/boruna-mcp/`

```
crates/boruna-mcp/
├── Cargo.toml
├── src/
│   ├── main.rs          # entry point: stdio transport
│   ├── server.rs        # BorunaMcpServer struct + ServerHandler impl
│   ├── tools/
│   │   ├── mod.rs
│   │   ├── compile.rs   # boruna_compile, boruna_ast, boruna_inspect
│   │   ├── run.rs       # boruna_run
│   │   ├── check.rs     # boruna_check, boruna_repair
│   │   ├── framework.rs # boruna_validate_app, boruna_framework_test
│   │   ├── workflow.rs  # boruna_workflow_validate, boruna_workflow_run
│   │   └── template.rs  # boruna_template_list, boruna_template_apply
│   ├── resources.rs     # MCP resources (language ref, stdlib sources)
│   └── prompts.rs       # MCP prompts (new_app, debug_error)
```

## Implementation Phases

### Phase 1: Core Server + 5 Essential Tools

The MVP covering the basic development loop.

**Files to create:**
- `crates/boruna-mcp/Cargo.toml`
- `crates/boruna-mcp/src/main.rs`
- `crates/boruna-mcp/src/server.rs`
- `crates/boruna-mcp/src/tools/mod.rs`
- `crates/boruna-mcp/src/tools/compile.rs`
- `crates/boruna-mcp/src/tools/run.rs`
- `crates/boruna-mcp/src/tools/check.rs`

**Files to modify:**
- `Cargo.toml` (workspace members)

#### 1.1 Cargo.toml

```toml
[package]
name = "boruna-mcp"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "boruna-mcp"
path = "src/main.rs"

[dependencies]
boruna-bytecode = { path = "../llmbc" }
boruna-compiler = { path = "../llmc" }
boruna-vm = { path = "../llmvm" }
boruna-tooling = { path = "../../tooling" }

rmcp = { version = "0.16", features = ["server", "transport-io", "macros"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1.0"
```

#### 1.2 Entry Point (main.rs)

```rust
use rmcp::{ServiceExt, transport::stdio};

mod server;
mod tools;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = server::BorunaMcpServer::new()
        .serve(stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
```

#### 1.3 Server Struct (server.rs)

```rust
use rmcp::{
    ServerHandler, model::*, handler::server::tool::ToolRouter,
    tool_handler, tool_router, ErrorData as McpError,
};

#[derive(Clone)]
pub struct BorunaMcpServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl BorunaMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    // tools defined via #[tool] in tools/*.rs, imported here
}

#[tool_handler]
impl ServerHandler for BorunaMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation {
                name: "boruna-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "Boruna MCP server — compile, run, diagnose, and manage .ax programs"
                    .to_string()
            ),
        }
    }
}
```

#### 1.4 Tool: boruna_compile

Input: `{ source: String, name?: String }`
Output: JSON with functions count, types count, constants count, or compile errors with spans.

Calls: `boruna_compiler::compile(name, &source)`

#### 1.5 Tool: boruna_run

Input: `{ source: String, policy?: "allow-all"|"deny-all", max_steps?: u64 }`
Output: JSON with result value, UI output, step count, or runtime error.

Calls: `boruna_compiler::compile()` then `Vm::new(module, gateway).run()`

#### 1.6 Tool: boruna_check

Input: `{ source: String, file_name?: String }`
Output: JSON array of diagnostics, each with severity, message, span (line, col, len), suggestions.

Calls: `DiagnosticCollector::new(file, &source).collect()` then `.to_json()`

#### 1.7 Tool: boruna_repair

Input: `{ source: String, file_name?: String, strategy?: "best"|"all" }`
Output: JSON with repaired source, applied patches, diagnostics before/after counts.

Calls: `DiagnosticCollector` + `RepairTool::repair()`

#### 1.8 Tool: boruna_ast

Input: `{ source: String }`
Output: Full AST as JSON (already serializable via serde).

Calls: `boruna_compiler::lexer::lex()` + `boruna_compiler::parser::parse()` + `serde_json::to_string_pretty()`

### Phase 2: Framework & Workflow Tools

**Files to create:**
- `crates/boruna-mcp/src/tools/framework.rs`
- `crates/boruna-mcp/src/tools/workflow.rs`
- `crates/boruna-mcp/src/tools/template.rs`

#### 2.1 Tool: boruna_validate_app

Input: `{ source: String }`
Output: JSON with has_init, has_update, has_view, has_policies, state_type, message_type.

Calls: `lexer::lex()` + `parser::parse()` + `AppValidator::validate()`

#### 2.2 Tool: boruna_framework_test

Input: `{ source: String, messages: [{ tag: String, payload: String }] }`
Output: JSON with init_state, cycles (each with state, effects), final_state, view.

Calls: `TestHarness::from_source()` + `harness.send(msg)` loop

#### 2.3 Tool: boruna_workflow_validate

Input: `{ workflow_json: String }`
Output: JSON with validity, step count, edge count, execution order.

Calls: `serde_json::from_str::<WorkflowDef>()` + `WorkflowValidator::validate()` + `topological_order()`

#### 2.4 Tool: boruna_workflow_run

Input: `{ workflow_json: String, source_files: Map<String, String>, policy?: String }`
Output: JSON with run_id, status, step results, duration.

Calls: `WorkflowRunner::run()` (needs temp dir for source files)

#### 2.5 Tool: boruna_template_list

Input: `{ templates_dir?: String }`
Output: JSON array of templates with name, version, description, deps, capabilities.

Calls: `boruna_tooling::templates::list_templates()`

#### 2.6 Tool: boruna_template_apply

Input: `{ template_name: String, args: Map<String, String>, templates_dir?: String, validate?: bool }`
Output: JSON with generated source, template name, deps, capabilities.

Calls: `boruna_tooling::templates::apply_template()`

### Phase 3: Resources & Prompts

**Files to create:**
- `crates/boruna-mcp/src/resources.rs`
- `crates/boruna-mcp/src/prompts.rs`

#### 3.1 Resources

MCP resources are read-only data the agent can pull:

- `boruna://language-reference` — language syntax quick reference (types, capabilities, patterns)
- `boruna://stdlib/{lib_name}` — source code of std-ui, std-forms, etc. from `libs/` directory

Implement via `list_resources()` + `read_resource()` on `ServerHandler`.

#### 3.2 Prompts

Reusable prompt templates agents can use:

- `new_app` — generates a boilerplate framework app with given State/Msg types
- `debug_error` — takes an error message and source, returns a diagnostic prompt

Implement via `list_prompts()` + `get_prompt()` on `ServerHandler`.

### Phase 4: Distribution & Configuration

#### 4.1 Installation

```bash
# From source
cargo install --path crates/boruna-mcp

# From crates.io (after publish)
cargo install boruna-mcp
```

#### 4.2 Claude Code Configuration

Add to `.claude/settings.json` or project `.mcp.json`:

```json
{
  "mcpServers": {
    "boruna": {
      "command": "boruna-mcp",
      "args": []
    }
  }
}
```

#### 4.3 Cursor Configuration

Add to `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "boruna": {
      "command": "boruna-mcp",
      "args": []
    }
  }
}
```

#### 4.4 VS Code (Copilot) Configuration

Add to `.vscode/mcp.json`:

```json
{
  "servers": {
    "boruna": {
      "type": "stdio",
      "command": "boruna-mcp",
      "args": []
    }
  }
}
```

## Acceptance Criteria

### Functional Requirements

- [ ] `boruna-mcp` binary starts and responds to MCP `initialize` handshake
- [ ] `tools/list` returns all registered tools with JSON schemas
- [ ] `boruna_compile` compiles valid source and returns structured module info
- [ ] `boruna_compile` returns structured errors with line/col spans for invalid source
- [ ] `boruna_run` executes source and returns result value
- [ ] `boruna_check` returns diagnostics array with spans and suggestions
- [ ] `boruna_repair` returns repaired source with applied patches report
- [ ] `boruna_ast` returns serialized AST
- [ ] `boruna_validate_app` validates App protocol conformance
- [ ] `boruna_framework_test` runs message sequences and returns state transitions
- [ ] `boruna_workflow_validate` validates workflow DAGs
- [ ] `boruna_template_list` and `boruna_template_apply` work with templates/ dir
- [ ] Works with Claude Code, Cursor, and VS Code Copilot

### Non-Functional Requirements

- [ ] All tools return JSON (no text-only output)
- [ ] Errors are MCP-compliant (McpError with code and message)
- [ ] No subprocess spawning — calls library functions directly
- [ ] Server handles malformed input gracefully (no panics)
- [ ] `cargo test -p boruna-mcp` passes
- [ ] `cargo clippy -p boruna-mcp -- -D warnings` is clean

### Quality Gates

- [ ] Tests for each tool (valid input, invalid input, edge cases)
- [ ] Integration test: initialize → tools/list → call_tool round-trip
- [ ] CI job added to `.github/workflows/ci.yml`

## Design Decisions (from SpecFlow analysis)

### Error Convention

Domain errors (compile failures, validation failures, runtime errors) are returned as **successful tool responses** with `{ "success": false, "error": {...} }` — NOT as MCP `ErrorData`. MCP-level errors are reserved for protocol/infrastructure failures (invalid params, server bugs). This lets agents parse domain errors without exception handling.

### Response Schema: `boruna_compile`

```json
// Success
{ "success": true, "module": { "name": "...", "functions": 5, "types": 3, "constants": 2, "entry": 0 } }

// Error — uses same shape as boruna_check diagnostics
{ "success": false, "errors": [{ "severity": "error", "message": "...", "line": 3, "col": 5, "span_len": 8 }] }
```

### Response Schema: `boruna_run`

```json
// Success
{ "success": true, "result": "42", "ui_output": [...], "steps": 1234, "event_log_summary": { "cap_calls": 2, "actor_spawns": 0 } }

// Step limit
{ "success": false, "error_kind": "step_limit_exceeded", "steps_used": 10000000, "max_steps": 10000000 }

// Runtime error
{ "success": false, "error_kind": "runtime_error", "message": "capability denied: net.fetch", "steps_used": 45 }
```

### Response Schema: `boruna_framework_test` (partial results on error)

```json
{
  "init_state": "State { value: 0 }",
  "cycles": [
    { "cycle": 1, "tag": "increment", "state_after": "State { value: 1 }", "effects": [] }
  ],
  "error": { "cycle": 2, "message": "..." },
  "final_state": "State { value: 1 }",
  "view": "UINode { tag: \"text\", text: \"1\" }"
}
```

### Blocking Operations: `tokio::task::spawn_blocking`

All VM execution (`boruna_run`, `boruna_framework_test`, `boruna_workflow_run`) MUST use `tokio::task::spawn_blocking` because the compiler, VM, and framework are synchronous. A 10M-step program blocking a Tokio thread would starve concurrent MCP calls.

### `boruna_repair` — expose `patch_id` for targeted repairs

```json
{ "source": "...", "strategy?: "best"|"all", "patch_id?: "P001" }
```

If `patch_id` is set, uses `RepairStrategy::ById`, overriding `strategy`.

### `boruna_run` — add `trace: bool` option

When `trace: true`, sets `vm.trace_enabled = true` and includes the opcode trace in the response. Defaults to `false`.

### `boruna_ast` — 100KB size limit

If serialized AST exceeds 100KB, return `{ "success": true, "truncated": true, "ast": "<first 100KB>..." }`.

### Server Configuration

Accept CLI args for runtime paths:

```
boruna-mcp [--templates-dir <path>] [--libs-dir <path>]
```

IDE configurations pass these:

```json
{
  "mcpServers": {
    "boruna": {
      "command": "boruna-mcp",
      "args": ["--templates-dir", "./templates", "--libs-dir", "./libs"]
    }
  }
}
```

### `boruna_workflow_run` — file mapping protocol

Keys in `source_files` MUST be the exact relative paths referenced in the workflow JSON steps' `source` fields. The server creates a temp dir, reconstructs the directory structure (e.g., `steps/analyze.ax` creates `<tempdir>/steps/analyze.ax`), writes `workflow.json` to the temp dir, and sets `RunOptions.workflow_dir` to the temp dir path. Temp dir is cleaned up after the call via RAII (`tempfile::tempdir()`).

### ApprovalGate — documented limitation

`boruna_workflow_run` returns `{ "status": "paused", "run_id": "...", "paused_at_step": "..." }` when hitting an ApprovalGate. No resume tool exists. This is documented in the tool description: "Workflows with approval_gate steps will return a paused status. Manual approval is not supported through MCP."

### Crate Directory: `crates/boruna-mcp/`

Uses the new naming convention (not `crates/llmmcp/`). This is the first crate to use the new name directly. Update `docs/RENAMING.md` accordingly.

## Deferred to Future Phases

| Tool | Reason | Phase |
|---|---|---|
| `boruna_trace` | Opcode-level debugging — nice-to-have, covered partially by `boruna_run --trace` | Phase 5 |
| `boruna_replay` | Determinism verification — important but needs event log persistence | Phase 5 |
| `boruna_evidence_verify/inspect` | Compliance verification after workflow runs | Phase 5 |
| `boruna_trace2tests_*` | Regression test generation — advanced tooling | Phase 6 |
| `boruna_pkg_*` | Package management tools | Phase 6 |
| `boruna_stdlib_check` | Validate library imports against local libs | Phase 5 |

## Dependencies & Risks

**Dependencies:**
- `rmcp` 0.16+ (stable, official SDK)
- `schemars` 1.0 (for JSON Schema generation)
- `tokio` 1.x (already used by the project for `serve` feature)
- `tempfile` (already in workspace, for workflow temp dirs)
- `clap` (for server CLI args `--templates-dir`, `--libs-dir`)

**Risks:**
- `rmcp` API may change — pin to `0.16` initially
- `boruna_ast` on large programs — mitigated by 100KB cap
- `boruna_workflow_run` temp dir disk exhaustion under concurrent load — mitigate with max 5 concurrent workflow runs
- `Vm`, `CapabilityGateway`, `TestHarness` must be `Send` for `spawn_blocking` — verify before implementing
- Source code size — enforce 1MB max input at MCP layer

## References

### Internal

- CLI entry point: `crates/llmvm-cli/src/main.rs`
- Serve feature (server pattern): `crates/llmvm-cli/src/serve.rs`
- Compiler public API: `boruna_compiler::compile()` in `crates/llmc/src/lib.rs`
- Diagnostics: `tooling/src/diagnostics/collector.rs`
- Repair: `tooling/src/repair/mod.rs`
- App validator: `crates/llmfw/src/validate.rs`
- Test harness: `crates/llmfw/src/testing.rs`
- Workflow runner: `orchestrator/src/workflow/runner.rs`
- Templates: `tooling/src/templates/mod.rs`
- Stdlib runner: `tooling/src/stdlib/mod.rs`

### External

- [rmcp SDK](https://github.com/modelcontextprotocol/rust-sdk) — official Rust MCP SDK
- [MCP Specification](https://modelcontextprotocol.io/specification/2025-06-18) — protocol spec
- [Shuttle MCP Guide](https://www.shuttle.dev/blog/2025/07/18/how-to-build-a-stdio-mcp-server-in-rust) — stdio server tutorial
- [MCPB Bundles](https://github.com/modelcontextprotocol/mcpb) — packaging format for Claude Desktop
