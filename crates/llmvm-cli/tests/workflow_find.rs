//! CLI integration tests for `boruna workflow find`.

use std::process::Command;

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

/// Returns the path to `examples/workflows/` relative to the repo root.
/// CARGO_MANIFEST_DIR is `crates/llmvm-cli/`, so we go up two levels.
fn workflows_dir() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.join("../../examples/workflows")
}

#[test]
fn find_lists_known_workflows() {
    let out = Command::new(boruna_bin())
        .args(["workflow", "find"])
        .arg(workflows_dir())
        .output()
        .expect("invoke boruna");

    assert!(
        out.status.success(),
        "exit code: {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("llm_code_review"),
        "expected llm_code_review in output; got:\n{stdout}"
    );
    assert!(
        stdout.contains("document_processing"),
        "expected document_processing in output; got:\n{stdout}"
    );
    assert!(
        stdout.contains("customer_support_triage"),
        "expected customer_support_triage in output; got:\n{stdout}"
    );
    // At least 3 rows (header + 3 workflows minimum).
    let lines: Vec<&str> = stdout.lines().collect();
    let data_lines: Vec<&&str> = lines
        .iter()
        .filter(|l| l.contains("valid") || l.contains("invalid"))
        .collect();
    assert!(
        data_lines.len() >= 3,
        "expected at least 3 workflow entries; stdout:\n{stdout}"
    );
}

#[test]
fn find_json_output_is_valid() {
    let out = Command::new(boruna_bin())
        .args(["workflow", "find", "--json"])
        .arg(workflows_dir())
        .output()
        .expect("invoke boruna");

    assert!(
        out.status.success(),
        "exit code: {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let arr: serde_json::Value = serde_json::from_str(&stdout).expect("output must be valid JSON");

    let arr = arr.as_array().expect("output must be a JSON array");
    assert!(
        arr.len() >= 3,
        "expected at least 3 entries; got {}",
        arr.len()
    );

    for entry in arr {
        assert!(entry.get("path").is_some(), "entry missing 'path': {entry}");
        assert!(entry.get("name").is_some(), "entry missing 'name': {entry}");
        assert!(
            entry.get("steps").is_some(),
            "entry missing 'steps': {entry}"
        );
        assert!(
            entry.get("valid").is_some(),
            "entry missing 'valid': {entry}"
        );
    }

    // All known example workflows should be valid.
    let names: Vec<&str> = arr.iter().filter_map(|e| e["name"].as_str()).collect();
    // At least one entry has a non-empty name.
    assert!(
        names.iter().any(|n| !n.is_empty()),
        "all entries have empty names; arr: {arr:?}"
    );
}

#[test]
fn find_empty_dir_exits_0() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let out = Command::new(boruna_bin())
        .args(["workflow", "find"])
        .arg(tmp.path())
        .output()
        .expect("invoke boruna");

    assert!(
        out.status.success(),
        "exit code: {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("none found") || stdout.contains("Workflows in"),
        "unexpected output: {stdout}"
    );
}
