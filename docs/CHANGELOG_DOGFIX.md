# Dogfix Changelog

Addresses dogfood findings F1–F5 from DOGFOOD_FINDINGS.md.

## New VM Opcodes

| Opcode | Byte | Purpose |
|--------|------|---------|
| MakeList(n) | 0x80 | Create native `Value::List` from N stack values |
| ListLen | 0x81 | List length → Int |
| ListGet | 0x82 | Index into list (bounds-checked) |
| ListPush | 0x83 | Append to list (returns new list, original unchanged) |
| ParseInt | 0x84 | String → Int (0 on parse failure) |
| StrContains | 0x85 | Substring test → Bool |
| StrStartsWith | 0x86 | Prefix test → Bool |

Backward compatible: ListLen/ListGet/ListPush also handle legacy `Record { type_id: 0xFFFF }` lists.

## New Language Features

### Record Spread (fixes F5)
```
State { ..state, status: "ok", mode: "idle" }
```
Compiler-only change. Lowers to: store base in temp → GetField for non-overridden fields → MakeRecord. No new opcodes needed.

### String Match (fixes F4)
```
match msg.tag {
    "create" => ...,
    "delete" => ...,
    _ => default,
}
```
Compiler detects string patterns → emits comparison chain (LoadLocal + PushConst + Eq + JmpIfNot) instead of Op::Match table. No VM changes.

### List Operations (fixes F1)
```
let items: List<Int> = [1, 2, 3]
let len: Int = list_len(items)
let first: Int = list_get(items, 0)
let bigger: List<Int> = list_push(items, 4)
```
Compiler emits `Op::MakeList(N)` for list literals. Builtin functions compile to direct opcodes.

### String Builtins (fixes F2 + F3)
```
let n: Int = parse_int("42")
let has: Bool = str_contains("hello world", "world")
let pre: Bool = str_starts_with("hello", "he")
```

## Crates Modified

| Crate | Changes |
|-------|---------|
| boruna-bytecode | 7 new opcode variants + byte tags |
| boruna-vm | 7 opcode handlers, IndexOutOfBounds error, 20 new tests |
| boruna-compiler | Lexer (DotDot token), AST (spread field), parser (spread + string patterns), codegen (MakeList, builtins, spread lowering, string match chains), typechecker (builtin functions, spread validation), 22 new tests |

## Examples Rewritten

All three dogfood examples rewritten using match + spread:

| Example | Before | After | Reduction |
|---------|--------|-------|-----------|
| admin_crud_app.ax | 441 lines | 156 lines | 65% |
| notification_app.ax | 287 lines | 119 lines | 59% |
| sync_todo_app.ax | 423 lines | 161 lines | 62% |

All examples produce the same output as before (0, 3, 3).

## Test Summary

42 new tests (137 → 179 total):
- 20 VM opcode tests in `crates/boruna-vm/src/tests.rs`
- 22 compiler E2E tests in `crates/boruna-compiler/src/tests.rs`
- 76 framework tests unchanged (pass without modification)

## Not Addressed

- **F6** (Actor wiring) — deferred, requires framework runtime changes
- **F7** (Effect execution) — deferred, requires host integration
- **F8** (Bool fields) — low severity, Int workaround acceptable
