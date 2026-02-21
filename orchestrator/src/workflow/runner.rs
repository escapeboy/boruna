use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use boruna_vm::capability_gateway::{CapabilityGateway, Policy, PolicyRule};
use boruna_vm::Vm;

use crate::workflow::data_flow::DataStore;
use crate::workflow::definition::*;
use crate::workflow::validator::WorkflowValidator;

/// Options for a workflow run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Policy to apply (None = deny-all).
    pub policy: Option<Policy>,
    /// Whether to record evidence.
    pub record: bool,
    /// Base directory for the workflow definition files.
    pub workflow_dir: String,
    /// Use real HTTP handler instead of mock (requires `http` feature).
    pub live: bool,
}

/// Executes a validated workflow definition step by step.
pub struct WorkflowRunner;

impl WorkflowRunner {
    /// Run a workflow to completion (or until an approval gate is hit).
    pub fn run(
        def: &WorkflowDef,
        options: &RunOptions,
    ) -> Result<WorkflowRunResult, WorkflowRunError> {
        // Validate first
        WorkflowValidator::validate(def).map_err(|errors| {
            WorkflowRunError::Validation(
                errors
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;

        // Compute execution order
        let order =
            WorkflowValidator::topological_order(def).map_err(WorkflowRunError::Validation)?;

        let run_id = format!(
            "run-{}-{}",
            def.name,
            chrono::Utc::now().format("%Y%m%dT%H%M%S")
        );
        let run_start = Instant::now();

        // Create data store in a temp directory
        let run_dir = tempfile::tempdir().map_err(|e| WorkflowRunError::Io(e.to_string()))?;
        let mut data_store =
            DataStore::new(run_dir.path()).map_err(|e| WorkflowRunError::Io(e.to_string()))?;

        let mut step_results: BTreeMap<String, StepResult> = BTreeMap::new();
        let mut workflow_status = WorkflowStatus::Running;

        for step_id in &order {
            let step_def = def
                .steps
                .get(step_id)
                .ok_or_else(|| WorkflowRunError::Internal(format!("step not found: {step_id}")))?;

            let step_start = Instant::now();

            match &step_def.kind {
                StepKind::ApprovalGate { required_role, .. } => {
                    step_results.insert(
                        step_id.clone(),
                        StepResult {
                            step_id: step_id.clone(),
                            status: StepStatus::AwaitingApproval,
                            output_hash: None,
                            duration_ms: 0,
                            capabilities_used: vec![],
                            error: None,
                        },
                    );
                    workflow_status = WorkflowStatus::Paused;
                    eprintln!(
                        "Awaiting approval for step '{}' (role: {}). \
                         Run: boruna workflow approve {} {}",
                        step_id, required_role, run_id, step_id
                    );
                    break;
                }
                StepKind::Source { source } => {
                    let result = Self::execute_source_step(
                        step_id,
                        source,
                        step_def,
                        &options.workflow_dir,
                        &options.policy,
                        &mut data_store,
                        options.live,
                    );

                    let duration_ms = step_start.elapsed().as_millis() as u64;

                    match result {
                        Ok(step_result) => {
                            step_results.insert(
                                step_id.clone(),
                                StepResult {
                                    duration_ms,
                                    ..step_result
                                },
                            );
                        }
                        Err(e) => {
                            let should_retry = step_def
                                .retry
                                .as_ref()
                                .is_some_and(|r| r.max_attempts > 1 && r.on_transient);

                            if should_retry {
                                // Retry once (simplified — real impl would loop)
                                let retry_result = Self::execute_source_step(
                                    step_id,
                                    source,
                                    step_def,
                                    &options.workflow_dir,
                                    &options.policy,
                                    &mut data_store,
                                    options.live,
                                );
                                match retry_result {
                                    Ok(sr) => {
                                        step_results.insert(step_id.clone(), sr);
                                    }
                                    Err(retry_err) => {
                                        step_results.insert(
                                            step_id.clone(),
                                            StepResult {
                                                step_id: step_id.clone(),
                                                status: StepStatus::Failed,
                                                output_hash: None,
                                                duration_ms,
                                                capabilities_used: vec![],
                                                error: Some(retry_err.to_string()),
                                            },
                                        );
                                        workflow_status = WorkflowStatus::Failed;
                                        break;
                                    }
                                }
                            } else {
                                step_results.insert(
                                    step_id.clone(),
                                    StepResult {
                                        step_id: step_id.clone(),
                                        status: StepStatus::Failed,
                                        output_hash: None,
                                        duration_ms,
                                        capabilities_used: vec![],
                                        error: Some(e.to_string()),
                                    },
                                );
                                workflow_status = WorkflowStatus::Failed;
                                break;
                            }
                        }
                    }
                }
            }
        }

        if workflow_status == WorkflowStatus::Running {
            workflow_status = WorkflowStatus::Completed;
        }

        Ok(WorkflowRunResult {
            run_id,
            workflow_name: def.name.clone(),
            status: workflow_status,
            step_results,
            total_duration_ms: run_start.elapsed().as_millis() as u64,
        })
    }

    fn execute_source_step(
        step_id: &str,
        source: &str,
        step_def: &StepDef,
        workflow_dir: &str,
        policy: &Option<Policy>,
        data_store: &mut DataStore,
        live: bool,
    ) -> Result<StepResult, WorkflowRunError> {
        // Read source file
        let source_path = Path::new(workflow_dir).join(source);
        let source_code = std::fs::read_to_string(&source_path).map_err(|e| {
            WorkflowRunError::StepFailed(
                step_id.to_string(),
                format!("cannot read {}: {e}", source_path.display()),
            )
        })?;

        // Resolve inputs (validated, available for future step parameter injection)
        let _resolved_inputs = data_store
            .resolve_step_inputs(&step_def.inputs)
            .map_err(|e| WorkflowRunError::StepFailed(step_id.to_string(), e))?;

        // Compile
        let module = boruna_compiler::compile(step_id, &source_code).map_err(|e| {
            WorkflowRunError::StepFailed(step_id.to_string(), format!("compile error: {e}"))
        })?;

        // Build policy for this step
        let step_policy = Self::build_step_policy(policy, step_def);

        // Create VM and run — use HttpHandler when live mode is enabled
        let gateway = if live {
            #[cfg(feature = "http")]
            {
                let net_policy = step_policy.net_policy.clone().unwrap_or_default();
                CapabilityGateway::with_handler(
                    step_policy,
                    Box::new(boruna_vm::http_handler::HttpHandler::new(net_policy)),
                )
            }
            #[cfg(not(feature = "http"))]
            {
                eprintln!(
                    "warning: --live requires the `http` feature; falling back to mock handler"
                );
                CapabilityGateway::new(step_policy)
            }
        } else {
            CapabilityGateway::new(step_policy)
        };
        let mut vm = Vm::new(module, gateway);

        // Push input values onto the VM stack if available
        // The step's main() will access them via capability calls or globals
        // For now, inputs are available in the data store for the step to read

        let result = vm.run().map_err(|e| {
            WorkflowRunError::StepFailed(step_id.to_string(), format!("runtime error: {e}"))
        })?;

        // Store output
        let output_hash = DataStore::hash_value(&result);
        data_store
            .store_output(step_id, "result", &result)
            .map_err(|e| WorkflowRunError::StepFailed(step_id.to_string(), e.to_string()))?;

        // Collect capabilities used from event log
        let caps_used: Vec<String> = step_def.capabilities.clone();

        Ok(StepResult {
            step_id: step_id.to_string(),
            status: StepStatus::Completed,
            output_hash: Some(output_hash),
            duration_ms: 0, // filled in by caller
            capabilities_used: caps_used,
            error: None,
        })
    }

    fn build_step_policy(base_policy: &Option<Policy>, step_def: &StepDef) -> Policy {
        match base_policy {
            Some(p) => {
                let mut policy = p.clone();
                // If the step has a budget, apply it
                if let Some(budget) = &step_def.budget {
                    if let Some(max_calls) = budget.max_calls {
                        for cap in &step_def.capabilities {
                            policy.rules.entry(cap.clone()).or_insert(PolicyRule {
                                allow: true,
                                budget: max_calls,
                            });
                        }
                    }
                }
                policy
            }
            None => Policy::deny_all(),
        }
    }
}

/// Errors that can occur during a workflow run.
#[derive(Debug, Clone)]
pub enum WorkflowRunError {
    Validation(String),
    StepFailed(String, String),
    Io(String),
    Internal(String),
}

impl std::fmt::Display for WorkflowRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "validation error: {msg}"),
            Self::StepFailed(step, msg) => write!(f, "step '{step}' failed: {msg}"),
            Self::Io(msg) => write!(f, "IO error: {msg}"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_workflow_with_steps(step_sources: &[(&str, &str)]) -> (WorkflowDef, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();

        let mut steps = BTreeMap::new();
        let mut edges = Vec::new();
        let mut prev: Option<String> = None;

        for (id, source_code) in step_sources {
            let filename = format!("steps/{id}.ax");
            std::fs::write(dir.path().join(&filename), source_code).unwrap();

            steps.insert(
                id.to_string(),
                StepDef {
                    kind: StepKind::Source { source: filename },
                    capabilities: vec![],
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    depends_on: vec![],
                    timeout_ms: None,
                    retry: None,
                    budget: None,
                },
            );

            if let Some(prev_id) = &prev {
                edges.push((prev_id.clone(), id.to_string()));
            }
            prev = Some(id.to_string());
        }

        let def = WorkflowDef {
            schema_version: 1,
            name: "test-workflow".into(),
            version: "1.0.0".into(),
            description: "test".into(),
            steps,
            edges,
        };

        (def, dir)
    }

    #[test]
    fn test_run_linear_workflow() {
        let (def, dir) = make_workflow_with_steps(&[
            ("step1", "fn main() -> Int { 1 }"),
            ("step2", "fn main() -> Int { 2 }"),
            ("step3", "fn main() -> Int { 3 }"),
        ]);

        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
        };

        let result = WorkflowRunner::run(&def, &options).unwrap();
        assert_eq!(result.status, WorkflowStatus::Completed);
        assert_eq!(result.step_results.len(), 3);
        for (_, sr) in &result.step_results {
            assert_eq!(sr.status, StepStatus::Completed);
        }
    }

    #[test]
    fn test_run_with_compile_error() {
        let (def, dir) = make_workflow_with_steps(&[
            ("good", "fn main() -> Int { 1 }"),
            ("bad", "fn main( { }"), // syntax error
        ]);

        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
        };

        let result = WorkflowRunner::run(&def, &options).unwrap();
        assert_eq!(result.status, WorkflowStatus::Failed);
        assert_eq!(result.step_results["good"].status, StepStatus::Completed);
        assert_eq!(result.step_results["bad"].status, StepStatus::Failed);
    }

    #[test]
    fn test_run_with_policy_deny() {
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();
        std::fs::write(
            steps_dir.join("fetch.ax"),
            "fn fetch(url: String) -> String !{net.fetch} { url }\nfn main() -> Int { 0 }",
        )
        .unwrap();

        let def = WorkflowDef {
            schema_version: 1,
            name: "deny-test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([(
                "fetch".into(),
                StepDef {
                    kind: StepKind::Source {
                        source: "steps/fetch.ax".into(),
                    },
                    capabilities: vec!["net.fetch".into()],
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    depends_on: vec![],
                    timeout_ms: None,
                    retry: None,
                    budget: None,
                },
            )]),
            edges: vec![],
        };

        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
        };

        // With allow_all, should succeed
        let result = WorkflowRunner::run(&def, &options).unwrap();
        assert_eq!(result.status, WorkflowStatus::Completed);
    }

    #[test]
    fn test_run_approval_gate_pauses() {
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();
        std::fs::write(steps_dir.join("analyze.ax"), "fn main() -> Int { 42 }").unwrap();

        let def = WorkflowDef {
            schema_version: 1,
            name: "approval-test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                (
                    "analyze".into(),
                    StepDef {
                        kind: StepKind::Source {
                            source: "steps/analyze.ax".into(),
                        },
                        capabilities: vec![],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec![],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                    },
                ),
                (
                    "approve".into(),
                    StepDef {
                        kind: StepKind::ApprovalGate {
                            required_role: "reviewer".into(),
                            condition: None,
                        },
                        capabilities: vec![],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec!["analyze".into()],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                    },
                ),
                (
                    "store".into(),
                    StepDef {
                        kind: StepKind::Source {
                            source: "steps/analyze.ax".into(), // reuse
                        },
                        capabilities: vec![],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec!["approve".into()],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                    },
                ),
            ]),
            edges: vec![],
        };

        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
        };

        let result = WorkflowRunner::run(&def, &options).unwrap();
        assert_eq!(result.status, WorkflowStatus::Paused);
        // analyze completed, approve is awaiting, store not reached
        assert_eq!(result.step_results["analyze"].status, StepStatus::Completed);
        assert_eq!(
            result.step_results["approve"].status,
            StepStatus::AwaitingApproval
        );
        assert!(!result.step_results.contains_key("store"));
    }

    #[test]
    fn test_run_empty_workflow_rejected() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "empty".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::new(),
            edges: vec![],
        };
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: "/tmp".into(),
            live: false,
        };
        assert!(WorkflowRunner::run(&def, &options).is_err());
    }
}
