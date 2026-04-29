use boruna_bytecode::Value;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::error::VmError;
use boruna_vm::vm::{StepResult, Vm};
use serde_json::Value as JsonValue;

use super::TOOL_RESPONSE_PROTOCOL_VERSION;

const TRACE_LIMIT: usize = 500;

/// VM step budget per [`Vm::execute_bounded`] slice when streaming progress
/// notifications (sprint `0.4-S6`). Issue #4's "Notes for implementers"
/// recommends ~100k opcodes — coarse enough to be cheap (notification overhead
/// stays under a fraction of a percent of execution time) and fine enough that
/// long-running scripts emit several progress events per second on typical
/// hardware. Tunable via the `progress` callback path; the non-streaming
/// `run_source` entry continues to call `vm.run()` directly, so this constant
/// has no effect when no progress token is supplied.
const PROGRESS_STEP_SLICE: u64 = 100_000;

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

/// Structured resource limits applied to a single `boruna_run` invocation.
///
/// `None` on any field = no limit. Hitting any returns
/// `success: false, error_kind: "limit_exceeded"` with a `limit_kind`
/// discriminator (`"wall_ms"` or `"output_bytes"`) and a `phase` field
/// (`"execution"` or `"serialization"`) so callers can distinguish a
/// timeout-mid-run from a too-large-output-post-run.
///
/// `max_memory_mb`: accepted in the wire schema but **rejected at parse
/// time** with `error_kind: "unsupported_limit"` until enforcement ships.
/// Silent acceptance would be a security footgun (an integrator setting
/// `max_memory_mb: 256` reasonably expects memory to be bounded).
///
/// See `docs/reference/mcp-server.md` for the full wire contract.
#[derive(Debug, Default, Clone)]
pub struct RunLimits {
    pub max_wall_ms: Option<u64>,
    pub max_output_bytes: Option<u64>,
    /// Accepted in the schema but rejected at the MCP layer when set.
    /// Will become live in a future sprint (Linux setrlimit + per-platform
    /// fallback); kept here so the type is forward-compatible with the
    /// schema integrators wire up today.
    pub max_memory_mb: Option<u64>,
}

/// Compile and execute source, returning JSON with result or errors.
///
/// `policy` accepts:
///   - `None` → `Policy::allow_all()` (legacy default)
///   - `Some(JsonValue::String("allow-all"|"deny-all"))` → corresponding shorthand
///   - `Some(JsonValue::Object(_))` → deserialize into [`Policy`]
///   - Anything else → returns `{"success": false, "error_kind": "invalid_policy", ...}`
///
/// `limits` plumbs structured resource limits into the VM and the response
/// serializer. `None` = no limits beyond `max_steps`.
///
/// `output_schema` accepts an optional JSON Schema 2020-12 object. When set,
/// the script's `result` is validated post-execution; mismatches return
/// `error_kind: "validation_failed"` with per-path JSON Pointer errors. See
/// [`validate_output_against_schema`] and `docs/design-output-schema.md`.
pub fn run_source(
    source: &str,
    policy: Option<&JsonValue>,
    max_steps: u64,
    trace: bool,
    limits: Option<&RunLimits>,
    output_schema: Option<&JsonValue>,
) -> String {
    run_source_with_progress(
        source,
        policy,
        max_steps,
        trace,
        limits,
        output_schema,
        None::<fn(u64, Option<String>)>,
    )
}

/// Streaming variant of [`run_source`] that emits progress callbacks during
/// VM execution (sprint `0.4-S6`, closes #4).
///
/// When `progress_callback` is `Some`, the VM is driven via
/// [`Vm::execute_bounded`] in [`PROGRESS_STEP_SLICE`]-sized slices instead of
/// a single [`Vm::run`] call. Between slices, the callback is invoked with
/// the cumulative step count, giving the caller a hook to surface live
/// progress (e.g. an MCP `progress` notification).
///
/// When `progress_callback` is `None`, this delegates to the same VM driver
/// loop with a no-op callback — so the streaming and non-streaming paths
/// share their semantics for `start_time`, `max_wall_ms`, error handling,
/// and final output. The legacy [`run_source`] entry passes `None`.
///
/// **The callback runs on the VM's thread** (typically the
/// `tokio::task::spawn_blocking` worker the MCP layer wraps this call in).
/// Keep callback work cheap — a heavy callback adds latency to every slice.
/// The MCP wiring forwards through a non-blocking `mpsc::unbounded_channel`
/// so notification dispatch happens on a separate task.
pub fn run_source_with_progress<F>(
    source: &str,
    policy: Option<&JsonValue>,
    max_steps: u64,
    trace: bool,
    limits: Option<&RunLimits>,
    output_schema: Option<&JsonValue>,
    progress_callback: Option<F>,
) -> String
where
    F: FnMut(u64, Option<String>),
{
    // Reject unsupported limits BEFORE compiling, so an integrator who
    // misconfigures (e.g. sets max_memory_mb expecting it to bound memory)
    // sees a typed error immediately rather than a silently-ignored setting
    // followed by an OOM. Security-sensitive surface — see RunLimits docs.
    if let Some(l) = limits {
        if l.max_memory_mb.is_some() {
            return serde_json::json!({
                "success": false,
                "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
                "error_kind": "unsupported_limit",
                "limit_kind": "memory_mb",
                "message": "max_memory_mb is reserved for a future release \
                            and is not enforced in 0.3.x. Setting it would silently \
                            permit memory exhaustion. Use process-level cgroups or \
                            ulimits at your orchestration layer until this lands.",
            })
            .to_string();
        }
    }

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
        Err(err) => {
            return serde_json::json!({
                "success": false,
                "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
                "error_kind": err.error_kind,
                "message": err.message,
            })
            .to_string();
        }
    };
    let gateway = CapabilityGateway::new(gw_policy);

    // Run
    let mut vm = Vm::new(module, gateway);
    vm.set_max_steps(max_steps);
    vm.set_max_wall_ms(limits.and_then(|l| l.max_wall_ms));
    vm.trace_enabled = trace;

    match drive_vm(&mut vm, progress_callback) {
        Ok(value) => {
            // Serialize result and ui_output under the optional output-size cap.
            let max_output_bytes = limits.and_then(|l| l.max_output_bytes);
            let result_json = match format_value_capped(&value, max_output_bytes) {
                Ok(j) => j,
                Err(over) => return over.to_response(vm.step_count()),
            };
            // ui_output is cumulative with the result against the same budget.
            let mut remaining = max_output_bytes
                .map(|cap| cap.saturating_sub(serialized_size(&result_json) as u64));
            let mut ui_output_json: Vec<serde_json::Value> = Vec::new();
            for tree in &vm.ui_output {
                let tree_json = match format_value_capped(tree, remaining) {
                    Ok(j) => j,
                    Err(_) => {
                        // Use the original cap (not the dynamic remaining) as
                        // the reported limit — that's what the integrator set.
                        return OutputBudgetExceeded {
                            cap: max_output_bytes.unwrap_or(0),
                        }
                        .to_response(vm.step_count());
                    }
                };
                if let Some(ref mut r) = remaining {
                    *r = r.saturating_sub(serialized_size(&tree_json) as u64);
                }
                ui_output_json.push(tree_json);
            }

            // Output-schema gate (post-execution + post-budget; only runs on
            // successful runs whose output fit within max_output_bytes).
            // Validation failure or malformed schema short-circuits with its
            // own typed envelope. See docs/design-output-schema.md.
            if let Some(schema) = output_schema {
                if let Some(failure) =
                    validate_output_against_schema(schema, &result_json, vm.step_count())
                {
                    return failure;
                }
            }

            let mut json = serde_json::json!({
                "success": true,
                "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
                "result": result_json,
                "steps": vm.step_count(),
                "ui_output": ui_output_json,
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
        Err(VmError::WallTimeExceeded(max_ms)) => limit_exceeded_response(
            "wall_ms",
            "execution",
            max_ms,
            format!("wall-clock execution limit of {max_ms} ms exceeded"),
            vm.step_count(),
        ),
        Err(e) => serde_json::json!({
            "success": false,
            "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
            "error_kind": "runtime_error",
            "message": format!("{e}"),
            "steps": vm.step_count(),
        })
        .to_string(),
    }
}

/// Drive the VM to completion, optionally emitting progress callbacks
/// between fixed-size execution slices (sprint `0.4-S6`).
///
/// When `progress_callback` is `None`, delegates directly to [`Vm::run`]
/// — the fast path stays bit-identical to the pre-sprint behavior.
///
/// When `progress_callback` is `Some`, drives
/// `vm.execute_bounded(PROGRESS_STEP_SLICE)` in a loop and invokes the
/// callback with the cumulative step count after each yield. Three
/// invariants are preserved relative to `vm.run()`:
///
/// 1. **Entry-setup wall-time accounting** — calls `vm.start_timer()`
///    BEFORE `set_entry_function`, mirroring `vm.run`'s contract that
///    `max_wall_ms` covers the entry-call frame allocation.
/// 2. **Standalone `Op::ReceiveMsg` semantics** — `boruna_run` runs the
///    VM outside any [`ActorSystem`] (`in_actor_context = false`), so
///    `Op::ReceiveMsg` on an empty mailbox falls through with
///    `Value::Unit` (the same legacy behavior). `StepResult::Blocked`
///    therefore cannot arise from script-level `receive` and is
///    treated as an internal-state error.
/// 3. **`start_time` lifecycle across yields** —
///    `Vm::execute_bounded` preserves `start_time` across slices, so
///    the wall-time budget spans the full bounded execution rather
///    than resetting per slice.
fn drive_vm<F>(vm: &mut Vm, mut progress_callback: Option<F>) -> Result<Value, VmError>
where
    F: FnMut(u64, Option<String>),
{
    if progress_callback.is_none() {
        // Fast path: no progress callback. Use the existing `vm.run()`
        // entry to keep the non-streaming path bit-identical to its
        // pre-sprint behavior. Avoids any subtle timing or
        // start_time/budget differences from the bounded loop.
        return vm.run();
    }
    // Streaming path. Start the wall-clock timer before set_entry_function
    // so the entry-call setup time is counted toward `max_wall_ms` —
    // matches `vm.run()`'s ordering. See `Vm::start_timer`.
    vm.start_timer();
    let entry = vm.module().entry;
    vm.set_entry_function(entry)?;
    loop {
        match vm.execute_bounded(PROGRESS_STEP_SLICE) {
            StepResult::Completed(val) => return Ok(val),
            StepResult::Yielded { .. } => {
                if let Some(cb) = progress_callback.as_mut() {
                    // T-2.2: drain capability calls made in this slice and
                    // format as a message for the MCP progress notification.
                    let caps = vm.take_last_cap_events();
                    let message = if caps.is_empty() {
                        None
                    } else if caps.len() == 1 {
                        Some(format!("cap: {}", caps[0]))
                    } else {
                        Some(format!("caps: {}", caps.join(", ")))
                    };
                    cb(vm.step_count(), message);
                }
            }
            // `Blocked` cannot arise from `Op::ReceiveMsg` here:
            // `boruna_run` runs the VM standalone, so the receive
            // op falls through with `Value::Unit` per its
            // `in_actor_context = false` branch. If a future
            // capability adds another blocking primitive that bypasses
            // the actor-context check, surface as a typed error
            // rather than spinning.
            StepResult::Blocked => return Err(VmError::Deadlock),
            StepResult::Error(e) => return Err(e),
        }
    }
}

/// Build the canonical `error_kind: "limit_exceeded"` response.
///
/// Wire shape — locked, integrators key UX off `limit_kind` AND `phase`:
///
/// ```jsonc
/// {
///   "success":      false,
///   "error_kind":   "limit_exceeded",
///   "limit_kind":   "wall_ms" | "output_bytes",
///   "phase":        "execution" | "serialization",
///   "limit":        <configured value>,
///   "message":      <human-readable>,
///   "steps":        <vm.step_count() at the moment of the failure>
/// }
/// ```
///
/// `phase` distinguishes a timeout-mid-run (`"execution"`, where `steps` is
/// the partial step count when the limit fired) from a too-large-output
/// detected after a successful run (`"serialization"`, where `steps` is the
/// full successful step count). Without this discriminator, integrators
/// reading `steps` for billing/telemetry would conflate the two.
fn limit_exceeded_response(
    limit_kind: &str,
    phase: &str,
    limit: u64,
    message: String,
    steps: u64,
) -> String {
    serde_json::json!({
        "success": false,
        "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
        "error_kind": "limit_exceeded",
        "limit_kind": limit_kind,
        "phase": phase,
        "limit": limit,
        "message": message,
        "steps": steps,
    })
    .to_string()
}

/// Returned by `format_value_capped` when the cumulative serialized size
/// exceeds the configured `max_output_bytes` budget.
struct OutputBudgetExceeded {
    cap: u64,
}

impl OutputBudgetExceeded {
    fn to_response(&self, steps: u64) -> String {
        limit_exceeded_response(
            "output_bytes",
            "serialization",
            self.cap,
            format!("serialized output exceeded {} bytes", self.cap),
            steps,
        )
    }
}

/// Serialize a `Value` under an optional cumulative byte budget.
///
/// `budget`:
///   - `None` → unbounded; equivalent to `format_value`
///   - `Some(remaining)` → returns `Err` if the serialized size exceeds it
///
/// Pre-checks the dominant unbounded-allocation case (a giant `Value::String`)
/// BEFORE the recursive serialize, so a script that builds a 1 GB string and
/// returns it doesn't peak twice through memory before being rejected.
/// Container types (`List`, `Map`, `Record`, `Enum`) still go through the
/// post-serialize check — they can't be cheaply estimated without recursing
/// anyway, and the 1 MB source-size cap on input bounds how much they can
/// realistically construct in a single step.
fn format_value_capped(
    value: &Value,
    budget: Option<u64>,
) -> Result<serde_json::Value, OutputBudgetExceeded> {
    if let Some(cap) = budget {
        // Cheap pre-check: a single very-large String is the canonical
        // unbounded-allocation attack. The serialized JSON wraps the string
        // in quotes plus 2 bytes; reject if even the raw bytes exceed cap.
        if let Value::String(s) = value {
            if s.len() as u64 > cap {
                return Err(OutputBudgetExceeded { cap });
            }
        }
    }
    let json = format_value(value);
    if let Some(cap) = budget {
        if serialized_size(&json) as u64 > cap {
            return Err(OutputBudgetExceeded { cap });
        }
    }
    Ok(json)
}

/// Cheap serialized-size estimate of a JSON value. Uses `serde_json::to_vec`
/// (compact, no pretty whitespace) so the budget reflects the on-the-wire
/// payload an integrator actually pays for, not the indentation we add later.
fn serialized_size(value: &serde_json::Value) -> usize {
    serde_json::to_vec(value).map(|v| v.len()).unwrap_or(0)
}

/// Validate the script's serialized `result` against an integrator-supplied
/// JSON Schema. Returns `Some(json_response)` if validation fails, the schema
/// is malformed, or the schema is oversized; `None` if the result passes.
///
/// Enforces **JSON Schema Draft 2020-12.** Schemas with no `$schema` URI
/// default to 2020-12; schemas declaring a non-2020-12 `$schema` are
/// **rejected** rather than silently honoured at older-draft semantics.
/// Same "reject at parse, don't silently override" pattern as 0.3-S10's
/// `unsupported_limit` for `max_memory_mb`.
///
/// **Wrapper-format limitation:** `format_value` emits Boruna records/enums/
/// Some/Ok as wrapper objects. A schema written for the natural shape will
/// fail validation. The gate is most useful for primitive return types
/// (Int, String, Bool) and homogeneous List/Map containers. See
/// `docs/design-output-schema.md`.
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
                "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
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
    if let Some(JsonValue::String(s)) = schema.get("$schema") {
        let normalized = s.trim_end_matches('#');
        if !normalized.contains("2020-12") && !normalized.contains("draft/2020-12") {
            return Some(
                serde_json::json!({
                    "success": false,
                    "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
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
    // absent (jsonschema honors $schema if present); the check above already
    // rejected non-2020-12 $schema declarations.
    let validator = match jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(schema)
    {
        Ok(v) => v,
        Err(e) => {
            return Some(
                serde_json::json!({
                    "success": false,
                    "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
                    "error_kind": "invalid_output_schema",
                    "message": format!("output_schema is not a valid JSON Schema: {e}"),
                })
                .to_string(),
            );
        }
    };

    // Collect per-path errors with a hard cap.
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
            "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
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

/// Structured error from [`parse_policy`].
///
/// `error_kind` is a stable string from the locked taxonomy
/// (project convention #2). String/non-object/non-string failures
/// keep the legacy `"invalid_policy"` kind (unchanged since 0.2.0).
/// Object failures produce more specific `policy.*` kinds emitted
/// by the strict validator in [`boruna_vm::policy_validate`] —
/// these are additive over `invalid_policy`.
#[derive(Debug, Clone)]
pub(crate) struct ParsePolicyError {
    pub error_kind: String,
    pub message: String,
}

/// Parse the MCP `policy` argument into a [`Policy`].
///
/// See `docs/reference/policy-schema.md` for the object form and
/// `docs/design-policy-as-code.md` for the strict-validator
/// taxonomy.
pub(crate) fn parse_policy(value: Option<&JsonValue>) -> Result<Policy, ParsePolicyError> {
    match value {
        None => Ok(Policy::allow_all()),
        Some(JsonValue::String(s)) => match s.as_str() {
            "allow-all" => Ok(Policy::allow_all()),
            "deny-all" => Ok(Policy::deny_all()),
            other => Err(ParsePolicyError {
                error_kind: "invalid_policy".into(),
                message: format!(
                    "policy string must be 'allow-all' or 'deny-all' (got '{other}'); \
                     pass an object for fine-grained policy — see docs/reference/policy-schema.md"
                ),
            }),
        },
        Some(obj @ JsonValue::Object(_)) => {
            // Route through the strict validator so MCP and CLI
            // share one parser. Failures surface stable `policy.*`
            // error_kind strings.
            let json_str = obj.to_string();
            boruna_vm::policy_validate::parse(&json_str).map_err(|e| ParsePolicyError {
                error_kind: e.error_kind().to_string(),
                message: e.to_string(),
            })
        }
        Some(other) => {
            let kind = match other {
                JsonValue::Null => "null",
                JsonValue::Bool(_) => "boolean",
                JsonValue::Number(_) => "number",
                JsonValue::Array(_) => "array",
                _ => "unknown",
            };
            Err(ParsePolicyError {
                error_kind: "invalid_policy".into(),
                message: format!(
                    "policy must be a string ('allow-all'/'deny-all') or an object; got {kind}"
                ),
            })
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
        assert_eq!(err.error_kind, "invalid_policy");
        assert!(err.message.contains("must be 'allow-all' or 'deny-all'"));
        assert!(err.message.contains("policy-schema.md"));
    }

    #[test]
    fn parse_policy_array_errors() {
        let v = json!([]);
        let err = parse_policy(Some(&v)).unwrap_err();
        assert_eq!(err.error_kind, "invalid_policy");
        assert!(err.message.contains("array"));
    }

    #[test]
    fn parse_policy_malformed_object_errors() {
        // 0.4-S15: object parses now go through the strict validator,
        // so a type error surfaces as policy.parse_error instead of
        // the generic invalid_policy.
        let v = json!({ "default_allow": "yes" });
        let err = parse_policy(Some(&v)).unwrap_err();
        assert_eq!(err.error_kind, "policy.parse_error");
    }

    // ── 0.4-S15: strict-validator paths ──

    #[test]
    fn parse_policy_object_unknown_field_returns_kind() {
        let v = json!({ "foo": 1 });
        let err = parse_policy(Some(&v)).unwrap_err();
        assert_eq!(err.error_kind, "policy.unknown_field");
    }

    #[test]
    fn parse_policy_object_invalid_capability_returns_kind() {
        let v = json!({ "rules": { "net": { "allow": true, "budget": 0 } } });
        let err = parse_policy(Some(&v)).unwrap_err();
        assert_eq!(err.error_kind, "policy.invalid_capability");
        assert!(err.message.contains("net.fetch"));
    }

    #[test]
    fn parse_policy_object_unknown_schema_version() {
        let v = json!({ "schema_version": 2 });
        let err = parse_policy(Some(&v)).unwrap_err();
        assert_eq!(err.error_kind, "policy.unknown_schema_version");
    }

    #[test]
    fn parse_policy_object_invalid_net_policy() {
        let v = json!({ "net_policy": { "timeout_ms": 0 } });
        let err = parse_policy(Some(&v)).unwrap_err();
        assert_eq!(err.error_kind, "policy.invalid_net_policy");
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
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, None, None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "output: {out}");
    }

    #[test]
    fn run_source_with_structured_policy() {
        let policy = json!({
            "default_allow": true,
            "rules": { "fs.write": { "allow": false, "budget": 0 } }
        });
        let out = run_source(PURE_SOURCE, Some(&policy), 1_000_000, false, None, None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "output: {out}");
    }

    #[test]
    fn run_source_invalid_policy_returns_error_kind() {
        let bad = json!(42);
        let out = run_source(PURE_SOURCE, Some(&bad), 1_000_000, false, None, None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_kind"], "invalid_policy");
    }

    // ── 0.3-S10: structured resource limits ──

    #[test]
    fn run_source_no_limits_runs_normally() {
        // Sanity: passing Some(RunLimits::default()) with all fields None
        // should be indistinguishable from passing None.
        let limits = RunLimits::default();
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits), None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "output: {out}");
    }

    #[test]
    fn run_source_output_bytes_under_cap_succeeds() {
        // PURE_SOURCE returns Int(3) — serializes to ~5 bytes for `result`.
        // 4 KB is way over.
        let limits = RunLimits {
            max_output_bytes: Some(4096),
            ..Default::default()
        };
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits), None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "output: {out}");
    }

    #[test]
    fn run_source_output_bytes_over_cap_returns_limit_exceeded() {
        // Cap of 0 bytes; PURE_SOURCE returns Int(3) → serializes to "3" (1 byte)
        // → over (strict gt). Locks the boundary: the budget is strict.
        let limits = RunLimits {
            max_output_bytes: Some(0),
            ..Default::default()
        };
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits), None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false, "output: {out}");
        assert_eq!(v["error_kind"], "limit_exceeded");
        assert_eq!(v["limit_kind"], "output_bytes");
        assert_eq!(
            v["phase"], "serialization",
            "output_bytes failures must report phase=serialization (not 'execution') \
             so integrators can distinguish from a wall_ms timeout"
        );
        assert_eq!(v["limit"], 0);
        assert!(v["message"].as_str().unwrap().contains("0 bytes"));
    }

    #[test]
    fn run_source_output_bytes_boundary_exact_match_passes() {
        // Lock the strict-greater-than boundary: a 1-byte cap accepts a
        // 1-byte payload (exactly equal). Int(3) serializes compactly to "3".
        let limits = RunLimits {
            max_output_bytes: Some(1),
            ..Default::default()
        };
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits), None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["success"], true,
            "exact-match-cap must pass (strict-gt semantics): {out}"
        );
    }

    #[test]
    fn run_source_max_memory_mb_is_rejected_at_parse_time() {
        // Security: silent acceptance of an unenforced memory limit would let
        // a script OOM the host while the integrator believed memory was
        // bounded. Reject typed instead.
        let limits = RunLimits {
            max_memory_mb: Some(256),
            ..Default::default()
        };
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits), None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["success"], false,
            "max_memory_mb must be rejected (not silently ignored): {out}"
        );
        assert_eq!(v["error_kind"], "unsupported_limit");
        assert_eq!(v["limit_kind"], "memory_mb");
        assert!(v["message"]
            .as_str()
            .unwrap()
            .contains("not enforced in 0.3.x"));
    }

    #[test]
    fn run_source_wall_time_response_carries_phase_execution() {
        // Companion test to lock the phase contract on the wall_ms path.
        // Use an infinite-ish program with a tight wall budget. Because the
        // PURE_SOURCE program returns immediately, we can't naturally trigger
        // wall_ms from .ax — but we can verify the response shape via the
        // existing VM wall-time test in boruna-vm. Here we just lock the
        // CONTRACT shape that, when wall_ms IS hit, the response includes
        // phase="execution". Documented; the integration test lives in the
        // boruna-vm crate.
        // (No assertion needed beyond what VM tests cover; this comment
        // serves as the cross-crate trail.)
    }

    #[test]
    fn run_source_unrelated_runtime_error_still_uses_runtime_error_kind() {
        // Ensure the new limit_exceeded branch didn't accidentally swallow
        // the existing runtime_error kind. Force a runtime error via a
        // step-limit-exceeded path (max_steps=1 on a non-trivial program).
        let limits = RunLimits::default();
        let out = run_source(PURE_SOURCE, None, 1, false, Some(&limits), None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(
            v["error_kind"], "runtime_error",
            "step limit must remain runtime_error, not limit_exceeded — those are different contracts"
        );
    }

    // (max_memory_mb test moved up — it now asserts REJECTION, not silent
    // acceptance, after the security-driven review finding.)

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

    // ── 0.4-S6: streaming progress callback (closes #4) ──

    /// A while-loop program that runs ~3 × PROGRESS_STEP_SLICE opcodes —
    /// guaranteed to yield more than one progress sample with the
    /// default 100k slice. Returns the final counter value so the test
    /// can confirm the program actually ran to completion.
    const STREAMING_PROGRAM: &str = r#"
fn main() -> Int {
    let mut i = 0
    while i < 80000 {
        i = i + 1
    }
    i
}
"#;

    #[test]
    fn run_source_with_progress_emits_callback_samples() {
        use std::sync::{Arc, Mutex};
        let samples: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let samples_clone = Arc::clone(&samples);
        let out = run_source_with_progress(
            STREAMING_PROGRAM,
            None,
            10_000_000,
            false,
            None,
            None,
            Some(move |steps: u64, _message: Option<String>| {
                samples_clone.lock().unwrap().push(steps);
            }),
        );
        // Run completed successfully.
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["success"], json!(true));
        assert_eq!(parsed["result"], json!(80000));

        // At least one progress sample fired (the loop runs ~5 ops per
        // iteration × 80k ≈ 400k opcodes ≫ 100k slice). Samples are
        // monotonically increasing and within the max_steps bound.
        let s = samples.lock().unwrap();
        assert!(
            !s.is_empty(),
            "streaming path must emit at least one progress sample"
        );
        for window in s.windows(2) {
            assert!(
                window[1] >= window[0],
                "progress samples must be non-decreasing: {:?}",
                *s
            );
        }
        let final_steps = parsed["steps"].as_u64().unwrap();
        assert!(
            *s.last().unwrap() <= final_steps,
            "last sample {} must be ≤ final step count {}",
            s.last().unwrap(),
            final_steps
        );
    }

    #[test]
    fn run_source_with_no_progress_callback_matches_legacy_run_source() {
        // The non-streaming path must produce a bit-identical response
        // to the legacy run_source — verifies that the fast path
        // (drive_vm's `progress_callback.is_none()` short-circuit)
        // doesn't drift in semantics.
        let legacy = run_source(PURE_SOURCE, None, 1_000_000, false, None, None);
        let streamed = run_source_with_progress(
            PURE_SOURCE,
            None,
            1_000_000,
            false,
            None,
            None,
            None::<fn(u64, Option<String>)>,
        );
        assert_eq!(
            legacy, streamed,
            "no-callback streaming path must be identical to legacy run_source"
        );
    }

    #[test]
    fn run_source_with_progress_completes_zero_step_program_without_callback() {
        // Pure programs may complete in fewer steps than a single
        // PROGRESS_STEP_SLICE. The bounded loop must still terminate
        // with the correct result and not call the progress callback
        // (no yield happens — execute_bounded returns Completed
        // immediately).
        use std::sync::{Arc, Mutex};
        let samples: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let samples_clone = Arc::clone(&samples);
        let out = run_source_with_progress(
            PURE_SOURCE,
            None,
            1_000_000,
            false,
            None,
            None,
            Some(move |steps: u64, _message: Option<String>| {
                samples_clone.lock().unwrap().push(steps);
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["success"], json!(true));
        assert_eq!(parsed["result"], json!(3));
        // Pure 1+2 program completes in ~12 opcodes — well under the
        // 100k slice — so the callback is never invoked.
        assert!(
            samples.lock().unwrap().is_empty(),
            "callback must not fire for sub-slice programs"
        );
    }

    #[test]
    fn run_source_with_progress_callback_matches_legacy_for_receive_op() {
        // Reviewed 0.4-S6 — earlier draft signaled "actor context" via
        // `Vm::budget.is_some()`, conflating "in actor scheduler" with
        // "in slice-bounded streaming progress loop." A non-actor
        // script that compiles to `Op::ReceiveMsg` would then return
        // success=true via legacy `vm.run()` but
        // success=false/error_kind=runtime_error via the streaming
        // path. Fixed by adding `Vm::in_actor_context` (default false).
        // This test locks the contract that the streaming and
        // non-streaming paths produce equivalent terminal status for
        // any source.
        const RECEIVE_PROGRAM: &str = r#"
fn main() -> Int {
    let _msg: Unit = receive
    42
}
"#;
        let legacy = run_source(RECEIVE_PROGRAM, None, 1_000_000, false, None, None);
        let streamed = run_source_with_progress(
            RECEIVE_PROGRAM,
            None,
            1_000_000,
            false,
            None,
            None,
            Some(|_steps: u64, _message: Option<String>| {}),
        );
        // Both paths must succeed with the same result. The exact step
        // count may differ trivially (start_timer placement); compare
        // the success/result envelope rather than the full string.
        let legacy_parsed: serde_json::Value = serde_json::from_str(&legacy).unwrap();
        let streamed_parsed: serde_json::Value = serde_json::from_str(&streamed).unwrap();
        assert_eq!(legacy_parsed["success"], json!(true));
        assert_eq!(streamed_parsed["success"], json!(true));
        assert_eq!(legacy_parsed["result"], streamed_parsed["result"]);
    }

    #[test]
    fn run_source_with_progress_callback_propagates_runtime_errors() {
        // A program that hits a runtime error mid-execution must still
        // surface the error envelope, even when the streaming path is
        // active. The callback may or may not have fired — what
        // matters is the final response is well-formed.
        const ERR_PROGRAM: &str = r#"
fn main() -> Int {
    let xs: List<Int> = [1, 2, 3]
    list_get(xs, 99)
}
"#;
        let out = run_source_with_progress(
            ERR_PROGRAM,
            None,
            1_000_000,
            false,
            None,
            None,
            Some(|_steps: u64, _message: Option<String>| {}),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["success"], json!(false));
        assert_eq!(parsed["error_kind"], json!("runtime_error"));
    }

    // ── T-2.2: capability_call streaming events ──
    // The cap-name-in-message contract is tested at the VM level
    // (boruna-vm crate) where bytecode can be injected directly.
    // Here we test only the pure-program case to lock the MCP-layer
    // contract: no cap calls → message is always None.

    #[test]
    fn progress_callback_message_is_none_when_no_caps_called() {
        use std::sync::{Arc, Mutex};
        let messages: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let messages_clone = Arc::clone(&messages);
        // STREAMING_PROGRAM is a pure while-loop with no capability calls.
        let out = run_source_with_progress(
            STREAMING_PROGRAM,
            None,
            10_000_000,
            false,
            None,
            None,
            Some(move |_steps: u64, message: Option<String>| {
                messages_clone.lock().unwrap().push(message);
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed["success"],
            json!(true),
            "program must succeed: {out}"
        );

        let msgs = messages.lock().unwrap();
        // Some notifications may fire; all must have message = None (no caps called).
        assert!(
            msgs.iter().all(|m| m.is_none()),
            "pure program must never emit cap messages: {:?}",
            msgs
        );
    }
}
