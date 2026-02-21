use boruna_bytecode::Value;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::vm::Vm;

const TRACE_LIMIT: usize = 500;

/// Compile and execute source, returning JSON with result or errors.
pub fn run_source(source: &str, policy: &str, max_steps: u64, trace: bool) -> String {
    // Compile
    let module = match boruna_compiler::compile("module", source) {
        Ok(m) => m,
        Err(e) => {
            return crate::tools::compile::compile_error_json(&e);
        }
    };

    // Build capability gateway
    let gw_policy = match policy {
        "deny-all" => Policy::deny_all(),
        _ => Policy::allow_all(),
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
