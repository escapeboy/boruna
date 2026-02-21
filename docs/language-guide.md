# Boruna Language Guide

## Overview

Boruna is a statically-typed, capability-safe programming language designed for LLM-native applications. All side effects are explicitly declared and enforced at both compile time and runtime.

## Types

### Primitives
- `Int` — 64-bit signed integer
- `Float` — 64-bit floating point
- `String` — UTF-8 string
- `Bool` — boolean (true/false)
- `Unit` — void/nothing

### Option and Result
```
Option<T>  — None | Some(value)
Result<T, E> — Ok(value) | Err(error)
```

### Records (Product Types)
```
type User {
    name: String,
    age: Int,
}
```

Records support spread syntax for partial updates:
```
let updated = User { ..original, age: 31 }
```
Fields not listed after the spread are copied from the base expression.

### Enums (Sum Types)
```
enum Color {
    Red,
    Green,
    Blue,
    Custom(String),
}
```

### Collections
```
List<T>
Map<K, V>
```

List operations:
```
let items: List<Int> = [1, 2, 3]
let len: Int = list_len(items)       // 3
let first: Int = list_get(items, 0)  // 1
let more: List<Int> = list_push(items, 4)  // [1, 2, 3, 4]
```

### String Builtins
```
let n: Int = parse_int("42")              // 42 (0 on failure)
let ok: Bool = str_contains("hello", "ell")  // true
let ok: Bool = str_starts_with("hello", "he") // true
```

`try_parse_int` returns `Result<Int, String>` instead of silently returning 0:
```
match try_parse_int("42") {
    Ok(n) => n,          // 42
    Err(e) => -1,        // not reached
}
match try_parse_int("abc") {
    Ok(n) => n,          // not reached
    Err(e) => -1,        // -1 (e = "invalid integer: abc")
}
```

## Functions

```
fn add(a: Int, b: Int) -> Int {
    a + b
}
```

### Capability Annotations

Functions that perform side effects must declare their capabilities:

```
fn fetch_data(url: String) -> String !{net} {
    // can use net.fetch capability
}

fn save_file(path: String, data: String) -> Bool !{fs.write} {
    // can use fs.write capability
}
```

### Contracts (requires/ensures)

```
fn divide(a: Int, b: Int) -> Int
    requires b != 0
{
    a / b
}
```

## Control Flow

### If/Else
```
if condition {
    // then
} else {
    // else
}
```

### Pattern Matching
```
match value {
    Some(x) => x + 1,
    None => 0,
}
```

String values can be matched directly:
```
match msg.tag {
    "create" => handle_create(state),
    "delete" => handle_delete(state),
    _ => state,
}
```

### While Loops
```
while condition {
    // body
}
```

## Operators

- Arithmetic: `+`, `-`, `*`, `/`, `%`
- Comparison: `==`, `!=`, `<`, `<=`, `>`, `>=`
- Logical: `&&`, `||`, `!`
- String concatenation: `++`

## Actors

```
fn worker() {
    let msg = receive
    // process message
}

fn main() {
    let w = spawn worker
    send w "hello"
}
```

## UI Emission

```
fn render(state: AppState) {
    emit state  // sends declarative UI tree to host
}
```

## Modules

```
module myapp

import utils
export fn main() { ... }
```
