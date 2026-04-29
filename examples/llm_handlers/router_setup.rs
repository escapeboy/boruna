//! Reference: load `providers.toml` and build an `LlmRouterHandler`.
//!
//! This is a SNIPPET, not a compiled module. It demonstrates the
//! convention `providers.toml.example` documents. Copy into your
//! integrator crate, adapt for your handler set, and call
//! `build_router(...)` at startup.
//!
//! Required deps in your fork:
//!
//! ```toml
//! [dependencies]
//! toml = "0.8"
//! serde = { version = "1", features = ["derive"] }
//! boruna-vm = { path = "..." }
//! boruna-bytecode = { path = "..." }
//! # Plus whichever provider crates the handlers you ship need.
//! ```

use std::collections::BTreeMap;

use boruna_vm::capability_gateway::{CapabilityHandler, LlmRouterHandler, MockHandler};

// In your fork, replace these `mod` lines with the real handler
// modules you copy in.
mod openai {
    pub struct OpenAiHandler;
    impl OpenAiHandler {
        pub fn from_env() -> Result<Self, String> {
            unimplemented!("paste examples/llm_handlers/openai/handler.rs")
        }
        pub fn with_model(self, _: impl Into<String>) -> Self {
            self
        }
    }
    impl boruna_vm::capability_gateway::CapabilityHandler for OpenAiHandler {
        fn handle(
            &mut self,
            _: &boruna_bytecode::Capability,
            _: &[boruna_bytecode::Value],
        ) -> Result<boruna_bytecode::Value, String> {
            unimplemented!()
        }
    }
}
mod anthropic {
    pub struct AnthropicHandler;
    impl AnthropicHandler {
        pub fn from_env() -> Result<Self, String> {
            unimplemented!("paste examples/llm_handlers/anthropic/handler.rs")
        }
        pub fn with_model(self, _: impl Into<String>) -> Self {
            self
        }
        pub fn with_max_tokens(self, _: u32) -> Self {
            self
        }
    }
    impl boruna_vm::capability_gateway::CapabilityHandler for AnthropicHandler {
        fn handle(
            &mut self,
            _: &boruna_bytecode::Capability,
            _: &[boruna_bytecode::Value],
        ) -> Result<boruna_bytecode::Value, String> {
            unimplemented!()
        }
    }
}
mod ollama {
    pub struct OllamaHandler;
    impl OllamaHandler {
        pub fn from_env() -> Result<Self, String> {
            unimplemented!("paste examples/llm_handlers/ollama/handler.rs")
        }
        pub fn with_host(self, _: impl Into<String>) -> Self {
            self
        }
        pub fn with_model(self, _: impl Into<String>) -> Self {
            self
        }
    }
    impl boruna_vm::capability_gateway::CapabilityHandler for OllamaHandler {
        fn handle(
            &mut self,
            _: &boruna_bytecode::Capability,
            _: &[boruna_bytecode::Value],
        ) -> Result<boruna_bytecode::Value, String> {
            unimplemented!()
        }
    }
}
mod vllm {
    pub struct VllmHandler;
    impl VllmHandler {
        pub fn from_env() -> Result<Self, String> {
            unimplemented!("paste examples/llm_handlers/vllm/handler.rs")
        }
        pub fn with_model(self, _: impl Into<String>) -> Self {
            self
        }
        pub fn with_api_key(self, _: impl Into<String>) -> Self {
            self
        }
    }
    impl boruna_vm::capability_gateway::CapabilityHandler for VllmHandler {
        fn handle(
            &mut self,
            _: &boruna_bytecode::Capability,
            _: &[boruna_bytecode::Value],
        ) -> Result<boruna_bytecode::Value, String> {
            unimplemented!()
        }
    }
}

// Suggested toml shape — matches `providers.toml.example`. Unknown
// keys (e.g. `rate_limit` sub-tables) are ignored at parse time so
// integrators can extend the file without forking the parser.
#[derive(Debug, serde::Deserialize)]
struct ProvidersConfig {
    providers: BTreeMap<String, ProviderEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct ProviderEntry {
    kind: String,
    /// Env var name holding the API key. None for handlers that
    /// don't need auth (Ollama, self-hosted vLLM without auth).
    api_key_env: Option<String>,
    /// Endpoint override — used by `kind = "ollama"` (`host`) and
    /// `kind = "vllm"` (`base_url`). The toml shape uses two
    /// different keys for clarity in config; we accept either here.
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    model: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
    // (timeout_secs is read but not propagated in this skeleton —
    // each handler's `from_env` constructs its own ureq client. In
    // your fork, add `with_timeout_secs(...)` builders to each
    // handler if you want declarative timeouts.)
}

/// Parse `providers.toml` content and build an `LlmRouterHandler`
/// with one entry per `[providers.*]` block. Unknown `kind`s
/// surface as an `Err` rather than a silent skip — prevents an
/// operator from believing their `kind = "anthropci"` typo is
/// being honored.
pub fn build_router(toml_text: &str) -> Result<LlmRouterHandler, String> {
    let cfg: ProvidersConfig = toml::from_str(toml_text)
        .map_err(|e| format!("providers.toml parse error: {e}"))?;

    let mut providers: BTreeMap<String, Box<dyn CapabilityHandler>> = BTreeMap::new();
    for (name, entry) in cfg.providers {
        // Resolve the api key from the named env var (if any). We
        // do this here rather than in each handler's `from_env` so
        // the toml controls *which* env var to read; this matters
        // when an integrator runs two openai-compatible providers
        // with different keys (e.g. an OpenAI account + an
        // OpenRouter account).
        let api_key = entry.api_key_env.as_ref().and_then(|var| {
            std::env::var(var).ok()
        });

        let handler: Box<dyn CapabilityHandler> = match entry.kind.as_str() {
            "openai" => {
                let mut h = openai::OpenAiHandler::from_env()?;
                if let Some(m) = entry.model {
                    h = h.with_model(m);
                }
                let _ = api_key; // OpenAI handler reads OPENAI_API_KEY internally
                Box::new(h)
            }
            "anthropic" => {
                let mut h = anthropic::AnthropicHandler::from_env()?;
                if let Some(m) = entry.model {
                    h = h.with_model(m);
                }
                if let Some(mt) = entry.max_tokens {
                    h = h.with_max_tokens(mt);
                }
                Box::new(h)
            }
            "ollama" => {
                let mut h = ollama::OllamaHandler::from_env()?;
                if let Some(host) = entry.host.or(entry.base_url) {
                    h = h.with_host(host);
                }
                if let Some(m) = entry.model {
                    h = h.with_model(m);
                }
                Box::new(h)
            }
            "vllm" => {
                // VLLM_BASE_URL must be set in env for from_env;
                // if the toml provides base_url, set it in the env
                // before calling from_env, OR build with a builder
                // method. (Keeping this skeleton simple: assume env.)
                let mut h = vllm::VllmHandler::from_env()?;
                if let Some(m) = entry.model {
                    h = h.with_model(m);
                }
                if let Some(k) = api_key {
                    h = h.with_api_key(k);
                }
                Box::new(h)
            }
            "bedrock" => {
                // Bedrock skeleton requires the AWS SDK — see
                // examples/llm_handlers/bedrock/handler.rs. This
                // branch is a placeholder; in your fork, paste the
                // real handler in and instantiate it here.
                return Err(format!(
                    "kind = \"bedrock\" requires the AWS SDK; see \
                     examples/llm_handlers/bedrock/README.md"
                ));
            }
            other => {
                return Err(format!(
                    "unknown provider kind '{other}' for entry '{name}' \
                     (known: openai, anthropic, ollama, vllm, bedrock)"
                ));
            }
        };
        providers.insert(name, handler);
    }

    Ok(LlmRouterHandler::new(providers, Box::new(MockHandler)))
}

// In your binary's main():
//
// fn main() -> Result<(), Box<dyn std::error::Error>> {
//     let toml_text = std::fs::read_to_string("providers.toml")?;
//     let router = build_router(&toml_text)?;
//     let runner = boruna_orchestrator::WorkflowRunner::new(/* ... */)
//         .with_handler(Box::new(router));
//     runner.run()?;
//     Ok(())
// }
