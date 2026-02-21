# feat: Framework Effect Execution, Replay Stability & Determinism Invariants

## Overview

Three interconnected improvements to the Boruna framework foundation:

1. **Effect Execution in Framework Runtime** — Effects are currently parsed and validated but never executed. Implement a host-side `EffectExecutor` that dispatches all 8 built-in effect kinds via the capability gateway and delivers results as callback messages to `update()`.
2. **Replay/Trace Log Stability** — The `EventLog` (VM-level) has no format version. Add versioning, fix the two inconsistent hashing approaches (SHA-256 in trace2tests vs non-crypto fold in CLI), and add golden replay tests that lock the serialization format.
3. **Determinism Invariant Tests** — Add 5+ new tests specifically protecting determinism, purity, effect ordering, and failure behavior.

## Problem Statement / Motivation

Per `docs/FRAMEWORK_STATUS.md`, the Effect System is at **85%** — parsed + validated but not host-executed (finding F7 in `docs/DOGFOOD_FINDINGS.md`). The `EventLog` struct has no version field, meaning any future changes to the `Event` enum will silently break all existing logs. The CLI `trace-hash` command uses a non-cryptographic fold hash while `trace2tests` uses SHA-256, creating inconsistency. These are the most critical gaps before the framework can be considered stable.

---

## Phase 1: Host Effect Execution

### Problem

`AppRuntime::send()` (`crates/llmfw/src/runtime.rs:128-179`) returns effects as data to the caller but never executes them. Tests currently simulate effect results by manually sending callback messages. There is no `EffectExecutor` module.

### Proposed Solution

Add an `EffectExecutor` trait and a `HostEffectExecutor` implementation inside `boruna-framework`. The executor:

1. Takes a `Vec<Effect>` from `AppRuntime::send()`
2. Dispatches each effect to the capability gateway based on `EffectKind`
3. Collects results as `AppMessage` with `callback_tag` as the message tag
4. Returns `Vec<AppMessage>` for the caller to feed back into `update()`

### Technical Approach

#### New file: `crates/llmfw/src/executor.rs`

```rust
// crates/llmfw/src/executor.rs

use crate::effect::{Effect, EffectKind};
use crate::runtime::AppMessage;
use crate::error::FrameworkError;
use boruna_bytecode::Value;

/// Trait for executing effects and producing callback messages.
pub trait EffectExecutor {
    fn execute(&mut self, effects: Vec<Effect>) -> Result<Vec<AppMessage>, FrameworkError>;
}

/// Mock executor for testing — returns deterministic stub results.
pub struct MockEffectExecutor {
    responses: BTreeMap<String, Value>,  // callback_tag -> response value
}

/// Host executor — dispatches to capability gateway.
pub struct HostEffectExecutor {
    gateway: CapabilityGateway,
}
```

#### Effect dispatch mapping (all 8 kinds):

| EffectKind     | Capability    | Gateway args            | Result → AppMessage                     |
|----------------|---------------|-------------------------|------------------------------------------|
| `HttpRequest`  | `net.fetch`   | `[payload_string]`      | `AppMessage::new(callback_tag, result)`  |
| `DbQuery`      | `db.query`    | `[payload_string]`      | `AppMessage::new(callback_tag, result)`  |
| `FsRead`       | `fs.read`     | `[payload_string]`      | `AppMessage::new(callback_tag, result)`  |
| `FsWrite`      | `fs.write`    | `[payload_string]`      | `AppMessage::new(callback_tag, result)`  |
| `Timer`        | `time.now`    | `[]`                    | `AppMessage::new(callback_tag, result)`  |
| `Random`       | `random`      | `[]`                    | `AppMessage::new(callback_tag, result)`  |
| `SpawnActor`   | `spawn`       | `[payload_string]`      | `AppMessage::new(callback_tag, result)`  |
| `EmitUi`       | `ui.render`   | `[payload_value]`       | No callback (fire-and-forget)            |

#### Changes to existing files

1. **`crates/llmfw/src/lib.rs`** — Add `pub mod executor;` re-export
2. **`crates/llmfw/src/runtime.rs`** — Add optional `send_with_executor()` method on `AppRuntime` that:
   - Calls `send()` to get `(state, effects, ui)`
   - Passes effects to executor
   - Returns `(state, callback_messages, ui)`
   - Does NOT break existing `send()` API
3. **`crates/llmfw/src/testing.rs`** — Add `TestHarness::send_with_effects()` using `MockEffectExecutor`

#### Key design decisions

- `EffectExecutor` is a trait — allows mock, host, and replay implementations
- `MockEffectExecutor` uses `BTreeMap` (not HashMap) for deterministic iteration
- Effect execution order is sequential (same order as returned by `update()`)
- `EmitUi` effects do not produce callback messages
- Failed effects produce error messages: `AppMessage::new(callback_tag, Value::Err(error_string))`
- All gateway calls go through `EventLog` for replay compatibility

### Files to create/modify

| File | Action | Description |
|------|--------|-------------|
| `crates/llmfw/src/executor.rs` | **Create** | `EffectExecutor` trait, `MockEffectExecutor`, `HostEffectExecutor` |
| `crates/llmfw/src/lib.rs` | Modify | Add `pub mod executor;` |
| `crates/llmfw/src/runtime.rs` | Modify | Add `send_with_executor()` method |
| `crates/llmfw/src/testing.rs` | Modify | Add `send_with_effects()` method on `TestHarness` |
| `crates/llmfw/src/tests.rs` | Modify | Add executor tests (see Phase 1 tests below) |

### Tests for Phase 1

```rust
// In crates/llmfw/src/tests.rs

// 1. MockEffectExecutor returns correct callback messages
#[test] fn test_mock_executor_delivers_callbacks()

// 2. HostEffectExecutor dispatches to gateway for each EffectKind
#[test] fn test_host_executor_http_request()
#[test] fn test_host_executor_db_query()
#[test] fn test_host_executor_fs_read()
#[test] fn test_host_executor_fs_write()
#[test] fn test_host_executor_timer()
#[test] fn test_host_executor_random()

// 3. EmitUi produces no callback
#[test] fn test_executor_emit_ui_no_callback()

// 4. Effect execution order matches update() return order
#[test] fn test_executor_preserves_effect_order()

// 5. Failed effects produce error messages
#[test] fn test_executor_failed_effect_delivers_error()

// 6. Full cycle: update() → execute effects → callback → update()
#[test] fn test_full_effect_round_trip()

// 7. send_with_executor() integration
#[test] fn test_runtime_send_with_executor()
```

### Risk assessment

- **Low risk**: `EffectExecutor` is additive — existing `send()` is untouched
- **Medium risk**: `HostEffectExecutor` needs access to a `CapabilityGateway` — may need to expose gateway from `AppRuntime` or inject at construction. Current `AppRuntime` creates its own gateway internally (`runtime.rs:65-70`). Solution: add `AppRuntime::new_with_gateway()` constructor while keeping `AppRuntime::new()` as-is.

---

## Phase 2: Replay/Trace Log Stability

### Problem

1. **`EventLog`** (`crates/llmvm/src/replay.rs:39-41`) has no version field. The `Event` enum has 7 variants — adding/changing any will break deserialization of existing logs.
2. **Two inconsistent trace hash approaches**:
   - `tooling/src/trace2tests/mod.rs:56-60` uses SHA-256 (cryptographic, stable)
   - `crates/llmvm-cli/src/main.rs:882-914` uses a byte-fold (non-crypto, collision-prone)
3. **`TraceFile`** (`tooling/src/trace2tests/mod.rs:10-24`) has `version: u32 = 1` but no migration logic.

### Proposed Solution

#### 2A: Version `EventLog`

Add a versioned wrapper:

```rust
// crates/llmvm/src/replay.rs

pub const EVENT_LOG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLog {
    pub version: u32,           // NEW field — defaults to 1
    events: Vec<Event>,
}
```

- `to_json()` always writes `version`
- `from_json()` checks version — returns error if unsupported
- Backward compatibility: if JSON lacks `version` field, assume version 1

#### 2B: Unify trace hashing

Replace the CLI fold-hash with SHA-256 from `trace2tests`:

```rust
// crates/llmvm-cli/src/main.rs — trace-hash command
// Replace fold-based hash (lines 908-909) with:
use boruna_tooling::trace2tests::sha256_hex;
let hash = sha256_hex(&trace);
println!("{}", hash);
```

This is a **breaking change** for anyone relying on the old fold-hash output. Justified because:
- The old hash was collision-prone and non-standard
- SHA-256 matches what `trace2tests` already uses
- No external consumers documented

#### 2C: Golden replay format tests

Add tests that lock the serialized format of `EventLog`:

```rust
// crates/llmvm/src/tests.rs

// Lock the JSON format of EventLog v1
#[test] fn test_event_log_v1_format_stability()
// Serialize known events → compare against hardcoded JSON string

// Verify version field is present
#[test] fn test_event_log_includes_version()

// Verify backward compat: JSON without version deserializes as v1
#[test] fn test_event_log_missing_version_defaults_to_v1()

// Verify unknown version is rejected
#[test] fn test_event_log_unknown_version_rejected()
```

### Files to modify

| File | Action | Description |
|------|--------|-------------|
| `crates/llmvm/src/replay.rs` | Modify | Add `version` field to `EventLog`, add `EVENT_LOG_VERSION` const |
| `crates/llmvm/src/tests.rs` | Modify | Add format stability tests |
| `crates/llmvm-cli/src/main.rs` | Modify | Replace fold-hash with SHA-256 in `trace-hash` command |
| `crates/llmvm-cli/Cargo.toml` | Modify | Add `boruna-tooling` dependency (if not already present) |
| `tooling/src/trace2tests/mod.rs` | Modify | Make `sha256_hex` public if not already |

### Tests for Phase 2

```rust
// 1. EventLog v1 JSON format is stable (golden test)
#[test] fn test_event_log_v1_format_stability()

// 2. Version field roundtrips through JSON
#[test] fn test_event_log_version_roundtrip()

// 3. Missing version → defaults to 1 (backward compat)
#[test] fn test_event_log_missing_version_defaults_v1()

// 4. Unknown version → error
#[test] fn test_event_log_rejects_unknown_version()

// 5. CLI trace-hash produces SHA-256 (64 hex chars)
// (manual verification or integration test)

// 6. Trace hash stability: same input → same hash across runs
#[test] fn test_trace_hash_deterministic_across_runs()
```

### Risk assessment

- **Low risk**: Adding `version` field with serde default is backward-compatible
- **Medium risk**: Changing CLI `trace-hash` output format. Mitigated by: no external consumers, and old format was known to be weak
- **Note**: `Cargo.toml` dependency addition for CLI crate needs verification — check if `boruna-tooling` is already a dependency

---

## Phase 3: Determinism Invariant Tests

### Problem

The framework has golden determinism tests, but no focused tests that specifically target:
- Determinism under effect execution
- Purity enforcement with the executor in the loop
- Effect ordering guarantees across replays
- Failure behavior determinism (what happens when effects fail)

### Proposed Solution

Add **7 new tests** (exceeding the minimum of 5) in `crates/llmfw/src/tests.rs`:

```rust
// === Determinism Invariants ===

// INV-1: Effect execution order is deterministic
// Run same app twice with MockEffectExecutor, verify callback message order is identical
#[test] fn test_invariant_effect_execution_order_deterministic()

// INV-2: Purity — update() cannot observe system state even through executor
// Verify that two runs with different mock responses for same callback_tag
// produce different states (effects are the ONLY channel for external data)
#[test] fn test_invariant_purity_only_effects_channel_data()

// INV-3: Effect list from update() is deterministic for same (state, msg)
// Run update() with identical state+msg 100 times, verify identical effects
#[test] fn test_invariant_update_effect_list_deterministic()

// INV-4: Failed effect produces deterministic error message
// Run with an effect that fails → verify error callback is identical across runs
#[test] fn test_invariant_failed_effect_deterministic_error()

// INV-5: Cycle log fingerprint is stable across runs with effects
// Run full app lifecycle with executor, compute cycle_fingerprint, compare
#[test] fn test_invariant_cycle_fingerprint_stable_with_effects()

// INV-6: State snapshot JSON is bitwise identical after identical message sequences
// (extends existing snapshot stability test to include effect callbacks)
#[test] fn test_invariant_snapshot_bitwise_identical_with_callbacks()

// INV-7: Replay of effect-producing app matches original execution
// Record run with effects → replay → verify states match
#[test] fn test_invariant_replay_with_effects_matches_original()
```

### Files to modify

| File | Action | Description |
|------|--------|-------------|
| `crates/llmfw/src/tests.rs` | Modify | Add 7 invariant tests |

### Risk assessment

- **No risk**: Pure test additions, no production code changes

---

## Acceptance Criteria

### Functional Requirements

- [ ] `EffectExecutor` trait defined with `execute(Vec<Effect>) -> Result<Vec<AppMessage>>`
- [ ] `MockEffectExecutor` works with `BTreeMap<String, Value>` for deterministic stub responses
- [ ] `HostEffectExecutor` dispatches all 8 effect kinds to capability gateway
- [ ] `EmitUi` effects do not produce callback messages
- [ ] Failed effects produce `Value::Err(String)` callback messages
- [ ] Effects execute in the order returned by `update()`
- [ ] `AppRuntime::send_with_executor()` exists without breaking `send()`
- [ ] `TestHarness::send_with_effects()` exists for convenient testing
- [ ] `EventLog` has `version: u32` field (default 1)
- [ ] `EventLog::from_json()` rejects unknown versions
- [ ] `EventLog::from_json()` handles missing version as v1
- [ ] CLI `trace-hash` uses SHA-256
- [ ] All 7 invariant tests pass

### Non-Functional Requirements

- [ ] No new sources of nondeterminism (all collections use `BTreeMap`)
- [ ] All existing 440+ tests still pass
- [ ] No breaking changes to public APIs (`send()`, `TestHarness::from_source()`, etc.)

### Quality Gates

- [ ] `cargo test --workspace` passes (all crates)
- [ ] `cargo clippy --workspace` has no new warnings
- [ ] No `HashMap` usage in new code (determinism invariant)

---

## Dependencies & Prerequisites

- No external dependencies needed
- `boruna-tooling`'s `sha256_hex()` may need to be made `pub` (currently might be `pub(crate)`)
- `CapabilityGateway` access needed in `HostEffectExecutor` — may need to expose from `AppRuntime` or inject

---

## Implementation Order

1. Phase 1A: Create `executor.rs` with `EffectExecutor` trait + `MockEffectExecutor`
2. Phase 1B: Add `send_with_executor()` to `AppRuntime` and `send_with_effects()` to `TestHarness`
3. Phase 1C: Implement `HostEffectExecutor` with capability gateway dispatch
4. Phase 1D: Add Phase 1 tests (7 tests)
5. Phase 2A: Add `version` field to `EventLog`, update `to_json()`/`from_json()`
6. Phase 2B: Add Phase 2 tests (6 tests)
7. Phase 2C: Unify CLI `trace-hash` to SHA-256
8. Phase 3: Add invariant tests (7 tests)
9. Run `cargo test --workspace`, fix any issues
10. Update `docs/FRAMEWORK_STATUS.md` (Effect System → 100%)

---

## References & Research

### Internal References

- Framework runtime: `crates/llmfw/src/runtime.rs:128-179` (AppRuntime::send)
- Effect parsing: `crates/llmfw/src/effect.rs:87-125` (parse_effects, parse_update_result)
- EventLog: `crates/llmvm/src/replay.rs:39-104` (EventLog struct, Event enum)
- ReplayEngine: `crates/llmvm/src/replay.rs:106-151` (verify)
- CapabilityGateway: `crates/llmvm/src/capability_gateway.rs:151-189` (call with logging)
- CLI trace-hash: `crates/llmvm-cli/src/main.rs:882-914` (fold-based hash)
- SHA-256 hash: `tooling/src/trace2tests/mod.rs:56-60` (sha256_hex)
- TraceFile version: `tooling/src/trace2tests/mod.rs:10-24` (TRACE_VERSION = 1)
- TestHarness: `crates/llmfw/src/testing.rs:10-160`
- Golden tests: `crates/llmfw/src/tests.rs:670-868`
- Dogfood findings: `docs/DOGFOOD_FINDINGS.md` (F7: Effect execution not implemented)
- Framework status: `docs/FRAMEWORK_STATUS.md` (Effect System 85%)
- Effects guide: `docs/EFFECTS_GUIDE.md` (lifecycle, dispatch mapping)
- Determinism contract: `docs/DETERMINISM_CONTRACT.md` (replay guarantees)
