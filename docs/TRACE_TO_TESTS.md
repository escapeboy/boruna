# Trace → Regression Tests + Minimizer

## Overview

The `trace2tests` module converts runtime execution traces into deterministic regression tests. It also provides a delta-debugging minimizer that shrinks failing traces to minimal reproducing sequences.

## Trace Schema

Version 1, stable JSON format.

```json
{
  "version": 1,
  "source_file": "path/to/app.ax",
  "source_hash": "sha256:<hex>",
  "cycles": [
    {
      "cycle": 1,
      "message": { "tag": "increment", "payload": {"Int": 0} },
      "state_before_hash": "sha256:<hex>",
      "state_after_hash": "sha256:<hex>",
      "state_after": {"Record": {"type_id": 0, "fields": [{"Int": 1}]}},
      "effects": [
        { "kind": "http_request", "payload_hash": "sha256:<hex>", "callback_tag": "on_response" }
      ],
      "ui_tree_hash": "sha256:<hex>"
    }
  ],
  "final_state_hash": "sha256:<hex>",
  "trace_hash": "sha256:<hex>"
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `version` | u32 | Schema version (always 1) |
| `source_file` | string | Path to source `.ax` file |
| `source_hash` | string | SHA-256 of source text |
| `cycles` | array | Ordered cycle records |
| `final_state_hash` | string | SHA-256 of final state |
| `trace_hash` | string | SHA-256 of canonical fingerprint |

### Hashing

All hashes use SHA-256 of canonical JSON serialization:
- Values are serialized via serde (deterministic for BTreeMap)
- The trace fingerprint concatenates all cycle data in stable format
- Same inputs always produce identical hashes

## Test Spec Format

Generated test specifications are self-contained JSON:

```json
{
  "version": 1,
  "name": "counter_regression",
  "source_file": "examples/counter.ax",
  "source_hash": "sha256:<hex>",
  "messages": [
    { "tag": "increment", "payload": {"Int": 0} },
    { "tag": "decrement", "payload": {"Int": 0} }
  ],
  "assertions": [
    { "kind": "final_state_hash", "expected": "sha256:<hex>", "description": "..." },
    { "kind": "trace_hash", "expected": "sha256:<hex>", "description": "..." },
    { "kind": "cycle_count", "expected": "2", "description": "..." }
  ]
}
```

### Assertion Kinds

| Kind | Description |
|------|-------------|
| `final_state_hash` | SHA-256 of final state matches |
| `trace_hash` | SHA-256 of full trace fingerprint matches |
| `cycle_count` | Number of cycles matches |

## Delta Debugging Minimizer

Implements the ddmin algorithm to shrink failing message sequences:

1. **Chunk removal**: Split into n chunks, try removing each
2. **Granularity increase**: If no chunk removal works, try finer splits
3. **1-minimal pass**: Try removing each individual message
4. Result is guaranteed 1-minimal (removing any single message stops the failure)

### Predicates

Built-in predicates:
- `panic`: Failure = runtime error during message processing
- State mismatch: Failure = final state hash differs from expected

External predicates: Any command that receives a temp trace file path and returns non-zero on failure.

## CLI Usage

### Record

```
boruna trace2tests record <file.ax> --messages "tag:payload,..." --out trace.json
```

### Generate

```
boruna trace2tests generate --trace trace.json --out test_spec.json [--name test_name]
```

### Run

```
boruna trace2tests run --spec test_spec.json [--source app.ax]
```

### Minimize

```
boruna trace2tests minimize --trace trace.json --source app.ax [--predicate panic]
boruna trace2tests minimize --trace trace.json --source app.ax --predicate "my_check.sh"
```

## Determinism Guarantees

- Same source + same messages = identical trace hash
- Generated tests are deterministic regression gates
- Minimizer produces deterministic output (same input → same minimal trace)
- All hashes use SHA-256 with canonical serialization

## Integration

- Trace files are compatible with the framework's CycleRecord format
- Test specs can be version-controlled alongside source
- Minimized traces export as regression tests via `generate`
- The full pipeline: record → minimize → generate → run
