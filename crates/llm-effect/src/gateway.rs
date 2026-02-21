use std::collections::BTreeMap;
use std::path::Path;

use boruna_bytecode::Value;
use serde::{Deserialize, Serialize};

use crate::cache::{CacheEntry, LlmCache};
use crate::context::ContextStore;
use crate::normalize::{self, LlmRequest};
use crate::policy::{self, LlmPolicy, LlmUsage};
use crate::prompt::PromptRegistry;

/// Execution mode for the LLM gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Mock backend — deterministic fixed responses.
    Mock,
    /// Record mode — calls external (or mock) and logs responses.
    Record,
    /// Replay mode — returns logged/cached responses only. Cache miss = hard error.
    Replay,
}

/// Result of an LLM effect execution.
#[derive(Debug, Clone)]
pub struct LlmEffectResult {
    pub request_id: String,
    pub result: Value,
    pub cached: bool,
}

/// Log entry for record/replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmLogEntry {
    pub request_hash: String,
    pub prompt_id: String,
    pub model: String,
    pub result: Value,
}

/// The LLM gateway — executes LLM effects with policy enforcement,
/// caching, and record/replay support.
pub struct LlmGateway {
    prompt_registry: PromptRegistry,
    context_store: ContextStore,
    cache: LlmCache,
    policy: LlmPolicy,
    usage: LlmUsage,
    mode: ExecutionMode,
    log: Vec<LlmLogEntry>,
}

impl LlmGateway {
    /// Create a new LLM gateway.
    pub fn new(
        prompts_dir: &Path,
        context_dir: &Path,
        cache_dir: &Path,
        policy: LlmPolicy,
        mode: ExecutionMode,
    ) -> Result<Self, String> {
        Ok(LlmGateway {
            prompt_registry: PromptRegistry::open(prompts_dir)?,
            context_store: ContextStore::open(context_dir)?,
            cache: LlmCache::open(cache_dir)?,
            policy,
            usage: LlmUsage::default(),
            mode,
            log: Vec::new(),
        })
    }

    /// Get the prompt registry (for registration/verification).
    pub fn prompt_registry(&self) -> &PromptRegistry {
        &self.prompt_registry
    }

    /// Get mutable prompt registry.
    pub fn prompt_registry_mut(&mut self) -> &mut PromptRegistry {
        &mut self.prompt_registry
    }

    /// Get the context store.
    pub fn context_store(&self) -> &ContextStore {
        &self.context_store
    }

    /// Get the context store mutably.
    pub fn context_store_mut(&mut self) -> &mut ContextStore {
        &mut self.context_store
    }

    /// Get current usage stats.
    pub fn usage(&self) -> &LlmUsage {
        &self.usage
    }

    /// Get the execution log.
    pub fn log(&self) -> &[LlmLogEntry] {
        &self.log
    }

    /// Execute an LLM effect.
    pub fn execute(&mut self, req: &LlmRequest) -> Result<LlmEffectResult, String> {
        // 1. Compute context size
        let context_bytes = self.compute_context_bytes(req)?;

        // 2. Check policy
        policy::check_policy(req, &self.policy, &self.usage, context_bytes)?;

        // 3. Get prompt and schema hashes for cache key
        let prompt_hash = self
            .prompt_registry
            .prompt_hash(&req.prompt_id)
            .unwrap_or_default();
        let schema_hash = self
            .prompt_registry
            .schema_hash(&req.output_schema_id)
            .unwrap_or_default();

        // 4. Compute cache key
        let cache_key = normalize::compute_cache_key(req, &prompt_hash, &schema_hash);
        let request_id = normalize::request_id_from_cache_key(&cache_key);

        // 5. Check cache (if mode allows)
        if req.cache_mode.should_read() || self.mode == ExecutionMode::Replay {
            if let Some(entry) = self.cache.read(&cache_key) {
                // Log for replay
                self.log.push(LlmLogEntry {
                    request_hash: cache_key.clone(),
                    prompt_id: req.prompt_id.clone(),
                    model: req.model.clone(),
                    result: entry.result.clone(),
                });
                self.usage.call_count += 1;
                self.usage.total_tokens_requested += req.max_output_tokens;
                return Ok(LlmEffectResult {
                    request_id,
                    result: entry.result,
                    cached: true,
                });
            }

            // In replay mode, cache miss is a hard error
            if self.mode == ExecutionMode::Replay {
                return Err(format!(
                    "LlmReplayMiss: no cached response for cache_key {cache_key}"
                ));
            }
        }

        // 6. Generate response (mock backend)
        let result = self.generate_mock_response(req)?;

        // 7. Write to cache (if mode allows)
        if req.cache_mode.should_write() {
            self.cache.write(&CacheEntry {
                cache_key: cache_key.clone(),
                prompt_id: req.prompt_id.clone(),
                model: req.model.clone(),
                schema_id: req.output_schema_id.clone(),
                prompt_hash: prompt_hash.clone(),
                result: result.clone(),
            })?;
        }

        // 8. Log
        self.log.push(LlmLogEntry {
            request_hash: cache_key,
            prompt_id: req.prompt_id.clone(),
            model: req.model.clone(),
            result: result.clone(),
        });

        // 9. Update usage
        self.usage.call_count += 1;
        self.usage.total_tokens_requested += req.max_output_tokens;

        Ok(LlmEffectResult {
            request_id,
            result,
            cached: false,
        })
    }

    fn compute_context_bytes(&self, req: &LlmRequest) -> Result<u64, String> {
        let mut total = 0u64;
        for hash in &req.context_refs {
            if let Ok(content) = self.context_store.get(hash) {
                total += content.len() as u64;
            }
        }
        Ok(total)
    }

    /// Generate a mock response based on the output schema.
    fn generate_mock_response(&self, req: &LlmRequest) -> Result<Value, String> {
        match req.output_schema_id.as_str() {
            "patch_bundle" => Ok(mock_patch_bundle()),
            _ => Ok(mock_json_object()),
        }
    }

    /// Validate that a result matches the expected schema type.
    /// For MVP: just checks it's a Map (json_object) or contains patch fields.
    pub fn validate_output(result: &Value, schema_id: &str) -> Result<(), String> {
        match schema_id {
            "patch_bundle" => match result {
                Value::Map(m) => {
                    if !m.contains_key("patches") {
                        return Err("patch_bundle: missing 'patches' field".into());
                    }
                    Ok(())
                }
                _ => Err("patch_bundle: expected Map".into()),
            },
            _ => {
                // Generic json_object: must be a Map
                match result {
                    Value::Map(_) => Ok(()),
                    _ => Err(format!(
                        "schema {schema_id}: expected Map, got {}",
                        result.type_name()
                    )),
                }
            }
        }
    }
}

/// Mock patch bundle response.
fn mock_patch_bundle() -> Value {
    let mut bundle = BTreeMap::new();
    bundle.insert("version".into(), Value::Int(1));

    let mut metadata = BTreeMap::new();
    metadata.insert("id".into(), Value::String("mock-pb-001".into()));
    metadata.insert("intent".into(), Value::String("mock patch".into()));
    metadata.insert("author".into(), Value::String("llm-mock".into()));
    bundle.insert("metadata".into(), Value::Map(metadata));

    let mut hunk = BTreeMap::new();
    hunk.insert("start_line".into(), Value::Int(1));
    hunk.insert("old_text".into(), Value::String("old".into()));
    hunk.insert("new_text".into(), Value::String("new".into()));

    let mut file_patch = BTreeMap::new();
    file_patch.insert("file".into(), Value::String("mock.ax".into()));
    file_patch.insert("hunks".into(), Value::List(vec![Value::Map(hunk)]));

    bundle.insert("patches".into(), Value::List(vec![Value::Map(file_patch)]));

    Value::Map(bundle)
}

/// Mock JSON object response.
fn mock_json_object() -> Value {
    let mut obj = BTreeMap::new();
    obj.insert("status".into(), Value::String("ok".into()));
    obj.insert("mock".into(), Value::Bool(true));
    Value::Map(obj)
}

#[cfg(test)]
mod gateway_tests {
    use super::*;
    use crate::normalize::CacheMode;
    use crate::prompt::PromptTemplate;

    fn setup_gateway(mode: ExecutionMode) -> (tempfile::TempDir, LlmGateway) {
        let dir = tempfile::tempdir().unwrap();
        let prompts_dir = dir.path().join("prompts");
        let context_dir = dir.path().join("context");
        let cache_dir = dir.path().join("cache");

        let mut gw = LlmGateway::new(
            &prompts_dir,
            &context_dir,
            &cache_dir,
            LlmPolicy::allow_all(),
            mode,
        )
        .unwrap();

        // Register a test prompt
        gw.prompt_registry_mut()
            .register_prompt(&PromptTemplate {
                id: "test.prompt".into(),
                version: "1.0.0".into(),
                template: "Hello {{name}}".into(),
                parameters: vec!["name".into()],
                default_model: "default".into(),
                default_max_tokens: 512,
                default_temperature: 0,
                default_schema_id: "json_object".into(),
            })
            .unwrap();

        (dir, gw)
    }

    fn make_request() -> LlmRequest {
        LlmRequest {
            prompt_id: "test.prompt".into(),
            args: BTreeMap::new(),
            context_refs: Vec::new(),
            model: "default".into(),
            max_output_tokens: 100,
            temperature: 0,
            output_schema_id: "json_object".into(),
            cache_mode: CacheMode::ReadWrite,
        }
    }

    #[test]
    fn test_mock_execution() {
        let (_dir, mut gw) = setup_gateway(ExecutionMode::Mock);
        let req = make_request();

        let result = gw.execute(&req).unwrap();
        assert!(!result.cached);
        assert!(!result.request_id.is_empty());

        // Result should be a valid json_object
        LlmGateway::validate_output(&result.result, "json_object").unwrap();
    }

    #[test]
    fn test_cache_hit() {
        let (_dir, mut gw) = setup_gateway(ExecutionMode::Mock);
        let req = make_request();

        // First call writes to cache
        let r1 = gw.execute(&req).unwrap();
        assert!(!r1.cached);

        // Second call reads from cache
        let r2 = gw.execute(&req).unwrap();
        assert!(r2.cached);
        assert_eq!(r1.result, r2.result);
    }

    #[test]
    fn test_cache_off() {
        let (_dir, mut gw) = setup_gateway(ExecutionMode::Mock);
        let mut req = make_request();
        req.cache_mode = CacheMode::Off;

        let r1 = gw.execute(&req).unwrap();
        assert!(!r1.cached);

        let r2 = gw.execute(&req).unwrap();
        assert!(!r2.cached);
    }

    #[test]
    fn test_replay_mode_hit() {
        let (_dir, mut gw) = setup_gateway(ExecutionMode::Mock);
        let req = make_request();

        // Populate cache in mock mode (1 call -> 1 log entry)
        let r1 = gw.execute(&req).unwrap();
        assert!(!r1.cached);
        assert_eq!(gw.log().len(), 1);

        // Second call hits cache (still mock mode, cache populated)
        let r2 = gw.execute(&req).unwrap();
        assert!(r2.cached);
        assert_eq!(gw.log().len(), 2);
        assert_eq!(r1.result, r2.result);
    }

    #[test]
    fn test_replay_mode_miss() {
        let (_dir, mut gw) = setup_gateway(ExecutionMode::Replay);
        let req = make_request();

        // Replay mode with empty cache -> hard error
        let result = gw.execute(&req);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("LlmReplayMiss"));
    }

    #[test]
    fn test_policy_enforcement() {
        let dir = tempfile::tempdir().unwrap();
        let prompts_dir = dir.path().join("prompts");
        let context_dir = dir.path().join("context");
        let cache_dir = dir.path().join("cache");

        let policy = LlmPolicy {
            max_calls: 1,
            ..Default::default()
        };

        let mut gw = LlmGateway::new(
            &prompts_dir,
            &context_dir,
            &cache_dir,
            policy,
            ExecutionMode::Mock,
        )
        .unwrap();

        gw.prompt_registry_mut()
            .register_prompt(&PromptTemplate {
                id: "test.prompt".into(),
                version: "1.0.0".into(),
                template: "Test".into(),
                parameters: vec![],
                default_model: "default".into(),
                default_max_tokens: 512,
                default_temperature: 0,
                default_schema_id: "json_object".into(),
            })
            .unwrap();

        let mut req = make_request();
        req.cache_mode = CacheMode::Off; // Disable cache so second call isn't a cache hit

        // First call succeeds
        gw.execute(&req).unwrap();

        // Second call fails — max_calls exceeded
        let result = gw.execute(&req);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("max calls exceeded"));
    }

    #[test]
    fn test_patch_bundle_mock() {
        let (_dir, mut gw) = setup_gateway(ExecutionMode::Mock);
        let mut req = make_request();
        req.output_schema_id = "patch_bundle".into();

        let result = gw.execute(&req).unwrap();
        LlmGateway::validate_output(&result.result, "patch_bundle").unwrap();
    }

    #[test]
    fn test_validate_output_json_object() {
        let mut m = BTreeMap::new();
        m.insert("key".into(), Value::String("val".into()));
        assert!(LlmGateway::validate_output(&Value::Map(m), "json_object").is_ok());
        assert!(LlmGateway::validate_output(&Value::Int(42), "json_object").is_err());
    }

    #[test]
    fn test_validate_output_patch_bundle() {
        let mut m = BTreeMap::new();
        m.insert("patches".into(), Value::List(vec![]));
        assert!(LlmGateway::validate_output(&Value::Map(m), "patch_bundle").is_ok());

        let empty = BTreeMap::new();
        assert!(LlmGateway::validate_output(&Value::Map(empty), "patch_bundle").is_err());
    }

    #[test]
    fn test_usage_tracking() {
        let (_dir, mut gw) = setup_gateway(ExecutionMode::Mock);
        let mut req = make_request();
        req.cache_mode = CacheMode::Off;

        gw.execute(&req).unwrap();
        assert_eq!(gw.usage().call_count, 1);
        assert_eq!(gw.usage().total_tokens_requested, 100);

        gw.execute(&req).unwrap();
        assert_eq!(gw.usage().call_count, 2);
        assert_eq!(gw.usage().total_tokens_requested, 200);
    }

    #[test]
    fn test_context_refs_with_store() {
        let (_dir, mut gw) = setup_gateway(ExecutionMode::Mock);

        // Put some context
        let hash = gw.context_store_mut().put("some context data").unwrap();

        let mut req = make_request();
        req.context_refs = vec![hash];

        let result = gw.execute(&req).unwrap();
        assert!(!result.request_id.is_empty());
    }
}
