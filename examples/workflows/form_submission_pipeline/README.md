# form_submission_pipeline

A 3-step workflow demonstrating form validation, authorization checking, and UI rendering.

## Steps

1. **validate_input** — Validates required fields and minimum lengths using `std-forms` and `std-validation`.
2. **check_authz** — Checks the submitting user's role level against a required permission threshold using `std-authz`.
3. **render_response** — Builds a confirmation UI tree using `std-ui`.

## Stdlib packages referenced

| Package | Functions used |
|---------|----------------|
| `std-forms` | `form_init`, `field_init`, `field_set_value`, `field_touch` |
| `std-validation` | `validate_required`, `validate_min_length`, `validation_merge` |
| `std-authz` | `authz_default_policy`, `authz_role_level`, `authz_check_level` |
| `std-ui` | `text`, `button`, `card`, `column` |

## Import note

Step files currently inline the stdlib surface directly with a comment header:
`// Inline from std.X — import pending full package resolver integration`

Full `import std.forms` syntax is parsed by the compiler but package path resolution
in workflow step context is a planned post-1.0 feature. The structural usage pattern
is identical to what import-based resolution will produce.

## Validate

```bash
cargo run --bin boruna -- workflow validate examples/workflows/form_submission_pipeline
```
