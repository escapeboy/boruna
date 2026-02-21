//! Standard library integration: compile, run, and verify std-* libraries.

use std::path::Path;

/// Compile and run a .llm library source, returning the main() result.
pub fn run_library(source: &str) -> Result<i64, String> {
    let module = boruna_compiler::compile("stdlib_test", source)
        .map_err(|e| format!("compile error: {e}"))?;
    let policy = boruna_vm::capability_gateway::Policy::default();
    let gateway = boruna_vm::capability_gateway::CapabilityGateway::new(policy);
    let mut vm = boruna_vm::Vm::new(module, gateway);
    let result = vm.run()
        .map_err(|e| format!("runtime error: {e}"))?;
    match result {
        boruna_bytecode::Value::Int(n) => Ok(n),
        other => Err(format!("expected Int, got {other}")),
    }
}

/// Compile a library source and verify it produces no errors.
pub fn verify_compiles(source: &str) -> Result<(), String> {
    boruna_compiler::compile("verify", source)
        .map_err(|e| format!("compile error: {e}"))?;
    Ok(())
}

/// Run a library source twice and verify deterministic output.
pub fn verify_determinism(source: &str) -> Result<(), String> {
    let r1 = run_library(source)?;
    let r2 = run_library(source)?;
    if r1 != r2 {
        return Err(format!("nondeterminism: {r1} != {r2}"));
    }
    Ok(())
}

/// Run a framework app through the test harness with messages.
pub fn run_framework_app(
    source: &str,
    messages: &[(&str, &str)],
) -> Result<Vec<boruna_framework::runtime::CycleRecord>, String> {
    let mut harness = boruna_framework::testing::TestHarness::from_source(source)
        .map_err(|e| format!("harness error: {e}"))?;
    for (tag, payload) in messages {
        let msg = boruna_framework::runtime::AppMessage::new(*tag, boruna_bytecode::Value::String(payload.to_string()));
        harness.send(msg).map_err(|e| format!("send: {e}"))?;
    }
    Ok(harness.cycle_log().to_vec())
}

/// Load a library from the libs/ directory.
pub fn load_library_source(libs_dir: &Path, lib_name: &str) -> Result<String, String> {
    let source_path = libs_dir.join(lib_name).join("src/core.ax");
    std::fs::read_to_string(&source_path)
        .map_err(|e| format!("read {}: {e}", source_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn libs_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libs")
    }

    // ── Compile Tests ──

    #[test]
    fn test_std_ui_compiles() {
        let src = load_library_source(&libs_dir(), "std-ui").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_validation_compiles() {
        let src = load_library_source(&libs_dir(), "std-validation").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_forms_compiles() {
        let src = load_library_source(&libs_dir(), "std-forms").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_authz_compiles() {
        let src = load_library_source(&libs_dir(), "std-authz").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_http_compiles() {
        let src = load_library_source(&libs_dir(), "std-http").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_db_compiles() {
        let src = load_library_source(&libs_dir(), "std-db").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_sync_compiles() {
        let src = load_library_source(&libs_dir(), "std-sync").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_routing_compiles() {
        let src = load_library_source(&libs_dir(), "std-routing").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_storage_compiles() {
        let src = load_library_source(&libs_dir(), "std-storage").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_notifications_compiles() {
        let src = load_library_source(&libs_dir(), "std-notifications").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    #[test]
    fn test_std_testing_compiles() {
        let src = load_library_source(&libs_dir(), "std-testing").unwrap();
        assert!(verify_compiles(&src).is_ok());
    }

    // ── Run Tests ──

    #[test]
    fn test_std_ui_runs() {
        let src = load_library_source(&libs_dir(), "std-ui").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_std_validation_runs() {
        let src = load_library_source(&libs_dir(), "std-validation").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 0); // validate_required("name", "") -> valid=0
    }

    #[test]
    fn test_std_forms_runs() {
        let src = load_library_source(&libs_dir(), "std-forms").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 1); // submitted=1
    }

    #[test]
    fn test_std_authz_runs() {
        let src = load_library_source(&libs_dir(), "std-authz").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 1); // admin can delete
    }

    #[test]
    fn test_std_http_runs() {
        let src = load_library_source(&libs_dir(), "std-http").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_std_db_runs() {
        let src = load_library_source(&libs_dir(), "std-db").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 1); // pagination_has_next
    }

    #[test]
    fn test_std_sync_runs() {
        let src = load_library_source(&libs_dir(), "std-sync").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 2); // pending_count after offline edits
    }

    #[test]
    fn test_std_routing_runs() {
        let src = load_library_source(&libs_dir(), "std-routing").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 1); // matched /users
    }

    #[test]
    fn test_std_storage_runs() {
        let src = load_library_source(&libs_dir(), "std-storage").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 2); // version after bump
    }

    #[test]
    fn test_std_notifications_runs() {
        let src = load_library_source(&libs_dir(), "std-notifications").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 1); // count after push+push+dismiss
    }

    #[test]
    fn test_std_testing_runs() {
        let src = load_library_source(&libs_dir(), "std-testing").unwrap();
        let result = run_library(&src).unwrap();
        assert_eq!(result, 3); // 3 passed
    }

    // ── Determinism Tests ──

    #[test]
    fn test_std_ui_determinism() {
        let src = load_library_source(&libs_dir(), "std-ui").unwrap();
        assert!(verify_determinism(&src).is_ok());
    }

    #[test]
    fn test_std_forms_determinism() {
        let src = load_library_source(&libs_dir(), "std-forms").unwrap();
        assert!(verify_determinism(&src).is_ok());
    }

    #[test]
    fn test_std_authz_determinism() {
        let src = load_library_source(&libs_dir(), "std-authz").unwrap();
        assert!(verify_determinism(&src).is_ok());
    }

    #[test]
    fn test_std_sync_determinism() {
        let src = load_library_source(&libs_dir(), "std-sync").unwrap();
        assert!(verify_determinism(&src).is_ok());
    }

    #[test]
    fn test_std_routing_determinism() {
        let src = load_library_source(&libs_dir(), "std-routing").unwrap();
        assert!(verify_determinism(&src).is_ok());
    }

    #[test]
    fn test_std_db_determinism() {
        let src = load_library_source(&libs_dir(), "std-db").unwrap();
        assert!(verify_determinism(&src).is_ok());
    }

    #[test]
    fn test_std_notifications_determinism() {
        let src = load_library_source(&libs_dir(), "std-notifications").unwrap();
        assert!(verify_determinism(&src).is_ok());
    }

    #[test]
    fn test_std_testing_determinism() {
        let src = load_library_source(&libs_dir(), "std-testing").unwrap();
        assert!(verify_determinism(&src).is_ok());
    }
}
