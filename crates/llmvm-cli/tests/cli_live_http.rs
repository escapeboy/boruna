//! A CLI built with `http` must use the real HTTP handler for `--live` in both `run` and
//! `workflow run`. `workflow run` used to fall back to the mock because the feature was not
//! forwarded to the orchestrator. `cargo test -p boruna-cli --features http --test cli_live_http`.
#![cfg(feature = "http")]

use std::process::Command;

const FALLBACK: &str = "requires the `http` feature";

#[test]
fn workflow_run_live_does_not_fall_back_to_the_mock() {
    let tmp = tempfile::tempdir().unwrap();
    let wf = tmp.path().join("wf");
    std::fs::create_dir_all(wf.join("steps")).unwrap();
    std::fs::write(wf.join("steps/a.ax"), "fn main() -> Int {\n    1\n}\n").unwrap();
    std::fs::write(
        wf.join("workflow.json"),
        r#"{"schema_version":1,"name":"live","version":"1.0.0","steps":{"a":{"kind":"source","source":"steps/a.ax"}},"edges":[]}"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(["workflow", "run"])
        .arg(&wf)
        .args(["--policy", "allow-all", "--live", "--data-dir"])
        .arg(tmp.path().join("data"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains(FALLBACK), "workflow run --live used the mock: {stderr}");
}

#[test]
fn run_live_does_not_fall_back_to_the_mock() {
    let tmp = tempfile::tempdir().unwrap();
    let prog = tmp.path().join("a.ax");
    std::fs::write(&prog, "fn main() -> Int {\n    1\n}\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(["run"])
        .arg(&prog)
        .arg("--live")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains(FALLBACK), "run --live used the mock: {stderr}");
}
