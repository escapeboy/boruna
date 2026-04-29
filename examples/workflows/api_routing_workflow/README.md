# api_routing_workflow

A 3-step workflow demonstrating request routing, state synchronization, and test assertions.

## Steps

1. **route_request** — Matches an incoming API path to a named route using `std-routing`.
2. **sync_state** — Queues, detects conflicts, resolves, and marks a sync operation using `std-sync`.
3. **assert_state** — Runs typed assertions against the resulting state using `std-testing`.

## Stdlib packages referenced

| Package | Functions used |
|---------|----------------|
| `std-routing` | `route_define`, `route_match_path`, `route_navigate`, `route_is_active` |
| `std-sync` | `sync_init`, `sync_queue_edit`, `sync_detect_conflict`, `sync_resolve_conflict`, `sync_mark_synced` |
| `std-testing` | `assert_eq_int`, `assert_true`, `assert_eq_string`, `test_summary`, `test_all_passed_3` |

## Import note

Step files currently inline the stdlib surface directly with a comment header:
`// Inline from std.X — import pending full package resolver integration`

Full `import std.routing` syntax is parsed by the compiler but package path resolution
in workflow step context is a planned post-1.0 feature. The structural usage pattern
is identical to what import-based resolution will produce.

## Validate

```bash
cargo run --bin boruna -- workflow validate examples/workflows/api_routing_workflow
```
