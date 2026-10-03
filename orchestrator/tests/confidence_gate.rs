//! Confidence-gated approval: the gate completes without a human only when a calibrated
//! score clears the threshold, and every other case pauses exactly as before.

use boruna_bytecode::Value;
use boruna_orchestrator::confidence::Decision;
use boruna_orchestrator::workflow::{
    DataStore, RunOptions, StepKind, StepStatus, WorkflowDef, WorkflowRunError, WorkflowRunner,
    WorkflowStatus,
};
use boruna_vm::capability_gateway::Policy;

/// 100 labelled examples, scores 0,10,..990, wrong below 500. At alpha 50 the threshold is 490.
fn write_calibration(dir: &std::path::Path) {
    let ex: Vec<serde_json::Value> = (0..100u32)
        .map(|i| serde_json::json!({"score": i * 10, "correct": i * 10 >= 500}))
        .collect();
    std::fs::write(
        dir.join("calibration.json"),
        serde_json::to_vec(&serde_json::json!({"version": 1, "examples": ex})).unwrap(),
    )
    .unwrap();
}

/// classify -> gate (confidence_gate on classify) -> after. `classify_body` is the `.ax` body.
fn workflow(classify_ret: &str, classify_body: &str) -> (tempfile::TempDir, WorkflowDef) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("steps")).unwrap();
    std::fs::write(
        dir.path().join("steps/classify.ax"),
        format!("fn main() -> {classify_ret} {{\n    {classify_body}\n}}\n"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("steps/after.ax"),
        "fn main() -> Int {\n    1\n}\n",
    )
    .unwrap();
    write_calibration(dir.path());
    let def = WorkflowDef::from_json(
        r#"{
          "schema_version": 1, "name": "gated", "version": "1.0.0",
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
    (dir, def)
}

fn options(dir: &tempfile::TempDir) -> RunOptions {
    RunOptions {
        policy: Some(Policy::allow_all()),
        workflow_dir: dir.path().to_str().unwrap().to_string(),
        ..RunOptions::default()
    }
}

#[test]
fn high_score_completes_the_gate_and_runs_the_next_step() {
    let (dir, def) = workflow("Int", "800");
    let result = WorkflowRunner::run(&def, &options(&dir)).unwrap();
    assert_eq!(result.status, WorkflowStatus::Completed);
    assert_eq!(result.step_results["gate"].status, StepStatus::Completed);
    assert_eq!(result.step_results["after"].status, StepStatus::Completed);
    assert_eq!(result.confidence_gates.len(), 1);
    let rec = &result.confidence_gates[0].record;
    assert_eq!(rec.decision, Decision::AutoApproved);
    assert_eq!(rec.threshold_permille, 490);
    assert_eq!(rec.score_permille, Some(800));
}

#[test]
fn auto_approved_gate_output_hash_equals_a_human_approval() {
    // A human approval stores an empty map; the auto-approval must hash identically so the
    // two routes produce the same bundle bytes downstream.
    let (dir, def) = workflow("Int", "800");
    let result = WorkflowRunner::run(&def, &options(&dir)).unwrap();
    let human = DataStore::hash_value(&Value::Map(Default::default()));
    assert_eq!(
        result.step_results["gate"].output_hash.as_deref(),
        Some(human.as_str())
    );
}

#[test]
fn score_below_threshold_pauses_for_a_human() {
    let (dir, def) = workflow("Int", "100");
    let result = WorkflowRunner::run(&def, &options(&dir)).unwrap();
    assert_eq!(result.status, WorkflowStatus::Paused);
    assert_eq!(
        result.step_results["gate"].status,
        StepStatus::AwaitingApproval
    );
    assert!(!result.step_results.contains_key("after"));
    assert_eq!(
        result.confidence_gates[0].record.decision,
        Decision::Escalated
    );
}

#[test]
fn score_exactly_at_the_threshold_is_approved() {
    let (dir, def) = workflow("Int", "490");
    let result = WorkflowRunner::run(&def, &options(&dir)).unwrap();
    assert_eq!(result.status, WorkflowStatus::Completed);
}

#[test]
fn a_score_that_is_not_an_int_in_range_pauses() {
    for (ret, body) in [("String", "\"900\""), ("Int", "1001"), ("Int", "0 - 5")] {
        let (dir, def) = workflow(ret, body);
        let result = WorkflowRunner::run(&def, &options(&dir)).unwrap();
        assert_eq!(result.status, WorkflowStatus::Paused, "{ret} {body}");
        let rec = &result.confidence_gates[0].record;
        assert_eq!(rec.score_permille, None, "{ret} {body}");
        assert_eq!(rec.decision, Decision::Escalated);
    }
}

#[test]
fn a_missing_calibration_file_fails_the_run_instead_of_pausing_quietly() {
    let (dir, def) = workflow("Int", "800");
    std::fs::remove_file(dir.path().join("calibration.json")).unwrap();
    let err = WorkflowRunner::run(&def, &options(&dir)).unwrap_err();
    assert!(
        matches!(err, WorkflowRunError::Io(ref m) if m.contains("calibration")),
        "{err:?}"
    );
}

#[test]
fn a_gate_without_confidence_gate_still_always_pauses() {
    let (dir, mut def) = workflow("Int", "1000");
    if let StepKind::ApprovalGate {
        confidence_gate, ..
    } = &mut def.steps.get_mut("gate").unwrap().kind
    {
        *confidence_gate = None;
    }
    let result = WorkflowRunner::run(&def, &options(&dir)).unwrap();
    assert_eq!(result.status, WorkflowStatus::Paused);
    assert!(result.confidence_gates.is_empty());
}

#[test]
fn workflows_without_a_confidence_gate_serialize_exactly_as_before() {
    // Existing workflow hashes must not change: the new field is skipped when absent.
    let (_dir, mut def) = workflow("Int", "800");
    if let StepKind::ApprovalGate {
        confidence_gate, ..
    } = &mut def.steps.get_mut("gate").unwrap().kind
    {
        *confidence_gate = None;
    }
    let json = serde_json::to_string(&def).unwrap();
    assert!(!json.contains("confidence_gate"), "{json}");
}

#[test]
fn gates_evaluated_in_concurrent_waves_behave_the_same() {
    let (dir, def) = workflow("Int", "800");
    let data = tempfile::tempdir().unwrap();
    let mut opts = options(&dir);
    opts.concurrency = 2;
    let result = WorkflowRunner::run_persistent(&def, &opts, data.path()).unwrap();
    assert_eq!(result.status, WorkflowStatus::Completed);
    assert_eq!(result.confidence_gates.len(), 1);
}

#[test]
fn submit_only_is_refused_with_a_clear_error_and_creates_no_run() {
    let (dir, def) = workflow("Int", "800");
    let data = tempfile::tempdir().unwrap();
    let mut opts = options(&dir);
    opts.submit_only = true;
    let err = WorkflowRunner::run_persistent(&def, &opts, data.path()).unwrap_err();
    assert!(
        matches!(err, WorkflowRunError::Validation(ref m) if m.contains("confidence_gate")),
        "{err:?}"
    );
    assert!(!data.path().join("runs.db").exists());
}
