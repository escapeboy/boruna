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

No child module is compiled or started. What happens depends on how the app is run:
- `AppRuntime::send` (and `boruna framework test`) checks the effect against the
  policy (capability `actor.spawn`) and returns it as data. Nothing is executed.
- With `MockEffectExecutor`, the parent gets a `child_spawned` message whose
  payload is a fake `ActorId` (1, 2, 3, …). No actor exists behind it.
- With `HostEffectExecutor`, the effect is passed to the capability gateway as
  `actor.spawn`. There is no actor runtime there; the default handler returns
  `Unit`, which is delivered as the `child_spawned` payload.

## Message Routing

- Parent → Child: `send_to_actor` is a recognised effect kind (capability
  `actor.send`), but nothing delivers it to an actor. `MockEffectExecutor`
  answers with `"delivered"`; `HostEffectExecutor` passes it to the gateway,
  whose default handler returns `Unit`.
- Child → Parent: not implemented at the framework level.

## Supervision

At the framework level there is no supervision: no child actors run, and no
`actor_error` message is ever produced.

The VM's `ActorSystem` does supervise: when an actor fails, it is marked failed,
the failure cascades to all its descendants, and the parent receives an
`Err(<error description>)` message. If the root actor fails, the error is
returned from the run.

## Scheduling

VM `ActorSystem`: round-robin scheduling, always deterministic. Pending messages
are delivered at the end of each round, sorted by (target ID, sender ID).

## Current Limitations

1. Actor spawning is not executed by the framework runtime (only parsed as effects).
2. No inter-actor message routing at the framework level.
3. No supervision at the framework level (the VM `ActorSystem` has it).
4. The VM's `ActorSystem` exists but is not integrated with `AppRuntime`.

## VM-Level Actor API

The VM provides these opcodes for actors:
- `SpawnActor(func_idx)` — spawn actor from function
- `SendMsg` — send message to actor ID
- `ReceiveMsg` — block for incoming message

These are available in bytecode but not yet connected to the framework's
effect-based actor model.
