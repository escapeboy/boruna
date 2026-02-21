use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn default_schema_version() -> u32 {
    1
}

/// A workflow definition — a DAG of steps with typed data flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub steps: BTreeMap<String, StepDef>,
    pub edges: Vec<(String, String)>,
}

/// Definition of a single workflow step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDef {
    #[serde(flatten)]
    pub kind: StepKind,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub retry: Option<RetryPolicy>,
    #[serde(default)]
    pub budget: Option<StepBudget>,
}

/// The kind of step — either source code execution or an approval gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StepKind {
    #[serde(rename = "source")]
    Source { source: String },
    #[serde(rename = "approval_gate")]
    ApprovalGate {
        required_role: String,
        #[serde(default)]
        condition: Option<String>,
    },
}

/// Retry policy for a step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    #[serde(default)]
    pub on_transient: bool,
}

/// Budget limits for a step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepBudget {
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_calls: Option<u64>,
}

/// Result of running a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_id: String,
    pub status: StepStatus,
    pub output_hash: Option<String>,
    pub duration_ms: u64,
    pub capabilities_used: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Status of a workflow step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    AwaitingApproval,
}

/// Overall status of a workflow run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Running,
    Completed,
    Failed,
    Paused,
}

/// Result of an entire workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunResult {
    pub run_id: String,
    pub workflow_name: String,
    pub status: WorkflowStatus,
    pub step_results: BTreeMap<String, StepResult>,
    pub total_duration_ms: u64,
}
