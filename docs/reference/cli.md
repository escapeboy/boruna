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
  --watch            Re-run on every change to the file (post-1.0)
```

Examples:

```bash
# Run in demo mode (no capabilities)
boruna run examples/hello.ax

# Run with all capabilities allowed
boruna run app.ax --policy allow-all

# Run with real HTTP (requires --features boruna-cli/http)
cargo run --features boruna-cli/http --bin boruna -- run app.ax --policy allow-all --live

# Watch mode — re-run on every save until Ctrl-C
boruna run app.ax --watch
```

### Watch mode

`--watch` re-executes the file on every change. Filesystem events
are debounced to 200ms, so a single editor save triggers exactly
one rerun even on platforms that emit a flurry of events per save.

A separator line marks each rerun:

```
── reloading app.ax at 14:03:11 ──
3
steps: 4
```

A failed run (compile error or runtime panic) does **not** exit
watch mode — the error is printed and the watcher waits for the
next save. Press `Ctrl-C` to exit.

Watch combines with the standard run flags:
`boruna run app.ax --watch --policy ./policy.json --record events.json`.

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

### `boruna workflow schedule`

Run a workflow on a cron schedule. Loops until SIGINT (Ctrl-C).

```bash
boruna workflow schedule <workflow-dir/> [options]

Options:
  --cron <expr>            5-field cron expression (required), e.g. "*/5 * * * *"
  --policy <name>          Capability policy (default: deny-all)
  --data-dir <dir>         Directory for runs.db and per-run output (default: .boruna/data)
  --max-concurrency <n>    Maximum concurrent runs; skips tick if a run is already active (default: 1)
  --live                   Enable real capability handlers (requires http feature)
```

When a scheduled tick fires while a previous run is still active, that tick is skipped.

### `boruna workflow eval`

Run a workflow against two LLM provider configurations and compare evidence bundles.

```bash
boruna workflow eval <workflow-dir/> --providers-a <file.json> --providers-b <file.json> [options]

Options:
  --providers-a <file>     First provider config JSON file (required)
  --providers-b <file>     Second provider config JSON file (required)
  --runs <n>               Runs per provider (default: 1)
  --data-dir <dir>         Directory for evidence bundles
  --json                   Machine-readable output
```

Reports per-step output agreement and timing differences between the two provider sets.

### `boruna workflow find`

Recursively discover `workflow.json` files under a directory tree.

```bash
boruna workflow find [dir] [--json]
```

Validates each discovered workflow and prints path, name, step count, and validity. `dir` defaults to the current directory. Use `--json` for a JSON array.

---

## `boruna evidence`

Inspect, verify, and manage evidence bundles.

```bash
boruna evidence create <run-id> --output-dir <dir> [--data-dir <dir>]
boruna evidence verify <bundle-dir/> [--bundle-encryption-key <hex>]
boruna evidence inspect <bundle-dir/> [--json] [--decrypt] [--bundle-encryption-key <hex>]
boruna evidence diff <bundle-a> <bundle-b> [--json]
boruna evidence gc-blobs [--data-dir <dir>] [--dry-run] [--json]
boruna evidence serve <bundle-dir/> [--port <port>]
boruna evidence rotate-kek <target> --old-kek <hex> --new-kek <hex> [options]
```

Examples:

```bash
# Build a bundle from a completed run
boruna evidence create abc123def456 --output-dir ./bundles

# Inspect a bundle
boruna evidence inspect .boruna/runs/20260315-143022-abc4d/
boruna evidence inspect .boruna/runs/20260315-143022-abc4d/ --json

# Verify a bundle
boruna evidence verify .boruna/runs/20260315-143022-abc4d/
```

### evidence inspect

For plaintext (non-encrypted) bundles, `evidence inspect` automatically shows step output content from `outputs/<step_id>/result.json`. Each step output is truncated at 500 characters in text mode. With `--json`, the full parsed content appears under the `"step_outputs"` key. Encrypted bundles print a hint to stderr if `--decrypt` / `--bundle-encryption-key` is not supplied.

### evidence create

Build an evidence bundle from a persisted run. Reads the run's metadata, step checkpoints, and hash-chained audit log; writes a bundle directory with `workflow.json`, `policy.json`, per-step outputs, `audit_log.json`, `env_fingerprint.json`, and `manifest.json`. Bundles are created on demand — the runner does not auto-create them.

```bash
boruna evidence create <run-id> --output-dir <dir> [--data-dir <dir>]
```

The bundle is written to `<output-dir>/<run-id>/`.

### evidence diff

Compare two evidence bundles side-by-side.

```
boruna evidence diff <bundle-a> <bundle-b> [--json]
```

Reports differences in workflow metadata, step outputs, audit event counts, and verification status. Use `--json` for machine-readable output.

Examples:

```bash
boruna evidence diff .boruna/runs/run-baseline/ .boruna/runs/run-rerun/
boruna evidence diff baseline/ rerun/ --json
```

### evidence gc-blobs

Sweep orphaned content-addressed blobs from the data directory.

```bash
boruna evidence gc-blobs [--data-dir <dir>] [--dry-run] [--json]
```

An orphan is a blob file no longer referenced by any run checkpoint. Reports `{deleted, skipped, bytes_freed}`. Use `--dry-run` to report without deleting.

### evidence serve

Start a local web UI to browse an evidence bundle.

```bash
boruna evidence serve <bundle-dir/> [--port <port>]
```

Serves a read-only inspector at `http://localhost:<port>` (default: 4444). Requires the `serve` feature:

```bash
cargo run --features boruna-cli/serve --bin boruna -- evidence serve ./bundles/my-run/
```

### evidence rotate-kek

Rotate the key-encryption key (KEK) on one or more encrypted bundles without re-encrypting file content.

```bash
boruna evidence rotate-kek <target> --old-kek <hex> --new-kek <hex> [options]

Options:
  --kek-id-from <id>     Only rotate bundles whose current kek_id matches (safety check)
  --kek-id-to <id>       kek_id written to the rotated manifest (default: "default")
  --dry-run              Print planned actions without modifying any bundle
  --parallelism <n>      Parallel bundle limit in batch mode (default: min(8, num_cpus))
```

`<target>` may be a single bundle directory or a parent directory whose immediate subdirectories are bundles (batch mode).

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
