# vLLM (and OpenAI-compatible) LLM Handler — Reference Example

Reference implementation of a `CapabilityHandler` that routes `llm.call` to any **OpenAI-compatible** chat-completions endpoint. Use this for:

- **vLLM** — self-hosted, GPU-backed
- **OpenRouter** — managed multi-model proxy
- **Together AI / Groq / Fireworks** — managed inference
- **LiteLLM proxy** — your own routing layer
- **SGLang** — self-hosted, structured-output capable

**Reference only — not a production handler.** See [`docs/guides/llm-integration.md`](../../../docs/guides/llm-integration.md) for the BYOH integration model.

## Why have this separate from the OpenAI handler?

The wire shape is identical (vLLM, OpenRouter, et al. mimic OpenAI's Chat Completions). The differences this handler makes explicit:

- **Configurable base URL** via `VLLM_BASE_URL`. The OpenAI handler is hardcoded to `https://api.openai.com/v1`.
- **Optional auth.** Self-hosted vLLM typically runs without an API key; managed proxies require one. The handler accepts both.
- **No safe default model.** Each endpoint serves its own catalog; you must call `.with_model("...")` explicitly.

If you're pointing Boruna at OpenAI itself, prefer the [OpenAI example](../openai/) — its API-key-required signature avoids accidental anonymous calls.

## Files

- `handler.rs` — the `VllmHandler` struct and its `CapabilityHandler` impl.

## Configuration

| Variable | Required? | Example |
|---|---|---|
| `VLLM_BASE_URL` | yes | `http://localhost:8000/v1`, `https://openrouter.ai/api/v1`, `https://api.together.xyz/v1` |
| `VLLM_API_KEY` | optional | required by managed proxies; not by self-hosted vLLM |
| (model) | required (programmatic) | call `.with_model("meta-llama/Llama-3-70b-Instruct")` etc. |

## Setup examples

**Self-hosted vLLM:**

```bash
# vLLM listens on port 8000 by default
vllm serve meta-llama/Llama-3.1-8B-Instruct
export VLLM_BASE_URL=http://localhost:8000/v1
# No API key needed
```

**OpenRouter:**

```bash
export VLLM_BASE_URL=https://openrouter.ai/api/v1
export VLLM_API_KEY=sk-or-v1-...
# Then .with_model("anthropic/claude-sonnet-4") etc.
```

**Together AI:**

```bash
export VLLM_BASE_URL=https://api.together.xyz/v1
export VLLM_API_KEY=...
# Then .with_model("meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo")
```

## What it doesn't do

- **Multi-provider routing.** Single endpoint per handler. See `examples/llm_handlers/router_setup.rs` to compose multiple `VllmHandler` instances under `LlmRouterHandler`.
- **Streaming.** Boruna's capability calls are synchronous.
- **Provider-specific extensions.** vLLM has guided-decoding parameters (`guided_json`, `guided_regex`) that the OpenAI shape doesn't carry. If you depend on those, extend the request `body` JSON in your fork.

## Determinism

Same posture as the OpenAI handler: `temperature: 0` (set by default). If you serve through vLLM directly, also pin `seed` in the request body. For managed proxies, determinism depends on the underlying model serving stack — read your proxy's docs.

See [`docs/guides/llm-integration.md`](../../../docs/guides/llm-integration.md) for the full discussion.
