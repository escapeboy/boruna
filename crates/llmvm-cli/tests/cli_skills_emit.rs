//! End-to-end checks for `boruna skills emit` and `boruna skills pack`.

use std::process::Command;

fn boruna(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(args)
        .output()
        .expect("run boruna")
}

fn stdout(o: &std::process::Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn cli_skill_lists_commands_that_the_hand_written_text_never_named() {
    let out = boruna(&["skills", "get", "cli"]);
    assert!(out.status.success());
    let body = stdout(&out);
    for cmd in ["simulate", "policy", "migrate", "metrics", "capability"] {
        assert!(
            body.contains(&format!("- `boruna {cmd}")),
            "generated reference misses `{cmd}`"
        );
    }
}

#[test]
fn emit_into_existing_regular_file_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("occupied");
    std::fs::write(&file, "x").unwrap();
    let out = boruna(&["skills", "emit", file.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("failed to emit"));
}

#[test]
fn emit_creates_one_folder_per_skill() {
    let dir = tempfile::tempdir().unwrap();
    let out = boruna(&["skills", "emit", dir.path().to_str().unwrap()]);
    assert!(out.status.success());
    for name in ["ax-language", "cli", "workflows", "diagnostics"] {
        assert!(dir.path().join(name).join("SKILL.md").is_file(), "{name}");
    }
}

#[test]
fn pack_is_deterministic_and_respects_budget() {
    let a = boruna(&[
        "skills",
        "pack",
        "approval gate",
        "--budget",
        "800",
        "--json",
    ]);
    let b = boruna(&[
        "skills",
        "pack",
        "approval gate",
        "--budget",
        "800",
        "--json",
    ]);
    assert!(a.status.success());
    assert_eq!(a.stdout, b.stdout);
    let v: serde_json::Value = serde_json::from_slice(&a.stdout).unwrap();
    assert!(v["tokens_estimated"].as_u64().unwrap() <= 800);
    assert!(!v["chunks"].as_array().unwrap().is_empty());
}

#[test]
fn pack_without_match_exits_1_and_names_the_query() {
    let out = boruna(&["skills", "pack", "zzzqqq"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("zzzqqq"));
}
