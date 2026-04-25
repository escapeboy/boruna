use boruna_bytecode::Value;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::vm::Vm;
use serde_json::Value as JsonValue;

const TRACE_LIMIT: usize = 500;

/// Cap on the per-call output_schema size (bytes of compact JSON).
/// Schemas larger than this are rejected with `invalid_output_schema` before
/// any compilation or execution work, mirroring the spirit of the 1 MB source
/// limit on the `source` parameter. Documented in `docs/design-output-schema.md`.
const MAX_OUTPUT_SCHEMA_SIZE: usize = 256 * 1024;

/// Cap on the per-call number of validation errors reported back. Matches the
/// shape of `TRACE_LIMIT` — pathological schemas (e.g. a wide `oneOf` against
/// an `additionalProperties: false` object) can produce thousands of errors,
/// each with a verbose message. We cap so a single bad schema can't blow the
/// MCP transport. When truncated, the response carries `truncated: true` and
/// `total_errors: N` so the integrator knows there were more.
const MAX_VALIDATION_ERRORS: usize = 100;

/// Compile and execute source, returning JSON with result or errors.
///
/// `policy` accepts:
///   - `None` → `Policy::allow_all()` (legacy default)
///   - `Some(JsonValue::String("allow-all"|"deny-all"))` → corresponding shorthand
///   - `Some(JsonValue::Object(_))` → deserialize into [`Policy`]
///   - Anything else → returns `{"success": false, "error_kind": "invalid_policy", ...}`
///
/// `output_schema` accepts an optional JSON Schema 2020-12 object. When set,
/// the script's `result` is validated against the schema post-execution. See
/// [`validate_output_against_schema`] and `docs/design-output-schema.md`.
pub fn run_source(
    source: &str,
    policy: Option<&JsonValue>,
    max_steps: u64,
    trace: bool,
    output_schema: Option<&JsonValue>,
) -> String {
    // Compile
    let module = match boruna_compiler::compile("module", source) {
        Ok(m) => m,
        Err(e) => {
            return crate::tools::compile::compile_error_json(&e);
        }
    };

    // Resolve policy
    let gw_policy = match parse_policy(policy) {
        Ok(p) => p,
        Err(msg) => {
            return serde_json::json!({
                "success": false,
                "error_kind": "invalid_policy",
                "message": msg,
            })
            .to_string();
        }
    };
    let gateway = CapabilityGateway::new(gw_policy);

    // Run
    let mut vm = Vm::new(module, gateway);
    vm.set_max_steps(max_steps);
    vm.trace_enabled = trace;

    match vm.run() {
        Ok(value) => {
            let result_json = format_value(&value);

            // Output-schema gate (post-execution; only runs on successful runs).
            // A schema-validation failure or a malformed schema is reported
            // BEFORE we serialize the success response — short-circuits with
            // its own typed envelope. See docs/design-output-schema.md.
            if let Some(schema) = output_schema {
                if let Some(failure) =
                    validate_output_against_schema(schema, &result_json, vm.step_count())
                {
                    return failure;
                }
            }

            let mut json = serde_json::json!({
                "success": true,
                "result": result_json,
                "steps": vm.step_count(),
                "ui_output": vm.ui_output.iter().map(format_value).collect::<Vec<_>>(),
            });

            if trace {
                let trace_entries: Vec<&str> = if vm.trace.len() > TRACE_LIMIT {
                    vm.trace[..TRACE_LIMIT].iter().map(|s| s.as_str()).collect()
                } else {
                    vm.trace.iter().map(|s| s.as_str()).collect()
                };
                json["trace"] = serde_json::json!(trace_entries);
                json["trace_truncated"] = serde_json::json!(vm.trace.len() > TRACE_LIMIT);
            }

            serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".into())
        }
        Err(e) => serde_json::json!({
            "success": false,
            "error_kind": "runtime_error",
            "message": format!("{e}"),
            "steps": vm.step_count(),
        })
        .to_string(),
    }
}

/// Validate the script's serialized `result` against an integrator-supplied
/// JSON Schema. Returns `Some(json_response)` if the validation either fails
/// or the schema itself is malformed/oversized; `None` if the result passes
/// (in which case the caller continues to the normal success response).
///
/// Enforces **JSON Schema Draft 2020-12** semantics. Schemas that omit
/// `$schema` default to 2020-12; schemas that explicitly declare a non-2020-12
/// `$schema` (e.g. `"http://json-schema.org/draft-04/schema#"`) are **rejected**
/// rather than silently honoured at the older-draft semantics. The jsonschema
/// crate honors `$schema` over `with_draft` (which is only a fallback), so the
/// only way to enforce 2020-12 is to reject the older declaration up front.
///
/// **Wrapper-format limitation (read this before writing schemas):** Boruna
/// values are JSON-serialized via `format_value`, which preserves runtime
/// shape — records become `{"type":"record","type_id":<n>,"fields":[<positional values>]}`,
/// enums become `{"type":"enum","type_id":<n>,"variant":<n>,"payload":...}`,
/// and `Some`/`Ok`/`Err` become `{"option":"Some","value":...}` etc. A schema
/// that expects the natural object shape (e.g. `{"type":"object","properties":{"name":...}}`)
/// will fail because the actual JSON has the wrapper shape. Today the gate is
/// most useful for primitive return types (`Int`, `String`, `Bool`, `List`,
/// `Map`); record/enum projection lands in a future sprint.
///
/// Wire shape on **schema mismatch**:
///
/// ```jsonc
/// {
///   "success":      false,
///   "error_kind":   "validation_failed",
///   "phase":        "output_validation",
///   "message":      "result does not match output_schema",
///   "errors":       [{ "path": "/status", "message": "..." }, ...],
///   "truncated":    false,                  // true if total_errors > 100
///   "total_errors": 3,                      // count of all errors
///   "steps":        <vm.step_count() at successful completion>
/// }
/// ```
///
/// Wire shape on **malformed or oversized schema** (separate kind so
/// integrators can distinguish "my schema is wrong" from "the script's
/// output is wrong"):
///
/// ```jsonc
/// { "success": false, "error_kind": "invalid_output_schema", "message": "..." }
/// ```
///
/// Path notation is **JSON Pointer** (`/status`, `/items/0/name`).
fn validate_output_against_schema(
    schema: &JsonValue,
    result: &JsonValue,
    steps: u64,
) -> Option<String> {
    // Cheap pre-check: bound the schema size before we hand it to jsonschema's
    // compiler, which can OOM on a large/recursive schema.
    let schema_size = serde_json::to_vec(schema).map(|v| v.len()).unwrap_or(0);
    if schema_size > MAX_OUTPUT_SCHEMA_SIZE {
        return Some(
            serde_json::json!({
                "success": false,
                "error_kind": "invalid_output_schema",
                "message": format!(
                    "output_schema exceeds {} bytes (got {})",
                    MAX_OUTPUT_SCHEMA_SIZE, schema_size
                ),
            })
            .to_string(),
        );
    }

    // Reject schemas that explicitly declare a non-2020-12 draft via `$schema`.
    // The jsonschema crate honors `$schema` over `with_draft` (which is only a
    // fallback), so a draft-04 schema would silently get draft-04 semantics —
    // an integrator-visible surprise that violates our documented 2020-12
    // contract. Rather than silently override, reject loudly. Same pattern as
    // `0.3-S10`'s `unsupported_limit` for `max_memory_mb`.
    if let Some(JsonValue::String(s)) = schema.get("$schema") {
        // Allow only 2020-12 URIs (the IANA-registered URL or any reasonable
        // form thereof). Anything else is rejected.
        let normalized = s.trim_end_matches('#');
        if !normalized.contains("2020-12") && !normalized.contains("draft/2020-12") {
            return Some(
                serde_json::json!({
                    "success": false,
                    "error_kind": "invalid_output_schema",
                    "message": format!(
                        "output_schema declares $schema='{s}'; only JSON Schema Draft 2020-12 \
                         is supported. Either omit $schema (we default to 2020-12) or set it \
                         to 'https://json-schema.org/draft/2020-12/schema'."
                    ),
                })
                .to_string(),
            );
        }
    }

    // Compile the schema. with_draft is the FALLBACK draft when $schema is
    // absent (jsonschema honors $schema if present). The check above already
    // rejected non-2020-12 $schema declarations, so this fallback is the
    // only path for schemas that omit the URI.
    let validator = match jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(schema)
    {
        Ok(v) => v,
        Err(e) => {
            return Some(
                serde_json::json!({
                    "success": false,
                    "error_kind": "invalid_output_schema",
                    "message": format!("output_schema is not a valid JSON Schema: {e}"),
                })
                .to_string(),
            );
        }
    };

    // Collect per-path errors with a hard cap. `iter_errors` yields
    // ValidationError structs whose `instance_path` is JSON Pointer.
    // We count via a separate iter pass to give integrators an honest
    // total even when truncated — the cost is one extra pass at
    // pathological-schema-time, which is the rare case anyway.
    let errors: Vec<serde_json::Value> = validator
        .iter_errors(result)
        .take(MAX_VALIDATION_ERRORS)
        .map(|err| {
            serde_json::json!({
                "path": err.instance_path.to_string(),
                "message": err.to_string(),
            })
        })
        .collect();

    if errors.is_empty() {
        return None;
    }

    let total_errors = validator.iter_errors(result).count();
    let truncated = total_errors > MAX_VALIDATION_ERRORS;

    Some(
        serde_json::json!({
            "success": false,
            "error_kind": "validation_failed",
            "phase": "output_validation",
            "message": "result does not match output_schema",
            "errors": errors,
            "truncated": truncated,
            "total_errors": total_errors,
            "steps": steps,
        })
        .to_string(),
    )
}

/// Parse the MCP `policy` argument into a [`Policy`].
///
/// See `docs/reference/policy-schema.md` for the object form.
pub(crate) fn parse_policy(value: Option<&JsonValue>) -> Result<Policy, String> {
    match value {
        None => Ok(Policy::allow_all()),
        Some(JsonValue::String(s)) => match s.as_str() {
            "allow-all" => Ok(Policy::allow_all()),
            "deny-all" => Ok(Policy::deny_all()),
            other => Err(format!(
                "policy string must be 'allow-all' or 'deny-all' (got '{other}'); \
                 pass an object for fine-grained policy — see docs/reference/policy-schema.md"
            )),
        },
        Some(obj @ JsonValue::Object(_)) => serde_json::from_value::<Policy>(obj.clone())
            .map_err(|e| format!("policy object failed to parse: {e}")),
        Some(other) => {
            let kind = match other {
                JsonValue::Null => "null",
                JsonValue::Bool(_) => "boolean",
                JsonValue::Number(_) => "number",
                JsonValue::Array(_) => "array",
                _ => "unknown",
            };
            Err(format!(
                "policy must be a string ('allow-all'/'deny-all') or an object; got {kind}"
            ))
        }
    }
}

fn format_value(value: &Value) -> serde_json::Value {
    match value {
        Value::Int(n) => serde_json::json!(n),
        Value::Float(f) => serde_json::json!(f),
        Value::String(s) => serde_json::json!(s),
        Value::Bool(b) => serde_json::json!(b),
        Value::Unit => serde_json::json!(null),
        Value::None => serde_json::json!({"option": "None"}),
        Value::Some(v) => serde_json::json!({"option": "Some", "value": format_value(v)}),
        Value::Ok(v) => serde_json::json!({"result": "Ok", "value": format_value(v)}),
        Value::Err(v) => serde_json::json!({"result": "Err", "value": format_value(v)}),
        Value::List(items) => {
            serde_json::json!(items.iter().map(format_value).collect::<Vec<_>>())
        }
        Value::Record { type_id, fields } => {
            serde_json::json!({
                "type": "record",
                "type_id": type_id,
                "fields": fields.iter().map(format_value).collect::<Vec<serde_json::Value>>(),
            })
        }
        Value::Enum {
            type_id,
            variant,
            payload,
        } => {
            serde_json::json!({
                "type": "enum",
                "type_id": type_id,
                "variant": variant,
                "payload": format_value(payload),
            })
        }
        Value::Map(entries) => {
            let obj: serde_json::Map<String, serde_json::Value> = entries
                .iter()
                .map(|(k, v)| (k.clone(), format_value(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        Value::ActorId(id) => serde_json::json!({"actor_id": id}),
        Value::FnRef(idx) => serde_json::json!({"fn_ref": idx}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PURE_SOURCE: &str = "fn main() -> Int {\n    1 + 2\n}\n";

    // ── parse_policy ──

    #[test]
    fn parse_policy_none_is_allow_all() {
        let p = parse_policy(None).unwrap();
        assert!(p.default_allow);
        assert!(p.rules.is_empty());
    }

    #[test]
    fn parse_policy_string_allow_all() {
        let v = json!("allow-all");
        let p = parse_policy(Some(&v)).unwrap();
        assert!(p.default_allow);
    }

    #[test]
    fn parse_policy_string_deny_all() {
        let v = json!("deny-all");
        let p = parse_policy(Some(&v)).unwrap();
        assert!(!p.default_allow);
    }

    #[test]
    fn parse_policy_unknown_string_errors() {
        let v = json!("garbage");
        let err = parse_policy(Some(&v)).unwrap_err();
        assert!(err.contains("must be 'allow-all' or 'deny-all'"));
        assert!(err.contains("policy-schema.md"));
    }

    #[test]
    fn parse_policy_array_errors() {
        let v = json!([]);
        let err = parse_policy(Some(&v)).unwrap_err();
        assert!(err.contains("array"));
    }

    #[test]
    fn parse_policy_malformed_object_errors() {
        let v = json!({ "default_allow": "yes" });
        let err = parse_policy(Some(&v)).unwrap_err();
        assert!(err.contains("policy object failed to parse"));
    }

    #[test]
    fn parse_policy_structured_object() {
        let v = json!({
            "default_allow": true,
            "rules": {
                "net.fetch": { "allow": false, "budget": 0 }
            }
        });
        let p = parse_policy(Some(&v)).unwrap();
        assert!(p.default_allow);
        let rule = p.rules.get("net.fetch").expect("net.fetch rule populated");
        assert!(!rule.allow);
    }

    #[test]
    fn parse_policy_round_trip() {
        let mut original = Policy::deny_all();
        use boruna_bytecode::Capability;
        original.allow(&Capability::TimeNow, 5);
        let serialized = serde_json::to_value(&original).unwrap();
        let parsed = parse_policy(Some(&serialized)).unwrap();
        assert_eq!(parsed.default_allow, original.default_allow);
        assert_eq!(parsed.rules.len(), original.rules.len());
        let parsed_rule = parsed.rules.get("time.now").expect("rule preserved");
        let orig_rule = original.rules.get("time.now").unwrap();
        assert_eq!(parsed_rule.allow, orig_rule.allow);
        assert_eq!(parsed_rule.budget, orig_rule.budget);
    }

    // ── run_source ──

    #[test]
    fn run_source_default_policy_pure_program() {
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "output: {out}");
    }

    #[test]
    fn run_source_with_structured_policy() {
        let policy = json!({
            "default_allow": true,
            "rules": { "fs.write": { "allow": false, "budget": 0 } }
        });
        let out = run_source(PURE_SOURCE, Some(&policy), 1_000_000, false, None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "output: {out}");
    }

    #[test]
    fn run_source_invalid_policy_returns_error_kind() {
        let bad = json!(42);
        let out = run_source(PURE_SOURCE, Some(&bad), 1_000_000, false, None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_kind"], "invalid_policy");
    }

    // ── 0.5-S6: output_schema gate ──

    #[test]
    fn run_source_output_schema_none_is_passthrough() {
        // Sanity: omitting the schema must not change behavior at all.
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["result"], 3);
    }

    #[test]
    fn run_source_output_schema_passing_returns_success() {
        // PURE_SOURCE returns Int(3). Schema accepts integer.
        let schema = json!({ "type": "integer" });
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "output: {out}");
        assert_eq!(v["result"], 3);
    }

    #[test]
    fn run_source_output_schema_failing_returns_validation_failed() {
        // PURE_SOURCE returns Int(3). Schema demands string.
        let schema = json!({ "type": "string" });
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_kind"], "validation_failed");
        assert_eq!(v["phase"], "output_validation");
        assert!(
            v["errors"].is_array() && !v["errors"].as_array().unwrap().is_empty(),
            "errors must be a non-empty array: {out}"
        );
        // The error path for a top-level type mismatch is the empty pointer.
        assert_eq!(v["errors"][0]["path"], "");
    }

    #[test]
    fn run_source_output_schema_failing_carries_per_path_errors() {
        // Use a script that returns a nested record so we can verify the
        // JSON Pointer path notation. PURE_SOURCE returns Int(3); we need
        // something with structure. Easiest: an enum/integer constraint.
        let schema = json!({
            "type": "integer",
            "minimum": 100,
            "maximum": 200
        });
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_kind"], "validation_failed");
        // Int(3) fails the minimum constraint.
        let errors = v["errors"].as_array().unwrap();
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("minimum")),
            "expected a minimum-constraint error, got: {errors:?}"
        );
    }

    #[test]
    fn run_source_invalid_schema_returns_invalid_output_schema() {
        // A schema that's syntactically a JSON object but not a valid
        // JSON Schema (e.g. `type` set to a number, which is invalid).
        let schema = json!({ "type": 42 });
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["success"], false,
            "malformed schema must NOT be silently accepted: {out}"
        );
        assert_eq!(v["error_kind"], "invalid_output_schema");
        assert!(v["message"].as_str().unwrap().contains("not a valid"));
    }

    #[test]
    fn run_source_runtime_error_takes_precedence_over_schema() {
        // Force a runtime error (max_steps=1 against a non-trivial program).
        // The output_schema gate must NOT replace the runtime_error kind —
        // schema validation is post-execution and shouldn't run if the run
        // didn't complete.
        let schema = json!({ "type": "string" });
        let out = run_source(PURE_SOURCE, None, 1, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(
            v["error_kind"], "runtime_error",
            "runtime errors must NOT be masked by validation_failed: {out}"
        );
    }

    #[test]
    fn run_source_output_schema_empty_object_accepts_anything() {
        // The empty schema {} is the trivially-true schema in JSON Schema —
        // accepts every value. Locks the "schema gate is opt-in by content,
        // not by mere presence" semantics.
        let schema = json!({});
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "empty schema must accept: {out}");
    }

    #[test]
    fn run_source_output_schema_rejects_non_2020_12_dollar_schema() {
        // Security: jsonschema honors `$schema` over `with_draft`, so a schema
        // declaring an older draft would silently get older semantics —
        // integrator-visible surprise. We reject non-2020-12 `$schema`
        // declarations rather than silently honouring them. Matches the
        // 0.3-S10 pattern of "reject at parse, don't silently override".
        let schema = json!({
            "$schema": "http://json-schema.org/draft-04/schema#",
            "type": "integer"
        });
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["success"], false,
            "non-2020-12 $schema must be rejected, not silently honoured: {out}"
        );
        assert_eq!(v["error_kind"], "invalid_output_schema");
        assert!(
            v["message"].as_str().unwrap().contains("2020-12"),
            "rejection message must mention 2020-12: {out}"
        );
    }

    #[test]
    fn run_source_output_schema_accepts_explicit_2020_12_dollar_schema() {
        // The complement: explicitly declaring 2020-12 must work.
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "integer"
        });
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["success"], true,
            "explicit 2020-12 $schema must be accepted: {out}"
        );
    }

    #[test]
    fn run_source_output_schema_size_limit_rejects_huge_schema() {
        // Construct a schema larger than MAX_OUTPUT_SCHEMA_SIZE (256 KB).
        // Easiest way: a giant `enum` array of strings.
        let huge_enum: Vec<String> = (0..40_000).map(|n| format!("v{n:06}")).collect();
        let schema = json!({ "type": "string", "enum": huge_enum });
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false, "huge schema must be rejected: {out}");
        assert_eq!(v["error_kind"], "invalid_output_schema");
        assert!(
            v["message"].as_str().unwrap().contains("exceeds"),
            "expected size-limit message, got: {}",
            v["message"]
        );
    }

    #[test]
    fn run_source_output_schema_includes_total_errors_field() {
        // Even when not truncated, `total_errors` and `truncated: false` must
        // be present so integrators can rely on the field existing.
        let schema = json!({ "type": "string" });
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&schema));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_kind"], "validation_failed");
        assert_eq!(v["truncated"], false);
        assert!(
            v["total_errors"].is_u64(),
            "total_errors must be present and a u64: {out}"
        );
        let total = v["total_errors"].as_u64().unwrap();
        assert!(
            total >= 1,
            "total_errors must be at least 1 for a failing schema: {out}"
        );
        let returned = v["errors"].as_array().unwrap().len();
        assert_eq!(
            returned as u64, total,
            "non-truncated total_errors must match returned errors length"
        );
    }

    #[test]
    fn run_source_output_schema_record_wrapper_format_documented_limitation() {
        // Documenting the known limitation: format_value emits Records as
        // {"type":"record","type_id":N,"fields":[positional values]}. A
        // schema that expects `{"type":"object","properties":{...}}` against
        // a record return will FAIL validation — even when the logical
        // record matches the integrator's mental model. Until a future
        // sprint adds logical projection, primitive return types are the
        // best fit for this gate.
        //
        // PURE_SOURCE returns Int(3), not a record — so we can't directly
        // demonstrate the wrapper here without a more involved fixture.
        // This test serves as a regression anchor: if `format_value` ever
        // changes its record/enum shape, the design doc and tool description
        // must be updated in lock-step. (No assertion needed beyond the
        // existing format_value tests; this comment is the cross-reference.)
    }

    // ── docs round-trip ──

    /// The three examples shipped in `docs/reference/policy-schema.md` must parse.
    /// If you edit those examples, edit these strings too.
    #[test]
    fn schema_doc_examples_parse() {
        let examples = [
            // 1. Allowlist domain only
            r#"{
              "default_allow": false,
              "rules": { "net.fetch": { "allow": true, "budget": 0 } },
              "net_policy": { "allowed_domains": ["api.openai.com"] }
            }"#,
            // 2. Allow-all minus filesystem writes
            r#"{
              "default_allow": true,
              "rules": { "fs.write": { "allow": false, "budget": 0 } }
            }"#,
            // 3. LLM call quota
            r#"{
              "default_allow": true,
              "rules": { "llm.call": { "allow": true, "budget": 5 } }
            }"#,
        ];
        for (i, src) in examples.iter().enumerate() {
            serde_json::from_str::<Policy>(src)
                .unwrap_or_else(|e| panic!("example {} failed to parse: {e}", i + 1));
        }
    }
}
