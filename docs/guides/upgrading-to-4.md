# Upgrading to Boruna 4.0

Boruna 4.0 ships `.ax` language 2.0. Two checks that were warnings in 3.x are now compile
errors, and a few programs that ran before now behave differently. Workflow files, evidence
bundles, policies and the MCP responses do not change.

3.x is end of life from the day 4.0.0 ships (see the [support policy](../lts.md)).

## 1. Find what no longer compiles

Run the checker on every `.ax` file. It lists every problem with a line number. The compiler
stops at the first one.

```bash
boruna lang check path/to/file.ax
```

| Code | What it rejects | How to fix it |
|---|---|---|
| `E010` | Reassigning a `let` declared without `mut` | `boruna lang repair path/to/file.ax` adds `mut` for you |
| `E010` | Reassigning a function parameter or a `for` variable | Copy it first: `let mut n_acc = n`, then assign to `n_acc` |
| `E009` | `let x: Int = "text"`: the annotation and the value disagree | Fix the annotation or the value |
| `E009` | `f("text")` where `f` takes an `Int` | Pass a value of the declared type |
| `E009` | `x = "text"` where `x` is an `Int` | Assign a value of the variable's type, or use a new binding |
| `E009` | `while 1 { ... }`: a condition that is not `Bool` | Write a comparison, e.g. `while n > 0` |

The type check is local and conservative. It only rejects a program when it can name both
types: literals, annotated bindings, record and enum constructors, and functions you defined.
It does not reject what it cannot see, such as the result of a built-in or an operator.

## 2. Check programs that relied on two old bugs

These programs compiled before and still compile, but their result changes.

**Names declared inside a block or a `match` arm now end there.** Before 4.0, a binding
declared inside `if`, a block, a loop body or a `match` arm replaced the outer binding with the
same name for the rest of the function:

```ax
fn main() -> Int {
    let n: Int = 1
    let o: Option<Int> = Some(40)
    let m: Int = match o {
        Some(n) => n
        None => 0
    }
    n + m
}
```

This returned `80` in 3.x and returns `41` now, as the language spec always said. Assigning to
an outer `let mut` binding from inside a block still updates it.

**An `if` without `else` used as a statement.** In 3.x, `if c { value }` followed by more
statements failed with "stack underflow" when `c` was false. It now runs on both paths. No
working program depended on the old behaviour, because it could only crash.

## 3. New words that cannot be names any more

`break` and `continue` are keywords. A variable, parameter or function with one of these
names has to be renamed.

## What you get in return

- `break` and `continue` in `while` and `for` loops. They work as statements in the loop body
  or in an `if` statement there. Inside an expression or a `match` arm they are a compile
  error.
- File operations for `--live` runs: `fs_list`, `fs_append` and `fs_delete`, next to
  `fs_read` and `fs_write`. They are limited to the policy's `fs_policy.allowed_roots`, like
  the others.
- `boruna ... | head` exits quietly instead of printing a panic when the reader closes the pipe.

The full list is in the [changelog](../../CHANGELOG.md).
