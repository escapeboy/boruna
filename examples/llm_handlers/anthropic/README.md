# Anthropic LLM Handler — Reference Example

This directory contains a reference implementation of a `CapabilityHandler` that routes `llm.call` capability invocations to the Anthropic Messages API.

**This is a reference only — not a production handler.** It demonstrates the BYOH (Bring Your Own Handler) integration pattern documented in [`docs/guides/llm-integration.md`](../../../docs/guides/llm-integration.md). Production deployments should fork this and harden it for their own provider, secret management, observability, and routing concerns.

## Files

- `handler.rs` — the `AnthropicHandler` struct and its `CapabilityHandler` impl. Self-contained: no Boruna-internal dependencies beyond `boruna-vm` and `boruna-bytecode`.

## How it differs from the OpenAI handler

The shape is intentionally near-identical so integrators can copy-and-tweak. The Anthropic-specific points:

| Concern | OpenAI | Anthropic |
|---|---|---|
| Endpoint | `/v1/chat/completions` | `/v1/messages` |
| Auth header | `Authorization: Bearer <key>` | `x-api-key: <key>` |
| Version pin | (URL only) | `anthropic-version: 2023-06-01` (required header) |
| `max_tokens` | optional | **required** — no "unlimited" |
| Response path | `choices[0].message.content` | `content[0].text` |
| Multi-turn shape | `{role, content}` flat strings | `{role, content}` with content as list-of-blocks |

## Building

The handler depends on `ureq` (HTTP client). Copy the file into your integrator crate, add `ureq = "2"` and `serde_json = "1"`, then wire the handler into your runner via `boruna_orchestrator::WorkflowRunner::with_handler` (or the equivalent in your harness).

## What it doesn't do

Deliberately omitted (these belong in your fork, not in a reference):

- **Multi-provider routing.** Single hard-coded endpoint. See `examples/llm_handlers/router_setup.rs` and `boruna_vm::capability_gateway::LlmRouterHandler` for the multi-provider dispatch pattern.
- **Token counting / budget tracking.** `boruna_orchestrator`'s per-step `budget.max_calls` covers call-count budgeting.
- **Retry on rate-limit / `429`.** Boruna's `RetryPolicy` (sprint `0.3-S5`) applies at the step level. For finer-grained retry-within-handler, add your own backoff loop reading the `retry-after` header Anthropic sends on overload.
- **Streaming.** Boruna's capability calls are synchronous; streaming responses must be collected.
- **Tool use.** This handler only extracts the first `content[i].text`. If you use Anthropic's tool-use protocol, branch on `content[i].type` (`text` vs `tool_use`).
- **Cost accounting.** Track usage by intercepting in your handler's `handle` method (Anthropic returns `usage.input_tokens` + `usage.output_tokens`).

## Determinism

This handler is non-deterministic at the LLM layer. For reproducible runs:
- Set `temperature: 0` in the request body (the default in this example).
- Pin the model version to a dated snapshot, e.g. `claude-sonnet-4-6` is the family alias — for replay reproducibility prefer the dated variant when Anthropic publishes one.
- Use `boruna workflow run --record-net-to <tape>` to capture the session, then `--replay-net-from <tape>` for replay (sprint `0.5-S7`).

See [`docs/guides/llm-integration.md`](../../../docs/guides/llm-integration.md) for the full discussion.
