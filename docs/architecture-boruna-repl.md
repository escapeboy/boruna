# Architecture — `boruna repl`

Companion to `docs/design-boruna-repl.md`. Implementation deferred past this sprint;
this doc fixes the shape so the next sprint can pick it up cleanly.

## Component map

| Component | Location | Role |
|---|---|---|
| `repl.rs` driver | `crates/llmvm-cli/src/repl.rs` | Top-level entry point, prompt loop |
| `ReplSession` | same | Holds persistent `Vm`, current `Module`, capability gateway |
| Meta-command parser | `crates/llmvm-cli/src/repl/meta.rs` | Parses `:load`, `:type`, `:reset`, etc. |
| Expression compiler | reuses `boruna_compiler` | Compiles a single expression as anonymous thunk |
| Pretty printer | reuses `boruna_bytecode::Value::pretty` | Already exists |
| Line editing | `rustyline` (new dep) | History, basic line editing |
| CLI entry | `crates/llmvm-cli/src/main.rs::Command::Repl` | clap subcommand |

## Data flow

```
User input line
  ↓
Line classifier (starts with ':' → meta, else → expression)
  ↓
[expression path]                    [meta path]
  ↓                                    ↓
boruna_compiler::compile_expr_in_ctx  meta::dispatch(:cmd, args)
  ↓                                    ↓
Module ⊕ thunk function                Update ReplSession state
  ↓
Vm.execute_bounded(thunk_entry, budget)
  ↓
Value::pretty → stdout
ReplSession.last_value = value
```

## File map (new files)

- `crates/llmvm-cli/src/repl.rs` (~250 lines)
- `crates/llmvm-cli/src/repl/meta.rs` (~150 lines)
- `crates/llmvm-cli/src/repl/session.rs` (~100 lines)
- `crates/llmvm-cli/Cargo.toml` — add `rustyline = { version = "14", default-features = false, features = ["with-file-history"] }`

## Compiler extension required

`boruna_compiler::compile(name, source) -> Module` exists. The REPL needs
`compile_expr_in_context(expr_source, module: &Module) -> Result<(Module, FunctionIndex)>`.
The new function:
1. Lexes the expression.
2. Wraps it in a synthetic `fn __repl_eval_<n>() -> a { <expr> }`.
3. Re-parses against the existing module's symbol table.
4. Typechecks against the module's inference context.
5. Emits an extended module with the new function appended.
6. Returns the function index.

The persistent `Vm` is dropped+recreated only on `:reset`. Within a session,
`vm.module_mut()` extends the module's constant pool and function table.

**Risk identified:** `Vm` may have internal invariants assuming the module is frozen at
construction time. Architecture phase identifies this as the first integration risk —
must verify in spike before committing build sprint scope.

## CLI surface

```
boruna repl [FILE]
  Start an interactive REPL.

  FILE  Optional .ax file to load at startup.

Options:
  --policy <name>      Capability policy (default: deny-all in REPL)
  --no-banner          Skip the welcome banner
  --history-file <p>   Override default history file location
```

## Meta-commands

| Command | Behavior |
|---|---|
| `:load <file>` | Reload module from file (replaces module, preserves VM unless `:reset`) |
| `:reload` | `:load` last-loaded file |
| `:type <expr>` | Print the type of `<expr>` without evaluating |
| `:reset` | New VM, new capability gateway, last-loaded module kept |
| `:step <action>` | For framework apps: dispatch one `Msg` and show new `State` |
| `:env` | List bindings in scope |
| `:help` | Show meta-command help |
| `:quit` / `:q` / Ctrl-D | Exit with 0 |

## Determinism / EventLog integration

REPL invocations emit an `EventLog::ReplEval { line_no, expr_hash, value_hash }` entry.
Per `project-conventions-2026-04` §15, these entries are **operational-only**: never feed
hash chains, never compared in replay verification. Documented in the doc-comment on
`Event::ReplEval`.

## Open architecture questions resolved

- **Symbol shadowing on `:load`:** new module fully replaces the old. No incremental merging.
  `:reset` is the only way to wipe VM state.
- **Where typechecker context lives:** `ReplSession` holds an `Arc<InferenceContext>` shared
  with the loaded module.
- **Pretty-printing depth limit:** existing `Value::pretty(max_depth=8, max_len=100)`. No
  REPL-specific override; matches `boruna evidence inspect`.

## Test plan reference

Test plan lives at `docs/test-plan-boruna-repl.md` (not written this sprint; deferred).
