# Canonical `error_kind` taxonomy

This is the single source of truth for the stable `error_kind` strings
emitted by the Boruna binary, MCP server, and coordinator HTTP API.

## Stability contract

- These strings are stable per [`docs/lts.md`](../lts.md) §B.6 ("Error
  taxonomy"). Once shipped in a tag, an `error_kind` is never renamed
  or removed inside the 1.x line.
- New `error_kind` values MAY be added in 1.x minor releases; integrators
  MUST tolerate values they don't recognize.
- Integrators MAY switch on these strings programmatically — the strings
  are part of the LTS-protected contract, not human-readable log copy.
- Numeric HTTP status codes paired with each `coord.*` kind are part of
  the same contract: 1.x will not change the status code attached to a
  given `error_kind`.

## How this list is maintained

Every entry below is verified against a literal-string grep of the
`crates/` and `orchestrator/` trees. When a new `error_kind` is added
to the source, it MUST be added here in the same change. CI may grow a
gate for this in a future sprint; today the discipline is reviewer-
enforced.

---

## `coord.*` — coordinator HTTP API

Emitted by the `boruna coordinator serve` HTTP surface. JSON body shape
is `{"error_kind": "...", "message": "...", "details": {...}?}` (see
`crates/llmvm-cli/src/coordinator.rs::ErrorBody`).

| `error_kind` | HTTP | Phase | Where it fires | Sprint | Caller-facing meaning |
|---|:--:|---|---|---|---|
| `coord.unauthorized` | 401 | N/A | `coordinator.rs::auth_middleware` | `0.5-S3` | Bearer token / mTLS identity missing or invalid. The shared-secret bearer auth and/or mTLS gate rejected the request. |
| `coord.identity_mismatch` | 401 | N/A | `coordinator.rs::handle_register` | `W6-A` | mTLS cert subject CN does not match the body `worker_id`. The cert proves a different worker identity than the one being registered. |
| `coord.invalid_request` | 400 | serialization | `coordinator.rs::handle_claim` | `0.5-S2` | Request body cannot be parsed as the expected shape. |
| `coord.unknown_worker` | 404 | N/A | `coordinator.rs::handle_claim`, `handle_report_*` | `0.5-S2` | The `worker_id` in the request is not registered with this coordinator. |
| `coord.unknown_capability` | 400 | N/A | `coordinator.rs::handle_register` | `W3-A` | A capability name in `advertise_caps` is not in the coord's known capability set. |
| `coord.binary_mismatch` | 409 | N/A | `coordinator.rs::handle_register` | `0.5-S2` | Worker's `boruna_version` does not match the coord's expected version (capability-set hash mismatch). |
| `coord.lease_expired` | 409 | N/A | `coordinator.rs::handle_report_*` | `0.5-S2` | The worker's lease on the step has already expired and another worker has been re-dispatched (per ADR 002). |
| `coord.step_not_found` | 404 | N/A | `coordinator.rs::handle_report_*` | `0.5-S2` | No step matching `(run_id, step_id)` is currently in flight. |
| `coord.output_too_large` | 413 | output_validation | (consumed by `worker.rs`) | `0.5-S7` | Step output exceeds the coord's accepted size. Worker treats this as a non-retryable step failure. |
| `coord.submit.invalid_workflow` | 400 | N/A | `coordinator.rs::handle_submit_run` | `0.5-S4` | Submitted workflow JSON fails validation (cycle, missing field, etc.). |
| `coord.submit.bad_payload` | 400 | serialization | `coordinator.rs::handle_submit_run` | `0.5-S4` | Submit-run request body cannot be parsed. |
| `coord.runs.not_found` | 404 | N/A | `coordinator.rs::handle_get_run`, `handle_approve_run`, `handle_trigger_run` | `0.5-S4` | No run with the given `run_id` exists. |
| `coord.approve.invalid_state` | 409 | N/A | `coordinator.rs::handle_approve_run` | `0.5-S6` | Approval request received for a run not currently waiting on an approval gate. |
| `coord.approve.bad_payload` | 400 | serialization | `coordinator.rs::handle_approve_run` | `0.5-S6` | Approval request body cannot be parsed. |
| `coord.trigger.invalid_state` | 409 | N/A | `coordinator.rs::handle_trigger_run` | `0.5-S6` | External-trigger request received for a run not currently waiting on the named trigger. |
| `coord.trigger.bad_token` | 401 | N/A | `coordinator.rs::handle_trigger_run` | `0.5-S6` | External-trigger token did not match the run's expected token. |
| `coord.trigger.bad_payload` | 400 | serialization | `coordinator.rs::handle_trigger_run` | `0.5-S6` | External-trigger request body cannot be parsed. |
| `coord.blobs.bad_hash` | 400 | N/A | `coordinator.rs::handle_get_blob` | `0.5-S7` | Blob lookup hash is not 64 hex chars. |
| `coord.blobs.not_found` | 404 | N/A | `coordinator.rs::handle_get_blob` | `0.5-S7` | No blob with the given content-hash exists in the coord's blob store. |
| `coord.unavailable` | 503 | N/A | `coordinator.rs::handle_health` | `W2` | Coord HTTP surface is up but a downstream dependency (SQLite store) is unhealthy; emitted on `/api/health`. |
| `coord.capability_version_mismatch` | 409 | N/A | `coordinator.rs::handle_claim` | `post1-T-1.3` | The worker's session covers every required capability NAME for at least one pending step, but at a version that does not match the coord's current `Capability::version()`. Operator action: roll out a worker build that advertises the matching version. Distinct from `coord.unknown_capability` (which fires at REGISTER for an unknown name) and from the silent W3-A skip (worker missing the capability entirely). |

## `evidence.*` — evidence bundle reader

Emitted by `boruna evidence verify` and `boruna evidence inspect`. See
`orchestrator/src/audit/encryption.rs::EncryptionError` for the source
strings.

| `error_kind` | Phase | Where it fires | Sprint | Caller-facing meaning |
|---|---|---|---|---|
| `evidence.encryption_key_required` | N/A | `orchestrator/src/audit/verify.rs::verify_bundle` | `W6-B` | Bundle is encrypted (manifest carries an `encryption` block) but no KEK has been supplied via `--bundle-encryption-key` or `BORUNA_BUNDLE_KEK`. |
| `evidence.encryption_key_mismatch` | N/A | `orchestrator/src/audit/verify.rs::verify_bundle` | `W6-B` | Supplied KEK does not unwrap the bundle's `wrapped_dek` (wrong key, or the DEK ciphertext was tampered). |
| `evidence.cipher_tag_invalid` | N/A | `orchestrator/src/audit/verify.rs::verify_bundle` | `W6-B` | AES-GCM authentication tag failed for at least one encrypted file — the bundle has been tampered after recording. Plaintext bytes are not returned to the caller. |
| `evidence.unsupported_algorithm` | N/A | (reserved) | `W6-B` | `encryption.algorithm` is set to a value other than `"aes-256-gcm"`. Reserved string for forward-compat per [`docs/spec/evidence-bundle-1.0.md`](../spec/evidence-bundle-1.0.md) §8.1. |

> **Note on the W1-C reader gate.** Bundles missing `bundle.json` or
> carrying an incompatible major `format_version` are rejected by
> `verify_bundle` with the diagnostic
> `unsupported evidence bundle format_version: found '<x>', expected major '<y>'`.
> This message is emitted as a `VerifyError`, not as a JSON
> `error_kind` field; tools wrapping the reader translate it to their
> own taxonomy. See `orchestrator/src/audit/verify.rs::VerifyError`.

## `workflow.*` — workflow JSON definition reader

Emitted by `boruna_orchestrator::WorkflowDef::from_json` and surfaces
in `boruna workflow validate` / `boruna workflow run` / the coord
`POST /api/runs` path.

| `error_kind` | Phase | Where it fires | Sprint | Caller-facing meaning |
|---|---|---|---|---|
| `workflow.missing_schema_version` | serialization | `orchestrator/src/workflow/definition.rs::DefinitionError::error_kind` | `W4` | `workflow.json` has no `schema_version` field. Required since v1.0; legacy workflows must be migrated. |
| `workflow.unsupported_schema_version` | serialization | `orchestrator/src/workflow/definition.rs::DefinitionError::error_kind` | `W4` | `workflow.json` carries a `schema_version` value this binary doesn't accept (e.g. `2` on a 1.x binary). |
| `workflow.invalid_json` | serialization | `orchestrator/src/workflow/definition.rs::DefinitionError::error_kind` | `W4` | `workflow.json` is not valid JSON or fails the workflow schema after the version gate. |

## `policy.*` — policy schema validator

Emitted by `boruna policy validate` and `boruna_run` (object-form
policy input). See [`docs/reference/policy-schema.md`](./policy-schema.md)
for full context.

| `error_kind` | Phase | Sprint | Caller-facing meaning |
|---|---|---|---|
| `policy.io_error` | serialization | `0.4-S15` | Policy file missing or unreadable. |
| `policy.parse_error` | serialization | `0.4-S15` | JSON syntax error or value-type mismatch. |
| `policy.unknown_schema_version` | serialization | `0.4-S15` | `schema_version` set to an unsupported value. |
| `policy.unknown_field` | serialization | `0.4-S15` | Unknown field at any level (top-level, `net_policy`, or inside a rule). |
| `policy.invalid_capability` | serialization | `0.4-S15` | Rule key is not a recognized canonical capability name (aliases like `net` are rejected). |
| `policy.invalid_net_policy` | serialization | `0.4-S15` | `net_policy` value out of range or unknown HTTP method. |

## MCP-layer top-level kinds

Emitted by the `boruna-mcp` server's tool layer. These predate the
namespaced `coord.*` / `evidence.*` schemes and are kept for
back-compat per the LTS contract.

| `error_kind` | Tool | Phase | Sprint | Caller-facing meaning |
|---|---|---|---|---|
| `invalid_policy` | `boruna_run` | serialization | `0.2.0` | Non-object policy input (string typo, array, number) was supplied. Object-form input that fails strict validation surfaces as a `policy.*` kind instead. |
| `invalid_output_schema` | `boruna_run` | serialization | `0.4-S16` | The supplied output JSON-schema is malformed or the run's output does not validate against it. |
| `unsupported_limit` | `boruna_run` | serialization | `0.4-S15` | A `limits.*` field is set to a value this binary cannot enforce yet. |
| `parse_error` | `boruna_workflow_validate`, `boruna_compile` | serialization | `0.2.0` | Input JSON / source could not be parsed at the lexer or serde stage. |
| `serialization_error` | `boruna_compile` | serialization | `0.2.0` | AST or compile output could not be serialized for return; internal-encoding failure. |
| `validation_error` | `boruna_workflow_validate` | output_validation | `0.2.0` | Workflow JSON parsed but failed structural validation (cycle, missing field, unknown step reference). |
| `validation_failed` | `boruna_run` | output_validation | `0.4-S16` | Run output failed JSON-schema validation. Response body carries per-path errors. |
| `runtime_error` | `boruna_run` | execution | `0.2.0` | VM error during execution — capability denied, type mismatch, etc. The `error` field carries the message. |
| `limit_exceeded` | `boruna_run` | execution / serialization | `0.4-S15` | A configured limit was hit. `limit_kind` discriminates: `step_limit`, `wall_ms` (execution), `output_bytes` (serialization). |
| `framework_error` | `boruna_validate_app`, `boruna_framework_test` | execution | `0.2.0` | Framework App protocol validation or test-harness error (init/update/view shape mismatch, message dispatch failure). |
| `template_error` | `boruna_template_apply` | execution | `0.2.0` | Template substitution failed (missing variable, unknown template, manifest-validation failure at apply time). |
| `invalid_args` | `boruna_template_apply` | serialization | `0.2.0` | Template `--args` payload could not be parsed as `key=value` pairs. |

## Conventions

- All `error_kind` strings are dotted, lower-snake-case, and
  hierarchical (`<namespace>.<short_kind>`). The namespace identifies
  the surface (coord HTTP API, evidence reader, workflow loader,
  policy validator, MCP top-level).
- HTTP status codes are documented for `coord.*` only — the other
  surfaces are CLI/MCP errors, not HTTP.
- "Phase" follows the project convention of distinguishing `serialization`
  (parse-time / shape rejection) from `output_validation`
  (post-execution shape rejection) from `execution` (runtime failures).
  `N/A` means the kind is a control-flow / policy-gate decision rather
  than a shape error.

## Cross-references

- [`docs/lts.md`](../lts.md) §B.6 — LTS commitment for `error_kind` strings.
- [`docs/reference/policy-schema.md`](./policy-schema.md) — full policy
  validator surface.
- [`docs/reference/cli.md`](./cli.md) — CLI surface; CLI errors are
  printed in `error_kind: <kind>` form on stderr.
- [`docs/spec/evidence-bundle-1.0.md`](../spec/evidence-bundle-1.0.md) §8 —
  evidence bundle encryption reader contract (source of `evidence.*`).
- [`docs/spec/workflow-dag-1.0.md`](../spec/workflow-dag-1.0.md) —
  workflow DAG schema (source of `workflow.*`).
