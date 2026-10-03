//! End-to-end checks for `boruna confidence threshold`.

use std::process::Command;

fn boruna(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(args)
        .output()
        .expect("run boruna")
}

/// 100 examples, scores 0,10,..990; wrong below 500.
fn calibration(dir: &std::path::Path) -> std::path::PathBuf {
    let ex: Vec<serde_json::Value> = (0..100u32)
        .map(|i| serde_json::json!({"score": i * 10, "correct": i * 10 >= 500}))
        .collect();
    let path = dir.join("cal.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"version": 1, "examples": ex})).unwrap(),
    )
    .unwrap();
    path
}

#[test]
fn threshold_and_decision_are_reported_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let cal = calibration(dir.path());
    let out = boruna(&[
        "confidence",
        "threshold",
        cal.to_str().unwrap(),
        "--alpha-permille",
        "50",
        "--score",
        "700",
        "--json",
    ]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["threshold_permille"], 490);
    assert_eq!(v["decision"], "auto_approved");
    assert_eq!(v["wrong_examples"], 50);
}

#[test]
fn score_below_threshold_is_escalated() {
    let dir = tempfile::tempdir().unwrap();
    let cal = calibration(dir.path());
    let out = boruna(&[
        "confidence",
        "threshold",
        cal.to_str().unwrap(),
        "--alpha-permille",
        "50",
        "--score",
        "100",
        "--json",
    ]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["decision"], "escalated");
}

#[test]
fn too_few_wrong_examples_report_never() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("few.json");
    std::fs::write(
        &path,
        r#"{"version":1,"examples":[{"score":900,"correct":true},{"score":10,"correct":false}]}"#,
    )
    .unwrap();
    let out = boruna(&[
        "confidence",
        "threshold",
        path.to_str().unwrap(),
        "--alpha-permille",
        "50",
        "--json",
    ]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["never_auto_approves"], true);
    assert!(v["threshold_permille"].is_null());
    assert_eq!(v["wrong_examples_needed"], 19);
}

#[test]
fn bad_alpha_and_bad_file_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let cal = calibration(dir.path());
    let out = boruna(&[
        "confidence",
        "threshold",
        cal.to_str().unwrap(),
        "--alpha-permille",
        "0",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "not json").unwrap();
    let out = boruna(&[
        "confidence",
        "threshold",
        bad.to_str().unwrap(),
        "--alpha-permille",
        "50",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not valid JSON"));
}
