# Framework Implementation Status

Last updated: 2026-02-21 (post-effect-execution)
Tests: 210 passing (6 boruna-bytecode + 53 boruna-compiler + 96 boruna-framework + 55 boruna-vm)

## Spec Section Checklist

### 1. Application Protocol
- [x] `init()` — 0 params, returns State
- [x] `update(state, msg)` — 2 params, no capabilities
- [x] `view(state)` — 1 param, no capabilities
- [x] `policies()` — optional, 0 params, no capabilities
- [x] Compile-time validation (AppValidator)
- [x] Signature checking (arity, caps)
- [x] State/Message type detection by convention
- [x] **Runtime purity enforcement** — update/view use `Policy::deny_all()`, CapCall produces `PurityViolation`

### 2. Effect System
- [x] Effect struct (kind, payload, callback_tag)
- [x] 8 EffectKind variants (HttpRequest, DbQuery, FsRead, FsWrite, Timer, Random, SpawnActor, EmitUi)
- [x] `parse_effects()` — handles both Value::List and Record{0xFFFF} (and native List post-dogfix)
- [x] `parse_update_result()` — splits (state, effects)
- [x] Effect-to-capability mapping
- [x] **Effect execution by framework runtime** — `EffectExecutor` trait, `MockEffectExecutor`, `HostEffectExecutor`

### 3. State Management
- [x] StateMachine with current/history/cycle
- [x] `transition()` — records snapshot
- [x] `snapshot()` — JSON serialization
- [x] `restore()` — JSON deserialization
- [x] `diff_from_cycle()` — field-level diffing
- [x] `diff_values()` — static diff utility
- [x] `rewind()` — time-travel to previous cycle
- [x] History cap (max 1000 snapshots)

### 4. UI Model
- [x] UINode { tag, props, children }
- [x] `value_to_ui_tree()` — VM Value → UINode
- [x] `ui_tree_to_value()` — UINode → VM Value
- [x] view() called after each update cycle

### 5. Actor Integration
- [ ] Child actor spawning via framework (VM-level ActorSystem exists but not wired)
- [ ] Message routing between actors
- [ ] Supervision model

### 6. Policy Layer
- [x] PolicySet { capabilities, max_effects_per_cycle, max_steps }
- [x] `from_value()` — parse from VM return (handles list literal representation)
- [x] `check_effect()` — per-effect validation
- [x] `check_batch()` — batch limit + per-effect
- [x] `allow_all()` — permissive default
- [x] `to_json()` — structured JSON diagnostic output
- [x] `error_to_json()` — structured error diagnostics
- [x] Deny specific capabilities, boundary testing
- [ ] Network allowlist, filesystem path allowlist (future)

### 7. Testing Harness
- [x] `TestHarness::from_source()`
- [x] `simulate()` — run message sequence
- [x] `assert_state_field()` — check field by index
- [x] `assert_effects()` — check effect kinds
- [x] `assert_state()` — full state equality
- [x] `replay_verify()` — identical-source replay check
- [x] `snapshot()`, `rewind()`, `cycle_log()`
- [x] `send_with_effects()` — execute effects and get callbacks
- [x] **Golden determinism tests** — counter, todo, effects apps
- [x] **Replay equivalence tests** — counter, effects apps
- [x] **Snapshot stability tests** — bitwise JSON comparison
- [x] **Determinism invariant tests** — 7 tests for effect ordering, purity, replay, failure
- [ ] `fuzz_messages()` — random message generation (future)

### 8. CLI Extensions
- [x] `framework new <name>` — template generation
- [x] `framework validate <file>` — protocol validation
- [x] `framework test <file>` — send messages, show state
- [x] `framework inspect-state <file>` — JSON snapshot + diffs
- [x] `framework simulate <file>` — step-by-step transitions
- [x] `framework inspect [--json]` — App contract summary
- [x] `framework diag` — structured JSON diagnostics
- [x] `framework trace-hash` — stable SHA-256 trace hash
- [x] `framework replay` — one-command replay from log

### 9. Examples
- [x] counter_app.ax → 1
- [x] todo_app.ax → 1
- [x] parallel_demo.ax → 3
- [x] All 7 original examples still pass
- [x] **admin_crud_app.ax** → 0 (dogfood: CRUD + auth + db effects)
- [x] **notification_app.ax** → 3 (dogfood: timer + http + rate limiting)
- [x] **sync_todo_app.ax** → 3 (dogfood: offline/online + conflict resolution)
- [x] All 13 examples produce correct output
- [x] Dogfood examples rewritten with match + spread (60-65% code reduction)

### 10. Documentation
- [x] FRAMEWORK_SPEC.md — design specification
- [x] FRAMEWORK_STATUS.md — implementation status (this file)
- [x] FRAMEWORK_API.md — public API reference
- [x] DETERMINISM_CONTRACT.md — determinism guarantees
- [x] APP_TEMPLATE.md — canonical file layout + skeleton
- [x] EFFECTS_GUIDE.md — effect system usage
- [x] ACTORS_GUIDE.md — actor integration (partial, honest about gaps)
- [x] TESTING_GUIDE.md — simulation, golden tests, replay, CLI
- [x] **DOGFOOD_FINDINGS.md** — limitations discovered during real app development
- [x] **CHANGELOG_DOGFIX.md** — summary of dogfix milestone changes

## Summary

| Area              | Status  | Notes                                    |
|-------------------|---------|------------------------------------------|
| App Protocol      | 100%    | Full purity enforcement at runtime       |
| Effect System     | 100%    | Full execution via EffectExecutor trait   |
| State Management  | 100%    | Complete                                 |
| UI Model          | 100%    | Complete                                 |
| Actor Integration | 10%     | VM-level only, not in framework          |
| Policy Layer      | 90%     | JSON diagnostics; missing allowlists     |
| Testing Harness   | 98%     | Golden + invariant + replay; missing fuzz |
| CLI Extensions    | 100%    | All 9 commands implemented               |
| Documentation     | 100%    | All 9 docs exist and match reality       |

## Hardening Pass Completed

1. **Purity enforcement**: update/view use deny-all policy ✓
2. **Golden determinism tests**: 7 golden tests (counter, todo, effects, replay, snapshot) ✓
3. **Policy JSON diagnostics**: `to_json()` + `error_to_json()` ✓
4. **CLI agent tooling**: inspect, diag, trace-hash, replay ✓
5. **API surface lock**: 4 API snapshot tests ✓
6. **Host integration**: 5 integration tests (boot→render→event cycle) ✓
7. **Documentation**: 6 new docs + status update ✓

## Dogfood Pass Completed

3 real applications built and tested:

1. **Admin CRUD** — db effects, role-based auth (admin/editor/viewer), full CRUD lifecycle
2. **Realtime Notifications** — timer polling, http effects, rate limiting, subscription management
3. **Offline Sync Todo** — offline queue, online sync, conflict detection + resolution

19 new tests:
- 3 golden determinism tests (one per app)
- 3 replay equivalence tests (one per app)
- 3 policy enforcement tests (capabilities, limits)
- 4 authorization tests (role-based access control)
- 3 app-specific behavior tests (rate limiting, message ordering, conflict resolution)
- 2 cross-cutting tests (offline queue, trace hash stability)
- 1 snapshot stability test

Findings documented in DOGFOOD_FINDINGS.md (8 findings, 5 positive observations).

## Dogfix Milestone Completed

Addressed 5 of 8 dogfood findings (F1–F5) with minimal, targeted changes:

### Language Features Added
1. **Real list support (F1)** — `MakeList`, `ListLen`, `ListGet`, `ListPush` opcodes; compiler emits native `Value::List`
2. **Record spread syntax (F5)** — `State { ..base, field: value }` lowers at compile time to field copy + override
3. **String matching (F4)** — `match msg.tag { "add" => ..., _ => ... }` compiles as comparison chains
4. **String builtins (F2+F3)** — `parse_int`, `str_contains`, `str_starts_with` as VM opcodes

### Test Additions
42 new tests:
- 20 VM opcode tests (list ops, parse_int, str_contains, str_starts_with) in boruna-vm
- 22 compiler E2E tests (list literals, spread, string match, builtins) in boruna-compiler

### Example Improvements
All 3 dogfood apps rewritten with match + spread:
- admin_crud: 441 → 156 lines (65% reduction)
- notification: 287 → 119 lines (59% reduction)
- sync_todo: 423 → 161 lines (62% reduction)

### Deferred
- F6 (Actor wiring) — requires framework runtime changes
- ~~F7 (Effect execution) — requires host integration~~ **Resolved**
- F8 (Bool fields) — low severity, workaround acceptable

## Effect Execution Milestone Completed

Addressed F7 (Effect Execution) with three changes:

### A. Host Effect Execution
- `EffectExecutor` trait — pluggable effect dispatch
- `MockEffectExecutor` — deterministic BTreeMap-based stub responses for testing
- `HostEffectExecutor` — dispatches all 9 effect kinds via `CapabilityGateway`
- `AppRuntime::send_with_executor()` — execute effects inline
- `TestHarness::send_with_effects()` — convenience for test code
- EmitUi is fire-and-forget (no callback); SpawnActor returns error (actors not wired)

### B. Replay/Trace Log Stability
- `EventLog` now has `version: u32` field (default 1, serde backward-compat)
- `from_json()` rejects unknown versions
- CLI `trace-hash` upgraded from fold-based hash to SHA-256 (consistent with trace2tests)
- 6 golden format stability tests lock the serialization format

### C. Determinism Invariants
7 invariant tests protecting:
- INV-1: Effect execution order determinism
- INV-2: Purity (effects are the only data channel)
- INV-3: Effect list determinism for same (state, msg)
- INV-4: Failed effect error determinism
- INV-5: Cycle fingerprint stability with effects
- INV-6: Snapshot bitwise identity with callbacks
- INV-7: Replay equivalence with effect-producing apps

### Test Additions
26 new tests:
- 13 executor tests (mock, host per-kind, ordering, error, round-trip, event log)
- 6 EventLog format stability tests (version, backward compat, golden format)
- 7 determinism invariant tests
