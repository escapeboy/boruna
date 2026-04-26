# Capability Policy Schema

The `policy` parameter on the MCP `boruna_run` tool (and the `--policy <file>` flag on the `boruna` CLI) accepts either:

- A string shorthand: `"allow-all"` or `"deny-all"`
- A **Policy object** matching the schema below

This page documents the object form. The machine-readable schema lives at [`policy.schema.json`](./policy.schema.json).

## Object form

```jsonc
{
  // Policy schema version. Currently always 1. Optional.
  "schema_version": 1,

  // Default behavior for capabilities NOT listed in `rules`.
  // false = deny by default (allowlist mode); true = allow by default (denylist mode).
  // Required for predictable behavior — do not omit.
  "default_allow": false,

  // Per-capability rules. Keys are capability names (see table below).
  "rules": {
    "net.fetch": {
      "allow":  true,   // boolean, required
      "budget": 0       // u64, required. 0 = unlimited; otherwise hard ceiling on call count.
    }
  },

  // Optional network-specific controls. Applied when `net.fetch` is allowed.
  "net_policy": {
    "allowed_domains":      ["api.openai.com", "*.our-api.example"], // empty = all
    "allowed_methods":      ["GET", "POST"],                          // empty = all
    "max_response_bytes":   10485760,                                 // default 10 MB
    "timeout_ms":           30000,                                    // default 30 s
    "allow_redirects":      true                                      // default true
  }
}
```

## Capability names

These are the strings you use as keys in `rules`. They mirror `boruna_bytecode::Capability::name()`.

| Capability | Key | Notes |
|---|---|---|
| Network fetch | `net.fetch` | HTTP GET/POST/etc. — also gated by `net_policy` |
| Filesystem read | `fs.read` | |
| Filesystem write | `fs.write` | |
| Database query | `db.query` | |
| UI render | `ui.render` | Framework `view()` output |
| Current time | `time.now` | Non-deterministic; deny in pure pipelines |
| Random number | `random` | Non-deterministic; deny in pure pipelines |
| LLM call | `llm.call` | External model invocation — apply `budget` to cap cost |
| Spawn actor | `actor.spawn` | |
| Send to actor | `actor.send` | |

**The strict validator rejects aliases.** Sprint `0.4-S15` locked the rule-key surface to canonical names only. A policy file with `"net"` as a rule key fails validation with `error_kind: "policy.invalid_capability"` and a hint to use `"net.fetch"`. Aliases were silently no-ops at gateway-check time before — fixing that footgun was the point of `0.4-S15` (project convention #1: reject at parse, don't silently override).

## Examples

### 1. Allowlist domain only — deny everything except `net.fetch` to `api.openai.com`

```json
{
  "default_allow": false,
  "rules": { "net.fetch": { "allow": true, "budget": 0 } },
  "net_policy": { "allowed_domains": ["api.openai.com"] }
}
```

### 2. Allow-all minus filesystem writes — useful for read-only workflows

```json
{
  "default_allow": true,
  "rules": { "fs.write": { "allow": false, "budget": 0 } }
}
```

### 3. LLM call quota — cap LLM invocations at 5 per run

```json
{
  "default_allow": true,
  "rules": { "llm.call": { "allow": true, "budget": 5 } }
}
```

When the budget is exceeded the run aborts with a `runtime_error` whose message references `CapabilityBudgetExceeded(LlmCall)`.

## Surprising behavior to know

- **`default_allow` defaults to `false`.** A `Policy {}` (empty object) denies everything. Always set `default_allow` explicitly.
- **`budget: 0` means unlimited**, not "zero allowed." Use `{ "allow": false, "budget": 0 }` to deny.
- **String shorthand and object form are not mixable.** Pass exactly one shape.
- **Unknown JSON shapes are rejected.** Old MCP clients that accidentally posted typo'd strings (e.g. `"alow-all"`) used to be silently treated as `allow-all`. They now return `success: false, error_kind: "invalid_policy"`. This is intentional — silent fall-through to allow-all was the bug FleetQ reported.
- **Unknown fields are rejected** (sprint `0.4-S15`). A typo like `"default_alow": true` no longer parses as `default_allow: false` (silent default); it fails with `error_kind: "policy.unknown_field"`.
- **Unsupported `schema_version` values are rejected.** This binary supports `schema_version: 1`. Setting `2` or any other value fails with `error_kind: "policy.unknown_schema_version"`.

## CLI tooling (sprint 0.4-S15)

```sh
# Strict-validate a policy file. Designed as a CI gate.
boruna policy validate policies/prod.json
# → exit 0 + "OK: ..." on success
# → exit 2 + stderr "error: policy.<kind>: ..." on validation error
# → exit 1 + stderr "error: policy.io_error: ..." on file IO error

# Machine-parseable output:
boruna policy validate --json policies/prod.json
# → {"ok":true} or {"ok":false,"errors":[{"error_kind":"policy.unknown_field",...}]}

# Print the effective policy (denormalized).
boruna policy show policies/prod.json
```

The MCP server exposes the same validator as `boruna_policy_validate`.

## Stable error_kind taxonomy

The strict validator emits these stable strings (project convention #2 — locked forever):

| `error_kind` | When |
|---|---|
| `policy.io_error` | File missing or unreadable |
| `policy.parse_error` | JSON syntax error or value type mismatch |
| `policy.unknown_schema_version` | `schema_version` is set to an unsupported value |
| `policy.unknown_field` | Unknown field at any level (top-level, `net_policy`, or inside a rule) |
| `policy.invalid_capability` | A rule key is not a recognized canonical capability name |
| `policy.invalid_net_policy` | `net_policy` value out of range or unknown method |

The `boruna_run` MCP tool **also** emits the legacy `error_kind: "invalid_policy"` for non-object input (string typos, arrays, numbers). The new `policy.*` kinds apply to object-form payloads only — they are additive over `invalid_policy`, not a replacement.

## Versioning

The schema carries `schema_version: 1`. Future breaking changes will bump this number; the MCP tool will continue to accept the old shape as long as `schema_version` matches a supported value. This field is what lets you cache `(script_hash, policy_hash)` results safely across binary upgrades. Sprint `0.4-S15` locked this contract: only `1` is currently accepted; new optional fields can be added at v1; shape changes require a version bump.

## Hashing for caching

Because `Policy` is `Serialize + Deserialize`, you can hash a normalized policy for cache keys:

```rust
let bytes = serde_json::to_vec(&policy).unwrap();
let hash  = sha2::Sha256::digest(&bytes);
```

Pair `hash(policy)` with `hash(source)` to memoize deterministic runs. (The capability-set identity portion — making the hash stable across binary upgrades — is tracked separately in the project roadmap.)
