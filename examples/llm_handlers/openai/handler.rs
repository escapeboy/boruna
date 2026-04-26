//! Reference `CapabilityHandler` impl for OpenAI Chat Completions.
//!
//! This is a reference for the BYOH integration pattern documented in
//! `docs/guides/llm-integration.md` — NOT a production handler. Fork and
//! harden for your own provider, secret management, observability, and
//! routing concerns.

use boruna_bytecode::{Capability, Value};
use boruna_vm::capability_gateway::{CapabilityHandler, MockHandler};

/// LLM handler that routes `llm.call` to OpenAI's Chat Completions API.
///
/// All other capabilities (`fs.read`, `time.now`, etc.) fall through to
/// `MockHandler`. Production handlers should compose with their full
/// capability surface, not just llm.call.
pub struct OpenAiHandler {
    api_key: String,
    model: String,
    client: ureq::Agent,
}

impl OpenAiHandler {
    /// Construct from `OPENAI_API_KEY` env var. Defaults to
    /// `gpt-4o-mini`. For deterministic runs, pin to a dated model
    /// (`gpt-4o-mini-2024-07-18`) and pass `temperature: 0` in your
    /// request body.
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| "OPENAI_API_KEY not set".to_string())?;
        Ok(Self {
            api_key,
            model: "gpt-4o-mini".to_string(),
            client: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(60))
                .build(),
        })
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
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

        let body = serde_json::json!({
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            // Pin temperature for replay-friendliness. Override in
            // your fork if you need sampling diversity.
            "temperature": 0,
        });

        let resp = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| format!("openai request failed: {e}"))?;

        let json: serde_json::Value = resp
            .into_json()
            .map_err(|e| format!("openai response parse: {e}"))?;
        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| "openai response missing choices[0].message.content".to_string())?;

        Ok(Value::String(content.to_string()))
    }
}

impl CapabilityHandler for OpenAiHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::LlmCall => self.handle_llm_call(args),
            _ => MockHandler.handle(cap, args),
        }
    }
}
