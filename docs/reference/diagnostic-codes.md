# Diagnostic Codes

Every diagnostic Boruna's toolchain emits carries a stable `E0NN` code. Codes are
**stable forever** — never reused, never renumbered. Tools and AI agents may switch
on them.

The registry is machine-readable. Query it directly:

```bash
boruna lang codes          # human table
boruna lang codes --json   # { "version": 1, "codes": [ ... ] }
```

Codes appear in `boruna lang check --json` output as the `id` field of each diagnostic.

| Code | Name | Category | Summary |
|------|------|----------|---------|
| `E001` | lexer-error | lexical | The source could not be tokenized (invalid character or token). |
| `E002` | parse-error | syntax | The token stream did not form a valid syntax tree. |
| `E003` | undefined-variable | name-resolution | A referenced variable is not defined in scope. |
| `E004` | undefined-function | name-resolution | A called function is not defined in the module. |
| `E005` | non-exhaustive-match | pattern-matching | A match expression does not cover all possible cases. |
| `E006` | unknown-field | type | A record field access or construction references an unknown field. |
| `E007` | capability-violation | capability | A function performs an effect it does not declare in its capability set. |
| `E008` | codegen-error | codegen | The typechecked program could not be lowered to bytecode. |
| `E009` | type-error | type | An expression's type does not match the type required by its context. |

The table above is generated from the same registry the CLI serves
(`tooling/src/diagnostics/registry.rs`). A drift test asserts the registry stays
1:1 with the `E0NN` constants the compiler emits.
