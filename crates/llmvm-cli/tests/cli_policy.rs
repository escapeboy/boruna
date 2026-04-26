//! CLI integration tests for `boruna policy {validate, show}`
//! (sprint 0.4-S15). Uses `env!("CARGO_BIN_EXE_boruna")` to invoke
//! the freshly compiled binary.

use std::process::Command;

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

fn fixture(name: &str) -> String {
    format!(
        "{}/tests/fixtures/policies/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    )
}

#[test]
fn policy_validate_ok_minimal() {
    let out = Command::new(boruna_bin())
        .args(["policy", "validate", &fixture("valid_minimal.json")])
        .output()
        .expect("invoke boruna");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("OK"));
}

#[test]
fn policy_validate_ok_full() {
    let out = Command::new(boruna_bin())
        .args(["policy", "validate", &fixture("valid_full.json")])
        .output()
        .expect("invoke boruna");
    assert!(out.status.success());
}

#[test]
fn policy_validate_unknown_field_exits_2() {
    let out = Command::new(boruna_bin())
        .args(["policy", "validate", &fixture("invalid_unknown_field.json")])
        .output()
        .expect("invoke boruna");
    assert_eq!(
        out.status.code(),
        Some(2),
        "should exit 2 on validation error"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("policy.unknown_field"),
        "stderr should contain stable error_kind. got: {stderr}"
    );
}

#[test]
fn policy_validate_invalid_capability_with_hint() {
    let out = Command::new(boruna_bin())
        .args([
            "policy",
            "validate",
            &fixture("invalid_capability_alias.json"),
        ])
        .output()
        .expect("invoke boruna");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("policy.invalid_capability"),
        "got: {stderr}"
    );
    assert!(
        stderr.contains("net.fetch"),
        "should hint at canonical name. got: {stderr}"
    );
}

#[test]
fn policy_validate_unknown_schema_version() {
    let out = Command::new(boruna_bin())
        .args([
            "policy",
            "validate",
            &fixture("invalid_schema_version.json"),
        ])
        .output()
        .expect("invoke boruna");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("policy.unknown_schema_version"),
        "got: {stderr}"
    );
}

#[test]
fn policy_validate_missing_file_exits_1() {
    let out = Command::new(boruna_bin())
        .args(["policy", "validate", "/no/such/policy.json"])
        .output()
        .expect("invoke boruna");
    assert_eq!(out.status.code(), Some(1), "should exit 1 on IO error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("policy.io_error"), "got: {stderr}");
}

#[test]
fn policy_validate_json_format_ok() {
    let out = Command::new(boruna_bin())
        .args([
            "policy",
            "validate",
            "--json",
            &fixture("valid_minimal.json"),
        ])
        .output()
        .expect("invoke boruna");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    assert_eq!(v["ok"], true);
}

#[test]
fn policy_validate_json_format_error() {
    let out = Command::new(boruna_bin())
        .args([
            "policy",
            "validate",
            "--json",
            &fixture("invalid_unknown_field.json"),
        ])
        .output()
        .expect("invoke boruna");
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    assert_eq!(v["ok"], false);
    assert_eq!(v["errors"][0]["error_kind"], "policy.unknown_field");
}

#[test]
fn policy_show_ok() {
    let out = Command::new(boruna_bin())
        .args(["policy", "show", &fixture("valid_full.json")])
        .output()
        .expect("invoke boruna");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Schema version: 1"));
    assert!(stdout.contains("net.fetch"));
    assert!(stdout.contains("Net policy:"));
}

#[test]
fn policy_show_minimal_reports_no_rules() {
    let out = Command::new(boruna_bin())
        .args(["policy", "show", &fixture("valid_minimal.json")])
        .output()
        .expect("invoke boruna");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Default behavior: deny"));
    assert!(stdout.contains("Rules: (none)"));
}

#[test]
fn workflow_run_with_invalid_policy_propagates_kind() {
    // Adversarial-review finding (HIGH): pre-fix, `boruna workflow
    // run --policy <bad>` used a lenient `from_str`, silently
    // swallowing alias capability keys and unknown fields. After
    // 0.4-S15 it goes through the strict validator and surfaces
    // the same `error_kind` as `policy validate` and `boruna run`.
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();

    // Minimal valid workflow.json so the workflow loader doesn't
    // fail before reaching the policy parse step.
    let wf_dir = dir.path().join("wf");
    std::fs::create_dir(&wf_dir).unwrap();
    let step_ax = wf_dir.join("step.ax");
    std::fs::write(&step_ax, "fn main() -> Int { 1 }\n").unwrap();
    std::fs::write(
        wf_dir.join("workflow.json"),
        r#"{
            "schema_version": 1,
            "name": "test",
            "version": "1.0.0",
            "steps": {
                "s1": {
                    "kind": "source",
                    "source": "step.ax",
                    "capabilities": [],
                    "outputs": {"result": "Int"}
                }
            },
            "edges": []
        }"#,
    )
    .unwrap();

    let mut policy_path = dir.path().join("p.json");
    {
        let mut f = std::fs::File::create(&policy_path).unwrap();
        f.write_all(br#"{"foo": 1}"#).unwrap();
    }
    policy_path = policy_path.canonicalize().unwrap();

    let out = Command::new(boruna_bin())
        .args([
            "workflow",
            "run",
            wf_dir.to_str().unwrap(),
            "--policy",
            policy_path.to_str().unwrap(),
            "--ephemeral",
        ])
        .output()
        .expect("invoke boruna");
    assert!(!out.status.success(), "should fail on invalid policy");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("policy.unknown_field"),
        "workflow run must propagate stable error_kind. stderr: {stderr}"
    );
}

#[test]
fn run_with_invalid_policy_propagates_kind() {
    // Regression: validate-vs-run drift. If `boruna policy validate
    // <file>` fails, `boruna run --policy <file>` must fail with the
    // same error_kind because both share the same parser.
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let ax = dir.path().join("hello.ax");
    std::fs::write(&ax, "fn main() -> Int { 1 }\n").unwrap();
    let mut policy_path = dir.path().join("p.json");
    {
        let mut f = std::fs::File::create(&policy_path).unwrap();
        f.write_all(br#"{ "rules": { "net": { "allow": true, "budget": 0 } } }"#)
            .unwrap();
    }
    policy_path = policy_path.canonicalize().unwrap();

    let out = Command::new(boruna_bin())
        .args([
            "run",
            ax.to_str().unwrap(),
            "--policy",
            policy_path.to_str().unwrap(),
        ])
        .output()
        .expect("invoke boruna");
    assert!(!out.status.success(), "should fail on invalid policy");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("policy.invalid_capability"),
        "should propagate stable error_kind from validator. stderr: {stderr}"
    );
}
