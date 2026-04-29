# std-llm

> Typed LLM effect wrappers for prompting and structured generation

**Package:** `std.llm`  **Version:** `0.1.0`  **Capabilities required:** `llm.call`

## Overview

`std-llm` wraps LLM calls as typed `Effect` values. Your `update` handler returns these effects; the Boruna runtime dispatches them through the `llm.call` capability and delivers responses back via the named `callback_tag`. This keeps model I/O out of pure logic and makes every prompt auditable in the evidence bundle. Temperature is stored as `Int * 100` (e.g. `72` = `0.72`) to avoid floating-point precision issues.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.llm": "0.1.0"
```

Your workflow or app policy must grant `llm.call` to the step that uses this library.

## API Reference

### Types

#### `Effect`

```
type Effect { kind: String, payload: String, callback_tag: String }
```

Returned by all request-building functions. Pass it back from `update` to trigger the LLM call. `kind` is either `"llm_call"` or `"llm_json_call"`.

#### `LlmRequest`

```
type LlmRequest { system_prompt: String, user_prompt: String, max_tokens: Int, temperature: Int }
```

A fully described request for use with `llm_call` or `llm_json_call`. `temperature` is `Int * 100` (e.g. `72` = `0.72`).

#### `LlmResponse`

```
type LlmResponse { content: String, tokens_used: Int, finish_reason: String }
```

Shape of the response delivered to the `callback_tag` handler.

### Functions

#### `llm_prompt(system: String, user: String, callback_tag: String) -> Effect`

Convenience function: builds a default `LlmRequest` (1024 max tokens, temperature 0.72) and emits an `"llm_call"` effect.

**Example**

```
fn main() -> Int {
  let eff: Effect = llm_prompt(
    "You are a helpful assistant.",
    "Summarize this document.",
    "summary_done"
  )
  0
}
```

#### `llm_call(req: LlmRequest, callback_tag: String) -> Effect`

Emits an `"llm_call"` effect from a fully specified `LlmRequest` — use when you need to control `max_tokens` or `temperature` explicitly.

#### `llm_json_call(req: LlmRequest, callback_tag: String) -> Effect`

Emits an `"llm_json_call"` effect, signalling to the runtime that the model should be prompted for structured JSON output.

#### `default_llm_request(system: String, user: String) -> LlmRequest`

Constructs an `LlmRequest` with default values: `max_tokens = 1024`, `temperature = 72`. Use when you want to inspect or mutate the request before passing it to `llm_call`.

## Capabilities

Requires `llm.call`. The VM's `CapabilityGateway` enforces this at runtime; the call is rejected if the active policy does not include `llm.call`.

## Notes / Limitations

- All functions produce `Effect` values — they do not perform any I/O themselves. Actual model calls happen in the runtime after the `update` function returns.
- `temperature` is encoded as `Int * 100`; `72` means `0.72`. There is no float conversion in-language.
- The `payload` field of the produced `Effect` is `system_prompt ++ "|" ++ user_prompt`; the runtime splits on `|` to reconstruct the two prompts.

## Version History

| Version | Change |
|---------|--------|
| `0.1.0` | Initial release. `LlmRequest`, `LlmResponse`, `Effect` types; `llm_call`, `llm_prompt`, `llm_json_call`, `default_llm_request` functions. |
