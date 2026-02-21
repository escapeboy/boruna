use std::collections::BTreeMap;

use boruna_bytecode::Value;

use crate::gateway::{ExecutionMode, LlmGateway};
use crate::normalize::{self, CacheMode, LlmRequest};
use crate::policy::LlmPolicy;
use crate::prompt::{PromptRegistry, PromptTemplate};

fn make_template(id: &str) -> PromptTemplate {
    PromptTemplate {
        id: id.into(),
        version: "1.0.0".into(),
        template: "Refactor: {{code}}".into(),
        parameters: vec!["code".into()],
        default_model: "default".into(),
        default_max_tokens: 512,
        default_temperature: 0,
        default_schema_id: "json_object".into(),
    }
}

fn make_request(prompt_id: &str) -> LlmRequest {
    LlmRequest {
        prompt_id: prompt_id.into(),
        args: BTreeMap::new(),
        context_refs: Vec::new(),
        model: "default".into(),
        max_output_tokens: 256,
        temperature: 0,
        output_schema_id: "json_object".into(),
        cache_mode: CacheMode::ReadWrite,
    }
}

// --- Integration: record-then-replay ---

#[test]
fn test_record_then_replay() {
    let dir = tempfile::tempdir().unwrap();
    let prompts_dir = dir.path().join("prompts");
    let context_dir = dir.path().join("context");
    let cache_dir = dir.path().join("cache");

    // Phase 1: Record (mock mode + cache write)
    {
        let mut gw = LlmGateway::new(
            &prompts_dir,
            &context_dir,
            &cache_dir,
            LlmPolicy::allow_all(),
            ExecutionMode::Mock,
        )
        .unwrap();
        gw.prompt_registry_mut()
            .register_prompt(&make_template("test.refactor"))
            .unwrap();

        let req = make_request("test.refactor");
        let r1 = gw.execute(&req).unwrap();
        assert!(!r1.cached);
    }

    // Phase 2: Replay (uses cached response)
    {
        let mut gw = LlmGateway::new(
            &prompts_dir,
            &context_dir,
            &cache_dir,
            LlmPolicy::allow_all(),
            ExecutionMode::Replay,
        )
        .unwrap();

        let req = make_request("test.refactor");
        let r2 = gw.execute(&req).unwrap();
        assert!(r2.cached);
    }
}

#[test]
fn test_replay_miss_is_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut gw = LlmGateway::new(
        &dir.path().join("prompts"),
        &dir.path().join("context"),
        &dir.path().join("cache"),
        LlmPolicy::allow_all(),
        ExecutionMode::Replay,
    )
    .unwrap();
    gw.prompt_registry_mut()
        .register_prompt(&make_template("test.x"))
        .unwrap();

    let req = make_request("test.x");
    let result = gw.execute(&req);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("LlmReplayMiss"),
        "expected LlmReplayMiss, got: {err}"
    );
}

// --- Schema validation ---

#[test]
fn test_reject_non_map_as_json_object() {
    let result = LlmGateway::validate_output(&Value::String("prose".into()), "json_object");
    assert!(result.is_err());
}

#[test]
fn test_reject_prose_as_patch_bundle() {
    let result = LlmGateway::validate_output(&Value::String("some text".into()), "patch_bundle");
    assert!(result.is_err());
}

#[test]
fn test_accept_valid_patch_bundle() {
    let mut m = BTreeMap::new();
    m.insert("patches".into(), Value::List(vec![]));
    m.insert("version".into(), Value::Int(1));
    assert!(LlmGateway::validate_output(&Value::Map(m), "patch_bundle").is_ok());
}

// --- Normalization determinism ---

#[test]
fn test_cache_key_determinism_across_sessions() {
    let req = LlmRequest {
        prompt_id: "p1".into(),
        args: {
            let mut m = BTreeMap::new();
            m.insert("x".into(), Value::Int(1));
            m.insert("y".into(), Value::String("z".into()));
            m
        },
        context_refs: vec!["ref_b".into(), "ref_a".into()],
        model: "default".into(),
        max_output_tokens: 512,
        temperature: 0,
        output_schema_id: "json_object".into(),
        cache_mode: CacheMode::Off,
    };

    let k1 = normalize::compute_cache_key(&req, "phash", "shash");
    let k2 = normalize::compute_cache_key(&req, "phash", "shash");
    assert_eq!(k1, k2);
}

// --- End-to-end: prompt compilation + context + execution ---

#[test]
fn test_end_to_end_with_context() {
    let dir = tempfile::tempdir().unwrap();
    let prompts_dir = dir.path().join("prompts");
    let context_dir = dir.path().join("context");
    let cache_dir = dir.path().join("cache");

    let mut gw = LlmGateway::new(
        &prompts_dir,
        &context_dir,
        &cache_dir,
        LlmPolicy::allow_all(),
        ExecutionMode::Mock,
    )
    .unwrap();

    // Register prompt and schema
    gw.prompt_registry_mut()
        .register_prompt(&make_template("test.e2e"))
        .unwrap();
    gw.prompt_registry_mut()
        .register_schema("json_object", r#"{"type":"object"}"#)
        .unwrap();

    // Store context
    let ctx_hash = gw
        .context_store_mut()
        .put("fn main() { println!(\"hello\"); }")
        .unwrap();

    // Execute
    let mut req = make_request("test.e2e");
    req.context_refs = vec![ctx_hash];

    let result = gw.execute(&req).unwrap();
    assert!(!result.cached);
    LlmGateway::validate_output(&result.result, "json_object").unwrap();
}

// --- Policy integration ---

#[test]
fn test_policy_blocks_over_budget_integration() {
    let dir = tempfile::tempdir().unwrap();
    let mut gw = LlmGateway::new(
        &dir.path().join("prompts"),
        &dir.path().join("context"),
        &dir.path().join("cache"),
        LlmPolicy {
            total_token_budget: 100,
            ..Default::default()
        },
        ExecutionMode::Mock,
    )
    .unwrap();
    gw.prompt_registry_mut()
        .register_prompt(&make_template("test.budget"))
        .unwrap();

    let mut req = make_request("test.budget");
    req.max_output_tokens = 256; // exceeds 100 budget
    req.cache_mode = CacheMode::Off;

    let result = gw.execute(&req);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("token budget exceeded"));
}

// --- Prompt registry verify ---

#[test]
fn test_prompt_registry_integrity_check() {
    let dir = tempfile::tempdir().unwrap();
    let mut reg = PromptRegistry::open(dir.path()).unwrap();
    reg.register_prompt(&make_template("t1")).unwrap();
    reg.register_schema("s1", r#"{"type":"object"}"#).unwrap();

    // Verify passes
    assert!(reg.verify().is_ok());

    // Tamper with prompt file
    std::fs::write(dir.path().join("t1.prompt.json"), "TAMPERED").unwrap();
    let result = reg.verify();
    assert!(result.is_err());
}
