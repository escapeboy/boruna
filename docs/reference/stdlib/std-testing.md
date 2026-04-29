# std-testing

> High-level test helpers for framework apps

**Package:** `std.testing`  **Version:** `0.1.0`  **Capabilities required:** none

## Overview

`std-testing` provides assertion functions and aggregation helpers for writing deterministic unit tests directly in `.ax`. Use it inside `fn main()` test programs or alongside the `boruna framework test` command to verify app behavior. Because everything is pure, test programs are fully deterministic and replay-safe.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.testing": "0.1.0"
```

## API Reference

### Types

#### `TestResult`

```
type TestResult { passed: Int, label: String, detail: String }
```

- `passed` — `1` if the assertion succeeded, `0` if it failed
- `label` — the name of this test case
- `detail` — `"ok"` on success, or a short failure reason

#### `TestSummary`

```
type TestSummary { total: Int, passed: Int, failed: Int }
```

Aggregate result for a group of tests.

### Functions

#### Assertions

##### `assert_eq_int(actual: Int, expected: Int, label: String) -> TestResult`

Passes if `actual == expected`.

**Example**
```
fn main() -> Int {
  let r: TestResult = assert_eq_int(2 + 2, 4, "addition")
  r.passed
}
```

##### `assert_eq_string(actual: String, expected: String, label: String) -> TestResult`

Passes if `actual == expected`.

##### `assert_true(value: Int, label: String) -> TestResult`

Passes if `value == 1`.

##### `assert_false(value: Int, label: String) -> TestResult`

Passes if `value == 0`.

##### `assert_gt(actual: Int, threshold: Int, label: String) -> TestResult`

Passes if `actual > threshold`.

##### `assert_lt(actual: Int, threshold: Int, label: String) -> TestResult`

Passes if `actual < threshold`.

##### `assert_not_empty(value: String, label: String) -> TestResult`

Passes if `value != ""`.

#### Aggregation

##### `test_summary(t1: TestResult, t2: TestResult, t3: TestResult) -> TestSummary`

Computes a summary across exactly three test results.

##### `test_all_passed_2(t1: TestResult, t2: TestResult) -> Int`

Returns `1` if both results passed.

##### `test_all_passed_3(t1: TestResult, t2: TestResult, t3: TestResult) -> Int`

Returns `1` if all three results passed.

**Example: full test program**
```
fn main() -> Int {
  let r1: TestResult = assert_eq_int(1 + 1, 2, "basic addition")
  let r2: TestResult = assert_true(1, "literal true")
  let r3: TestResult = assert_not_empty("hello", "non-empty string")
  let summary: TestSummary = test_summary(r1, r2, r3)
  summary.failed
}
```

A `main` that returns `0` indicates all tests passed (zero failures).

## Capabilities

None. All functions are pure.

## Notes / Limitations

- `test_summary` handles exactly three tests. To summarize more, compute intermediate summaries or use `test_all_passed_2` / `test_all_passed_3` in a chain.
- There is no test runner built into the library itself. Run test programs with `boruna run <file>.ax` or `boruna framework test` for framework apps.
- Failure detail is limited to `"mismatch"`, `"expected true"`, `"expected false"`, `"not greater than"`, `"not less than"`, or `"was empty"`. Custom detail messages are not yet supported; use `label` to identify the assertion.
