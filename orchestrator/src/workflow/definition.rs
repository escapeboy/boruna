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

/// The kind of step.
///
/// - `Source` — compile and run an `.ax` file.
/// - `ApprovalGate` — pause the run until an operator records an
///   approval/rejection via `boruna workflow approve` (sprint 0.3-S2c).
/// - `ExternalTrigger` — pause the run until an external event arrives
///   via `boruna workflow trigger <run-id> <step-id>` (sprint 0.3-S15).
///   Designed for webhook-driven workflows: the operator's webhook
///   receiver bridges to the CLI. The trigger payload becomes the
///   step's output value, available to downstream steps via
///   `step_input`. Boruna stays a CLI tool — no in-binary HTTP
///   server. See `docs/design-0.3-s15-external-trigger.md`.
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
    #[serde(rename = "external_trigger")]
    ExternalTrigger {
        /// Optional human-readable description of what event the
        /// workflow expects (e.g. "Stripe payment.succeeded webhook").
        /// Surfaced in `boruna workflow show` and operator logs;
        /// purely informational, not enforced.
        #[serde(default)]
        description: Option<String>,
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
    /// Number of attempts the step took to reach its terminal state.
    /// `1` = first-try success or single-attempt failure; `>1` = retry
    /// policy fired. Operational only — depends on whether transient
    /// failures happened. Defaults to `1` when deserialized from
    /// pre-`0.3-S11` JSON for back-compat. Mirror of
    /// `step_checkpoints.attempt_count` in the persistent store.
    #[serde(default = "default_attempt_count")]
    pub attempt_count: u32,
}

fn default_attempt_count() -> u32 {
    1
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
    /// Step is paused waiting for an external event (sprint 0.3-S15).
    /// Resume after `boruna workflow trigger <run-id> <step-id>` to
    /// advance with the trigger payload as the step's output.
    AwaitingExternalEvent,
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
