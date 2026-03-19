# .ax Language Reference

`.ax` is Boruna's statically-typed, deterministic scripting language. It compiles to Boruna bytecode and runs on the Boruna VM. It is designed for workflow steps: small, focused, pure functions with explicit capability declarations for any side effects.

## File structure

Every standalone `.ax` file must define `fn main() -> Int`. The main function is the entry point for `boruna run`.

```ax
fn main() -> Int {
    42
}
```

## Types

| Type | Description | Example |
|------|-------------|---------|
| `Int` | 64-bit signed integer | `42`, `-7` |
| `Float` | 64-bit float | `3.14` |
| `String` | UTF-8 string | `"hello"` |
| `Bool` | Boolean | `true`, `false` |
| `Unit` | No value | `()` |
| `Option<T>` | Optional value | `Some(42)`, `None` |
| `Result<T, E>` | Success or error | `Ok(42)`, `Err("msg")` |
| `List<T>` | Ordered list | `[1, 2, 3]` |
| `Map<K, V>` | Key-value map | `{"a": 1, "b": 2}` |
| Records | Named fields | `Point { x: 1, y: 2 }` |
| Enums | Tagged union | `Shape::Circle { radius: 5 }` |

## Variables

Variables are immutable. Use `let` with an explicit type annotation:

```ax
let name: String = "Boruna"
let count: Int = 0
let flag: Bool = true
```

No semicolons. Each statement is on its own line.

## Functions

```ax
fn add(a: Int, b: Int) -> Int {
    a + b
}
```

The last expression in a function body is the return value. No `return` keyword needed.

## Capability annotations

Functions that perform side effects must declare the required capabilities:

```ax
fn fetch(url: String) -> String !{net.fetch} {
    // live implementation
}

fn call_model(prompt: String) -> String !{llm.call} {
    // live implementation
}

// Multiple capabilities
fn fetch_and_cache(url: String) -> String !{net.fetch, fs.write} {
    // live implementation
}
```

Without the annotation, the VM will reject any attempt to call the capability at runtime.

## Records

Define named record types:

```ax
record Point {
    x: Int,
    y: Int,
}

let p: Point = Point { x: 3, y: 4 }
let px: Int = p.x
```

Record spread creates an updated copy:

```ax
let p2: Point = Point { ..p, y: 10 }
```

## Enums

```ax
enum Shape {
    Circle { radius: Float },
    Rectangle { width: Float, height: Float },
}

let s: Shape = Shape::Circle { radius: 5.0 }
```

## Pattern matching

```ax
let result: String = match s {
    Shape::Circle { radius } => "circle"
    Shape::Rectangle { width, height } => "rectangle"
    _ => "unknown"
}
```

Match on Option:

```ax
let value: Option<Int> = Some(42)
let n: Int = match value {
    Some(x) => x
    None => 0
}
```

Match on Result:

```ax
let r: Result<Int, String> = Ok(99)
let out: Int = match r {
    Ok(v) => v
    Err(_) => -1
}
```

Match on strings:

```ax
let greeting: String = match lang {
    "en" => "hello"
    "es" => "hola"
    _ => "hi"
}
```

## Conditionals

```ax
let label: String = if score > 90 {
    "pass"
} else {
    "fail"
}
```

## Lists

```ax
let items: List<Int> = [1, 2, 3, 4, 5]
```

List operations are available through the standard library.

## Maps

```ax
let config: Map<String, Int> = { "timeout": 30, "retries": 3 }
```

## Framework apps

Framework apps implement the Elm architecture. They must define:

```ax
fn init() -> State { ... }
fn update(state: State, msg: Msg) -> UpdateResult { ... }
fn view(state: State) -> UINode { ... }
```

Where `State`, `Msg`, `Effect`, `UpdateResult`, `UINode`, and `PolicySet` are the framework protocol types. See [FRAMEWORK_SPEC.md](../FRAMEWORK_SPEC.md) for the full protocol.

## Syntax quick reference

```ax
// Comments use double-slash

// Variables (immutable, type required)
let x: Int = 42

// Function
fn square(n: Int) -> Int {
    n * n
}

// Capability function
fn now() -> Int !{time.now} {
    // implementation
}

// Record literal
Point { x: 1, y: 2 }

// Record spread
Point { ..point, x: 10 }

// Enum variant
Shape::Circle { radius: 5.0 }

// Pattern match
match x {
    0 => "zero"
    _ => "nonzero"
}

// Option
Some(42)
None

// Result
Ok("value")
Err("message")

// List
[1, 2, 3]

// Map
{ "key": "value" }
```

## What .ax is not

`.ax` is deliberately minimal. It does not have:
- Mutable variables (use record spread for state transitions)
- Loops (use recursion or standard library functions)
- Exceptions (use `Result<T, E>`)
- Implicit side effects (every effect must be declared)
- Generics (types are concrete at definition time)
- Modules/imports (capability is via the standard library system)

These omissions are intentional. They keep the language deterministic and auditable.
