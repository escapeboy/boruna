# LLM Handler Examples — BYOH Reference Library

This directory contains reference `CapabilityHandler` implementations for the most common LLM providers. They demonstrate the **Bring Your Own Handler (BYOH)** integration pattern Boruna uses for `Capability::LlmCall` — see [`docs/guides/llm-integration.md`](../../docs/guides/llm-integration.md) for the design rationale.

**These are reference snippets, not compiled examples.** Each handler is ~80–120 LOC of self-contained code; copy the `handler.rs` into your integrator crate and adapt for your provider, secret management, observability, and routing concerns.

## Why BYOH (and not a shipped adapter crate)?

Provider APIs churn. API keys need org-specific secret management. Production integrators usually already have an LLM client. Shipping default handlers in core would constrain integrators and tie Boruna's release cadence to provider releases. See the [LLM integration guide](../../docs/guides/llm-integration.md#why-byoh) for the full discussion.

## Available handlers

| Provider | Use when | Path |
|---|---|---|
| **OpenAI** | Talking directly to api.openai.com | [`openai/`](./openai/) |
| **Anthropic** | Talking directly to api.anthropic.com | [`anthropic/`](./anthropic/) |
| **Ollama** | Local development, air-gapped CI, deterministic-with-seed runs | [`ollama/`](./ollama/) |
| **vLLM (and OpenAI-compatible proxies)** | Self-hosted vLLM, OpenRouter, Together, Groq, LiteLLM, SGLang | [`vllm/`](./vllm/) |
| **AWS Bedrock** | Bedrock-hosted Claude / Llama / Titan / Mistral / Cohere via SigV4 | [`bedrock/`](./bedrock/) |

Each subdirectory has a `handler.rs` and a `README.md`. The READMEs document the per-provider gotchas (auth, response shape, determinism options, what the reference deliberately omits).

## Multi-provider routing

Boruna ships a **pure-routing** primitive in `boruna-vm`: `LlmRouterHandler` (`crates/llmvm/src/capability_gateway.rs`). It dispatches `Capability::LlmCall` to a registered provider handler based on a `provider/model` prefix in `args[1]`. The router itself adds zero provider compatibility commitments to core.

Compose any subset of the handler examples above into a router:

```rust
use std::collections::BTreeMap;
use boruna_vm::capability_gateway::{
    CapabilityHandler, LlmRouterHandler, MockHandler,
};

let mut providers: BTreeMap<String, Box<dyn CapabilityHandler>> = BTreeMap::new();
providers.insert("openai".into(),    Box::new(OpenAiHandler::from_env()?));
providers.insert("anthropic".into(), Box::new(AnthropicHandler::from_env()?));
providers.insert("ollama".into(),    Box::new(OllamaHandler::from_env()?));

let router = LlmRouterHandler::new(providers, Box::new(MockHandler));

// In .ax: let r1 = llm_call("Summarize:", "openai/gpt-4o-mini")
//         let r2 = llm_call("Critique:",  "anthropic/claude-sonnet-4-6")
// Router parses the prefix and dispatches to the right handler.
```

See [`router_setup.rs`](./router_setup.rs) for a more complete example, including loading a routing table from `providers.toml.example`.

## Declarative routing config (optional)

For integrators who want to declare their provider lineup in config rather than code, [`providers.toml.example`](./providers.toml.example) shows a documented schema. The format is a starting point — adopt, extend, or ignore as fits your deployment. Boruna does not parse this file; the schema is just a convention for integrators who want a uniform shape.

[`router_setup.rs`](./router_setup.rs) shows a reference parser that turns the toml into an `LlmRouterHandler`.

## Pattern across all examples

Every reference handler follows the same shape so integrators can copy-and-tweak:

1. **Construct from env** — `from_env() -> Result<Self, String>`. Read API keys, base URLs, model defaults.
2. **Builder methods** — `with_model`, `with_endpoint`, `with_api_key` (where applicable). Override env defaults programmatically.
3. **`handle_llm_call`** — extract the prompt from `args[0]` (a `Value::String`), POST to the provider, parse the response. Return `Value::String`.
4. **`CapabilityHandler::handle`** — match `Capability::LlmCall` and route to `handle_llm_call`; everything else falls through to `MockHandler`.
5. **`temperature: 0` by default** — for replay-friendliness. Override in your fork if you need sampling diversity.

## Determinism guidance

Across all providers:
- **Pin model versions** to dated snapshots (`gpt-4o-mini-2024-07-18`, not `gpt-4o-mini`).
- **`temperature: 0`** in the request body.
- **Set `seed`** where the provider supports it (Ollama: yes; vLLM: yes; Anthropic: no; OpenAI: yes via `seed` parameter; Bedrock-hosted models: varies).
- **Record + replay**: `boruna workflow run --record-net-to <tape>` captures HTTP traffic; `--replay-net-from <tape>` replays it. This works for HTTP-based handlers (everyone except Bedrock, which goes through the AWS SDK rather than Boruna's HTTP recorder).

See [`docs/guides/llm-integration.md`](../../docs/guides/llm-integration.md) for the full discussion of why BYOH, what the trait contract looks like, and how to wire a handler into a `WorkflowRunner` invocation.
