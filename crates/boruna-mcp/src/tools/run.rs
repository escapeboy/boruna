use boruna_bytecode::Value;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::vm::Vm;
use serde_json::Value as JsonValue;

const TRACE_LIMIT: usize = 500;

/// Compile and execute source, returning JSON with result or errors.
///
/// `policy` accepts:
///   - `None` → `Policy::allow_all()` (legacy default)
///   - `Some(JsonValue::String("allow-all"|"deny-all"))` → corresponding shorthand
///   - `Some(JsonValue::Object(_))` → deserialize into [`Policy`]
///   - Anything else → returns `{"success": false, "error_kind": "invalid_policy", ...}`
pub fn run_source(source: &str, policy: Option<&JsonValue>, max_steps: u64, trace: bool) -> String {
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
            let mut json = serde_json::json!({
                "success": true,
                "result": format_value(&value),
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
        let out = run_source(PURE_SOURCE, None, 1_000_000, false);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "output: {out}");
    }

    #[test]
    fn run_source_with_structured_policy() {
        let policy = json!({
            "default_allow": true,
            "rules": { "fs.write": { "allow": false, "budget": 0 } }
        });
        let out = run_source(PURE_SOURCE, Some(&policy), 1_000_000, false);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true, "output: {out}");
    }

    #[test]
    fn run_source_invalid_policy_returns_error_kind() {
        let bad = json!(42);
        let out = run_source(PURE_SOURCE, Some(&bad), 1_000_000, false);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_kind"], "invalid_policy");
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
