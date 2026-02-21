use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use serde_json;

use boruna_bytecode::Value;

/// An LLM call request — all fields needed to compute the deterministic cache key.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub prompt_id: String,
    pub args: BTreeMap<String, Value>,
    pub context_refs: Vec<String>,
    pub model: String,
    pub max_output_tokens: u64,
    pub temperature: u64,
    pub output_schema_id: String,
    pub cache_mode: CacheMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    Read,
    Write,
    ReadWrite,
    Off,
}

impl CacheMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "read" => CacheMode::Read,
            "write" => CacheMode::Write,
            "readwrite" => CacheMode::ReadWrite,
            _ => CacheMode::Off,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CacheMode::Read => "read",
            CacheMode::Write => "write",
            CacheMode::ReadWrite => "readwrite",
            CacheMode::Off => "off",
        }
    }

    pub fn should_read(&self) -> bool {
        matches!(self, CacheMode::Read | CacheMode::ReadWrite)
    }

    pub fn should_write(&self) -> bool {
        matches!(self, CacheMode::Write | CacheMode::ReadWrite)
    }
}

/// Parse an LlmRequest from a framework Effect payload Value.
///
/// Expected: Record/Map with fields matching LlmCall spec.
pub fn parse_llm_request(payload: &Value) -> Result<LlmRequest, String> {
    let map = match payload {
        Value::Map(m) => m,
        _ => return Err("LlmCall payload must be a Map".into()),
    };

    let prompt_id = get_string(map, "prompt_id")
        .ok_or("missing prompt_id")?;
    let model = get_string(map, "model")
        .unwrap_or_else(|| "default".into());
    let max_output_tokens = get_int(map, "max_output_tokens")
        .unwrap_or(1024) as u64;
    let temperature = get_int(map, "temperature")
        .unwrap_or(0) as u64;
    let output_schema_id = get_string(map, "output_schema_id")
        .unwrap_or_else(|| "json_object".into());
    let cache_mode_str = get_string(map, "cache_mode")
        .unwrap_or_else(|| "readwrite".into());

    let args = match map.get("args") {
        Some(Value::Map(m)) => m.clone(),
        _ => BTreeMap::new(),
    };

    let context_refs = match map.get("context_refs") {
        Some(Value::List(items)) => {
            items.iter().filter_map(|v| {
                if let Value::String(s) = v { Some(s.clone()) } else { None }
            }).collect()
        }
        _ => Vec::new(),
    };

    Ok(LlmRequest {
        prompt_id,
        args,
        context_refs,
        model,
        max_output_tokens,
        temperature,
        output_schema_id,
        cache_mode: CacheMode::from_str(&cache_mode_str),
    })
}

fn get_string(map: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn get_int(map: &BTreeMap<String, Value>, key: &str) -> Option<i64> {
    match map.get(key) {
        Some(Value::Int(n)) => Some(*n),
        _ => None,
    }
}

/// Compute the canonical JSON for a Value (sorted keys, no whitespace).
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Unit => "null".into(),
        Value::Bool(b) => if *b { "true".into() } else { "false".into() },
        Value::Int(n) => n.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        Value::None => "null".into(),
        Value::Some(v) => canonical_json(v),
        Value::Ok(v) => canonical_json(v),
        Value::Err(v) => {
            format!("{{\"err\":{}}}", canonical_json(v))
        }
        Value::Map(m) => {
            // BTreeMap is already sorted
            let pairs: Vec<String> = m.iter()
                .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), canonical_json(v)))
                .collect();
            format!("{{{}}}", pairs.join(","))
        }
        Value::List(items) => {
            let parts: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Record { fields, .. } => {
            let parts: Vec<String> = fields.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Enum { variant, payload, .. } => {
            format!("{{\"v\":{},\"p\":{}}}", variant, canonical_json(payload))
        }
        Value::ActorId(id) => id.to_string(),
        Value::FnRef(id) => id.to_string(),
    }
}

/// Compute the deterministic cache key for an LLM request.
///
/// Includes prompt and schema content hashes if available.
pub fn compute_cache_key(
    req: &LlmRequest,
    prompt_content_hash: &str,
    schema_content_hash: &str,
) -> String {
    let mut hasher = Sha256::new();

    // prompt_id
    hasher.update(canonical_value_json(&Value::String(req.prompt_id.clone())).as_bytes());
    hasher.update(b"|");

    // args (BTreeMap — already sorted)
    let args_value = Value::Map(req.args.clone());
    hasher.update(canonical_json(&args_value).as_bytes());
    hasher.update(b"|");

    // context_refs (sorted)
    let mut sorted_refs = req.context_refs.clone();
    sorted_refs.sort();
    for r in &sorted_refs {
        hasher.update(r.as_bytes());
        hasher.update(b",");
    }
    hasher.update(b"|");

    // model
    hasher.update(req.model.as_bytes());
    hasher.update(b"|");

    // max_output_tokens
    hasher.update(req.max_output_tokens.to_string().as_bytes());
    hasher.update(b"|");

    // temperature
    hasher.update(req.temperature.to_string().as_bytes());
    hasher.update(b"|");

    // output_schema_id
    hasher.update(req.output_schema_id.as_bytes());
    hasher.update(b"|");

    // prompt content hash
    hasher.update(prompt_content_hash.as_bytes());
    hasher.update(b"|");

    // schema content hash
    hasher.update(schema_content_hash.as_bytes());

    format!("sha256:{:x}", hasher.finalize())
}

fn canonical_value_json(v: &Value) -> String {
    canonical_json(v)
}

/// Derive a stable request_id from a cache key (first 16 hex chars).
pub fn request_id_from_cache_key(cache_key: &str) -> String {
    let hex = cache_key.strip_prefix("sha256:").unwrap_or(cache_key);
    hex[..hex.len().min(16)].to_string()
}

#[cfg(test)]
mod normalize_tests {
    use super::*;

    #[test]
    fn test_canonical_json_map_sorted() {
        let mut m = BTreeMap::new();
        m.insert("z".into(), Value::Int(1));
        m.insert("a".into(), Value::Int(2));
        let json = canonical_json(&Value::Map(m));
        assert_eq!(json, r#"{"a":2,"z":1}"#);
    }

    #[test]
    fn test_canonical_json_string_escaped() {
        let json = canonical_json(&Value::String("hello \"world\"".into()));
        assert_eq!(json, r#""hello \"world\"""#);
    }

    #[test]
    fn test_cache_key_deterministic() {
        let req = LlmRequest {
            prompt_id: "test.prompt".into(),
            args: BTreeMap::new(),
            context_refs: vec!["hash_b".into(), "hash_a".into()],
            model: "default".into(),
            max_output_tokens: 512,
            temperature: 0,
            output_schema_id: "json_object".into(),
            cache_mode: CacheMode::ReadWrite,
        };
        let k1 = compute_cache_key(&req, "phash1", "shash1");
        let k2 = compute_cache_key(&req, "phash1", "shash1");
        assert_eq!(k1, k2);
        assert!(k1.starts_with("sha256:"));
    }

    #[test]
    fn test_cache_key_context_ref_order_invariant() {
        let req1 = LlmRequest {
            prompt_id: "p".into(),
            args: BTreeMap::new(),
            context_refs: vec!["a".into(), "b".into()],
            model: "m".into(),
            max_output_tokens: 100,
            temperature: 0,
            output_schema_id: "s".into(),
            cache_mode: CacheMode::Off,
        };
        let mut req2 = req1.clone();
        req2.context_refs = vec!["b".into(), "a".into()];

        let k1 = compute_cache_key(&req1, "", "");
        let k2 = compute_cache_key(&req2, "", "");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_request_id_from_cache_key() {
        let key = "sha256:abcdef1234567890ffffffff";
        let id = request_id_from_cache_key(key);
        assert_eq!(id, "abcdef1234567890");
    }

    #[test]
    fn test_parse_llm_request() {
        let mut map = BTreeMap::new();
        map.insert("prompt_id".into(), Value::String("test.prompt".into()));
        map.insert("model".into(), Value::String("fast".into()));
        map.insert("max_output_tokens".into(), Value::Int(256));
        map.insert("temperature".into(), Value::Int(0));
        map.insert("output_schema_id".into(), Value::String("patch_bundle".into()));
        map.insert("cache_mode".into(), Value::String("readwrite".into()));

        let req = parse_llm_request(&Value::Map(map)).unwrap();
        assert_eq!(req.prompt_id, "test.prompt");
        assert_eq!(req.model, "fast");
        assert_eq!(req.max_output_tokens, 256);
        assert_eq!(req.output_schema_id, "patch_bundle");
    }

    #[test]
    fn test_parse_llm_request_missing_prompt_id() {
        let map = BTreeMap::new();
        let result = parse_llm_request(&Value::Map(map));
        assert!(result.is_err());
    }
}
