//! Integration test: `boruna evidence inspect` shows step output content
//! for plaintext (non-encrypted) bundles.

use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

/// Build a minimal plaintext bundle directory with:
///   manifest.json  — minimal BundleManifest fields
///   outputs/step_1/result.json — {"value": 42}
fn build_plaintext_bundle(base: &std::path::Path) -> std::path::PathBuf {
    let bundle_dir = base.join("run-plaintext-inspect");
    fs::create_dir_all(&bundle_dir).unwrap();

    // Minimal manifest.json that BundleManifest::deserialize accepts.
    let manifest = serde_json::json!({
        "run_id": "run-plaintext-inspect",
        "workflow_name": "test-workflow",
        "started_at": "2026-01-01T00:00:00Z",
        "completed_at": "2026-01-01T00:00:01Z",
        "bundle_hash": "aaaa",
        "workflow_hash": "bbbb",
        "policy_hash": "cccc",
        "audit_log_hash": "dddd",
        "file_checksums": {},
        "env_fingerprint": {
            "boruna_version": "0.0.0-test",
            "rust_version": "1.0.0",
            "os": "linux",
            "arch": "x86_64",
            "hostname": "test-host"
        }
    });
    fs::write(
        bundle_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // Step output.
    let step_out_dir = bundle_dir.join("outputs").join("step_1");
    fs::create_dir_all(&step_out_dir).unwrap();
    fs::write(step_out_dir.join("result.json"), r#"{"value": 42}"#).unwrap();

    bundle_dir
}

#[test]
fn inspect_shows_step_outputs_for_plaintext_bundle() {
    let dir = tempdir().unwrap();
    let bundle_dir = build_plaintext_bundle(dir.path());

    let output = Command::new(boruna_bin())
        .args(["evidence", "inspect", bundle_dir.to_str().unwrap()])
        .output()
        .expect("inspect failed to spawn");

    assert!(
        output.status.success(),
        "inspect should succeed; status {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("step_1"),
        "stdout should contain the step id 'step_1'; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("42"),
        "stdout should contain the value '42' from result.json; stdout was:\n{stdout}"
    );
    assert!(
        stdout.contains("Step Outputs"),
        "stdout should contain the 'Step Outputs' section header; stdout was:\n{stdout}"
    );
}

#[test]
fn inspect_json_includes_step_outputs_for_plaintext_bundle() {
    let dir = tempdir().unwrap();
    let bundle_dir = build_plaintext_bundle(dir.path());

    let output = Command::new(boruna_bin())
        .args([
            "evidence",
            "inspect",
            bundle_dir.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("inspect --json failed to spawn");

    assert!(
        output.status.success(),
        "inspect --json should succeed; status {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON");

    assert!(
        parsed.get("step_outputs").is_some(),
        "JSON output should have 'step_outputs' key; got:\n{stdout}"
    );
    let step_outputs = &parsed["step_outputs"];
    assert!(
        step_outputs.get("step_1").is_some(),
        "step_outputs should have 'step_1' key; got:\n{step_outputs}"
    );
    assert_eq!(
        step_outputs["step_1"]["value"],
        serde_json::json!(42),
        "step_1 value should be 42"
    );
}
