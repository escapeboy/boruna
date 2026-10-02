//! A confidence-gated workflow through the real CLI: persisted pause/approve/resume,
//! auto-approval, evidence bundle creation, verification and tampering.

use std::path::{Path, PathBuf};
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

/// classify (returns `score`) -> gate (confidence gate, threshold 490 at alpha 50) -> after.
fn workflow(root: &Path, score: i64) -> PathBuf {
    let dir = root.join("wf");
    std::fs::create_dir_all(dir.join("steps")).unwrap();
    std::fs::write(
        dir.join("steps/classify.ax"),
        format!("fn main() -> Int {{\n    {score}\n}}\n"),
    )
    .unwrap();
    std::fs::write(dir.join("steps/after.ax"), "fn main() -> Int {\n    1\n}\n").unwrap();
    let ex: Vec<serde_json::Value> = (0..100u32)
        .map(|i| serde_json::json!({"score": i * 10, "correct": i * 10 >= 500}))
        .collect();
    std::fs::write(
        dir.join("calibration.json"),
        serde_json::to_vec(&serde_json::json!({"version": 1, "examples": ex})).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("workflow.json"),
        r#"{
          "schema_version": 1, "name": "conf-demo", "version": "1.0.0",
          "steps": {
            "classify": {"kind": "source", "source": "steps/classify.ax"},
            "gate": {"kind": "approval_gate", "required_role": "reviewer",
                     "depends_on": ["classify"],
                     "confidence_gate": {"source_step": "classify",
                                         "calibration": "calibration.json",
                                         "alpha_permille": 50}},
            "after": {"kind": "source", "source": "steps/after.ax", "depends_on": ["gate"]}
          },
          "edges": [["classify","gate"],["gate","after"]]
        }"#,
    )
    .unwrap();
    dir
}

fn run_id(out: &Output) -> String {
    text(out)
        .lines()
        .find_map(|l| l.trim().strip_prefix("run_id: ").map(str::to_string))
        .expect("run_id line")
}

fn bundle_dir(base: &Path) -> PathBuf {
    std::fs::read_dir(base)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

#[test]
fn auto_approved_run_produces_a_verifiable_bundle_that_detects_tampering() {
    let tmp = tempfile::tempdir().unwrap();
    let wf = workflow(tmp.path(), 800);
    let data = tmp.path().join("data");
    let out = boruna(&[
        "workflow",
        "run",
        p(&wf),
        "--policy",
        "allow-all",
        "--data-dir",
        p(&data),
    ]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("Completed"), "{}", text(&out));
    let rid = run_id(&out);

    let ev = tmp.path().join("ev");
    let out = boruna(&[
        "evidence",
        "create",
        &rid,
        "--data-dir",
        p(&data),
        "--output-dir",
        p(&ev),
    ]);
    assert!(out.status.success(), "{}", text(&out));
    let bundle = bundle_dir(&ev);
    assert!(bundle.join("confidence_gates.json").is_file());
    assert!(bundle.join("confidence/gate.calibration.json").is_file());
    let ok = boruna(&["evidence", "verify", p(&bundle)]);
    assert!(ok.status.success(), "{}", text(&ok));

    // 1. Flip the recorded decision.
    let forged = tmp.path().join("forged");
    copy_dir(&bundle, &forged);
    let gates = forged.join("confidence_gates.json");
    let edited = std::fs::read_to_string(&gates)
        .unwrap()
        .replace("auto_approved", "escalated");
    std::fs::write(&gates, edited).unwrap();
    let bad = boruna(&["evidence", "verify", p(&forged)]);
    assert!(!bad.status.success());
    assert!(
        text(&bad).contains("evidence.confidence_gate"),
        "{}",
        text(&bad)
    );

    // 2. Swap the calibration file.
    let swapped = tmp.path().join("swapped");
    copy_dir(&bundle, &swapped);
    std::fs::write(swapped.join("confidence/gate.calibration.json"), "{}").unwrap();
    assert!(!boruna(&["evidence", "verify", p(&swapped)])
        .status
        .success());

    // 3. Drop confidence_gates.json to hide the gate.
    let dropped = tmp.path().join("dropped");
    copy_dir(&bundle, &dropped);
    std::fs::remove_file(dropped.join("confidence_gates.json")).unwrap();
    let bad = boruna(&["evidence", "verify", p(&dropped)]);
    assert!(!bad.status.success());
}

#[test]
fn escalated_run_waits_for_a_human_and_resume_does_not_record_it_twice() {
    let tmp = tempfile::tempdir().unwrap();
    let wf = workflow(tmp.path(), 100);
    let data = tmp.path().join("data");
    let out = boruna(&[
        "workflow",
        "run",
        p(&wf),
        "--policy",
        "allow-all",
        "--data-dir",
        p(&data),
    ]);
    assert!(text(&out).contains("Paused"), "{}", text(&out));
    assert!(text(&out).contains("AwaitingApproval"), "{}", text(&out));
    let rid = run_id(&out);

    let out = boruna(&["workflow", "approve", &rid, "gate", "--data-dir", p(&data)]);
    assert!(out.status.success(), "{}", text(&out));
    let out = boruna(&[
        "workflow",
        "resume",
        &rid,
        "--data-dir",
        p(&data),
        "--workflow-dir",
        p(&wf),
        "--policy",
        "allow-all",
    ]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("'after': Completed"), "{}", text(&out));

    let ev = tmp.path().join("ev");
    let out = boruna(&[
        "evidence",
        "create",
        &rid,
        "--data-dir",
        p(&data),
        "--output-dir",
        p(&ev),
    ]);
    assert!(out.status.success(), "{}", text(&out));
    let bundle = bundle_dir(&ev);
    let gates: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("confidence_gates.json")).unwrap())
            .unwrap();
    assert_eq!(gates.as_array().unwrap().len(), 1);
    assert_eq!(gates[0]["decision"], "escalated");
    assert!(boruna(&["evidence", "verify", p(&bundle)]).status.success());
}

#[test]
fn submit_only_refuses_confidence_gates() {
    let tmp = tempfile::tempdir().unwrap();
    let wf = workflow(tmp.path(), 800);
    let data = tmp.path().join("data");
    let out = boruna(&[
        "workflow",
        "run",
        p(&wf),
        "--policy",
        "allow-all",
        "--data-dir",
        p(&data),
        "--submit-only",
    ]);
    assert!(!out.status.success());
    assert!(text(&out).contains("confidence_gate"), "{}", text(&out));
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
}
