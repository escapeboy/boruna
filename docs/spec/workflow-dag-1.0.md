---
schema_version: 1
status: stable
last_revised: 2026-04-28
sprint: W4
---

# Workflow DAG Schema — version 1.0

This document is the formal contract for `workflow.json` files
consumed by `boruna workflow validate`, `boruna workflow run`,
the coordinator/worker HTTP protocol, and any downstream
reader. It freezes the on-disk shape at sprint **W4** of the
0.5.0 spec-freeze track.

## Versioning

The schema follows a **simple-semver** rule:

- The `schema_version` field is a **single integer** representing
  the current major version. Today: `1`.
- Minor revisions ("1.y") add **optional, additive** fields.
  Readers built against any 1.x MUST accept any 1.y document
  (y >= x) and silently ignore fields they do not know.
- Major revisions ("2.x") signal a breaking change. Readers built
  for 1.x MUST refuse a `schema_version: 2` document — they cannot
  guarantee correctness against a format they have not been
  taught.

This implies a forward-compatibility promise within a major and
explicit operator action across a major.

## Reader gate

The reference Rust reader is
[`boruna_orchestrator::workflow::definition::WorkflowDef`][def].
It enforces the gate at parse time, per project conventions §1
(*reject at parse, not later*):

| Condition                       | Result                                     |
|---------------------------------|--------------------------------------------|
| `schema_version` field absent   | `WorkflowParseError::MissingSchemaVersion` |
| `schema_version > 1`            | `WorkflowParseError::UnsupportedSchemaVersion { found, supported_max }` |
| `schema_version` not an integer | `WorkflowParseError::InvalidJson`          |
| `schema_version: 1` + valid body| `Ok(WorkflowDef)`                          |

Stable surface strings (project conventions §2):

- `workflow.missing_schema_version`
- `workflow.unsupported_schema_version`
- `workflow.invalid_json`

The constant
`boruna_orchestrator::WORKFLOW_DAG_SCHEMA_VERSION = 1` is the
authoritative max-supported version for this build.

[def]: ../../orchestrator/src/workflow/definition.rs

## JSON Schema

Authoritative JSON Schema (Draft 2020-12) for a `workflow.json`
document conforming to schema_version 1:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://boruna.dev/spec/workflow-dag/1.0.json",
  "title": "Boruna Workflow DAG (schema_version 1)",
  "type": "object",
  "required": ["schema_version", "name", "version", "steps", "edges"],
  "properties": {
    "schema_version": {
      "description": "Major schema version. MUST be 1 for this spec.",
      "type": "integer",
      "minimum": 1,
      "maximum": 1
    },
    "name": {
      "description": "Human-readable workflow identifier.",
      "type": "string",
      "minLength": 1
    },
    "version": {
      "description": "Operator-controlled workflow version (semver string).",
      "type": "string",
      "minLength": 1
    },
    "description": {
      "description": "Optional free-form description.",
      "type": "string"
    },
    "steps": {
      "description": "Map of step id -> step definition. Order is canonicalized via BTreeMap on read.",
      "type": "object",
      "minProperties": 1,
      "additionalProperties": { "$ref": "#/$defs/Step" }
    },
    "edges": {
      "description": "Explicit DAG edges as [from_step_id, to_step_id] pairs. Edges may also be implied via a step's `depends_on`. Cycles are rejected by the validator.",
      "type": "array",
      "items": {
        "type": "array",
        "minItems": 2,
        "maxItems": 2,
        "items": { "type": "string" }
      }
    }
  },
  "$defs": {
    "Step": {
      "type": "object",
      "required": ["kind"],
      "properties": {
        "kind": {
          "description": "One of: \"source\", \"approval_gate\", \"external_trigger\".",
          "type": "string",
          "enum": ["source", "approval_gate", "external_trigger"]
        },
        "source": {
          "description": "Path (relative to workflow dir) to the .ax file. Required for kind=\"source\".",
          "type": "string"
        },
        "required_role": {
          "description": "Required reviewer role. Required for kind=\"approval_gate\".",
          "type": "string"
        },
        "condition": {
          "description": "Optional gate condition expression (kind=\"approval_gate\"). Informational; not currently enforced by the runner.",
          "type": ["string", "null"]
        },
        "description": {
          "description": "Optional human-readable description (kind=\"external_trigger\").",
          "type": ["string", "null"]
        },
        "capabilities": {
          "description": "Capabilities this step is allowed to invoke. Subset of the workflow policy's allow-list.",
          "type": "array",
          "items": { "type": "string" },
          "default": []
        },
        "inputs": {
          "description": "Map of local input name -> upstream reference of the form \"<step_id>.<output_name>\". Cross-step data flow.",
          "type": "object",
          "additionalProperties": { "type": "string" },
          "default": {}
        },
        "outputs": {
          "description": "Map of output name -> declared type label (informational). The runner stores the actual step value under the key \"result\".",
          "type": "object",
          "additionalProperties": { "type": "string" },
          "default": {}
        },
        "depends_on": {
          "description": "Implicit edges: this step runs only after every listed step has completed. Combined with `edges` to form the full DAG.",
          "type": "array",
          "items": { "type": "string" },
          "default": []
        },
        "timeout_ms": {
          "description": "Wall-clock budget per attempt. Exceeding fails the attempt with `wall_time_exceeded`.",
          "type": ["integer", "null"],
          "minimum": 0
        },
        "retry": { "$ref": "#/$defs/RetryPolicy" },
        "budget": { "$ref": "#/$defs/StepBudget" }
      }
    },
    "RetryPolicy": {
      "type": ["object", "null"],
      "required": ["max_attempts"],
      "properties": {
        "max_attempts": {
          "description": "Upper bound on attempts. <= 1 means single-attempt always.",
          "type": "integer",
          "minimum": 1
        },
        "on_transient": {
          "description": "Legacy gate (used when `retry_on` is empty): true retries on any failure, false is single-attempt.",
          "type": "boolean",
          "default": false
        },
        "retry_on": {
          "description": "Allowlist of error classes that should trigger a retry. Unknown class strings are silently ignored (typo-safe; conservative-by-default).",
          "type": "array",
          "items": { "type": "string" },
          "default": []
        }
      }
    },
    "StepBudget": {
      "type": ["object", "null"],
      "properties": {
        "max_tokens": { "type": ["integer", "null"], "minimum": 0 },
        "max_calls":  { "type": ["integer", "null"], "minimum": 0 }
      }
    }
  }
}
```

Notes:

- The schema does NOT use `"additionalProperties": false`. Forward-
  compatibility (§Versioning above) requires that 1.x readers
  accept additive fields introduced in 1.y minors.
- The schema does NOT require `description`, `capabilities`,
  `inputs`, `outputs`, `depends_on`, `timeout_ms`, `retry`,
  `budget`. The reference reader applies serde defaults.
- The schema does NOT enforce DAG acyclicity, edge endpoint
  existence, or input reference well-formedness — these are
  enforced by the *validator* (`WorkflowValidator::validate`), one
  layer above structural parsing.

## Field-level documentation

### Top-level

- **`schema_version`** (required, integer = 1) — major schema
  version. See §Versioning and §Reader gate above.
- **`name`** (required, non-empty string) — workflow identifier
  surfaced in CLI output, logs, and evidence bundles.
- **`version`** (required, non-empty string) — operator-controlled
  workflow version (typically semver). Independent of
  `schema_version`; this versions the *workflow*, not the schema.
- **`description`** (optional string) — free-form prose.
- **`steps`** (required, non-empty map) — `step_id ->` step body.
  Step ids are unique within a workflow.
- **`edges`** (required array) — `[from_step_id, to_step_id]`
  pairs. Combined with each step's `depends_on` to form the DAG.

### Step body

A step's `kind` discriminates which fields are required:

- **`kind: "source"`** — compile and run an `.ax` file. Requires
  `source` (path relative to the workflow directory). The step's
  return value is stored as the canonical `result` output and is
  available to downstream steps via `<step_id>.result`.
- **`kind: "approval_gate"`** — pause the run until an operator
  records an approval/rejection via `boruna workflow approve`.
  Requires `required_role`. Optional `condition` is informational.
- **`kind: "external_trigger"`** — pause the run until an external
  event arrives via `boruna workflow trigger <run-id> <step-id>`.
  Optional `description` is operator-facing only.

Common optional fields (all kinds):

- `capabilities`, `inputs`, `outputs`, `depends_on`, `timeout_ms`,
  `retry`, `budget` — see the JSON Schema §$defs above for shape
  and defaults. Detailed runner semantics: see
  [`docs/architecture-coordinator-worker-http.md`](../architecture-coordinator-worker-http.md)
  and [`docs/architecture-output-blob-refs.md`](../architecture-output-blob-refs.md).

## Backwards-compatibility commitment

For any reader built against schema_version 1.x:

1. **Within a major (1.x → 1.y, y > x):** the reader MUST accept
   the document. Future minors only ADD fields; they MUST NOT
   change the meaning of existing fields, MUST NOT introduce new
   required fields, and MUST NOT tighten existing field types.
2. **Across a major (1.x → 2.0):** the reader MUST refuse with a
   typed `UnsupportedSchemaVersion` error. The operator must
   upgrade Boruna or downgrade the workflow.

This commitment is exercised in unit tests at
`orchestrator/tests/workflow_integration.rs` (sprint W4):

- `workflow_def_loads_with_schema_version_1`
- `workflow_def_rejects_missing_schema_version`
- `workflow_def_rejects_future_major_version`
- `workflow_def_accepts_unknown_optional_fields` *(forward-compat)*
- `example_workflows_all_validate`

## Replay invariant (§15)

`schema_version` is part of the canonical-JSON serialization that
feeds `WorkflowRunner::workflow_hash_from_def`. Two workflows
that differ only in `schema_version` produce different
`workflow_hash` values, so evidence bundles correctly bind the
schema generation under which a run was executed. Replay
verification fails closed if the on-disk schema_version no longer
matches the recorded hash.

## Post-1.0 additive notes (no version bump)

These behaviors are additive extensions of the 1.0 spec — they are
visible only to operators who opt in, do not change the on-disk
shape of any 1.0 workflow, and do not change the
`schema_version: 1` constant.

- **Versioned worker capability advertisements (post1-T-1.3).**
  `RegisterRequest.advertised_capabilities` (the wire shape used at
  worker registration, not part of the workflow def) now accepts
  `{name, version}` objects in addition to bare strings. The coord
  normalizes legacy strings to the coord's current
  `Capability::version()` for that name. Routing and the new
  `coord.capability_version_mismatch` claim error are documented in
  `docs/reference/error-kinds.md`. No workflow JSON change.

## Cross-references

- [`docs/architecture-coordinator-worker-http.md`](../architecture-coordinator-worker-http.md)
  — the wire protocol that ships `WorkflowDef` between
  coordinator and worker.
- [`docs/architecture-claim-lease-persistence.md`](../architecture-claim-lease-persistence.md)
  — how the persisted run metadata embeds `WorkflowDef` for
  resume.
- [`docs/architecture-output-blob-refs.md`](../architecture-output-blob-refs.md)
  — how step outputs (the `result` slot) reference large blobs.
- [`docs/ORCHESTRATOR_SPEC.md`](../ORCHESTRATOR_SPEC.md)
  — broader orchestrator design rationale.
