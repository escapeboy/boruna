# Architecture: Versioned Capability Identity

**Sprint:** `0.3-S11` · **Issue:** [#3](https://github.com/escapeboy/boruna/issues/3) · **Status:** Plan

## Component map

```
┌─────────────────────────────────────────────────────────────┐
│ crates/llmbc/src/capability.rs (new methods)                │
│   ├─ Capability::version() -> &'static str                  │
│   ├─ Capability::ALL: &[Capability; 10] (canonical order)   │
│   └─ capability_set::*                                      │
│      ├─ CapabilityIdentity { name, version }                │
│      ├─ CapabilitySetReport { name, version, caps, hash }   │
│      └─ compute_capability_set_hash() -> String             │
└─────────────────────────────────────────────────────────────┘
            │                                    │
            ▼                                    ▼
┌──────────────────────────────┐   ┌────────────────────────────────┐
│ crates/llmvm-cli (CLI)       │   │ crates/boruna-mcp (MCP server) │
│ `boruna capability list      │   │ `boruna_capability_list` tool  │
│   [--json]`                  │   │  → calls same library helper   │
│  → calls same library helper │   │                                │
└──────────────────────────────┘   └────────────────────────────────┘
```

**Single source of truth:** the report builder lives in `boruna-bytecode`, so CLI + MCP cannot drift.

## Data model

```rust
// crates/llmbc/src/capability.rs

impl Capability {
    /// Canonical iteration order for hashing. Sorted by `name()`.
    pub const ALL: [Capability; 10] = [
        Capability::ActorSend,
        Capability::ActorSpawn,
        Capability::DbQuery,
        Capability::FsRead,
        Capability::FsWrite,
        Capability::LlmCall,
        Capability::NetFetch,
        Capability::Random,
        Capability::TimeNow,
        Capability::UiRender,
    ];

    /// Contract version for this capability.
    /// Bump only when argument shape, return shape, or side-effect semantics change.
    /// All shipped at "1" in 0.2.0; future bumps require an entry in CHANGELOG and a
    /// note in docs/reference/capability-identity.md.
    pub fn version(&self) -> &'static str {
        match self {
            Capability::NetFetch
            | Capability::FsRead
            | Capability::FsWrite
            | Capability::DbQuery
            | Capability::UiRender
            | Capability::TimeNow
            | Capability::Random
            | Capability::LlmCall
            | Capability::ActorSpawn
            | Capability::ActorSend => "1",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilitySetReport {
    pub name: &'static str,        // always "boruna"
    pub version: &'static str,     // CARGO_PKG_VERSION
    pub capabilities: Vec<CapabilityIdentity>,
    pub capability_set_hash: String, // "sha256:<hex>"
}

pub fn capability_set_report(boruna_version: &'static str) -> CapabilitySetReport { ... }
```

## Hash algorithm

Goal: identical hash on identical contract surface, regardless of build host or Rust version.

```text
input  := for each cap in Capability::ALL (already sorted by name):
            format!("{}\t{}\n", cap.name(), cap.version())
          concatenated, UTF-8.
hash   := SHA-256(input)
output := "sha256:" + hex(lower-case)
```

The `\t` separator between name and version, `\n` between entries, lower-case hex digest. **No JSON in the hash input** — JSON serialization is host-dependent (key ordering, whitespace).

## Surface

### CLI

```
boruna capability list             # human-readable table
boruna capability list --json      # CapabilitySetReport JSON
```

`--json` is the canonical machine surface and matches the MCP tool exactly.

### MCP

New tool `boruna_capability_list`, no parameters. Returns `CapabilitySetReport` wrapped with `success: true`. Internal failure (only possible if SHA-256 ever fails, which it cannot) returns `success: false, error_kind: "internal"`.

## Dependencies to add

- `sha2 = "0.10"` → `crates/llmbc/Cargo.toml`. Already in workspace transitively via `llm-effect`.
- `hex = "0.4"` for lower-case hex encoding (or hand-roll the 16-byte loop — small enough to inline).

**Decision:** hand-roll hex. 6 lines of code, removes a dep from a foundational crate.

## Files modified / created

| File | Change |
|---|---|
| `crates/llmbc/Cargo.toml` | Add `sha2 = "0.10"` |
| `crates/llmbc/src/capability.rs` | Add `ALL`, `version()`, `CapabilityIdentity`, `CapabilitySetReport`, `capability_set_report()` |
| `crates/llmbc/src/lib.rs` | Re-export new types |
| `crates/llmvm-cli/src/main.rs` | Add `Command::Capability(CapabilityCommand)` subcommand |
| `crates/boruna-mcp/src/tools/mod.rs` | Add `pub mod capability;` |
| `crates/boruna-mcp/src/tools/capability.rs` | New: `list_capabilities() -> String` |
| `crates/boruna-mcp/src/server.rs` | Register `boruna_capability_list` tool |
| `docs/reference/capability-identity.md` | Contract documentation + caching recipe |
| `CHANGELOG.md` | `[Unreleased]` entry |

## Risks

- **Forgetting to bump per-cap version.** Mitigation: docs explicitly call this out; future capability changes will be reviewed against this contract.
- **Hash format drift.** Mitigation: documented byte-for-byte algorithm; deterministic test asserts hash == known-good value for current 10 capabilities.
- **Adding a new capability silently changes the hash.** This is **correct behavior** — any contract surface change should change the hash. Documented as such.
