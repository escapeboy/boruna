//! Runtime side of `ConfidenceGate`: read the calibration file and the score, decide, and
//! turn the decision into an audit event and evidence-bundle records.
//!
//! The pure maths lives in [`crate::confidence`]. This module only does the I/O and wiring,
//! so every execution path (ephemeral, persisted, concurrent waves) calls the same code.

use boruna_bytecode::Value;

use crate::audit::{AuditEntry, AuditEvent};
use crate::confidence::{
    calibration_sha256, decide, risk_threshold, CalibrationSet, GateRecord, MAX_SCORE,
};
use crate::workflow::definition::{ConfidenceGate, GateEvaluation};
use crate::workflow::runner::WorkflowRunError;
use crate::workflow::validator::is_safe_relative_path;

/// `rule` value of the `PolicyEvaluated` audit event that carries a gate decision.
pub const AUDIT_RULE: &str = "confidence_gate";

/// Calibration files above this size are rejected. A labelled set of 100k examples is far
/// more than a gate needs and is well under it.
const MAX_CALIBRATION_BYTES: u64 = 4 * 1024 * 1024;

/// The score a source step reported, if its `result` is an `Int` in `0..=1000`.
pub fn score_from_value(v: &Value) -> Option<u32> {
    match v {
        Value::Int(i) if (0..=i64::from(MAX_SCORE)).contains(i) => Some(*i as u32),
        _ => None,
    }
}

/// Evaluate one gate. `source_result` is the source step's `result` output; an `Err` (the
/// output is missing) is treated as "no score" and escalates to a human.
///
/// A calibration file that is missing, unreadable or invalid is a configuration error and
/// fails the run rather than silently pausing, so a typo cannot hide a disabled gate.
pub fn evaluate(
    step_id: &str,
    gate: &ConfidenceGate,
    workflow_dir: &str,
    source_result: Result<Value, String>,
) -> Result<GateEvaluation, WorkflowRunError> {
    if !is_safe_relative_path(&gate.calibration) {
        return Err(WorkflowRunError::Validation(format!(
            "gate '{step_id}': calibration path '{}' must be relative without '..'",
            gate.calibration
        )));
    }
    let path = std::path::Path::new(workflow_dir).join(&gate.calibration);
    let meta = std::fs::metadata(&path).map_err(|e| {
        WorkflowRunError::Io(format!(
            "gate '{step_id}': cannot read calibration '{}': {e}",
            path.display()
        ))
    })?;
    if meta.len() > MAX_CALIBRATION_BYTES {
        return Err(WorkflowRunError::Validation(format!(
            "gate '{step_id}': calibration '{}' is {} bytes, over the {} byte cap",
            path.display(),
            meta.len(),
            MAX_CALIBRATION_BYTES
        )));
    }
    let raw = std::fs::read(&path).map_err(|e| {
        WorkflowRunError::Io(format!(
            "gate '{step_id}': cannot read calibration '{}': {e}",
            path.display()
        ))
    })?;
    let text = std::str::from_utf8(&raw).map_err(|e| {
        WorkflowRunError::Validation(format!("gate '{step_id}': calibration is not UTF-8: {e}"))
    })?;
    let set = CalibrationSet::from_json(text)
        .map_err(|e| WorkflowRunError::Validation(format!("gate '{step_id}': {e}")))?;
    let threshold = risk_threshold(&set, gate.alpha_permille)
        .map_err(|e| WorkflowRunError::Validation(format!("gate '{step_id}': {e}")))?;
    let score = source_result.ok().as_ref().and_then(score_from_value);

    Ok(GateEvaluation {
        record: GateRecord {
            step_id: step_id.to_string(),
            source_step: gate.source_step.clone(),
            alpha_permille: gate.alpha_permille,
            calibration_sha256: calibration_sha256(&raw),
            calibration_examples: set.examples.len(),
            threshold_permille: threshold,
            score_permille: score,
            decision: decide(threshold, score),
        },
        calibration: text.to_string(),
    })
}

/// The audit event that records a gate decision. Uses the existing `PolicyEvaluated`
/// variant so older readers still parse the log; the decision field holds the full
/// [`GateRecord`] as compact JSON, so the hash chain commits to it.
pub fn audit_event(eval: &GateEvaluation) -> AuditEvent {
    AuditEvent::PolicyEvaluated {
        step_id: eval.record.step_id.clone(),
        rule: AUDIT_RULE.to_string(),
        decision: serde_json::to_string(&eval.record)
            .expect("GateRecord serializes: plain data, no maps with non-string keys"),
    }
}

/// Gate records found in an audit log, in log order. Entries that were redacted or whose
/// decision does not parse are skipped here; `evidence verify` reports them separately.
pub fn records_from_audit(entries: &[AuditEntry]) -> Vec<GateRecord> {
    entries
        .iter()
        .filter(|e| e.redacted.is_none())
        .filter_map(|e| match &e.event {
            AuditEvent::PolicyEvaluated { rule, decision, .. } if rule == AUDIT_RULE => {
                serde_json::from_str::<GateRecord>(decision).ok()
            }
            _ => None,
        })
        .collect()
}

/// Rebuild the gate evaluations of a persisted run: the decisions come from the audit
/// chain, the calibration text from the run metadata. Fails when a decision has no stored
/// calibration, because a bundle without it could not be verified.
pub fn evaluations_from_run(
    entries: &[AuditEntry],
    calibrations: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<GateEvaluation>, String> {
    records_from_audit(entries)
        .into_iter()
        .map(|record| {
            let calibration = calibrations.get(&record.step_id).cloned().ok_or_else(|| {
                format!(
                    "gate '{}': calibration text is missing from the run metadata",
                    record.step_id
                )
            })?;
            Ok(GateEvaluation {
                record,
                calibration,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::Decision;

    fn gate(cal: &str) -> ConfidenceGate {
        ConfidenceGate {
            source_step: "classify".into(),
            calibration: cal.into(),
            alpha_permille: 50,
        }
    }

    fn write_calibration(dir: &std::path::Path) {
        let ex: Vec<serde_json::Value> = (0..100u32)
            .map(|i| serde_json::json!({"score": i * 10, "correct": i * 10 >= 500}))
            .collect();
        std::fs::write(
            dir.join("cal.json"),
            serde_json::to_vec(&serde_json::json!({"version": 1, "examples": ex})).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn score_must_be_an_int_in_range() {
        assert_eq!(score_from_value(&Value::Int(0)), Some(0));
        assert_eq!(score_from_value(&Value::Int(1000)), Some(1000));
        assert_eq!(score_from_value(&Value::Int(1001)), None);
        assert_eq!(score_from_value(&Value::Int(-1)), None);
        assert_eq!(score_from_value(&Value::Float(0.9)), None);
        assert_eq!(score_from_value(&Value::String("900".into())), None);
    }

    #[test]
    fn high_score_auto_approves_and_low_or_missing_escalates() {
        let dir = tempfile::tempdir().unwrap();
        write_calibration(dir.path());
        let wd = dir.path().to_str().unwrap();
        let hi = evaluate("g", &gate("cal.json"), wd, Ok(Value::Int(800))).unwrap();
        assert_eq!(hi.record.decision, Decision::AutoApproved);
        assert_eq!(hi.record.threshold_permille, 490);
        let lo = evaluate("g", &gate("cal.json"), wd, Ok(Value::Int(100))).unwrap();
        assert_eq!(lo.record.decision, Decision::Escalated);
        let missing = evaluate("g", &gate("cal.json"), wd, Err("no output".into())).unwrap();
        assert_eq!(missing.record.score_permille, None);
        assert_eq!(missing.record.decision, Decision::Escalated);
        let wrong_type = evaluate("g", &gate("cal.json"), wd, Ok(Value::Float(0.99))).unwrap();
        assert_eq!(wrong_type.record.decision, Decision::Escalated);
    }

    #[test]
    fn bad_calibration_is_an_error_not_a_silent_pause() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path().to_str().unwrap();
        assert!(evaluate("g", &gate("missing.json"), wd, Ok(Value::Int(900))).is_err());
        std::fs::write(dir.path().join("bad.json"), "nope").unwrap();
        assert!(evaluate("g", &gate("bad.json"), wd, Ok(Value::Int(900))).is_err());
        assert!(evaluate("g", &gate("../escape.json"), wd, Ok(Value::Int(900))).is_err());
        assert!(evaluate("g", &gate("/etc/passwd"), wd, Ok(Value::Int(900))).is_err());
    }

    #[test]
    fn decision_round_trips_through_the_audit_event() {
        let dir = tempfile::tempdir().unwrap();
        write_calibration(dir.path());
        let eval = evaluate(
            "g",
            &gate("cal.json"),
            dir.path().to_str().unwrap(),
            Ok(Value::Int(800)),
        )
        .unwrap();
        let mut log = crate::audit::AuditLog::new();
        log.append(audit_event(&eval));
        let recs = records_from_audit(log.entries());
        assert_eq!(recs, vec![eval.record.clone()]);
        // Embedded text verifies against the record.
        let set = CalibrationSet::from_json(&eval.calibration).unwrap();
        assert_eq!(
            eval.record.verify(&set, eval.calibration.as_bytes()),
            Ok(())
        );
    }
}
