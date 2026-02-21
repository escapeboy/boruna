# Dogfood Findings

Discovered during implementation of three real applications:
- Admin CRUD System (`examples/admin_crud/`)
- Realtime Notification System (`examples/realtime_notifications/`)
- Offline-First Sync Todo (`examples/offline_sync_todo/`)

## Critical Findings

### F1: No Dynamic Collection Operations

**Severity**: High — fundamentally shapes app architecture.

The language has no list/array manipulation. Lists are compiled to fixed-size
records (`MakeRecord(0xFFFF, count)`). There are no opcodes for:
- Append/push to list
- Index into list
- Get list length
- Iterate over list
- Remove from list
- Filter/map/reduce

**Impact**: Apps cannot store variable-length collections in State.
All three apps use "view model" architecture: State holds scalars and
the current operation, while persistent data lives behind effects (db/http).

**Workaround**: Model state as a fixed-field view model. Use effects for
persistence. This is actually correct architecture for the Elm pattern, but
it was forced rather than chosen.

**Recommendation**: Add list opcodes to the VM:
- `ListLen` — push list length
- `ListGet(index)` — index into list
- `ListPush` — append to list (returns new list)
- `ListSlice` — subsequence

This would unlock apps that keep collections in local state (e.g., a real
todo list with individual items, not just counts).

### F2: No String-to-Int Conversion

**Severity**: Medium.

Cannot parse numeric values from string payloads. Effect callbacks deliver
results as strings, but there's no way to convert "42" to Int(42) in
pure update() logic.

**Impact**: Notification app cannot parse sequence numbers from server
responses. Sync app cannot parse remote version numbers. All numeric
tracking must be done purely through local counters.

**Workaround**: Use local Int counters for all numeric state. Treat string
payloads as opaque data or status indicators ("ok", "conflict", etc.).

**Recommendation**: Add `parse_int(s: String) -> Option<Int>` builtin.

### F3: No String Contains/StartsWith

**Severity**: Medium.

String comparison is limited to equality (`==`, `!=`) and lexicographic
ordering (`<`, `>`, `<=`, `>=`). There is no `contains()`, `starts_with()`,
or `split()` operation.

**Impact**: Cannot parse structured payloads. For example, "conflict:5"
cannot be split into tag="conflict" and version=5. Each distinct response
must be a separate message tag or simple string value.

**Workaround**: Use distinct message tags for each response variant rather
than structured payload strings.

**Recommendation**: Add string builtins: `str_contains`, `str_starts_with`.
Parsing/splitting is a lower priority — it requires Int conversion too.

### F4: Deeply Nested If-Else Chains

**Severity**: Medium — affects readability and maintainability.

Without pattern matching on string values or a switch/match statement for
strings, message dispatch requires deeply nested if-else chains. The admin
CRUD app has 10+ nesting levels.

**Impact**: Code is hard to read and error-prone. Each new message tag adds
another nesting level.

**Workaround**: Structure code with consistent indentation and comments.

**Recommendation**: Support string matching in `match` expressions:
```llm
match msg.tag {
    "create_user" => ...,
    "delete_user" => ...,
    _ => ...,
}
```

### F5: No Record Update Syntax

**Severity**: Medium.

Changing one field of a record requires reconstructing the entire record with
all fields copied. The State record in admin_crud has 11 fields — every state
transition repeats all 11 field assignments even when only 1 changes.

**Impact**: Massive code duplication. High risk of copy-paste errors.

**Workaround**: Accept the verbosity. Be extremely careful when copying
field assignments between branches.

**Recommendation**: Add record update syntax:
```llm
State { ..state, status: "ok" }
```

## Moderate Findings

### F6: Actor System Not Wired Into Framework

**Severity**: Moderate (known gap, documented in FRAMEWORK_STATUS.md).

The VM has ActorSystem with spawn/send/receive opcodes, but the framework
does not wire actors into the App protocol runtime. The notification app
simulates realtime events using timer effects instead of actors.

**Impact**: Cannot demonstrate true multi-actor patterns. The timer-based
simulation is deterministic and replayable, which partially validates the
architecture, but real actor scheduling is untested.

**Recommendation**: Wire ActorSystem into AppRuntime as the next milestone.

### F7: Effect Execution Not Implemented

**Severity**: Moderate (known gap).

Effects are parsed and validated but not executed by the framework. The host
is responsible for execution. In tests, we simulate effect results by sending
callback messages manually.

**Impact**: Apps cannot actually perform IO. The test pattern of manually
delivering callback messages is adequate for determinism testing but doesn't
prove end-to-end behavior.

**Recommendation**: Implement a MockEffectExecutor for testing that
automatically delivers callback messages for known effect types.

### F8: Bool Type Requires Int Workaround

**Severity**: Low.

While the language has a Bool type, record fields in practice must use Int
(0/1) for boolean-like values. The notification app uses `subscribed: Int`
and `online: Int` instead of Bool because the interaction between Bool and
if-expressions in records is unreliable.

**Recommendation**: Verify Bool field access and comparison work correctly
in all contexts.

## What Worked Well

### W1: View Model Architecture

State-as-view-model is the correct pattern for capability-gated apps. All
three apps demonstrate this naturally: State is small, deterministic, and
fully serializable. Persistent data lives behind effects.

### W2: Effect-Based IO

The declarative effect system works exactly as designed. Effects are produced
by pure update logic, validated against policy, and would be executed by the
host. The separation is clean and replayable.

### W3: Deterministic Replay

All three apps pass golden determinism tests and replay equivalence tests.
The framework's determinism guarantees hold up under real usage patterns:
CRUD flows, subscription lifecycles, offline/online transitions, and conflict
resolution.

### W4: Policy Enforcement

Role-based authorization (admin CRUD), rate limiting (notifications), and
network budgets (sync todo) all work via the existing policy layer. No
changes were needed.

### W5: Cycle Logging

The cycle log captures complete history: message, state before/after, effects,
and UI tree. This enables the trace hash and replay commands to work
out of the box.

## Priority Recommendations

| Priority | Finding | Change Required |
|----------|---------|-----------------|
| 1 | F1 | VM list opcodes (ListLen, ListGet, ListPush) |
| 2 | F5 | Record update syntax (`..state`) |
| 3 | F4 | String matching in match expressions |
| 4 | F2 | parse_int builtin |
| 5 | F3 | String contains/starts_with builtins |
| 6 | F6 | Wire actors into framework |

None of these are needed for the current apps to work correctly. They would
reduce boilerplate and enable richer applications.
