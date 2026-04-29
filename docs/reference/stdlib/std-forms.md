# std-forms

> Model-driven form engine with validation and state tracking

**Package:** `std.forms`  **Version:** `0.1.0`  **Capabilities required:** none

## Overview

`std-forms` manages the lifecycle of multi-field forms inside framework apps. It provides two complementary state types — `FieldState` for individual inputs and `FormState` for the whole form — and a set of pure functions to transition between them. Use it in your `update(State, Msg) -> UpdateResult` handler to track dirty/touched flags, field errors, and submission state without writing that boilerplate yourself.

`std-forms` depends on `std.validation` for composable rule checking.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.forms": "0.1.0"
```

`std.validation` is pulled in automatically as a transitive dependency.

## API Reference

### Types

#### `FieldState`

```
type FieldState { name: String, value: String, touched: Int, dirty: Int, error: String }
```

- `touched` — `1` if the user has focused the field at least once
- `dirty` — `1` if the value has changed from its initial state
- `error` — current validation error message, or `""` when valid

#### `FormState`

```
type FormState {
    field_count: Int,
    submitted: Int,
    valid: Int,
    current_field: String,
    current_value: String,
    current_error: String
}
```

- `submitted` — `1` after a successful `form_submit`
- `valid` — `0` as soon as any field error is set; reset by `form_clear_errors`

### Functions

#### Form lifecycle

##### `form_init(field_count: Int) -> FormState`

Creates a fresh form. Pass the number of fields to pre-declare capacity.

##### `form_set_field(form: FormState, name: String, value: String) -> FormState`

Records the most-recently changed field name and value. Call this on every input change message.

##### `form_set_error(form: FormState, field: String, error: String) -> FormState`

Attaches a validation error to a named field and marks the form invalid.

##### `form_clear_errors(form: FormState) -> FormState`

Clears all errors and resets `valid` to `1`.

##### `form_submit(form: FormState) -> FormState`

Transitions the form to the submitted state if `valid == 1`. No-ops if the form is invalid.

##### `form_reset(form: FormState) -> FormState`

Resets all fields to their initial state while preserving `field_count`.

#### Form predicates

##### `form_is_valid(form: FormState) -> Int`

Returns `1` if no errors are set.

##### `form_is_submitted(form: FormState) -> Int`

Returns `1` after a successful submission.

#### Field lifecycle

##### `field_init(name: String) -> FieldState`

Creates a fresh field with an empty value and no errors.

##### `field_set_value(field: FieldState, value: String) -> FieldState`

Updates the field value and marks it dirty.

##### `field_touch(field: FieldState) -> FieldState`

Marks the field as touched (user has interacted with it).

##### `field_set_error(field: FieldState, error: String) -> FieldState`

Attaches a validation error message.

##### `field_clear_error(field: FieldState) -> FieldState`

Clears any error message.

##### `field_is_valid(field: FieldState) -> Int`

Returns `1` if the field has no error.

##### `field_reset(field: FieldState) -> FieldState`

Resets to empty, untouched, and error-free while keeping the field name.

**Example**
```
fn main() -> Int {
  let form: FormState = form_init(2)
  let name_field: FieldState = field_init("name")
  let name_typed: FieldState = field_set_value(name_field, "Alice")
  let name_visited: FieldState = field_touch(name_typed)
  let form2: FormState = form_set_field(form, "name", "Alice")
  let result: FormState = form_submit(form2)
  result.submitted
}
```

## Capabilities

None. All state transitions are pure functions.

## Notes / Limitations

- `FormState` stores only the most-recently changed field. Apps with multiple fields should maintain per-field `FieldState` values in their own `State` record and use `FormState` for overall validity and submission tracking.
- `field_count` is informational only; the engine does not iterate over fields.
