//! Adapter: `boruna_orchestrator::audit::log::AuditLog` → [`ItfDoc`].
//!
//! Each `AuditEntry` becomes one ITF state; the event variant becomes the
//! state's `#meta.action`. Variable bindings carry the event's data fields.
//!
//! ITF is operational-only by Boruna's convention (`project-conventions-2026-04`
//! §15) — this conversion does NOT recompute hashes or claim equivalence with
//! the source bundle. The original `audit_log.json` remains the authoritative
//! replay artifact.

use std::collections::BTreeMap;

use boruna_orchestrator::audit::log::{AuditEntry, AuditEvent, AuditLog};

use super::itf::{ItfDoc, ItfState, ItfStatus, ItfValue};

/// Convert an `AuditLog` to an `ItfDoc`. The `source` string is recorded
/// verbatim in `#meta.source` (typically `"boruna <version>"`).
pub fn audit_log_to_itf(log: &AuditLog, source: impl Into<String>) -> ItfDoc {
    let entries = log.entries();
    let status = derive_status(entries);
    let mut doc = ItfDoc::new(source, status);
    for (i, entry) in entries.iter().enumerate() {
        doc.states.push(entry_to_state(entry, i as u64));
    }
    doc.derive_vars();
    doc
}

fn derive_status(entries: &[AuditEntry]) -> ItfStatus {
    // Heuristic mirror of evidence bundle semantics:
    // - any StepFailed or ApprovalDenied → Violation
    // - WorkflowCompleted reached → Ok
    // - else Unknown (in-flight or truncated bundle)
    let mut saw_completed = false;
    for entry in entries {
        match &entry.event {
            AuditEvent::StepFailed { .. } | AuditEvent::ApprovalDenied { .. } => {
                return ItfStatus::Violation;
            }
            AuditEvent::WorkflowCompleted { .. } => saw_completed = true,
            _ => {}
        }
    }
    if saw_completed {
        ItfStatus::Ok
    } else {
        ItfStatus::Unknown
    }
}

fn entry_to_state(entry: &AuditEntry, index: u64) -> ItfState {
    let (action, vars) = event_to_action_and_vars(&entry.event);
    let mut state = ItfState::new().with_index(index).with_action(action);
    state.vars = vars;
    state.set("__sequence", ItfValue::Int(entry.sequence as i64));
    state.set("__entry_hash", ItfValue::Str(entry.entry_hash.clone()));
    state.set("__prev_hash", ItfValue::Str(entry.prev_hash.clone()));
    state
}

fn event_to_action_and_vars(ev: &AuditEvent) -> (&'static str, BTreeMap<String, ItfValue>) {
    let mut vars = BTreeMap::new();
    match ev {
        AuditEvent::WorkflowStarted {
            workflow_hash,
            policy_hash,
        } => {
            vars.insert("workflow_hash".into(), ItfValue::Str(workflow_hash.clone()));
            vars.insert("policy_hash".into(), ItfValue::Str(policy_hash.clone()));
            ("WorkflowStarted", vars)
        }
        AuditEvent::StepStarted {
            step_id,
            input_hash,
        } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("input_hash".into(), ItfValue::Str(input_hash.clone()));
            ("StepStarted", vars)
        }
        AuditEvent::StepCompleted {
            step_id,
            output_hash,
            duration_ms,
        } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("output_hash".into(), ItfValue::Str(output_hash.clone()));
            vars.insert("duration_ms".into(), ItfValue::Int(*duration_ms as i64));
            ("StepCompleted", vars)
        }
        AuditEvent::StepFailed { step_id, error } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("error".into(), ItfValue::Str(error.clone()));
            ("StepFailed", vars)
        }
        AuditEvent::CapabilityInvoked {
            step_id,
            capability,
            allowed,
        } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("capability".into(), ItfValue::Str(capability.clone()));
            vars.insert("allowed".into(), ItfValue::Bool(*allowed));
            ("CapabilityInvoked", vars)
        }
        AuditEvent::PolicyEvaluated {
            step_id,
            rule,
            decision,
        } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("rule".into(), ItfValue::Str(rule.clone()));
            vars.insert("decision".into(), ItfValue::Str(decision.clone()));
            ("PolicyEvaluated", vars)
        }
        AuditEvent::BudgetConsumed {
            step_id,
            tokens,
            remaining,
        } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("tokens".into(), ItfValue::Int(*tokens as i64));
            vars.insert("remaining".into(), ItfValue::Int(*remaining as i64));
            ("BudgetConsumed", vars)
        }
        AuditEvent::ApprovalRequested { step_id, role } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("role".into(), ItfValue::Str(role.clone()));
            ("ApprovalRequested", vars)
        }
        AuditEvent::ApprovalGranted { step_id, approver } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("approver".into(), ItfValue::Str(approver.clone()));
            ("ApprovalGranted", vars)
        }
        AuditEvent::ApprovalDenied { step_id, reason } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("reason".into(), ItfValue::Str(reason.clone()));
            ("ApprovalDenied", vars)
        }
        AuditEvent::ExternalTriggerReceived {
            step_id,
            payload_hash,
        } => {
            vars.insert("step_id".into(), ItfValue::Str(step_id.clone()));
            vars.insert("payload_hash".into(), ItfValue::Str(payload_hash.clone()));
            ("ExternalTriggerReceived", vars)
        }
        AuditEvent::WorkflowCompleted {
            result_hash,
            total_duration_ms,
        } => {
            vars.insert("result_hash".into(), ItfValue::Str(result_hash.clone()));
            vars.insert(
                "total_duration_ms".into(),
                ItfValue::Int(*total_duration_ms as i64),
            );
            ("WorkflowCompleted", vars)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boruna_orchestrator::audit::log::AuditEvent;

    #[test]
    fn empty_log_yields_empty_unknown_doc() {
        let log = AuditLog::new();
        let doc = audit_log_to_itf(&log, "boruna-test");
        assert_eq!(doc.states.len(), 0);
        assert_eq!(doc.meta.status, ItfStatus::Unknown);
    }

    #[test]
    fn completed_log_marks_status_ok() {
        let mut log = AuditLog::new();
        log.append(AuditEvent::WorkflowStarted {
            workflow_hash: "wh".into(),
            policy_hash: "ph".into(),
        });
        log.append(AuditEvent::WorkflowCompleted {
            result_hash: "rh".into(),
            total_duration_ms: 100,
        });
        let doc = audit_log_to_itf(&log, "boruna-test");
        assert_eq!(doc.meta.status, ItfStatus::Ok);
        assert_eq!(doc.states.len(), 2);
    }

    #[test]
    fn failed_step_marks_status_violation() {
        let mut log = AuditLog::new();
        log.append(AuditEvent::WorkflowStarted {
            workflow_hash: "wh".into(),
            policy_hash: "ph".into(),
        });
        log.append(AuditEvent::StepFailed {
            step_id: "s1".into(),
            error: "oops".into(),
        });
        log.append(AuditEvent::WorkflowCompleted {
            result_hash: "rh".into(),
            total_duration_ms: 100,
        });
        let doc = audit_log_to_itf(&log, "boruna-test");
        assert_eq!(doc.meta.status, ItfStatus::Violation);
    }

    #[test]
    fn approval_denied_marks_status_violation() {
        let mut log = AuditLog::new();
        log.append(AuditEvent::ApprovalDenied {
            step_id: "s1".into(),
            reason: "policy".into(),
        });
        let doc = audit_log_to_itf(&log, "boruna-test");
        assert_eq!(doc.meta.status, ItfStatus::Violation);
    }

    #[test]
    fn state_carries_action_and_hash_metadata() {
        let mut log = AuditLog::new();
        log.append(AuditEvent::WorkflowStarted {
            workflow_hash: "wh".into(),
            policy_hash: "ph".into(),
        });
        let doc = audit_log_to_itf(&log, "boruna-test");
        let s = &doc.states[0];
        assert_eq!(
            s.meta.as_ref().unwrap().action.as_deref(),
            Some("WorkflowStarted")
        );
        assert!(matches!(s.vars.get("__entry_hash"), Some(ItfValue::Str(_))));
        assert!(matches!(s.vars.get("workflow_hash"), Some(ItfValue::Str(s)) if s == "wh"));
    }

    #[test]
    fn derive_vars_includes_event_field_names() {
        let mut log = AuditLog::new();
        log.append(AuditEvent::StepCompleted {
            step_id: "s1".into(),
            output_hash: "oh".into(),
            duration_ms: 42,
        });
        let doc = audit_log_to_itf(&log, "boruna-test");
        assert!(doc.vars.iter().any(|v| v == "duration_ms"));
        assert!(doc.vars.iter().any(|v| v == "output_hash"));
        assert!(doc.vars.iter().any(|v| v == "step_id"));
    }

    #[test]
    fn all_event_variants_have_action_name() {
        // Exhaustive — every variant must produce a non-empty action label.
        // Compiler enforces match exhaustiveness in event_to_action_and_vars;
        // this test just spot-checks the most common shapes.
        for ev in [
            AuditEvent::WorkflowStarted {
                workflow_hash: "x".into(),
                policy_hash: "y".into(),
            },
            AuditEvent::StepStarted {
                step_id: "s".into(),
                input_hash: "h".into(),
            },
            AuditEvent::StepCompleted {
                step_id: "s".into(),
                output_hash: "h".into(),
                duration_ms: 0,
            },
            AuditEvent::CapabilityInvoked {
                step_id: "s".into(),
                capability: "c".into(),
                allowed: true,
            },
            AuditEvent::BudgetConsumed {
                step_id: "s".into(),
                tokens: 0,
                remaining: 0,
            },
            AuditEvent::ApprovalRequested {
                step_id: "s".into(),
                role: "r".into(),
            },
            AuditEvent::ApprovalGranted {
                step_id: "s".into(),
                approver: "a".into(),
            },
            AuditEvent::PolicyEvaluated {
                step_id: "s".into(),
                rule: "r".into(),
                decision: "d".into(),
            },
            AuditEvent::ExternalTriggerReceived {
                step_id: "s".into(),
                payload_hash: "h".into(),
            },
            AuditEvent::WorkflowCompleted {
                result_hash: "h".into(),
                total_duration_ms: 0,
            },
        ] {
            let (action, vars) = event_to_action_and_vars(&ev);
            assert!(!action.is_empty());
            assert!(!vars.is_empty());
        }
    }
}
