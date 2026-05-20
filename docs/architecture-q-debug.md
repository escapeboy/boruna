# Architecture — `debug` builtin

Companion to `docs/design-q-debug.md`. **DEFERRED past this sprint.**

> **Build-phase discovery (2026-05-20):** every `__builtin_*` in Boruna is realized as a
> dedicated opcode (`Op::StringToUpper`, `Op::ListLenBuiltin`, …). There is no string-dispatched
> intrinsic table at runtime — the compiler emits the right opcode per builtin name. Adding
> `debug` therefore requires:
>
> - 2 new variants in `crates/llmbc/src/opcode.rs` (`Op::Debug`, `Op::DebugMsg`)
> - Bumping `BYTECODE_VERSION` from `"1.0"` to `"1.1"` (per §1.2(6) of the spec, additive
>   opcodes are a minor bump)
> - Adding §4 entries to `docs/spec/bytecode-1.0.md` (or splitting the spec into `bytecode-1.1.md`)
> - VM dispatch arm in `crates/llmvm/src/vm.rs` per opcode
> - Codegen arm in `crates/llmc/src/codegen.rs` per builtin name
>
> That is clean spec evolution — within the additive contract — but it is not the "low-risk
> bundle" the user picked. Deferred to its own follow-up sprint where the bytecode evolution
> can be reviewed end-to-end (compiler + VM + spec + CHANGELOG `### Decided` entry per
> `project-conventions-2026-04` §19) without contaminating the lower-risk literate/ITF work.



## Component map

| Component | Location | Role |
|---|---|---|
| Builtin parsing | `crates/llmc/src/parser.rs` | No change — `debug` is a normal identifier |
| Builtin name resolution | `crates/llmc/src/codegen.rs` | New entries `__builtin_debug` / `__builtin_debug_msg` |
| VM intrinsic | `crates/llmvm/src/lib.rs` (where current intrinsics live) | New ops for stderr-print-and-return |
| Stdlib re-export | `libs/` core preamble | Optional `pub def debug(v) = __builtin_debug(v)` |

## Behavior

- `debug(value)` — prints `pretty(value)` to stderr, followed by newline. Returns `value`.
- `debug(msg, value)` — prints `msg + " " + pretty(value)` to stderr, followed by newline.
  Returns `value`.

## Codegen wiring

`crates/llmc/src/codegen.rs` currently dispatches builtins by name in the `Call` lowering
branch. Adding two new arms:

```rust
"__builtin_debug" if args.len() == 1 => {
    // compile single arg, emit OP_BUILTIN_DEBUG(1)
}
"__builtin_debug_msg" if args.len() == 2 => {
    // compile msg, then value, emit OP_BUILTIN_DEBUG(2)
}
```

OR (simpler, given the existing builtin pattern in the codegen): two distinct builtin function
indices, dispatched via the existing `__builtin_*` table without new opcodes.

**Decision:** reuse the existing string-dispatched builtin mechanism. No new opcode. The VM
intrinsic table grows by 2 entries.

## VM intrinsic dispatch

`crates/llmvm/src/...` (location TBD by inspection during build):

```rust
match builtin_name {
    "__builtin_debug" => {
        let value = stack.pop().unwrap();
        eprintln!("{}", value.pretty());
        stack.push(value);
    }
    "__builtin_debug_msg" => {
        let value = stack.pop().unwrap();
        let msg = match stack.pop().unwrap() {
            Value::String(s) => s,
            other => format!("{:?}", other),
        };
        eprintln!("{} {}", msg, value.pretty());
        stack.push(value);
    }
    // ... existing arms ...
}
```

## Surface in `.ax`

The user-visible name is just `debug` (one arg) or `debug` (two args). At the parser level
nothing changes. At name resolution, `debug(x)` lowers to `__builtin_debug(x)` and
`debug(msg, x)` lowers to `__builtin_debug_msg(msg, x)`.

**Open: how does the resolver disambiguate two arities?** Two options:

1. **Surface name overloading via arity:** the resolver looks at `args.len()` and picks the
   right `__builtin_debug*`. This matches how some existing builtins seem to work
   (`__builtin_list_*` arity-keyed).
2. **Two surface names:** `debug` (one-arg) and `debug_msg` (two-arg). Loses some convenience
   but no resolver complexity.

**Decision during build:** start with option 2 (two surface names: `debug`, `debug_msg`). It's
simpler and matches Boruna's existing "no overloading" stance. The README/CHANGELOG names the
two-arg form `debug_msg`. If the dev experience suffers, we can revisit overloading in a
later sprint.

## Typing

Both are polymorphic identity functions:

```
debug : forall a. a -> a
debug_msg : forall a. (String, a) -> a
```

In the typechecker, these are registered as the same shape as existing polymorphic builtins.
Compatible with all `Value` variants since output type = input type.

## Determinism / capability

- No capability gating. Per design doc decision #2 — `debug` is OS-level stderr, not the
  capability-mediated `print`.
- Per §15: output to stderr is operational-only. Never affects evidence bundles or replay.
- VM event log: no entry emitted for `debug` calls. They are invisible to replay.

## File map (new code this sprint)

| File | LoC est. |
|---|---|
| `crates/llmc/src/codegen.rs` (add 2 builtin arms) | +20 |
| `crates/llmvm/src/...` (add 2 intrinsics + stack handling) | +30 |
| `crates/llmc/tests/debug_builtin.rs` (compile-time tests) | ~60 |
| `crates/llmvm/tests/debug_builtin.rs` (runtime tests) | ~80 |
| `docs/reference/ax-language.md` (document) | +25 |

**Total: ~215 lines.** Smallest of the three implementation items.

## Test plan reference

Test plan: `docs/test-plan-q-debug.md` (written this sprint).
