//! `boruna_policy_validate` MCP tool — strict-validate a policy JSON
//! body and return ok / error_kind. See `docs/design-policy-as-code.md`.

use super::TOOL_RESPONSE_PROTOCOL_VERSION;
use boruna_vm::policy_validate::{self, PolicyParseError, POLICY_SCHEMA_VERSION};

/// Validate a policy JSON body. Returns a successful tool response
/// with `success: true` if valid, or `success: true` (the tool
/// itself succeeded) with an `errors` array describing each
/// validation failure. Per project convention #2, `error_kind`
/// strings are stable.
///
/// Note: domain errors (validation failures) are reported as a
/// successful tool response with the structured `errors` field —
/// this matches the `boruna_check` shape and lets agents reason
/// about the result without distinguishing tool errors from
/// validation errors.
pub fn validate_policy(json: &str) -> String {
    match policy_validate::parse(json) {
        Ok(p) => serde_json::json!({
            "success": true,
            "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
            "ok": true,
            "schema_version": p.schema_version,
        })
        .to_string(),
        Err(e) => serde_json::json!({
            "success": true,
            "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
            "ok": false,
            "schema_version": POLICY_SCHEMA_VERSION,
            "errors": [error_to_json(&e)],
        })
        .to_string(),
    }
}

fn error_to_json(e: &PolicyParseError) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("error_kind".into(), e.error_kind().into());
    obj.insert("message".into(), e.to_string().into());
    match e {
        PolicyParseError::UnknownField { path, .. } => {
            obj.insert("path".into(), path.clone().into());
        }
        PolicyParseError::InvalidCapability { found, hint } => {
            obj.insert("found".into(), found.clone().into());
            if let Some(h) = hint {
                obj.insert("hint".into(), h.clone().into());
            }
        }
        PolicyParseError::InvalidNetPolicy { field, .. } => {
            obj.insert("field".into(), (*field).into());
        }
        PolicyParseError::UnknownSchemaVersion(v) => {
            obj.insert("found".into(), serde_json::Value::Number((*v).into()));
        }
        _ => {}
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn validate_ok_minimal() {
        let v = parse(&validate_policy("{}"));
        assert_eq!(v["success"], true);
        assert_eq!(v["ok"], true);
        assert_eq!(v["schema_version"], 1);
    }

    #[test]
    fn validate_ok_full() {
        let json = r#"{
            "default_allow": true,
            "rules": {"net.fetch": {"allow": true, "budget": 5}},
            "net_policy": {"timeout_ms": 1000, "max_response_bytes": 100, "allowed_methods": ["GET"]}
        }"#;
        let v = parse(&validate_policy(json));
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn validate_unknown_field() {
        let v = parse(&validate_policy(r#"{"foo": 1}"#));
        assert_eq!(v["success"], true);
        assert_eq!(v["ok"], false);
        assert_eq!(v["errors"][0]["error_kind"], "policy.unknown_field");
        assert_eq!(v["errors"][0]["path"], "foo");
    }

    #[test]
    fn validate_invalid_capability_with_hint() {
        let v = parse(&validate_policy(
            r#"{"rules": {"net": {"allow": true, "budget": 0}}}"#,
        ));
        assert_eq!(v["errors"][0]["error_kind"], "policy.invalid_capability");
        assert_eq!(v["errors"][0]["found"], "net");
        assert_eq!(v["errors"][0]["hint"], "net.fetch");
    }

    #[test]
    fn validate_unknown_schema_version() {
        let v = parse(&validate_policy(r#"{"schema_version": 2}"#));
        assert_eq!(
            v["errors"][0]["error_kind"],
            "policy.unknown_schema_version"
        );
        assert_eq!(v["errors"][0]["found"], 2);
    }

    #[test]
    fn validate_parse_error() {
        let v = parse(&validate_policy("{"));
        assert_eq!(v["errors"][0]["error_kind"], "policy.parse_error");
    }

    #[test]
    fn validate_invalid_net_policy() {
        let v = parse(&validate_policy(r#"{"net_policy": {"timeout_ms": 0}}"#));
        assert_eq!(v["errors"][0]["error_kind"], "policy.invalid_net_policy");
        assert_eq!(v["errors"][0]["field"], "timeout_ms");
    }
}
