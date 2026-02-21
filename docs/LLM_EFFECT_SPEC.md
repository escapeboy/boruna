# LLM Effect Specification

Token-optimized, deterministic LLM integration for the Boruna platform.

## 1. Effect Shape

LLM calls are produced as effects from `update()`, like all other side effects.

### Effect::LlmCall

```
EffectKind::LlmCall
payload: {
    prompt_id:        String        // references prompt registry
    args:             Map<String, Value>  // template arguments
    context_refs:     List<Hash>    // content-addressed blob references
    model:            String        // logical model name (e.g. "default", "fast")
    max_output_tokens: Int          // token budget for this call
    temperature:      Int           // 0 = deterministic (default), 100 = max creativity
    output_schema_id: String        // schema ID for structured output validation
    cache_mode:       String        // "read" | "write" | "readwrite" | "off"
}
callback_tag: String  // message tag for Msg::LlmResult delivery
```

Apps produce this effect via a record literal. The framework runtime executes it.

### Capability Requirement

`EffectKind::LlmCall` maps to capability `"llm.call"`. Must be declared in policy.

## 2. Output Shape

LLM calls MUST return structured output. No prose.

### LlmResult

One of:
- **PatchBundle**: An orchestrator-format patch bundle (preferred for code generation)
- **JsonObject**: A `Value::Map` validated against the referenced `output_schema_id`

If the output fails schema validation, the effect returns `Value::Err("schema_violation: ...")`.

### Schema Registry

Output schemas are stored alongside prompts:

```
prompts/
  schemas/
    <schema_id>.json    // JSON Schema definition
```

Schemas are content-hashed. The hash is included in the cache key.

## 3. Normalization Rules (Cache Key)

The deterministic cache key is computed as:

```
SHA-256(
    canonical_json(prompt_id) +
    canonical_json(args, sorted keys) +
    sorted(context_refs) +
    model +
    max_output_tokens.to_string() +
    temperature.to_string() +
    output_schema_id +
    prompt_content_hash +
    schema_content_hash
)
```

### Canonical JSON

- Keys sorted lexicographically
- No trailing commas
- No whitespace between tokens
- Numbers in minimal representation
- UTF-8 normalized (NFC)

### Context Ref Ordering

Context refs are sorted lexicographically before hashing.

## 4. Replay Rules

### Record Mode (default)

When an `LlmCall` effect executes:
1. Compute the cache key (normalized request)
2. Execute the call (mock or external)
3. Log `Event::LlmCall { request_hash, response }` to the EventLog
4. If `cache_mode` includes "write", store in deterministic cache

### Replay Mode

When replaying:
1. Compute the cache key
2. Look up in replay log (by request hash)
3. If found: return logged response
4. If NOT found: **hard error** (`LlmReplayMiss`). Determinism violation.

External model calls NEVER happen during replay.

## 5. Prompt Registry

### Storage Layout

```
prompts/
    registry.json               // manifest of all prompts
    <prompt_id>.prompt.json     // prompt template + metadata
    schemas/
        <schema_id>.json        // output schemas
```

### registry.json

```json
{
    "version": 1,
    "prompts": {
        "<prompt_id>": {
            "file": "<prompt_id>.prompt.json",
            "content_hash": "sha256:...",
            "version": "1.0.0"
        }
    },
    "schemas": {
        "<schema_id>": {
            "file": "schemas/<schema_id>.json",
            "content_hash": "sha256:..."
        }
    }
}
```

### Prompt Template Format

```json
{
    "id": "refactor.extract_fn",
    "version": "1.0.0",
    "template": "Given the following code:\n{{code}}\n\nExtract a function named {{fn_name}} ...",
    "parameters": ["code", "fn_name"],
    "default_model": "default",
    "default_max_tokens": 1024,
    "default_temperature": 0,
    "default_schema_id": "patch_bundle"
}
```

Template uses `{{param}}` placeholders. Final prompt text is never in app code.

### Prompt Compilation

`compile(prompt_template, args) -> final_prompt_text`

Replaces `{{param}}` with arg values. Validates all parameters are provided.

## 6. Context Store

Content-addressed blob store for passing context to LLM calls.

### Layout

```
context_store/
    blobs/
        <sha256_hash>           // raw content
    index.json                  // optional: hash -> metadata
```

### Operations

- `put(content) -> hash`: SHA-256 the content, store in blobs/, return hash
- `get(hash) -> content`: Read from blobs/
- `pack(hashes, max_bytes) -> Vec<(hash, content)>`: Concatenate blobs up to size limit

### Triage

When total context exceeds `llm_max_context_bytes` policy:
- Select blobs in the order provided (stable ordering)
- Truncate at the byte limit
- Return only included blobs

## 7. Policy Budgets

Extend `PolicySet` with LLM-specific fields:

```rust
pub struct LlmPolicy {
    pub total_token_budget: u64,      // 0 = unlimited
    pub max_calls: u64,               // 0 = unlimited
    pub allowed_models: Vec<String>,  // empty = all allowed
    pub max_context_bytes: u64,       // 0 = unlimited
    pub prompt_allowlist: Vec<String>, // empty = all allowed
}
```

### Enforcement

Before executing an LlmCall effect:
1. Check `prompt_id` against `prompt_allowlist` (if non-empty)
2. Check `model` against `allowed_models` (if non-empty)
3. Check `max_output_tokens` against remaining `total_token_budget`
4. Check call count against `max_calls`
5. Check total context size against `max_context_bytes`

On violation: return `FrameworkError::PolicyViolation(...)`.

### Budget Tracking

The runtime tracks cumulative token usage and call count per session.

## 8. Deterministic Cache

### Layout

```
llm_cache/
    <cache_key_hash>.json
```

### Cache Entry

```json
{
    "cache_key": "sha256:...",
    "prompt_id": "...",
    "model": "...",
    "schema_id": "...",
    "prompt_hash": "sha256:...",
    "result": { ... },
    "created_at": "2026-01-01T00:00:00Z"
}
```

### Cache Modes

- `"read"`: Read from cache; do not write
- `"write"`: Write to cache; do not read (force refresh)
- `"readwrite"`: Read first; write on miss
- `"off"`: Skip cache entirely

### Determinism

Cache files use canonical JSON (sorted keys, minimal whitespace).

## 9. Capability Gateway: LLM Backend

### Mock Backend (default)

Returns fixed responses based on `output_schema_id`:
- `"patch_bundle"` -> Returns a minimal valid PatchBundle
- Any other schema -> Returns a minimal valid JSON object matching the schema

Used in tests and CI. Deterministic by definition.

### External Backend (opt-in)

Calls a real LLM API. Disabled by default. Enabled via:
- Environment variable: `LLM_BACKEND=external`
- Or runtime configuration

The external backend is behind a feature flag and not required for any test.

## 10. Framework Integration

### Purity Preserved

- `update()` and `view()` remain pure (no capabilities)
- `update()` returns `Effect::LlmCall` as data
- The framework runtime executes the effect
- Result delivered as `Msg::LlmResult { request_id, result }`

### Request ID

`request_id` = first 16 hex chars of the cache key hash. Stable and deterministic.

### Callback Flow

```
update(state, msg) -> (new_state, [Effect::LlmCall { ... }])
  |
  v  framework executes effect
  |
  v  result delivered as:
update(new_state, Msg::LlmResult { request_id, result })
```

## 11. Orchestrator Integration

### LlmMockGateAdapter

Verifies no external LLM calls happen during deterministic test pipelines.
Reports estimated token consumption from request/response sizes.

## 12. Non-Goals

- Real LLM API integration (external backend is a stub)
- Streaming responses
- Multi-turn conversations
- Fine-tuning or training
- Token counting accuracy (estimates only)
