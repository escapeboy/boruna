use boruna_bytecode::Value;
use boruna_framework::runtime::{AppMessage, AppRuntime};

/// Validate that source conforms to the App protocol.
pub fn validate_app(source: &str) -> String {
    // Parse to AST first
    let tokens = match boruna_compiler::lexer::lex(source) {
        Ok(t) => t,
        Err(e) => {
            return crate::tools::compile::compile_error_json(&e);
        }
    };
    let program = match boruna_compiler::parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            return crate::tools::compile::compile_error_json(&e);
        }
    };

    // Use AppValidator
    match boruna_framework::validate::AppValidator::validate(&program) {
        Ok(result) => {
            serde_json::json!({
                "success": true,
                "has_init": result.has_init,
                "has_update": result.has_update,
                "has_view": result.has_view,
                "has_policies": result.has_policies,
                "state_type": result.state_type,
                "message_type": result.message_type,
                "errors": result.errors,
                "valid": result.errors.is_empty() && result.has_init && result.has_update && result.has_view,
            })
            .to_string()
        }
        Err(e) => {
            serde_json::json!({
                "success": false,
                "error_kind": "framework_error",
                "message": format!("{e}"),
            })
            .to_string()
        }
    }
}

/// Run a framework app with a sequence of messages.
pub fn test_app(source: &str, messages: &[String]) -> String {
    // Compile
    let module = match boruna_compiler::compile("app", source) {
        Ok(m) => m,
        Err(e) => {
            return crate::tools::compile::compile_error_json(&e);
        }
    };

    // Create runtime
    let mut runtime = match AppRuntime::new(module) {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({
                "success": false,
                "error_kind": "framework_error",
                "message": format!("{e}"),
            })
            .to_string();
        }
    };

    let init_state = format_value_brief(runtime.state());

    let mut cycles = Vec::new();
    for msg_str in messages {
        let (tag, payload) = parse_message(msg_str);
        let msg = AppMessage::new(tag, payload);

        match runtime.send(msg) {
            Ok((new_state, effects, ui_tree)) => {
                cycles.push(serde_json::json!({
                    "message": msg_str,
                    "state": format_value_brief(&new_state),
                    "effects": effects.len(),
                    "ui_tree": ui_tree.as_ref().map(format_value_brief),
                }));
            }
            Err(e) => {
                cycles.push(serde_json::json!({
                    "message": msg_str,
                    "error": format!("{e}"),
                }));
                // Return partial results
                return serde_json::json!({
                    "success": false,
                    "error_kind": "framework_error",
                    "message": format!("cycle {} failed: {e}", cycles.len()),
                    "init_state": init_state,
                    "cycles": cycles,
                })
                .to_string();
            }
        }
    }

    serde_json::json!({
        "success": true,
        "init_state": init_state,
        "cycles": cycles,
        "final_state": format_value_brief(runtime.state()),
        "total_cycles": runtime.cycle(),
    })
    .to_string()
}

/// Parse "tag:payload" message format.
fn parse_message(s: &str) -> (String, Value) {
    let s = s.trim();
    if let Some(idx) = s.find(':') {
        let tag = s[..idx].to_string();
        let payload_str = s[idx + 1..].trim();
        let payload = if let Ok(n) = payload_str.parse::<i64>() {
            Value::Int(n)
        } else {
            Value::String(payload_str.to_string())
        };
        (tag, payload)
    } else {
        (s.to_string(), Value::Int(0))
    }
}

/// Brief value formatting for state display.
fn format_value_brief(value: &Value) -> serde_json::Value {
    match value {
        Value::Int(n) => serde_json::json!(n),
        Value::Float(f) => serde_json::json!(f),
        Value::String(s) => serde_json::json!(s),
        Value::Bool(b) => serde_json::json!(b),
        Value::Unit => serde_json::json!(null),
        Value::None => serde_json::json!("None"),
        Value::Some(v) => serde_json::json!({"Some": format_value_brief(v)}),
        Value::Ok(v) => serde_json::json!({"Ok": format_value_brief(v)}),
        Value::Err(v) => serde_json::json!({"Err": format_value_brief(v)}),
        Value::List(items) => {
            serde_json::json!(items.iter().map(format_value_brief).collect::<Vec<_>>())
        }
        Value::Record { fields, .. } => {
            serde_json::json!(fields
                .iter()
                .map(format_value_brief)
                .collect::<Vec<serde_json::Value>>())
        }
        Value::Enum {
            variant, payload, ..
        } => {
            serde_json::json!({
                "variant": variant,
                "payload": format_value_brief(payload),
            })
        }
        Value::Map(entries) => {
            let obj: serde_json::Map<String, serde_json::Value> = entries
                .iter()
                .map(|(k, v)| (k.clone(), format_value_brief(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        Value::ActorId(id) => serde_json::json!({"actor_id": id}),
        Value::FnRef(idx) => serde_json::json!({"fn_ref": idx}),
    }
}
