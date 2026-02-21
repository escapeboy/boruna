use std::path::Path;

use crate::adapters::{self, CompileAdapter, GateAdapter, GateContext, TestAdapter, ReplayAdapter};
use crate::engine::{NodeStatus, Role, Scheduler, WorkGraph};
use crate::patch::PatchBundle;
use crate::storage::Store;

/// Default storage directory relative to workspace root.
const STORAGE_DIR: &str = "orchestrator/storage";

fn store_for(workspace: &Path) -> Result<Store, String> {
    Store::new(&workspace.join(STORAGE_DIR))
}

fn load_active_graph(store: &Store) -> Result<WorkGraph, String> {
    let graph_id = store.latest_graph()?
        .ok_or("no work graph found; run 'orch plan' first")?;
    store.load_graph(&graph_id)
}

/// `orch plan <spec.json>` — Create a DAG from a plan specification file.
pub fn cmd_plan(workspace: &Path, spec_path: &Path) -> Result<(), String> {
    let data = std::fs::read_to_string(spec_path)
        .map_err(|e| format!("cannot read spec: {e}"))?;
    let graph: WorkGraph = serde_json::from_str(&data)
        .map_err(|e| format!("invalid spec JSON: {e}"))?;

    // Validate DAG
    let sched = Scheduler::new(graph.clone(), 4);
    sched.validate()?;

    let store = store_for(workspace)?;
    store.save_graph(&graph)?;

    let order = sched.topological_order()?;
    println!("created work graph: {}", graph.id);
    println!("  {} nodes", graph.nodes.len());
    println!("  execution order: {}", order.join(" → "));

    Ok(())
}

/// `orch next --role <role>` — Assign the next ready node for a role.
pub fn cmd_next(workspace: &Path, role: Role) -> Result<(), String> {
    let store = store_for(workspace)?;
    let graph = load_active_graph(&store)?;
    let mut sched = Scheduler::new(graph, 4);

    match sched.assign_next(role.clone()) {
        Some(node_id) => {
            // Acquire locks for the node's outputs
            let node = sched.graph.node(&node_id).unwrap();
            let outputs = node.outputs.clone();
            let mut locks = store.load_locks()?;
            let timestamp = chrono::Utc::now().to_rfc3339();

            match locks.acquire(&node_id, &outputs, &timestamp) {
                Ok(()) => {
                    store.save_locks(&locks)?;
                    store.save_graph(&sched.graph)?;
                    let node = sched.graph.node(&node_id).unwrap();
                    println!("assigned: {} ({})", node_id, node.description);
                    println!("  role: {role}");
                    if !outputs.is_empty() {
                        println!("  locked: {}", outputs.join(", "));
                    }
                }
                Err(conflict) => {
                    // Mark blocked instead
                    sched.mark_blocked(&node_id)?;
                    store.save_graph(&sched.graph)?;
                    println!("node {node_id} blocked: {conflict}");
                }
            }
        }
        None => {
            println!("no ready nodes for role '{role}'");
        }
    }

    Ok(())
}

/// `orch apply <bundle.patchbundle.json>` — Apply a patch bundle and run gates.
pub fn cmd_apply(workspace: &Path, bundle_path: &Path) -> Result<(), String> {
    let bundle = PatchBundle::load(bundle_path)?;

    // Validate bundle format
    if let Err(errors) = bundle.validate() {
        println!("bundle validation failed:");
        for e in &errors {
            println!("  - {e}");
        }
        return Err("invalid bundle".into());
    }

    let store = store_for(workspace)?;
    let locks = store.load_locks()?;

    // Check lock conflicts
    let conflicts = locks.check_conflicts(
        &bundle.metadata.id,
        &bundle.metadata.touched_modules,
    );
    if !conflicts.is_empty() {
        println!("lock conflicts:");
        for c in &conflicts {
            println!("  - {c}");
        }
        return Err("cannot apply: lock conflict".into());
    }

    // Apply the patch bundle
    println!("applying bundle: {} ({})", bundle.metadata.id, bundle.metadata.intent);
    let rollback = bundle.apply(workspace)?;
    println!("  patches applied to {} files", bundle.patches.len());

    // Run gates
    let adapters: Vec<Box<dyn GateAdapter>> = build_gate_adapters(&bundle);
    let ctx = GateContext {
        workspace_root: workspace,
        example_files: vec![],
    };

    println!("running gates...");
    let results = adapters::run_gates(&adapters, &ctx);

    let all_pass = results.iter().all(|r| r.status != adapters::GateStatus::Fail);

    for r in &results {
        let icon = match r.status {
            adapters::GateStatus::Pass => "PASS",
            adapters::GateStatus::Fail => "FAIL",
            adapters::GateStatus::Skip => "SKIP",
        };
        println!("  [{icon}] {} ({}ms)", r.gate, r.duration_ms);
    }

    // Save gate results
    let gate_json = serde_json::json!({
        "bundle_id": bundle.metadata.id,
        "results": results,
        "all_pass": all_pass,
    });
    store.save_gate_result(&bundle.metadata.id, &gate_json)?;

    if all_pass {
        println!("all gates passed");
        // Save rollback bundle for future use
        let rollback_path = store.bundles_dir()
            .join(format!("{}-rollback.patchbundle.json", bundle.metadata.id));
        rollback.save(&rollback_path)?;
        println!("  rollback saved to {}", rollback_path.display());
    } else {
        println!("gate failure — rolling back patches");
        rollback.apply(workspace)?;
        println!("  rollback complete");
        return Err("gates failed".into());
    }

    Ok(())
}

/// `orch review <bundle.patchbundle.json>` — Review a bundle: validate + gates + checklist.
pub fn cmd_review(workspace: &Path, bundle_path: &Path) -> Result<(), String> {
    let bundle = PatchBundle::load(bundle_path)?;

    println!("=== Review: {} ===", bundle.metadata.id);
    println!("intent: {}", bundle.metadata.intent);
    println!("author: {}", bundle.metadata.author);
    println!("risk: {:?}", bundle.metadata.risk_level);
    println!("modules: {}", bundle.metadata.touched_modules.join(", "));
    println!();

    // 1. Validate format
    print!("format validation: ");
    match bundle.validate() {
        Ok(()) => println!("PASS"),
        Err(errors) => {
            println!("FAIL");
            for e in &errors {
                println!("  - {e}");
            }
            return Ok(output_review_result("reject", "format validation failed"));
        }
    }

    // 2. Check content hash
    let hash = bundle.content_hash();
    println!("content hash: {hash}");

    // 3. Run gates (compile + test)
    let adapters: Vec<Box<dyn GateAdapter>> = build_gate_adapters(&bundle);
    let ctx = GateContext {
        workspace_root: workspace,
        example_files: vec![],
    };

    println!("\nrunning gates...");
    let results = adapters::run_gates(&adapters, &ctx);

    let all_pass = results.iter().all(|r| r.status != adapters::GateStatus::Fail);

    for r in &results {
        let icon = match r.status {
            adapters::GateStatus::Pass => "PASS",
            adapters::GateStatus::Fail => "FAIL",
            adapters::GateStatus::Skip => "SKIP",
        };
        println!("  [{icon}] {} ({}ms)", r.gate, r.duration_ms);
    }

    if !all_pass {
        return Ok(output_review_result("reject", "gate check failed"));
    }

    // 4. Reviewer checklist
    println!("\nreviewer checklist:");
    for item in &bundle.reviewer_checklist {
        println!("  [ ] {item}");
    }

    // 5. Two-person rule check
    println!("\ntwo-person rule: reviewer must differ from author ({})", bundle.metadata.author);

    // Store gate results
    let store = store_for(workspace)?;
    let gate_json = serde_json::json!({
        "bundle_id": bundle.metadata.id,
        "review": true,
        "results": results,
        "all_pass": all_pass,
        "content_hash": hash,
    });
    store.save_gate_result(&format!("{}-review", bundle.metadata.id), &gate_json)?;

    output_review_result("approve", "all gates passed, checklist presented");
    Ok(())
}

/// `orch status` — Show current graph state.
pub fn cmd_status(workspace: &Path) -> Result<(), String> {
    let store = store_for(workspace)?;
    let graph = load_active_graph(&store)?;
    let sched = Scheduler::new(graph.clone(), 4);
    let summary = sched.summary();

    println!("=== Work Graph: {} ===", graph.id);
    println!("{}", graph.description);
    println!();
    println!("nodes: {} total", summary.total);
    println!("  passed:  {}", summary.passed);
    println!("  running: {}", summary.running);
    println!("  ready:   {}", summary.ready);
    println!("  blocked: {}", summary.blocked);
    println!("  failed:  {}", summary.failed);
    println!("  pending: {}", summary.pending);
    println!();

    for node in &graph.nodes {
        let status_icon = match node.status {
            NodeStatus::Passed => "[OK]",
            NodeStatus::Running => "[..]",
            NodeStatus::Ready => "[>>]",
            NodeStatus::Blocked => "[!!]",
            NodeStatus::Failed => "[XX]",
            NodeStatus::Pending => "[  ]",
        };
        println!("  {status_icon} {} — {} ({})", node.id, node.description, node.owner_role);
        if !node.dependencies.is_empty() {
            println!("        deps: {}", node.dependencies.join(", "));
        }
    }

    // Show locks
    let locks = store.load_locks()?;
    let active = locks.active_locks();
    if !active.is_empty() {
        println!("\nactive locks:");
        for lock in active {
            println!("  {} → {} (since {})", lock.module, lock.held_by, lock.acquired_at);
        }
    }

    Ok(())
}

/// `orch report --json` — Machine-readable summary.
pub fn cmd_report(workspace: &Path) -> Result<(), String> {
    let store = store_for(workspace)?;
    let graph = load_active_graph(&store)?;
    let sched = Scheduler::new(graph.clone(), 4);
    let summary = sched.summary();
    let locks = store.load_locks()?;

    let nodes_json: Vec<serde_json::Value> = graph.nodes.iter().map(|n| {
        serde_json::json!({
            "id": n.id,
            "description": n.description,
            "status": n.status,
            "role": n.owner_role,
            "dependencies": n.dependencies,
            "outputs": n.outputs,
            "tags": n.tags,
        })
    }).collect();

    let locks_json: Vec<serde_json::Value> = locks.active_locks().iter().map(|l| {
        serde_json::json!({
            "module": l.module,
            "held_by": l.held_by,
            "acquired_at": l.acquired_at,
        })
    }).collect();

    let report = serde_json::json!({
        "graph_id": graph.id,
        "description": graph.description,
        "total_nodes": summary.total,
        "passed": summary.passed,
        "running": summary.running,
        "ready": summary.ready,
        "blocked": summary.blocked,
        "failed": summary.failed,
        "pending": summary.pending,
        "nodes": nodes_json,
        "locks": locks_json,
    });

    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    Ok(())
}

fn output_review_result(decision: &str, reason: &str) {
    println!("\n=== Review Result: {} ===", decision.to_uppercase());
    println!("reason: {reason}");
}

fn build_gate_adapters(bundle: &PatchBundle) -> Vec<Box<dyn GateAdapter>> {
    let mut adapters: Vec<Box<dyn GateAdapter>> = Vec::new();

    if bundle.expected_checks.compile {
        adapters.push(Box::new(CompileAdapter));
    }
    if bundle.expected_checks.test {
        adapters.push(Box::new(TestAdapter));
    }
    if bundle.expected_checks.replay {
        adapters.push(Box::new(ReplayAdapter { expected_hashes: vec![] }));
    }

    adapters
}
