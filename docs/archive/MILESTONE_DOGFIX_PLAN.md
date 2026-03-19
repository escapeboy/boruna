# Milestone: Dogfix — Addressing Dogfood Findings

## Chosen Fixes

| Phase | Finding | Change | Risk |
|-------|---------|--------|------|
| 1 | F1: No dynamic collections | VM list opcodes + compiler emits `Value::List` | Medium — new opcodes |
| 2 | F5: No record update syntax | `..base` spread in record literals (compiler-only) | Low — no VM changes |
| 3 | F4: String matching broken | Fix match codegen + VM string comparison | Low — fix existing bug |
| 4 | F2+F3: Missing string ops | `parse_int`, `str_contains`, `str_starts_with` builtins | Low — new opcodes |
| — | F6/F7: Actors + effect exec | Deferred — not needed for this milestone | — |

## Phase 1 — Real List Support

### Semantics
- Lists are immutable persistent vectors (copy-on-write via Vec::clone + push).
- Element type: dynamic `Value` at VM level. Compiler tracks `List<T>` in AST.
- Lists serialize to JSON as `{"List": [...]}` (already supported by serde).
- Out-of-bounds `ListGet` produces `VmError::IndexOutOfBounds { index, length }`.

### Bytecode opcodes (4 new)

| Opcode | Byte | Stack effect | Description |
|--------|------|-------------|-------------|
| `ListNew` | 0x80 | → list | Push empty `Value::List(vec![])` |
| `ListLen` | 0x81 | list → int | Push `Value::Int(list.len())` |
| `ListGet` | 0x82 | list, index → value | Push `list[index]` or error |
| `ListPush` | 0x83 | list, value → list' | Push new list with value appended |

### Touchpoints

| Crate | File | Change |
|-------|------|--------|
| boruna-bytecode | opcode.rs | Add 4 opcodes + byte tags |
| boruna-bytecode | value.rs | No change (`Value::List` already exists) |
| boruna-vm | vm.rs | Handle 4 new opcodes in execute() |
| boruna-compiler | codegen.rs | `Expr::List` emits `MakeList` (new opcode) instead of `MakeRecord(0xFFFF)` |
| boruna-compiler | codegen.rs | Recognize `list_len()`, `list_get()`, `list_push()` as builtins → emit opcodes |
| boruna-framework | effect.rs | `as_list()` already handles `Value::List` — no change needed |
| boruna-framework | policy.rs | `as_list()` already handles `Value::List` — no change needed |

**Decision**: Instead of `MakeList`, reuse `MakeRecord(0xFFFF, N)` for list literals but convert at VM level. This avoids a new opcode for list construction. The VM already creates `Value::Record { type_id: 0xFFFF }` — we change it to create `Value::List` instead.

**Revised**: Actually simpler — add `MakeList(u8)` opcode. It's cleaner than overloading `MakeRecord`. Compiler emits `MakeList(N)` for list literals. VM creates `Value::List(vec![...])`.

| Opcode | Byte | Stack effect | Description |
|--------|------|-------------|-------------|
| `MakeList(u8)` | 0x80 | N values → list | Pop N values, create list |
| `ListLen` | 0x81 | list → int | Push length |
| `ListGet` | 0x82 | list, index → value | Index access |
| `ListPush` | 0x83 | list, value → list' | Append (returns new list) |

### Builtin functions (compiler-recognized)
- `list_len(x)` → emit `ListLen`
- `list_get(x, i)` → emit `ListGet`
- `list_push(x, v)` → emit `ListPush`

The compiler recognizes these names in `Expr::Call` and emits the corresponding opcodes directly instead of `Op::Call`.

### Backward compatibility
- Existing bytecode with `MakeRecord(0xFFFF, N)` continues to work — `as_list()` still handles it.
- New bytecode uses `MakeList(N)`. Old VMs will fail on new bytecode (forward-incompatible).
- Replay logs are unaffected — they record capability calls, not internal ops.
- State snapshots: `Value::List` serializes differently from `Record { type_id: 0xFFFF }`. **Breaking for snapshot comparison**, but not for replay (replay compares behavior, not serialized form).

## Phase 2 — Record Update Syntax

### Syntax
```
State { ..state, status: "ok", count: state.count + 1 }
```

### Semantics
- `..expr` must appear as the first entry in the field list.
- The base expression must evaluate to a Record with the same type.
- Overridden fields replace base fields; unspecified fields are copied.
- Compile-time: base record type must match target type name.

### AST change
```rust
Expr::Record { type_name, fields, spread: Option<Box<Expr>> }
```
Add `spread` field to `Expr::Record`.

### Touchpoints

| Crate | File | Change |
|-------|------|--------|
| boruna-compiler | ast.rs | Add `spread: Option<Box<Expr>>` to `Record` variant |
| boruna-compiler | parser.rs | Parse `..expr` before first field in record literal |
| boruna-compiler | codegen.rs | Emit spread: load base, GetField for non-overridden, override for specified |
| boruna-compiler | lexer.rs | Add `..` token (`DotDot`) |

### Lowering strategy
Given `State { ..base, field_a: val_a }` where State has fields [f0, f1, field_a, f3]:
1. Evaluate `base`, store in temp local.
2. For each field in the target type:
   - If overridden in the literal: emit the override expression.
   - Else: emit `LoadLocal(temp)` + `GetField(idx)`.
3. Emit `MakeRecord(type_id, total_fields)`.

No new VM opcodes needed.

### Backward compatibility
- Pure compiler change. Existing bytecode is unaffected.
- New source code compiles to standard `MakeRecord` — old VMs can run it.

## Phase 3 — String Matching in `match`

### Current state (broken)
`Pattern::StringLit` exists in the AST and parser but `pattern_to_tag()` maps it to `-1` (wildcard), making all string patterns act as catch-all.

### Fix strategy
Use a **string match table** approach:
1. Codegen: For string patterns, store the string literal in the constant pool. Use a special tag range (starting at 0x1000) to indicate "string match — compare with constant at index X".
2. VM: When processing a `Match` instruction, if the arm tag >= 0x1000, compare the scrutinee (which must be a `Value::String`) against the constant at index `tag - 0x1000`.

Actually, simpler approach: **Compile string match as a chain of if-else comparisons.**

The `match` with string patterns compiles to:
```
emit scrutinee → store in temp
LoadLocal(temp) → PushConst("create_user") → Eq → JmpIfNot(next_arm)
  ... arm body ... Jmp(end)
LoadLocal(temp) → PushConst("delete_user") → Eq → JmpIfNot(next_arm)
  ... arm body ... Jmp(end)
_ arm body
end:
```

This avoids changing the bytecode `MatchArm` structure entirely. The `Op::Match` instruction is used only for non-string patterns. For string match, the compiler emits comparison chains.

### Touchpoints

| Crate | File | Change |
|-------|------|--------|
| boruna-compiler | codegen.rs | Detect string patterns in match → emit comparison chain instead of `Op::Match` |
| boruna-bytecode | — | No changes |
| boruna-vm | — | No changes |

### Backward compatibility
- Pure compiler change. Output is standard Eq/JmpIfNot ops.
- Old VMs can run new bytecode.

## Phase 4 — String Builtins

### Functions
- `parse_int(s: String) -> Int` — Parse string to integer. Returns 0 on failure (or we use Option; for simplicity, return 0 on invalid input, matching the "no Option at call site" pattern).
  - **Decision**: Return `Value::Int(n)` on success, `Value::Int(0)` on failure. Reason: apps currently use Int for everything. Adding Option<Int> requires unwrapping patterns the language doesn't handle well.
- `str_contains(s: String, sub: String) -> Bool`
- `str_starts_with(s: String, prefix: String) -> Bool`

### Opcodes (3 new)

| Opcode | Byte | Stack effect | Description |
|--------|------|-------------|-------------|
| `ParseInt` | 0x84 | string → int | Parse or return 0 |
| `StrContains` | 0x85 | string, substring → bool | Contains check |
| `StrStartsWith` | 0x86 | string, prefix → bool | Prefix check |

### Touchpoints

| Crate | File | Change |
|-------|------|--------|
| boruna-bytecode | opcode.rs | Add 3 opcodes |
| boruna-vm | vm.rs | Handle 3 new opcodes |
| boruna-compiler | codegen.rs | Recognize builtin names → emit opcodes |

### Determinism
- `parse_int`: deterministic (no locale, pure ASCII digit parsing).
- `str_contains`: deterministic (byte-level comparison).
- `str_starts_with`: deterministic (byte-level comparison).

### Backward compatibility
- New opcodes — old VMs cannot run new bytecode using these.
- Replay unaffected (these are pure, no capability calls).

## Phase 5 — Deferred

F6 (actors) and F7 (effect execution) are not addressed. No changes needed for this milestone.

## Compatibility Guarantee

### Existing bytecode
- `.axbc` files compiled before this milestone **continue to run** on the updated VM.
- `MakeRecord(0xFFFF, N)` still works — the VM still handles it.
- `as_list()` helpers still handle both representations.

### Replay logs
- Replay logs record capability calls (CapCall/CapResult events).
- None of the changes affect capability calls.
- Replay logs remain valid and verifiable.

### State snapshots
- **Minor breaking change**: Recompiled apps will produce `Value::List(...)` instead of `Value::Record { type_id: 0xFFFF }` in state snapshots.
- This affects golden test hashes. Golden tests will be updated.
- Semantic equivalence is preserved — the data is the same.

### Migration
- No migration tooling needed.
- Recompile source → new bytecode uses new opcodes.
- Old bytecode files continue to run without recompilation.

## Expected Impact on Examples

| Example | Phase 1 (Lists) | Phase 2 (Spread) | Phase 3 (Match) | Phase 4 (Builtins) |
|---------|-----------------|-------------------|-----------------|---------------------|
| counter_app | No change | No change | No change | No change |
| todo_app | No change | No change | No change | No change |
| parallel_demo | No change | No change | No change | No change |
| admin_crud | No change | Reduce boilerplate | Replace if-else | No change |
| notification_app | No change | Reduce boilerplate | Replace if-else | Use parse_int |
| sync_todo_app | Store real todo list | Reduce boilerplate | Replace if-else | Use parse_int |

## Test Plan

Each phase adds:
1. Unit tests for new opcodes/features (in respective crate test files).
2. Golden determinism tests pass (updated golden values where snapshot format changed).
3. Replay equivalence tests pass.
4. All existing tests continue to pass.
