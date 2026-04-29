# std-storage

> Typed local persistence abstraction

**Package:** `std.storage`  **Version:** `0.1.0`  **Capabilities required:** `fs.read`, `fs.write`

## Overview

`std-storage` provides a namespaced key-value persistence layer as `Effect` values. Operations are described purely at the `.ax` level; actual I/O happens in the Boruna runtime. Entries carry a monotonic `version` counter which makes conflict detection straightforward and keeps all storage operations fully replay-compatible.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.storage": "0.1.0"
```

Your policy must grant both `fs.read` and `fs.write`.

## API Reference

### Types

#### `Effect`

```
type Effect { kind: String, payload: String, callback_tag: String }
```

#### `StorageKey`

```
type StorageKey { namespace: String, key: String }
```

A typed composite key. The namespace scopes keys to a logical bucket (e.g. `"app"`, `"cache"`).

#### `StorageEntry`

```
type StorageEntry { namespace: String, key: String, value: String, version: Int }
```

A versioned key-value record. `value` is stored as a JSON string by convention. `version` is a monotonic integer incremented by `storage_bump_version`.

### Functions

#### Key construction

##### `storage_key(namespace: String, key: String) -> StorageKey`

Constructs a typed key. Useful for passing keys around without losing the namespace.

#### Effect builders

##### `storage_get(namespace: String, key: String, callback_tag: String) -> Effect`

Produces a read effect. The runtime delivers the stored value to `callback_tag`.

**Example**
```
fn main() -> Int {
  let eff: Effect = storage_get("app", "settings", "settings_loaded")
  0
}
```

##### `storage_set(namespace: String, key: String, value: String, callback_tag: String) -> Effect`

Produces a write effect. `value` is written at the given key.

##### `storage_delete(namespace: String, key: String, callback_tag: String) -> Effect`

Produces a delete effect. The key is removed from the namespace.

##### `storage_list(namespace: String, callback_tag: String) -> Effect`

Produces a list effect. The runtime delivers all keys in the namespace to `callback_tag`.

#### Entry helpers

##### `storage_make_entry(namespace: String, key: String, value: String, version: Int) -> StorageEntry`

Constructs a `StorageEntry` with an explicit version. Use when deserializing a stored entry.

##### `storage_bump_version(entry: StorageEntry, new_value: String) -> StorageEntry`

Returns a new entry with `value` updated and `version` incremented by one.

**Example: optimistic update**
```
fn main() -> Int {
  let entry: StorageEntry = storage_make_entry("app", "counter", "0", 1)
  let updated: StorageEntry = storage_bump_version(entry, "1")
  updated.version
}
```

##### `storage_is_newer(a: StorageEntry, b: StorageEntry) -> Int`

Returns `1` if `a.version > b.version`. Use for last-write-wins conflict resolution.

## Capabilities

Requires `fs.read` for read/list operations and `fs.write` for write/delete operations. The capabilities are enforced separately — a step that only reads can be granted `fs.read` alone.

## Notes / Limitations

- `value` is an untyped `String`. By convention store JSON, but the library does not enforce any encoding.
- Versioning is local only; it does not synchronize with remote storage. For distributed conflict resolution use `std-sync`.
- All four effect kinds (`storage_read`, `storage_write`, `storage_delete`, `storage_list`) are replay-safe: the runtime records the result in the `EventLog` and replays it deterministically.
