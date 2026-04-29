//! Reference `CapabilityHandler` impl for vLLM (and any other
//! OpenAI-compatible endpoint: OpenRouter, Together, Groq,
//! self-hosted SGLang, LiteLLM proxy, etc.).
//!
//! This is a reference for the BYOH integration pattern documented in
//! `docs/guides/llm-integration.md` — NOT a production handler.

use boruna_bytecode::{Capability, Value};
use boruna_vm::capability_gateway::{CapabilityHandler, MockHandler};

/// LLM handler that routes `llm.call` to a vLLM (or any other
/// OpenAI-compatible) endpoint.
///
/// **The wire shape is identical to the OpenAI handler;** the only
/// real differences are the base URL and that the API key may be
/// optional (self-hosted vLLM often runs without auth). If you're
/// pointing at OpenAI itself, prefer the dedicated `openai/` example.
///
/// All other capabilities fall through to `MockHandler`.
pub struct VllmHandler {
    base_url: String,
    api_key: Option<String>,
    model: String,
    client: ureq::Agent,
}

impl VllmHandler {
    /// Construct from env. `VLLM_BASE_URL` is required (e.g.
    /// `http://localhost:8000/v1` for a local vLLM, or
    /// `https://openrouter.ai/api/v1` for OpenRouter).
    /// `VLLM_API_KEY` is optional — set when the endpoint requires
    /// auth (most managed proxies do; self-hosted vLLM does not by
    /// default).
    pub fn from_env() -> Result<Self, String> {
        let base_url = std::env::var("VLLM_BASE_URL")
            .map_err(|_| "VLLM_BASE_URL not set (e.g. http://localhost:8000/v1)".to_string())?;
        let api_key = std::env::var("VLLM_API_KEY").ok();
        Ok(Self {
            base_url,
            api_key,
            // No safe default — the operator must specify the model
            // their endpoint actually serves. Fail loudly via the
            // upstream 404 if the caller forgets.
            model: String::new(),
            client: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(120))
                .build(),
        })
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    fn handle_llm_call(&self, args: &[Value]) -> Result<Value, String> {
        if self.model.is_empty() {
            return Err("vllm: no model configured; call .with_model(...)".into());
        }

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
            "temperature": 0,
        });

        let url = format!(
            "{}/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let mut req = self
            .client
            .post(&url)
            .set("Content-Type", "application/json");
        if let Some(ref key) = self.api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }
        let resp = req
            .send_json(body)
            .map_err(|e| format!("vllm request failed: {e}"))?;

        let json: serde_json::Value = resp
            .into_json()
            .map_err(|e| format!("vllm response parse: {e}"))?;
        // OpenAI-compatible response shape — same path as the OpenAI
        // handler.
        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| "vllm response missing choices[0].message.content".to_string())?;

        Ok(Value::String(content.to_string()))
    }
}

impl CapabilityHandler for VllmHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::LlmCall => self.handle_llm_call(args),
            _ => MockHandler.handle(cap, args),
        }
    }
}
