//! Reference `CapabilityHandler` skeleton for AWS Bedrock.
//!
//! Bedrock signs every request with AWS SigV4. Hand-rolling SigV4
//! is ~100 LOC of HMAC-SHA256 plumbing that adds nothing
//! illustrative, so this skeleton uses the official AWS SDK
//! (`aws-sdk-bedrockruntime`) instead. Adopt the pattern shown
//! here and adapt to your IAM / region / model choice.
//!
//! This is a reference for the BYOH integration pattern documented in
//! `docs/guides/llm-integration.md` — NOT a production handler.
//!
//! ## Required dependencies for your fork
//!
//! ```toml
//! [dependencies]
//! aws-config = { version = "1", features = ["behavior-version-latest"] }
//! aws-sdk-bedrockruntime = "1"
//! tokio = { version = "1", features = ["rt"] }
//! serde_json = "1"
//! ```
//!
//! ## Why a per-instance tokio runtime?
//!
//! `CapabilityHandler::handle` is sync; the AWS SDK is async. We
//! own a `tokio::runtime::Runtime` per `BedrockHandler` instance and
//! `block_on` each call. Same pattern Boruna's S3 / GCS / Azure
//! `BundleStorage` adapters use — see
//! `orchestrator/src/audit/storage_s3.rs` for the canonical
//! discussion.

use boruna_bytecode::{Capability, Value};
use boruna_vm::capability_gateway::{CapabilityHandler, MockHandler};

// NOTE: this snippet uses placeholder type names so the reference
// compiles in the user's mind without dragging the AWS SDK into
// Boruna's workspace. In your fork, replace these with the real
// `aws_sdk_bedrockruntime::Client` etc.
//
// Mark these `unimplemented!()` so a copy-paste user gets a clear
// "you need to wire this" message at runtime if they forget to
// substitute a real client.
struct BedrockClient;
impl BedrockClient {
    async fn invoke_model(&self, _model_id: &str, _body: Vec<u8>) -> Result<Vec<u8>, String> {
        unimplemented!(
            "replace BedrockClient with aws_sdk_bedrockruntime::Client \
             and call .invoke_model().model_id(...).body(...).send().await"
        )
    }
}

/// LLM handler that routes `llm.call` to AWS Bedrock's
/// `InvokeModel` API.
///
/// The wire shape varies by underlying model family. This skeleton
/// shows the **Anthropic-on-Bedrock** request shape (Claude models),
/// which is the most commonly-deployed combination. For Llama on
/// Bedrock, swap the request body for Llama's prompt format; for
/// Titan, swap to Titan's `inputText`+`textGenerationConfig`
/// shape. Bedrock's `InvokeModel` returns the model's raw
/// response body — your handler is responsible for parsing it.
pub struct BedrockHandler {
    client: BedrockClient,
    model_id: String,
    runtime: std::sync::Arc<tokio::runtime::Runtime>,
}

impl BedrockHandler {
    /// Construct from the standard AWS env / shared-config / IMDS
    /// credential chain. `BEDROCK_MODEL_ID` selects the model
    /// (e.g. `anthropic.claude-3-5-sonnet-20241022-v2:0`).
    ///
    /// In your fork:
    /// ```ignore
    /// let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
    ///     .load()
    ///     .await;
    /// let client = aws_sdk_bedrockruntime::Client::new(&config);
    /// ```
    pub fn from_env() -> Result<Self, String> {
        let model_id = std::env::var("BEDROCK_MODEL_ID")
            .map_err(|_| "BEDROCK_MODEL_ID not set".to_string())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("bedrock: failed to build tokio runtime: {e}"))?;
        Ok(Self {
            client: BedrockClient, // replace with real aws_sdk_bedrockruntime::Client
            model_id,
            runtime: std::sync::Arc::new(runtime),
        })
    }

    fn handle_llm_call(&self, args: &[Value]) -> Result<Value, String> {
        let prompt = args
            .first()
            .and_then(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .ok_or_else(|| "llm.call: first arg must be a String prompt".to_string())?;

        // Anthropic-on-Bedrock request shape. For other model
        // families, swap this body construction.
        let body = serde_json::json!({
            "anthropic_version": "bedrock-2023-05-31",
            "max_tokens": 4096,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0,
        });
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| format!("bedrock body serialize: {e}"))?;

        let resp_bytes = self.runtime.block_on(async {
            self.client.invoke_model(&self.model_id, body_bytes).await
        })?;

        let json: serde_json::Value = serde_json::from_slice(&resp_bytes)
            .map_err(|e| format!("bedrock response parse: {e}"))?;
        // Anthropic-on-Bedrock response: `{content: [{type, text}], ...}`
        // — same shape as the direct Anthropic API.
        let content = json
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| "bedrock response missing content[0].text".to_string())?;

        Ok(Value::String(content.to_string()))
    }
}

impl CapabilityHandler for BedrockHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::LlmCall => self.handle_llm_call(args),
            _ => MockHandler.handle(cap, args),
        }
    }
}
