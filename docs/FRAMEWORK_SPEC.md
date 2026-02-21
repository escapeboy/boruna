# Boruna Application Framework Specification

## Overview

The framework defines a mandatory application protocol for all Boruna programs.
Every application follows a strict structure: init → update → view → effects cycle.

The VM is the kernel. The framework is userland. The runtime does not depend on the framework.

## 1. Application Protocol

Every app must implement four functions:

```
fn init() -> State
fn update(state: State, msg: Message) -> UpdateResult
fn view(state: State) -> UITree
fn policies() -> PolicySet
```

### Rules

- `update()` must be pure — no capability annotations allowed.
- `update()` returns `UpdateResult { state: State, effects: List<Effect> }`.
- `view()` must be pure — returns a declarative UITree.
- `init()` may use capabilities for initial setup.
- `policies()` declares required capabilities and constraints.

### Compile-Time Validation

The framework compiler validates:
- All four functions exist with correct signatures.
- `update()` has no capability annotations.
- `view()` has no capability annotations.
- State type is defined and serializable.
- Message type is an enum.

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
- `spawn_actor` — creates child actor
- `emit_ui` — emits UI tree to host

The framework runtime executes effects between update cycles.
Effect results are delivered as messages to the next `update()` call.

## 3. State Management

- State must be a record type.
- State is serialized to JSON between cycles for snapshots.
- Framework provides:
  - `snapshot(state)` — serialize state to JSON string
  - `restore(json)` — deserialize state from JSON string
  - `diff(old, new)` — produce list of changed fields

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

Built-in testing functions:
- `simulate(init, messages)` — run message sequence, return final state
- `assert_state(state, field, expected)` — check state field
- `assert_effects(effects, expected_kinds)` — check effect kinds
- `replay_verify(log1, log2)` — compare execution logs

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
