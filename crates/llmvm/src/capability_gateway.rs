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

/// Multi-provider router for `Capability::LlmCall` (sprint `0.4-S13`).
///
/// Wraps a registry of provider handlers (one per LLM backend —
/// OpenAI, Anthropic, Ollama, vLLM, custom) and dispatches each
/// `Capability::LlmCall` to the right one based on a `provider/model`
/// prefix in `args[1]`. Non-LLM capability calls delegate to a
/// fallback handler so the router composes with the existing
/// `StepInputHandler` / `MockHandler` / `HttpHandler` stack.
///
/// **Why this lives in core but doesn't ship provider implementations:**
/// per the BYOH decision in `0.3-S8` (see `docs/guides/llm-integration.md`),
/// Boruna does not ship default LLM handlers. Each provider's HTTP
/// client, auth, and response-shape parsing belong to the integrator
/// and follow their own release cadence. The router is pure routing
/// logic — it adds no provider compatibility commitments to core.
///
/// **Routing convention:** `args[1]` is parsed as `provider/model`
/// (e.g. `openai/gpt-4`, `anthropic/claude-3-5-sonnet-20241022`,
/// `ollama/llama3:8b`). The portion before the first `/` selects the
/// provider; the full string (including provider prefix) is forwarded
/// to the provider's handler unchanged so providers can still use
/// the model name internally.
///
/// **Conventions on the LLM call shape:** `args[0]` is the prompt
/// (per the integration guide); `args[1]` is the routing key. Providers
/// are free to interpret remaining args as they like.
///
/// # Minimal example
///
/// ```ignore
/// use std::collections::BTreeMap;
/// use boruna_vm::capability_gateway::{
///     CapabilityHandler, LlmRouterHandler, MockHandler,
/// };
///
/// let mut providers: BTreeMap<String, Box<dyn CapabilityHandler>> = BTreeMap::new();
/// providers.insert("openai".into(), Box::new(my_openai_handler));
/// providers.insert("anthropic".into(), Box::new(my_anthropic_handler));
///
/// let router = LlmRouterHandler::new(providers, Box::new(MockHandler));
///
/// // .ax code: let response = llm_call("Summarize:", "openai/gpt-4")
/// // → router parses "openai/gpt-4" → routes to my_openai_handler
/// ```
pub struct LlmRouterHandler {
    providers: BTreeMap<String, Box<dyn CapabilityHandler>>,
    fallback: Box<dyn CapabilityHandler>,
}

impl LlmRouterHandler {
    /// Construct a router with a pre-built provider registry and a
    /// fallback handler for non-LLM capability calls.
    pub fn new(
        providers: BTreeMap<String, Box<dyn CapabilityHandler>>,
        fallback: Box<dyn CapabilityHandler>,
    ) -> Self {
        LlmRouterHandler {
            providers,
            fallback,
        }
    }

    /// Register an additional provider after construction. Returns
    /// the previously-registered handler if `name` was already in the
    /// registry — caller chooses whether to log, panic, or chain.
    pub fn add_provider(
        &mut self,
        name: &str,
        handler: Box<dyn CapabilityHandler>,
    ) -> Option<Box<dyn CapabilityHandler>> {
        self.providers.insert(name.to_string(), handler)
    }

    /// List the registered provider names. Useful for surfacing a
    /// helpful error message when an `.ax` step references an
    /// unknown provider.
    pub fn registered_providers(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}

impl CapabilityHandler for LlmRouterHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        // Non-LLM calls pass through to the fallback unchanged.
        // The router is specifically about multi-provider LLM
        // dispatch; it shouldn't impose semantics on net.fetch /
        // db.query / etc.
        if !matches!(cap, Capability::LlmCall) {
            return self.fallback.handle(cap, args);
        }

        // args[0] is the prompt (per the BYOH convention); args[1]
        // is the routing key. A 0-arg or 1-arg LlmCall has no
        // provider hint — surface a typed error rather than
        // silently picking a default (no good default exists when
        // multiple providers are registered).
        let model_arg = args.get(1).ok_or_else(|| {
            "llm router: llm.call requires a model name as args[1] in \
             'provider/model' format (e.g. 'openai/gpt-4')"
                .to_string()
        })?;
        let model_str = match model_arg {
            Value::String(s) => s.as_str(),
            other => {
                return Err(format!(
                    "llm router: args[1] (model) must be a String, got {}",
                    other.type_name()
                ));
            }
        };
        let provider_name = match model_str.split_once('/') {
            Some((provider, _model)) if !provider.is_empty() => provider,
            _ => {
                return Err(format!(
                    "llm router: model '{model_str}' must be in 'provider/model' \
                     format (e.g. 'openai/gpt-4'). Registered providers: {:?}",
                    self.registered_providers()
                ));
            }
        };

        match self.providers.get_mut(provider_name) {
            Some(handler) => handler.handle(cap, args),
            None => Err(format!(
                "llm router: unknown provider '{provider_name}' (registered: {:?})",
                self.registered_providers()
            )),
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

#[cfg(test)]
mod llm_router_tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Test handler that records every call and returns a tagged
    /// response so tests can verify which provider was hit.
    struct RecordingHandler {
        tag: String,
        calls: std::sync::Arc<std::sync::Mutex<Vec<(Capability, Vec<Value>)>>>,
    }

    impl CapabilityHandler for RecordingHandler {
        fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
            self.calls
                .lock()
                .unwrap()
                .push((cap.clone(), args.to_vec()));
            Ok(Value::String(format!("response-from-{}", self.tag)))
        }
    }

    fn make_recorder(
        tag: &str,
    ) -> (
        Box<dyn CapabilityHandler>,
        std::sync::Arc<std::sync::Mutex<Vec<(Capability, Vec<Value>)>>>,
    ) {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let handler = RecordingHandler {
            tag: tag.to_string(),
            calls: calls.clone(),
        };
        (Box::new(handler), calls)
    }

    fn make_router_with_two_providers() -> (
        LlmRouterHandler,
        std::sync::Arc<std::sync::Mutex<Vec<(Capability, Vec<Value>)>>>,
        std::sync::Arc<std::sync::Mutex<Vec<(Capability, Vec<Value>)>>>,
    ) {
        let (openai_handler, openai_calls) = make_recorder("openai");
        let (anthropic_handler, anthropic_calls) = make_recorder("anthropic");
        let mut providers: BTreeMap<String, Box<dyn CapabilityHandler>> = BTreeMap::new();
        providers.insert("openai".to_string(), openai_handler);
        providers.insert("anthropic".to_string(), anthropic_handler);
        let router = LlmRouterHandler::new(providers, Box::new(MockHandler));
        (router, openai_calls, anthropic_calls)
    }

    #[test]
    fn routes_to_openai_provider_when_model_starts_with_openai_slash() {
        let (mut router, openai_calls, anthropic_calls) = make_router_with_two_providers();
        let result = router.handle(
            &Capability::LlmCall,
            &[
                Value::String("Summarize:".into()),
                Value::String("openai/gpt-4".into()),
            ],
        );
        assert_eq!(
            result.unwrap(),
            Value::String("response-from-openai".into())
        );
        assert_eq!(openai_calls.lock().unwrap().len(), 1);
        assert_eq!(anthropic_calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn routes_to_anthropic_provider_when_model_starts_with_anthropic_slash() {
        let (mut router, openai_calls, anthropic_calls) = make_router_with_two_providers();
        router
            .handle(
                &Capability::LlmCall,
                &[
                    Value::String("Summarize:".into()),
                    Value::String("anthropic/claude-3-5-sonnet-20241022".into()),
                ],
            )
            .unwrap();
        assert_eq!(openai_calls.lock().unwrap().len(), 0);
        assert_eq!(anthropic_calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn forwards_full_args_to_provider_unchanged() {
        // The provider handler must receive the prompt + the full
        // model string (with provider prefix), not the model name
        // alone — providers may want to use the prefix internally
        // for telemetry / billing tagging.
        let (mut router, openai_calls, _) = make_router_with_two_providers();
        router
            .handle(
                &Capability::LlmCall,
                &[
                    Value::String("Summarize:".into()),
                    Value::String("openai/gpt-4".into()),
                    Value::Float(0.7),
                ],
            )
            .unwrap();
        let calls = openai_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (_, args) = &calls[0];
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], Value::String("Summarize:".into()));
        assert_eq!(args[1], Value::String("openai/gpt-4".into()));
        assert_eq!(args[2], Value::Float(0.7));
    }

    #[test]
    fn errors_on_unknown_provider() {
        let (mut router, _, _) = make_router_with_two_providers();
        let err = router
            .handle(
                &Capability::LlmCall,
                &[
                    Value::String("hello".into()),
                    Value::String("unknown/model".into()),
                ],
            )
            .expect_err("unknown provider must error");
        assert!(err.contains("unknown provider 'unknown'"));
        // Error message should list registered providers for triage.
        assert!(err.contains("openai"));
        assert!(err.contains("anthropic"));
    }

    #[test]
    fn errors_on_missing_model_arg() {
        let (mut router, _, _) = make_router_with_two_providers();
        let err = router
            .handle(&Capability::LlmCall, &[Value::String("hello".into())])
            .expect_err("missing model arg must error");
        assert!(err.contains("requires a model name as args[1]"));
    }

    #[test]
    fn errors_when_model_arg_is_not_a_string() {
        let (mut router, _, _) = make_router_with_two_providers();
        let err = router
            .handle(
                &Capability::LlmCall,
                &[Value::String("hello".into()), Value::Int(42)],
            )
            .expect_err("non-string model arg must error");
        assert!(err.contains("args[1] (model) must be a String"));
    }

    #[test]
    fn errors_on_malformed_model_string_no_slash() {
        let (mut router, _, _) = make_router_with_two_providers();
        let err = router
            .handle(
                &Capability::LlmCall,
                &[
                    Value::String("hello".into()),
                    Value::String("just-a-model-name".into()),
                ],
            )
            .expect_err("missing slash must error");
        assert!(err.contains("'provider/model' format"));
    }

    #[test]
    fn errors_on_empty_provider_prefix() {
        // "/gpt-4" has an empty provider prefix — not a valid lookup.
        let (mut router, _, _) = make_router_with_two_providers();
        let err = router
            .handle(
                &Capability::LlmCall,
                &[
                    Value::String("hello".into()),
                    Value::String("/gpt-4".into()),
                ],
            )
            .expect_err("empty provider prefix must error");
        assert!(err.contains("'provider/model' format"));
    }

    #[test]
    fn non_llm_calls_pass_through_to_fallback() {
        // A non-LLM capability call must hit the fallback handler,
        // not error and not consult the provider registry. The
        // fallback in the test setup is MockHandler, which returns
        // canned values for each capability.
        let (mut router, openai_calls, anthropic_calls) = make_router_with_two_providers();
        let result = router
            .handle(&Capability::TimeNow, &[])
            .expect("non-LLM call must succeed via fallback");
        // MockHandler returns Int(1700000000) for TimeNow.
        assert_eq!(result, Value::Int(1700000000));
        // Provider handlers were NOT consulted.
        assert_eq!(openai_calls.lock().unwrap().len(), 0);
        assert_eq!(anthropic_calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn add_provider_replaces_existing_and_returns_old() {
        // The router supports late registration / replacement. The
        // returned Option<Box<dyn ...>> lets callers chain or
        // explicitly drop the previous handler.
        let (mut router, _, _) = make_router_with_two_providers();
        let (replacement, replacement_calls) = make_recorder("openai-v2");
        let prior = router.add_provider("openai", replacement);
        assert!(
            prior.is_some(),
            "replacing a provider must return the prior handler"
        );

        // After replacement, calls route to the new handler.
        router
            .handle(
                &Capability::LlmCall,
                &[
                    Value::String("hi".into()),
                    Value::String("openai/gpt-4".into()),
                ],
            )
            .unwrap();
        assert_eq!(replacement_calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn registered_providers_returns_lexicographic_order() {
        // BTreeMap keys are sorted, so registered_providers() output
        // is deterministic. Lock that contract — tests that match
        // on error-message provider lists rely on it.
        let (router, _, _) = make_router_with_two_providers();
        assert_eq!(router.registered_providers(), vec!["anthropic", "openai"]);
    }
}
