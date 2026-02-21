# Boruna

[![CI](https://github.com/escapeboy/boruna/actions/workflows/ci.yml/badge.svg)](https://github.com/escapeboy/boruna/actions/workflows/ci.yml)

A deterministic, capability-safe programming language and runtime for building LLM-native applications. Written entirely in Rust.

Boruna compiles `.ax` source files to bytecode, executes them on a custom VM, and enforces that **every side effect is declared and gated** at compile time. The runtime guarantees deterministic execution: same input always produces the same output.

## What Boruna Is

- A **statically typed language** with records, enums, pattern matching, and capability annotations
- A **bytecode VM** with deterministic scheduling, actor system, and replay support
- An **Elm-architecture framework** (`init` / `update` / `view`) for building stateful apps
- A **capability gateway** that intercepts and controls all side effects (network, database, filesystem, LLM calls)
- A **package system** with content-addressed hashing (SHA-256) and deterministic dependency resolution
- A **multi-agent orchestrator** for coordinating parallel work with patch bundles and conflict resolution
- A **developer tooling suite** with diagnostics, auto-repair, trace-to-test generation, and app templates

## What Boruna Is Not

- Not a general-purpose systems language (no manual memory management, no FFI)
- Not a replacement for Python, JavaScript, or Rust for everyday programming
- Not production-ready yet (v0.1.0 — the language and tooling are functional but evolving)
- Not designed for raw performance (determinism and safety are prioritized over speed)
- Not a wrapper around an existing language (it has its own syntax, type system, compiler, and VM)

## Quick Start

```bash
# Build
cargo build --workspace

# Run a program
cargo run --bin boruna -- run examples/hello.ax

# Run all 501 tests
cargo test --workspace
```

### Hello World

```
// hello.ax
fn main() -> Int {
    let x: Int = 40
    let y: Int = 2
    x + y
}
```

```bash
cargo run --bin boruna -- run examples/hello.ax
```

### Fibonacci

```
fn fib(n: Int) -> Int {
    if n <= 1 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn main() -> Int {
    fib(10)
}
```

### Capability-Gated Functions

Functions that perform side effects must declare their capabilities:

```
fn fetch_data(url: String) -> String !{net.fetch} {
    // Only callable when net.fetch is allowed by the active policy
}

fn save_file(path: String, data: String) -> Bool !{fs.write} {
    // Only callable when fs.write is allowed
}

fn pure_add(a: Int, b: Int) -> Int {
    // No annotation needed — this is pure
    a + b
}
```

### Framework App (Elm Architecture)

```
type State { count: Int }
type Msg { tag: String, payload: Int }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }

fn init() -> State {
    State { count: 0 }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    let new_count: Int = if msg.tag == "increment" {
        state.count + 1
    } else {
        if msg.tag == "reset" { 0 } else { state.count }
    }
    UpdateResult {
        state: State { count: new_count },
        effects: [],
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "counter", text: "count" }
}
```

```bash
# Validate app structure
cargo run --bin boruna -- framework validate examples/framework/counter_app.ax

# Test with message sequence
cargo run --bin boruna -- framework test examples/framework/counter_app.ax \
    -m "increment:1,increment:1,reset:0"
```

## Language Features

| Feature | Example |
|---------|---------|
| Static types | `Int`, `Float`, `String`, `Bool`, `Unit` |
| Option / Result | `Option<T>`, `Result<T, E>` |
| Records | `User { name: "Ada", age: 30 }` |
| Record spread | `User { ..old_user, age: 31 }` |
| Enums | `enum Color { Red, Green, Custom(String) }` |
| Pattern matching | `match val { "a" => 1, _ => 0 }` |
| Collections | `List<T>`, `Map<K, V>` |
| Capability annotations | `fn f() -> T !{net.fetch}` |
| Contracts | `fn div(a: Int, b: Int) -> Int requires b != 0` |
| Actors | `spawn`, `send`, `receive` |
| String concat | `"hello" ++ " world"` |

## Architecture

```
                    .ax source
                        |
                    [Compiler]
                  lexer -> parser -> typeck -> codegen
                        |
                    .axbc bytecode
                        |
                      [VM]
             CapabilityGateway + Policy
                        |
            +-----------+-----------+
            |           |           |
        [Framework]  [Actors]  [Effects]
        init/update  spawn     http, db,
        /view        send/recv fs, llm...
```

### Crates

| Crate | Directory | Purpose |
|-------|-----------|---------|
| `boruna-bytecode` | `crates/llmbc` | Opcodes, Module, Value, Capability definitions |
| `boruna-compiler` | `crates/llmc` | Lexer, parser, type checker, code generator |
| `boruna-vm` | `crates/llmvm` | Virtual machine, actor system, replay engine |
| `boruna-framework` | `crates/llmfw` | Elm-architecture runtime, validation, test harness |
| `boruna-effect` | `crates/llm-effect` | LLM integration, prompt management, caching |
| `boruna-cli` | `crates/llmvm-cli` | CLI binary with all subcommands |
| `boruna-tooling` | `tooling` | Diagnostics, auto-repair, trace-to-tests, templates |
| `boruna-pkg` | `packages` | Package registry, resolver, lockfiles |
| `boruna-orchestrator` | `orchestrator` | Multi-agent coordination, patch management |

### Standard Libraries

11 deterministic libraries, all pure-functional with declared capabilities:

| Library | Capabilities | Purpose |
|---------|-------------|---------|
| `std-ui` | none | Declarative UI primitives (row, column, button, table...) |
| `std-forms` | none | Form engine with validation |
| `std-validation` | none | Reusable validation rules |
| `std-authz` | none | Role and permission checks |
| `std-routing` | none | Declarative URL routing |
| `std-testing` | none | Test helpers and assertions |
| `std-http` | `net.fetch` | HTTP request abstractions |
| `std-db` | `db.query` | Database query builder |
| `std-storage` | `fs.read`, `fs.write` | Local persistence |
| `std-notifications` | `time.now` | Notification management |
| `std-sync` | `net.fetch` | Offline-first sync with conflict resolution |

### App Templates

```bash
cargo run --bin boruna -- template list
cargo run --bin boruna -- template apply crud-admin \
    --args "entity_name=products,fields=name|price" --validate
```

| Template | Description |
|----------|-------------|
| `crud-admin` | Full CRUD admin panel with auth and DB |
| `form-basic` | Simple validated form |
| `auth-app` | Authentication flow with role management |
| `realtime-feed` | Live event feed with polling |
| `offline-sync` | Offline-first app with sync queue |

## Key Guarantees

### Determinism

All execution is deterministic. Same bytecode + same messages = same state transitions, same effects, same UI output. The VM uses `BTreeMap` (not `HashMap`) for ordered iteration. Time, randomness, and I/O are virtualized through the capability gateway.

### Capability Safety

Every side effect must be declared with `!{capability}` annotations. The VM's `CapabilityGateway` checks every call against the active `Policy`. Framework apps enforce that `update()` and `view()` are pure — they cannot make capability calls.

### Replay

Execution can be recorded and replayed deterministically. The `EventLog` captures capability call arguments and results. The `ReplayEngine` feeds recorded results back during replay and verifies that the call sequence is identical.

### Actor Determinism

The actor system uses deterministic round-robin scheduling with sorted message delivery `(target_id, sender_id)`. Bounded execution budgets prevent any single actor from starving others.

## CLI Reference

```bash
boruna compile app.ax              # Compile to bytecode
boruna run app.ax                  # Run a program
boruna run app.ax --policy allow-all  # Run with all capabilities enabled
boruna ast app.ax                  # Print AST
boruna trace app.ax                # Run with execution tracing
boruna replay app.ax trace.json    # Replay from recorded trace
boruna inspect app.axbc            # Inspect bytecode module

boruna framework validate app.ax   # Validate app structure
boruna framework test app.ax -m "msg:val,msg:val"  # Test with messages

boruna lang check app.ax --json    # Structured diagnostics
boruna lang repair app.ax          # Auto-repair from diagnostics

boruna template list               # List available templates
boruna template apply <name> --args "k=v"  # Generate from template
```

---

## For LLMs and Coding Agents

Boruna is designed to be understood and operated by LLMs and autonomous coding agents. This section provides the references needed to work with the codebase effectively.

### Entry Points for Agents

| Task | Where to Start |
|------|---------------|
| Understand the project | [`CLAUDE.md`](CLAUDE.md) — build commands, architecture, invariants |
| Learn the language | [`docs/language-guide.md`](docs/language-guide.md) — types, syntax, capabilities |
| Build a framework app | [`docs/FRAMEWORK_API.md`](docs/FRAMEWORK_API.md) — AppRuntime, TestHarness, Effect |
| Write and run tests | [`docs/TESTING_GUIDE.md`](docs/TESTING_GUIDE.md) — TestHarness usage, golden tests, CLI testing |
| Work with effects | [`docs/EFFECTS_GUIDE.md`](docs/EFFECTS_GUIDE.md) — effect lifecycle, capability mapping |
| Use the actor system | [`docs/ACTORS_GUIDE.md`](docs/ACTORS_GUIDE.md) — spawn, send, receive, supervision |
| Integrate LLM calls | [`docs/LLM_EFFECT_SPEC.md`](docs/LLM_EFFECT_SPEC.md) — prompt registry, caching, schemas |
| Understand determinism | [`docs/DETERMINISM_CONTRACT.md`](docs/DETERMINISM_CONTRACT.md) — what is and isn't deterministic |
| Coordinate multi-agent work | [`docs/ORCHESTRATOR_SPEC.md`](docs/ORCHESTRATOR_SPEC.md) — work graphs, patch bundles, review |
| Manage packages | [`docs/PACKAGE_SPEC.md`](docs/PACKAGE_SPEC.md) — manifests, lockfiles, resolution |
| Inspect bytecode | [`docs/bytecode-spec.md`](docs/bytecode-spec.md) — opcodes, module format, binary layout |
| Use diagnostics/repair | [`docs/DIAGNOSTICS_AND_REPAIR.md`](docs/DIAGNOSTICS_AND_REPAIR.md) — structured diagnostics, auto-fix |
| Generate tests from traces | [`docs/TRACE_TO_TESTS.md`](docs/TRACE_TO_TESTS.md) — record, minimize, generate |

### Compiler Pipeline (for code generation agents)

```
Source (.ax)
    → lexer::lex()       → Vec<Token>
    → parser::parse()    → Program (AST)
    → typeck::check()    → Result<(), CompileError>
    → codegen::emit()    → Module (bytecode)
```

Entry point: `boruna_compiler::compile(name, source) -> Result<Module, CompileError>`

### VM Execution (for runtime agents)

```rust
use boruna_compiler::compile;
use boruna_vm::{Vm, CapabilityGateway, Policy};

let module = compile("app", source)?;
let gateway = CapabilityGateway::new(Policy::allow_all());
let mut vm = Vm::new(module, gateway);
let result = vm.run()?;
```

### Framework Testing (for test agents)

```rust
use boruna_framework::testing::TestHarness;
use boruna_framework::runtime::AppMessage;
use boruna_bytecode::Value;

let mut harness = TestHarness::from_source(source)?;
harness.send(AppMessage::new("increment", Value::Int(0)))?;
assert_eq!(harness.state(), &expected);

// Replay verification
let identical = harness.replay_verify(source, messages)?;
```

### Capability List (for policy agents)

| Capability | ID | Gate |
|------------|-----|------|
| `net.fetch` | 0 | HTTP requests |
| `db.query` | 1 | Database queries |
| `fs.read` | 2 | File reads |
| `fs.write` | 3 | File writes |
| `time.now` | 4 | Current time |
| `random` | 5 | Random values |
| `ui.render` | 6 | UI emission |
| `llm.call` | 7 | LLM API calls |
| `actor.spawn` | 8 | Spawn child actors |
| `actor.send` | 9 | Send actor messages |

### Critical Rules for Agents

1. **Never break determinism** — use `BTreeMap`, never `HashMap`. No randomness in pure code.
2. **Declare all capabilities** — functions with side effects need `!{capability}` annotations.
3. **Keep `update()` and `view()` pure** — no capability calls allowed in these functions.
4. **Run `cargo test --workspace`** after every change — 501 tests must pass.
5. **Run `cargo clippy --workspace -- -D warnings`** — zero warnings allowed.

## License

MIT
