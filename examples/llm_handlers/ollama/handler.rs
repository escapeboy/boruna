//! Reference `CapabilityHandler` impl for Ollama (local-LLM runner).
//!
//! This is a reference for the BYOH integration pattern documented in
//! `docs/guides/llm-integration.md` — NOT a production handler. Fork
//! and harden as needed.

use boruna_bytecode::{Capability, Value};
use boruna_vm::capability_gateway::{CapabilityHandler, MockHandler};

/// LLM handler that routes `llm.call` to a local Ollama daemon.
///
/// Ollama listens on `http://localhost:11434` by default. Override
/// with `OLLAMA_HOST` env var (matches Ollama's own convention) or
/// [`OllamaHandler::with_host`].
///
/// All other capabilities fall through to `MockHandler`. Production
/// handlers should compose with their full capability surface, not
/// just llm.call.
pub struct OllamaHandler {
    host: String,
    model: String,
    client: ureq::Agent,
}

impl OllamaHandler {
    /// Construct from env. `OLLAMA_HOST` defaults to
    /// `http://localhost:11434`. Defaults model to `llama3.2:3b` —
    /// override with [`with_model`] for your specific model tag.
    pub fn from_env() -> Result<Self, String> {
        let host = std::env::var("OLLAMA_HOST")
            .unwrap_or_else(|_| "http://localhost:11434".to_string());
        Ok(Self {
            host,
            model: "llama3.2:3b".to_string(),
            client: ureq::AgentBuilder::new()
                // Local inference can be slow; bump the timeout
                // proportional to the largest model you serve.
                .timeout(std::time::Duration::from_secs(300))
                .build(),
        })
    }

    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
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

        // Use the /api/generate endpoint (single-prompt completion).
        // For multi-turn, switch to /api/chat with `messages: [...]`.
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            // Disable streaming so we get a single JSON response
            // body the synchronous handler can return in one shot.
            "stream": false,
            "options": {
                // Pin temperature for replay-friendliness.
                "temperature": 0,
                // Pin seed too — Ollama supports it. Removes the
                // last source of stochasticity for replay.
                "seed": 42,
            },
        });

        let url = format!("{}/api/generate", self.host.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| format!("ollama request failed: {e}"))?;

        let json: serde_json::Value = resp
            .into_json()
            .map_err(|e| format!("ollama response parse: {e}"))?;
        let content = json
            .get("response")
            .and_then(|c| c.as_str())
            .ok_or_else(|| "ollama response missing `response` field".to_string())?;

        Ok(Value::String(content.to_string()))
    }
}

impl CapabilityHandler for OllamaHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::LlmCall => self.handle_llm_call(args),
            _ => MockHandler.handle(cap, args),
        }
    }
}
