# Testing Guide

## TestHarness

The primary testing tool. No host UI required.

```rust
use boruna_framework::testing::TestHarness;
use boruna_framework::runtime::AppMessage;
use boruna_bytecode::Value;

let mut harness = TestHarness::from_source(SOURCE)?;
```

## Send Messages

```rust
let (state, effects) = harness.send(
    AppMessage::new("increment", Value::Int(0))
)?;
```

## Simulate Sequences

```rust
let final_state = harness.simulate(vec![
    AppMessage::new("add", Value::Int(0)),
    AppMessage::new("add", Value::Int(0)),
    AppMessage::new("complete", Value::Int(0)),
])?;
```

## Assertions

```rust
// Check a specific field by index
harness.assert_state_field(0, &Value::Int(3))?;

// Check full state equality
harness.assert_state(&expected_value)?;

// Check effects from last cycle
harness.assert_effects(&["http_request"])?;
```

## Snapshots

```rust
let json = harness.snapshot();  // JSON string of current state
```

## Time Travel

```rust
harness.rewind(0)?;  // Go back to init state
```

## Replay Verification

```rust
let messages = vec![
    AppMessage::new("increment", Value::Int(0)),
    AppMessage::new("increment", Value::Int(0)),
];
for msg in &messages {
    harness.send(msg.clone())?;
}

// Replay same messages on fresh runtime, verify identical states
let identical = harness.replay_verify(SOURCE, messages)?;
assert!(identical);
```

## Golden Tests

Golden tests verify determinism. Run the same messages twice, compare:

```rust
let mut h1 = TestHarness::from_source(SOURCE)?;
let mut h2 = TestHarness::from_source(SOURCE)?;

for msg in &messages {
    h1.send(msg.clone())?;
    h2.send(msg.clone())?;
}

assert_eq!(h1.state(), h2.state());
assert_eq!(h1.snapshot(), h2.snapshot());
```

## CLI Testing

```bash
# Validate app protocol
boruna framework validate my_app.ax

# Send messages and see state
boruna framework test my_app.ax -m "add:0,add:0,complete:0"

# Inspect state as JSON
boruna framework inspect-state my_app.ax -m "add:0,add:0"

# Step-by-step simulation
boruna framework simulate my_app.ax "add:0,add:0,complete:0"

# Machine-readable diagnostics
boruna framework diag my_app.ax -m "add:0,add:0"

# Stable trace hash (for CI comparison)
boruna framework trace-hash my_app.ax -m "add:0,add:0"

# App contract summary
boruna framework inspect my_app.ax --json
```

## Message Format

CLI messages use `tag:payload` format:
- `increment:0` → tag="increment", payload=Int(0)
- `fetch:https://example.com` → tag="fetch", payload=String("https://example.com")
- `reset` → tag="reset", payload=Int(0) (default)

Comma-separated for sequences: `add:0,add:0,complete:0`

## Writing Test Apps

Include `fn main() -> Int` for standalone execution:

```ax
fn main() -> Int {
    let s0: State = init()
    let r1: UpdateResult = update(s0, Msg { tag: "add", payload: 0 })
    let r2: UpdateResult = update(r1.state, Msg { tag: "add", payload: 0 })
    r2.state.total
}
```

Run directly: `boruna run my_app.ax`
