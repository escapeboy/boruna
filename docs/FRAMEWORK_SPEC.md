# Boruna Application Framework Specification

## Overview

The framework defines a mandatory application protocol for all Boruna programs.
Every application follows a strict structure: init → update → view → effects cycle.

The VM is the kernel. The framework is userland. The runtime does not depend on the framework.

## 1. Application Protocol

Every app must implement three functions, plus an optional fourth:

```
fn init() -> State
fn update(state: State, msg: Message) -> UpdateResult
fn view(state: State) -> UITree
fn policies() -> PolicySet   // optional
```

### Rules

- `update()` must be pure — no capability annotations allowed.
- `update()` returns `UpdateResult { state: State, effects: List<Effect> }`.
- `view()` must be pure — returns a declarative UITree.
- `init()` may use capabilities for initial setup.
- `policies()` declares required capabilities and constraints. It is optional:
  when it is missing, the runtime uses `PolicySet::allow_all()` and
  `boruna framework validate` reports `policies: none (using defaults)`.

### Compile-Time Validation

The framework validator (`AppValidator`) checks:
- `init()`, `update()` and `view()` exist; `policies()` is optional.
- Parameter counts: `init()` 0, `update()` 2, `view()` 1, `policies()` 0.
- `update()`, `view()` and `policies()` have no capability annotations.

It also detects the State and Message types by name only (a type named `State`
or ending in `State`; `Msg`, `Message`, or ending in `Msg`) and reports them.
It does not check that State is serializable or that the Message type is an
enum — a record `Msg` passes.

## 2. Effect System

Effects are declarative descriptions of side effects:

```
type Effect {
    kind: String,       // one of the built-in effect kinds below
    payload: Value,     // structured payload (type depends on effect kind)
    callback_tag: String,  // message tag for delivering the result
}
```

Built-in effect kinds:
- `http_request` — maps to `net.fetch` capability
- `db_query` — maps to `db.query` capability
- `fs_read` — maps to `fs.read` capability
- `fs_write` — maps to `fs.write` capability
- `timer` — maps to `time.now` capability
- `random` — maps to `random` capability
- `spawn_actor` — maps to `actor.spawn` capability (creates child actor)
- `send_to_actor` — maps to `actor.send` capability
- `llm_call` — maps to `llm.call` capability
- `emit_ui` — emits UI tree to host (`ui.render`)

`AppRuntime::send` validates effects against the policy and returns them; it
does not execute them. Execution happens only through an `EffectExecutor`
(`AppRuntime::send_with_executor`), which turns each effect result into a
message tagged with `callback_tag` for the next `update()` call.

## 3. State Management

- State must be a record type.
- State is serialized to JSON between cycles for snapshots.
- The Rust `StateMachine` type provides (these are Rust methods, not `.ax` built-ins):
  - `snapshot()` — serialize current state to JSON string
  - `restore(json)` — deserialize state from JSON string
  - `diff_values(old, new)` / `diff_from_cycle(cycle)` — produce list of changed fields

## 4. UI Model

```
type UINode {
    tag: String,
    props: String,
    children_json: String,
}
```

UITree is a UINode at the root, with children encoded as JSON.
The view function returns a UINode.

Constraints:
- Pure function of State.
- No side effects.
- Host renders the tree.
- User events become Messages fed to `update()`.

## 5. Actor Integration

- Child actors use the same App protocol.
- Parent spawns child via `spawn_actor` effect.
- Messages between actors are routed by the framework runtime.
- Supervision: if a child crashes, parent receives an error message.

## 6. Policy Layer

```
type PolicySet {
    capabilities: List<String>,
    max_effects_per_cycle: Int,
    max_steps: Int,
}
```

Policy violations:
- Abort safely with structured error.
- Error is replay-compatible.

## 7. Testing Harness

Testing functions are methods on the Rust `TestHarness` type, not `.ax` built-ins
(see [FRAMEWORK_API.md](./FRAMEWORK_API.md)):
- `simulate(messages)` — run message sequence, return final state
- `assert_state(expected)` / `assert_state_field(index, expected)` — check state
- `assert_effects(expected_kinds)` — check effect kinds of the last cycle
- `replay_verify(source, messages)` — re-run the messages and compare states

From the CLI, use `boruna framework test` / `simulate` / `replay`.

Testing does not require a host UI.

## 8. Implementation

The framework is a Rust crate `boruna-framework` that provides:
- `AppValidator` — compile-time validation of App protocol
- `AppRuntime` — execution loop for the App protocol
- `EffectExecutor` — maps effects to capability calls
- `StateMachine` — state transition engine with snapshot/diff
- `TestHarness` — testing utilities

The framework compiles `.ax` sources through the normal compiler,
then wraps execution in the App protocol runtime.
