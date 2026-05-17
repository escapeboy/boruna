# Skill: The .ax Language

A concise reference for writing `.ax` source. For the full spec see
`docs/reference/ax-language.md` in the Boruna repository.

## Program shape

Every standalone `.ax` file needs an entry point:

```
fn main() -> Int {
    return 0
}
```

`main` must return `Int` — it becomes the process exit code.

## Types

- Scalars: `Int`, `Float`, `String`, `Bool`, `Unit`
- Containers: `Option<T>`, `Result<T, E>`, `List<T>`, `Map<K, V>`
- User-defined: `record` and `enum`

## Functions

```
fn add(a: Int, b: Int) -> Int {
    return a + b
}
```

A function that performs a side effect must declare the capability it uses:

```
fn fetch(url: String) -> String !{net.fetch} {
    ...
}
```

The `!{...}` capability set is mandatory and checked by the compiler — calling
an effectful operation without declaring its capability is diagnostic `E007`.

## Records

```
record State {
    count: Int,
    name: String,
}
```

Construct and update with spread syntax:

```
let s = State { count: 0, name: "init" }
let s2 = State { ..s, count: 1 }
```

## Enums and pattern matching

```
enum Color { Red, Green, Blue }

let label = match c {
    Color::Red => "red",
    Color::Green => "green",
    Color::Blue => "blue",
}
```

`match` must be exhaustive — a missing case is diagnostic `E005`. Use `_` as a
catch-all when total coverage is not needed.

## Capabilities

Capabilities name the side effects a program may perform. Common ones:
`net.fetch`, `db.query`, `fs.read`, `fs.write`, `llm.call`. They are declared
on functions and enforced at runtime by the VM against the active policy.
Pure functions declare no capabilities and are fully deterministic.

## Determinism rule

`.ax` code is deterministic: same input always produces same output. No
wall-clock time, no randomness, no hidden global state in pure code. All
non-determinism enters only through declared capabilities.

## Next steps

- `boruna skills get cli` — the command surface.
- `boruna skills get diagnostics` — error codes and the repair loop.
- `boruna check <file>.ax` equivalents: `boruna lang check <file>.ax --json`.
