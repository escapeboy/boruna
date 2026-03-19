# CLI Reference

The `boruna` binary is the primary interface for compiling, running, inspecting, and managing `.ax` programs and workflows.

## Installation

```bash
cargo build --workspace
# Binary at: target/debug/boruna
```

To use `boruna` without `cargo run --bin boruna --`, add `target/debug/` to your `PATH` or install with:

```bash
cargo install --path crates/llmvm-cli
```

## Top-level commands

```
boruna <command> [options]

Commands:
  compile     Compile a .ax file to bytecode
  run         Run a .ax file
  trace       Run and emit an execution trace
  replay      Replay from a recorded event log
  inspect     Inspect a compiled module
  ast         Print the AST for a .ax file
  lang        Language diagnostics and repair
  framework   Framework app validation and testing
  workflow    Workflow validation and execution
  evidence    Evidence bundle inspection and verification
  template    Template listing and application
  trace2tests Generate regression tests from traces
```

---

## `boruna compile`

Compile a `.ax` source file to bytecode.

```bash
boruna compile <file.ax>
```

Outputs the compiled module summary (functions, capabilities declared). Does not execute.

---

## `boruna run`

Compile and run a `.ax` file.

```bash
boruna run <file.ax> [options]

Options:
  --policy <name>    Capability policy: allow-all, deny-all (default: deny-all)
  --record           Write an event log to .boruna/runs/<id>/
  --live             Enable real capability handlers (requires http feature)
  --trace            Emit a full execution trace to stdout
  --step-limit <n>   Abort if execution exceeds n steps
```

Examples:

```bash
# Run in demo mode (no capabilities)
boruna run examples/hello.ax

# Run with all capabilities allowed
boruna run app.ax --policy allow-all

# Run with real HTTP (requires --features boruna-cli/http)
cargo run --features boruna-cli/http --bin boruna -- run app.ax --policy allow-all --live
```

---

## `boruna trace`

Run a `.ax` file and emit a step-by-step execution trace.

```bash
boruna trace <file.ax> [--policy <name>]
```

---

## `boruna replay`

Replay execution from a recorded event log.

```bash
boruna replay <file.ax> --from <event-log-path> [--verify]

Options:
  --verify    Compare replay output to recorded output; fail if they differ
```

---

## `boruna inspect`

Inspect a compiled bytecode module.

```bash
boruna inspect <file.ax>
```

Prints: function table, constant pool, declared capabilities, bytecode disassembly.

---

## `boruna ast`

Print the parsed AST for a `.ax` file as JSON.

```bash
boruna ast <file.ax> [--json]
```

---

## `boruna lang`

Language diagnostics and auto-repair.

```bash
boruna lang check <file.ax> [--json]
boruna lang repair <file.ax>

Subcommands:
  check     Run diagnostics: type errors, undeclared capabilities, unreachable code
  repair    Apply auto-repair suggestions from diagnostics
```

Examples:

```bash
# Check for errors with machine-readable output
boruna lang check app.ax --json

# Automatically repair issues
boruna lang repair app.ax
```

---

## `boruna framework`

Validate and test framework apps (Elm-architecture `.ax` apps with init/update/view).

```bash
boruna framework validate <file.ax>
boruna framework test <file.ax> [options]

Options for test:
  -m <messages>    Comma-separated message sequence, e.g. "increment:1,reset:0"
```

Examples:

```bash
boruna framework validate examples/framework/counter_app.ax
boruna framework test examples/framework/counter_app.ax -m "increment:1,increment:1,reset:0"
```

---

## `boruna workflow`

Validate and run workflow DAGs.

```bash
boruna workflow validate <workflow-dir/>
boruna workflow run <workflow-dir/> [options]

Options for run:
  --policy <name>    Capability policy (default: deny-all)
  --record           Write evidence bundle to .boruna/runs/<id>/
  --live             Enable real capability handlers
  --replay <dir>     Replay from an existing evidence bundle
  --verify           (with --replay) Verify outputs match recorded values
```

Examples:

```bash
# Validate DAG structure
boruna workflow validate examples/workflows/llm_code_review

# Run in demo mode
boruna workflow run examples/workflows/llm_code_review --policy allow-all

# Run and record evidence
boruna workflow run examples/workflows/llm_code_review --policy allow-all --record

# Run with real HTTP and LLM calls
cargo run --features boruna-cli/http --bin boruna -- \
  workflow run examples/workflows/llm_code_review \
  --policy allow-all --live --record
```

---

## `boruna evidence`

Inspect and verify evidence bundles.

```bash
boruna evidence inspect <bundle-dir/> [--json]
boruna evidence verify <bundle-dir/>
```

Examples:

```bash
boruna evidence inspect .boruna/runs/20260315-143022-abc4d/
boruna evidence inspect .boruna/runs/20260315-143022-abc4d/ --json
boruna evidence verify .boruna/runs/20260315-143022-abc4d/
```

---

## `boruna template`

List and apply app templates.

```bash
boruna template list
boruna template apply <name> [options]

Options for apply:
  --args <key=value,...>    Template variable substitutions
  --validate                Validate the generated output after applying
```

Examples:

```bash
boruna template list
boruna template apply crud-admin --args "entity_name=products,fields=name|price" --validate
```

Available templates: `crud-admin`, `form-basic`, `auth-app`, `realtime-feed`, `offline-sync`

---

## `boruna trace2tests`

Generate regression tests from execution traces.

```bash
boruna trace2tests <trace-file> --output <test-dir/>
```

See [TRACE_TO_TESTS.md](../TRACE_TO_TESTS.md) for details.

---

## Global options

```
  --help      Print help for any command
  --version   Print the Boruna version
```
