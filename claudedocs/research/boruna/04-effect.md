# Boruna Research — 04: LLM Effect / Integration (`boruna-effect`)

Slice: `crates/llm-effect/` (dir) → crate `boruna-effect`, plus reference handlers in
`examples/llm_handlers/`. Read-only. Every claim cites `path:line`; unverified items are
marked "not verified", not guessed.

---

## 1. Purpose & Architecture

`boruna-effect` is the deterministic LLM-call subsystem: prompt registry, content-addressed
context store, response cache, per-session policy, and record/replay. Its public entry point
is `LlmGateway::execute(&LlmRequest)` (`crates/llm-effect/src/gateway.rs:104`).

**Critical architectural finding — TWO disconnected LLM subsystems:**

1. **`boruna-effect` (`LlmGateway`)** — the full-featured deterministic gateway described in
   this report. It is **NOT wired into any binary or runtime path.** There are zero
   `LlmGateway::new` call sites and zero `use boruna_effect` statements outside the crate's
   own tests (verified: `grep -rn 'LlmGateway::new\|use boruna_effect' --include=*.rs`
   returns nothing outside `crates/llm-effect/src`). `crates/llmfw/Cargo.toml:11` and
   `orchestrator/Cargo.toml:96` declare a dependency, but no source under `crates/llmfw/src/`
   or `orchestrator/src/` references it (the only orchestrator hit is a `cargo test -p
   boruna-effect` invocation in `orchestrator/src/adapters/mod.rs:383-385`).

2. **VM capability gateway (`LlmRouterHandler` + `MockHandler`)** in
   `crates/llmvm/src/capability_gateway.rs` — this is the **actual runtime path** for
   `Capability::LlmCall`. `MockHandler` returns `{status:"ok", mock:true}`
   (`capability_gateway.rs:177-184`). `LlmRouterHandler` (`capability_gateway.rs:322-411`)
   dispatches by a `provider/model` prefix in `args[1]` to bring-your-own-handler (BYOH)
   providers. **It has no cache, no context store, no prompt registry, and no per-call token
   budget of its own** — none of the effect crate's determinism machinery participates in a
   live `llm_call`.

Consequence: the deterministic replay/cache/policy story is implemented and unit-tested in
`boruna-effect`, but the code that actually executes an `llm.call` at runtime does not use
any of it. The two halves have never been connected. (This is the single most important
finding of this slice — see GAP G1.)

**Determinism model (as designed in the effect crate):**
- Cache key = SHA-256 over `prompt_id`, canonical-JSON `args`, **sorted** `context_refs`,
  `model`, `max_output_tokens`, `temperature`, `output_schema_id`, prompt content-hash,
  schema content-hash (`normalize.rs:174-223`). Order-invariant on context refs
  (`normalize.rs:191-192`, test `normalize.rs:273-290`).
- `ExecutionMode::Replay`: cache hit returns stored value; cache miss is a **hard error**
  `LlmReplayMiss` (`gateway.rs:126-149`).
- Prompt/schema integrity: content-hash verification via `PromptRegistry::verify()`
  (`prompt.rs:176-216`).

---

## 2. Component Inventory

| File | Responsibility | Key types / fns | Status |
|---|---|---|---|
| `src/lib.rs` | Module wiring | 6 pub mods (`gateway.rs:1-9`) | Complete |
| `src/gateway.rs` | Orchestrates a call: context sizing → policy → cache key → cache read → **mock** response → cache write → log | `LlmGateway`, `ExecutionMode{Mock,Record,Replay}`, `execute()` (:104), `generate_mock_response()` (:197), `validate_output()` (:206) | **Partial** — backend is mock-only; `Record` mode is inert (see G2) |
| `src/normalize.rs` | Request parsing + canonical JSON + deterministic cache key | `LlmRequest`, `CacheMode`, `parse_llm_request()` (:60), `canonical_json()` (:120), `compute_cache_key()` (:174) | Complete |
| `src/prompt.rs` | Prompt/schema registry, `{{var}}` template compilation, content-hash integrity | `PromptRegistry`, `PromptTemplate`, `register_prompt()` (:91), `compile_prompt()` (:220), `verify()` (:176), `content_hash()` (:254) | **Partial** — path-safety gap (S1), compiled prompt unused by gateway (G3) |
| `src/context.rs` | Content-addressed blob store for context, byte-bounded packing | `ContextStore`, `put()` (:31), `get()` (:39), `validate_hash()` (:20), `pack()` (:53) | Complete |
| `src/cache.rs` | Filesystem LLM response cache, canonical JSON, hex-key validation | `LlmCache`, `CacheEntry`, `validate_key()` (:36), `entry_path()` (:49), `read/write/exists/delete/clear` | Complete |
| `src/policy.rs` | Per-session LLM quota enforcement | `LlmPolicy` (token budget, max_calls, model allowlist, context bytes, prompt allowlist), `LlmUsage`, `check_policy()` (:52) | Complete (but unwired — see G1) |
| `src/tests.rs` | Integration: record→replay, schema validation, cache-key determinism, policy | 9 tests | Complete |
| `examples/llm_handlers/anthropic/handler.rs` | Reference BYOH handler, Anthropic Messages API | `AnthropicHandler`, real `ureq` POST to `api.anthropic.com` (:71-81) | **Demo (real HTTP)** |
| `examples/llm_handlers/openai/handler.rs` | Reference BYOH, OpenAI Chat Completions | `OpenAiHandler`, real POST to `api.openai.com` (:61-67) | **Demo (real HTTP)** |
| `examples/llm_handlers/ollama/handler.rs` | Reference BYOH, local Ollama (`/api/generate`, seed=42) | `OllamaHandler` (:79-85) | **Demo (real HTTP)** |
| `examples/llm_handlers/vllm/handler.rs` | Reference BYOH, OpenAI-compatible endpoints | `VllmHandler`, optional bearer auth (:84-93) | **Demo (real HTTP)** |
| `examples/llm_handlers/bedrock/handler.rs` | AWS Bedrock skeleton | `BedrockHandler`; `BedrockClient::invoke_model` = `unimplemented!()` (:44-50) | **Stub** |
| `examples/llm_handlers/router_setup.rs` | Load `providers.toml` → `LlmRouterHandler` | `build_router()` (:150); provider `mod`s are `unimplemented!()` placeholders | **Snippet (non-compiling by design)** |
| `examples/llm_handlers/providers.toml.example` | Declarative provider-routing convention | `[providers.*]` with `api_key_env` | Doc |

Note: `examples/llm_handlers/` is documentation/reference; the four real handlers make live
HTTP calls via `ureq` but are explicitly "NOT a production handler" (e.g.
`anthropic/handler.rs:5-6`). They are not compiled into the workspace.

---

## 3. GAPS

- **G1 — `boruna-effect` is entirely unwired [HIGH].** No runtime consumes `LlmGateway`,
  `LlmPolicy`, `LlmCache`, `ContextStore`, or `PromptRegistry`. Verified: no `LlmGateway::new`
  / `use boruna_effect` outside `crates/llm-effect/src` (only Cargo dep declarations at
  `crates/llmfw/Cargo.toml:11`, `orchestrator/Cargo.toml:96`). The live `llm.call` path
  (`crates/llmvm/src/capability_gateway.rs:177`, `:322`) reimplements a much thinner routing
  layer with no cache/replay/context/token-budget. The platform's headline "deterministic,
  replayable LLM calls with policy budgets" is therefore **not exercised end-to-end** by any
  binary; it exists only as tested-but-dormant library code.

- **G2 — `ExecutionMode::Record` is inert [MED].** `execute()` never branches on `Record`
  (`gateway.rs:104-184`); the only response generator is `generate_mock_response()`
  (`gateway.rs:153, 197`). The doc comment "Record mode — calls external (or mock) and logs
  responses" (`gateway.rs:18-19`) is aspirational — there is no external-call code path in the
  crate. `Mock` and `Record` behave identically. "Replay of real responses" is thus never
  tested with a real backend (`tests.rs:37-79` records mock output).

- **G3 — Compiled prompt text is never sent anywhere [MED].** `PromptRegistry::compile_prompt`
  (`prompt.rs:220`) substitutes args into the template, but `LlmGateway::execute` never calls
  it. `execute()` derives the cache key from `req.args` (`gateway.rs:122` →
  `normalize.rs:186`) and returns a mock; the actual prompt string is neither assembled nor
  transmitted. So the prompt registry is currently only a hashing/integrity fixture, not a
  prompt-delivery mechanism.

- **G4 — `compute_context_bytes` silently ignores missing/invalid context refs [LOW].**
  `gateway.rs:186-194` does `if let Ok(content) = self.context_store.get(hash)` and skips
  errors. A ref that fails hex validation or is absent contributes 0 bytes, so a
  `max_context_bytes` policy could be under-counted. Low impact because the store is
  content-addressed and the effect crate is unwired.

- **G5 — `LlmRouterHandler` has no per-call quota / model allowlist [MED].** The live path
  (`capability_gateway.rs:359-411`) validates only that `args[1]` parses as `provider/model`
  and that the provider is registered. Token budgets, call counts, and model allowlists exist
  **only** in the unwired `policy.rs`. In the live VM, `llm.call` is gated solely by the
  generic capability allow/deny (framework mapping `crates/llmfw/src/executor.rs:133`;
  capability id 7 at `crates/llmbc/src/capability.rs:38,55`); quantitative LLM budgets are not
  enforced at runtime. (The generic capability gate wrapper that calls `handle()` lives in the
  VM slice and was not read here — not verified.)

---

## 4. Security (in scope)

### Cache / context path-key injection
- **`LlmCache` key validation — SAFE.** `validate_key()` (`cache.rs:36-46`) rejects empty and
  any non-`ascii_hexdigit` key; `entry_path()` (`cache.rs:49-54`) strips the `sha256:` prefix
  then validates before `join`. All of `read/write/exists/delete` route through `entry_path`.
  A key like `../../etc/passwd` is rejected (contains `/`, `.`). **[SAFE]**
- **`ContextStore` hash validation — SAFE.** `validate_hash()` (`context.rs:20-28`) is hex-only;
  applied in `get()` (`context.rs:40`) and `exists()` (`context.rs:47`). `put()`
  (`context.rs:31-36`) computes the hash itself via `sha256_hex`, so the filename is always
  hex and needs no external validation. **[SAFE]**
- **`PromptRegistry` filename construction — path traversal, NEEDS-REVIEW.**
  `register_prompt` builds the filename as `format!("{}.prompt.json", template.id)` with **no
  validation of `template.id`** (`prompt.rs:96-97`); `register_schema` builds
  `format!("schemas/{schema_id}.json")` with no validation of `schema_id`
  (`prompt.rs:119-120`). A `template.id` / `schema_id` containing `/` or `..` writes outside
  `base_dir` (arbitrary-file write). Symmetrically, `load_prompt` / `load_schema` join
  `entry.file` taken straight from the deserialized `registry.json` manifest with no
  validation (`prompt.rs:142, 154`), so a crafted `registry.json` yields arbitrary-file read.
  This crate does **not** replicate the codebase's documented `..`/absolute-path rejection
  used elsewhere (PatchBundle, per MEMORY.md). No live exploit today because the registry is
  unwired (G1) and callers in-repo pass literal test IDs, but if `boruna-effect` is ever wired
  with user-influenced `prompt_id`/`schema_id`, this is a traversal sink. **[NEEDS-REVIEW]**

### Prompt injection
- **Template substitution has no escaping/guardrails — NEEDS-REVIEW.** `compile_prompt`
  (`prompt.rs:234-237`) does raw `result.replace("{{key}}", value)` of every arg value into
  the template. Untrusted arg values are interpolated verbatim; there is no delimiter
  hardening, no separation of instructions vs. data, and multi-pass replacement means a value
  that itself contains `{{other}}` could interact with later substitutions (order is BTreeMap
  key order). This is the classic indirect-prompt-injection surface. **Mitigating context:**
  the compiled prompt is never actually sent (G3), and in the live VM path the raw prompt is
  `args[0]` passed directly to the provider as user `content`
  (`anthropic/handler.rs:65`, `openai/handler.rs:55`) with likewise no sanitization — the
  platform provides no prompt-injection guardrail at either layer. **[NEEDS-REVIEW]**
- Context blobs are packed by hash and (in the dead path) counted for size only
  (`gateway.rs:186-194`); no untrusted-content tainting or provenance is tracked. **[NEEDS-REVIEW]**

### Secret / API-key handling
- **No hardcoded secrets anywhere in scope — SAFE.** All reference handlers read keys from env
  vars: `ANTHROPIC_API_KEY` (`anthropic/handler.rs:28`), `OPENAI_API_KEY`
  (`openai/handler.rs:28`), `VLLM_API_KEY` (`vllm/handler.rs:37`); Bedrock uses the AWS
  credential chain (`bedrock/handler.rs:80-81`); Ollama needs none. Keys are held in-struct as
  `String` and sent via `x-api-key` / `Authorization: Bearer` headers
  (`anthropic/handler.rs:75`, `openai/handler.rs:64`, `vllm/handler.rs:89`). **[SAFE]**
- **`providers.toml.example` explicitly forbids inlining keys** — uses `api_key_env` and warns
  "Don't put the key itself in the toml" (`providers.toml.example:23-25`). `build_router`
  resolves keys from the named env var, not the file (`router_setup.rs:162-164`). **[SAFE]**
- Minor hygiene notes (not vulns): keys live in plain `String` (not zeroized on drop); error
  strings interpolate the request error but **not** the key
  (e.g. `openai/handler.rs:67`) — no key leakage in error paths observed. **[SAFE]**

### Capability gating on `LlmCall` (id = 7)
- Capability is defined and numbered 7 (`crates/llmbc/src/capability.rs:16,38,55,71,87`);
  framework maps `EffectKind::LlmCall → Capability::LlmCall`
  (`crates/llmfw/src/executor.rs:133`); the VM dispatches it in `MockHandler`
  (`capability_gateway.rs:177`) and routes it in `LlmRouterHandler`
  (`capability_gateway.rs:359`). Whether the enclosing capability-gateway wrapper checks the
  active `Policy` (allow/deny) *before* invoking `handle()` is in the VM slice and was **not
  verified here**. What is confirmed in this slice: the *quantitative* LLM policy (budgets,
  allowlists) in `policy.rs` is not on the live path (G1/G5). **[NEEDS-REVIEW]**

---

## 5. Coverage Statement

Read in full: all 8 source files of `boruna-effect`
(`lib.rs`, `cache.rs`, `context.rs`, `gateway.rs`, `normalize.rs`, `policy.rs`, `prompt.rs`,
`tests.rs`) and all reference material in `examples/llm_handlers/` (anthropic, openai, ollama,
vllm, bedrock `handler.rs`; `router_setup.rs`; `providers.toml.example`). Cross-checked wiring
by grepping the whole repo for `LlmGateway` / `boruna_effect` consumers and read the live
`LlmCall` path in `crates/llmvm/src/capability_gateway.rs:160-411` plus the capability
definition (`crates/llmbc/src/capability.rs`) and framework mapping
(`crates/llmfw/src/effect.rs`, `executor.rs:133`). **Not covered / not verified:** the READMEs
in each handler dir (skimmed titles only); the VM-side capability-gateway *policy-check
wrapper* that decides allow/deny before `handle()` (belongs to the VM slice); the framework
`executor.rs` effect-dispatch body beyond the line-133 mapping.
