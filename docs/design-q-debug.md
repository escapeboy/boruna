# Design — `debug` builtin (print-and-pass-through)

## Status

Planned for v1.5.0. **Implemented this sprint** (per `/sprint-orchestrate full`).
Borrowed from Quint's `q::debug`. Source: research_quint_borrowable_ideas_2026-05-20.md, rec #8.

## Context

Quint provides `q::debug(msg, value)` that prints `msg value` to stderr and returns `value`
unchanged. Works inline inside any expression: `(x' = q::debug("new x:", x + 1))`. It's the
`dbg!` macro equivalent for spec authors — print-and-passthrough without restructuring the
expression.

Boruna's `.ax` has no equivalent. Today, debugging an intermediate value means restructuring
the surrounding `let` chain into multiple bindings. That's friction. The `dbg!` macro-equivalent
ergonomics are universally valued (Rust, OCaml's `dump`, Haskell's `Debug.Trace.trace`).

## Why

Trivial cost, real DX win. Pairs naturally with the REPL (when REPL ships, `debug` lets users
sprinkle traces in the loaded module and see them on every eval).

## Goals

1. `debug(value)` — prints `value` to stderr in pretty-printed form; returns `value` unchanged.
2. `debug(msg, value)` — prints `msg + " " + pretty(value)` to stderr; returns `value` unchanged.
3. Behavior is consistent across the VM and the REPL (when REPL ships).
4. Output goes to stderr (so it doesn't pollute stdout pipelines).
5. No capability required (debug printing is OS-level stdout, not the gated `print` capability).

## Non-goals

- Conditional debug (`debug_if(cond, ...)`). Users can wrap themselves.
- Tracing-level integration (`tracing::debug!`-style). This is for `.ax` authors, not embedding
  hosts.
- Suppressing debug output via a flag. If you don't want output, don't call `debug`.
- Persisting debug output to the evidence bundle. Operational-only side effect.

## Forcing questions

**Who needs this? What are they doing today?**
Anyone writing a non-trivial `.ax` expression. Today: extracting the intermediate to a `let`
binding, calling `__builtin_int_to_string` + `step_input`, restructuring control flow.
With `debug`: one inline call.

**What's the narrowest MVP someone would pay for?**
Two-argument form. One-argument is convenience.

**What would make someone say "whoa"?**
That this *isn't already in the language.* It's universally expected of any modern lang.

**How does this compound over time?**
Once REPL ships, `debug` becomes the natural exploration tool. Once `simulate` ships, `debug`
in invariants helps debug witness predicates.

## Scope (this sprint)

| In | Out |
|---|---|
| `debug(value: a) -> a` builtin | Conditional `debug_if` variants |
| `debug(msg: String, value: a) -> a` builtin | Output redirection / suppression flags |
| Pretty-printing via existing `Value::pretty` | Custom format strings |
| Stderr write, no capability check | Tracing-crate integration |
| Tests covering both arities + Result/Option pretty | (no MCP tool exposure this sprint) |

## Decisions

1. **Builtin naming:** `debug` (no namespace), not `q::debug`. The Quint name is Quint-branded;
   Boruna uses unprefixed names matching the existing builtin convention (`__builtin_list_len`,
   etc.). At the `.ax` source level, it's just `debug(...)`.
2. **Codegen:** Add `__builtin_debug` (1-arg) and `__builtin_debug_msg` (2-arg) opcodes in
   `crates/llmc/src/codegen.rs` following the existing builtin-dispatch pattern.
3. **VM intrinsic:** Two intrinsics in `crates/llmvm/src/intrinsics.rs` (or wherever existing
   intrinsics live) that pop arg(s), print to stderr, push result.
4. **Output format:** uses the existing `Value::pretty` (already exists for the dashboard /
   evidence inspect). One-arg form prints `value`; two-arg form prints `msg ⎵ value`. No
   ANSI codes — matches the `boruna --json` ethos that all output should be machine-stable.
5. **Polymorphism:** the typechecker treats `debug` as a polymorphic identity function
   `forall a. a -> a` (one-arg form) or `forall a. (String, a) -> a` (two-arg form). The two
   forms are separate names at the surface — Boruna doesn't currently support overloading.

## Risks

- **Determinism contract:** `debug` is a side-effecting function. Per §15, side-effecting
  evaluations must not feed replay-verified state. Since `debug` returns its input value
  unchanged, the evidence bundle is unaffected (the value flow is pure-equivalent). Stderr
  output is operational-only. Documented in CHANGELOG.
- **Float/Record pretty-printing.** `Value::pretty` for records can be long. Mitigation: rely
  on existing truncation in `Value::pretty` (it already handles deep / long records). No new
  truncation logic in `debug`.

## Implementation effort estimate

- Codegen entries (2 builtins): ~20 lines
- VM intrinsics: ~15 lines
- Tests in compiler + VM: ~80 lines
- Documentation update in `docs/reference/ax-language.md` or builtin reference: ~20 lines

**Total: ~135 lines.** Smallest of the three implementation items.
