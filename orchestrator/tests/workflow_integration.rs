use std::collections::BTreeMap;

use boruna_orchestrator::audit::*;
use boruna_orchestrator::workflow::*;

// === Workflow Validation Tests ===

#[test]
fn test_validate_llm_code_review_example() {
    let json =
        std::fs::read_to_string("../examples/workflows/llm_code_review/workflow.json").unwrap();
    let def: WorkflowDef = serde_json::from_str(&json).unwrap();
    assert!(WorkflowValidator::validate(&def).is_ok());
    let order = WorkflowValidator::topological_order(&def).unwrap();
    assert_eq!(order, vec!["fetch_diff", "analyze", "report"]);
}

#[test]
fn test_validate_document_processing_example() {
    let json =
        std::fs::read_to_string("../examples/workflows/document_processing/workflow.json").unwrap();
    let def: WorkflowDef = serde_json::from_str(&json).unwrap();
    assert!(WorkflowValidator::validate(&def).is_ok());
    let order = WorkflowValidator::topological_order(&def).unwrap();
    // ingest must come first, merge must come last
    assert_eq!(order[0], "ingest");
    assert_eq!(*order.last().unwrap(), "merge");
}

#[test]
fn test_validate_customer_support_triage_example() {
    let json =
        std::fs::read_to_string("../examples/workflows/customer_support_triage/workflow.json")
            .unwrap();
    let def: WorkflowDef = serde_json::from_str(&json).unwrap();
    assert!(WorkflowValidator::validate(&def).is_ok());
}

// === Workflow Execution Tests ===

#[test]
fn test_run_llm_code_review_completes() {
    let json =
        std::fs::read_to_string("../examples/workflows/llm_code_review/workflow.json").unwrap();
    let def: WorkflowDef = serde_json::from_str(&json).unwrap();

    let options = RunOptions {
        policy: Some(boruna_vm::capability_gateway::Policy::allow_all()),
        record: false,
        workflow_dir: "../examples/workflows/llm_code_review".into(),
    };

    let result = WorkflowRunner::run(&def, &options).unwrap();
    assert_eq!(result.status, WorkflowStatus::Completed);
    assert_eq!(result.step_results.len(), 3);
}

#[test]
fn test_run_document_processing_completes() {
    let json =
        std::fs::read_to_string("../examples/workflows/document_processing/workflow.json").unwrap();
    let def: WorkflowDef = serde_json::from_str(&json).unwrap();

    let options = RunOptions {
        policy: Some(boruna_vm::capability_gateway::Policy::allow_all()),
        record: false,
        workflow_dir: "../examples/workflows/document_processing".into(),
    };

    let result = WorkflowRunner::run(&def, &options).unwrap();
    assert_eq!(result.status, WorkflowStatus::Completed);
    assert_eq!(result.step_results.len(), 5);
}

#[test]
fn test_run_customer_support_triage_pauses_at_approval() {
    let json =
        std::fs::read_to_string("../examples/workflows/customer_support_triage/workflow.json")
            .unwrap();
    let def: WorkflowDef = serde_json::from_str(&json).unwrap();

    let options = RunOptions {
        policy: Some(boruna_vm::capability_gateway::Policy::allow_all()),
        record: false,
        workflow_dir: "../examples/workflows/customer_support_triage".into(),
    };

    let result = WorkflowRunner::run(&def, &options).unwrap();
    assert_eq!(result.status, WorkflowStatus::Paused);
    assert_eq!(result.step_results["receive"].status, StepStatus::Completed);
    assert_eq!(result.step_results["triage"].status, StepStatus::Completed);
    assert_eq!(
        result.step_results["approve"].status,
        StepStatus::AwaitingApproval
    );
    assert!(!result.step_results.contains_key("route"));
}

// === Determinism Tests ===

#[test]
fn test_workflow_determinism_same_result() {
    let json =
        std::fs::read_to_string("../examples/workflows/llm_code_review/workflow.json").unwrap();
    let def: WorkflowDef = serde_json::from_str(&json).unwrap();

    let options = RunOptions {
        policy: Some(boruna_vm::capability_gateway::Policy::allow_all()),
        record: false,
        workflow_dir: "../examples/workflows/llm_code_review".into(),
    };

    let result1 = WorkflowRunner::run(&def, &options).unwrap();
    let result2 = WorkflowRunner::run(&def, &options).unwrap();

    // Same status
    assert_eq!(result1.status, result2.status);
    // Same step results (same output hashes)
    for (id, sr1) in &result1.step_results {
        let sr2 = &result2.step_results[id];
        assert_eq!(sr1.status, sr2.status);
        assert_eq!(sr1.output_hash, sr2.output_hash);
    }
}

// === Audit Log Tests ===

#[test]
fn test_audit_log_chain_100_entries() {
    let mut log = AuditLog::new();
    for i in 0..100 {
        log.append(log::AuditEvent::StepStarted {
            step_id: format!("step_{i}"),
            input_hash: format!("hash_{i}"),
        });
    }
    assert_eq!(log.entries().len(), 100);
    assert!(log.verify().is_ok());
}

#[test]
fn test_audit_log_serialize_roundtrip() {
    let mut log = AuditLog::new();
    log.append(log::AuditEvent::WorkflowStarted {
        workflow_hash: "abc".into(),
        policy_hash: "def".into(),
    });
    log.append(log::AuditEvent::StepCompleted {
        step_id: "s1".into(),
        output_hash: "out".into(),
        duration_ms: 50,
    });
    log.append(log::AuditEvent::WorkflowCompleted {
        result_hash: "res".into(),
        total_duration_ms: 100,
    });

    let json = log.to_json().unwrap();
    let restored = AuditLog::from_json(&json).unwrap();
    assert!(restored.verify().is_ok());
    assert_eq!(log.hash(), restored.hash());
}

// === Evidence Bundle Tests ===

#[test]
fn test_evidence_bundle_full_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let json =
        std::fs::read_to_string("../examples/workflows/llm_code_review/workflow.json").unwrap();
    let def: WorkflowDef = serde_json::from_str(&json).unwrap();

    let mut builder =
        evidence::EvidenceBundleBuilder::new(dir.path(), "run-evidence-test", &def.name).unwrap();
    builder.add_workflow_def(&json).unwrap();
    builder.add_policy(r#"{"default_allow":true}"#).unwrap();
    builder
        .add_step_output("fetch_diff", "result", r#"42"#)
        .unwrap();
    builder
        .add_step_output("analyze", "result", r#"85"#)
        .unwrap();
    builder.add_step_output("report", "result", r#"1"#).unwrap();

    let mut audit = AuditLog::new();
    audit.append(log::AuditEvent::WorkflowStarted {
        workflow_hash: "w".into(),
        policy_hash: "p".into(),
    });
    audit.append(log::AuditEvent::WorkflowCompleted {
        result_hash: "r".into(),
        total_duration_ms: 10,
    });

    let manifest = builder.finalize(&audit).unwrap();
    assert!(!manifest.bundle_hash.is_empty());

    // Verify the bundle
    let bundle_dir = dir.path().join("run-evidence-test");
    let result = verify::verify_bundle(&bundle_dir);
    assert!(result.valid, "errors: {:?}", result.errors);
}

#[test]
fn test_evidence_bundle_tamper_detected() {
    let dir = tempfile::tempdir().unwrap();
    let mut builder =
        evidence::EvidenceBundleBuilder::new(dir.path(), "run-tamper-test", "test").unwrap();
    builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
    builder.add_policy(r#"{}"#).unwrap();

    let audit = AuditLog::new();
    builder.finalize(&audit).unwrap();

    let bundle_dir = dir.path().join("run-tamper-test");

    // Tamper with a file
    std::fs::write(bundle_dir.join("workflow.json"), r#"{"name":"EVIL"}"#).unwrap();

    let result = verify::verify_bundle(&bundle_dir);
    assert!(!result.valid);
    assert!(result.errors.iter().any(|e| e.contains("checksum")));
}

// === Schema Compatibility Tests ===

#[test]
fn test_workflow_def_missing_schema_version_defaults() {
    // Simulate a v0 workflow.json without schema_version
    let json = r#"{
        "name": "legacy",
        "version": "0.9.0",
        "steps": {
            "a": { "kind": "source", "source": "a.ax" }
        },
        "edges": []
    }"#;
    let def: WorkflowDef = serde_json::from_str(json).unwrap();
    assert_eq!(def.schema_version, 1); // default
    assert!(WorkflowValidator::validate(&def).is_ok());
}

#[test]
fn test_step_def_minimal_fields() {
    // Only required fields, all optionals use defaults
    let json = r#"{
        "name": "minimal",
        "version": "1.0.0",
        "steps": {
            "step1": { "kind": "source", "source": "step.ax" }
        },
        "edges": []
    }"#;
    let def: WorkflowDef = serde_json::from_str(json).unwrap();
    let step = &def.steps["step1"];
    assert!(step.capabilities.is_empty());
    assert!(step.inputs.is_empty());
    assert!(step.outputs.is_empty());
    assert!(step.depends_on.is_empty());
    assert!(step.timeout_ms.is_none());
    assert!(step.retry.is_none());
    assert!(step.budget.is_none());
}

#[test]
fn test_policy_schema_version_default() {
    let json = r#"{"rules":{},"default_allow":true}"#;
    let policy: boruna_vm::capability_gateway::Policy = serde_json::from_str(json).unwrap();
    assert_eq!(policy.schema_version, 1);
}

// === Data Flow Tests ===

#[test]
fn test_data_flow_between_steps() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = DataStore::new(dir.path()).unwrap();

    // Step A produces output
    let value = boruna_bytecode::Value::Int(42);
    store.store_output("step_a", "result", &value).unwrap();

    // Step B reads step A's output
    let mut inputs = BTreeMap::new();
    inputs.insert("input_val".into(), "step_a.result".into());
    let resolved = store.resolve_step_inputs(&inputs).unwrap();
    assert_eq!(resolved["input_val"], boruna_bytecode::Value::Int(42));
}

#[test]
fn test_data_flow_hash_determinism() {
    let v1 = boruna_bytecode::Value::String("test data".into());
    let v2 = boruna_bytecode::Value::String("test data".into());
    assert_eq!(DataStore::hash_value(&v1), DataStore::hash_value(&v2));

    let v3 = boruna_bytecode::Value::String("different".into());
    assert_ne!(DataStore::hash_value(&v1), DataStore::hash_value(&v3));
}
