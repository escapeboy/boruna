# Design: Fine-Grained Capability Policy in `boruna_run`

**Sprint A** · **Status:** in progress · **Author:** Claude (from FleetQ feedback 2026-04-25)

## Problem

The MCP `boruna_run` tool accepts `policy: Option<String>` and string-matches `"allow-all"` / `"deny-all"`. Anything else — including the rich, already-implemented per-capability rules in `boruna-vm::capability_gateway::Policy` — is silently ignored. Integrators must therefore choose "trust everything or nothing," which contradicts Boruna's headline pitch of *"explicit capability gates: `net.fetch`, `fs.read`, …"*.

This is purely a **surface gap**. The `Policy` struct, `PolicyRule`, `NetPolicy` (allowed domains/methods/byte limits/timeout), per-capability budgets, and the CLI's JSON-file path mode are all already implemented and tested.

## Forcing questions

### Who needs this? What are they doing today?

Production integrators wiring Boruna into multi-tenant agent platforms. Confirmed customer: **FleetQ** (4 product surfaces: skill type, workflow node, MCP tools over stdio + HTTP/SSE, Livewire UI). Today they render a single boolean toggle in their UI ("trust everything or nothing") because the MCP surface forces it. That toggle is no finer than running unsandboxed code with a generic try/catch — it makes the capability system invisible to operators.

### Narrowest MVP someone would pay for

`boruna_run` accepts a structured `policy` object (alongside the legacy string shorthand) that round-trips through `serde_json` into the existing `Policy` struct. Plus a documented JSON Schema for the policy shape so FleetQ's UI can render a per-capability matrix without guessing field names. That's it. ~50 lines of code, zero new VM features, full backwards compat.

### What would make someone say "whoa"

The structured policy is the **same struct** the VM already uses for replay verification and evidence bundles. The exact policy chosen by an operator becomes part of the audit chain. FleetQ can show end users not just "this script ran" but "this script ran under this exact policy, here's the hash, here's the evidence bundle to prove no other capability was reachable."

### How does this compound over time

- Unblocks P1 #3 (versioned capability hash): once policies are first-class JSON, hashing `(script, input, policy, capability_set)` for caching is trivial.
- Unblocks P1 #5 (resource limits): same `RunParams` mechanism — add a `limits` field next to `policy`.
- Unblocks P1 #6 (validate response stability): once we lock down policy schema we can document the validate response schema with the same versioning approach.
- Becomes a **selling point**: "the only deterministic LLM runtime where each tool invocation publishes its capability bound."

## Non-goals (scope discipline)

- ❌ Not implementing P1 #3 (capability_set_hash) — separate issue.
- ❌ Not implementing P1 #5 (memory/syscall limits) — wall-clock timeout already exists; rest is OS-level work.
- ❌ Not implementing P2 #7 (record/replay for net.fetch) — separate sprint.
- ❌ No new CLI subcommands. CLI already accepts `--policy <json-file>`.
- ❌ No `schemars` dependency added to `boruna-vm` — JSON schema written by hand to keep crate dependency surface minimal.
- ❌ No breaking change to `"allow-all"` / `"deny-all"` shorthand strings.

## Acceptance criteria

1. `boruna_run` accepts `policy` as either:
   - A string `"allow-all"` or `"deny-all"` (legacy, default `"allow-all"`).
   - A JSON object that deserializes into `boruna_vm::capability_gateway::Policy`.
   - Anything else returns a structured error in the tool response (`success: false, error_kind: "invalid_policy"`).
2. Per-capability denial works end-to-end through the MCP tool (test asserts a script that calls `NetFetch` is rejected when `net.fetch` is set to `{ "allow": false }`).
3. Per-capability budget works end-to-end (test asserts the third call to `LlmCall` errors when `budget: 2`).
4. `NetPolicy.allowed_domains` is enforced when paired with the `http` feature (test gated behind `--features http`).
5. `docs/reference/policy-schema.md` documents the schema with copy-paste-runnable JSON examples for FleetQ.
6. `cargo test --workspace` passes; `cargo clippy --workspace -- -D warnings` clean.
7. No change to the existing `Policy` struct shape (so existing `--policy file.json` users are unaffected).

## Risks

- **Risk:** Existing MCP clients passing `policy: "allow-all"` still work? **Mitigation:** Use `serde_json::Value` for the field; check `as_str()` first, fall through to `from_value::<Policy>()`. Round-trip test for both forms.
- **Risk:** `Policy` derives `Default` with `default_allow: false` — a user posting `{"rules": {...}}` without `default_allow` gets deny-all behavior for unlisted caps, which may surprise. **Mitigation:** Document this explicitly in the schema doc as "default deny by default — set `default_allow: true` to allowlist via deny-list pattern."
