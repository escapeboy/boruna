---
spec_id: ax-language
language_version: "1.0"
status: stable
last_revised: 2026-04-28
audience: language implementers, compiler authors, security auditors
---

# `.ax` Language Specification — Version 1.0

This document is the **formal specification** of the `.ax` source language. It is the authoritative reference for any independent implementation of an `.ax` parser, type checker, or compiler.

The narrative, example-driven companion is [`docs/reference/ax-language.md`](../reference/ax-language.md). When the two disagree, **this spec wins**.

The reference implementation lives in `crates/llmc/` (lexer, parser, typechecker, codegen) of the Boruna repository. Implementations against this spec MAY use a different code organization but MUST be observationally equivalent to the rules below.

## 1. Conformance and versioning

### 1.1 Version identifier

The current language version is **`1.0`**. Implementations MUST expose this value programmatically.

In the reference implementation:

```rust
// crates/llmc/src/lib.rs
pub const LANGUAGE_VERSION: &str = "1.0";
```

The version string is a `<major>.<minor>` decimal number. A program written against `1.x` MUST compile against any `1.y` implementation where `y >= x`.

### 1.2 Backwards-compatibility commitment for 1.x

Within the `1.x` line:

1. **Additive only.** New keywords, types, opcodes, and capabilities MAY be added.
2. **No renames.** The names `Int`, `Float`, `String`, `Bool`, `Unit`, `Option`, `Result`, `List`, `Map`, `Some`, `None`, `Ok`, `Err`, every reserved word in §2.4, and every capability in §6.2 are **frozen for 1.x**.
3. **No removed builtins.** A function or operator that compiles in `1.x` MUST continue to compile in any later `1.y`.
4. **No tightened type rules.** A program that type-checks in `1.x` MUST type-check in `1.y >= 1.x`. Type rules MAY be **relaxed** (more programs accepted) but MAY NOT be tightened.
5. **Capability annotation set is monotonic.** A capability listed on a function signature in `1.x` remains valid in `1.y >= 1.x`. New capabilities added in later 1.y minor versions MUST be optional — programs that do not request them remain valid.
6. **Reserved words for future use.** See §2.4. Implementations MUST tokenize them as reserved and MUST reject them outside of well-formed positions, even though they have no defined semantics in 1.0.

Breaking changes (renames, removals, tightening) are deferred to `2.0` or later.

### 1.3 Conformance levels

A `1.0` implementation MUST:
- Accept every program admitted by the grammar in §3 that satisfies the type rules in §4.
- Reject every program that violates the type rules in §4.
- Enforce capability declarations (§6) at the boundary defined by the host runtime.
- Preserve determinism (§7).

A `1.0` implementation MAY:
- Emit additional diagnostics or hints.
- Provide additional optimization passes, as long as they do not change observable behavior of well-formed programs.

## 2. Lexical structure

The source text is a UTF-8 byte string. Tokenization is greedy left-to-right, longest-match.

### 2.1 Whitespace and line endings

Whitespace characters are: `' '` (U+0020), `'\t'` (U+0009), `'\r'` (U+000D), `'\n'` (U+000A). Whitespace is significant only as a token separator. Statements are terminated by newlines (no semicolons).

### 2.2 Comments

```
LineComment ::= "//" {any character except newline} newline
```

Block comments are NOT defined in 1.0.

### 2.3 Identifiers

```
Identifier ::= IdStart IdContinue*
IdStart    ::= "a".."z" | "A".."Z" | "_"
IdContinue ::= IdStart | "0".."9"
```

Identifiers are case-sensitive. The identifier `_` (single underscore) is a **wildcard** in patterns (§5) but is otherwise a normal identifier.

### 2.4 Keywords and reserved words

**Keywords** (have meaning in 1.0; MUST NOT be used as identifiers):

```
fn   let   if   else   match   record   enum   true   false
Some None Ok  Err
```

**Type names treated as reserved identifiers** (§4.1):

```
Int Float String Bool Unit Option Result List Map
```

**Reserved-for-future-use** (a 1.0 implementation MUST tokenize these as reserved and reject them in identifier position; they have no semantics in 1.0):

```
mut for while loop return break continue trait impl import module
where as in pub priv async await yield static const ref self Self
type spawn actor receive
```

### 2.5 Literals

```
IntLit    ::= ["-"] Digit+
FloatLit  ::= ["-"] Digit+ "." Digit+
BoolLit   ::= "true" | "false"
StringLit ::= "\"" {StringChar} "\""
StringChar::= any UTF-8 codepoint except "\"" and "\\", or one of the escapes:
              "\\\\" | "\\\"" | "\\n" | "\\t" | "\\r"
UnitLit   ::= "(" ")"
Digit     ::= "0".."9"
```

`IntLit` MUST fit in a 64-bit signed integer. `FloatLit` MUST be a finite IEEE 754 double-precision value.

### 2.6 Operators and punctuation

```
+  -  *  /  %      arithmetic
== != <  <= >  >=  comparison
&& || !            logical
=                  binding
->                 return type
=>                 match arm
::                 enum variant
..                 record spread
{ } ( ) [ ]        delimiters
, : .              separators / field access
!{ }               capability annotation opener / closer
"  \\              string delimiter / escape
```

## 3. Syntactic grammar (EBNF)

EBNF conventions: `{X}` = zero or more, `[X]` = optional, `|` = alternation, `"x"` = terminal, `Newline` = a newline outside any expression.

### 3.1 Programs

```
Program ::= {Item}
Item    ::= FnDecl | RecordDecl | EnumDecl
```

Every `.ax` source file is a Program. A standalone executable program MUST contain a top-level item `fn main() -> Int`.

### 3.2 Declarations

```
FnDecl     ::= "fn" Identifier "(" [Params] ")" "->" Type [CapAnnot] Block
Params     ::= Param {"," Param}
Param      ::= Identifier ":" Type
CapAnnot   ::= "!{" CapName {"," CapName} "}"
CapName    ::= Identifier {"." Identifier}

RecordDecl ::= "record" Identifier "{" {FieldDecl ","} "}"
FieldDecl  ::= Identifier ":" Type

EnumDecl   ::= "enum" Identifier "{" {VariantDecl ","} "}"
VariantDecl::= Identifier "{" {FieldDecl ","} "}"
```

Trailing commas are permitted in `Params`, `FieldDecl` lists, and `VariantDecl` lists.

### 3.3 Types

```
Type ::= "Int"
       | "Float"
       | "String"
       | "Bool"
       | "Unit"
       | "Option" "<" Type ">"
       | "Result" "<" Type "," Type ">"
       | "List"   "<" Type ">"
       | "Map"    "<" Type "," Type ">"
       | Identifier                           (* user-declared record or enum *)
```

There are no anonymous tuples, function types as first-class type expressions, or generic user types in 1.0.

### 3.4 Statements and expressions

```
Block      ::= "{" {Stmt Newline} [Expr] "}"
Stmt       ::= LetStmt | ExprStmt
LetStmt    ::= "let" Identifier ":" Type "=" Expr
ExprStmt   ::= Expr

Expr       ::= If | Match | Binary | Unary | Call | RecordLit | EnumLit
             | ListLit | MapLit | FieldAccess | Path | Literal | Block
             | "(" Expr ")"

If         ::= "if" Expr Block "else" Block
Match      ::= "match" Expr "{" {MatchArm} "}"
MatchArm   ::= Pattern "=>" Expr [","]

Binary     ::= Expr BinOp Expr
BinOp      ::= "+"|"-"|"*"|"/"|"%"|"=="|"!="|"<"|"<="|">"|">="|"&&"|"||"
Unary      ::= ("-"|"!") Expr

Call       ::= Path "(" [Args] ")"
Args       ::= Expr {"," Expr}

RecordLit  ::= TypeName "{" [Spread ","] {FieldInit ","} "}"
Spread     ::= ".." Expr
FieldInit  ::= Identifier ":" Expr

EnumLit    ::= TypeName "::" Identifier "{" {FieldInit ","} "}"

ListLit    ::= "[" [Expr {"," Expr}] "]"
MapLit     ::= "{" [MapEntry {"," MapEntry}] "}"
MapEntry   ::= Expr ":" Expr

FieldAccess::= Expr "." Identifier
Path       ::= Identifier {"::" Identifier}
TypeName   ::= Identifier
Literal    ::= IntLit | FloatLit | StringLit | BoolLit | UnitLit
             | "Some" "(" Expr ")" | "None"
             | "Ok"   "(" Expr ")" | "Err" "(" Expr ")"
```

Operator precedence (lowest to highest binding):

```
1. ||
2. &&
3. == != < <= > >=
4. + -
5. * / %
6. unary - !
7. . :: () (postfix call/field/path)
```

All binary operators except comparisons are left-associative. Comparisons are non-associative (chaining like `a < b < c` is rejected).

### 3.5 Patterns

```
Pattern    ::= "_"                                 (* wildcard *)
             | Identifier                           (* binding *)
             | Literal
             | "Some" "(" Pattern ")"
             | "None"
             | "Ok"   "(" Pattern ")"
             | "Err"  "(" Pattern ")"
             | TypeName "::" Identifier "{" {FieldPat ","} "}"
FieldPat   ::= Identifier [":" Pattern]
```

A `FieldPat` of the form `name` is shorthand for `name: name`.

## 4. Type system

Types: `Int`, `Float`, `String`, `Bool`, `Unit`, `Option<T>`, `Result<T, E>`, `List<T>`, `Map<K, V>`, user-declared records, user-declared enums.

There are **no user-definable generic types**, no traits, no subtyping. Every type is concrete at definition time. Type equivalence is structural for built-in generics (`List<Int>` ≡ `List<Int>`) and nominal for records and enums.

### 4.1 Built-in type rules (judgments)

We use `Γ ⊢ e : T` for "in environment Γ, expression `e` has type `T`". `Γ` maps identifiers to types and tracks the in-scope capability set `Φ` (see §6).

**Literals:**
```
─────────────────────                ─────────────────────
Γ ⊢ IntLit n   : Int                Γ ⊢ FloatLit f : Float

─────────────────────                ─────────────────────
Γ ⊢ StringLit s: String             Γ ⊢ BoolLit b  : Bool

─────────────────────
Γ ⊢ ()         : Unit
```

**Variables:**
```
   x : T  ∈  Γ
─────────────────────
Γ ⊢ x : T
```

**Let bindings:**
```
Γ ⊢ e : T          T = AnnotatedType
────────────────────────────────────────
Γ, x : T  ⊢   following statements
```

A `let` annotation MUST exactly match the inferred type of the right-hand side (no coercion, no widening). 1.0 has no implicit conversions between `Int` and `Float`.

**If-else:**
```
Γ ⊢ cond : Bool       Γ ⊢ b1 : T       Γ ⊢ b2 : T
────────────────────────────────────────────────────
Γ ⊢ if cond { b1 } else { b2 } : T
```

Both branches MUST exist and have the same type. There is no single-branch `if` as an expression.

**Arithmetic:**
```
Γ ⊢ a : T   Γ ⊢ b : T    T ∈ {Int, Float}
─────────────────────────────────────────
Γ ⊢ a + b : T            (same for - * / %)
```

`%` is defined for `Int` only.

**Comparison:**
```
Γ ⊢ a : T   Γ ⊢ b : T    T ∈ {Int, Float, String, Bool}
─────────────────────────────────────────────────────────
Γ ⊢ a == b : Bool        (same for !=)

Γ ⊢ a : T   Γ ⊢ b : T    T ∈ {Int, Float, String}
─────────────────────────────────────────────────
Γ ⊢ a <  b : Bool        (same for <= > >=)
```

**Logical:**
```
Γ ⊢ a : Bool   Γ ⊢ b : Bool                 Γ ⊢ a : Bool
────────────────────────────                ──────────────
Γ ⊢ a && b : Bool   (same for ||)           Γ ⊢ !a : Bool
```

**String concatenation:**
The `+` operator on two `String` values yields a `String`. Mixed-type concatenation is rejected.

### 4.2 Compound types

**Option / Result / List / Map literals:**
```
Γ ⊢ e : T                                Γ ⊢ e : T
──────────────────────                   ──────────────────────
Γ ⊢ Some(e) : Option<T>                  Γ ⊢ Ok(e)  : Result<T, _>

                                         Γ ⊢ e : E
─────────────────                        ──────────────────────
Γ ⊢ None : Option<_>                     Γ ⊢ Err(e) : Result<_, E>

Γ ⊢ e1 : T  ...  Γ ⊢ en : T               Γ ⊢ k_i : K   Γ ⊢ v_i : V (for all i)
─────────────────────────────             ─────────────────────────────────
Γ ⊢ [e1, ..., en] : List<T>                Γ ⊢ {k_1: v_1, ...} : Map<K, V>
```

The element type of `None`, `Ok`, `Err`, `[]`, and `{}` is determined by the surrounding context (let annotation or function return type).

**Records:**
```
record R { f1: T1, ..., fn: Tn }   ∈   Γ
Γ ⊢ e1 : T1   ...   Γ ⊢ en : Tn
─────────────────────────────────────────
Γ ⊢ R { f1: e1, ..., fn: en } : R
```

**Record spread:**
```
Γ ⊢ base : R     R = record { ..., fk: Tk, ... }
Γ ⊢ ek_new : Tk   (for each overridden field)
────────────────────────────────────────────────
Γ ⊢ R { ..base, fk: ek_new, ... } : R
```

Spread rules:
1. The base expression MUST have type `R`, the same record type as the literal.
2. Overrides MAY override any subset of fields, including none.
3. A field MUST NOT be specified more than once.
4. Spread MUST appear first inside the braces; non-spread fields override.
5. The result is a fresh record value with the original `base` unchanged (records are immutable, §7.2).

**Enums:**
```
enum E { V { f1: T1, ..., fn: Tn }, ... }   ∈   Γ
Γ ⊢ e1 : T1   ...   Γ ⊢ en : Tn
─────────────────────────────────────────────
Γ ⊢ E::V { f1: e1, ..., fn: en } : E
```

### 4.3 Functions and calls

```
fn f(p1: T1, ..., pn: Tn) -> R [!{φ_f}]   ∈   Γ
Γ ⊢ a1 : T1   ...   Γ ⊢ an : Tn
φ_f ⊆ current capability set Φ
─────────────────────────────────────────────────
Γ ⊢ f(a1, ..., an) : R
```

Function bodies type-check with the parameters in scope. The last expression of the block determines the return type and MUST equal `R`.

### 4.4 Pattern matching

Match expressions select an arm whose pattern matches the scrutinee. All arms MUST produce the same type.

```
Γ ⊢ s : T     for each arm i:  Pi covers some subset of T
                                Γ, bindings(Pi) ⊢ ei : U
arms are exhaustive over T
────────────────────────────────────────────────────────
Γ ⊢ match s { P1 => e1, ..., Pn => en } : U
```

#### 4.4.1 Exhaustiveness

A match is **exhaustive** when every value of the scrutinee type is matched by at least one arm. The compiler MUST reject non-exhaustive matches.

Exhaustiveness rules per scrutinee type:

| Scrutinee type | Exhaustive iff |
|----------------|----------------|
| `Bool` | both `true` and `false` arms, OR a binding/wildcard arm |
| `Option<T>` | both `Some(_)` and `None` arms, OR a binding/wildcard arm |
| `Result<T, E>` | both `Ok(_)` and `Err(_)` arms, OR a binding/wildcard arm |
| enum `E` | every variant of `E`, OR a binding/wildcard arm |
| `Int`, `Float`, `String` | a binding or wildcard arm is REQUIRED |
| record `R` | a binding or wildcard arm is REQUIRED (records are not destructured exhaustively in 1.0) |
| `Unit` | trivially exhaustive (the only inhabitant is `()`) |

A wildcard `_` or a bare-identifier binding always makes a match exhaustive.

#### 4.4.2 Reachability

The compiler SHOULD warn on unreachable arms (an arm whose pattern is fully covered by an earlier arm). 1.0 does not require this warning to be a hard error.

## 5. Pattern binding

Pattern matching introduces bindings into the arm's scope:

- `_` introduces no binding.
- An `Identifier` pattern binds the matched value under that name.
- `Some(P)`, `Ok(P)`, `Err(P)` recursively bind from `P`.
- `E::V { f1: P1, f2, ... }` binds each `Pi`. The shorthand `f2` binds the value of field `f2` to the identifier `f2`.

Bindings are only in scope within the arm's right-hand side.

## 5a. Standard built-in functions

The following functions are provided by the runtime in every compilation unit. They require no import. Their names begin with `__builtin_` to prevent shadowing by user-defined identifiers.

All built-ins are pure (no capability annotation). Their semantics are defined below.

### String operations

| Name | Signature | Semantics |
|------|-----------|-----------|
| `__builtin_int_to_string` | `(Int) -> String` | Returns the decimal string representation of the argument. |
| `__builtin_float_to_string` | `(Float) -> String` | Returns a string representation of the float argument. |
| `__builtin_string_len` | `(String) -> Int` | Returns the number of bytes in the UTF-8 encoding of the string. |
| `__builtin_string_chars` | `(String) -> List<String>` | Returns a list of single-character strings, one per Unicode scalar value. |
| `__builtin_string_contains` | `(String, String) -> Bool` | Returns `true` iff the first argument contains the second as a substring. |
| `__builtin_string_starts_with` | `(String, String) -> Bool` | Returns `true` iff the first argument begins with the prefix given by the second. |
| `__builtin_string_ends_with` | `(String, String) -> Bool` | Returns `true` iff the first argument ends with the suffix given by the second. |
| `__builtin_string_to_upper` | `(String) -> String` | Returns a copy of the string with all ASCII alphabetic characters uppercased. |
| `__builtin_string_to_lower` | `(String) -> String` | Returns a copy of the string with all ASCII alphabetic characters lowercased. |
| `__builtin_string_trim` | `(String) -> String` | Returns a copy of the string with leading and trailing ASCII whitespace removed. |
| `__builtin_string_join` | `(List<String>, String) -> String` | Returns the elements of the list concatenated, with the second argument inserted between each pair of adjacent elements. |

### List operations

| Name | Signature | Semantics |
|------|-----------|-----------|
| `__builtin_list_len` | `(List<T>) -> Int` | Returns the number of elements in the list. |
| `__builtin_list_is_empty` | `(List<T>) -> Bool` | Returns `true` iff the list has zero elements. |
| `__builtin_list_head` | `(List<T>) -> Option<T>` | Returns `Some(first)` if the list is non-empty, otherwise `None`. |
| `__builtin_list_tail` | `(List<T>) -> List<T>` | Returns a new list containing all elements after the first. Returns an empty list if the argument is empty. |
| `__builtin_list_append` | `(List<T>, T) -> List<T>` | Returns a new list equal to the original with the second argument appended at the end. |
| `__builtin_list_concat` | `(List<T>, List<T>) -> List<T>` | Returns a new list that is the concatenation of the two arguments, in order. |
| `__builtin_list_reverse` | `(List<T>) -> List<T>` | Returns a new list containing the same elements in reversed order. |

All list built-ins are non-mutating; the original list is unchanged. This is consistent with the immutability requirement in §7.2.

## 6. Capability semantics

### 6.1 Annotation form

A function declaration MAY include a capability annotation after the return type:

```
fn f(...) -> R !{cap_1, cap_2, ..., cap_n}
```

Each `cap_i` is a dotted name like `net.fetch`. The annotation is the **declared capability set** of the function, written `φ_f`.

A function without an annotation has `φ_f = ∅`.

### 6.2 The capability namespace (1.0 frozen set)

The following capabilities are defined in 1.0. Implementations MUST recognize each and MUST NOT silently rename them:

| Name         | Numeric ID | Description                                      |
|--------------|-----------:|--------------------------------------------------|
| `net.fetch`  | 0          | Network requests                                 |
| `fs.read`    | 1          | File system read                                 |
| `fs.write`   | 2          | File system write                                |
| `db.query`   | 3          | Database queries                                 |
| `ui.render`  | 4          | UI rendering                                     |
| `time.now`   | 5          | Current time                                     |
| `random`     | 6          | Random number generation                         |
| `llm.call`   | 7          | LLM provider call                                |
| `actor.spawn`| 8          | Spawn an actor                                   |
| `actor.send` | 9          | Send an actor message                            |
| `step.input` | 10         | Read a workflow step's resolved input value      |

Implementations MAY add new capabilities at IDs ≥ 11 in future 1.y releases; the IDs and names above are frozen.

### 6.3 Propagation rule

For any call `g(...)` made within the body of `f`:

```
φ_g  ⊆  φ_f
```

The required capability set of every callee MUST be a subset of the caller's declared set. Equivalently: a function may only invoke effects it has declared.

This is checked statically at type-check time. The reference implementation rejects programs that violate this with a `CapabilityError`.

### 6.4 Composition

Capability sets compose as set union along call edges:

```
φ_caller_required = ⋃ { φ_g : g is reachable from caller }
```

For a top-level entry point (e.g. `main`), the union of `φ_g` over all reachable `g` is the **effective capability set** of the program.

### 6.5 Runtime gating

At runtime, the host VM holds an active **policy** Π that is a subset of the capability namespace. When bytecode invokes a capability `c`:

1. The VM checks `c ∈ Π`. If not, the call fails with a capability-denied error.
2. The VM dispatches to the registered handler for `c`.
3. The result (or error) is recorded in the event log for replay (§7.3).

Capability annotations are static facts about the program; the policy Π is a runtime fact about the deployment. The two MUST agree at execution time.

### 6.6 No ambient capabilities

There are no implicit, ambient, or hidden capabilities. Every effect MUST be declared in `φ_f` and MUST be present in Π. Functions without annotations are pure with respect to the capability set; they MUST NOT directly invoke any capability call.

## 7. Determinism

### 7.1 Definition

For any program `P` and any input `I`, executing `P(I)` MUST produce the same observable result on every run, given the same recorded capability outcomes (§7.3).

### 7.2 Immutability

All values are immutable. There is no mutable cell, no `mut` keyword, no in-place update. Record spread (§4.2) constructs a new value.

### 7.3 Replay model

Side effects are recorded by the host VM at the boundary defined in §6.5. A recorded run produces an event log of capability invocations and outcomes. Replaying the program against the same event log MUST produce the same output, byte-for-byte (subject to the implementation's serialization choices for `Map` ordering — see §7.4).

### 7.4 Map iteration order

`Map<K, V>` iteration order is **insertion order** in 1.0. The reference implementation uses a `BTreeMap`, which yields key-sorted iteration. A 1.0-conformant implementation MUST choose one and document it; both choices are observationally indistinguishable for inputs that do not depend on iteration order, and 1.x will not break programs that rely on either choice.

> Implementations SHOULD document which order they provide. The reference implementation is key-sorted.

### 7.5 Forbidden in pure code

Pure (non-capability-annotated) code MUST NOT:
- Read the system clock
- Generate randomness
- Read from / write to the file system or network
- Depend on memory addresses, hash randomization seeds, or thread scheduling

If any of these are needed, they MUST go through a capability call (§6).

## 8. Standalone programs and entry points

### 8.1 Executable programs

A `.ax` source file intended as a standalone program MUST declare:

```
fn main() -> Int { ... }
```

The integer return value is the program's exit code. `0` is success.

### 8.2 Framework apps

A framework app (Boruna's Elm-architecture protocol) MUST declare three top-level functions:

```
fn init() -> State
fn update(state: State, msg: Msg) -> UpdateResult
fn view(state: State) -> UINode
```

with user-declared types `State`, `Msg`, `Effect`, `UpdateResult`, `UINode`, and `PolicySet`. The framework protocol layered on `.ax` is documented separately in [`docs/concepts/`](../concepts/) and is not part of the language spec proper.

## 9. Source examples (informative)

The following are well-formed `1.0` programs.

**Hello, sum:**
```ax
fn add(a: Int, b: Int) -> Int {
    a + b
}

fn main() -> Int {
    add(2, 40)
}
```

**Record spread:**
```ax
record Point {
    x: Int,
    y: Int,
}

fn shift_y(p: Point, dy: Int) -> Point {
    Point { ..p, y: p.y + dy }
}

fn main() -> Int {
    let p: Point = Point { x: 3, y: 4 }
    let q: Point = shift_y(p, 10)
    q.y
}
```

**Capability-annotated function (compile-only — runtime denied without `net.fetch` policy):**
```ax
fn fetch(url: String) -> String !{net.fetch} {
    "stub"
}

fn main() -> Int {
    let body: String = fetch("https://example.com")
    0
}
```

**Match exhaustiveness over an enum:**
```ax
enum Shape {
    Circle    { radius: Float },
    Rectangle { width: Float, height: Float },
}

fn label(s: Shape) -> String {
    match s {
        Shape::Circle    { radius } => "circle"
        Shape::Rectangle { width, height } => "rectangle"
    }
}

fn main() -> Int {
    0
}
```

## 10. Errors and diagnostics (informative)

The reference implementation surfaces errors at three layers — lexer, parser, and type checker — via the `CompileError` enum. Conformant implementations are not required to use the same error categories, but SHOULD distinguish:

- **Lexical errors** (unterminated string, invalid escape, illegal character)
- **Parse errors** (unexpected token, missing delimiter)
- **Type errors** (mismatched types, undefined identifier, non-exhaustive match, capability propagation violation, duplicate field, missing field, invalid spread)

## 11. Bytecode mapping (informative)

`.ax` source compiles to Boruna bytecode (`.axbc`). The bytecode opcode set, capability IDs, and binary format are specified in [`docs/bytecode-spec.md`](../bytecode-spec.md). The capability ID table in §6.2 is **frozen jointly** with the bytecode spec for 1.x.

## 12. Cross-references

- Narrative reference (with worked examples): [`docs/reference/ax-language.md`](../reference/ax-language.md)
- Bytecode binary format and opcodes: [`docs/bytecode-spec.md`](../bytecode-spec.md)
- Capability concept and policy: [`docs/concepts/capabilities.md`](../concepts/capabilities.md)
- Determinism rationale: [`docs/concepts/determinism.md`](../concepts/determinism.md)

## 13. Change log for this specification

- **1.0** (2026-04-28) — Initial freeze. Sprint W1-B. Captures the language as shipped in Boruna v0.5.0.
