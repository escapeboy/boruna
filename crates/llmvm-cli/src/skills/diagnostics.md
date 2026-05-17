# Skill: Diagnostics and Repair

Boruna emits structured diagnostics designed for both humans and agents. The
human reads the message; the agent reads the JSON.

## The check/repair loop

```
boruna lang check app.ax --json     # 1. get structured diagnostics
boruna lang repair app.ax           # 2. apply suggested fixes
boruna lang check app.ax --json     # 3. confirm the fix
```

## Diagnostic JSON shape

`boruna lang check --json` returns a `DiagnosticSet`:

```
{
  "version": 1,
  "file": "app.ax",
  "diagnostics": [
    {
      "id": "E003",
      "severity": "error",
      "message": "unknown identifier 'foo'",
      "location": { "file": "app.ax", "line": 3, "col": 9 },
      "suggested_patches": [
        { "id": "declare-missing-symbol", "description": "...",
          "confidence": "high", "edits": [ ... ] }
      ]
    }
  ]
}
```

Each diagnostic carries a stable `id` (an `E0NN` code), a `severity`, a
`location`, and zero or more `suggested_patches`. Switch on `id` — codes are
stable forever.

## Stable diagnostic codes

Resolve any code without reading compiler source:

```
boruna lang codes --json
```

| Code | Meaning |
|------|---------|
| `E001` | lexer error — source could not be tokenized |
| `E002` | parse error — invalid syntax tree |
| `E003` | undefined variable |
| `E004` | undefined function |
| `E005` | non-exhaustive match |
| `E006` | unknown record field |
| `E007` | capability violation — undeclared effect |
| `E008` | codegen error |
| `E009` | type error |

## Repair strategies

```
boruna lang repair app.ax --apply best   # highest-confidence patch (default)
boruna lang repair app.ax --apply all    # every suggested patch
boruna lang repair app.ax --apply <id>   # one specific patch by id
```

After repair, the tool reports how many patches applied and whether a
verification re-check passed. Always re-run `lang check` to confirm.
