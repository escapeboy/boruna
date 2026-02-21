# Structured Diagnostics + Suggested Patches

## Overview

The `tooling` crate provides machine-readable diagnostics and auto-repair for `.ax` source files.

Every diagnostic is emitted in two formats:
- Human-readable text (one line per diagnostic)
- Stable JSON (version 1, serializable as `diagnostics.json`)

## Error Codes

| Code | Category | Description |
|------|----------|-------------|
| E001 | Lexer | Invalid token / unexpected character |
| E002 | Parse | Syntax error |
| E003 | Type | Undefined variable (with name suggestion) |
| E004 | Type | Undefined function |
| E005 | Analysis | Non-exhaustive match (missing enum variants) |
| E006 | Analysis | Unknown record field (with closest-name suggestion) |
| E007 | Analysis | Capability violation (update/view must be pure) |
| E008 | Codegen | Code generation error |
| E009 | Type | General type error |

## Suggested Patches

Each diagnostic may include `suggested_patches`. Each patch contains:
- `id`: Stable identifier for this fix
- `description`: Human-readable summary
- `confidence`: `high`, `medium`, or `low`
- `rationale`: Why this fix is suggested
- `edits`: Array of `TextEdit` (file, start_line, old_text, new_text)

TextEdits are compatible with the PatchBundle Hunk format.

## JSON Format

```json
{
  "version": 1,
  "file": "path/to/file.ax",
  "diagnostics": [
    {
      "id": "E005",
      "severity": "error",
      "message": "non-exhaustive match on 'msg' of type 'Msg': missing variants: Clear, Remove",
      "location": {
        "file": "path/to/file.ax",
        "line": 7,
        "col": null,
        "end_line": null,
        "end_col": null
      },
      "suggested_patches": [
        {
          "id": "E005-add-arms",
          "description": "add missing match arms: Clear, Remove",
          "confidence": "high",
          "rationale": "match expression does not cover variants: Clear, Remove",
          "edits": [
            {
              "file": "path/to/file.ax",
              "start_line": 10,
              "old_text": "    }",
              "new_text": "        Clear => { /* TODO */ }\n        Remove => { /* TODO */ }\n    }"
            }
          ]
        }
      ],
      "related": []
    }
  ]
}
```

## CLI Usage

### Check

```
boruna lang check path/to/file.ax            # human-readable output
boruna lang check path/to/file.ax --json     # JSON to stdout
boruna lang check path/to/file.ax -o diag.json  # JSON to file
```

### Repair

```
boruna lang repair path/to/file.ax                     # auto-fix with best suggestions
boruna lang repair path/to/file.ax --apply all          # apply all suggestions
boruna lang repair path/to/file.ax --apply E005-add-arms  # apply specific fix
boruna lang repair path/to/file.ax --from diag.json     # use pre-computed diagnostics
```

## Analysis Passes

### Match Exhaustiveness (E005)

Detects when a `match` on a typed enum parameter does not cover all variants.
Skips if a wildcard (`_`) or catch-all identifier pattern is present.

### Record Field Validation (E006)

Detects when a record literal uses a field name not present in the type definition.
Suggests the closest valid field name via Levenshtein distance.

### Capability Purity (E007)

In framework apps (those with init/update/view), detects when `update()` or `view()` declare capabilities.
Suggests removing the `!{...}` annotation.

### Undefined Variable (E003)

Enhances compiler errors with name suggestions. Collects all defined names (functions, parameters, local variables, types, builtins) and finds the closest match.

## Repair Tool

The repair tool:
1. Reads diagnostics (from JSON or runs check)
2. Selects patches based on strategy (best/all/specific ID)
3. Applies text edits to the source (reverse line order to avoid offset drift)
4. Re-runs diagnostics to verify the fix
5. Reports before/after diagnostic count and verify status

Patches are applied deterministically: same input always produces the same output.
