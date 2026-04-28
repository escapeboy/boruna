# MCP Server Tool Reference

The `boruna-mcp` binary exposes Boruna's toolchain to AI coding agents (Claude Code, Cursor, Codex, ...) over a JSON-RPC stdio transport. This page documents the wire contract for every tool — parameter names, types, return shapes, and `error_kind` values.

If you only want to register the server in your IDE, see [`AGENTS.md`](../../AGENTS.md). The Rust source of record is [`crates/boruna-mcp/src/server.rs`](../../crates/boruna-mcp/src/server.rs).

## Quick start

```bash
# Register the server in your IDE's MCP config (e.g. .mcp.json for Claude Code):
{
  "mcpServers": {
    "boruna": {
      "command": "boruna-mcp",
      "args": ["--templates-dir", "/path/to/templates", "--libs-dir", "/path/to/libs"],
      "env": {}
    }
  }
}
```

Both `--templates-dir` and `--libs-dir` are optional. Defaults are `templates` and `libs` relative to the working directory.

## Conventions

The tools below share several conventions:

- **All tools return JSON inside an MCP `Content::text` payload.** Responses are pretty-printed.
- **Every response — success and failure — carries `protocol_version: 1`.** This is the wire-format version of the response envelope. It bumps **only** on a breaking shape change (field rename, removal, type change, or `error_kind` semantics change). Additive changes keep the version. Integrators should reject any response whose `protocol_version` exceeds the version they were built against, and may safely upgrade their parsers when the field stays the same. See the [Stability section](#stability) for the full versioning policy.
- **Domain errors are returned as `success: false` JSON, not as MCP errors.** This includes compile failures, runtime errors, validation errors, parse errors, etc. MCP-protocol errors (returned as `McpError`) are reserved for transport-level problems — most commonly when a `source` argument exceeds the **1 MB limit** enforced by every source-accepting tool.
- **Source code is passed as a string**, not a file path. Tool callers are responsible for reading files themselves.
- **Synchronous Boruna APIs run inside `tokio::task::spawn_blocking`.** That means tool calls don't starve the MCP event loop, but each call is single-threaded.
- **`success: true` responses always include the success flag.** Field shapes after the flag vary per tool — see each section.
- **`error_kind` values are stable strings.** Integrators may switch on them safely. New `error_kind` values may be added in a non-breaking way; existing ones are not renamed.

## Tools

> **Note on the JSON examples below:** for brevity, the per-tool examples show only the body fields. Every actual response — success and failure — also includes `"protocol_version": 1` immediately after `"success"`. See the [Conventions](#conventions) and [Stability](#stability) sections for the full contract.

### `boruna_compile`

Compile `.ax` source code and return module info.

**Parameters**

| Field | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | The `.ax` source code to compile. Max 1 MB. |
| `name` | string | no | Module name. Default `"module"`. |

**Returns (success)**

```json
{
  "success": true,
  "module": {
    "name": "module",
    "version": 1,
    "functions": 3,
    "types": 1,
    "constants": 7,
    "entry": 0
  }
}
```

**Returns (failure)**

```json
{
  "success": false,
  "errors": [
    { "severity": "error", "code": "E001", "message": "...", "line": 10, "col": 5 }
  ]
}
```

Error codes: `E001` (lexer), `E002` (parser), `E008` (codegen), `E009` (typechecker).

---

### `boruna_ast`

Parse `.ax` source code and return the AST as JSON.

**Parameters**

| Field | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | The `.ax` source code to parse. Max 1 MB. |

**Returns (success)**

```json
{
  "success": true,
  "truncated": false,
  "ast": { /* program AST as JSON */ }
}
```

If the AST exceeds 100 KB, the tool returns the truncated string and metadata instead of the parsed object:

```json
{
  "success": true,
  "truncated": true,
  "ast_size": 134217,
  "ast": "..."
}
```

**Returns (failure)** — same `errors` shape as `boruna_compile` (lexer or parser errors only), **or** the following if AST JSON serialization itself fails (rare):

```json
{ "success": false, "error_kind": "serialization_error", "message": "..." }
```

---

### `boruna_run`

Compile and execute `.ax` source code under a capability policy.

**Parameters**

| Field | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | The `.ax` source code to run. Max 1 MB. |
| `policy` | string \| object | no | `"allow-all"` / `"deny-all"` shorthand, **or** a Policy object — see [`policy-schema.md`](./policy-schema.md). Default `"allow-all"`. |
| `max_steps` | integer (u64) | no | VM step ceiling. Default `10000000`. |
| `trace` | boolean | no | Emit an opcode-level execution trace in the response. Default `false`. |

> **There is no `input` parameter.** Script authors interpolate runtime values as literals into the `.ax` source before submission. This keeps the determinism contract clean: the `(source, policy)` tuple fully determines the run.

**Returns (success)**

```json
{
  "success": true,
  "result": <value>,
  "steps": 142,
  "ui_output": [<value>, ...]
}
```

If `trace: true`:

```json
{
  "success": true,
  "result": <value>,
  "steps": 142,
  "ui_output": [],
  "trace": ["op:0", "op:1", ...],
  "trace_truncated": false
}
```

The trace is capped at 500 entries; `trace_truncated: true` indicates the suffix was discarded.

**Returns (failure)**

```json
{ "success": false, "error_kind": "runtime_error",  "message": "...", "steps": 7 }
{ "success": false, "error_kind": "invalid_policy", "message": "..." }
```

Plus the `errors` shape from `boruna_compile` if compilation fails before the run starts.

`error_kind` values: `runtime_error`, `invalid_policy`. Compile failures are returned without an `error_kind` (they use the `errors` array). Exceeding `max_steps` is reported as `runtime_error` with a step-limit message — there is no distinct kind for it.

**Value encoding** — primitives are passed through directly (`Int` → number, `String` → string, `Bool` → bool, `Unit` → `null`). Tagged values use the shapes:

```json
{"option": "None"}                         // Value::None
{"option": "Some", "value": <inner>}       // Value::Some
{"result": "Ok",   "value": <inner>}       // Value::Ok
{"result": "Err",  "value": <inner>}       // Value::Err
{"type": "record", "type_id": 4, "fields": [...]}
{"type": "enum",   "type_id": 6, "variant": "Tag", "payload": <inner>}
{"actor_id": 1}                            // Value::ActorId
{"fn_ref": 12}                             // Value::FnRef
```

`Value::List` becomes a JSON array; `Value::Map` becomes a JSON object.

---

### `boruna_check`

Run diagnostics on `.ax` source.

**Parameters**

| Field | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | The `.ax` source code. Max 1 MB. |
| `file_name` | string | no | Filename used in diagnostic locations. Default `"<source>"`. |

**Returns**

```json
{
  "success": true,
  "file": "<source>",
  "diagnostics_count": 2,
  "diagnostics": [
    {
      "id": "MISSING_MAIN",
      "severity": "error",
      "message": "...",
      "location": { "file": "<source>", "line": 1, "col": 1, "end_line": 1, "end_col": 1 },
      "patches": [
        {
          "id": "add_main",
          "description": "...",
          "confidence": "High",
          "rationale": "..."
        }
      ]
    }
  ]
}
```

`location` and `patches` are present only when applicable. `confidence` is one of `"High"`, `"Medium"`, `"Low"`.

This tool returns `success: true` even when diagnostics are found — diagnostics are *findings*, not failures. Check `diagnostics_count` and per-entry `severity` to react.

---

### `boruna_repair`

Auto-repair `.ax` source using diagnostic patches.

**Parameters**

| Field | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | The `.ax` source code to repair. Max 1 MB. |
| `file_name` | string | no | Filename used in diagnostic locations. Default `"<source>"`. |
| `strategy` | string | no | `"best"` (default) — apply the highest-confidence patch per diagnostic. `"all"` — apply every patch. Ignored if `patch_id` is set. |
| `patch_id` | string | no | Apply only the patch with this ID; sets strategy to `"by_id"`. |

**Returns**

```json
{
  "success": true,
  "repaired_source": "<the modified .ax source>",
  "patches_applied": 2,
  "patches_skipped": 0,
  "applied": [{ "diagnostic_id": "...", "patch_id": "...", "description": "..." }],
  "skipped": [{ "diagnostic_id": "...", "reason": "..." }],
  "verify_passed": true,
  "diagnostics_before": 2,
  "diagnostics_after": 0
}
```

`verify_passed: true` means re-running diagnostics on the repaired source produced no errors. Always inspect `diagnostics_after` before trusting the repair.

---

### `boruna_validate_app`

Validate that `.ax` source conforms to the App protocol (Elm-style `init` / `update` / `view`).

**Parameters**

| Field | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | The `.ax` source code. Max 1 MB. |

**Returns**

```json
{
  "success": true,
  "has_init": true,
  "has_update": true,
  "has_view": true,
  "has_policies": false,
  "state_type": "State",
  "message_type": "Msg",
  "errors": [],
  "valid": true
}
```

`valid: true` when `errors` is empty AND all three of `has_init` / `has_update` / `has_view` are true.

**Returns (failure)** — `success: false`, `error_kind: "framework_error"` if the validator itself crashed; compile errors use the `errors` shape from `boruna_compile`.

---

### `boruna_framework_test`

Run a framework App by sending a sequence of messages.

**Parameters**

| Field | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | The `.ax` framework app source. Max 1 MB. |
| `messages` | string[] | yes | Messages as `"tag:payload"` strings (e.g. `["increment:1", "reset:0"]`). Payloads parse as integer if possible, otherwise string. |

**Returns**

```json
{
  "success": true,
  "init_state": <value>,
  "cycles": [
    { "message": "increment:1", "state": <value>, "effects": 0, "ui_tree": <value or null> },
    ...
  ],
  "final_state": <value>,
  "total_cycles": 2
}
```

If a cycle fails, the tool returns `success: false`, `error_kind: "framework_error"`, and includes the partial `cycles` (with the failing entry containing an `error` field) plus the `init_state` so callers can debug the divergence.

Value formatting in this tool is **brief** (different from `boruna_run` — see source `format_value_brief`): records render as flat field arrays, enums as `{variant, payload}`, options/results as `{Tag: value}`.

---

### `boruna_workflow_validate`

Validate a workflow definition (JSON).

**Parameters**

| Field | Type | Required | Description |
|---|---|---|---|
| `workflow_json` | string | yes | The full `workflow.json` content as a string. |

**Returns**

```json
{
  "success": true,
  "workflow_name": "code_review",
  "workflow_version": "1.0.0",
  "steps_count": 3,
  "edges_count": 2,
  "execution_order": ["fetch_pr", "analyze", "post_comment"]
}
```

**Returns (failure)**

```json
{ "success": false, "error_kind": "parse_error",      "message": "..." }
{ "success": false, "error_kind": "validation_error", "errors": [{ "kind": "Cycle", "message": "..." }] }
```

`error_kind: "parse_error"` means the JSON itself was malformed. `validation_error` means the JSON parsed but the DAG is invalid (cycle, missing step reference, etc.). Each entry in `errors` carries a `kind` (debug-formatted Rust enum variant) and a human-readable `message`.

---

### `boruna_template_list`

List available Boruna app templates.

**Parameters** — none.

**Returns**

```json
{
  "success": true,
  "count": 5,
  "templates": [
    {
      "name": "crud-admin",
      "version": "1.0.0",
      "description": "...",
      "dependencies": ["std-ui", "std-forms"],
      "capabilities": ["db.query"],
      "args": ["entity_name", "fields"]
    }
  ]
}
```

`args` lists only the variable names; consult `template apply` for substitution.

**Returns (failure)** — `success: false`, `error_kind: "template_error"`, `message: ...` (e.g. when the templates dir doesn't exist or contains malformed manifests).

---

### `boruna_template_apply`

Apply a template with variable substitution.

**Parameters**

| Field | Type | Required | Description |
|---|---|---|---|
| `template_name` | string | yes | Template name (e.g. `"crud-admin"`). |
| `args` | string[] | yes | Arguments as `"key=value"` strings (e.g. `["entity_name=products", "fields=name|price"]`). |
| `validate` | boolean | no | Compile the rendered output to verify it parses. Default `false`. |

**Returns**

```json
{
  "success": true,
  "template_name": "crud-admin",
  "output_file": "...",
  "source": "<rendered .ax source>",
  "dependencies": ["std-ui"],
  "capabilities": ["db.query"],
  "validation": { "passed": true }
}
```

The `validation` object is present only when `validate: true`. If validation fails:

```json
"validation": { "passed": false, "error": "..." }
```

**Returns (failure)**

```json
{ "success": false, "error_kind": "invalid_args",   "message": "argument must be key=value format, got: ..." }
{ "success": false, "error_kind": "template_error", "message": "..." }
```

## Limits

- **Source size:** every tool that accepts a `source` parameter rejects payloads above **1 MB** at the MCP layer (returned as an MCP `invalid_params` error, not as JSON). This is enforced in `crates/boruna-mcp/src/server.rs::validate_source`.
- **AST size (boruna_ast):** ASTs above 100 KB are returned truncated as a string (with `truncated: true` and `ast_size`), not as a parsed JSON object.
- **Trace size (boruna_run):** execution traces are capped at 500 entries (`trace_truncated` indicates suffix discarded).
- **Process model:** all tool calls run synchronously inside `spawn_blocking`. The MCP server is single-tenant by design; long-running tool calls block the response, not the event loop.

## Stability

- **`protocol_version: 1`** is the wire-format version of the response envelope, present on every tool response (success and failure). Locked by `crates/boruna-mcp/src/tools/mod.rs::TOOL_RESPONSE_PROTOCOL_VERSION`; a regression test (`protocol_version_tests`) asserts coverage across both success and failure paths of every tool. Bumped only on a breaking shape change anywhere in the envelope; additive changes keep the version.
- **Tool names** (`boruna_compile`, `boruna_run`, ...) are stable. Renames require a major version bump.
- **`error_kind` values** are stable strings. New ones may be added; existing ones are not renamed.
- **Top-level response fields** (`success`, `protocol_version`, named result fields) are stable. New fields are additive.
- **Value encoding inside `result`** (the tagged shapes for `Option` / `Result` / records / enums) is locked.

### Versioning policy for `protocol_version`

| Change | Bump? |
|---|---|
| Adding a new optional response field | **No** |
| Adding a new `error_kind` value | **No** |
| Adding a new tool | **No** |
| Renaming an existing field | **Yes** |
| Removing an existing field | **Yes** |
| Changing the type of an existing field | **Yes** |
| Changing what an existing `error_kind` means | **Yes** |
| Changing the `result` value encoding (Option/Result/record tagged shapes) | **Yes** |

When `protocol_version` bumps, integrators see `protocol_version: 2` (or higher) and can branch on the version to support both shapes during a migration window.

Pairs with [`Policy.schema_version`](./policy-schema.md) (currently `1`) — together they cover both the request-side policy schema and the response-side envelope.

## See also

- [`policy-schema.md`](./policy-schema.md) — the structured `policy` parameter for `boruna_run`
- [`policy.schema.json`](./policy.schema.json) — machine-readable JSON Schema for the same
- [`cli.md`](./cli.md) — the matching `boruna` CLI commands
- [`AGENTS.md`](../../AGENTS.md) — coding-agent-facing integration guide
- Rust source of record: [`crates/boruna-mcp/src/server.rs`](../../crates/boruna-mcp/src/server.rs)
