use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
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
    /// Capability policy: 'allow-all' or 'deny-all' (default: 'allow-all')
    policy: Option<String>,
    /// Maximum execution steps (default: 10000000)
    max_steps: Option<u64>,
    /// Enable opcode-level execution trace (default: false)
    trace: Option<bool>,
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
        description = "Compile and execute .ax source code. Returns the result value, UI output, step count, and optionally an execution trace. Domain errors (compile failures, runtime errors, step limit exceeded) are returned as JSON with success=false."
    )]
    async fn boruna_run(
        &self,
        Parameters(params): Parameters<RunParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_source(&params.source)?;
        let source = params.source;
        let policy = params.policy.unwrap_or_else(|| "allow-all".into());
        let max_steps = params.max_steps.unwrap_or(10_000_000);
        let trace = params.trace.unwrap_or(false);
        let result = tokio::task::spawn_blocking(move || {
            tools::run::run_source(&source, &policy, max_steps, trace)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("task join error: {e}"), None))?;
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
