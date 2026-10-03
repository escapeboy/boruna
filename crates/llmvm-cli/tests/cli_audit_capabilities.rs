//! `workflow run --record` bundles record, for every step, the hash of the inputs it ran on
//! (`StepStarted`) and each capability call it made (`CapabilityInvoked`), and still verify.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn boruna(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(args)
        .output()
        .expect("run boruna")
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// upstream returns a string; downstream reads it with `step_input` (a `step.input` call).
fn workflow(root: &Path) -> PathBuf {
    let dir = root.join("wf");
    std::fs::create_dir_all(dir.join("steps")).unwrap();
    std::fs::write(
        dir.join("steps/upstream.ax"),
        "fn main() -> String {\n    \"hello\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("steps/downstream.ax"),
        "fn main() -> String {\n    let got: String = step_input(\"msg\")\n    got\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("workflow.json"),
        r#"{
  "schema_version": 1,
  "name": "audit-capabilities",
  "version": "1.0.0",
  "steps": {
    "upstream": { "kind": "source", "source": "steps/upstream.ax" },
    "downstream": {
      "kind": "source",
      "source": "steps/downstream.ax",
      "inputs": { "msg": "upstream.result" },
      "depends_on": ["upstream"]
    }
  },
  "edges": [["upstream", "downstream"]]
}"#,
    )
    .unwrap();
    dir
}

fn audit_events(bundle: &Path) -> Vec<(String, Value)> {
    let text = std::fs::read_to_string(bundle.join("audit_log.json")).unwrap();
    let log: Value = serde_json::from_str(&text).unwrap();
    let entries = log
        .get("entries")
        .and_then(Value::as_array)
        .or_else(|| log.as_array())
        .expect("audit log entries")
        .clone();
    entries
        .into_iter()
        .map(|e| {
            let event = e.get("event").cloned().unwrap_or(e);
            let (kind, body) = event.as_object().unwrap().iter().next().unwrap();
            (kind.clone(), body.clone())
        })
        .collect()
}

#[test]
fn recorded_bundle_has_step_inputs_and_capability_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let wf = workflow(tmp.path());
    let data = tmp.path().join("data");
    let evidence = tmp.path().join("evidence");
    let run = boruna(&[
        "workflow",
        "run",
        p(&wf),
        "--policy",
        "allow-all",
        "--record",
        "--data-dir",
        p(&data),
        "--evidence-dir",
        p(&evidence),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let bundle = std::fs::read_dir(&evidence)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();

    let events = audit_events(&bundle);
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    for step in ["upstream", "downstream"] {
        assert!(
            events.iter().any(|(k, b)| k == "StepStarted"
                && b["step_id"] == step
                && b["input_hash"].as_str().is_some_and(|h| h.len() == 64)),
            "no StepStarted for {step}: {kinds:?}"
        );
    }
    let calls: Vec<&Value> = events
        .iter()
        .filter(|(k, _)| k == "CapabilityInvoked")
        .map(|(_, b)| b)
        .collect();
    assert_eq!(calls.len(), 1, "{kinds:?}");
    assert_eq!(calls[0]["step_id"], "downstream");
    assert_eq!(calls[0]["capability"], "step.input");
    assert_eq!(calls[0]["allowed"], true);

    let verify = boruna(&["evidence", "verify", p(&bundle)]);
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stdout)
    );
}
