use boruna_orchestrator::conflict::*;
use boruna_orchestrator::engine::*;
use boruna_orchestrator::patch::*;
use boruna_orchestrator::storage::*;

// === DAG Scheduling Tests ===

#[test]
fn test_dag_linear_chain() {
    let graph = WorkGraph {
        schema_version: 1,
        id: "G-linear".into(),
        description: "A → B → C".into(),
        nodes: vec![
            node("A", &[], Role::Implementer),
            node("B", &["A"], Role::Reviewer),
            node("C", &["B"], Role::Implementer),
        ],
    };
    let mut sched = Scheduler::new(graph, 4);
    sched.validate().unwrap();

    // Only A is ready
    assert_eq!(sched.ready_nodes(), vec!["A"]);

    // Pass A → B becomes ready
    sched.mark_passed("A").unwrap();
    assert_eq!(sched.ready_nodes(), vec!["B"]);

    // Pass B → C becomes ready
    sched.mark_passed("B").unwrap();
    assert_eq!(sched.ready_nodes(), vec!["C"]);

    // Pass C → nothing more
    sched.mark_passed("C").unwrap();
    assert!(sched.ready_nodes().is_empty());
    assert_eq!(sched.summary().passed, 3);
}

#[test]
fn test_dag_diamond_dependency() {
    // A → B, A → C, B → D, C → D
    let graph = WorkGraph {
        schema_version: 1,
        id: "G-diamond".into(),
        description: "diamond".into(),
        nodes: vec![
            node("A", &[], Role::Implementer),
            node("B", &["A"], Role::Implementer),
            node("C", &["A"], Role::Implementer),
            node("D", &["B", "C"], Role::Reviewer),
        ],
    };
    let mut sched = Scheduler::new(graph, 4);
    sched.validate().unwrap();

    assert_eq!(sched.ready_nodes(), vec!["A"]);
    sched.mark_passed("A").unwrap();

    // B and C are both ready now
    let ready = sched.ready_nodes();
    assert_eq!(ready.len(), 2);
    assert!(ready.contains(&"B".to_string()));
    assert!(ready.contains(&"C".to_string()));

    // D requires both B and C
    sched.mark_passed("B").unwrap();
    assert!(sched.ready_nodes().is_empty() || !sched.ready_nodes().contains(&"D".to_string()));

    sched.mark_passed("C").unwrap();
    assert_eq!(sched.ready_nodes(), vec!["D"]);
}

#[test]
fn test_dag_cycle_rejected() {
    let graph = WorkGraph {
        schema_version: 1,
        id: "G-cycle".into(),
        description: "cycle".into(),
        nodes: vec![
            node("A", &["C"], Role::Implementer),
            node("B", &["A"], Role::Implementer),
            node("C", &["B"], Role::Implementer),
        ],
    };
    let sched = Scheduler::new(graph, 4);
    assert!(sched.validate().is_err());
}

#[test]
fn test_dag_concurrency_enforcement() {
    let graph = WorkGraph {
        schema_version: 1,
        id: "G-conc".into(),
        description: "3 parallel tasks".into(),
        nodes: vec![
            node("A", &[], Role::Implementer),
            node("B", &[], Role::Implementer),
            node("C", &[], Role::Implementer),
        ],
    };
    let mut sched = Scheduler::new(graph, 2); // max 2 parallel

    // Only 2 ready due to limit
    let ready = sched.ready_nodes();
    assert_eq!(ready.len(), 2);

    // Set one running
    sched.graph.node_mut("A").unwrap().status = NodeStatus::Running;
    // Only 1 slot left
    assert_eq!(sched.ready_nodes().len(), 1);

    // Set both running
    sched.graph.node_mut("B").unwrap().status = NodeStatus::Running;
    assert_eq!(sched.ready_nodes().len(), 0);

    // Pass one → slot opens
    sched.mark_passed("A").unwrap();
    assert_eq!(sched.ready_nodes().len(), 1);
}

#[test]
fn test_dag_failed_blocks_dependents() {
    let graph = WorkGraph {
        schema_version: 1,
        id: "G-fail".into(),
        description: "failure".into(),
        nodes: vec![
            node("A", &[], Role::Implementer),
            node("B", &["A"], Role::Implementer),
        ],
    };
    let mut sched = Scheduler::new(graph, 4);

    sched.mark_failed("A").unwrap();
    // B should never become ready since A is failed (not passed)
    assert!(sched.ready_nodes().is_empty());
}

// === Bundle Validation Tests ===

#[test]
fn test_bundle_validate_complete() {
    let bundle = sample_bundle();
    assert!(bundle.validate().is_ok());
}

#[test]
fn test_bundle_validate_missing_fields() {
    let mut bundle = sample_bundle();
    bundle.metadata.id = "".into();
    bundle.metadata.intent = "".into();
    bundle.metadata.author = "".into();
    let errors = bundle.validate().unwrap_err();
    assert_eq!(errors.len(), 3);
}

#[test]
fn test_bundle_hash_deterministic() {
    let b1 = sample_bundle();
    let b2 = sample_bundle();
    assert_eq!(b1.content_hash(), b2.content_hash());
}

#[test]
fn test_bundle_hash_differs_on_change() {
    let b1 = sample_bundle();
    let mut b2 = sample_bundle();
    b2.patches[0].hunks[0].new_text = "changed".into();
    assert_ne!(b1.content_hash(), b2.content_hash());
}

#[test]
fn test_bundle_apply_rollback_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "line_one\nline_two\nline_three\n").unwrap();

    let bundle = PatchBundle {
        version: 1,
        metadata: PatchMetadata {
            id: "PB-rt".into(),
            intent: "roundtrip".into(),
            author: "test".into(),
            timestamp: "now".into(),
            touched_modules: vec![],
            risk_level: RiskLevel::Low,
        },
        patches: vec![FilePatch {
            file: "test.txt".into(),
            hunks: vec![Hunk {
                start_line: 2,
                old_text: "line_two".into(),
                new_text: "modified_two".into(),
            }],
        }],
        expected_checks: ExpectedChecks {
            compile: true,
            test: false,
            replay: false,
            diagnostics_count: None,
        },
        reviewer_checklist: vec![],
    };

    let original = std::fs::read_to_string(&file).unwrap();
    let rollback = bundle.apply(dir.path()).unwrap();

    // Verify change applied
    let changed = std::fs::read_to_string(&file).unwrap();
    assert!(changed.contains("modified_two"));
    assert!(!changed.contains("line_two"));

    // Rollback
    rollback.apply(dir.path()).unwrap();
    let restored = std::fs::read_to_string(&file).unwrap();
    assert_eq!(restored.trim(), original.trim());
}

#[test]
fn test_bundle_apply_fails_on_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "actual_content\n").unwrap();

    let bundle = PatchBundle {
        version: 1,
        metadata: PatchMetadata {
            id: "PB-mismatch".into(),
            intent: "mismatch".into(),
            author: "test".into(),
            timestamp: "now".into(),
            touched_modules: vec![],
            risk_level: RiskLevel::Low,
        },
        patches: vec![FilePatch {
            file: "test.txt".into(),
            hunks: vec![Hunk {
                start_line: 1,
                old_text: "expected_content".into(),
                new_text: "new_content".into(),
            }],
        }],
        expected_checks: ExpectedChecks {
            compile: false,
            test: false,
            replay: false,
            diagnostics_count: None,
        },
        reviewer_checklist: vec![],
    };

    let result = bundle.apply(dir.path());
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("does not match"));

    // File unchanged
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content, "actual_content\n");
}

// === Lock Conflict Tests ===

#[test]
fn test_lock_acquire_release_cycle() {
    let mut locks = LockTable::new();

    locks
        .acquire("N1", &["boruna-bytecode".into(), "boruna-vm".into()], "t1")
        .unwrap();
    assert_eq!(locks.active_locks().len(), 2);

    locks.release("N1");
    assert_eq!(locks.active_locks().len(), 0);

    // Can now acquire same modules with different node
    locks
        .acquire("N2", &["boruna-bytecode".into()], "t2")
        .unwrap();
    assert_eq!(locks.active_locks().len(), 1);
}

#[test]
fn test_lock_conflict_blocks_node() {
    let mut locks = LockTable::new();
    locks
        .acquire("N1", &["boruna-bytecode".into()], "t1")
        .unwrap();

    // N2 tries to lock same module
    let result = locks.acquire("N2", &["boruna-bytecode".into()], "t2");
    assert!(result.is_err());
    let conflict = result.unwrap_err();
    assert_eq!(conflict.held_by, "N1");
    assert_eq!(conflict.requested_by, "N2");
}

#[test]
fn test_lock_no_conflict_different_modules() {
    let mut locks = LockTable::new();
    locks
        .acquire("N1", &["boruna-bytecode".into()], "t1")
        .unwrap();
    locks.acquire("N2", &["boruna-vm".into()], "t2").unwrap();
    assert_eq!(locks.active_locks().len(), 2);
}

#[test]
fn test_lock_conflict_scenario_from_plan() {
    // Simulates conflict_plan.json: WN-100 and WN-101 both want llmbc
    let mut locks = LockTable::new();

    // WN-100 locks llmbc first
    locks
        .acquire("WN-100", &["boruna-bytecode".into()], "t1")
        .unwrap();

    // WN-101 tries llmbc → conflict
    let result = locks.acquire("WN-101", &["boruna-bytecode".into()], "t2");
    assert!(result.is_err());

    // WN-102 locks docs → no conflict
    locks.acquire("WN-102", &["docs".into()], "t3").unwrap();

    // After WN-100 finishes
    locks.release("WN-100");

    // WN-101 can now acquire
    locks
        .acquire("WN-101", &["boruna-bytecode".into()], "t4")
        .unwrap();
}

// === Deterministic Gating Integration (Mock) ===

#[test]
fn test_gate_results_storage() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();

    let gate_result = serde_json::json!({
        "node_id": "WN-001",
        "gates": {
            "compile": { "status": "pass", "duration_ms": 3200 },
            "test": { "status": "pass", "duration_ms": 8100, "total": 179, "passed": 179 },
        }
    });

    store.save_gate_result("WN-001", &gate_result).unwrap();
    let loaded = store.load_gate_result("WN-001").unwrap();

    assert_eq!(loaded["gates"]["test"]["total"], 179);
    assert_eq!(loaded["gates"]["compile"]["status"], "pass");
}

#[test]
fn test_full_workflow_with_storage() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();

    // Create graph
    let graph = WorkGraph {
        schema_version: 1,
        id: "G-workflow".into(),
        description: "full workflow test".into(),
        nodes: vec![
            node("W1", &[], Role::Implementer),
            node("W2", &["W1"], Role::Reviewer),
        ],
    };

    store.save_graph(&graph).unwrap();

    // Load and verify
    let loaded = store.load_graph("G-workflow").unwrap();
    assert_eq!(loaded.nodes.len(), 2);

    // Create scheduler and advance
    let mut sched = Scheduler::new(loaded, 4);
    let assigned = sched.assign_next(Role::Implementer);
    assert_eq!(assigned, Some("W1".into()));

    // Save updated state
    store.save_graph(&sched.graph).unwrap();

    // Mark passed, save
    sched.mark_passed("W1").unwrap();
    store.save_graph(&sched.graph).unwrap();

    // Reviewer can now proceed
    let assigned = sched.assign_next(Role::Reviewer);
    assert_eq!(assigned, Some("W2".into()));
}

#[test]
fn test_parallel_plan_scheduling() {
    // Load the parallel plan example
    let data = include_str!("../spec/examples/parallel_modules_plan.json");
    let graph: WorkGraph = serde_json::from_str(data).unwrap();
    let sched = Scheduler::new(graph, 4);
    sched.validate().unwrap();

    // WN-010 and WN-020 should both be ready (independent)
    let ready = sched.ready_nodes();
    assert!(ready.contains(&"WN-010".to_string()));
    assert!(ready.contains(&"WN-020".to_string()));
    assert!(!ready.contains(&"WN-011".to_string()));
    assert!(!ready.contains(&"WN-021".to_string()));
}

#[test]
fn test_single_module_plan_order() {
    let data = include_str!("../spec/examples/single_module_plan.json");
    let graph: WorkGraph = serde_json::from_str(data).unwrap();
    let sched = Scheduler::new(graph, 4);
    let order = sched.topological_order().unwrap();

    let p1 = order.iter().position(|x| x == "WN-001").unwrap();
    let p2 = order.iter().position(|x| x == "WN-002").unwrap();
    let p3 = order.iter().position(|x| x == "WN-003").unwrap();
    assert!(p1 < p2);
    assert!(p2 < p3);
}

#[test]
fn test_conflict_plan_locks() {
    let data = include_str!("../spec/examples/conflict_plan.json");
    let graph: WorkGraph = serde_json::from_str(data).unwrap();
    let sched = Scheduler::new(graph, 4);
    sched.validate().unwrap();

    // Both WN-100 and WN-101 are ready (no deps)
    let ready = sched.ready_nodes();
    assert!(ready.contains(&"WN-100".to_string()));
    assert!(ready.contains(&"WN-101".to_string()));

    // Simulate lock conflict
    let mut locks = LockTable::new();
    locks
        .acquire("WN-100", &["boruna-bytecode".into()], "t1")
        .unwrap();

    // WN-101 blocked
    assert!(locks
        .acquire("WN-101", &["boruna-bytecode".into()], "t2")
        .is_err());

    // WN-102 (docs) not blocked
    locks.acquire("WN-102", &["docs".into()], "t3").unwrap();
}

// === Helpers ===

fn node(id: &str, deps: &[&str], role: Role) -> WorkNode {
    WorkNode {
        id: id.to_string(),
        description: format!("node {id}"),
        inputs: vec![],
        outputs: vec![id.to_lowercase()],
        dependencies: deps.iter().map(|s| s.to_string()).collect(),
        owner_role: role,
        tags: vec![],
        status: NodeStatus::Pending,
        assigned_to: None,
        patch_bundle: None,
        review_result: None,
    }
}

fn sample_bundle() -> PatchBundle {
    PatchBundle {
        version: 1,
        metadata: PatchMetadata {
            id: "PB-test-001".into(),
            intent: "test bundle".into(),
            author: "test-agent".into(),
            timestamp: "2026-02-20T00:00:00Z".into(),
            touched_modules: vec!["boruna-bytecode".into()],
            risk_level: RiskLevel::Low,
        },
        patches: vec![FilePatch {
            file: "test.txt".into(),
            hunks: vec![Hunk {
                start_line: 1,
                old_text: "original".into(),
                new_text: "modified".into(),
            }],
        }],
        expected_checks: ExpectedChecks {
            compile: true,
            test: true,
            replay: false,
            diagnostics_count: None,
        },
        reviewer_checklist: vec!["Check backward compatibility".into()],
    }
}
