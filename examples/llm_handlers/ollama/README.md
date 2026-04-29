# Ollama LLM Handler — Reference Example

Reference implementation of a `CapabilityHandler` that routes `llm.call` to a local [Ollama](https://ollama.com/) daemon. Handy for offline development, air-gapped deployments, and CI/test runs against small local models.

**This is a reference only — not a production handler.** See [`docs/guides/llm-integration.md`](../../../docs/guides/llm-integration.md) for the BYOH integration model.

## Why use Ollama with Boruna?

- **Reproducible local dev.** No external API key needed; spin up `ollama serve` and iterate against a 3B-parameter model.
- **Deterministic-ish.** Ollama supports a `seed` parameter; combined with `temperature: 0` you get bit-for-bit reproducibility within a single model version.
- **Air-gapped CI.** Burn the model into the CI image once, then no network egress is required for LLM-using workflows.

## Setup

```bash
# Install Ollama (macOS)
brew install ollama
ollama serve &

# Pull the model the handler defaults to
ollama pull llama3.2:3b
```

Then point Boruna at the daemon (default `http://localhost:11434` matches Ollama's own default — no env config needed for local use).

## Files

- `handler.rs` — the `OllamaHandler` struct and its `CapabilityHandler` impl.

## Configuration

| Variable | Default | Notes |
|---|---|---|
| `OLLAMA_HOST` | `http://localhost:11434` | Override to point at a remote/network-mounted daemon |
| (model) | `llama3.2:3b` | Override via `with_model("...")` — must already be pulled |

## What it doesn't do

- **Multi-provider routing.** Single endpoint. See `examples/llm_handlers/router_setup.rs` for the multi-provider dispatch pattern.
- **Streaming.** Boruna's capability calls are synchronous; this handler sets `stream: false` and collects the single JSON response.
- **Multi-turn chat.** Uses `/api/generate` (single prompt). Switch to `/api/chat` and pass `messages: [...]` for conversation context.
- **Model auto-pull.** If the model isn't already loaded, Ollama returns a 404 — surface this in your fork as a clearer error.
- **GPU detection / load balancing.** Ollama handles those internally per-instance.

## Determinism

Ollama is the easiest LLM to make deterministic:
- `temperature: 0` (set by default in this example).
- `seed: 42` (set by default — change but pin for your replay tape).
- Pin the model tag to a content-addressed digest if you really care: `ollama pull llama3.2:3b@sha256:...` and reference the digest in `with_model`.

Combined with `boruna workflow run --record-net-to <tape>`, you get a fully replayable session.

See [`docs/guides/llm-integration.md`](../../../docs/guides/llm-integration.md) for the full discussion.
