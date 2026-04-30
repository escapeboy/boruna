# std-json

> JSON-flavoured data extraction and manipulation utilities

**Package:** `std.json`  **Version:** `0.1.0`  **Capabilities required:** none

## Overview

`std-json` provides pure-functional helpers for building JSON-formatted strings from typed Boruna values. It covers the four JSON scalar types (string, number, boolean, null), object wrapping, and array wrapping. Because it has no capability requirements it can be used in any step regardless of policy. All functions are deterministic and produce no side effects.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.json": "0.1.0"
```

No capability grants are required.

## API Reference

### Types

#### `JsonResult`

```
type JsonResult { ok: Bool, value: String, error: String }
```

A tagged result for operations that may fail. When `ok` is `true`, `value` holds the result; when `false`, `error` holds the reason.

### Functions

#### Result constructors

##### `json_ok(value: String) -> JsonResult`

Wraps a successful string result in a `JsonResult` with `ok = true`.

##### `json_err(error: String) -> JsonResult`

Wraps an error message in a `JsonResult` with `ok = false`.

#### Field serialisers

##### `json_string_field(key: String, value: String) -> String`

Returns a JSON key-value fragment for a string value: `"key": "value"`.

**Example**

```
fn main() -> Int {
  let field: String = json_string_field("name", "Alice")
  // field == "\"name\": \"Alice\""
  let obj: String = json_object(field)
  // obj == "{\"name\": \"Alice\"}"
  0
}
```

##### `json_int_field(key: String, value: Int) -> String`

Returns a JSON key-value fragment for an integer value: `"key": <n>`.

##### `json_bool_field(key: String, value: Bool) -> String`

Returns a JSON key-value fragment for a boolean value: `"key": true` or `"key": false`.

##### `json_null_field(key: String) -> String`

Returns a JSON key-value fragment for a null value: `"key": null`.

#### Container builders

##### `json_object(fields: String) -> String`

Wraps a pre-serialised fields string in `{...}`. Combine multiple fields with string concatenation before passing.

##### `json_array_wrap(items: String) -> String`

Wraps a pre-serialised items string in `[...]`.

#### Utilities

##### `json_escape(s: String) -> String`

Escapes a string for safe inclusion in a JSON value. Correctly escapes `"` and `\` characters.

##### `int_to_string(n: Int) -> String`

Converts an integer to its decimal string representation. Wraps `__builtin_int_to_string`.

## Capabilities

None. `std-json` is pure-functional with no side effects.

## Notes

- There is no recursive structure support; callers must build nested JSON by concatenating field strings manually before passing to `json_object`.

## Version History

| Version | Change |
|---------|--------|
| `0.1.0` | Initial release. `JsonResult` type; `json_ok`, `json_err`, `json_string_field`, `json_int_field`, `json_bool_field`, `json_null_field`, `json_object`, `json_array_wrap`, `json_escape`, `int_to_string`. |
