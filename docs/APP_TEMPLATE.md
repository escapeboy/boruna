# App Template

## Canonical File Layout

```
my_app/
  my_app.ax       # Single source file
```

## Minimal App Skeleton

```ax
// my_app — Boruna Framework App

type State { value: Int }
type Msg { tag: String, payload: Int }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }

fn init() -> State {
    State { value: 0 }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    UpdateResult {
        state: State { value: state.value + msg.payload },
        effects: [],
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "text", text: "value" }
}

fn main() -> Int {
    let s: State = init()
    s.value
}
```

## Required Types

| Type           | Fields                                             |
|----------------|----------------------------------------------------|
| `State`        | Your app state. Must be a record.                  |
| `Msg`          | `tag: String, payload: <T>`. Tag dispatches logic. |
| `Effect`       | `kind: String, payload: String, callback_tag: String` |
| `UpdateResult` | `state: State, effects: List<Effect>`              |
| `UINode`       | `tag: String` + any additional fields              |

## Required Functions

| Function   | Signature                                    | Pure? |
|------------|----------------------------------------------|-------|
| `init()`   | `() -> State`                                | No    |
| `update()` | `(State, Msg) -> UpdateResult`               | Yes   |
| `view()`   | `(State) -> UINode`                          | Yes   |

## Optional Functions

| Function     | Signature              | Purpose                     |
|--------------|------------------------|-----------------------------|
| `policies()` | `() -> PolicySet`     | Declare allowed capabilities |
| `main()`     | `() -> Int`           | Standalone test entry point  |

## Create From CLI

```bash
boruna framework new my_app
boruna framework validate my_app/my_app.ax
boruna run my_app/my_app.ax
```

## Policy Template

```ax
type PolicySet { capabilities: List<String>, max_effects: Int, max_steps: Int }

fn policies() -> PolicySet {
    PolicySet {
        capabilities: ["net.fetch", "time.now"],
        max_effects: 10,
        max_steps: 1000000,
    }
}
```
