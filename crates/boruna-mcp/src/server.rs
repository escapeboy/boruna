use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools;

const MAX_SOURCE_SIZE: usize = 1_048_576; // 1 MB

/// Validate source input size and return an McpError if too large.
fn validate_source(source: &str) -> Result<(), McpError> {
    if source.len() > MAX_SOURCE_SIZE {
        return Err(McpError::invalid_params("source exceeds 1 MB limit", None));
    }
    Ok(())
}

// ── Parameter structs ──

#[derive(Serialize, Deserialize, JsonSchema)]
struct CompileParams {
    /// The .ax source code to compile
    source: String,
    /// Module name (defaults to 'module')
    name: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct AstParams {
    /// The .ax source code to parse
    source: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct RunParams {
    /// The .ax source code to run
    source: String,
    /// Capability policy. Either:
    ///   - String shorthand "allow-all" or "deny-all" (default: "allow-all")
    ///   - A Policy object — see docs/reference/policy-schema.md for the schema and examples
    ///
    /// Unknown strings or unparseable objects return success=false, error_kind="invalid_policy".
    #[serde(default)]
    policy: Option<serde_json::Value>,
    /// Maximum execution steps (default: 10000000). Deterministic ceiling.
    max_steps: Option<u64>,
    /// Enable opcode-level execution trace (default: false)
    trace: Option<bool>,
    /// Optional structured resource limits.
    /// Hitting any returns success=false, error_kind="limit_exceeded",
    /// limit_kind="<wall_ms|output_bytes>". See docs/reference/mcp-server.md.
    #[serde(default)]
    limits: Option<RunLimitsParams>,
    /// Optional JSON Schema 2020-12 object. When set, the script's `result`
    /// is validated against the schema after a successful run. A failure
    /// returns success=false, error_kind="validation_failed", phase=
    /// "output_validation", with per-path errors. A malformed schema returns
    /// error_kind="invalid_output_schema". See docs/design-output-schema.md.
    #[serde(default)]
    output_schema: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, JsonSchema, Default)]
struct RunLimitsParams {
    /// Wall-clock execution limit in milliseconds. Operational guardrail —
    /// non-deterministic on overrun (fast vs. slow host). For deterministic
    /// limits use `max_steps`.
    #[serde(default)]
    max_wall_ms: Option<u64>,
    /// Maximum cumulative serialized output size in bytes (covers `result`
    /// and `ui_output`). Aborts during serialization once exceeded.
    #[serde(default)]
    max_output_bytes: Option<u64>,
    /// Reserved for a future release; **not enforced in 0.3.x**. Accepted in
    /// the schema so integrators can wire it into their UIs today; the value
    /// is ignored by the runtime. Process-level cgroups/ulimits remain the
    /// supported way to bound memory until this lands.
    #[serde(default)]
    max_memory_mb: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct CheckParams {
    /// The .ax source code to check
    source: String,
    /// File name for diagnostic locations (default: '<source>')
    file_name: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct RepairParams {
    /// The .ax source code to repair
    source: String,
    /// File name for diagnostics (default: '<source>')
    file_name: Option<String>,
    /// Repair strategy: 'best' (default) or 'all'
    strategy: Option<String>,
    /// Apply a specific patch by ID (overrides strategy)
    patch_id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct ValidateAppParams {
    /// The .ax source code to validate
    source: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct FrameworkTestParams {
    /// The .ax framework app source code
    source: String,
    /// Messages to send as 'tag:payload' strings (e.g. ['increment:1', 'reset:0'])
    messages: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct WorkflowValidateParams {
    /// The workflow.json content as a string
    workflow_json: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct TemplateApplyParams {
    /// Template name (e.g. 'crud-admin', 'form-basic')
    template_name: String,
    /// Template arguments as key=value pairs (e.g. ['entity_name=products', 'fields=name|price'])
    args: Vec<String>,
    /// Validate that the generated source compiles (default: false)
    validate: Option<bool>,
}

// ── Server ──

#[derive(Clone)]
pub struct BorunaMcpServer {
    tool_router: ToolRouter<Self>,
    templates_dir: String,
    #[allow(dead_code)]
    libs_dir: String,
}

#[tool_router]
impl BorunaMcpServer {
    pub fn new(templates_dir: String, libs_dir: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            templates_dir,
            libs_dir,
        }
    }

    // ── Compile Tools ──

    #[tool(
        description = "Compile .ax source code and return module info (function count, type count, constants) or structured compile errors with line/col spans."
    )]
    async fn boruna_compile(
        &self,
        Parameters(params): Parameters<CompileParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_source(&params.source)?;
        let source = params.source;
        let name = params.name.unwrap_or_else(|| "module".into());
        let result =
            tokio::task::spawn_blocking(move || tools::compile::compile_source(&source, &name))
                .await
                .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(
        description = "Parse .ax source code and return the AST as JSON. Large ASTs are truncated at 100KB."
    )]
    async fn boruna_ast(
        &self,
        Parameters(params): Parameters<AstParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_source(&params.source)?;
        let source = params.source;
        let result = tokio::task::spawn_blocking(move || tools::compile::parse_ast(&source))
            .await
            .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    // ── Run Tool ──

    #[tool(
        description = "Compile and execute .ax source code under a capability policy. The `policy` parameter accepts either the string shorthand 'allow-all' / 'deny-all' OR a structured Policy object (per-capability allow/budget rules, allowlist vs. denylist mode, and a NetPolicy with allowed_domains / methods / byte limits / timeout) — see docs/reference/policy-schema.md for the full schema and examples. The optional `limits` object enforces structured resource limits (max_wall_ms, max_output_bytes; max_memory_mb is reserved for a future release) — overruns return success=false, error_kind='limit_exceeded' with a `limit_kind` discriminator. Returns the result value, UI output, step count, and optionally an execution trace. Domain errors (compile failures, runtime errors, step limit exceeded, invalid_policy, limit_exceeded) are returned as JSON with success=false."
    )]
    async fn boruna_run(
        &self,
        Parameters(params): Parameters<RunParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        validate_source(&params.source)?;
        let source = params.source;
        let policy = params.policy;
        let max_steps = params.max_steps.unwrap_or(10_000_000);
        let trace = params.trace.unwrap_or(false);
        let limits = params.limits.map(|l| tools::run::RunLimits {
            max_wall_ms: l.max_wall_ms,
            max_output_bytes: l.max_output_bytes,
            max_memory_mb: l.max_memory_mb,
        });
        let output_schema = params.output_schema;

        // 0.4-S6: streaming progress notifications. When the caller
        // includes a `progressToken` in the request's `_meta` field
        // (per MCP spec), the VM is driven via execute_bounded() in
        // 100k-step slices and emits a `notifications/progress` event
        // between slices. Without a progressToken the legacy sync path
        // runs unchanged — backward compatible.
        let progress_token = ctx.meta.get_progress_token();
        let result = if let Some(token) = progress_token {
            // Bridge the blocking VM thread to the async notify_progress
            // call via an unbounded mpsc channel. Sends from the VM
            // never block; the async forwarder task drains and posts to
            // the peer. When the blocking task completes, dropping the
            // sender closes the channel and the forwarder ends.
            //
            // **`unbounded` is intentional** — a single VM emits at
            // most max_steps / PROGRESS_STEP_SLICE samples (10M / 100k =
            // 100 by default), so the queue is shallow and a bounded
            // channel adds backpressure complexity for no real benefit.
            // If max_steps grew unboundedly this would need revisiting.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
            let peer = ctx.peer.clone();
            let token_clone = token.clone();
            let forwarder = tokio::spawn(async move {
                while let Some(steps) = rx.recv().await {
                    // Best-effort delivery: a notify failure (e.g. the
                    // client disconnected mid-run) is logged-and-
                    // continued. The VM keeps running and will surface
                    // its own terminal result.
                    // `total: None` — `max_steps` is a SAFETY ceiling
                    // (default 10M), not an expected step count, so
                    // setting `total` to it would make percentage UIs
                    // show ~0-2% for typical programs that complete in
                    // well under the cap. With None, MCP clients
                    // treat `progress` as a monotonic count without a
                    // percentage interpretation. Reviewed in 0.4-S6.
                    let _ = peer
                        .notify_progress(ProgressNotificationParam {
                            progress_token: token_clone.clone(),
                            progress: steps as f64,
                            total: None,
                            message: None,
                        })
                        .await;
                }
            });

            let result = tokio::task::spawn_blocking(move || {
                tools::run::run_source_with_progress(
                    &source,
                    policy.as_ref(),
                    max_steps,
                    trace,
                    limits.as_ref(),
                    output_schema.as_ref(),
                    Some(move |steps: u64| {
                        // Channel is closed only after the blocking
                        // task drops its sender, which happens after
                        // run_source_with_progress returns. Send
                        // failures here are unreachable in practice
                        // but ignored if they ever occur.
                        let _ = tx.send(steps);
                    }),
                )
            })
            .await
            .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;

            // Wait for the forwarder to drain any remaining samples
            // before returning. spawn_blocking has already finished, so
            // the sender side is closed and the forwarder will exit
            // promptly.
            let _ = forwarder.await;
            result
        } else {
            tokio::task::spawn_blocking(move || {
                tools::run::run_source(
                    &source,
                    policy.as_ref(),
                    max_steps,
                    trace,
                    limits.as_ref(),
                    output_schema.as_ref(),
                )
            })
            .await
            .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?
        };
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    // ── Diagnostics Tools ──

    #[tool(
        description = "Run diagnostics on .ax source code. Returns structured diagnostics with severity, message, source location, and suggested patches."
    )]
    async fn boruna_check(
        &self,
        Parameters(params): Parameters<CheckParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_source(&params.source)?;
        let source = params.source;
        let file_name = params.file_name.unwrap_or_else(|| "<source>".into());
        let result =
            tokio::task::spawn_blocking(move || tools::check::check_source(&source, &file_name))
                .await
                .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(
        description = "Auto-repair .ax source code using diagnostic suggestions. Returns the repaired source and a report of applied/skipped patches."
    )]
    async fn boruna_repair(
        &self,
        Parameters(params): Parameters<RepairParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_source(&params.source)?;
        let source = params.source;
        let file_name = params.file_name.unwrap_or_else(|| "<source>".into());
        let strategy = params.strategy.unwrap_or_else(|| "best".into());
        let patch_id = params.patch_id;
        let result = tokio::task::spawn_blocking(move || {
            tools::check::repair_source(&source, &file_name, &strategy, patch_id.as_deref())
        })
        .await
        .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    // ── Framework Tools ──

    #[tool(
        description = "Validate that .ax source conforms to the Boruna App protocol (init/update/view functions). Returns which functions are present and their types."
    )]
    async fn boruna_validate_app(
        &self,
        Parameters(params): Parameters<ValidateAppParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_source(&params.source)?;
        let source = params.source;
        let result = tokio::task::spawn_blocking(move || tools::framework::validate_app(&source))
            .await
            .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(
        description = "Run a Boruna framework app by sending a sequence of messages. Returns init state, each cycle's state transition and effects, and the final view. Partial results are returned if a cycle fails."
    )]
    async fn boruna_framework_test(
        &self,
        Parameters(params): Parameters<FrameworkTestParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_source(&params.source)?;
        let source = params.source;
        let messages = params.messages;
        let result =
            tokio::task::spawn_blocking(move || tools::framework::test_app(&source, &messages))
                .await
                .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    // ── Workflow Tools ──

    #[tool(
        description = "Validate a workflow definition (JSON). Checks DAG structure, detects cycles, and returns topological execution order."
    )]
    async fn boruna_workflow_validate(
        &self,
        Parameters(params): Parameters<WorkflowValidateParams>,
    ) -> Result<CallToolResult, McpError> {
        let workflow_json = params.workflow_json;
        let result =
            tokio::task::spawn_blocking(move || tools::workflow::validate_workflow(&workflow_json))
                .await
                .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    // ── Template Tools ──

    #[tool(
        description = "List available Boruna app templates with their names, versions, descriptions, dependencies, and required capabilities."
    )]
    async fn boruna_template_list(&self) -> Result<CallToolResult, McpError> {
        let dir = self.templates_dir.clone();
        let result = tokio::task::spawn_blocking(move || tools::template::list_templates(&dir))
            .await
            .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    #[tool(
        description = "Apply a Boruna app template with variable substitution. Optionally validates that the output compiles."
    )]
    async fn boruna_template_apply(
        &self,
        Parameters(params): Parameters<TemplateApplyParams>,
    ) -> Result<CallToolResult, McpError> {
        let dir = self.templates_dir.clone();
        let template_name = params.template_name;
        let args = params.args;
        let validate = params.validate.unwrap_or(false);
        let result = tokio::task::spawn_blocking(move || {
            tools::template::apply_template(&dir, &template_name, &args, validate)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    // ── Capability Identity Tool ──

    #[tool(
        description = "List all capabilities this Boruna binary exposes, with a stable `capability_set_hash`. \
                       Use the hash as part of a cache key — `(source_hash, policy_hash, capability_set_hash)` — \
                       to safely memoize deterministic run results across binary upgrades. \
                       The hash changes only when the capability contract surface changes (new capability added, \
                       or an existing capability's argument/return shape changes). \
                       See docs/reference/capability-identity.md for the algorithm and caching recipe."
    )]
    async fn boruna_capability_list(&self) -> Result<CallToolResult, McpError> {
        let result = tokio::task::spawn_blocking(tools::capability::list_capabilities)
            .await
            .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }
}

#[tool_handler]
impl ServerHandler for BorunaMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "boruna-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "Boruna MCP server — compile, run, diagnose, and manage .ax programs. \
                 All tools accept .ax source code as strings and return structured JSON. \
                 Domain errors (compile failures, runtime errors) are returned as successful \
                 tool responses with success=false, not as MCP errors."
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}
