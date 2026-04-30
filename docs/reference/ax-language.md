# .ax Language Reference

> **Looking for the formal specification?** This page is the narrative, example-driven reference. The authoritative grammar, type rules, and capability semantics live in [`docs/spec/ax-language-1.0.md`](../spec/ax-language-1.0.md). When the two disagree, the spec wins.

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

## Import statements

```ax
import "std-name"
```

Import statements load a standard library package at compile time. The library source is inlined into the compilation unit before type-checking. The import line itself is removed from the compiled output.

Standard library packages are resolved from the `libs/` directory relative to the current working directory. When a library source is inlined, any `fn main() -> Int` stub present in the library file is stripped so it does not conflict with the importing program's own `main`.

Example:

```ax
import "std-json"

fn main() -> Int {
    let s: String = int_to_string(42)
    0
}
```

## Built-in functions

These functions are provided by the runtime and do not need to be imported:

| Function | Signature | Description |
|----------|-----------|-------------|
| `__builtin_int_to_string` | `(Int) -> String` | Convert an integer to its decimal string representation |
| `__builtin_float_to_string` | `(Float) -> String` | Convert a float to its string representation |
| `__builtin_string_len` | `(String) -> Int` | Length of a string in bytes |
| `__builtin_string_chars` | `(String) -> List<String>` | Split a string into a list of single-character strings |
| `__builtin_string_contains` | `(String, String) -> Bool` | True if first string contains the second |
| `__builtin_string_starts_with` | `(String, String) -> Bool` | True if string starts with prefix |
| `__builtin_string_ends_with` | `(String, String) -> Bool` | True if string ends with suffix |
| `__builtin_string_to_upper` | `(String) -> String` | Uppercase copy |
| `__builtin_string_to_lower` | `(String) -> String` | Lowercase copy |
| `__builtin_string_trim` | `(String) -> String` | Strip leading/trailing whitespace |
| `__builtin_string_join` | `(List<String>, String) -> String` | Join list with separator |
| `__builtin_string_split` | `(String, String) -> List<String>` | Split string on a delimiter |
| `__builtin_string_replace` | `(String, String, String) -> String` | Replace first occurrence of pattern |
| `__builtin_string_slice` | `(String, Int, Int) -> String` | Substring by byte offsets |
| `__builtin_int_parse` | `(String) -> Result<Int, String>` | Parse a decimal integer string |
| `__builtin_float_parse` | `(String) -> Result<Float, String>` | Parse a float string |
| `__builtin_bool_to_string` | `(Bool) -> String` | Convert a bool to "true" or "false" |
| `__builtin_list_len` | `(List<T>) -> Int` | Number of elements |
| `__builtin_list_is_empty` | `(List<T>) -> Bool` | True if list has zero elements |
| `__builtin_list_head` | `(List<T>) -> Option<T>` | First element, or None |
| `__builtin_list_tail` | `(List<T>) -> List<T>` | All elements after the first |
| `__builtin_list_append` | `(List<T>, T) -> List<T>` | New list with item added at end |
| `__builtin_list_concat` | `(List<T>, List<T>) -> List<T>` | Concatenate two lists |
| `__builtin_list_reverse` | `(List<T>) -> List<T>` | Reversed copy |
| `__builtin_map_get` | `(Map<String, V>, String) -> Option<V>` | Look up a key; returns `Some(v)` or `None` |
| `__builtin_map_set` | `(Map<String, V>, String, V) -> Map<String, V>` | Return a new map with key set to value |
| `__builtin_map_remove` | `(Map<String, V>, String) -> Map<String, V>` | Return a new map with key removed |
| `__builtin_map_contains_key` | `(Map<String, V>, String) -> Bool` | True if key is present |
| `__builtin_map_keys` | `(Map<String, V>) -> List<String>` | All keys in sorted order |
| `__builtin_map_values` | `(Map<String, V>) -> List<V>` | All values in key-sorted order |
| `__builtin_map_len` | `(Map<String, V>) -> Int` | Number of entries |

These built-ins are also wrapped in `std-json` (via `int_to_string`, `json_escape`) and can be called directly in any `.ax` file.

**Note on naming:** The `__builtin_` prefix distinguishes these from user-defined functions and prevents shadowing. User-facing wrappers in stdlib packages use cleaner names.

## What .ax is not

`.ax` is deliberately minimal. It does not have:
- Mutable variables (use record spread for state transitions)
- Loops (use recursion or standard library functions)
- Exceptions (use `Result<T, E>`)
- Implicit side effects (every effect must be declared)
- Generics (types are concrete at definition time)

These omissions are intentional. They keep the language deterministic and auditable.
