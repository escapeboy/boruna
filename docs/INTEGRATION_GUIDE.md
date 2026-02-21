# Using Boruna in Existing Projects

This guide explains how to integrate Boruna into existing applications. Boruna is designed as an embeddable platform — you can use its compiler, VM, framework, and tooling as Rust crate dependencies within your own projects.

## Table of Contents

- [Overview](#overview)
- [Adding Boruna as a Dependency](#adding-boruna-as-a-dependency)
- [Use Case 1: Embedded Scripting with Capability Gating](#use-case-1-embedded-scripting-with-capability-gating)
- [Use Case 2: Deterministic LLM Integration](#use-case-2-deterministic-llm-integration)
- [Use Case 3: Stateful UI Components (Elm Architecture)](#use-case-3-stateful-ui-components-elm-architecture)
- [Use Case 4: Safe Plugin / Extension System](#use-case-4-safe-plugin--extension-system)
- [Use Case 5: Multi-Agent Orchestration](#use-case-5-multi-agent-orchestration)
- [Use Case 6: Trace-Based Testing](#use-case-6-trace-based-testing)
- [Capability Policies](#capability-policies)
- [Standard Libraries](#standard-libraries)
- [API Quick Reference](#api-quick-reference)

## Overview

Boruna is not just a standalone language — it is a set of composable Rust crates. You can embed any layer into an existing Rust application:

| What you need | Crate(s) to use |
|---|---|
| Compile and run `.ax` scripts | `boruna-compiler` + `boruna-vm` |
| Sandboxed scripting with controlled side effects | `boruna-compiler` + `boruna-vm` (with custom `Policy`) |
| Elm-architecture stateful apps | `boruna-framework` |
| Deterministic LLM calls with caching and replay | `boruna-effect` |
| Multi-agent task coordination | `boruna-orchestrator` |
| Diagnostics, auto-repair, test generation | `boruna-tooling` |
| Package management for `.ax` modules | `boruna-pkg` |

## Adding Boruna as a Dependency

Add the crates you need to your `Cargo.toml`:

```toml
[dependencies]
# Core: compile and run .ax programs
boruna-compiler = { path = "../boruna/crates/llmc" }
boruna-vm = { path = "../boruna/crates/llmvm" }
boruna-bytecode = { path = "../boruna/crates/llmbc" }

# Framework: Elm-architecture apps
boruna-framework = { path = "../boruna/crates/llmfw" }

# LLM integration
boruna-effect = { path = "../boruna/crates/llm-effect" }

# Tooling: diagnostics, repair, templates
boruna-tooling = { path = "../boruna/tooling" }

# Orchestration: multi-agent coordination
boruna-orchestrator = { path = "../boruna/orchestrator" }

# Packages: dependency management
boruna-pkg = { path = "../boruna/packages" }
```

When Boruna is published to crates.io, replace `path` with `version`:

```toml
[dependencies]
boruna-compiler = "0.1"
boruna-vm = "0.1"
```

## Use Case 1: Embedded Scripting with Capability Gating

Run user-provided `.ax` scripts inside your application with full control over what side effects are allowed.

### Compile and Run a Script

```rust
use boruna_compiler::compile;
use boruna_vm::{Vm, CapabilityGateway, Policy};

fn run_user_script(source: &str) -> Result<boruna_bytecode::Value, String> {
    // Compile .ax source to bytecode
    let module = compile("user_script", source)
        .map_err(|e| format!("Compile error: {:?}", e))?;

    // Create a VM with a restrictive policy (deny all side effects)
    let gateway = CapabilityGateway::new(Policy::deny_all());
    let mut vm = Vm::new(module, gateway);

    // Execute and return the result
    vm.run().map_err(|e| format!("Runtime error: {:?}", e))
}
```

### Allow Specific Capabilities

```rust
use boruna_vm::{CapabilityGateway, Policy};

// Allow only HTTP requests and time access
let policy = Policy::default()
    .allow("net.fetch")
    .allow("time.now");

let gateway = CapabilityGateway::new(policy);
let mut vm = Vm::new(module, gateway);
```

### Example: Config-Driven Business Rules

Embed Boruna as a rules engine where business logic is written in `.ax` files and loaded at runtime:

```rust
use boruna_compiler::compile;
use boruna_vm::{Vm, CapabilityGateway, Policy};
use boruna_bytecode::Value;

/// Evaluate a pricing rule written in Boruna
fn evaluate_pricing(rule_source: &str) -> Result<Value, String> {
    let module = compile("pricing_rule", rule_source)
        .map_err(|e| format!("{:?}", e))?;

    // Business rules are pure — no side effects needed
    let gateway = CapabilityGateway::new(Policy::deny_all());
    let mut vm = Vm::new(module, gateway);
    vm.run().map_err(|e| format!("{:?}", e))
}

// The .ax rule file:
// fn main() -> Int {
//     let base_price: Int = 100
//     let discount: Int = 15
//     base_price - discount
// }
```

Benefits:
- Rules can be updated without recompiling your Rust application.
- `Policy::deny_all()` guarantees rules cannot access the network, filesystem, or database.
- Execution is deterministic — same rule always produces the same result.

## Use Case 2: Deterministic LLM Integration

Use Boruna's effect system to add LLM calls to your application with built-in caching, replay, and budget enforcement.

### How It Works

LLM calls in Boruna are **effects**, not direct API calls. Your `.ax` code declares what it wants from the LLM, and the framework runtime executes the call through the capability gateway.

```
update(state, msg) -> (new_state, [Effect::LlmCall { ... }])
   |
   v  framework executes the effect
   |
   v  result delivered as:
update(new_state, Msg { tag: "llm_result", payload: result })
```

### Example: LLM-Powered Feature in .ax

```ax
type State { status: String, last_result: String }
type Msg { tag: String, payload: String }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }

fn update(state: State, msg: Msg) -> UpdateResult {
    match msg.tag {
        "analyze" => {
            UpdateResult {
                state: State { status: "waiting", last_result: state.last_result },
                effects: [
                    Effect {
                        kind: "llm_call",
                        payload: "analysis.summarize",
                        callback_tag: "llm_result",
                    },
                ],
            }
        },
        "llm_result" => {
            UpdateResult {
                state: State { status: "done", last_result: msg.payload },
                effects: [],
            }
        },
        _ => { UpdateResult { state: state, effects: [] } },
    }
}
```

### Replay Safety

Every LLM call is logged to the `EventLog`. During replay, the recorded response is returned instead of calling the LLM again. This guarantees:

- **Deterministic tests** — no flaky results from LLM variance.
- **Cost control** — cached results avoid duplicate API calls.
- **Audit trail** — every LLM interaction is recorded and reproducible.

### Policy Budgets

Control LLM usage with policy:

```json
{
  "allowed_capabilities": ["llm.call"],
  "llm_policy": {
    "total_token_budget": 10000,
    "max_calls": 50,
    "allowed_models": ["default"],
    "max_context_bytes": 65536,
    "prompt_allowlist": ["analysis.summarize", "refactor.extract_fn"]
  }
}
```

See [LLM_EFFECT_SPEC.md](LLM_EFFECT_SPEC.md) for full details on prompts, caching, and schemas.

## Use Case 3: Stateful UI Components (Elm Architecture)

Use Boruna's framework to embed stateful, testable UI components in your application.

### From Rust

```rust
use boruna_framework::runtime::{AppRuntime, AppMessage};
use boruna_framework::testing::TestHarness;
use boruna_compiler::compile;
use boruna_bytecode::Value;

// Load and run a framework app
let source = std::fs::read_to_string("components/counter.ax").unwrap();
let module = compile("counter", &source).unwrap();
let mut runtime = AppRuntime::new(module).unwrap();

// Send messages
let (state, effects, ui) = runtime.send(AppMessage::new("increment", Value::Int(0))).unwrap();

// Read state
println!("Current state: {:?}", runtime.state());

// Render view
let ui_tree = runtime.view().unwrap();
```

### The .ax Component

```ax
type State { count: Int }
type Msg { tag: String, payload: Int }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }

fn init() -> State {
    State { count: 0 }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    match msg.tag {
        "increment" => {
            UpdateResult {
                state: State { count: state.count + 1 },
                effects: [],
            }
        },
        "reset" => {
            UpdateResult {
                state: State { count: 0 },
                effects: [],
            }
        },
        _ => { UpdateResult { state: state, effects: [] } },
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "counter", text: "count" }
}
```

### Testing Components

```rust
use boruna_framework::testing::TestHarness;
use boruna_framework::runtime::AppMessage;
use boruna_bytecode::Value;

let source = include_str!("../components/counter.ax");
let mut harness = TestHarness::from_source(source).unwrap();

// Send messages and verify state
harness.send(AppMessage::new("increment", Value::Int(0))).unwrap();
harness.send(AppMessage::new("increment", Value::Int(0))).unwrap();
assert_eq!(harness.cycle(), 2);

// Replay verification — proves determinism
let is_deterministic = harness.replay_verify(source, vec![
    AppMessage::new("increment", Value::Int(0)),
    AppMessage::new("increment", Value::Int(0)),
]).unwrap();
assert!(is_deterministic);
```

### State Snapshots and Time Travel

```rust
// Take a snapshot at any point
let snapshot = runtime.snapshot();

// Rewind to a previous cycle
runtime.rewind(1).unwrap();

// Diff state between cycles
let diffs = runtime.diff_from(0);
for diff in &diffs {
    println!("Field '{}' changed: {:?} -> {:?}",
        diff.field_name, diff.old_value, diff.new_value);
}
```

See [FRAMEWORK_API.md](FRAMEWORK_API.md) for the full API.

## Use Case 4: Safe Plugin / Extension System

Use Boruna as a sandboxed plugin system where third-party code runs with explicit, controlled permissions.

### Architecture

```
Your Rust Application
    |
    +-- Plugin Loader
    |       reads .ax files from plugins/ directory
    |       compiles each with boruna_compiler::compile()
    |
    +-- Plugin Runner
    |       creates Vm with per-plugin Policy
    |       executes plugin, collects results
    |
    +-- Capability Gateway
            intercepts all side effects
            enforces per-plugin permissions
```

### Example: Plugin with Controlled HTTP Access

```rust
use boruna_compiler::compile;
use boruna_vm::{Vm, CapabilityGateway, Policy};

fn run_plugin(plugin_source: &str, allow_network: bool) -> Result<boruna_bytecode::Value, String> {
    let module = compile("plugin", plugin_source)
        .map_err(|e| format!("{:?}", e))?;

    let policy = if allow_network {
        Policy::default().allow("net.fetch")
    } else {
        Policy::deny_all()
    };

    let gateway = CapabilityGateway::new(policy);
    let mut vm = Vm::new(module, gateway);
    vm.run().map_err(|e| format!("{:?}", e))
}
```

### Policy File per Plugin

Each plugin can declare what capabilities it needs in its `package.ax.json`:

```json
{
  "name": "my.plugin",
  "version": "1.0.0",
  "description": "Analytics aggregator",
  "dependencies": {},
  "required_capabilities": ["net.fetch"],
  "exposed_modules": ["core"]
}
```

Your host application validates these declarations against its own security policy before running the plugin.

### Available Capabilities

| Capability | What it gates |
|---|---|
| `net.fetch` | HTTP requests |
| `db.query` | Database queries |
| `fs.read` | File reads |
| `fs.write` | File writes |
| `time.now` | Current time access |
| `random` | Random value generation |
| `ui.render` | UI tree emission |
| `llm.call` | LLM API calls |
| `actor.spawn` | Child actor creation |
| `actor.send` | Actor message sending |

## Use Case 5: Multi-Agent Orchestration

Use the orchestrator to coordinate multiple AI agents working on your existing codebase.

### How It Works

The orchestrator models work as a DAG (directed acyclic graph). Each node is a task assigned to an agent role (Planner, Implementer, Reviewer). Changes are submitted as patch bundles and gated through deterministic checks.

### Setup

```bash
# Initialize orchestration in your project
cd your-project/
boruna-orch plan spec.json
```

### Plan Specification

```json
{
  "nodes": [
    {
      "id": "WN-001",
      "description": "Add input validation to API endpoints",
      "inputs": ["src/api/handlers.rs"],
      "outputs": ["src/api/handlers.rs", "src/api/validation.rs"],
      "dependencies": [],
      "owner_role": "Implementer",
      "tags": ["api", "validation"]
    },
    {
      "id": "WN-002",
      "description": "Review validation changes",
      "inputs": ["src/api/handlers.rs", "src/api/validation.rs"],
      "outputs": [],
      "dependencies": ["WN-001"],
      "owner_role": "Reviewer",
      "tags": ["api", "review"]
    }
  ]
}
```

### Workflow

```bash
# Agent picks up next task
boruna-orch next --role Implementer

# Agent submits changes as a patch bundle
boruna-orch apply changes.patchbundle.json

# Reviewer reviews (runs compile, test, replay gates)
boruna-orch review changes.patchbundle.json

# Check progress
boruna-orch status
```

### Conflict Prevention

The orchestrator acquires module-level locks when a node starts. If two agents try to modify the same file, the second one is blocked until the first completes. This prevents merge conflicts in multi-agent workflows.

See [ORCHESTRATOR_SPEC.md](ORCHESTRATOR_SPEC.md) for full details.

## Use Case 6: Trace-Based Testing

Record execution traces from production or staging, then generate regression tests automatically.

### Record a Trace

```bash
boruna trace app.ax > trace.json
```

### Generate Tests from Trace

```bash
boruna trace2tests trace.json --output generated_tests/
```

### Minimize Failing Traces

When a trace reveals a bug, minimize it to the smallest reproducing sequence using delta debugging:

```bash
boruna trace2tests trace.json --minimize --output minimal_test/
```

### Replay Verification

```bash
boruna replay app.ax trace.json
```

Replay feeds recorded capability results back into the VM and verifies the execution sequence is identical. Any divergence is a determinism violation.

See [TRACE_TO_TESTS.md](TRACE_TO_TESTS.md) for full details.

## Capability Policies

Policies control what side effects are allowed at runtime. This is the core security boundary.

### Policy Modes

```rust
// Allow everything (development/testing)
Policy::allow_all()

// Deny everything (pure computation only)
Policy::deny_all()

// Selective (production)
Policy::default()
    .allow("net.fetch")
    .allow("time.now")
```

### Policy File (`policy.ax.json`)

```json
{
  "allowed_capabilities": ["net.fetch", "db.query"],
  "denied_capabilities": []
}
```

### CLI Policy Flag

```bash
boruna run app.ax --policy allow-all
boruna run app.ax --policy deny-all
boruna run app.ax --policy policy.ax.json
```

### Enforcement

The `CapabilityGateway` intercepts every capability call at runtime. If a function annotated with `!{net.fetch}` tries to execute and `net.fetch` is not in the allowed set, the VM returns an error immediately. No network call is made.

## Standard Libraries

Boruna ships 11 standard libraries you can use in your `.ax` code. All are pure-functional with declared capabilities.

| Library | Capabilities | Use for |
|---|---|---|
| `std-ui` | none | Declarative UI primitives (row, column, button, table) |
| `std-forms` | none | Form engine with field validation |
| `std-validation` | none | Reusable validation rules (email, length, range) |
| `std-authz` | none | Role and permission checks |
| `std-routing` | none | Declarative URL routing |
| `std-testing` | none | Test helpers and assertions |
| `std-http` | `net.fetch` | HTTP request helpers (GET, POST, PUT, DELETE) |
| `std-db` | `db.query` | Database query builder |
| `std-storage` | `fs.read`, `fs.write` | Local persistence |
| `std-notifications` | `time.now` | Notification scheduling and management |
| `std-sync` | `net.fetch` | Offline-first sync with conflict resolution |

### Adding a Library

```bash
boruna-pkg add std-http 0.1.0
boruna-pkg install
```

### Using in Code

```ax
// Your app can use std-http helpers
fn update(state: State, msg: Msg) -> UpdateResult {
    match msg.tag {
        "fetch_users" => {
            UpdateResult {
                state: State { ..state, status: "loading" },
                effects: [
                    http_get("https://api.example.com/users", "users_loaded"),
                ],
            }
        },
        _ => { UpdateResult { state: state, effects: [] } },
    }
}
```

## API Quick Reference

### Compile and Run

```rust
use boruna_compiler::compile;
use boruna_vm::{Vm, CapabilityGateway, Policy};

let module = compile("name", source)?;
let gateway = CapabilityGateway::new(Policy::allow_all());
let mut vm = Vm::new(module, gateway);
let result = vm.run()?;
```

### Framework App

```rust
use boruna_framework::runtime::{AppRuntime, AppMessage};
use boruna_bytecode::Value;

let module = compile("app", source)?;
let mut runtime = AppRuntime::new(module)?;
runtime.send(AppMessage::new("msg_tag", Value::Int(0)))?;
let state = runtime.state();
let ui = runtime.view()?;
```

### Test Harness

```rust
use boruna_framework::testing::TestHarness;
use boruna_framework::runtime::AppMessage;
use boruna_bytecode::Value;

let mut harness = TestHarness::from_source(source)?;
harness.send(AppMessage::new("increment", Value::Int(0)))?;
let deterministic = harness.replay_verify(source, messages)?;
```

### Bounded Actor Execution

```rust
use boruna_vm::{Vm, StepResult};

match vm.execute_bounded(1000) {
    StepResult::Completed(value) => { /* done */ },
    StepResult::Yielded { steps_used } => { /* preempted, resume later */ },
    StepResult::Blocked => { /* waiting on message */ },
    StepResult::Error(err) => { /* handle error */ },
}
```

### Diagnostics

```rust
use boruna_tooling::diagnostics;

let diags = diagnostics::check(source);
for d in &diags {
    println!("{}: {} at {:?}", d.severity, d.message, d.span);
}
```

### Auto-Repair

```rust
use boruna_tooling::repair;

let repaired_source = repair::apply(source, &diags, repair::Strategy::Best)?;
```

## What Boruna Does Not Do

- **No FFI** — Boruna cannot call C libraries, system calls, or arbitrary Rust functions. All interaction with the outside world goes through the capability gateway.
- **No general-purpose systems programming** — It is designed for deterministic application logic, not low-level code.
- **No implicit side effects** — Every IO operation must be declared. There is no escape hatch.
- **No remote package registry yet** — Packages are local-only in v0.1.0.
- **No production runtime yet** — Boruna is functional but still v0.1.0. The APIs will evolve.
