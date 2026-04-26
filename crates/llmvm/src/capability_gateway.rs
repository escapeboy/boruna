use boruna_bytecode::{Capability, Value};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::VmError;
use crate::replay::EventLog;

/// Policy rule for a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    pub allow: bool,
    /// Maximum invocations allowed (0 = unlimited).
    pub budget: u64,
}

impl Default for PolicyRule {
    fn default() -> Self {
        PolicyRule {
            allow: true,
            budget: 0,
        }
    }
}

/// Network-specific policy controls for HTTP capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetPolicy {
    /// Allowed domains (e.g. ["api.example.com", "*.googleapis.com"]). Empty = all.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Allowed HTTP methods (e.g. ["GET", "POST"]). Empty = all.
    #[serde(default)]
    pub allowed_methods: Vec<String>,
    /// Maximum response body size in bytes (default 10 MB).
    #[serde(default = "default_max_response")]
    pub max_response_bytes: usize,
    /// Request timeout in milliseconds (default 30000).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Whether to follow redirects (default true).
    #[serde(default = "default_true")]
    pub allow_redirects: bool,
}

fn default_max_response() -> usize {
    10 * 1024 * 1024
}

fn default_timeout() -> u64 {
    30_000
}

fn default_true() -> bool {
    true
}

impl Default for NetPolicy {
    fn default() -> Self {
        NetPolicy {
            allowed_domains: Vec::new(),
            allowed_methods: Vec::new(),
            max_response_bytes: default_max_response(),
            timeout_ms: default_timeout(),
            allow_redirects: true,
        }
    }
}

/// Policy configuration for the capability gateway.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Policy {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub rules: BTreeMap<String, PolicyRule>,
    /// Default rule for capabilities not explicitly listed.
    pub default_allow: bool,
    /// Network-specific policy controls (for NetFetch capability).
    #[serde(default)]
    pub net_policy: Option<NetPolicy>,
}

fn default_schema_version() -> u32 {
    1
}

impl Policy {
    /// Create a permissive policy that allows everything.
    pub fn allow_all() -> Self {
        Policy {
            schema_version: 1,
            rules: BTreeMap::new(),
            default_allow: true,
            net_policy: None,
        }
    }

    /// Create a deny-all policy.
    pub fn deny_all() -> Self {
        Policy::default()
    }

    /// Allow a specific capability with an optional budget.
    pub fn allow(&mut self, cap: &Capability, budget: u64) -> &mut Self {
        self.rules.insert(
            cap.name().to_string(),
            PolicyRule {
                allow: true,
                budget,
            },
        );
        self
    }

    /// Deny a specific capability.
    pub fn deny(&mut self, cap: &Capability) -> &mut Self {
        self.rules.insert(
            cap.name().to_string(),
            PolicyRule {
                allow: false,
                budget: 0,
            },
        );
        self
    }
}

/// Capability gateway — all side effects go through here.
pub struct CapabilityGateway {
    policy: Policy,
    usage: BTreeMap<String, u64>,
    /// Host-provided handler for capability calls.
    handler: Box<dyn CapabilityHandler>,
}

/// Trait for host-provided capability implementations.
pub trait CapabilityHandler: Send {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String>;
}

/// Default handler that returns mock values (for testing / sandbox).
pub struct MockHandler;

impl CapabilityHandler for MockHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::TimeNow => Ok(Value::Int(1700000000)),
            Capability::Random => Ok(Value::Float(0.42)),
            Capability::NetFetch => {
                let url = args.first().map(|v| format!("{v}")).unwrap_or_default();
                Ok(Value::String(format!(
                    "{{\"mock\": true, \"url\": \"{url}\"}}"
                )))
            }
            Capability::FsRead => {
                let path = args.first().map(|v| format!("{v}")).unwrap_or_default();
                Ok(Value::String(format!("mock file content for {path}")))
            }
            Capability::FsWrite => Ok(Value::Bool(true)),
            Capability::DbQuery => Ok(Value::List(vec![])),
            Capability::UiRender => Ok(Value::Unit),
            Capability::LlmCall => {
                // Mock LLM returns a structured JSON object
                let mut result = std::collections::BTreeMap::new();
                result.insert("status".into(), Value::String("ok".into()));
                result.insert("mock".into(), Value::Bool(true));
                Ok(Value::Map(result))
            }
            Capability::ActorSpawn | Capability::ActorSend => {
                // Actor ops are handled at the opcode level, not through the gateway
                Ok(Value::Unit)
            }
            Capability::StepInput => {
                // 0.3-S14: the MockHandler returns an empty string for
                // step.input — the real implementation lives in the
                // orchestrator's StepInputHandler which wraps this
                // mock and serves resolved upstream outputs. When no
                // wrapping handler is installed (e.g. ephemeral .ax
                // runs invoked via `boruna run`), step_input returns
                // empty so steps don't crash.
                Ok(Value::String(String::new()))
            }
        }
    }
}

/// Handler that serves `step.input` capability calls from a
/// runner-provided map of resolved upstream outputs. All other
/// capabilities delegate to a wrapped inner handler (typically
/// `MockHandler` or a BYOH live handler).
///
/// Introduced in `0.3-S14`. The orchestrator constructs one of these
/// per step execution, populating `inputs` from the workflow
/// definition's `inputs: { name: "upstream.output" }` declarations
/// resolved against the data store. The .ax step body calls
/// `step_input("name")` which compiles to `Op::CapCall(StepInput, 1)`
/// and dispatches here, returning the JSON-encoded upstream value
/// as a `Value::String`.
///
/// **Type contract:** the language layer is String-to-String.
/// Upstream outputs are JSON-encoded compactly (matching
/// `DataStore::hash_value`'s serialization). Downstream steps that
/// need typed access parse the JSON inline.
///
/// **Determinism:** input values come from the persisted upstream
/// step's output. Same inputs → same outputs across machines and
/// runs.
///
/// **Unknown input names error** (project-conventions §1
/// reject-at-parse). The runner pre-validates declared inputs in
/// `workflow.json`, so a `step_input("missing_name")` call from a
/// .ax step is unambiguously a bug — typo, refactor lag, or stale
/// step source. Surfacing as a typed `Err` propagates through the
/// VM as a runtime error; the operator sees a clean failure
/// instead of silent-empty data corruption downstream.
pub struct StepInputHandler {
    inputs: BTreeMap<String, Value>,
    inner: Box<dyn CapabilityHandler>,
}

impl StepInputHandler {
    pub fn new(inputs: BTreeMap<String, Value>, inner: Box<dyn CapabilityHandler>) -> Self {
        Self { inputs, inner }
    }
}

impl CapabilityHandler for StepInputHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        if matches!(cap, Capability::StepInput) {
            let name = match args.first() {
                Some(Value::String(s)) => s.as_str(),
                _ => return Err("step.input: expected String argument (input name)".to_string()),
            };
            // Look up the resolved upstream Value, then JSON-encode
            // for the language-level String contract. The encoding
            // matches `DataStore::hash_value` (compact JSON) so an
            // operator running `sha256sum` on the embedded payload
            // matches what the persistence layer wrote.
            match self.inputs.get(name) {
                Some(value) => match serde_json::to_string(value) {
                    Ok(json) => Ok(Value::String(json)),
                    Err(e) => Err(format!("step.input: serialize '{name}': {e}")),
                },
                None => {
                    // 0.3-S14 review: surface unknown-name as a typed
                    // error instead of silent empty (project-
                    // conventions §1). The runner validates declared
                    // inputs before dispatch; a name not in the map
                    // is unambiguously a .ax-source bug.
                    let mut declared: Vec<&str> = self.inputs.keys().map(String::as_str).collect();
                    declared.sort();
                    Err(format!(
                        "step.input: unknown name '{name}' (declared inputs: {declared:?})"
                    ))
                }
            }
        } else {
            self.inner.handle(cap, args)
        }
    }
}

/// Replay handler that returns values from a recorded log.
pub struct ReplayHandler {
    events: Vec<Value>,
    cursor: usize,
}

impl ReplayHandler {
    pub fn new(events: Vec<Value>) -> Self {
        ReplayHandler { events, cursor: 0 }
    }
}

impl CapabilityHandler for ReplayHandler {
    fn handle(&mut self, _cap: &Capability, _args: &[Value]) -> Result<Value, String> {
        if self.cursor < self.events.len() {
            let val = self.events[self.cursor].clone();
            self.cursor += 1;
            Ok(val)
        } else {
            Err("replay log exhausted".into())
        }
    }
}

impl CapabilityGateway {
    pub fn new(policy: Policy) -> Self {
        CapabilityGateway {
            policy,
            usage: BTreeMap::new(),
            handler: Box::new(MockHandler),
        }
    }

    /// Get the policy (for cloning into child actors).
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn with_handler(policy: Policy, handler: Box<dyn CapabilityHandler>) -> Self {
        CapabilityGateway {
            policy,
            usage: BTreeMap::new(),
            handler,
        }
    }

    /// Execute a capability call with policy enforcement.
    ///
    /// **Telemetry:** wraps the call body in a `tracing::info_span!` named
    /// `boruna.cap` (the `cap.name` field carries the specific capability).
    /// When no subscriber is installed (the default), the span macros are
    /// essentially no-ops. The `telemetry` feature on `boruna-vm` adds an
    /// OpenTelemetry exporter that consumes these spans; see
    /// [`crate::telemetry`] and `docs/design-otel.md`.
    ///
    /// **Determinism contract (per ADR 001):** span attributes are
    /// operational metadata only — never feed an `EventLog`, `AuditLog`, or
    /// `EvidenceBundle`. Capability args are NOT included in attributes
    /// (privacy + size); only their cumulative byte count is.
    pub fn call(
        &mut self,
        cap: &Capability,
        args: &[Value],
        log: &mut EventLog,
    ) -> Result<Value, VmError> {
        let name = cap.name();
        let bytes_in = approx_bytes(args);

        // Span open. Empty fields are filled via Span::record below.
        let span = tracing::info_span!(
            "boruna.cap",
            cap.name = name,
            bytes_in = bytes_in,
            bytes_out = tracing::field::Empty,
            cap.budget_remaining = tracing::field::Empty,
            error.kind = tracing::field::Empty,
        );
        let _enter = span.enter();

        // Check policy
        let rule = self.policy.rules.get(name);
        let allowed = match rule {
            Some(r) => r.allow,
            None => self.policy.default_allow,
        };
        if !allowed {
            span.record("error.kind", "denied");
            return Err(VmError::CapabilityDenied(*cap));
        }

        // Check budget. The `cap.budget_remaining` attribute records the
        // **post-call** quota — number of calls still allowed AFTER this one
        // is counted. So `cap.budget_remaining=0` means "this was the last
        // permitted call". On the rejection path, also `0`, but distinguished
        // by `error.kind=budget_exceeded`. Operators querying traces should
        // join on (cap.budget_remaining, error.kind) to disambiguate.
        let count = self.usage.entry(name.to_string()).or_insert(0);
        *count += 1;
        if let Some(r) = rule {
            if r.budget > 0 {
                if *count > r.budget {
                    span.record("error.kind", "budget_exceeded");
                    span.record("cap.budget_remaining", 0u64);
                    return Err(VmError::CapabilityBudgetExceeded(*cap));
                }
                span.record("cap.budget_remaining", r.budget.saturating_sub(*count));
            }
        }

        // Log the call (replay-verified state)
        log.log_cap_call(cap, args);

        // Invoke handler
        let result = match self.handler.handle(cap, args) {
            Ok(v) => v,
            Err(e) => {
                span.record("error.kind", "runtime_error");
                return Err(VmError::AssertionFailed(format!("capability error: {e}")));
            }
        };

        // Log the result (replay-verified state)
        log.log_cap_result(cap, &result);

        // Operational telemetry: record output size on the span.
        span.record("bytes_out", approx_value_bytes(&result));

        Ok(result)
    }

    pub fn usage(&self) -> &BTreeMap<String, u64> {
        &self.usage
    }
}

/// Best-effort byte-count estimate for telemetry attributes only.
///
/// Recurses through every container variant (`List`, `Map`, `Record`,
/// `Enum`, `Some`/`Ok`/`Err`) so that `bytes_out` for record-returning
/// capabilities (the dominant shape for `db.query` and `llm.call`) is
/// not structurally zero. Counts UTF-8 byte length of every embedded
/// `String`. Numeric/Bool/Unit/None/ActorId/FnRef contribute 0 — they're
/// fixed-size and not the payload story we're trying to surface.
///
/// This is OPERATIONAL metadata only; never feed it into an audit hash.
fn approx_value_bytes(value: &Value) -> u64 {
    match value {
        Value::String(s) => s.len() as u64,
        Value::List(items) => items.iter().map(approx_value_bytes).sum(),
        Value::Map(entries) => entries
            .iter()
            .map(|(k, v)| k.len() as u64 + approx_value_bytes(v))
            .sum(),
        Value::Record { fields, .. } => fields.iter().map(approx_value_bytes).sum(),
        Value::Enum { payload, .. } => approx_value_bytes(payload),
        Value::Some(v) | Value::Ok(v) | Value::Err(v) => approx_value_bytes(v),
        // Numeric / Bool / Unit / None / ActorId / FnRef: fixed-size, no
        // string payload to surface in operational telemetry. Contribute 0.
        _ => 0,
    }
}

/// Cumulative byte count of `String` content reachable from the args slice
/// (recursively, through containers). See `approx_value_bytes`.
fn approx_bytes(args: &[Value]) -> u64 {
    args.iter().map(approx_value_bytes).sum()
}
