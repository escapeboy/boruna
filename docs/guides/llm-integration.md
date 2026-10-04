# LLM Integration Guide

Boruna ships five LLM providers that answer `llm_call(prompt, "provider/model")` in `.ax`:
OpenAI, any OpenAI-compatible endpoint (vLLM, OpenRouter, Together, Groq, LiteLLM), Anthropic,
Ollama and AWS Bedrock. You turn them on from the CLI with a `providers.json` and `--live`
(see [Built-in providers](#built-in-providers)). If you embed Boruna in your own Rust program you
can still plug in your own handler instead: that is the **Bring Your Own Handler (BYOH)** model
described in the rest of this guide.

## Built-in providers

```json
{
  "providers": {
    "openai":    { "kind": "openai",        "api_key_env": "OPENAI_API_KEY" },
    "anthropic": { "kind": "anthropic",     "api_key_env": "ANTHROPIC_API_KEY", "max_tokens": 1024 },
    "local":     { "kind": "ollama",        "base_url": "http://localhost:11434" },
    "vllm":      { "kind": "openai_compat", "base_url": "http://gpu-box:8000/v1" },
    "bedrock":   { "kind": "bedrock",       "region": "us-east-1" }
  }
}
```

```bash
boruna run review.ax --live --providers providers.json
boruna workflow run my_workflow --policy policy.json --live --providers providers.json --record
```

- The name before the slash in the model argument picks the provider:
  `llm_call(p, "local/llama3.1")` goes to the `local` entry with model `llama3.1`.
- API keys come from the environment variable named in `api_key_env`; the file holds only the
  variable's name. Bedrock reads `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and optionally
  `AWS_SESSION_TOKEN`, and signs requests with SigV4. A missing variable is reported before the
  program runs.
- Optional per provider: `base_url`, `max_tokens`, `temperature`, `timeout_ms`.
- Without `--live` the deterministic mock answers and no request is made.
- The policy still decides: `llm.call` must be allowed, and the calling function must declare
  `!{llm.call}`.
- Every call and its reply are recorded (VM event log, and `CapabilityInvoked` in workflow
  evidence). Keys never appear in arguments, replies, logs or evidence. A `run --record` log
  replays the recorded replies without calling the provider.
- Provider replies are not deterministic. Determinism holds for replay of a recorded run, not for
  two live runs.
- `--providers` with the older capability-keyed format (`{"llm.call": {...}}`) is still accepted
  but only validated; it does not call providers.
- `workflow resume` and `workflow schedule` take the same `--providers` flag; with `--live` the
  resumed or scheduled steps call the providers. `workflow eval --live` calls each side's
  providers (see the [model evaluation guide](model-eval.md)).

### Local programs (`kind: "command"`)

A `command` provider runs a program on your machine for each call, such as the command-line tool
that comes with a Claude, ChatGPT or Gemini subscription. No API key is needed; the program uses
its own login.

```json
{
  "providers": {
    "claude": { "kind": "command",
                "command": ["claude", "-p", "--tools", "", "--strict-mcp-config",
                            "--no-session-persistence", "--model", "{model}"] },
    "codex":  { "kind": "command",
                "command": ["codex", "exec", "--sandbox", "read-only",
                            "--skip-git-repo-check", "--ephemeral", "-"] },
    "gemini": { "kind": "command",
                "command": ["gemini", "--approval-mode", "plan", "-m", "{model}", "-p", ""] }
  }
}
```

- `llm_call(p, "claude/sonnet")` runs the `claude` entry with `{model}` replaced by `sonnet`. The
  program is started directly, without a shell. The prompt is written to its stdin; its stdout,
  without trailing whitespace, is the reply.
- A non-zero exit fails the call with the exit code and the end of stderr (e.g. "not logged in").
- The model comes from `.ax` code, so a model that starts with `-` or contains control characters
  is refused: otherwise `llm_call(p, "claude/--dangerously-skip-permissions")` would pass a flag
  to the tool.
- A program may exit without reading the whole prompt; that is not an error.
- `timeout_ms` (default 300000) kills a program that runs too long; `max_output_bytes` (default
  4 MiB) refuses a larger reply. `api_key_env`, `base_url` and `region` are not accepted.
- Works in every build: it does not need the `http` feature.
- Recording and replay work as for the other providers: a replay uses the recorded reply and does
  not start the program.
- **These tools are agents.** With their defaults they can edit files and run commands, and
  Boruna's policy cannot see or stop what the program does. Switch their tools off, as above:
  `claude --tools ""`, `codex --sandbox read-only`, `gemini --approval-mode plan`. Check the
  flags of your tool's version.
- Whoever can edit `providers.json` can make Boruna run any program as you. Treat the file like
  a script.
- Codex with a ChatGPT account accepts only the models its config allows, so the example passes
  no model and uses the account's default. On Windows, npm-installed tools are `.cmd` files:
  write `"claude.cmd"`.

This document explains why, what the contract looks like, and how to wire up handlers for common providers (OpenAI, Anthropic, vLLM, Ollama).

## Why BYOH?

Three reasons.

1. **Provider churn doesn't destabilize Boruna.** OpenAI's API changes; so does Anthropic's; so does the long tail. If a default handler shipped in core, every provider release would risk a Boruna patch release. With BYOH, your handler tracks your provider on your release cadence.

2. **API key management belongs in your application.** Boruna is a deterministic execution chassis. Secret loading, rotation, observability, billing attribution — all of these are concerns that vary wildly per organization. Pushing them into core would constrain integrators (e.g. FleetQ) who already have their own conventions.

3. **Most production integrators already have an LLM client.** The platform's primary integrators run their own LLM infrastructure (vLLM clusters, OpenRouter proxies, custom routing). Shipping a default handler would just add another thing for them to override.

A "convenience CLI handler" was considered and rejected — it would lock the project into provider compatibility commitments without serving the production integrators it's primarily meant for.

## The contract

`CapabilityHandler` is a single-method trait in `boruna-vm::capability_gateway`:

```rust
pub trait CapabilityHandler: Send {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String>;
}
```

For LLM calls, the relevant capability is `Capability::LlmCall`. The first argument is conventionally the prompt (a `Value::String`); subsequent arguments are provider-specific. The return is the LLM's response, also conventionally `Value::String` or `Value::Map { "content": ..., "finish_reason": ... }`.

The handler runs inside the VM's step execution. It has Send bound but no Sync — each VM instance owns its own handler. In the concurrent execution path (`--concurrency > 1`), each worker constructs its own handler.

## Minimal example

```rust
use boruna_bytecode::{Capability, Value};
use boruna_vm::capability_gateway::CapabilityHandler;

pub struct OpenAiHandler {
    api_key: String,
    client: ureq::Agent,
}

impl OpenAiHandler {
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| "OPENAI_API_KEY not set".to_string())?;
        Ok(Self {
            api_key,
            client: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(60))
                .build(),
        })
    }
}

impl CapabilityHandler for OpenAiHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::LlmCall => self.handle_llm_call(args),
            // Delegate everything else to the default mock handler.
            _ => boruna_vm::capability_gateway::MockHandler.handle(cap, args),
        }
    }
}

impl OpenAiHandler {
    fn handle_llm_call(&mut self, args: &[Value]) -> Result<Value, String> {
        let prompt = args
            .first()
            .and_then(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .ok_or_else(|| "llm.call: first arg must be a String prompt".to_string())?;

        let body = serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": prompt}],
        });

        let resp = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| format!("openai request failed: {e}"))?;

        let json: serde_json::Value = resp
            .into_json()
            .map_err(|e| format!("openai response parse: {e}"))?;
        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| "openai response missing choices[0].message.content".to_string())?;

        Ok(Value::String(content.to_string()))
    }
}
```

A complete reference handler lives in `examples/llm_handlers/openai/`. The directory contains `handler.rs` (a self-contained module you can copy into your integrator crate) and a README documenting the per-provider gotchas.

## Provider variants

Reference handlers ship for the most common providers (post1-T-1.2). Each is a copy-and-tweak template, **not** a compiled crate Boruna pulls in:

| Provider | Use when | Path |
|---|---|---|
| OpenAI | api.openai.com | [`examples/llm_handlers/openai/`](../../examples/llm_handlers/openai/) |
| Anthropic | api.anthropic.com | [`examples/llm_handlers/anthropic/`](../../examples/llm_handlers/anthropic/) |
| Ollama | local development, air-gapped CI | [`examples/llm_handlers/ollama/`](../../examples/llm_handlers/ollama/) |
| vLLM (and OpenAI-compatible proxies) | self-hosted vLLM, OpenRouter, Together, Groq, LiteLLM | [`examples/llm_handlers/vllm/`](../../examples/llm_handlers/vllm/) |
| AWS Bedrock | Bedrock-hosted Claude / Llama / Titan / Mistral / Cohere | [`examples/llm_handlers/bedrock/`](../../examples/llm_handlers/bedrock/) |

The umbrella [`examples/llm_handlers/README.md`](../../examples/llm_handlers/README.md) gives the cross-provider overview and links each handler's gotchas (auth header, response path, determinism options).

A documented `providers.toml.example` schema and a [`router_setup.rs`](../../examples/llm_handlers/router_setup.rs) reference parser show one convention for declaring your provider lineup in config rather than code. The format is a starting point — Boruna does not parse it.

For multi-provider routing, use the built-in `LlmRouterHandler` (covered next), or have your handler switch on a `model` argument (passed as `args[1]`) or on a thread-local context.

### `LlmRouterHandler` — built-in multi-provider dispatch (sprint `0.4-S13`)

If you have multiple providers wired in (OpenAI + Anthropic + a local Ollama, say) and don't want to write your own dispatch logic, Boruna ships a `LlmRouterHandler` in `boruna-vm::capability_gateway`. It takes a registry of provider handlers keyed by name and dispatches each `Capability::LlmCall` based on a `provider/model` prefix in `args[1]`:

```rust
use std::collections::BTreeMap;
use boruna_vm::capability_gateway::{
    CapabilityHandler, LlmRouterHandler, MockHandler,
};

let mut providers: BTreeMap<String, Box<dyn CapabilityHandler>> = BTreeMap::new();
providers.insert("openai".into(), Box::new(my_openai_handler));
providers.insert("anthropic".into(), Box::new(my_anthropic_handler));
providers.insert("ollama".into(), Box::new(my_ollama_handler));

// Non-LLM calls pass through to the fallback (typically MockHandler
// in tests; in production you'd plug a HttpHandler etc).
let router = LlmRouterHandler::new(providers, Box::new(MockHandler));

// Pass `router` into `CapabilityGateway::with_handler` like any
// other CapabilityHandler.
```

`.ax` callers then write:

```
let response = llm_call("Summarize:", "openai/gpt-4")
let response2 = llm_call("Translate:", "anthropic/claude-3-5-sonnet-20241022")
```

The full model string (including the `provider/` prefix) is forwarded to the provider's handler unchanged, so providers can use the prefix internally (e.g. for billing tags).

The router does not impose provider compatibility commitments on core — it's pure routing logic. You still bring your own per-provider handler implementation. `boruna-vm` ships zero provider HTTP code.

## Determinism considerations

LLM calls are inherently non-deterministic at the model layer (sampling temperature, randomness, model version drift). Three things keep Boruna's determinism contract intact even with non-deterministic handlers:

1. **`output_hash` reflects what was actually returned.** If the LLM returned different text on two runs, the `output_hash` differs and downstream replay-comparison surfaces the divergence.
2. **`net.fetch` record-replay** (sprint `0.5-S7`) lets you record an LLM session and replay deterministically — useful for testing and audit.
3. **Persisted `output_json`** (sprint `0.3-S2b`) means a successful LLM call's output survives process restarts. Resume picks up the recorded value rather than re-calling the LLM.

For workflows that need bit-identical replay, set `temperature: 0` and a pinned `model` version, then use `--record-net-to` to capture the network transactions for future replay.

## Capability policy

Steps that call LLMs must declare the capability in the workflow definition:

```json
{
  "steps": {
    "summarize": {
      "kind": "source",
      "source": "steps/summarize.ax",
      "capabilities": ["llm.call"],
      "budget": { "max_calls": 3 }
    }
  }
}
```

The `budget.max_calls` field caps how many `llm.call` invocations the step is allowed. Exceeding the budget returns a `CapabilityBudgetExceeded` runtime error. Per-step budgeting is enforced by `CapabilityGateway` regardless of which handler is plugged in.

## Testing your handler

Two patterns:

1. **Mock at the trait level** — pass a stub `CapabilityHandler` that returns canned responses. Fast, doesn't hit the network. Use this for the bulk of your test suite.
2. **Record + replay** — run once with the real handler and `--record-net-to <tape>`, then in tests run with `--replay-net-from <tape>`. Captures the actual provider response shape but stays offline. Recommended for integration tests and CI.

## Where to look in the code

- `crates/llmvm/src/capability_gateway.rs` — `CapabilityHandler` trait + `MockHandler` (default).
- `crates/llm-effect/` — higher-level prompt, cache, context primitives (provider-agnostic).
- `examples/llm_handlers/` — reference handler implementations.

## What this guide is NOT

- Not a tutorial on prompt engineering. See `crates/llm-effect/`'s prompt module for the prompt-building primitives Boruna ships.
- Not a recommendation of a specific provider. Pick what your organization standardizes on.
- Not a streaming-API guide. Boruna's capability calls are synchronous; streaming responses must be collected to a single value before returning.
