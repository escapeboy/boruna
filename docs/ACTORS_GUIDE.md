# Actors Guide

## Status

Actor integration in the framework is **partial**. The VM has an `ActorSystem`
with basic spawn/send/receive mechanics. The framework does not yet wire actors
into the App protocol runtime.

## Architecture

```
Parent App
  ├── init() → State
  ├── update(state, msg) → UpdateResult
  │     └── effects: [Effect { kind: "spawn_actor", ... }]
  ├── view(state) → UINode
  └── Child Actor (same App protocol)
        ├── init() → State
        ├── update(state, msg) → UpdateResult
        └── view(state) → UINode
```

## Spawning Actors

Return a `spawn_actor` effect from `update()`:

```ax
Effect { kind: "spawn_actor", payload: "child_module", callback_tag: "child_spawned" }
```

The framework runtime will:
1. Compile and initialize the child module.
2. Assign it an actor ID.
3. Deliver a `child_spawned` message to the parent with the actor ID.

## Message Routing

Messages between actors use the tag routing convention:
- Parent → Child: effect with `kind: "send_to_actor"` (not yet implemented)
- Child → Parent: effect with `kind: "emit_ui"` or framework routing

## Supervision

If a child actor crashes (runtime error), the parent receives an error message:
- Tag: `"actor_error"`
- Payload: error description

## Scheduling

In single-actor mode: FIFO message processing, deterministic.
In multi-actor mode: round-robin scheduling, deterministic order in debug mode.

## Current Limitations

1. Actor spawning is not executed by the framework runtime (only parsed as effects).
2. No inter-actor message routing at the framework level.
3. No supervision tree implementation.
4. The VM's `ActorSystem` exists but is not integrated with `AppRuntime`.

## VM-Level Actor API

The VM provides these opcodes for actors:
- `SpawnActor(func_idx)` — spawn actor from function
- `SendMsg` — send message to actor ID
- `ReceiveMsg` — block for incoming message

These are available in bytecode but not yet connected to the framework's
effect-based actor model.
