//! `fs_read`, `fs_write`, `time_now` and `random_int` through the real CLI: real under
//! `--live` (files limited by `fs_policy.allowed_roots`), mock values without it, and the
//! live values come back unchanged from `boruna replay`.

use std::path::Path;
use std::process::{Command, Output};

fn boruna(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(args)
        .output()
        .expect("run boruna")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A policy allowing everything, with file access limited to `root`.
fn policy(dir: &Path, root: &Path) -> std::path::PathBuf {
    let f = dir.join("policy.json");
    let json = serde_json::json!({
        "default_allow": true,
        "fs_policy": {"allowed_roots": [root.to_str().unwrap()], "allow_delete": true}
    });
    std::fs::write(&f, json.to_string()).unwrap();
    f
}

const FILES: &str = "fn save(p: String, s: String) -> Bool !{fs.write} {\n    fs_write(p, s)\n}\nfn load(p: String) -> String !{fs.read} {\n    fs_read(p)\n}\nfn main() -> String {\n    let ok: Bool = save(\"PATH\", \"hello from boruna\")\n    load(\"PATH\")\n}\n";

#[test]
fn live_files_round_trip_inside_the_allowed_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let target = root.join("note.txt");
    let prog = tmp.path().join("files.ax");
    std::fs::write(
        &prog,
        FILES.replace("PATH", &p(&target).replace('\\', "\\\\")),
    )
    .unwrap();
    let pol = policy(tmp.path(), &root);

    let out = boruna(&["run", p(&prog), "--policy", p(&pol), "--live"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("hello from boruna"), "{}", text(&out));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "hello from boruna"
    );
}

#[test]
fn live_write_outside_the_allowed_root_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let outside = tmp.path().join("outside.txt");
    let prog = tmp.path().join("files.ax");
    std::fs::write(
        &prog,
        FILES.replace("PATH", &p(&outside).replace('\\', "\\\\")),
    )
    .unwrap();
    let pol = policy(tmp.path(), &root);

    let out = boruna(&["run", p(&prog), "--policy", p(&pol), "--live"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("outside fs_policy.allowed_roots"),
        "{}",
        text(&out)
    );
    assert!(!outside.exists());
}

const CLOCK: &str = "fn stamp() -> Int !{time.now} {\n    time_now()\n}\nfn roll() -> Int !{random} {\n    random_int(1, 1000000)\n}\nfn main() -> Int {\n    stamp() * 0 + roll() + stamp() / 1000000000000\n}\n";

#[test]
fn without_live_the_clock_and_random_are_fixed() {
    let tmp = tempfile::tempdir().unwrap();
    let prog = tmp.path().join("clock.ax");
    std::fs::write(&prog, CLOCK).unwrap();
    let out = boruna(&["run", p(&prog), "--policy", "allow-all"]);
    assert!(out.status.success(), "{}", text(&out));
    // Mock: random_int(1, ..) -> 1, time 1_700_000_000_000 / 10^12 -> 1.
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).lines().next(),
        Some("2")
    );
}

#[test]
fn live_clock_and_random_replay_to_the_same_result() {
    let tmp = tempfile::tempdir().unwrap();
    let prog = tmp.path().join("clock.ax");
    std::fs::write(&prog, CLOCK).unwrap();
    let log = tmp.path().join("log.json");
    let out = boruna(&[
        "run",
        p(&prog),
        "--policy",
        "allow-all",
        "--live",
        "--record",
        p(&log),
    ]);
    assert!(out.status.success(), "{}", text(&out));
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap()
        .to_string();
    let value: i64 = first.parse().unwrap();
    // A real clock read (>= 2026 / 10^12 ms = 1) plus a draw in [1, 1_000_000].
    assert!(value >= 2, "{first}");

    let out = boruna(&["replay", p(&prog), p(&log)]);
    assert!(out.status.success(), "{}", text(&out));
    let t = text(&out);
    assert!(t.contains(&format!("replay result: {first}")), "{t}");
    assert!(t.contains("Identical"), "{t}");
}

#[test]
fn workflow_steps_get_real_files_under_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let target = root.join("report.txt");
    let wf = tmp.path().join("wf");
    std::fs::create_dir_all(wf.join("steps")).unwrap();
    std::fs::write(
        wf.join("steps/write.ax"),
        format!(
            "fn main() -> Bool !{{fs.write}} {{\n    fs_write(\"{}\", \"step output\")\n}}\n",
            p(&target).replace('\\', "\\\\")
        ),
    )
    .unwrap();
    std::fs::write(
        wf.join("workflow.json"),
        r#"{"schema_version":1,"name":"fs-step","version":"1.0.0","steps":{"write":{"kind":"source","source":"steps/write.ax","capabilities":["fs.write"]}},"edges":[]}"#,
    )
    .unwrap();
    let pol = policy(tmp.path(), &root);
    let out = boruna(&["workflow", "run", p(&wf), "--policy", p(&pol), "--live"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("Completed"), "{}", text(&out));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "step output");
}

#[test]
fn live_list_append_and_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let dir = p(&root).replace('\\', "\\\\");
    let prog = tmp.path().join("ops.ax");
    std::fs::write(
        &prog,
        format!(
            "fn add(p: String, s: String) -> Bool !{{fs.write}} {{\n    fs_append(p, s)\n}}\nfn rm(p: String) -> Bool !{{fs.write}} {{\n    fs_delete(p)\n}}\nfn ls(d: String) -> List<String> !{{fs.read}} {{\n    fs_list(d)\n}}\nfn main() -> Int {{\n    let a: Bool = add(\"{dir}/a.txt\", \"one\")\n    let b: Bool = add(\"{dir}/b.txt\", \"two\")\n    let c: Bool = rm(\"{dir}/a.txt\")\n    __builtin_list_len(ls(\"{dir}\"))\n}}\n"
        ),
    )
    .unwrap();
    let pol = policy(tmp.path(), &root);
    let out = boruna(&["run", p(&prog), "--policy", p(&pol), "--live"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).lines().next(),
        Some("1")
    );
    assert!(!root.join("a.txt").exists());
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "two");
}

/// `boruna ... | head` must not panic when the reader closes the pipe early.
#[cfg(unix)]
#[test]
fn a_closed_stdout_pipe_ends_the_process_quietly() {
    use std::process::Stdio;
    let tmp = tempfile::tempdir().unwrap();
    let prog = tmp.path().join("slow.ax");
    // Enough work that the pipe is closed before the result is printed.
    std::fs::write(
        &prog,
        "fn main() -> Int {\n    let mut i: Int = 0\n    while i < 300000 {\n        i = i + 1\n    }\n    i\n}\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(["run", p(&prog), "--max-steps", "100000000"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(!stderr.contains("Broken pipe"), "{stderr}");
}
