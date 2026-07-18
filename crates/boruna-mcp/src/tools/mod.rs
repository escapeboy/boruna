pub mod capability;
pub mod check;
pub mod compile;
pub mod framework;
pub mod policy;
pub mod run;
pub mod sealed;
pub mod symbols;
pub mod template;
pub mod workflow;

/// Wire-format version of `boruna-mcp` tool responses.
///
/// Bumped on **breaking shape changes** to any tool response envelope —
/// field rename, removal, type change, or change to `error_kind` semantics.
/// Additive changes (new optional field) keep the same protocol_version.
///
/// Every tool response — success and failure — includes this field so
/// integrators can build version-aware parsers. Documented in
/// `docs/reference/mcp-server.md` under "Stability".
///
/// FleetQ ask [#6](https://github.com/escapeboy/boruna/issues/6).
pub(crate) const TOOL_RESPONSE_PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod protocol_version_tests {
    //! Lock the invariant: every tool response — success AND failure — carries
    //! the protocol_version field. If a new tool is added or a new failure
    //! path is introduced without `protocol_version`, this suite catches it.
    //!
    //! This is the regression test for FleetQ #6 (stable response schemas).

    use super::*;
    use serde_json::Value;

    fn parse(json: &str) -> Value {
        serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("response was not valid JSON: {e}\n--- payload ---\n{json}"))
    }

    fn assert_protocol_version(json: &str, label: &str) {
        let v = parse(json);
        assert_eq!(
            v["protocol_version"].as_u64(),
            Some(TOOL_RESPONSE_PROTOCOL_VERSION as u64),
            "{label}: missing or wrong protocol_version. payload: {json}"
        );
    }

    // ── compile / ast ──

    #[test]
    fn compile_success_carries_protocol_version() {
        let out = compile::compile_source("fn main() -> Int { 1 + 2 }\n", "module");
        assert_protocol_version(&out, "compile success");
    }

    #[test]
    fn compile_failure_carries_protocol_version() {
        let out = compile::compile_source("this is not valid .ax", "module");
        assert_protocol_version(&out, "compile failure");
    }

    #[test]
    fn ast_success_carries_protocol_version() {
        let out = compile::parse_ast("fn main() -> Int { 1 }\n");
        assert_protocol_version(&out, "ast success");
    }

    #[test]
    fn ast_failure_carries_protocol_version() {
        let out = compile::parse_ast("@@@ not valid");
        assert_protocol_version(&out, "ast failure");
    }

    // ── symbols ──

    #[test]
    fn symbols_success_carries_protocol_version() {
        let out = symbols::extract_symbols("fn main() -> Int { 1 }\n");
        assert_protocol_version(&out, "symbols success");
    }

    #[test]
    fn symbols_parse_failure_carries_protocol_version() {
        let out = symbols::extract_symbols("@@@ not valid");
        assert_protocol_version(&out, "symbols parse failure");
    }

    // ── run ──

    #[test]
    fn run_success_carries_protocol_version() {
        let out = run::run_source(
            "fn main() -> Int { 1 + 2 }\n",
            None,
            1_000_000,
            false,
            None,
            None,
        );
        assert_protocol_version(&out, "run success");
    }

    #[test]
    fn run_invalid_policy_carries_protocol_version() {
        let bad = serde_json::json!(42);
        let out = run::run_source(
            "fn main() -> Int { 1 }\n",
            Some(&bad),
            1_000_000,
            false,
            None,
            None,
        );
        assert_protocol_version(&out, "run invalid_policy");
    }

    #[test]
    fn run_compile_failure_carries_protocol_version() {
        let out = run::run_source("@@@ not valid", None, 1_000_000, false, None, None);
        assert_protocol_version(&out, "run compile failure");
    }

    // ── run_sealed ──

    #[test]
    fn run_sealed_success_carries_protocol_version() {
        let out = sealed::run_sealed("fn main() -> Int { 1 + 2 }\n", None, 1_000_000);
        assert_protocol_version(&out, "run_sealed success");
    }

    #[test]
    fn run_sealed_compile_failure_carries_protocol_version() {
        let out = sealed::run_sealed("@@@ not valid", None, 1_000_000);
        assert_protocol_version(&out, "run_sealed compile failure");
    }

    #[test]
    fn run_sealed_invalid_policy_carries_protocol_version() {
        let bad = serde_json::json!(42);
        let out = sealed::run_sealed("fn main() -> Int { 1 }\n", Some(&bad), 1_000_000);
        assert_protocol_version(&out, "run_sealed invalid_policy");
    }

    // ── check / repair ──

    #[test]
    fn check_carries_protocol_version() {
        let out = check::check_source("fn main() -> Int { 1 }\n", "<test>");
        assert_protocol_version(&out, "check");
    }

    #[test]
    fn repair_carries_protocol_version() {
        let out = check::repair_source("fn main() -> Int { 1 }\n", "<test>", "best", None);
        assert_protocol_version(&out, "repair");
    }

    // ── framework ──

    #[test]
    fn validate_app_compile_failure_carries_protocol_version() {
        // Compile-failure path returns the compile error JSON shape; ensure it
        // carries the version too.
        let out = framework::validate_app("@@@ not valid");
        assert_protocol_version(&out, "validate_app compile failure");
    }

    // ── workflow ──

    #[test]
    fn workflow_validate_parse_error_carries_protocol_version() {
        let out = workflow::validate_workflow("not json");
        assert_protocol_version(&out, "workflow parse_error");
    }

    #[test]
    fn workflow_validate_validation_error_carries_protocol_version() {
        // A workflow_json that parses as WorkflowDef but fails validation
        // (no steps). schema_version is required as of sprint W4.
        let empty =
            r#"{"schema_version":1,"name":"empty","version":"1.0.0","steps":{},"edges":[]}"#;
        let out = workflow::validate_workflow(empty);
        assert_protocol_version(&out, "workflow validation_error");
    }

    // ── template ──

    #[test]
    fn template_list_failure_carries_protocol_version() {
        // Pointing at a non-existent dir guarantees a failure path.
        let out = template::list_templates("/nonexistent/templates/dir/xyz");
        assert_protocol_version(&out, "template_list failure");
    }

    #[test]
    fn template_apply_invalid_args_carries_protocol_version() {
        let out = template::apply_template(
            "/nonexistent",
            "anything",
            &["missing_equals_sign".to_string()],
            false,
        );
        assert_protocol_version(&out, "template_apply invalid_args");
    }

    #[test]
    fn template_apply_template_error_carries_protocol_version() {
        let out = template::apply_template(
            "/nonexistent/templates/dir/xyz",
            "missing-template",
            &["k=v".to_string()],
            false,
        );
        assert_protocol_version(&out, "template_apply template_error");
    }

    // ── policy_validate (0.4-S15) ──

    #[test]
    fn policy_validate_ok_carries_protocol_version() {
        let out = policy::validate_policy("{}");
        assert_protocol_version(&out, "policy_validate ok");
    }

    #[test]
    fn policy_validate_failure_carries_protocol_version() {
        let out = policy::validate_policy(r#"{"foo": 1}"#);
        assert_protocol_version(&out, "policy_validate failure");
    }

    #[test]
    fn policy_validate_parse_error_carries_protocol_version() {
        let out = policy::validate_policy("{");
        assert_protocol_version(&out, "policy_validate parse_error");
    }

    // ── meta ──

    #[test]
    fn protocol_version_constant_is_one() {
        // Locks the wire-format version. Bump only on a breaking shape change
        // anywhere in the tool envelope. See docs/reference/mcp-server.md.
        assert_eq!(TOOL_RESPONSE_PROTOCOL_VERSION, 1);
    }
}
