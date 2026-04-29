//! Reference `CapabilityHandler` impl for Anthropic Messages API.
//!
//! This is a reference for the BYOH integration pattern documented in
//! `docs/guides/llm-integration.md` — NOT a production handler. Fork and
//! harden for your own provider, secret management, observability, and
//! routing concerns.

use boruna_bytecode::{Capability, Value};
use boruna_vm::capability_gateway::{CapabilityHandler, MockHandler};

/// LLM handler that routes `llm.call` to Anthropic's Messages API.
///
/// All other capabilities (`fs.read`, `time.now`, etc.) fall through to
/// `MockHandler`. Production handlers should compose with their full
/// capability surface, not just llm.call.
pub struct AnthropicHandler {
    api_key: String,
    model: String,
    max_tokens: u32,
    client: ureq::Agent,
}

impl AnthropicHandler {
    /// Construct from `ANTHROPIC_API_KEY` env var. Defaults to the
    /// latest Sonnet snapshot — pin to a dated model for replay
    /// reproducibility.
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| "ANTHROPIC_API_KEY not set".to_string())?;
        Ok(Self {
            api_key,
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            client: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(60))
                .build(),
        })
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    fn handle_llm_call(&self, args: &[Value]) -> Result<Value, String> {
        let prompt = args
            .first()
            .and_then(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .ok_or_else(|| "llm.call: first arg must be a String prompt".to_string())?;

        // Anthropic's Messages API requires `max_tokens`. There is no
        // "unlimited" option; pick an upper bound that fits your
        // workload's longest expected response.
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": [{"role": "user", "content": prompt}],
            // Pin temperature for replay-friendliness. Override in
            // your fork if you need sampling diversity.
            "temperature": 0,
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            // Anthropic uses `x-api-key`, NOT `Authorization: Bearer`.
            .set("x-api-key", &self.api_key)
            // The version header is required and pins the wire shape.
            // Keep this in sync with your prompt assumptions.
            .set("anthropic-version", "2023-06-01")
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| format!("anthropic request failed: {e}"))?;

        let json: serde_json::Value = resp
            .into_json()
            .map_err(|e| format!("anthropic response parse: {e}"))?;
        // Anthropic returns `content: [{type: "text", text: "..."}]`
        // (a list of content blocks). Most calls return a single text
        // block; we extract `content[0].text`. If your prompt reliably
        // produces tool-use blocks, branch on `content[i].type`.
        let content = json
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| "anthropic response missing content[0].text".to_string())?;

        Ok(Value::String(content.to_string()))
    }
}

impl CapabilityHandler for AnthropicHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::LlmCall => self.handle_llm_call(args),
            _ => MockHandler.handle(cap, args),
        }
    }
}
