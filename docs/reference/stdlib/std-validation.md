# std-validation

> Reusable composable validation rules

**Package:** `std.validation`  **Version:** `0.1.0`  **Capabilities required:** none

## Overview

`std-validation` provides a small set of pure, composable validation functions. Each returns a `ValidationResult` that carries a pass/fail flag and an error message. Chain results with `validation_merge` to apply multiple rules in sequence and surface the first failure. `std-forms` depends on this library for field-level validation.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.validation": "0.1.0"
```

## API Reference

### Types

#### `ValidationError`

```
type ValidationError { field: String, message: String }
```

#### `ValidationResult`

```
type ValidationResult { valid: Int, error_field: String, error_message: String }
```

- `valid` — `1` if the rule passed, `0` if it failed
- `error_field` — the field name on failure, `""` on success
- `error_message` — a short description of what failed, `""` on success

### Functions

#### String rules

##### `validate_required(field: String, value: String) -> ValidationResult`

Fails if `value` is an empty string.

**Example**
```
fn main() -> Int {
  let r: ValidationResult = validate_required("email", "")
  r.valid
}
```

##### `validate_not_empty(field: String, value: String) -> ValidationResult`

Alias for `validate_required`.

##### `validate_min_length(field: String, value: String, min: Int) -> ValidationResult`

Fails if the string length is less than `min`.

##### `validate_max_length(field: String, value: String, max: Int) -> ValidationResult`

Fails if the string length exceeds `max`.

##### `validate_numeric(field: String, value: String) -> ValidationResult`

Fails if `value` cannot be parsed as an integer.

##### `validate_equals(field: String, actual: String, expected: String) -> ValidationResult`

Fails if `actual != expected`. Use for password confirmation fields.

#### Integer rules

##### `validate_min_value(field: String, value: Int, min: Int) -> ValidationResult`

Fails if `value < min`.

##### `validate_max_value(field: String, value: Int, max: Int) -> ValidationResult`

Fails if `value > max`.

#### Composition

##### `validation_ok() -> ValidationResult`

Returns a pre-built passing result. Use as the initial accumulator in a chain.

##### `validation_fail(field: String, message: String) -> ValidationResult`

Returns a pre-built failing result with a custom message.

##### `validation_merge(a: ValidationResult, b: ValidationResult) -> ValidationResult`

Returns `a` if it failed, otherwise returns `b`. Use to apply rules in priority order and surface the first failure.

**Example: chaining multiple rules**
```
fn main() -> Int {
  let r1: ValidationResult = validate_required("username", "alice")
  let r2: ValidationResult = validate_min_length("username", "alice", 3)
  let r3: ValidationResult = validate_max_length("username", "alice", 20)
  let result: ValidationResult = validation_merge(validation_merge(r1, r2), r3)
  result.valid
}
```

## Capabilities

None. All functions are pure.

## Notes / Limitations

- `string_length` is a runtime built-in; the stub in `core.ax` returns `0` and is replaced at compile time.
- `validate_numeric` uses `try_parse_int` (a pattern-matching built-in); the library does not support float parsing.
- `validation_merge` returns the first failure only — it does not accumulate multiple errors. If you need to collect all errors, maintain a list of `ValidationResult` in your app state.
