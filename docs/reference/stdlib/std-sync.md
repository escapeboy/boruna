# std-sync

> Offline queue and conflict resolution helpers

**Package:** `std.sync`  **Version:** `0.1.0`  **Capabilities required:** `net.fetch`

## Overview

`std-sync` tracks local-vs-remote divergence and manages an offline edit queue inside framework apps. All conflict-resolution logic is pure; only `sync_effect` produces a network `Effect`. Use it to build apps that continue working offline and flush pending changes when connectivity is restored.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.sync": "0.1.0"
```

Your policy must grant `net.fetch` for `sync_effect` calls to succeed.

## API Reference

### Types

#### `Effect`

```
type Effect { kind: String, payload: String, callback_tag: String }
```

#### `SyncState`

```
type SyncState {
    online: Int,
    pending_count: Int,
    synced_count: Int,
    status: String,
    local_version: Int,
    remote_version: Int,
    conflicts: Int,
    resolved: Int
}
```

- `online` — `1` when connected, `0` when offline
- `pending_count` — number of local edits not yet synced
- `synced_count` — cumulative count of successfully synced operations
- `status` — one of `"idle"`, `"queued"`, `"syncing"`, `"synced"`, `"offline"`, `"conflict"`, `"resolving"`
- `local_version` / `remote_version` — monotonic counters; divergence indicates pending changes
- `conflicts` / `resolved` — total conflicts detected vs resolved

### Functions

#### Initialization

##### `sync_init() -> SyncState`

Returns a fresh sync state: online, no pending edits, version `0`.

#### State transitions

##### `sync_queue_edit(state: SyncState) -> SyncState`

Records one new local edit. Increments `pending_count` and `local_version`. Sets status to `"syncing"` if online, `"queued"` if offline.

##### `sync_mark_synced(state: SyncState) -> SyncState`

Acknowledges one successfully synced edit. Decrements `pending_count` and advances `remote_version` to match `local_version`. Status becomes `"synced"` when the queue is drained.

##### `sync_go_offline(state: SyncState) -> SyncState`

Marks the state as offline and sets status to `"offline"`.

##### `sync_go_online(state: SyncState) -> SyncState`

Marks the state as online. Status becomes `"syncing"` if there are pending edits, `"synced"` otherwise.

##### `sync_detect_conflict(state: SyncState) -> SyncState`

Increments the conflict counter and sets status to `"conflict"`. Call when the server rejects an edit due to a version mismatch.

##### `sync_resolve_conflict(state: SyncState, strategy: String) -> SyncState`

Increments `resolved`, bumps `local_version`, and sets status to `"resolving"`. `strategy` is passed for audit purposes; conflict resolution logic is app-specific.

#### Predicates

##### `sync_needs_push(state: SyncState) -> Int`

Returns `1` if there are pending edits and the device is online — the signal to fire a sync effect.

##### `sync_is_idle(state: SyncState) -> Int`

Returns `1` if status is `"idle"` or `"synced"`.

##### `sync_has_conflicts(state: SyncState) -> Int`

Returns `1` if any unresolved conflicts remain.

#### Effects

##### `sync_effect(endpoint: String, callback_tag: String) -> Effect`

Produces an HTTP request effect targeting `endpoint`. Return this from `update` when `sync_needs_push` is true.

**Example**
```
fn main() -> Int {
  let s0: SyncState = sync_init()
  let s1: SyncState = sync_queue_edit(s0)
  let s2: SyncState = sync_go_offline(s1)
  let s3: SyncState = sync_queue_edit(s2)
  let s4: SyncState = sync_go_online(s3)
  s4.pending_count
}
```

## Capabilities

Requires `net.fetch` for `sync_effect`. All state-transition functions are pure and require no capability.

## Notes / Limitations

- `sync_resolve_conflict` records a resolution but does not merge data. The calling app is responsible for choosing and applying the correct value before calling this function.
- `sync_needs_push` checks connectivity and pending count only; rate-limiting and retry backoff are not built in. Combine with `std-http`'s `RetryConfig` for production use.
