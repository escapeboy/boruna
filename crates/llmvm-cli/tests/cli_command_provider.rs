//! `--live --providers` with a `command` provider: `llm_call` runs a local program, prompt on
//! stdin, reply on stdout. A small shell script stands in for a CLI such as `claude -p`. It
//! counts its runs in a file, so the tests can tell whether it was started. Unix only (the
//! stand-in is a shell script); works with or without the `http` feature.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn boruna(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(args)
        .output()
        .expect("run boruna")
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A fake CLI: appends a line to `runs` per call and answers `model=<argv[1]> prompt=<stdin>`.
fn fake_cli(dir: &Path) -> (PathBuf, PathBuf) {
    let runs = dir.join("runs");
    let script = dir.join("fake-cli");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho run >> '{}'\nprintf 'model=%s prompt=%s' \"$1\" \"$(cat)\"\n",
            runs.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, runs)
}

fn runs(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |s| s.lines().count())
}

fn setup(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let (script, runs) = fake_cli(dir);
    let providers = dir.join("providers.json");
    std::fs::write(
        &providers,
        format!(
            r#"{{"providers":{{"cli":{{"kind":"command","command":["{}","{{model}}"],"timeout_ms":20000}}}}}}"#,
            script.display()
        ),
    )
    .unwrap();
    let program = dir.join("ask.ax");
    std::fs::write(
        &program,
        "fn main() -> String !{llm.call} {\n    llm_call(\"ping\", \"cli/small\")\n}\n",
    )
    .unwrap();
    (providers, program, runs)
}

// I1
#[test]
fn live_run_calls_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, program, runs_file) = setup(dir.path());
    let out = boruna(&["run", p(&program), "--live", "--providers", p(&providers)]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("model=small prompt=ping"), "{stdout}");
    assert_eq!(runs(&runs_file), 1);
}

// I2
#[test]
fn without_live_the_program_is_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, program, runs_file) = setup(dir.path());
    let out = boruna(&["run", p(&program), "--providers", p(&providers)]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("mock"));
    assert_eq!(runs(&runs_file), 0);
}

// I3: a recorded run replays from the log without starting the program again.
#[test]
fn recorded_reply_replays_without_running_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, program, runs_file) = setup(dir.path());
    let log = dir.path().join("events.json");
    let out = boruna(&[
        "run",
        p(&program),
        "--live",
        "--providers",
        p(&providers),
        "--record",
        p(&log),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(runs(&runs_file), 1);
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("model=small prompt=ping"));

    let out = boruna(&["compile", p(&program)]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytecode = dir.path().join("ask.axbc");
    let out = boruna(&["replay", p(&bytecode), p(&log)]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(runs(&runs_file), 1, "replay must not start the program");
}

// I4: the policy still decides; a denied llm.call never starts the program.
#[test]
fn denied_llm_call_does_not_run_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let (providers, program, runs_file) = setup(dir.path());
    let out = boruna(&[
        "run",
        p(&program),
        "--live",
        "--providers",
        p(&providers),
        "--policy",
        "deny-all",
    ]);
    assert!(!out.status.success());
    assert_eq!(runs(&runs_file), 0);
}

#[test]
fn a_failing_program_fails_the_call_with_its_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let providers = dir.path().join("providers.json");
    std::fs::write(
        &providers,
        r#"{"providers":{"cli":{"kind":"command","command":["sh","-c","echo 'please log in' >&2; exit 1"]}}}"#,
    )
    .unwrap();
    let program = dir.path().join("ask.ax");
    std::fs::write(
        &program,
        "fn main() -> String !{llm.call} {\n    llm_call(\"ping\", \"cli/m\")\n}\n",
    )
    .unwrap();
    let out = boruna(&["run", p(&program), "--live", "--providers", p(&providers)]);
    assert!(!out.status.success());
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        all.contains("please log in") && all.contains("code 1"),
        "{all}"
    );
}

// The CLI restores the default SIGPIPE action; a program that exits without reading a large
// prompt must not kill boruna (exit 141). Runs the real binary, unlike the unit test.
#[test]
fn program_that_ignores_a_large_prompt_does_not_kill_boruna() {
    let dir = tempfile::tempdir().unwrap();
    let providers = dir.path().join("providers.json");
    std::fs::write(
        &providers,
        r#"{"providers":{"q":{"kind":"command","command":["sh","-c","echo early"]}}}"#,
    )
    .unwrap();
    let program = dir.path().join("big.ax");
    std::fs::write(
        &program,
        "fn big(n: Int) -> String {\n    let mut s = \"0123456789abcdef\"\n    let mut i = 0\n    while i < n {\n        s = s + s\n        i = i + 1\n    }\n    s\n}\nfn main() -> String !{llm.call} {\n    llm_call(big(18), \"q/m\")\n}\n",
    )
    .unwrap();
    let out = boruna(&["run", p(&program), "--live", "--providers", p(&providers)]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("early"));
}
