# Design: Output JSON Schema Validation Gate

**Sprint:** `0.5-S6` · **Issue:** [#8](https://github.com/escapeboy/boruna/issues/8) · **Status:** Think

## Who

Production integrators (canonical: FleetQ) calling `boruna_run` from a typed host language. They want the runtime to validate that the script's return value matches an expected shape before they consume it — instead of writing the same JSON-Schema validation in PHP/TS/Python on top of every Boruna call.

## What they're doing today

Reading `result` from `boruna_run`, hand-writing a JSON Schema check in their host language for every script type, branching on schema validation failure to surface a clean error to the end user. Three problems:

1. **Duplicated validation logic** — every integrator re-implements the same gate.
2. **No structured error from Boruna itself** — when validation fails, it's "this script returned the wrong shape" but no machine-readable detail at the protocol layer.
3. **No `error_kind` to switch on** — a script that returned-the-wrong-shape and a script that crashed both surface as "the result was not what we wanted" with no ability to bill, retry, or surface differently.

## MVP someone would pay for

```json
boruna_run({
  source: "...",
  policy: ...,
  output_schema: {
    "type": "object",
    "required": ["status", "items"],
    "properties": {
      "status": { "type": "string", "enum": ["ok", "warn"] },
      "items":  { "type": "array", "minItems": 1 }
    }
  }
})
```

If the script's `result` doesn't match:

```json
{
  "success": false,
  "error_kind": "validation_failed",
  "phase": "output_validation",
  "message": "result does not match output_schema",
  "errors": [
    { "path": "/status", "message": "..." },
    { "path": "/items",  "message": "..." }
  ],
  "steps": <vm.step_count() at successful completion>
}
```

If the schema itself is invalid:

```json
{ "success": false, "error_kind": "invalid_output_schema", "message": "..." }
```

## What would make someone say "whoa"

> "Boruna validates the agent's return value against my schema BEFORE I receive it, with structured per-path errors? I can drop my whole validation layer."

That's the win. Pairs naturally with the just-shipped `error_kind` family (`limit_exceeded` per `0.3-S10`, `unsupported_limit`, `invalid_policy`).

## How this compounds

1. **Agent loops self-correct.** An LLM-driven agent that returns a wrong-shaped result gets a precise per-path error message; the next iteration can fix exactly the field that failed without guessing.
2. **Sets the validation pattern for future tools.** `boruna_workflow_run` could carry per-step `output_schema`. `boruna_framework_test` could validate the final `view` shape. The same `validation_failed` envelope reuses cleanly.
3. **Reuses JSON Schema 2020-12** — the same dialect we already publish at `docs/reference/policy.schema.json`. Integrators pick up one mental model.
4. **Composes with `0.3-S10` limits.** `output_schema` rejects "wrong shape"; `max_output_bytes` rejects "too big"; `max_wall_ms` rejects "too slow". Three orthogonal gates, all typed.

## Out of scope for v1

- **Compiler-level `!{out_schema=...}` annotation** (the issue's first proposed form). Larger language design question; the MCP-parameter form gets the win without touching the compiler. Defer.
- **Schema validation on `ui_output`.** v1 validates only `result`. The same machinery extends trivially when there's a real ask.
- **Custom keywords / format extensions.** Use draft 2020-12 stock keywords only; integrators wanting custom validation can post-process.
- **Output schema as a CLI flag.** MCP only for v1. CLI follows in a 0.2.x patch if asked.

## Known limitation: wrapper-format for Record/Enum/Some/Ok

**Read this before writing schemas for v1.**

`format_value` (the function that converts Boruna `Value`s to JSON for the response) preserves the runtime shape with synthetic wrapper objects:

| Boruna value | JSON shape |
|---|---|
| `Int(3)` | `3` |
| `String("hi")` | `"hi"` |
| `Bool(true)` | `true` |
| `List([1,2])` | `[1,2]` |
| `Map({"k":"v"})` | `{"k":"v"}` |
| `Some(x)` | `{"option":"Some","value":<x>}` |
| `Ok(x)` | `{"result":"Ok","value":<x>}` |
| `Record { type_id: N, fields: [...] }` | `{"type":"record","type_id":N,"fields":[<positional values>]}` |
| `Enum { type_id, variant, payload }` | `{"type":"enum","type_id":N,"variant":<n>,"payload":...}` |

A schema that expects the natural shape (e.g. `{"type":"object","properties":{"name":{"type":"string"}}}`) for a record return will **always** get `validation_failed`, because the actual JSON has the wrapper shape and positional fields (no field names).

**Practical recommendation for v1:**
- Use `output_schema` to validate primitive return types (`Int`, `String`, `Bool`) and homogeneous `List` / `Map` containers — schemas for these match the on-the-wire JSON exactly.
- For record/enum returns, either: (a) project to a primitive at the script boundary, (b) write a schema against the wrapper shape (`{"type":"object","required":["type","fields"],...}` — ugly but valid), or (c) wait for a future sprint that adds logical projection (by-name field rendering).

The wrapper-format choice predates this sprint (lives in `tools/run.rs::format_value`); changing it requires coordinating with every consumer of the `result` shape and is out of scope here.

## Hard limits on the schema parameter

- **256 KB max schema size** (compact JSON bytes). Larger schemas return `error_kind: "invalid_output_schema"` before any compilation happens. Mirrors the spirit of the 1 MB source-size cap. A schema with a 40 000-element `enum` will exceed the cap.
- **100 errors max in the response.** A pathological schema (e.g. wide `oneOf` against `additionalProperties: false`) can produce thousands of errors. We cap at 100 and emit `truncated: true` plus `total_errors: N` so integrators know there were more without paying transport for the full list.
- **JSON Schema Draft 2020-12 enforced.** Schemas with no `$schema` URI default to 2020-12. Schemas that explicitly declare a non-2020-12 `$schema` (`"http://json-schema.org/draft-04/schema#"` etc.) are **rejected** with `error_kind: "invalid_output_schema"` rather than silently honoured at the older-draft semantics. (`jsonschema` honours `$schema` over `with_draft`, so the only way to enforce 2020-12 is to reject the older declaration up front. Same "reject at parse, don't silently override" pattern as `0.3-S10`'s `unsupported_limit` for `max_memory_mb`.) Schemas with `"$schema": "https://json-schema.org/draft/2020-12/schema"` are accepted.

## Determinism contract (per ADR 001)

`output_schema` is a **post-execution gate** — the VM run itself is unaffected. The validation step compares already-produced output against a schema. Both sides are deterministic given the same inputs:
- A passing script + valid schema → `success: true` always
- A failing script + same schema → `validation_failed` with the same per-path errors always
- A failing script + invalid schema → `invalid_output_schema` always

Replay-safe by construction. Can be fed into audit hashes if a future sprint wants to (the orchestrator's Runner doesn't today; out of scope).

## Library choice: `jsonschema`

Per the issue's implementer note, the Rust ecosystem standard is the `jsonschema` crate. Supports draft 2020-12 (matches `policy.schema.json`). Pure-Rust (no C deps), works under musl, output-mode supports structured per-path errors.

Add as a dependency to `boruna-mcp` only — the orchestrator doesn't need it, the VM doesn't need it. Single binary impact: `boruna-mcp` gets ~200KB of validation code; `boruna` CLI is unchanged.

## Acceptance criteria

1. `boruna_run` accepts an optional `output_schema: object` parameter (any valid JSON Schema 2020-12 object).
2. After `vm.run()` succeeds and the result is serialized, the result JSON is validated against the schema.
3. **Schema not provided** (`None` or omitted) → behavior unchanged (passthrough).
4. **Schema provided + result valid** → `success: true` with the normal response shape.
5. **Schema provided + result invalid** → `success: false, error_kind: "validation_failed", phase: "output_validation"` with `errors: [{path, message}]` array of per-path failures.
6. **Schema itself malformed** (compile-time error from `jsonschema`) → `success: false, error_kind: "invalid_output_schema", message: "..."`.
7. **Schema validation failures DO NOT replace runtime errors.** A script that crashes returns `runtime_error`, not `validation_failed`. A script that times out returns `limit_exceeded`. The schema gate runs only on successfully-completed runs.
8. The `errors` array path uses **JSON Pointer** notation (`/status`, `/items/0/name`) — the standard for JSON Schema diagnostics, what every integrator's UI already expects.
9. Test coverage: schema-not-set, schema-set-valid, schema-set-invalid (one error), schema-set-invalid (multiple errors), schema-itself-invalid, schema-set-but-runtime-error.
10. Adding `output_schema` is **additive** to `boruna_run` — keeps existing signatures stable, doesn't change `success: true` shape.
