# Boruna Patch Demo

Demonstrates the Boruna effect system:

1. App sends `Refactor` message
2. `update()` produces `Effect::LlmCall` with `prompt_id: "demo.refactor"`
3. Framework executes effect via LLM gateway (mock backend)
4. Result (a PatchBundle) delivered back as `LlmResult` message
5. State transitions: idle -> waiting -> done

Mock mode returns a fixed PatchBundle. Deterministic and replay-safe.
