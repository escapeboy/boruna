# Effects Guide

## Overview

Effects are declarative descriptions of side effects. `update()` never performs IO directly.
Instead, it returns a list of effects. The framework runtime executes them via the capability gateway.

## Effect Structure

```ax
type Effect { kind: String, payload: String, callback_tag: String }
```

- `kind` — which effect to execute (see table below)
- `payload` — data for the effect (URL, query, path, etc.)
- `callback_tag` — message tag for the result delivery

## Built-in Effect Kinds

| Kind           | Capability  | Description              |
|----------------|-------------|--------------------------|
| `http_request` | `net.fetch` | HTTP GET/POST request    |
| `db_query`     | `db.query`  | Database query           |
| `fs_read`      | `fs.read`   | Read file                |
| `fs_write`     | `fs.write`  | Write file               |
| `timer`        | `time.now`  | Get current time         |
| `random`       | `random`    | Get random value         |
| `spawn_actor`  | `spawn`     | Spawn child actor        |
| `emit_ui`      | `ui.render` | Emit UI tree to host     |

## Returning Effects From update()

```ax
fn update(state: State, msg: Msg) -> UpdateResult {
    if msg.tag == "fetch" {
        UpdateResult {
            state: state,
            effects: [
                Effect {
                    kind: "http_request",
                    payload: "https://api.example.com/data",
                    callback_tag: "data_received",
                },
            ],
        }
    } else {
        UpdateResult { state: state, effects: [] }
    }
}
```

## Effect Lifecycle

1. `update()` returns `UpdateResult { state, effects }`.
2. Framework validates effects against the policy.
3. Framework (or host) executes each effect via capability gateway.
4. Effect results are delivered as new messages with `callback_tag` as the tag.
5. `update()` handles the callback message in the next cycle.

## Multiple Effects Per Cycle

Return multiple effects in the list. They execute in order.

```ax
effects: [
    Effect { kind: "http_request", payload: "url1", callback_tag: "result" },
    Effect { kind: "http_request", payload: "url2", callback_tag: "result" },
    Effect { kind: "http_request", payload: "url3", callback_tag: "result" },
]
```

## Policy Constraints

Effects are checked against the app's `PolicySet`:
- Only listed capabilities are allowed.
- `max_effects_per_cycle` limits how many effects per update.
- Violations produce `FrameworkError::PolicyViolation`.

## Determinism

Effects themselves are deterministic data. Their execution results are logged
by the capability gateway. Replay substitutes recorded results, making the
entire execution deterministic.
