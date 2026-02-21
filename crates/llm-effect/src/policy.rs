use serde::{Deserialize, Serialize};

use crate::normalize::LlmRequest;

/// LLM-specific policy constraints.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmPolicy {
    /// Schema version for forward/backward compatibility.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Total token budget across all LLM calls in a session (0 = unlimited).
    pub total_token_budget: u64,
    /// Maximum number of LLM calls per session (0 = unlimited).
    pub max_calls: u64,
    /// Allowed model names (empty = all allowed).
    pub allowed_models: Vec<String>,
    /// Maximum context bytes per call (0 = unlimited).
    pub max_context_bytes: u64,
    /// Allowed prompt IDs (empty = all allowed).
    pub prompt_allowlist: Vec<String>,
}

fn default_schema_version() -> u32 {
    1
}

impl LlmPolicy {
    /// A permissive policy for testing.
    pub fn allow_all() -> Self {
        LlmPolicy::default()
    }

    /// A restrictive policy for testing.
    pub fn restrictive(budget: u64, max_calls: u64) -> Self {
        LlmPolicy {
            total_token_budget: budget,
            max_calls,
            ..Default::default()
        }
    }
}

/// Tracks cumulative LLM usage within a session.
#[derive(Debug, Clone, Default)]
pub struct LlmUsage {
    pub total_tokens_requested: u64,
    pub call_count: u64,
}

/// Check an LLM request against policy and usage.
/// Returns Ok(()) or a structured error string.
pub fn check_policy(
    req: &LlmRequest,
    policy: &LlmPolicy,
    usage: &LlmUsage,
    context_bytes: u64,
) -> Result<(), String> {
    // Check prompt allowlist
    if !policy.prompt_allowlist.is_empty()
        && !policy.prompt_allowlist.iter().any(|p| p == &req.prompt_id)
    {
        return Err(format!("prompt '{}' not in allowlist", req.prompt_id));
    }

    // Check model allowlist
    if !policy.allowed_models.is_empty() && !policy.allowed_models.iter().any(|m| m == &req.model) {
        return Err(format!("model '{}' not in allowed models", req.model));
    }

    // Check token budget
    if policy.total_token_budget > 0 {
        let projected = usage.total_tokens_requested + req.max_output_tokens;
        if projected > policy.total_token_budget {
            return Err(format!(
                "token budget exceeded: {} + {} > {}",
                usage.total_tokens_requested, req.max_output_tokens, policy.total_token_budget
            ));
        }
    }

    // Check call count
    if policy.max_calls > 0 && usage.call_count + 1 > policy.max_calls {
        return Err(format!(
            "max calls exceeded: {} >= {}",
            usage.call_count, policy.max_calls
        ));
    }

    // Check context bytes
    if policy.max_context_bytes > 0 && context_bytes > policy.max_context_bytes {
        return Err(format!(
            "context too large: {context_bytes} > {}",
            policy.max_context_bytes
        ));
    }

    Ok(())
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    use crate::normalize::CacheMode;
    use std::collections::BTreeMap;

    fn make_request() -> LlmRequest {
        LlmRequest {
            prompt_id: "test.prompt".into(),
            args: BTreeMap::new(),
            context_refs: Vec::new(),
            model: "default".into(),
            max_output_tokens: 100,
            temperature: 0,
            output_schema_id: "json_object".into(),
            cache_mode: CacheMode::Off,
        }
    }

    #[test]
    fn test_allow_all_passes() {
        let req = make_request();
        let policy = LlmPolicy::allow_all();
        let usage = LlmUsage::default();
        assert!(check_policy(&req, &policy, &usage, 0).is_ok());
    }

    #[test]
    fn test_token_budget_exceeded() {
        let req = make_request(); // 100 tokens
        let policy = LlmPolicy {
            total_token_budget: 50,
            ..Default::default()
        };
        let usage = LlmUsage::default();
        let result = check_policy(&req, &policy, &usage, 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("token budget exceeded"));
    }

    #[test]
    fn test_token_budget_with_prior_usage() {
        let req = make_request(); // 100 tokens
        let policy = LlmPolicy {
            total_token_budget: 150,
            ..Default::default()
        };
        let usage = LlmUsage {
            total_tokens_requested: 60,
            call_count: 1,
        };
        // 60 + 100 = 160 > 150
        let result = check_policy(&req, &policy, &usage, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_max_calls_exceeded() {
        let req = make_request();
        let policy = LlmPolicy {
            max_calls: 2,
            ..Default::default()
        };
        let usage = LlmUsage {
            total_tokens_requested: 0,
            call_count: 2,
        };
        let result = check_policy(&req, &policy, &usage, 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("max calls exceeded"));
    }

    #[test]
    fn test_model_not_allowed() {
        let req = make_request(); // model = "default"
        let policy = LlmPolicy {
            allowed_models: vec!["fast".into(), "smart".into()],
            ..Default::default()
        };
        let usage = LlmUsage::default();
        let result = check_policy(&req, &policy, &usage, 0);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("model 'default' not in allowed models"));
    }

    #[test]
    fn test_model_allowed() {
        let mut req = make_request();
        req.model = "fast".into();
        let policy = LlmPolicy {
            allowed_models: vec!["fast".into()],
            ..Default::default()
        };
        let usage = LlmUsage::default();
        assert!(check_policy(&req, &policy, &usage, 0).is_ok());
    }

    #[test]
    fn test_context_too_large() {
        let req = make_request();
        let policy = LlmPolicy {
            max_context_bytes: 100,
            ..Default::default()
        };
        let usage = LlmUsage::default();
        let result = check_policy(&req, &policy, &usage, 200);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("context too large"));
    }

    #[test]
    fn test_prompt_not_in_allowlist() {
        let req = make_request(); // prompt_id = "test.prompt"
        let policy = LlmPolicy {
            prompt_allowlist: vec!["allowed.prompt".into()],
            ..Default::default()
        };
        let usage = LlmUsage::default();
        let result = check_policy(&req, &policy, &usage, 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not in allowlist"));
    }

    #[test]
    fn test_prompt_in_allowlist() {
        let req = make_request();
        let policy = LlmPolicy {
            prompt_allowlist: vec!["test.prompt".into()],
            ..Default::default()
        };
        let usage = LlmUsage::default();
        assert!(check_policy(&req, &policy, &usage, 0).is_ok());
    }
}
