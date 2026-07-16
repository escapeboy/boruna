# Boruna Front-End Research: Bytecode + Compiler (`crates/llmbc`, `crates/llmc`)

READ-ONLY audit. Every claim cites `path:line` against files actually read. Where a claim depends on VM behavior not in scope, it is tagged `NEEDS-REVIEW` rather than asserted.

Scope: the `.ax` source → bytecode path. `crates/llmbc` (dir name; crate = `boruna-bytecode`) and `crates/llmc` (crate = `boruna-compiler`).

---

## 1. Purpose & Architecture

The front-end turns `.ax` source text into a serializable `Module` of bytecode. The pipeline is a single linear function:

`boruna_compiler::compile(name, source)` (`crates/llmc/src/lib.rs:31-36`):
```
lexer::lex(source)      → Vec<Token>       (crates/llmc/src/lexer.rs:253)
parser::parse(tokens)   → Program (AST)    (crates/llmc/src/parser.rs:120)
typeck::check(&program) → () | error       (crates/llmc/src/typeck.rs:27)
codegen::emit(name,&p)  → Module           (crates/llmc/src/codegen.rs:13)
```

- **`boruna-bytecode`** defines the target IR: `Op` (`opcode.rs`), `Value` (`value.rs`), `Capability` (`capability.rs`), and the container `Module`/`Function`/`TypeDef` with JSON + binary (ser)ialization and capability call-graph analysis (`module.rs`).
- **`boruna-compiler`** is a hand-written recursive-descent front-end. Lexing is `logos`-based (`lexer.rs:20`); parsing is Pratt-style precedence climbing (`parser.rs:566-742`); "type checking" is name-resolution only (`typeck.rs`); codegen is a two-pass stack-machine emitter (`codegen.rs`).

Two independent version identifiers exist and are **not** the same thing:
- `BYTECODE_VERSION = "1.1"` — public spec string (`llmbc/src/lib.rs:32`).
- `module::VERSION = 1` (u16) — the wire byte actually checked on load (`llmbc/src/module.rs:10,129`).
- `LANGUAGE_VERSION = "1.0"` — the `.ax` language surface (`llmc/src/lib.rs:21`).

---

## 2. Component Inventory

| File | Responsibility | Key types / fns | Status |
|---|---|---|---|
| `llmbc/src/lib.rs` | Crate exports; frozen `BYTECODE_VERSION` const | `BYTECODE_VERSION` | Complete |
| `llmbc/src/value.rs` | Runtime value enum + Display/truthiness | `Value` (15 variants), `is_truthy`, `type_name` | Complete |
| `llmbc/src/opcode.rs` | Opcode set + byte-tag table | `Op` (~90 variants), `to_byte_tag` | Complete (but `to_byte_tag` is test-only, see §3) |
| `llmbc/src/module.rs` | Module/Function/TypeDef, JSON+binary ser, cap call-graph analysis | `Module`, `Function`, `MatchArm`, `to_bytes`/`from_bytes`, `to_json`/`from_json`, `transitively_invokes`, `needed_capabilities`, `over_declared_capabilities` | Complete |
| `llmbc/src/capability.rs` | Capability enum + stable identity hashing | `Capability` (11), `from_id`/`id`/`name`/`from_name`, `compute_capability_set_hash`, `capability_set_report` | Complete |
| `llmbc/src/tests.rs` | Unit tests (sampled) | version, roundtrip, cap-hash goldens | Complete (tests) |
| `llmc/src/lib.rs` | `compile()` entry + `LANGUAGE_VERSION` | `compile`, `language_version` | Complete |
| `llmc/src/error.rs` | Compile error enum | `CompileError` (Lexer/Parse/Type/Codegen) | Complete |
| `llmc/src/ast.rs` | AST node definitions | `Program`, `FnDef`, `Expr`, `Stmt`, `Pattern`, `TypeExpr` | Complete (but has variants no producer emits, see §3) |
| `llmc/src/lexer.rs` | Tokenizer + trivia (comments) + line/col tracking | `TokenKind`, `Token`, `lex`, `lex_full` | Complete |
| `llmc/src/parser.rs` | Recursive-descent parser + typo suggestions | `Parser`, `parse_*`, `display_token`, `keyword_suggestion_*` | **Partial** — `for`/Map/Fn types, indirect-call target not handled |
| `llmc/src/typeck.rs` | Name resolution + builtin registry | `TypeChecker`, `check_*`, `ident_suggestion_suffix` | **Partial by design** — no real type checking (see §3) |
| `llmc/src/codegen.rs` | AST → bytecode emitter | `Emitter`, `FnEmitter`, `emit_*`, `pattern_to_tag`, `resolve_field` | **Partial** — `ensures` unemitted, several stubs |
| `llmc/src/suggest.rs` | Levenshtein-1 typo suggestions | `keyword_suggestion`, `suggestion_from` | Complete |
| `llmc/src/tests.rs`, `tests/parser_suggestions.rs`, `tests/debug_builtins.rs` | Tests (sampled) | e2e compile+run, suggestions, debug builtins | Complete (tests) |

---

## 3. GAPS (TODO / dead code / half-done features / doc↔code drift)

### G1 — `ensures` postconditions are parsed but NEVER emitted (dead feature + doc drift). Severity: HIGH
`ensures` is a first-class clause: lexed (`lexer.rs:65`), parsed and stored (`parser.rs:376-378`, `ast.rs:38`), and re-rendered by the formatter (`tooling/src/format/mod.rs:447-449`). But **codegen emits nothing for `f.ensures`** — the only `Op::Assert` emission in the whole compiler is for `requires` (`codegen.rs:120-125`; comment at :118 even says "`Op::Assert` is emitted only here"). Repo-wide grep confirms no `ensures` read in `codegen.rs`.
- **Doc drift, confirmed:** the VM comment at `crates/llmvm/src/vm.rs:578` states `Op::Assert` "is emitted only by codegen for `requires`/`ensures`" — false; `ensures` is never enforced. `error.rs:48` and `runner.rs:3234` similarly describe a "`requires`/`ensures` contract" that only half exists.
- Net effect: a program declaring `ensures result > 0` compiles clean and the postcondition is silently a no-op. No test covers `ensures` codegen (test list in `llmc/src/tests.rs` has `test_requires_emits_assert_opcode` at :204 but no `ensures` analogue).

### G2 — Indirect / higher-order calls are a hardcoded stub. Severity: HIGH
`codegen.rs:528-534`: when the call target is not a directly-named function, codegen pushes args, pushes the callee expr, then emits `Op::Call(0, argc)` — **function index hardcoded to 0**. Any call through a variable, a `FnRef`, or a returned function dispatches to function #0 regardless of the actual target. `Expr::Ident` for a bare function name pushes a `Value::FnRef` (`codegen.rs:231-233`) that can never be correctly invoked. Higher-order functions are effectively non-functional. Comment at :528/:533 admits "not supported yet; fallback".

### G3 — Type checker performs no type checking (soundness holes). Severity: HIGH (by-design MVP, but broad)
`typeck.rs` does name resolution and builds an arity map, but:
- **No arity enforcement.** `functions` stores param counts (`typeck.rs:106`) but no call site is ever checked against them. A user function called with the wrong number of args type-checks and codegens (`codegen.rs:520-525` emits `Op::Call` with whatever `args.len()` is).
- **No type consistency at all** — `Int + String`, wrong return type, wrong argument types all pass. `check_type_def` is an explicit no-op (`typeck.rs:135-138`).
- **Record fields not validated** — undefined record type, missing/extra/mis-typed fields are never checked (no record logic in `check_expr`, `typeck.rs:216-223` only recurses into field values).
- **Match exhaustiveness / enum variants not checked.**
- **`requires`/`ensures` not checked to be `Bool`** — they are name-resolved only (they flow through `check_expr` via the function body? No — note `check_fn` at `typeck.rs:127-133` only checks `f.body`; `f.requires`/`f.ensures` are **not name-resolved at all**, so an undefined variable inside a `requires` clause is not caught by typeck and only surfaces — if at all — at codegen).
This matches the honest self-description at `typeck.rs:25-26` ("For MVP, this does basic validation"). Flagged as a gap because `docs`/CLAUDE.md describe the language as "statically typed."

### G4 — Field access is name-searched across ALL types with a silent 0 fallback. Severity: MEDIUM
`resolve_field` (`codegen.rs:809-829`) searches every record type in the module for the field name and returns the **first** positional match; if not found, returns `0`. Consequences:
- Two record types with the same field name at different positions collide — the first type's index wins for both (field resolution is not type-directed; codegen has no expression-type info).
- A typo'd or nonexistent field silently compiles to `GetField(0)` (reads field 0) rather than erroring — compounded by G3 (no type check catches it).
- `Expr::Record` uses `type_map.get(type_name).copied().unwrap_or(0)` (`codegen.rs:703`) — an unknown record type silently becomes `type_id 0`.

### G5 — `Some`/`Ok`/`Err` codegen uses magic enum type-ids, diverging from the dedicated `Value` variants. Severity: MEDIUM / NEEDS-REVIEW
`Value` has first-class `Some`/`Ok`/`Err` variants (`value.rs:22-26`), and `Expr::NoneLit` emits `Value::None` directly (`codegen.rs:224-227`). But:
- `Expr::SomeExpr` emits `MakeEnum(0xFFFE, 1)` (`codegen.rs:755`) preceded by **dead code**: it pushes a `Value::Bool(true)` "marker" const and immediately `Op::Pop`s it (`codegen.rs:748-750`) — a pointless push/pop leftover.
- `Expr::OkExpr`/`ErrExpr` emit `MakeEnum(0xFFFD, 0|1)` (`codegen.rs:759,763`).
So `Some(x)`/`Ok(x)`/`Err(x)` produce `Value::Enum{type_id:0xFFFE/0xFFFD,...}`, **not** `Value::Some/Ok/Err`. Whether the VM and pattern-matching normalize these magic ids back is a VM concern (NEEDS-REVIEW), but the representation is internally inconsistent and the marker push/pop is unambiguous dead code.

### G6 — Enum-variant match tags all collapse to −1 (variant matching broken). Severity: HIGH / NEEDS-REVIEW
`pattern_to_tag` (`codegen.rs:832-845`) maps `EnumVariant(_,_) => -1`, `StringLit => -1`, `Wildcard => -1`, `Ident => -1` — the same sentinel used for catch-all. In a non-string `Op::Match`, multiple distinct enum-variant arms all get `tag: -1` (comment at :842 says "simplified"), so the VM's match table cannot distinguish them. Matching on user enum variants by value appears non-functional; needs VM-side confirmation of how `-1` tags are dispatched (NEEDS-REVIEW), but the emitted table cannot encode which variant.

### G7 — Lexer/parser recognize `for`/`in` but the parser has no for-loop production. Severity: MEDIUM
`for`/`in` are lexed (`lexer.rs:78-82`), have display + suggestion entries (`parser.rs:39-41,114-115`, `suggest.rs:18`), but `parse_stmt` (`parser.rs:501-564`) handles only `let`/`return`/`while`/expr. A `for` at statement position falls through to `parse_expr` → `parse_primary`, which has no `For` arm → "expected expression, found For". `for` loops parse-fail despite the keyword reservation.

### G8 — `Map<K,V>` and `Fn(...)->T` type expressions are unparseable. Severity: MEDIUM
`TypeExpr::Map` and `TypeExpr::Fn` exist in the AST (`ast.rs:56-57`) and `type_expr_to_string` renders them (`codegen.rs:891-904`), but `parse_type_expr` (`parser.rs:460-485`) only special-cases `Option`/`Result`/`List`; anything else becomes `TypeExpr::Named` (:483). Writing `Map<String,Int>` as an annotation parses `Map` as a bare name, then the `<` is a syntax error. `Value::Map` and the `Map*` opcodes exist, but the type cannot be spelled in source. (CLAUDE.md lists `Map<K,V>` as a language type.)

### G9 — `to_byte_tag` is dead for real serialization; the documented "unknown-opcode rejection" contract is not implemented. Severity: MEDIUM (doc drift)
`opcode.rs:239-323` assigns byte tags (e.g. `Debug`→0xA7), and `lib.rs:28-31` documents that "a 1.0 reader presented with a 1.1 module containing either opcode MUST reject with an unknown-opcode typed error." But the actual binary format (`module.rs:104-139`) is `magic + u16 version + u32 len + serde_json payload` — opcodes serialize as **JSON enum names**, never byte tags. `to_byte_tag` is referenced only from `llmbc/src/tests.rs` (grep confirms). Consequently there is **no** unknown-opcode rejection path; a stale reader would fail deserialization generically (unknown JSON variant), and the byte-tag table / spec claim is aspirational.

### G10 — Bytecode version gating is coarse and string-vs-wire split. Severity: LOW / NEEDS-REVIEW
`from_bytes` rejects only `version != VERSION` where `VERSION == 1` (`module.rs:129-131`); the `"1.1"` `BYTECODE_VERSION` string is never consulted at load. The "1.x accepts 1.y, reject 2.0" contract (`lib.rs:25-26`) is realized only as an exact-`u16`-equality check on a byte that is currently always `1`. No minor-version negotiation exists.

### G11 — `while`-body last expression statement leaves a value on the stack each iteration. Severity: MEDIUM / NEEDS-REVIEW
`emit_block` pops the result of every `Stmt::Expr` **except the last** in the block (`codegen.rs:148-159`). For a `while` body, the loop discards the block value, so if the body's final statement is a bare expression, its value is pushed but never popped and the loop jumps back (`codegen.rs:192-201`) — one leaked stack slot per iteration → unbounded stack growth / imbalance. Bodies ending in `let`/assignment (which leave nothing) are safe, which is why `test_e2e_while_loop` (`llmc/src/tests.rs:408`) passes. VM stack-depth behavior determines whether this traps or corrupts (NEEDS-REVIEW).

### G12 — Capability annotations unknown to `from_name` are silently dropped. Severity: LOW / NEEDS-REVIEW
Codegen resolves declared caps via `Capability::from_name` with no else-branch (`codegen.rs:103-107`) — a mistyped `!{net.ftch}` is silently discarded, producing a function that declares fewer capabilities than the author intended, with no diagnostic. `from_name` (`capability.rs:78-93`) also accepts short aliases (`"net"`, `"db"`, `"llm"`, …) not documented in the `.ax` reference.

---

## 4. SECURITY (in scope: malformed-input robustness, overflow, deser safety)

No `unsafe` blocks exist in either crate (read of all source files). The compile path returns typed `CompileError` on bad input rather than panicking. Findings:

### S1 — Silent integer truncation of counts/indices via `as u8`/`as i32`. Severity: LOW–MEDIUM. Tag: `[CONFIRMED]`
- `codegen.rs:743` `MakeList(items.len() as u8)` — a list literal with >255 elements wraps the count silently (length mismatch vs. values actually pushed → stack/semantic corruption). Same pattern: `MakeRecord(_, fields.len() as u8)` (`codegen.rs:730,736`), `Op::Call(_, args.len() as u8)` (`codegen.rs:524,534`), function `arity as u8` (`codegen.rs:137`).
- `pattern_to_tag`: `Pattern::IntLit(n) => *n as i32` (`codegen.rs:841`) — an integer match arm ≥ 2³¹ truncates the tag, so at runtime the arm matches a *different* integer than written. If a `match` is used for an authorization/branching decision this is a silent correctness/security bug. Tag `[CONFIRMED]` as a bug; security impact is context-dependent (`NEEDS-REVIEW`).
- Const-pool / local indices use `len() as u32` (`module.rs:143`, `codegen.rs` locals). Wrapping requires >4 billion entries (memory-exhausted first) — not practically reachable.

### S2 — `Module::from_bytes` / `from_json` deserialize attacker-controlled bytes via serde_json. Severity: LOW. Tag: `[SAFE]` (with caveat)
`from_bytes` (`module.rs:121-139`) bounds-checks header length (`< 10`), magic, `u16` version, and payload length (`data.len() < 10 + len`) before slicing — no unchecked indexing on short/truncated input (covered by `test_invalid_magic`, `llmbc/src/tests.rs:271`). Deserialization uses `serde_json::from_slice`, which is memory-safe and enforces a default recursion limit (~128) that returns `Err` on deeply-nested JSON rather than overflowing the stack. A crafted module can still assert arbitrary `entry`, `type_id`, `func_idx`, jump targets, and `locals` counts — **the front-end does no validation of loaded modules' internal consistency**; that trust boundary is the VM's to enforce (out of scope, `NEEDS-REVIEW` for the VM audit). No path-traversal or filesystem surface exists in these two crates.

### S3 — Parser `unreachable!()` calls. Severity: NONE. Tag: `[SAFE]`
`parser.rs:750,757,764,988,995,1016` use `unreachable!()` immediately after a `peek()` match guards the same variant then `advance()` returns it — the discriminant cannot change between peek and advance (single-threaded, no mutation). Not reachable from malformed input; they guard internal logic only.

### S4 — `.unwrap()` in `suggest.rs:25`. Severity: NONE. Tag: `[SAFE]`
`keyword_suggestion` unwraps `KEYWORDS.iter().find(|kw| *kw == s)` where `s` was just produced *from* `KEYWORDS` by `suggest_unique_within_1` — the element is guaranteed present. Not input-driven.

### S5 — Lexer line/col tracking is approximate. Severity: NONE (diagnostic quality only). Tag: `[SAFE]`
On encountering `\n` inside `text_before`, `line_start` is set to `span.start` rather than the position just after the newline (`lexer.rs:194-198`), so reported columns can drift on multi-token lines. Diagnostic cosmetics; no memory/safety impact. Over-long integer literals that overflow `i64` cause the logos callback to return `None` → a clean `CompileError::Lexer` (`lexer.rs:85,235-241`), not a panic.

### S6 — Determinism of typeck suggestions over `HashSet`/`HashMap`. Severity: NONE. Tag: `[SAFE]`
`typeck.rs` uses `HashSet`/`HashMap` (against the repo's BTreeMap-for-determinism invariant), but the suggestion logic (`suggest.rs:41-70`) returns `None` whenever ≥2 candidates fall within distance 1, so iteration order cannot change the result; and compile errors surface in source order (`typeck.rs:116-122` iterates the items `Vec`). Output is deterministic despite the hash containers.

---

## 5. COVERAGE

**Read in full (line-by-line):** all 5 non-test source files of `llmbc` — `lib.rs`, `value.rs`, `opcode.rs`, `module.rs`, `capability.rs`; all 8 non-test source files of `llmc` — `lib.rs`, `error.rs`, `ast.rs`, `lexer.rs`, `parser.rs`, `typeck.rs`, `codegen.rs`, `suggest.rs`. That is the complete `.ax`→bytecode surface.

**Sampled (grep + targeted section reads, not full line-by-line):** `llmbc/src/tests.rs` (505 lines) and `llmc/src/tests.rs` (757 lines) plus `llmc/tests/parser_suggestions.rs`, `llmc/tests/debug_builtins.rs` — read test-function inventories and version/roundtrip/requires assertions to corroborate the source findings (notably: `requires` has codegen tests, `ensures` has none). Test bodies were not exhaustively read.

**Not verified (out of scope / deferred to VM audit):** actual VM handling of `MakeEnum(0xFFFE/0xFFFD)` normalization (G5), `-1` match-tag dispatch (G6), stack-depth behavior on the while-body leak (G11), and loaded-module internal-consistency validation (S2). Each is tagged `NEEDS-REVIEW` above rather than asserted.
