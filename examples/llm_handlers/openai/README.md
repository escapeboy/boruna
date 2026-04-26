# OpenAI LLM Handler — Reference Example

This directory contains a reference implementation of a `CapabilityHandler` that routes `llm.call` capability invocations to the OpenAI Chat Completions API.

**This is a reference only — not a production handler.** It demonstrates the BYOH (Bring Your Own Handler) integration pattern documented in [`docs/guides/llm-integration.md`](../../../docs/guides/llm-integration.md). Production deployments should fork this and harden it for their own provider, secret management, observability, and routing concerns.

## Files

- `handler.rs` — the `OpenAiHandler` struct and its `CapabilityHandler` impl. Self-contained: no Boruna-internal dependencies beyond `boruna-vm` and `boruna-bytecode`.
- `main.rs` — a thin CLI that loads `OPENAI_API_KEY` from env, builds a handler, and runs a workflow with it.

## Building

The handler depends on the `http` feature (via `ureq`):

```bash
cd examples/llm_handlers/openai
OPENAI_API_KEY=sk-... cargo run --release -- ../path/to/workflow_dir
```

## What it doesn't do

Deliberately omitted (these belong in your fork, not in a reference):

- **Multi-provider routing.** Single hard-coded endpoint.
- **Token counting / budget tracking.** `boruna_orchestrator`'s per-step `budget.max_calls` covers call-count budgeting.
- **Retry on rate-limit.** Boruna's `RetryPolicy` (sprint `0.3-S5`) applies at the step level. For finer-grained retry-within-handler, add your own backoff loop.
- **Streaming.** Boruna's capability calls are synchronous; streaming responses must be collected.
- **Cost accounting.** Track usage by intercepting in your handler's `handle` method.
- **Secret rotation.** `from_env` reads once at startup. Long-running daemons should refresh.

## Determinism

This handler is non-deterministic at the LLM layer. For reproducible runs:
- Set `temperature: 0` in the request body.
- Pin the model version (e.g. `gpt-4o-mini-2024-07-18`, not `gpt-4o-mini`).
- Use `boruna workflow run --record-net-to <tape>` to capture the session, then `--replay-net-from <tape>` for replay (sprint `0.5-S7`).

See `docs/guides/llm-integration.md` for the full discussion.
