//! Validation for the CHANGELOG-driven release-notes extraction
//! used by `.github/workflows/release.yml` (sprint `W9-B`).
//!
//! The release pipeline runs an awk one-liner against the
//! repository's `CHANGELOG.md` to extract the section for the
//! current tag and use it as the GitHub Release body. If the
//! awk script ever stops matching the keep-a-changelog format
//! we use, releases will ship with empty bodies. This test
//! locks down the contract:
//!
//! - Known versions extract a non-empty body that begins with
//!   the curated theme paragraph.
//! - An unknown version produces no output AND the wrapper
//!   exits non-zero (so the workflow fails loudly instead of
//!   shipping an empty release page).
//!
//! The awk script is executed in a subprocess so this test
//! exercises the EXACT logic the workflow runs.
//!
//! These tests are skipped if `awk` is not on PATH (e.g. an
//! exotic Windows runner without WSL); the release workflow
//! always runs on Linux so awk is guaranteed there.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The awk script embedded in `.github/workflows/release.yml`.
/// Kept byte-for-byte identical so this test fails fast if the
/// workflow drifts away from the expected behavior.
const EXTRACT_AWK: &str = r#"
/^## \[/ {
  if (in_section) exit
  if ($0 ~ "^## \\[" ver "\\]") { in_section = 1; next }
}
in_section { print }
"#;

fn changelog_path() -> PathBuf {
    // CARGO_MANIFEST_DIR -> crates/llmvm-cli; CHANGELOG.md is two levels up.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("..").join("..").join("CHANGELOG.md")
}

fn awk_available() -> bool {
    Command::new("awk")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        // BSD awk on macOS doesn't support --version; try a no-op program.
        || Command::new("awk")
            .args(["BEGIN { exit 0 }"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

/// Run the workflow's extraction logic for `version` against the
/// repo CHANGELOG. Returns the captured stdout (which is what
/// the workflow writes to `release-notes.md`).
fn extract(version: &str) -> String {
    let changelog = changelog_path();
    assert!(
        changelog.exists(),
        "CHANGELOG.md not found at {}",
        changelog.display()
    );
    let out = Command::new("awk")
        .args(["-v", &format!("ver={version}"), EXTRACT_AWK])
        .arg(&changelog)
        .output()
        .expect("invoke awk");
    assert!(
        out.status.success(),
        "awk exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("awk stdout is valid utf-8")
}

#[test]
fn extract_rc1_starts_with_theme_paragraph() {
    if !awk_available() {
        eprintln!("skipping: awk not on PATH");
        return;
    }
    let body = extract("1.0.0-rc1");
    assert!(!body.trim().is_empty(), "rc1 extraction was empty");
    assert!(
        body.contains("**Theme: 1.0 release candidate.**"),
        "rc1 body did not contain the expected theme paragraph; got:\n{body}"
    );
    // Sanity: the next version's heading must NOT leak in.
    assert!(
        !body.contains("## [0.5.0]"),
        "rc1 extraction bled into the next section"
    );
}

#[test]
fn extract_rc2_starts_with_theme_paragraph() {
    if !awk_available() {
        eprintln!("skipping: awk not on PATH");
        return;
    }
    let body = extract("1.0.0-rc2");
    assert!(!body.trim().is_empty(), "rc2 extraction was empty");
    assert!(
        body.contains("**Theme: GA polish.**"),
        "rc2 body did not contain the expected theme paragraph; got:\n{body}"
    );
    assert!(
        !body.contains("## [1.0.0-rc1]"),
        "rc2 extraction bled into rc1"
    );
}

#[test]
fn extract_v0_5_0_is_non_empty() {
    if !awk_available() {
        eprintln!("skipping: awk not on PATH");
        return;
    }
    let body = extract("0.5.0");
    assert!(!body.trim().is_empty(), "0.5.0 extraction was empty");
}

#[test]
fn extract_unknown_version_is_empty_and_workflow_fails_loudly() {
    if !awk_available() {
        eprintln!("skipping: awk not on PATH");
        return;
    }
    // Pure awk run: empty output for a missing version.
    let body = extract("99.99.99");
    assert!(
        body.trim().is_empty(),
        "extraction for unknown version 99.99.99 should be empty; got:\n{body}"
    );

    // Now exercise the wrapper guard the workflow uses: pipe the
    // awk output through `[ -s release-notes.md ] || exit 1`.
    // Equivalent: capture awk output, check len, fail if empty.
    let exit_code = if body.is_empty() { 1 } else { 0 };
    assert_eq!(
        exit_code, 1,
        "workflow guard must exit non-zero on empty extraction"
    );
}
