//! CLI integration tests for `boruna fmt`.
//!
//! Validates the three behaviors that matter for CI integration:
//!
//! 1. `boruna fmt --check <unformatted>` exits 1.
//! 2. `boruna fmt <unformatted>` rewrites the file in place.
//! 3. After in-place rewrite, `boruna fmt --check <file>` exits 0.

use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

const UNFORMATTED: &str = "fn main() -> Int {\nlet x: Int = 1\nx\n}\n";

#[test]
fn fmt_check_exits_1_on_unformatted() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("u.ax");
    fs::write(&path, UNFORMATTED).unwrap();

    let out = Command::new(boruna_bin())
        .args(["fmt", "--check"])
        .arg(&path)
        .output()
        .expect("invoke boruna");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not formatted"), "stderr was: {stderr}");

    // The file must not have been modified by --check.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, UNFORMATTED);
}

#[test]
fn fmt_rewrites_in_place_and_check_then_passes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("u.ax");
    fs::write(&path, UNFORMATTED).unwrap();

    // Rewrite in place.
    let out = Command::new(boruna_bin())
        .arg("fmt")
        .arg(&path)
        .output()
        .expect("invoke boruna");
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let after = fs::read_to_string(&path).unwrap();
    assert_ne!(after, UNFORMATTED, "file should have been rewritten");
    assert!(after.contains("    let x: Int = 1"), "got: {after}");

    // Subsequent --check must exit 0 cleanly.
    let check = Command::new(boruna_bin())
        .args(["fmt", "--check"])
        .arg(&path)
        .output()
        .expect("invoke boruna");
    assert!(
        check.status.success(),
        "after fmt, --check must succeed; stderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn fmt_check_exits_2_on_parse_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.ax");
    fs::write(&path, "fn main( -> Int { 0 }").unwrap();

    let out = Command::new(boruna_bin())
        .args(["fmt", "--check"])
        .arg(&path)
        .output()
        .expect("invoke boruna");
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit code 2 (parse error). stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
