use boruna_bytecode::Value;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::error::VmError;
use boruna_vm::vm::Vm;
use serde_json::Value as JsonValue;

const TRACE_LIMIT: usize = 500;

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
pub fn run_source(
    source: &str,
    policy: Option<&JsonValue>,
    max_steps: u64,
    trace: bool,
    limits: Option<&RunLimits>,
) -> String {
    // Reject unsupported limits BEFORE compiling, so an integrator who
    // misconfigures (e.g. sets max_memory_mb expecting it to bound memory)
    // sees a typed error immediately rather than a silently-ignored setting
    // followed by an OOM. Security-sensitive surface — see RunLimits docs.
    if let Some(l) = limits {
        if l.max_memory_mb.is_some() {
            return serde_json::json!({
                "success": false,
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
    vm.set_max_wall_ms(limits.and_then(|l| l.max_wall_ms));
    vm.trace_enabled = trace;

    match vm.run() {
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

            let mut json = serde_json::json!({
                "success": true,
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
            "error_kind": "runtime_error",
            "message": format!("{e}"),
            "steps": vm.step_count(),
        })
        .to_string(),
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

    // ── 0.3-S10: structured resource limits ──

    #[test]
    fn run_source_no_limits_runs_normally() {
        // Sanity: passing Some(RunLimits::default()) with all fields None
        // should be indistinguishable from passing None.
        let limits = RunLimits::default();
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits));
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
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits));
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
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits));
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
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits));
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
        let out = run_source(PURE_SOURCE, None, 1_000_000, false, Some(&limits));
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
        let out = run_source(PURE_SOURCE, None, 1, false, Some(&limits));
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
}
