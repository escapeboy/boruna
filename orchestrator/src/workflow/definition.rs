use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Highest workflow-DAG schema major version this build understands.
///
/// See `docs/spec/workflow-dag-1.0.md` for the formal contract.
/// Forward-compat: a 1.x reader accepts any 1.y workflow (additive
/// fields are ignored, never required). A `schema_version >= 2`
/// document is rejected — those readers must come from a future
/// build that knows the new shape.
pub const WORKFLOW_DAG_SCHEMA_VERSION: u32 = 1;

/// Typed errors from `WorkflowDef::from_json` (sprint W4).
///
/// Surface contract:
/// - `MissingSchemaVersion` → `error_kind: workflow.missing_schema_version`
/// - `UnsupportedSchemaVersion` → `error_kind: workflow.unsupported_schema_version`
/// - `InvalidJson` → `error_kind: workflow.invalid_json`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowParseError {
    /// `schema_version` field absent. Reject per spec §1 (no
    /// silent default — legacy fixtures must be migrated).
    MissingSchemaVersion,
    /// `schema_version` major exceeds what this build supports.
    UnsupportedSchemaVersion { found: u32, supported_max: u32 },
    /// JSON failed to parse, or required structural fields are
    /// missing / wrong-typed.
    InvalidJson(String),
}

impl WorkflowParseError {
    /// Stable string for surfacing over MCP / HTTP per project
    /// conventions §2.
    pub fn error_kind(&self) -> &'static str {
        match self {
            Self::MissingSchemaVersion => "workflow.missing_schema_version",
            Self::UnsupportedSchemaVersion { .. } => "workflow.unsupported_schema_version",
            Self::InvalidJson(_) => "workflow.invalid_json",
        }
    }
}

impl std::fmt::Display for WorkflowParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSchemaVersion => write!(
                f,
                "workflow.json is missing required field `schema_version` \
                 (expected `\"schema_version\": {WORKFLOW_DAG_SCHEMA_VERSION}`); \
                 see docs/spec/workflow-dag-1.0.md"
            ),
            Self::UnsupportedSchemaVersion {
                found,
                supported_max,
            } => write!(
                f,
                "workflow.json `schema_version: {found}` is not supported \
                 by this build (max supported: {supported_max}); upgrade \
                 Boruna to read this workflow"
            ),
            Self::InvalidJson(msg) => write!(f, "invalid workflow.json: {msg}"),
        }
    }
}

impl std::error::Error for WorkflowParseError {}

/// A workflow definition — a DAG of steps with typed data flow.
///
/// Persistent shape: `workflow.json` on disk. Spec frozen at sprint
/// W4: see `docs/spec/workflow-dag-1.0.md`. The `schema_version`
/// field is REQUIRED on every workflow.json. The custom
/// `Deserialize` impl rejects missing or unsupported versions with
/// `WorkflowParseError`.
///
/// Forward-compat: 1.x readers accept any 1.y workflow (additive
/// fields tolerated by serde defaults / `#[serde(default)]`). Do
/// NOT add `#[serde(deny_unknown_fields)]`.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowDef {
    pub schema_version: u32,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub steps: BTreeMap<String, StepDef>,
    pub edges: Vec<(String, String)>,
}

impl WorkflowDef {
    /// Parse a workflow.json document. Rejects missing or
    /// unsupported `schema_version` with a typed error per
    /// project-conventions §1 (reject at parse, not later).
    pub fn from_json(json: &str) -> Result<Self, WorkflowParseError> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| WorkflowParseError::InvalidJson(e.to_string()))?;

        // Gate 1: `schema_version` MUST be present and an integer.
        let sv = match value.get("schema_version") {
            None => return Err(WorkflowParseError::MissingSchemaVersion),
            Some(serde_json::Value::Null) => return Err(WorkflowParseError::MissingSchemaVersion),
            Some(v) => v.as_u64().ok_or_else(|| {
                WorkflowParseError::InvalidJson(
                    "field `schema_version` must be a non-negative integer".into(),
                )
            })?,
        };
        let sv: u32 = sv
            .try_into()
            .map_err(|_| WorkflowParseError::UnsupportedSchemaVersion {
                found: u32::MAX,
                supported_max: WORKFLOW_DAG_SCHEMA_VERSION,
            })?;

        // Gate 2: major version bound. 1.x readers accept any 1.y;
        // 2+ is a hard reject (future format).
        if sv > WORKFLOW_DAG_SCHEMA_VERSION {
            return Err(WorkflowParseError::UnsupportedSchemaVersion {
                found: sv,
                supported_max: WORKFLOW_DAG_SCHEMA_VERSION,
            });
        }

        // Gate 3: parse the rest. Forward-compat: unknown fields
        // are ignored (no `deny_unknown_fields`).
        serde_json::from_value::<Self>(value)
            .map_err(|e| WorkflowParseError::InvalidJson(e.to_string()))
    }
}

// Custom `Deserialize` so every existing call site
// (`serde_json::from_str::<WorkflowDef>(_)`) goes through the same
// gate — including the persistence layer's metadata blob and the
// CLI's workflow.json reader. The error message is folded into a
// generic `serde::de::Error` because callers read the textual form;
// the typed `WorkflowParseError` is reachable via `from_json`.
impl<'de> Deserialize<'de> for WorkflowDef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        // Mirror of the public struct without the custom
        // Deserialize, used purely for the structural decode after
        // schema_version is gated.
        #[derive(Deserialize)]
        struct Raw {
            schema_version: Option<u32>,
            name: String,
            version: String,
            #[serde(default)]
            description: String,
            steps: BTreeMap<String, StepDef>,
            edges: Vec<(String, String)>,
        }

        let raw = Raw::deserialize(deserializer)?;
        let sv = raw.schema_version.ok_or_else(|| {
            D::Error::custom(WorkflowParseError::MissingSchemaVersion.to_string())
        })?;
        if sv > WORKFLOW_DAG_SCHEMA_VERSION {
            return Err(D::Error::custom(
                WorkflowParseError::UnsupportedSchemaVersion {
                    found: sv,
                    supported_max: WORKFLOW_DAG_SCHEMA_VERSION,
                }
                .to_string(),
            ));
        }
        Ok(WorkflowDef {
            schema_version: sv,
            name: raw.name,
            version: raw.version,
            description: raw.description,
            steps: raw.steps,
            edges: raw.edges,
        })
    }
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
    /// Minimum capability version requirements for distributed-execution
    /// workers claiming this step. Format: `{"llm.call": "2.0"}`.
    /// A worker must advertise >= this version for each listed capability
    /// to be eligible to claim the step. Workers without an explicit
    /// version declaration for a capability default to "1.0".
    #[serde(default)]
    pub required_capability_versions: BTreeMap<String, String>,
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
///
/// **Retry semantics (sprint `0.4-S8`):**
/// 1. If `retry_on` is non-empty, the step is retried ONLY when the
///    failure's error class is in the allowlist.
/// 2. If `retry_on` is empty, fall back to the legacy `on_transient`
///    gate: `true` retries on any failure, `false` is single-attempt.
///
/// `retry_on` accepts the class strings defined in
/// [`crate::workflow::runner::error_class`] (e.g.
/// `"wall_time_exceeded"`, `"runtime_error"`, `"capability_denied"`).
/// Unknown strings are silently ignored — they never match a failure
/// class — so an operator typo means "do not retry on that class"
/// rather than a hard parse error. This is conservative-by-default.
///
/// `max_attempts` applies as the upper bound regardless of which
/// gate fires; `max_attempts <= 1` means single-attempt always.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    #[serde(default)]
    pub on_transient: bool,
    /// Sprint 0.4-S8: explicit error-class allowlist. When non-empty,
    /// the retry loop retries only when the step failure's class is
    /// in this list. See struct-level docs for the fallback semantics
    /// when this is empty.
    #[serde(default)]
    pub retry_on: Vec<String>,
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
