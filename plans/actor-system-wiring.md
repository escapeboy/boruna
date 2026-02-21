# feat: Wire Actor System into VM and Framework Runtime

**Type**: Enhancement (major feature)
**Severity**: Moderate (F6 from DOGFOOD_FINDINGS)
**Date**: 2026-02-21

## Overview

Wire the existing ActorSystem skeleton into functional multi-actor execution. The bytecode layer (opcodes, AST, codegen, Value::ActorId) is 100% complete. The VM and framework layers are stubbed. This plan connects the dots: bounded execution, deterministic scheduling, message passing, event logging, framework effect integration, and supervision.

## Problem Statement

The VM has `SpawnActor`, `SendMsg`, `ReceiveMsg` opcodes that compile correctly but execute as no-ops:
- `SpawnActor` always returns `ActorId(0)`
- `SendMsg` discards both arguments
- `ReceiveMsg` always returns `Unit`

The `ActorSystem` struct exists with `spawn_root()`, `deliver_messages()`, and `run_single()`, but `deliver_messages()` is dead code and there is no multi-actor scheduling loop. The framework's `HostEffectExecutor` returns `"unsupported effect: spawn_actor"`.

**Impact**: Cannot build real concurrent applications. Timer-based simulation works (notification_app.ax) but true actor patterns are blocked.

## Architecture

### Current State
```
Compiler (100%)          VM (stubbed)              Framework (0%)
spawn → SpawnActor(idx)  push ActorId(0)           SpawnActor → "unsupported"
send  → SendMsg          pop & discard             no SendToActor effect
receive → ReceiveMsg     push Unit                 no routing
```

### Target State
```
Compiler (100%)          VM (functional)            Framework (wired)
spawn → SpawnActor(idx)  scheduler.spawn_child()   SpawnActor → create child
send  → SendMsg          scheduler.queue_send()    SendToActor → route msg
receive → ReceiveMsg     mailbox.pop / block       child → parent routing
```

### Key Design Decisions

1. **Bounded execution**: New `execute_bounded(budget) -> StepResult` on `Vm`. Existing `run()` unchanged (calls `execute_bounded(max_steps)` internally). No breaking API change.

2. **Scheduler location**: Integrated into `ActorSystem` as `run() -> Result<Value, VmError>` replacing `run_single()` for multi-actor mode. `run_single()` preserved for backward compat.

3. **Children are raw VM functions** (not Elm apps). `SpawnActor(func_idx)` spawns a function from the same Module. Child runs that function, can send/receive messages. Only root actor uses the Elm architecture.

4. **FIFO mailboxes** (no selective receive). Simple, deterministic, sufficient for the Elm pattern.

5. **Messages delivered in batches at round boundaries**. Deterministic order: messages appear in target's mailbox in the order they were sent during the round (determined by actor execution order).

6. **Capability gating**: Add `Capability::ActorSpawn` (id: 8) and `Capability::ActorSend` (id: 9). Functions using actor opcodes must declare capabilities. `PolicySet::allow_all()` updated to include them.

7. **Master EventLog**: `ActorSystem` owns a single `EventLog`. Actor VMs do not have individual logs in multi-actor mode — the scheduler writes all events to the master log.

8. **view() is root-only**. Child actors are headless workers. Root actor aggregates child state via messages.

9. **Module cloning**: Each actor gets a cloned Module (matches current `Vm::new` pattern). Optimize to `Arc<Module>` later if memory is an issue.

## Implementation Phases

### Phase 1: Bounded Execution (VM layer foundation)

Add `StepResult` enum and `execute_bounded()` to `Vm` without changing any existing API.

**Files to modify**:
- `crates/llmvm/src/vm.rs` — add `StepResult` enum, `execute_bounded()` method, `actor_id` field, `mailbox` field, `outgoing_messages` Vec, `spawn_requests` Vec
- `crates/llmvm/src/vm.rs` — existing `run()` calls `execute_bounded(self.max_steps)` internally (backward compat)

**New types**:
```rust
// crates/llmvm/src/vm.rs

pub enum StepResult {
    Completed(Value),
    Yielded { steps_used: u64 },
    Blocked,
    Error(VmError),
}

pub struct SpawnRequest {
    pub func_idx: u32,
}
```

**Changes to Vm struct**:
```rust
pub struct Vm {
    // ... existing fields ...
    actor_id: u64,                          // NEW: which actor this VM belongs to
    mailbox: VecDeque<Message>,             // NEW: incoming messages
    outgoing_messages: Vec<(u64, Value)>,   // NEW: (target_id, payload)
    spawn_requests: Vec<SpawnRequest>,      // NEW: pending spawns
}
```

**Tests** (in `crates/llmvm/src/tests.rs`):
- `test_execute_bounded_completes` — simple program finishes within budget
- `test_execute_bounded_yields` — tight loop exceeds budget, returns `Yielded`
- `test_execute_bounded_backward_compat` — `run()` still works identically
- `test_receive_msg_blocks_when_empty` — `ReceiveMsg` returns `Blocked` with empty mailbox
- `test_receive_msg_pops_from_mailbox` — `ReceiveMsg` returns message when available

### Phase 2: Opcode Wiring (VM layer)

Wire the three stubbed opcodes to use the new VM fields instead of no-ops.

**Files to modify**:
- `crates/llmvm/src/vm.rs` — change `SpawnActor`, `SendMsg`, `ReceiveMsg` handlers

**SpawnActor handler**:
```rust
Op::SpawnActor(func_idx) => {
    let child_id = self.next_spawn_id;
    self.next_spawn_id += 1;
    self.spawn_requests.push(SpawnRequest { func_idx });
    self.push(Value::ActorId(child_id))?;
}
```
Note: `next_spawn_id` is assigned by the scheduler before each actor's turn, so the returned ID is deterministic.

**SendMsg handler**:
```rust
Op::SendMsg => {
    let payload = self.pop()?;
    let target = self.pop()?;
    match target {
        Value::ActorId(id) => {
            self.outgoing_messages.push((id, payload));
        }
        _ => return Err(VmError::TypeError(
            format!("SendMsg: expected ActorId, got {:?}", target)
        )),
    }
}
```

**ReceiveMsg handler**:
```rust
Op::ReceiveMsg => {
    if let Some(msg) = self.mailbox.pop_front() {
        self.push(msg.payload)?;
    } else {
        // Rewind IP so ReceiveMsg re-executes when resumed
        if let Some(frame) = self.call_stack.last_mut() {
            frame.ip -= 1; // back up to ReceiveMsg instruction
        }
        return Ok(StepResult::Blocked);
    }
}
```

**Tests**:
- `test_spawn_actor_returns_unique_ids` — two spawns return different ActorIds
- `test_spawn_actor_creates_request` — spawn_requests Vec populated
- `test_send_msg_queues_outgoing` — outgoing_messages populated correctly
- `test_send_msg_type_error` — sending to non-ActorId produces TypeError
- `test_receive_msg_consumes_fifo` — messages consumed in order

### Phase 3: Deterministic Scheduler (VM layer core)

Replace the ActorSystem internals with a working round-robin scheduler.

**Files to modify**:
- `crates/llmvm/src/actor.rs` — rewrite `ActorSystem`, add `ActorStatus`, `Actor` enrichment

**New types**:
```rust
// crates/llmvm/src/actor.rs

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActorStatus {
    Runnable,
    Blocked,
    Completed,
    Failed,
}

pub struct Actor {
    pub id: u64,
    vm: Vm,
    status: ActorStatus,
    parent: Option<u64>,
    children: Vec<u64>,
    entry_function: String,
}
```

**ActorSystem::run() algorithm**:
```
loop {
    if run_queue empty && no pending messages:
        if any actors blocked → return Err(Deadlock)
        else → return root result

    round += 1
    if round > max_rounds → return Err(MaxRoundsExceeded)

    // Phase 1: Execute each runnable actor for budget reductions
    for actor_id in run_queue.drain():
        log SchedulerTick(round, actor_id)
        assign next_spawn_id based on next_id
        match actor.vm.execute_bounded(budget):
            Completed → mark completed, notify parent
            Yielded → re-enqueue
            Blocked → move to wait set
            Error → mark failed, notify parent (supervision)
        collect spawn_requests → create child actors
        collect outgoing_messages → add to pending

    // Phase 2: Deliver pending messages (deterministic order)
    sort pending by (target_id, sender_id) for determinism
    for (from, to, payload) in pending:
        log MessageSend(from, to, payload)
        push to target.mailbox
        log MessageReceive(to, payload)

    // Phase 3: Wake blocked actors with non-empty mailboxes
    for actor in wait_set:
        if mailbox non-empty → move to run_queue
}
```

**Actor mailbox is on Vm, not Actor**: Since `ReceiveMsg` accesses the mailbox inside `execute_bounded()`, the mailbox lives on the `Vm` struct (added in Phase 1). The scheduler delivers messages directly to `actor.vm.mailbox`.

**Tests**:
- `test_single_actor_backward_compat` — `run()` with one actor produces same result as old `run_single()`
- `test_two_actors_round_robin` — parent spawns child, both execute
- `test_message_passing_parent_to_child` — parent sends, child receives
- `test_message_passing_child_to_parent` — child sends, parent receives
- `test_message_delivery_order_deterministic` — multiple senders to same target, order is consistent
- `test_blocked_actor_wakes_on_message` — actor blocked on ReceiveMsg resumes when message arrives
- `test_deadlock_detection` — two actors both blocked, no messages pending
- `test_max_rounds_exceeded` — infinite message ping-pong hits max_rounds
- `test_scheduler_event_log` — SchedulerTick, ActorSpawn, MessageSend events logged

### Phase 4: EventLog Completion (VM layer)

Add missing logging methods and extend replay verification.

**Files to modify**:
- `crates/llmvm/src/replay.rs` — add `log_message_receive()`, `log_scheduler_tick()`, extend `ReplayEngine::verify()`

**New methods**:
```rust
pub fn log_message_receive(&mut self, actor_id: u64, payload: &Value) {
    self.events.push(Event::MessageReceive {
        actor_id,
        payload: payload.clone(),
    });
}

pub fn log_scheduler_tick(&mut self, round: u64, active_actor: u64) {
    self.events.push(Event::SchedulerTick {
        round,
        active_actor,
    });
}
```

**ReplayEngine extension**: New method `verify_full()` that compares ALL event types (CapCall + ActorSpawn + MessageSend + MessageReceive + SchedulerTick), not just CapCall.

**Tests**:
- `test_log_message_receive_roundtrip` — serialize/deserialize MessageReceive
- `test_log_scheduler_tick_roundtrip` — serialize/deserialize SchedulerTick
- `test_replay_verify_full_identical` — full verification passes for identical logs
- `test_replay_verify_full_diverged_scheduler` — different scheduling order detected
- `test_replay_verify_full_diverged_messages` — different message order detected

### Phase 5: Capability Gating (Bytecode + Framework)

Add actor capabilities so spawning/sending are policy-gated.

**Files to modify**:
- `crates/llmbc/src/capability.rs` — add `ActorSpawn` (id: 8), `ActorSend` (id: 9)
- `crates/llmfw/src/policy.rs` — update `allow_all()` to include new capabilities
- `crates/llmfw/src/executor.rs` — update `effect_to_capability()` for SpawnActor
- `crates/llmfw/src/effect.rs` — add `SendToActor` effect kind

**New capabilities**:
```rust
// crates/llmbc/src/capability.rs
pub enum Capability {
    // ... existing 8 ...
    ActorSpawn,  // id: 8, name: "actor.spawn"
    ActorSend,   // id: 9, name: "actor.send"
}
```

**New effect kind**:
```rust
// crates/llmfw/src/effect.rs
pub enum EffectKind {
    // ... existing 9 ...
    SendToActor,  // name: "send_to_actor"
}
```

**Tests**:
- `test_capability_actor_spawn_roundtrip` — id/name/from_name for ActorSpawn
- `test_capability_actor_send_roundtrip` — id/name/from_name for ActorSend
- `test_policy_allow_all_includes_actors` — allow_all() permits actor ops
- `test_policy_deny_actor_spawn` — can deny spawning specifically
- `test_effect_send_to_actor_parsing` — effect kind parsed from "send_to_actor"

### Phase 6: Framework Integration (Framework layer)

Wire `HostEffectExecutor` to handle `SpawnActor` and `SendToActor` effects.

**Files to modify**:
- `crates/llmfw/src/executor.rs` — handle SpawnActor and SendToActor in HostEffectExecutor
- `crates/llmfw/src/runtime.rs` — add `actor_system: Option<ActorSystem>` to AppRuntime
- `crates/llmfw/src/testing.rs` — update MockEffectExecutor for actor effects
- `crates/llmfw/src/error.rs` — add actor-related FrameworkError variants

**SpawnActor effect handling**:
```rust
// In HostEffectExecutor::execute()
EffectKind::SpawnActor => {
    // payload is Value::String("function_name")
    let func_name = match &effect.payload {
        Value::String(s) => s.clone(),
        _ => {
            messages.push(AppMessage::new(
                &effect.callback_tag,
                Value::String("spawn_actor: payload must be function name".into()),
            ));
            continue;
        }
    };
    // Look up function index in module
    // Create child actor via ActorSystem
    // Return ActorId as callback
    messages.push(AppMessage::new(
        &effect.callback_tag,
        Value::ActorId(child_id),
    ));
}
```

**SendToActor effect handling**:
```rust
EffectKind::SendToActor => {
    // payload is Value::Record { fields: [actor_id, message] }
    // Route message through ActorSystem
    // Return ack as callback
}
```

**MockEffectExecutor updates**:
```rust
// MockEffectExecutor returns deterministic mock values:
EffectKind::SpawnActor => Value::ActorId(self.next_mock_actor_id()),
EffectKind::SendToActor => Value::String("delivered".into()),
```

**Tests**:
- `test_host_executor_spawn_actor` — creates child, returns ActorId
- `test_host_executor_spawn_invalid_function` — bad function name returns error
- `test_host_executor_send_to_actor` — routes message to child
- `test_host_executor_send_to_dead_actor` — returns error callback
- `test_mock_executor_spawn_actor` — mock returns deterministic ActorId
- `test_app_runtime_with_actors` — full init→update→spawn→message→view cycle
- `test_app_runtime_backward_compat` — existing apps work without actors

### Phase 7: Supervision (VM + Framework)

Implement OneForOne supervision: child crash delivers error message to parent.

**Files to modify**:
- `crates/llmvm/src/actor.rs` — handle actor failure in scheduler, notify parent
- `crates/llmvm/src/error.rs` — add `Deadlock`, `MaxRoundsExceeded` variants

**Supervision behavior**:
- When a child actor's `execute_bounded()` returns `Error(e)`:
  1. Mark child as `ActorStatus::Failed`
  2. Queue message to parent: `Message { from: child_id, payload: Value::Record { fields: [Value::String("actor_error"), Value::String(error_desc), Value::ActorId(child_id)] } }`
  3. Mark all of the child's children as `Failed` too (cascade)
- No automatic restart in Phase 1 (keep it simple). Parent can re-spawn if needed.

**New error variants**:
```rust
// crates/llmvm/src/error.rs
pub enum VmError {
    // ... existing ...
    Deadlock,
    MaxRoundsExceeded(u64),
}
```

**Tests**:
- `test_supervision_child_crash_notifies_parent` — parent receives error message
- `test_supervision_cascade_failure` — grandchild crash cascades
- `test_deadlock_error_variant` — VmError::Deadlock display
- `test_max_rounds_error_variant` — VmError::MaxRoundsExceeded display

### Phase 8: CLI Integration & Examples

Add multi-actor support to CLI and create example programs.

**Files to modify**:
- `crates/llmvm-cli/src/main.rs` — use ActorSystem for programs with actor ops
- `examples/framework/` — add actor example(s)

**CLI changes**: When `run` subcommand detects actor opcodes in the module, use `ActorSystem::run()` instead of `Vm::run()`.

**Example**: `examples/framework/actor_ping_pong.ax`
```ax
fn pong() -> Int {
    let msg = receive
    let sender = msg.from
    send sender 42
    0
}

fn main() -> Int {
    let child = spawn pong
    send child 1
    let result = receive
    result
}
```

**Tests**:
- `test_ping_pong_example` — example compiles and runs correctly
- `test_actor_example_deterministic` — two runs produce identical results

## Error Handling

| Error | Trigger | Response |
|-------|---------|----------|
| `TypeError` | SendMsg to non-ActorId | VmError returned |
| `ActorNotFound` | Send to dead/unknown actor | Error message to sender |
| `Deadlock` | All actors blocked, no pending | VmError::Deadlock |
| `MaxRoundsExceeded` | Round count > max_rounds | VmError::MaxRoundsExceeded |
| `SpawnFailed` | Invalid function index | Error callback to parent |
| `MailboxFull` | Mailbox exceeds 10,000 (future) | Not in Phase 1 |

## Backward Compatibility

Every change preserves existing behavior:
- `Vm::run()` unchanged — internally delegates to `execute_bounded(max_steps)` and converts `StepResult::Completed` to the existing `Result<Value, VmError>`
- `ActorSystem::run_single()` preserved alongside new `run()`
- All 471 existing tests must continue to pass
- Framework apps without actor effects work identically
- CLI `run` for single-actor programs unchanged

## Risk Analysis

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Bounded execution changes break existing VM behavior | Low | High | `run()` wraps `execute_bounded()` transparently; test backward compat first |
| Scheduler introduces nondeterminism | Medium | High | Fixed reduction budget, sorted message delivery, monotonic IDs, all logged |
| Memory growth with many actors (Module cloning) | Low | Medium | Each Module is small (bytecode); defer Arc optimization |
| ReceiveMsg IP rewind is fragile | Medium | Medium | Thorough testing; the rewind-by-1 pattern is well-established (BEAM does this) |
| New Capability variants break existing policy tests | Low | Low | Update `allow_all()` atomically; test policy round-trips |

## Dependencies

- No external crate dependencies needed
- All changes are within the existing Rust workspace
- Phase 1-4 are VM-only (boruna-vm crate)
- Phase 5 touches boruna-bytecode and boruna-framework
- Phase 6-7 are framework-only (boruna-framework crate)
- Phase 8 is CLI + examples

## Testing Strategy

**Minimum 40 new tests across all phases**:
- Phase 1: 5 tests (bounded execution)
- Phase 2: 5 tests (opcode wiring)
- Phase 3: 9 tests (scheduler)
- Phase 4: 5 tests (EventLog + replay)
- Phase 5: 5 tests (capabilities)
- Phase 6: 7 tests (framework integration)
- Phase 7: 4 tests (supervision)
- Phase 8: 2 tests (examples)

**Golden determinism tests**: Multi-actor programs run twice → identical EventLogs.

**Backward compat tests**: Added FIRST, before any changes, to lock current behavior.

## Documentation Updates

- `docs/FRAMEWORK_STATUS.md` — Actor Integration 10% → 80%+
- `docs/ACTORS_GUIDE.md` — update from "partial" to reflect reality
- `docs/DETERMINISM_CONTRACT.md` — validate multi-actor section matches implementation
- No new docs needed (existing docs already describe the target architecture)

## References

### Internal
- `crates/llmvm/src/actor.rs` — ActorSystem skeleton
- `crates/llmvm/src/vm.rs:270-282` — stubbed opcodes
- `crates/llmfw/src/executor.rs:106` — SpawnActor → None
- `crates/llmvm/src/replay.rs` — EventLog with actor event types
- `crates/llmbc/src/capability.rs` — Capability enum (needs ActorSpawn, ActorSend)
- `docs/DOGFOOD_FINDINGS.md` — F6: Actor wiring recommendation
- `docs/FRAMEWORK_SPEC.md` section 5 — Actor Integration spec
- `docs/DETERMINISM_CONTRACT.md` — Multi-actor determinism rules

### External
- BEAM VM scheduling — bounded reduction model (4000 reductions/slice)
- FoundationDB deterministic simulation — single-threaded actor testing
- Erlang/OTP supervision trees — OneForOne, OneForAll, RestForOne strategies
- Lunatic runtime — per-actor VM isolation pattern
