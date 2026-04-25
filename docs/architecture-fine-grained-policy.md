# Architecture: Fine-Grained Capability Policy in `boruna_run`

**Sprint A** · See `docs/design-fine-grained-policy.md` for problem framing.

## Surface area

```
crates/boruna-mcp/src/server.rs        ← RunParams.policy: serde_json::Value
crates/boruna-mcp/src/tools/run.rs     ← run_source signature + parse helper
docs/reference/policy-schema.md        ← new doc
docs/reference/policy.schema.json      ← new artifact (machine-readable)
crates/boruna-mcp/tests/run_policy.rs  ← new integration test (or extend existing)
```

No changes to: `boruna-vm`, `boruna-cli`, `boruna-orchestrator`, any other crate.

## Data flow

```
MCP client (FleetQ)
  │
  │  { "policy": <string | object>, "source": "...", ... }
  ▼
boruna-mcp::server::boruna_run
  │  RunParams { policy: Option<serde_json::Value>, ... }
  ▼
boruna-mcp::tools::run::run_source(source, policy, max_steps, trace) -> String
  │  parse_policy(policy: Option<&Value>) -> Result<Policy, PolicyError>
  │     │  None                    → Policy::allow_all()  (legacy default)
  │     │  Some(String "allow-all") → Policy::allow_all()
  │     │  Some(String "deny-all")  → Policy::deny_all()
  │     │  Some(Object)             → serde_json::from_value::<Policy>(v)
  │     │  Some(other)              → Err — return success=false, error_kind=invalid_policy
  ▼
CapabilityGateway::new(policy)
  ▼
Vm::run() — existing capability + budget enforcement
```

## Component design

### 1. `RunParams` change

```rust
// crates/boruna-mcp/src/server.rs
#[derive(Serialize, Deserialize, JsonSchema)]
struct RunParams {
    /// The .ax source code to run
    source: String,
    /// Capability policy. Either:
    ///   - "allow-all" / "deny-all" (string shorthand, default "allow-all")
    ///   - A Policy object — see docs/reference/policy-schema.md for the schema
    #[serde(default)]
    policy: Option<serde_json::Value>,
    /// Maximum execution steps (default: 10000000)
    max_steps: Option<u64>,
    /// Enable opcode-level execution trace (default: false)
    trace: Option<bool>,
}
```

`serde_json::Value` keeps schemars happy (it generates `{}` permissive schema). The MCP tool description carries the human-readable contract; the dedicated docs file carries the formal schema.

### 2. `run_source` signature change

```rust
// crates/boruna-mcp/src/tools/run.rs
pub fn run_source(
    source: &str,
    policy: Option<&serde_json::Value>,
    max_steps: u64,
    trace: bool,
) -> String { ... }

fn parse_policy(value: Option<&serde_json::Value>) -> Result<Policy, String> {
    match value {
        None => Ok(Policy::allow_all()),
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "allow-all" => Ok(Policy::allow_all()),
            "deny-all"  => Ok(Policy::deny_all()),
            other => Err(format!(
                "policy string must be 'allow-all' or 'deny-all' (got '{other}'); pass an object for fine-grained policy"
            )),
        },
        Some(obj @ serde_json::Value::Object(_)) => {
            serde_json::from_value::<Policy>(obj.clone())
                .map_err(|e| format!("policy object failed to parse: {e}"))
        }
        Some(other) => Err(format!(
            "policy must be a string ('allow-all'/'deny-all') or an object; got {}", 
            type_name(other)
        )),
    }
}
```

On `Err`, `run_source` returns:

```json
{ "success": false, "error_kind": "invalid_policy", "message": "<reason>" }
```

This matches the existing `error_kind: "runtime_error"` pattern in `run.rs` so MCP consumers handle it identically.

### 3. JSON Schema artifact

`docs/reference/policy.schema.json` — hand-written Draft 2020-12 schema mirroring the `Policy` struct. Pinned `"$id"` so FleetQ can reference it from their UI generator.

`docs/reference/policy-schema.md` — prose explainer with three copy-paste examples:
1. **Allowlist domain only**: `{ "default_allow": false, "rules": { "net.fetch": { "allow": true, "budget": 0 } }, "net_policy": { "allowed_domains": ["api.openai.com"] } }`
2. **Allow-all minus filesystem writes**: `{ "default_allow": true, "rules": { "fs.write": { "allow": false, "budget": 0 } } }`
3. **LLM call quota**: `{ "default_allow": true, "rules": { "llm.call": { "allow": true, "budget": 5 } } }`

## Capability-name reference (for FleetQ UI)

These are the strings `Capability::name()` returns — they're what `Policy.rules` keys against:

| Capability | name() string |
|---|---|
| `TimeNow` | `time.now` |
| `Random` | `random` |
| `NetFetch` | `net.fetch` |
| `FsRead` | `fs.read` |
| `FsWrite` | `fs.write` |
| `DbQuery` | `db.query` |
| `UiRender` | `ui.render` |
| `LlmCall` | `llm.call` |
| `ActorSpawn` | `actor.spawn` |
| `ActorSend` | `actor.send` |

(Verify these names from `boruna_bytecode::Capability::name()` during Build.)

## Backwards compatibility

| Existing client behavior | New behavior | Compat? |
|---|---|---|
| `boruna_run({source})` (no policy) | Defaults to `allow-all` | ✅ Identical |
| `boruna_run({source, policy: "allow-all"})` | Same | ✅ Identical |
| `boruna_run({source, policy: "deny-all"})` | Same | ✅ Identical |
| `boruna_run({source, policy: "garbage"})` | Was: silently treated as `allow-all`. Now: returns `invalid_policy` error. | ⚠️ **Breaking** for clients passing typo'd strings |
| `boruna_run({source, policy: {...}})` | Was: silently treated as `allow-all`. Now: parsed as Policy. | ⚠️ Behavior change for clients accidentally passing objects |

**Decision on the two ⚠️ cases:** Both should be breaking. Silently ignoring an unrecognized policy was the bug; FleetQ explicitly called this out as the "single biggest gap." Documented in CHANGELOG.

## Build sequence

1. Verify `Capability::name()` mapping (read `crates/llmbc/src/lib.rs` or wherever `Capability` lives).
2. Edit `RunParams` in `server.rs`.
3. Edit `run_source` + add `parse_policy` in `tools/run.rs`.
4. Add 3 unit tests in `tools/run.rs` (string allow-all, structured deny-list, invalid input).
5. Add 1 integration-style test exercising `boruna_run` end-to-end with a structured policy that denies `LlmCall`.
6. Write `docs/reference/policy.schema.json` and `docs/reference/policy-schema.md`.
7. Update `CHANGELOG.md` under `Unreleased > Changed` and `Unreleased > Added`.
8. Run `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all -- --check`.
