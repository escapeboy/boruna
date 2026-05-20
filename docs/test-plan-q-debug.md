# Test plan — `debug` / `debug_msg` builtins

For the `debug` print-and-passthrough builtin.
Architecture: `docs/architecture-q-debug.md`.

## Acceptance criteria

1. `debug(value)` prints `pretty(value)\n` to stderr and returns `value` unchanged.
2. `debug_msg(msg, value)` prints `msg ⎵ pretty(value)\n` to stderr and returns `value` unchanged.
3. Stdout is untouched.
4. Return type matches input type (no implicit conversion).
5. No capability required (does not invoke the capability gateway).
6. Works inside any expression context (let binding, function body, action update, list comp).
7. Does NOT emit any EventLog entry (operational-only).

## Compiler tests (in `crates/llmc/tests/debug_builtin.rs`)

| ID | `.ax` source | Expected outcome |
|---|---|---|
| D01 | `fn main() -> Int { debug(42) }` | compiles, function returns Int |
| D02 | `fn main() -> Int { debug_msg("x:", 42) }` | compiles, function returns Int |
| D03 | `fn main() -> String { debug("hello") }` | compiles, function returns String |
| D04 | `fn main() -> List<Int> { debug([1,2,3]) }` | compiles |
| D05 | `fn main() -> Option<Int> { debug(Some(7)) }` | compiles |
| D06 | `fn main() -> Int { let x = debug(10); x + 1 }` | compiles, return type Int |
| D07 | `fn main() -> Int { debug() }` (zero args) | compile error: arity mismatch |
| D08 | `fn main() -> Int { debug(1, 2, 3) }` (three args) | compile error: arity mismatch |
| D09 | `fn main() -> Int { debug_msg(1, 2) }` (msg not String) | compile error: type mismatch |

## VM tests (in `crates/llmvm/tests/debug_builtin.rs`)

Use `gag::BufferRedirect` or direct stderr capture in tests.

| ID | Source | stderr | stdout | return value |
|---|---|---|---|---|
| V01 | `debug(42)` | `42\n` | (empty) | `Value::Int(42)` |
| V02 | `debug_msg("answer:", 42)` | `answer: 42\n` | (empty) | `Value::Int(42)` |
| V03 | `debug("hello")` | `"hello"\n` | (empty) | `Value::String("hello")` |
| V04 | `debug([1,2,3])` | `[1, 2, 3]\n` | (empty) | `Value::List(...)` |
| V05 | `debug(Some(7))` | `Some(7)\n` | (empty) | `Value::Some(...)` |
| V06 | `debug(Err("boom"))` | `Err("boom")\n` | (empty) | `Value::Err(...)` |
| V07 | `let _ = debug(1); debug(2); 3` | `1\n2\n` | (empty) | `Value::Int(3)` |

## Determinism tests

| ID | Concern | Test |
|---|---|---|
| Z01 | Replay verification ignores debug calls | run a workflow with `debug` calls, take evidence bundle, `boruna evidence verify` passes |
| Z02 | Same input → same output (excluding stderr) | run twice, assert evidence bundles byte-identical |
| Z03 | Stderr output is operational-only | run with `--record`, evidence bundle does NOT include the stderr content |

## Capability gateway tests

| ID | Scenario | Test |
|---|---|---|
| K01 | `debug` works under `--policy deny-all` | runs successfully |
| K02 | `debug` does NOT invoke `CapabilityGateway::request_capability` | gateway spy records 0 invocations |

## Stdlib integration

After implementation, smoke test: `cargo test -p boruna-tooling test_std_*` still passes —
ensures the `debug` builtin doesn't shadow or break any existing library symbol.

## Out of scope (deferred)

- `debug_if(cond, ...)` conditional variant
- Output redirection / suppression flags
- `tracing` crate integration
- Color / ANSI formatting
- Pretty-printer depth override
