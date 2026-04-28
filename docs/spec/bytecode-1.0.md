---
spec_id: bytecode
bytecode_version: "1.0"
status: stable
last_revised: 2026-04-28
sprint: W9-A
audience: VM implementers, compiler authors, replay/evidence tooling
---

# Boruna Bytecode Specification — Version 1.0

This document is the **formal specification** of the Boruna bytecode format (`.boruna_bytecode` / `.axbc`). It is the authoritative reference for any independent implementation of a Boruna VM, replay engine, or evidence verifier.

The narrative, example-driven companion is [`docs/bytecode-spec.md`](../bytecode-spec.md). When the two disagree, **this spec wins**.

The reference implementation lives in `crates/llmbc/` (`Op`, `Value`, `Capability`, `Module`) and `crates/llmvm/` (the bytecode interpreter) of the Boruna repository. Implementations against this spec MAY use a different code organization but MUST be observationally equivalent to the rules below.

## 1. Conformance and versioning

### 1.1 Version identifier

The current bytecode version is **`1.0`**. Implementations MUST expose this value programmatically.

In the reference implementation:

```rust
// crates/llmbc/src/lib.rs
pub const BYTECODE_VERSION: &str = "1.0";
```

The spec version `1.0` is the public, semver-like format identifier. The on-disk module header carries an internal `version` byte (currently `1`, see §3.1) which is incremented for any wire-format change inside the `1.x` line; clarifying spec edits do not bump it.

The `BYTECODE_VERSION` string is a `<major>.<minor>` decimal number. A bytecode module emitted against `1.x` MUST load and execute against any `1.y` VM where `y >= x`.

### 1.2 Backwards-compatibility commitment for 1.x

Within the `1.x` line:

1. **Trace-and-output stability.** Any 1.0 bytecode module loaded by any 1.x VM MUST produce the same trace (event log) and the same output value, given the same recorded capability outcomes.
2. **Opcode discriminants frozen.** The byte tags assigned in §4.2 are locked. Reusing a tag for a different opcode is a 2.0 break.
3. **Capability IDs frozen.** The (id, name) pairs in §6.1 are locked for 1.x. Reordering or renaming is a 2.0 break.
4. **Value variants frozen.** The set and shape of `Value` variants in §5.1 are locked. New variants are a 2.0 break (existing modules MAY contain them only if every reachable VM supports them — i.e. additive only at a 1.y minor bump with reader gating).
5. **Module header layout frozen.** The magic bytes, version field width, and length-prefixed payload structure (§3.1) are locked.
6. **Additive opcodes are a minor bump.** A 1.y `y > 0` MAY introduce new opcodes at currently-unassigned discriminants. A 1.0 reader presented with such a module MUST reject the module with a typed unknown-opcode error rather than guess. (See §10.)

Breaking changes (renames, removals, discriminant reuse, header reshuffles) are deferred to `2.0` or later.

### 1.3 Conformance levels

A `1.0` VM implementation MUST:

- Accept any module whose header magic and bytecode_version match §3.1 and whose internal payload deserializes against §3.2.
- Execute every opcode in §4 with the stack/operand effects and behavior described.
- Enforce capability gating (§6) against the active host policy at every `CapCall`.
- Preserve determinism (§7) for all pure code (code paths not invoking `CapCall`, `SpawnActor`, `SendMsg`, or `ReceiveMsg`).
- Reject unknown opcodes, unknown capability IDs, and `bytecode_version >= 2` modules with a typed error (§10).

A `1.0` VM implementation MAY:

- Provide additional debugging, tracing, or profiling instrumentation, as long as it does not change observable execution behavior.
- Implement opcodes via JIT or AOT translation, as long as the stack effect and outputs match.

## 2. Document conventions

- "Reader" means any consumer of a bytecode module (VM, evidence verifier, disassembler).
- "Writer" means any producer of a bytecode module (the compiler, codegen tools).
- "MUST" / "MUST NOT" / "SHOULD" / "MAY" follow [RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) usage.
- All multi-byte integers are **little-endian** unless explicitly stated.
- "Stack effect" describes operand stack changes during execution (not constant-pool or local-frame changes).

## 3. Module format

A bytecode module is the unit of distribution. Each module is a self-contained graph of functions, types, constants, and globals.

### 3.1 On-disk header

Modules are persisted in a length-prefixed binary container:

| Offset | Size (bytes) | Field            | Notes                                                    |
|-------:|-------------:|------------------|----------------------------------------------------------|
| 0      | 4            | `magic`          | Exactly the four bytes `0x4C 0x4C 0x4D 0x42` (`"LLMB"`). |
| 4      | 2            | `version`        | Little-endian `u16`. `0x0001` in 1.0.                   |
| 6      | 4            | `payload_length` | Little-endian `u32`. Byte length of the payload that follows. |
| 10     | `payload_length` | `payload`    | UTF-8 JSON encoding of the `Module` struct (§3.2).      |

Readers MUST:

- Reject any input shorter than 10 bytes with a typed error.
- Reject any input whose first four bytes do not equal the magic.
- Reject any input whose `version` field is not `0x0001` with an `UnsupportedVersion` error. This is the §1 application: reject across-major bytecode_version, never silently accept.
- Reject any input where `data.len() < 10 + payload_length` with a truncation error.

### 3.2 Payload encoding

The payload is canonical JSON of the `Module` shape. Field order in the on-disk representation matches Rust struct field order; readers MUST tolerate any order (JSON object semantics).

```json
{
  "name": "<module name>",
  "version": 1,
  "constants": [<Value>, ...],
  "globals": ["<global name>", ...],
  "types":   [<TypeDef>, ...],
  "functions": [<Function>, ...],
  "entry": <function index, u32>
}
```

The reference encoder uses `serde_json::to_vec` (no whitespace control; readers MUST NOT depend on a specific JSON whitespace style). The reference decoder uses `serde_json::from_slice`.

**Encoding choice (informative).** 1.0 specifies a JSON payload wrapped in the binary header. This choice trades raw bytes-per-module for human-debuggability and stable cross-language deserialization. A future major (`2.0`) MAY swap to a denser binary encoding (CBOR, postcard, custom); until then, the JSON contract is part of the surface.

### 3.3 Sections

The payload contains five logical sections:

- **`name`** — UTF-8 module identifier. Used for diagnostics; not security-load-bearing.
- **`version`** — Internal `u16` version of the wire format; currently `1`. This is distinct from the spec `bytecode_version` (§1.1).
- **`constants`** — The constant pool. Indexed by `PushConst(idx)`. Each entry is a `Value` (§5).
- **`globals`** — Names of module-level globals. Indexed by `LoadGlobal(idx)` / `StoreGlobal(idx)`.
- **`types`** — Type definitions for records and enums. See §3.4. Indexed by the `type_id` operand of `MakeRecord`, `MakeEnum`, and the `type_id` field of `Value::Record` / `Value::Enum`.
- **`functions`** — The function table. Each entry is a `Function`. Indexed by `Call(fn_idx, _)`, `SpawnActor(fn_idx)`, and `Value::FnRef(idx)`.
- **`entry`** — The index into `functions` of the program entry point (`main`).

There is no separate "capability table" in the wire format; declared capabilities live on each `Function` (§3.4) as a `Vec<Capability>`. The frozen capability namespace is in §6.

### 3.4 Function and type shapes

```text
Function {
  name: String,
  arity: u8,
  locals: u16,
  code: Vec<Op>,
  capabilities: Vec<Capability>,
  match_tables: Vec<Vec<MatchArm>>,
}

MatchArm {
  tag: i32,        // variant index or literal tag; -1 = wildcard
  target: u32,     // jump offset within the function's code
}

TypeDef {
  name: String,
  kind: TypeKind,
}

TypeKind ::=
    Record { fields: Vec<(String, String)> }      // (field name, type name)
  | Enum   { variants: Vec<(String, Option<String>)> }  // (variant name, optional payload type name)
```

Field invariants:

- `arity` is the number of arguments the function expects on the stack at entry. The VM pops them into locals 0..arity.
- `locals` is the total local slot count (including parameters).
- `capabilities` lists the static capability set declared by this function (§6).
- `match_tables` are referenced by `Match(table_idx)` opcodes; each entry is the arm list for one match.

## 4. Opcodes

The VM is **stack-based**. Each function body is a flat sequence of `Op`s. The operand stack and the local frame are private to each function activation.

### 4.1 Stack effect notation

`(a, b → c)` reads as: pops `a` then `b` (so `b` is on top), pushes `c`. Order matters; the stack-top operand is the rightmost in the "before" list.

`(→ x)` pushes `x` onto the stack; `(x →)` pops `x` from the stack.

### 4.2 Opcode table

Every opcode below is part of the frozen 1.0 set. The byte tag column gives the discriminant used by `Op::to_byte_tag()` in the reference implementation; tags are locked for 1.x.

| Opcode                 | Byte tag | Stack effect                       | Behavior                                                                                                       |
|------------------------|---------:|------------------------------------|----------------------------------------------------------------------------------------------------------------|
| `PushConst(idx)`       | `0x01`   | (→ v)                              | Push `constants[idx]`. Deterministic.                                                                          |
| `LoadLocal(idx)`       | `0x02`   | (→ v)                              | Push `locals[idx]` from the current frame. Deterministic.                                                      |
| `StoreLocal(idx)`      | `0x03`   | (v →)                              | Pop and store into `locals[idx]`. Deterministic.                                                               |
| `LoadGlobal(idx)`      | `0x04`   | (→ v)                              | Push `globals[idx]` from the module-level globals. Deterministic.                                              |
| `StoreGlobal(idx)`     | `0x05`   | (v →)                              | Pop and store into `globals[idx]`. Deterministic.                                                              |
| `Call(fn_idx, arity)`  | `0x06`   | (a₁..aₙ → r)                       | Pop `arity` args (left-to-right pushed → rightmost on top), call `functions[fn_idx]`, push return value.       |
| `Ret`                  | `0x07`   | (v →)                              | Return from current function. Pops return value from the callee stack and pushes it on the caller stack.       |
| `Jmp(off)`             | `0x08`   | (—)                                | Unconditional jump to instruction offset `off` within the current function.                                    |
| `JmpIf(off)`           | `0x09`   | (cond →)                           | Pop; if `cond.is_truthy()` (§5.2), jump to `off`. Otherwise fall through.                                     |
| `JmpIfNot(off)`        | `0x0A`   | (cond →)                           | Pop; if NOT `cond.is_truthy()`, jump to `off`. Otherwise fall through.                                         |
| `Match(table_idx)`     | `0x0B`   | (v → ...)                          | Pop scrutinee; consult `match_tables[table_idx]`. First arm whose `tag` matches: jump to `arm.target`. Fallthrough on wildcard (`tag == -1`). |
| `MakeRecord(type_id, n)` | `0x0C` | (f₁..fₙ → r)                       | Pop `n` field values, push `Value::Record { type_id, fields }` with fields in pop-order reversed.              |
| `MakeEnum(type_id, var)` | `0x0D` | (payload → e)                      | Pop one payload value (or `Unit`), push `Value::Enum { type_id, variant: var, payload }`.                      |
| `GetField(idx)`        | `0x0E`   | (r → v)                            | Pop a `Record`, push the field at index `idx`.                                                                 |
| `SpawnActor(fn_idx)`   | `0x0F`   | (→ id)                             | Spawn an actor running `functions[fn_idx]`; push `Value::ActorId(_)`. **Capability-gated** by `actor.spawn`.    |
| `SendMsg`              | `0x10`   | (target_id, msg →)                 | Pop message and target id; deliver to actor mailbox. **Capability-gated** by `actor.send`.                     |
| `ReceiveMsg`           | `0x11`   | (→ msg)                            | Block current actor until a message arrives; push it.                                                          |
| `Assert(err_idx)`      | `0x12`   | (v →)                              | Pop; if `!v.is_truthy()`, abort with `constants[err_idx]` as the error. Otherwise no-op.                       |
| `CapCall(cap_id, n)`   | `0x13`   | (a₁..aₙ → r)                       | Pop `n` args, invoke capability `cap_id` (§6) via the host gateway, push the result. **Capability-gated.**     |
| `Add`                  | `0x20`   | (a, b → a+b)                       | Numeric add. Both operands MUST be `Int`+`Int` or `Float`+`Float`.                                             |
| `Sub`                  | `0x21`   | (a, b → a−b)                       | Numeric subtract.                                                                                              |
| `Mul`                  | `0x22`   | (a, b → a·b)                       | Numeric multiply.                                                                                              |
| `Div`                  | `0x23`   | (a, b → a/b)                       | Numeric divide. `Int / 0` traps; `Float / 0.0` follows IEEE 754 (`±inf` / `NaN`).                              |
| `Mod`                  | `0x24`   | (a, b → a%b)                       | Integer remainder. Defined for `Int` only; `Mod 0` traps.                                                      |
| `Neg`                  | `0x25`   | (a → −a)                           | Numeric negation.                                                                                              |
| `Eq`                   | `0x30`   | (a, b → Bool)                      | Structural equality (§5.3).                                                                                    |
| `Neq`                  | `0x31`   | (a, b → Bool)                      | Negated `Eq`.                                                                                                  |
| `Lt`                   | `0x32`   | (a, b → Bool)                      | Total ordering for `Int`, `Float`, `String` (§5.3). Other types: VM error.                                     |
| `Lte`                  | `0x33`   | (a, b → Bool)                      | Same domain as `Lt`.                                                                                           |
| `Gt`                   | `0x34`   | (a, b → Bool)                      | Same domain as `Lt`.                                                                                           |
| `Gte`                  | `0x35`   | (a, b → Bool)                      | Same domain as `Lt`.                                                                                           |
| `Not`                  | `0x40`   | (a → Bool)                         | Boolean negation; argument MUST be `Bool`.                                                                     |
| `And`                  | `0x41`   | (a, b → Bool)                      | Boolean AND. Eager (both operands evaluated by codegen before the opcode).                                     |
| `Or`                   | `0x42`   | (a, b → Bool)                      | Boolean OR. Eager.                                                                                             |
| `Concat`               | `0x50`   | (s1, s2 → s)                       | UTF-8 string concatenation; both operands MUST be `String`.                                                    |
| `Pop`                  | `0x60`   | (v →)                              | Discard top of stack.                                                                                          |
| `Dup`                  | `0x61`   | (v → v, v)                         | Duplicate top of stack. Reference-counted clone for owned data.                                                |
| `EmitUi`               | `0x70`   | (tree → tree)                      | Emit a UI descriptor (top of stack) to the host UI sink. Stack effect is conventionally the identity (the tree is left on the stack for chaining); see §4.3. **Capability-gated** by `ui.render`. |
| `MakeList(n)`          | `0x80`   | (v₁..vₙ → list)                    | Pop `n` values (rightmost on top), push a `List` containing them in pop-reversed order.                        |
| `ListLen`              | `0x81`   | (list → Int)                       | Pop a `List`, push its length.                                                                                 |
| `ListGet`              | `0x82`   | (list, idx → v)                    | Pop list and index. If `0 <= idx < len`, push the element. Out-of-bounds traps.                                |
| `ListPush`             | `0x83`   | (list, v → list')                  | Pop list and value; push a NEW list (immutable; original unchanged) with the value appended.                   |
| `ParseInt`             | `0x84`   | (String → Int)                     | Pop a `String`, parse as decimal `i64`; push `0` on parse failure. Determinism preserved.                      |
| `StrContains`          | `0x85`   | (haystack, needle → Bool)          | Pop two `String`s; push `Bool`.                                                                                |
| `StrStartsWith`        | `0x86`   | (string, prefix → Bool)            | Pop two `String`s; push `Bool`.                                                                                |
| `TryParseInt`          | `0x87`   | (String → Result<Int, String>)     | Pop a `String`; push `Ok(Int)` on success, `Err(String)` on failure.                                           |
| `Nop`                  | `0xFE`   | (—)                                | No effect. Useful for jump targets.                                                                            |
| `Halt`                 | `0xFF`   | (—)                                | Halt VM execution. The top of stack is the program result.                                                     |

### 4.3 Per-opcode semantic notes

- `EmitUi`'s observable effect is the rendered tree. The VM treats it as identity on the stack; the host UI sink is invoked with the tree as a side effect at the `ui.render` capability boundary.
- `Call` records a return pointer in the call stack; `Ret` returns to it. The depth of the call stack is host-bounded.
- `Match` arms are tried in order; the first match wins. A `tag == -1` arm is unconditional and acts as the wildcard.
- `SpawnActor`, `SendMsg`, and `ReceiveMsg` interact with the host scheduler. The scheduler ordering MUST be deterministic (round-robin sorted by `(target_id, sender_id)`; see [`docs/concepts/determinism.md`](../concepts/determinism.md)).

### 4.4 Reserved opcode space

The byte tag space contains gaps (e.g. `0x14`–`0x1F`, `0x26`–`0x2F`, `0x36`–`0x3F`, `0x43`–`0x4F`, `0x51`–`0x5F`, `0x62`–`0x6F`, `0x71`–`0x7F`, `0x88`–`0xFD`). These are **reserved for additive opcodes in 1.y minor bumps**. A 1.0 reader MUST reject any encountered tag not listed in §4.2 with an unknown-opcode error.

## 5. Value model

### 5.1 Value variants

The frozen `Value` discriminant set in 1.0:

| Variant            | Payload                                           | Type tag (`type_name()`) | Notes                                                                                  |
|--------------------|---------------------------------------------------|--------------------------|----------------------------------------------------------------------------------------|
| `Unit`             | (none)                                            | `"Unit"`                 | The single inhabitant of the `Unit` type.                                              |
| `Bool(b)`          | `bool`                                            | `"Bool"`                 |                                                                                        |
| `Int(n)`           | `i64`                                             | `"Int"`                  | 64-bit signed two's complement.                                                        |
| `Float(f)`         | `f64`                                             | `"Float"`                | IEEE 754 double. See §7 for determinism caveats.                                       |
| `String(s)`        | UTF-8 `String`                                    | `"String"`               |                                                                                        |
| `None`             | (none)                                            | `"None"`                 | `Option::None`.                                                                        |
| `Some(v)`          | `Box<Value>`                                      | `"Some"`                 | `Option::Some`.                                                                        |
| `Ok(v)`            | `Box<Value>`                                      | `"Ok"`                   | `Result::Ok`.                                                                          |
| `Err(v)`           | `Box<Value>`                                      | `"Err"`                  | `Result::Err`.                                                                         |
| `Record`           | `{ type_id: u32, fields: Vec<Value> }`            | `"Record"`               | Field order matches the corresponding `TypeDef::Record::fields` order.                 |
| `Enum`             | `{ type_id: u32, variant: u8, payload: Box<Value> }` | `"Enum"`              | Variant index matches `TypeDef::Enum::variants` order.                                 |
| `List(items)`      | `Vec<Value>`                                      | `"List"`                 | Insertion-ordered, immutable; `ListPush` returns a new list.                           |
| `Map(entries)`     | `BTreeMap<String, Value>`                         | `"Map"`                  | **Always key-sorted** in 1.0. See §7.3.                                                |
| `ActorId(id)`      | `u64`                                             | `"ActorId"`              | Opaque actor handle. Compared by id.                                                   |
| `FnRef(idx)`       | `u32`                                             | `"FnRef"`                | Function-table index. Used for higher-order references.                                |

This set is **frozen for 1.x**. Adding a variant is a 2.0 break.

### 5.2 Truthiness

The `is_truthy(v)` predicate, used by `JmpIf`, `JmpIfNot`, and `Assert`:

| Variant      | Truthy iff                  |
|--------------|-----------------------------|
| `Unit`       | always false                |
| `Bool(b)`    | `b == true`                 |
| `Int(n)`     | `n != 0`                    |
| `Float(f)`   | `f != 0.0` (note: `NaN != 0.0` is true under IEEE) |
| `String(s)`  | `!s.is_empty()`             |
| `None`       | always false                |
| `Some(_)`    | always true                 |
| `Ok(_)`      | always true                 |
| `Err(_)`     | always false                |
| `Record`     | always true                 |
| `Enum`       | always true                 |
| `List(l)`    | `!l.is_empty()`             |
| `Map(m)`     | `!m.is_empty()`             |
| `ActorId(_)` | always true                 |
| `FnRef(_)`   | always true                 |

### 5.3 Equality and ordering

- **Equality (`Eq` / `Neq`)** is structural: same variant + same payload (recursively, with byte-equal `String`, IEEE-bit-equal `Float`, structural `Vec` and `BTreeMap`). `NaN != NaN` (IEEE); writers SHOULD avoid relying on `Float` equality.
- **Ordering (`Lt`, `Lte`, `Gt`, `Gte`)** is defined for `Int` (signed numeric), `Float` (IEEE; `NaN` comparisons are always false), and `String` (lexicographic UTF-8 byte order). Ordering on other types is a runtime error.
- The discriminant order in §5.1 is **not** an ordering relation; cross-variant comparisons (e.g. `Int < String`) trap.

### 5.4 In-memory representation

The reference VM holds `Value`s as boxed/refcounted handles (Rust `Box`/`Vec`/`String`/`BTreeMap`). Implementations MAY use any representation that preserves the equality, ordering, and truthiness contracts above.

### 5.5 Serialization to evidence-bundle JSON

Evidence bundles serialize `Value`s as canonical JSON via the same `serde::Serialize` derive used for the bytecode payload (§3.2). The JSON surface for each variant mirrors the Rust enum's tagged form (e.g. `Some(v)` → `{"Some": <v>}`; `Ok(v)` → `{"Ok": <v>}`; `Record` → `{"Record": {"type_id": ..., "fields": [...]}}`). Because `Map` is a `BTreeMap`, the JSON object key order in the bundle is **always sorted by key** — this is a load-bearing determinism contract (§7.3) for `workflow_hash` and replay verification.

## 6. Capability table

### 6.1 Frozen 1.0 capability namespace

Implementations MUST recognize each capability below by both its numeric ID and its wire string. The (id, name) pairs are **locked for 1.x**. Reordering is a 2.0 break.

| Capability   | Numeric ID | Wire name     | Replay-verified? | Description                                          |
|--------------|-----------:|---------------|------------------|------------------------------------------------------|
| `NetFetch`   | 0          | `net.fetch`   | yes              | HTTP request; result recorded in event log.          |
| `FsRead`     | 1          | `fs.read`     | yes              | Read file contents.                                  |
| `FsWrite`    | 2          | `fs.write`    | yes              | Write file contents.                                 |
| `DbQuery`    | 3          | `db.query`    | yes              | Database query.                                      |
| `UiRender`   | 4          | `ui.render`   | operational      | Emit a UI tree to the host sink.                     |
| `TimeNow`    | 5          | `time.now`    | yes              | Read wall clock; replay reproduces the recorded value. |
| `Random`     | 6          | `random`      | yes              | Generate randomness; replay reproduces recorded bytes. |
| `LlmCall`    | 7          | `llm.call`    | yes              | LLM provider invocation.                             |
| `ActorSpawn` | 8          | `actor.spawn` | yes              | Spawn an actor.                                      |
| `ActorSend`  | 9          | `actor.send`  | yes              | Send a message to an actor.                          |
| `StepInput`  | 10         | `step.input`  | yes              | Read an upstream workflow step's resolved input.     |

The 1.0 capability set has **exactly 11 entries**. Implementations MAY add new capabilities at IDs ≥ 11 in future 1.y releases; previously-assigned IDs and names MUST NOT change.

The `Capability::ALL` constant in the reference implementation enumerates these in **canonical sorted-by-name order** (locked by `tests::test_capability_all_is_sorted_by_name`). This sort order feeds `compute_capability_set_hash` (see [`docs/reference/capability-identity.md`](../reference/capability-identity.md)).

### 6.2 Capability semantics

- **Static side.** A `Function`'s `capabilities: Vec<Capability>` declares the set of capabilities its body MAY directly invoke. The compiler enforces propagation (every callee's set is a subset of the caller's; see [`docs/spec/ax-language-1.0.md`](./ax-language-1.0.md) §6.3).
- **Runtime side.** At each `CapCall`, the host gateway checks the capability id against the active policy Π. If absent, the call fails with a capability-denied error. If present, the gateway dispatches to the registered handler.
- **Recording.** Replay-verified capabilities have their (request, result) pair appended to the event log. Operational capabilities (currently only `ui.render`) emit observable side effects but are NOT replayed for output verification.

### 6.3 Capability contract version

Each capability also carries a contract `version: &'static str` (currently `"1"` for all 11). The version pins the call/return shape and side-effect semantics; it is bumped only on contract-breaking changes (not on every binary release). The combined `(name, version)` set is hashed by `compute_capability_set_hash` and exposed via the `CapabilitySetReport` wire structure. See [`docs/reference/capability-identity.md`](../reference/capability-identity.md) for the byte-exact hash algorithm.

## 7. Determinism contract

The bytecode VM promises that two runs of the same module against the same recorded capability outcomes produce **byte-identical** observable output, traces, and evidence-bundle hashes.

### 7.1 Pure code

Code paths that do not invoke `CapCall`, `SpawnActor`, `SendMsg`, `ReceiveMsg`, or `EmitUi` are deterministic by construction:

- All `Value`s are immutable.
- All control flow is data-driven from the operand stack and locals.
- No opcode reads the system clock, randomness, or environment.
- No opcode depends on memory addresses, hash randomization, or thread scheduling.

### 7.2 Floating-point

`Float` arithmetic uses IEEE 754 double precision. The bytecode VM does NOT reorder operations, fuse multiply-adds, or use higher-precision intermediates. The same module on different hardware MUST produce identical bit patterns for `Add`, `Sub`, `Mul`, `Div`, and `Neg` over `Float` operands.

`NaN` payloads are NOT canonicalized; bit patterns from upstream computations propagate unchanged. Writers SHOULD treat `NaN` as a non-deterministic source and avoid hashing or logging it.

### 7.3 Map iteration order

`Value::Map` is a `BTreeMap<String, Value>`. Iteration order is **always key-sorted, ascending byte-lexicographic on UTF-8 keys**. This is locked for 1.x:

- Evidence-bundle JSON serialization uses this order.
- `workflow_hash` and bundle hashes depend on this order.
- Any 1.x VM that diverges from key-sorted Map iteration is non-conformant.

(Compare: `.ax` source language §7.4 leaves implementations a choice between insertion order and key-sorted; the bytecode level is stricter — only key-sorted is conformant.)

### 7.4 Actor scheduling

The actor scheduler is deterministic: round-robin over actors sorted by `(target_id, sender_id)`. The scheduler tick, message-send, and message-receive events are all logged in the `EventLog` and replay-verified. See [`docs/concepts/determinism.md`](../concepts/determinism.md).

### 7.5 Capability outcomes are inputs

A capability call's result is an **input** to the deterministic computation, not a deterministic output of it. Replay re-uses the recorded outcome rather than re-invoking the capability. This is what allows recording a run against a live network or LLM and replaying it offline with byte-identical results.

## 8. Replay-verified vs. operational state

Every byte of state the VM tracks falls into one of two classes. Replay verification covers only the first.

### 8.1 Replay-verified state

These are deterministic functions of the module + input + recorded event log. Replay MUST produce the same bytes:

- The module on disk (constants, code, types, functions, globals, entry).
- The operand stack at every instruction boundary, for the entire run.
- The local frames of every active call.
- The values pushed to / popped from the operand stack by each opcode.
- Every `Value` materialized during execution.
- The `EventLog`: `SchedulerTick`, `ActorSpawn`, `MessageSend`, `MessageReceive`, `CapCall` (request + recorded result).
- The serialized output (`Halt`'s top-of-stack).
- The evidence-bundle JSON serialization of the above.
- Map iteration order (always key-sorted; §7.3).
- Float arithmetic results (IEEE 754; §7.2).

### 8.2 Operational state

These exist in a real run but are NOT replay-verified. They MAY differ between record and replay:

- Wall-clock time of execution (the `time.now` capability *result* is replay-verified; the timestamp on the bundle envelope is not).
- VM step-count budgets used by `execute_bounded` (the schedule's logical order is preserved; the per-step CPU budget is not part of the trace).
- Host-process resource usage (RAM, file descriptors, network sockets).
- The `ui.render` sink's actual rendered pixels (`EmitUi` emits the tree; the host's interpretation is operational).
- Logger output, metrics, and Prometheus counters.

Cross-reference: [`docs/concepts/determinism.md`](../concepts/determinism.md) for the rationale and worked examples; [`docs/concepts/capabilities.md`](../concepts/capabilities.md) for the capability-gating model.

## 9. Diagnostics (informative)

The reference implementation surfaces module-level errors via `BytecodeError`:

- `InvalidMagic` — the first four bytes did not match `MAGIC`.
- `UnsupportedVersion(v)` — the header `version` field was not `1`.
- `InvalidBytecode(msg)` — payload truncation, malformed code, or other structural defects.
- `Serialization(msg)` — the JSON payload failed to encode/decode.

VM-level errors (stack underflow, type mismatch, capability denied, division by zero, list out-of-bounds, mod by zero, etc.) are surfaced through the VM's `VmError` type; conformant implementations are not required to use the same names but SHOULD distinguish stack/type/capability/arithmetic categories.

## 10. Reader contract for 1.x VMs

A VM advertising 1.x conformance MUST:

- **Accept any 1.0 module.** Forward-compat for additive changes within 1.x is the contract; if a 1.0 module presents itself, the VM MUST execute it without modification.
- **Reject `bytecode_version >= 2`.** A typed `UnsupportedVersion` (or equivalent) MUST be returned. Silent acceptance of a future major is forbidden — this is the §1 application of "reject at parse, don't silently override".
- **Reject unknown opcodes.** Any byte tag not in §4.2 (or §4.4's reserved space, when the VM is 1.0-only) MUST trigger a typed unknown-opcode error rather than be silently treated as `Nop`.
- **Reject unknown capability IDs.** A `CapCall` with `cap_id` outside the frozen 1.0 set (and not registered for the VM's specific 1.y minor version) MUST fail with a typed capability-unknown error.
- **Preserve the determinism contract** (§7) for all conformant inputs.

A 1.y VM (`y > 0`) accepting an additively-extended module MUST still reject any 2.x module and any opcode/capability not in its known set.

## 11. Cross-references

- Source language: [`docs/spec/ax-language-1.0.md`](./ax-language-1.0.md)
- Workflow DAG: [`docs/spec/workflow-dag-1.0.md`](./workflow-dag-1.0.md)
- Evidence bundle: [`docs/spec/evidence-bundle-1.0.md`](./evidence-bundle-1.0.md)
- Spec index and authoring rules: [`docs/spec/README.md`](./README.md)
- Informal narrative reference: [`docs/bytecode-spec.md`](../bytecode-spec.md)
- Determinism rationale: [`docs/concepts/determinism.md`](../concepts/determinism.md)
- Capability concept and policy: [`docs/concepts/capabilities.md`](../concepts/capabilities.md)
- Capability hash protocol: [`docs/reference/capability-identity.md`](../reference/capability-identity.md)

## 12. Change log for this specification

- **1.0** (2026-04-28) — Initial freeze. Sprint W9-A. Captures the bytecode format as shipped in Boruna v1.0.0-rc2: magic `LLMB`, internal version `1`, JSON-payload module wire format, 48 frozen opcodes, 15 `Value` variants, 11 capabilities at contract version `"1"`, key-sorted `Map` iteration, deterministic actor scheduling.
