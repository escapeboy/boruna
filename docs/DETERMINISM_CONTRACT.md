# Determinism Contract

## What Is Deterministic

Given the same bytecode module and the same message sequence:

1. **State transitions**: Every `update(state, msg)` produces the identical `Value` output.
2. **Effect lists**: The effects returned by `update()` are identical in kind, payload, and order.
3. **UI trees**: The `view(state)` output is identical for identical state.
4. **Scheduling order**: In single-actor mode, messages are processed FIFO.
5. **Cycle count**: The number of cycles matches exactly.

## What Must Be Virtualized

These sources of non-determinism are kept outside the pure core:

| Source      | How Virtualized                                         |
|-------------|---------------------------------------------------------|
| Time        | `timer` effect → capability gateway → logged in events  |
| Randomness  | `random` effect → capability gateway → logged in events |
| Network I/O | `http_request` effect → capability gateway → logged     |
| File I/O    | `fs_read`/`fs_write` effects → capability gateway       |
| Database    | `db_query` effect → capability gateway                  |

All external interactions go through the capability gateway, which logs
every call and result in the `EventLog`. Replay substitutes recorded
results, guaranteeing identical execution.

## What Must Be Logged

The framework `CycleRecord` logs per cycle:
- `cycle` — cycle number
- `message` — the input message (tag + payload)
- `state_before` — state value before update
- `state_after` — state value after update
- `effects` — effect list returned by update
- `ui_tree` — view output

The VM `EventLog` logs:
- `CapCall` — capability name + arguments
- `CapResult` — capability name + return value
- `UiEmit` — emitted UI tree
- `ActorSpawn`, `MessageSend`, `MessageReceive`, `SchedulerTick`

## Replay Contract

1. Record: Run the app with a real capability handler. Save the `EventLog`.
2. Replay: Run the same bytecode with `ReplayHandler` seeded from recorded `CapResult` values.
3. Verify: The replay `EventLog` must produce identical `CapCall` sequences (same capability, same args, same order).

If verification fails, either:
- The bytecode is non-deterministic (bug in compiler/VM).
- An external value leaked into the pure core (bug in framework).

## Multi-Actor Determinism

In single-actor mode, messages are processed FIFO. In multi-actor mode,
the VM uses round-robin scheduling across actors. The exact scheduling
order is captured in the EventLog via `SchedulerTick`, `ActorSpawn`,
`MessageSend`, and `MessageReceive` events.

During replay, the EventLog enforces the identical scheduling sequence.
This means multi-actor execution is deterministic as long as it is replayed
from the same EventLog. Two independent runs with the same bytecode and
messages are NOT guaranteed to produce the same scheduling order — only
record-then-replay guarantees identical execution.

If your application requires fully deterministic multi-actor ordering
without replay, restrict to single-actor mode or use explicit message
sequencing in your update logic.

## Enforcement

- `update()` and `view()` run with `Policy::deny_all()` — no capability calls allowed.
- Effects are the only way to request external data.
- The capability gateway is not accessible during pure function execution.
- Violation = `FrameworkError::PurityViolation`.

## Golden Test Protocol

Golden tests hash the following after running a fixed message sequence:
1. Final state JSON snapshot.
2. Full cycle log (state_before, state_after, effects per cycle).
3. Concatenated effect kind strings.

If the hash changes, the test fails with a diff showing exactly what diverged.
