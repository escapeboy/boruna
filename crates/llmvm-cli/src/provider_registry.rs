//! LLM provider registry — config-driven handler selection
//! (post1/scheduler-registry-rolling).
//!
//! Loads a `providers.json` config file that maps capability names
//! (e.g. `"llm.call"`) to provider configurations. The registry
//! validates the config and provides a human-readable description
//! for logging. It does NOT instantiate actual HTTP handlers —
//! those require the `boruna-vm/http` feature and live in
//! `examples/llm_handlers/`. Full BYOH wiring is documented in
//! `docs/guides/llm-integration.md`.
//!
//! # Config file format (`providers.json`)
//!
//! ```json
//! {
//!   "llm.call": {
//!     "provider": "anthropic",
//!     "model": "claude-3-5-sonnet-20241022",
//!     "api_key_env": "ANTHROPIC_API_KEY"
//!   }
//! }
//! ```
//!
//! Supported providers: `anthropic`, `ollama`, `openai_compat`,
//! `deny` (blocks all calls with an error), `passthrough` (no-op,
//! returns empty string — useful for tests).

use std::collections::BTreeMap;

use serde::Deserialize;

/// Known provider identifiers.
const KNOWN_PROVIDERS: &[&str] = &[
    "anthropic",
    "ollama",
    "openai_compat",
    "deny",
    "passthrough",
];

/// Configuration for a single capability's LLM provider.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub provider: String,
    pub model: Option<String>,
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
}

/// Registry that maps capability names to provider configurations.
#[derive(Debug)]
pub struct ProviderRegistry {
    configs: BTreeMap<String, ProviderConfig>,
}

impl ProviderRegistry {
    /// Load a provider registry from a JSON file on disk.
    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_json(&content)
    }

    /// Parse a provider registry from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let raw: BTreeMap<String, ProviderConfig> =
            serde_json::from_str(json).map_err(|e| format!("invalid providers.json: {e}"))?;
        for (cap, cfg) in &raw {
            if !KNOWN_PROVIDERS.contains(&cfg.provider.as_str()) {
                return Err(format!(
                    "unknown provider {:?} for capability {cap:?}; \
                     expected one of: {}",
                    cfg.provider,
                    KNOWN_PROVIDERS.join(", ")
                ));
            }
        }
        Ok(Self { configs: raw })
    }

    /// Returns a human-readable description of the configured
    /// providers, safe for logging. Never includes actual API key
    /// values — only the environment variable NAME is shown.
    pub fn describe(&self) -> String {
        if self.configs.is_empty() {
            return "(no providers configured)".to_string();
        }
        let entries: Vec<String> = self
            .configs
            .iter()
            .map(|(cap, cfg)| {
                let model_part = cfg
                    .model
                    .as_deref()
                    .map(|m| format!(", model={m}"))
                    .unwrap_or_default();
                let key_part = cfg
                    .api_key_env
                    .as_deref()
                    .map(|k| format!(", key_env={k}"))
                    .unwrap_or_default();
                let url_part = cfg
                    .base_url
                    .as_deref()
                    .map(|u| format!(", base_url={u}"))
                    .unwrap_or_default();
                format!(
                    "{cap} -> {}{}{}{}",
                    cfg.provider, model_part, key_part, url_part
                )
            })
            .collect();
        entries.join("; ")
    }

    /// Returns the provider config for a given capability, if any.
    #[allow(dead_code)]
    pub fn get(&self, capability: &str) -> Option<&ProviderConfig> {
        self.configs.get(capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_valid_config() {
        let json = r#"{
            "llm.call": {
                "provider": "anthropic",
                "model": "claude-3-5-sonnet-20241022",
                "api_key_env": "ANTHROPIC_API_KEY"
            }
        }"#;
        let reg = ProviderRegistry::from_json(json).expect("valid config");
        let cfg = reg.get("llm.call").expect("llm.call present");
        assert_eq!(cfg.provider, "anthropic");
        assert_eq!(cfg.model.as_deref(), Some("claude-3-5-sonnet-20241022"));
        assert_eq!(cfg.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn load_invalid_provider_name() {
        let json = r#"{"llm.call": {"provider": "bogus_provider"}}"#;
        let err = ProviderRegistry::from_json(json).unwrap_err();
        assert!(err.contains("unknown provider"), "error: {err}");
    }

    #[test]
    fn load_missing_api_key_env() {
        // api_key_env is optional — the key may be set at runtime.
        let json =
            r#"{"llm.call": {"provider": "anthropic", "model": "claude-3-5-sonnet-20241022"}}"#;
        let reg = ProviderRegistry::from_json(json).expect("valid without api_key_env");
        assert!(reg.get("llm.call").unwrap().api_key_env.is_none());
    }

    #[test]
    fn describe_hides_secrets() {
        let json = r#"{
            "llm.call": {
                "provider": "anthropic",
                "api_key_env": "ANTHROPIC_API_KEY"
            }
        }"#;
        // Simulate a user who has set the actual key in the environment.
        // describe() must not read env vars — it only shows the variable NAME.
        std::env::set_var("ANTHROPIC_API_KEY", "sk-secret-do-not-log");
        let reg = ProviderRegistry::from_json(json).expect("valid");
        let desc = reg.describe();
        assert!(
            !desc.contains("sk-secret-do-not-log"),
            "describe() leaked secret: {desc}"
        );
        assert!(
            desc.contains("ANTHROPIC_API_KEY"),
            "describe() should show key_env name: {desc}"
        );
    }
}
