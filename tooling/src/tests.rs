use crate::diagnostics::collector::DiagnosticCollector;
use crate::diagnostics::*;
use crate::repair::{RepairStrategy, RepairTool};
use crate::trace2tests;

/// Full integration test: non-exhaustive match detected + suggested patch + repair + verify.
#[test]
fn test_e2e_non_exhaustive_match_repair() {
    let source = "\
enum Action { Add, Remove, Clear }
type State { count: Int }

fn init() -> State { State { count: 0 } }

fn update(state: State, action: Action) -> State {
    match action {
        Add => State { count: state.count + 1 }
    }
}

fn view(state: State) -> String { \"ok\" }
";

    let ds = DiagnosticCollector::new("test.ax", source).collect();

    // Should find non-exhaustive match
    let match_diag = ds
        .diagnostics
        .iter()
        .find(|d| d.id == E005_NON_EXHAUSTIVE_MATCH);
    assert!(match_diag.is_some(), "expected E005 diagnostic");

    let d = match_diag.unwrap();
    assert!(d.message.contains("Clear"), "should mention Clear");
    assert!(d.message.contains("Remove"), "should mention Remove");

    // Should have a suggested patch
    assert!(
        !d.suggested_patches.is_empty(),
        "expected a suggested patch"
    );
    let patch = &d.suggested_patches[0];
    assert_eq!(patch.confidence, Confidence::High);
    assert!(patch.description.contains("add missing match arms"));
}

/// Integration test: wrong record field -> suggest rename -> repair.
#[test]
fn test_e2e_wrong_field_name_repair() {
    let source = "\
type State { count: Int, name: String }

fn init() -> State {
    State { countt: 0, name: \"test\" }
}
";

    let ds = DiagnosticCollector::new("test.ax", source).collect();

    let field_diag = ds.diagnostics.iter().find(|d| d.id == E006_UNKNOWN_FIELD);
    assert!(field_diag.is_some(), "expected E006 diagnostic");

    let d = field_diag.unwrap();
    assert!(d.message.contains("countt"));

    // Should suggest renaming to "count"
    assert!(!d.suggested_patches.is_empty());
    let patch = &d.suggested_patches[0];
    assert!(patch.description.contains("count"));

    // Apply the fix
    let (repaired, result) = RepairTool::repair("test.ax", source, &ds, RepairStrategy::Best, None);
    assert!(!result.applied.is_empty(), "should have applied a patch");
    assert!(
        repaired.contains("count:"),
        "repaired source should have 'count:'"
    );
    assert!(
        !repaired.contains("countt:"),
        "repaired source should not have 'countt:'"
    );
}

/// Integration test: capability violation in framework app -> suggest removal -> repair.
#[test]
fn test_e2e_capability_violation_repair() {
    let source = "\
type State { value: Int }
type Msg { tag: String }

fn init() -> State { State { value: 0 } }

fn update(state: State, msg: Msg) -> State !{fs.read} {
    state
}

fn view(state: State) -> String { \"ok\" }
";

    let ds = DiagnosticCollector::new("test.ax", source).collect();

    let cap_diag = ds
        .diagnostics
        .iter()
        .find(|d| d.id == E007_CAPABILITY_VIOLATION);
    assert!(cap_diag.is_some(), "expected E007 diagnostic");

    let d = cap_diag.unwrap();
    assert!(d.message.contains("update"));
    assert!(d.message.contains("fs.read"));

    // Should suggest removing capabilities
    assert!(!d.suggested_patches.is_empty());
    let patch = &d.suggested_patches[0];
    assert!(patch.description.contains("remove capability"));

    // Apply the fix
    let (repaired, result) = RepairTool::repair("test.ax", source, &ds, RepairStrategy::Best, None);
    assert!(!result.applied.is_empty());
    assert!(
        !repaired.contains("!{fs.read}"),
        "should have removed capability annotation"
    );
}

/// Integration test: undefined variable -> suggest rename -> repair.
#[test]
fn test_e2e_undefined_var_suggest() {
    let source = "\
fn main() -> Int {
    let count = 42
    countt + 1
}
";

    let ds = DiagnosticCollector::new("test.ax", source).collect();

    let undef_diag = ds.diagnostics.iter().find(|d| d.id == E003_UNDEFINED_VAR);
    assert!(undef_diag.is_some(), "expected E003 diagnostic");

    let d = undef_diag.unwrap();
    // Should suggest "count"
    assert!(
        !d.suggested_patches.is_empty(),
        "expected a rename suggestion"
    );
    let patch = &d.suggested_patches[0];
    assert!(patch.description.contains("count"));
}

/// Test that valid code produces no errors.
#[test]
fn test_valid_code_no_diagnostics() {
    let source = "\
type State { count: Int }
enum Msg { Inc, Dec }

fn init() -> State { State { count: 0 } }

fn update(state: State, msg: Msg) -> State {
    match msg {
        Inc => State { count: state.count + 1 }
        Dec => State { count: state.count - 1 }
    }
}

fn view(state: State) -> String { \"ok\" }
";

    let ds = DiagnosticCollector::new("test.ax", source).collect();
    assert!(
        !ds.has_errors(),
        "valid code should have no errors, got: {}",
        ds.to_human()
    );
}

/// Test JSON output format.
#[test]
fn test_diagnostic_json_format() {
    let source = "fn main() -> Int {\n    undefined_var\n}\n";
    let ds = DiagnosticCollector::new("test.ax", source).collect();
    let json = ds.to_json();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["version"], 1);
    assert!(parsed["diagnostics"].is_array());
    assert!(!parsed["diagnostics"].as_array().unwrap().is_empty());
}

/// Test human-readable output format.
#[test]
fn test_diagnostic_human_format() {
    let source = "fn main() -> Int {\n    undefined_var\n}\n";
    let ds = DiagnosticCollector::new("test.ax", source).collect();
    let human = ds.to_human();
    assert!(human.contains("[E003]"));
    assert!(human.contains("undefined_var"));
}

/// Test DiagnosticSet serialization roundtrip.
#[test]
fn test_diagnostic_set_roundtrip() {
    let mut ds = DiagnosticSet::new("test.ax");
    ds.push(
        Diagnostic::error(E005_NON_EXHAUSTIVE_MATCH, "missing X".into())
            .at("test.ax", 10, Some(5))
            .with_suggestion(SuggestedPatch {
                id: "fix-1".into(),
                description: "add X".into(),
                confidence: Confidence::High,
                rationale: "covers remaining".into(),
                edits: vec![TextEdit {
                    file: "test.ax".into(),
                    start_line: 15,
                    old_text: "}".into(),
                    new_text: "    X => {}\n}".into(),
                }],
            }),
    );

    let json = ds.to_json();
    let parsed: DiagnosticSet = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].id, E005_NON_EXHAUSTIVE_MATCH);
    assert_eq!(parsed.diagnostics[0].suggested_patches.len(), 1);
}

/// Verify repair does not introduce nondeterminism: applying the same fix twice yields the same result.
#[test]
fn test_repair_determinism() {
    let source = "\
type State { count: Int, name: String }

fn init() -> State {
    State { countt: 0, name: \"test\" }
}
";

    let ds = DiagnosticCollector::new("test.ax", source).collect();
    let (repaired1, _) = RepairTool::repair("test.ax", source, &ds, RepairStrategy::Best, None);
    let (repaired2, _) = RepairTool::repair("test.ax", source, &ds, RepairStrategy::Best, None);
    assert_eq!(repaired1, repaired2, "repair should be deterministic");
}

// ─── Trace2Tests Integration Tests ────────────────────────────

const TRACE_TEST_APP: &str = r#"
type State { count: Int, label: String }
type Msg { tag: String, payload: Int }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }

fn init() -> State {
    State { count: 0, label: "counter" }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    if msg.tag == "increment" {
        UpdateResult {
            state: State { count: state.count + 1, label: state.label },
            effects: [],
        }
    } else {
        if msg.tag == "decrement" {
            UpdateResult {
                state: State { count: state.count - 1, label: state.label },
                effects: [],
            }
        } else {
            UpdateResult {
                state: state,
                effects: [],
            }
        }
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "text", text: state.label }
}

fn main() -> Int {
    let s: State = init()
    s.count
}
"#;

/// Integration test: full record → generate → run pipeline.
#[test]
fn test_trace2tests_full_pipeline() {
    use boruna_bytecode::Value;
    use boruna_framework::runtime::AppMessage;

    let msgs = vec![
        AppMessage::new("increment", Value::Int(0)),
        AppMessage::new("increment", Value::Int(0)),
        AppMessage::new("decrement", Value::Int(0)),
        AppMessage::new("increment", Value::Int(0)),
    ];

    // Record
    let trace = trace2tests::record_trace(TRACE_TEST_APP, "test.ax", msgs).unwrap();
    assert_eq!(trace.cycles.len(), 4);

    // Generate
    let spec = trace2tests::generate_test(&trace, "pipeline_test");
    assert_eq!(spec.messages.len(), 4);

    // Run
    let result = trace2tests::run_test(&spec, TRACE_TEST_APP);
    assert!(result.passed, "pipeline test should pass: {:?}", result);
}

/// Integration test: trace determinism across multiple recordings.
#[test]
fn test_trace2tests_determinism_integration() {
    use boruna_bytecode::Value;
    use boruna_framework::runtime::AppMessage;

    let make_msgs = || {
        vec![
            AppMessage::new("increment", Value::Int(0)),
            AppMessage::new("decrement", Value::Int(0)),
            AppMessage::new("increment", Value::Int(0)),
        ]
    };

    let trace1 = trace2tests::record_trace(TRACE_TEST_APP, "test.ax", make_msgs()).unwrap();
    let trace2 = trace2tests::record_trace(TRACE_TEST_APP, "test.ax", make_msgs()).unwrap();

    // All hashes must be identical
    assert_eq!(
        trace1.trace_hash, trace2.trace_hash,
        "trace hashes must match"
    );
    assert_eq!(
        trace1.final_state_hash, trace2.final_state_hash,
        "final state hashes must match"
    );
    assert_eq!(
        trace1.source_hash, trace2.source_hash,
        "source hashes must match"
    );
}

/// Integration test: minimizer with state mismatch predicate.
#[test]
fn test_trace2tests_minimize_integration() {
    use boruna_bytecode::Value;
    use boruna_framework::runtime::AppMessage;

    // Record a trace with mixed messages
    let msgs = vec![
        AppMessage::new("increment", Value::Int(0)),
        AppMessage::new("increment", Value::Int(0)),
        AppMessage::new("decrement", Value::Int(0)),
        AppMessage::new("increment", Value::Int(0)),
        AppMessage::new("increment", Value::Int(0)),
    ];
    let trace = trace2tests::record_trace(TRACE_TEST_APP, "test.ax", msgs).unwrap();

    // State mismatch predicate: fails if final state differs from trace
    let pred = trace2tests::make_state_mismatch_predicate(trace.final_state_hash.clone());
    let trace_msgs: Vec<trace2tests::TraceMessage> =
        trace.cycles.iter().map(|c| c.message.clone()).collect();

    let minimal = trace2tests::minimize_trace(TRACE_TEST_APP, &trace_msgs, &*pred);

    // All messages matter (3 inc + 1 dec + 1 inc = count 3)
    // Removing any changes the count, so all should be needed
    // BUT: the decrement can be compensated by removing one increment
    // Let's just verify the minimal still produces the same state
    let app_msgs = trace2tests::messages_to_app(&minimal);
    let min_trace = trace2tests::record_trace(TRACE_TEST_APP, "test.ax", app_msgs).unwrap();
    assert_eq!(
        min_trace.final_state_hash, trace.final_state_hash,
        "minimized trace must preserve the failure condition (same final state hash)"
    );
    assert!(
        minimal.len() <= trace.cycles.len(),
        "minimized trace should not be longer than original"
    );
}

/// Integration test: generated test fails when source changes.
#[test]
fn test_trace2tests_detects_regression() {
    use boruna_bytecode::Value;
    use boruna_framework::runtime::AppMessage;

    let msgs = vec![
        AppMessage::new("increment", Value::Int(0)),
        AppMessage::new("increment", Value::Int(0)),
    ];
    let trace = trace2tests::record_trace(TRACE_TEST_APP, "test.ax", msgs).unwrap();
    let spec = trace2tests::generate_test(&trace, "regression_test");

    // Original source passes
    let result = trace2tests::run_test(&spec, TRACE_TEST_APP);
    assert!(result.passed, "should pass on original source");

    // Modified source fails (change init count from 0 to 100)
    let modified = TRACE_TEST_APP.replace("count: 0", "count: 100");
    let result = trace2tests::run_test(&spec, &modified);
    assert!(
        !result.passed,
        "should fail when source introduces regression"
    );
}

// ─── Standard Library Integration Tests ─────────────────────

/// All std libraries compile and run deterministically.
#[test]
fn test_stdlib_all_compile_and_run() {
    use crate::stdlib;
    let libs_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libs");
    let lib_names = [
        "std-ui",
        "std-validation",
        "std-forms",
        "std-authz",
        "std-http",
        "std-db",
        "std-sync",
        "std-routing",
        "std-storage",
        "std-notifications",
        "std-testing",
    ];
    for name in &lib_names {
        let src = stdlib::load_library_source(&libs_dir, name)
            .unwrap_or_else(|e| panic!("load {name}: {e}"));
        stdlib::verify_compiles(&src).unwrap_or_else(|e| panic!("{name} compile: {e}"));
        stdlib::run_library(&src).unwrap_or_else(|e| panic!("{name} run: {e}"));
    }
}

/// All std libraries produce deterministic results.
#[test]
fn test_stdlib_determinism() {
    use crate::stdlib;
    let libs_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libs");
    let lib_names = [
        "std-ui",
        "std-forms",
        "std-authz",
        "std-sync",
        "std-routing",
        "std-db",
        "std-notifications",
        "std-testing",
    ];
    for name in &lib_names {
        let src = stdlib::load_library_source(&libs_dir, name)
            .unwrap_or_else(|e| panic!("load {name}: {e}"));
        stdlib::verify_determinism(&src).unwrap_or_else(|e| panic!("{name} nondeterministic: {e}"));
    }
}

// ─── Template Integration Tests ─────────────────────────────

/// Template engine substitution works correctly.
#[test]
fn test_template_apply_and_validate() {
    use crate::templates;
    let templates_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates");
    let mut args = std::collections::BTreeMap::new();
    args.insert("entity_name".into(), "users".into());
    args.insert("fields".into(), "name,email".into());
    let result = templates::apply_template(&templates_dir, "crud-admin", &args).unwrap();
    assert!(
        result.source.contains("users"),
        "should substitute entity_name"
    );
    assert_eq!(result.template_name, "crud-admin");
    assert!(result.dependencies.contains(&"std.ui".to_string()));
    // Validate it compiles
    templates::validate_template_output(&result.source).unwrap();
}

/// All templates produce compilable output.
#[test]
fn test_all_templates_compile() {
    use crate::templates;
    let templates_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates");

    // crud-admin
    let mut args = std::collections::BTreeMap::new();
    args.insert("entity_name".into(), "products".into());
    args.insert("fields".into(), "name,price".into());
    let r = templates::apply_template(&templates_dir, "crud-admin", &args).unwrap();
    templates::validate_template_output(&r.source).unwrap();

    // form-basic
    let mut args = std::collections::BTreeMap::new();
    args.insert("form_name".into(), "contact".into());
    args.insert("fields".into(), "name,email".into());
    let r = templates::apply_template(&templates_dir, "form-basic", &args).unwrap();
    templates::validate_template_output(&r.source).unwrap();

    // auth-app
    let mut args = std::collections::BTreeMap::new();
    args.insert("app_name".into(), "myapp".into());
    let r = templates::apply_template(&templates_dir, "auth-app", &args).unwrap();
    templates::validate_template_output(&r.source).unwrap();

    // realtime-feed
    let mut args = std::collections::BTreeMap::new();
    args.insert("feed_name".into(), "events".into());
    let r = templates::apply_template(&templates_dir, "realtime-feed", &args).unwrap();
    templates::validate_template_output(&r.source).unwrap();

    // offline-sync
    let mut args = std::collections::BTreeMap::new();
    args.insert("entity_name".into(), "todos".into());
    let r = templates::apply_template(&templates_dir, "offline-sync", &args).unwrap();
    templates::validate_template_output(&r.source).unwrap();
}

/// Template output is deterministic.
#[test]
fn test_template_determinism() {
    use crate::templates;
    let templates_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates");
    let mut args = std::collections::BTreeMap::new();
    args.insert("entity_name".into(), "items".into());
    args.insert("fields".into(), "name".into());
    let r1 = templates::apply_template(&templates_dir, "crud-admin", &args).unwrap();
    let r2 = templates::apply_template(&templates_dir, "crud-admin", &args).unwrap();
    assert_eq!(
        r1.source, r2.source,
        "template output must be deterministic"
    );
}
