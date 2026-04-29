# std-authz

> Role and permission enforcement via policies

**Package:** `std.authz`  **Version:** `0.1.0`  **Capabilities required:** none

## Overview

`std-authz` provides a simple, policy-driven authorization model based on named roles and numeric privilege levels. Call `authz_check` in your `update` handler before applying any state change that requires a permission gate. Because all functions are pure and deterministic, authorization decisions are fully auditable and replayable from the evidence bundle.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.authz": "0.1.0"
```

## API Reference

### Types

#### `AuthzPolicy`

```
type AuthzPolicy {
    admin_role: String,
    editor_role: String,
    viewer_role: String,
    admin_level: Int,
    editor_level: Int,
    viewer_level: Int
}
```

Maps role names to numeric privilege levels. Higher levels have broader access.

#### `Role`

```
type Role { name: String, level: Int }
```

#### `Permission`

```
type Permission { resource: String, action: String }
```

#### `RolePermission`

```
type RolePermission { role_name: String, resource: String, action: String }
```

#### `AuthzResult`

```
type AuthzResult { allowed: Int, reason: String }
```

- `allowed` — `1` if the operation is permitted, `0` if denied
- `reason` — human-readable explanation for logging

### Functions

##### `authz_default_policy() -> AuthzPolicy`

Returns the built-in three-tier policy: `admin` (level 100), `editor` (level 50), `viewer` (level 10).

##### `authz_check(policy: AuthzPolicy, role_name: String, resource: String, action: String) -> AuthzResult`

The primary authorization gate. Checks whether `role_name` is permitted to perform `action` on `resource`.

Supported actions: `"read"` (requires viewer+), `"write"` (requires editor+), `"delete"` (requires admin).

**Parameters**
- `policy` — the active `AuthzPolicy`
- `role_name` — the principal's role string
- `resource` — the resource being accessed (informational; used for the reason message)
- `action` — `"read"`, `"write"`, or `"delete"`

**Returns** — `AuthzResult` with `allowed: 1` or `allowed: 0` and a descriptive reason.

**Example**
```
fn main() -> Int {
  let policy: AuthzPolicy = authz_default_policy()
  let result: AuthzResult = authz_check(policy, "editor", "documents", "write")
  result.allowed
}
```

##### `authz_require_role(policy: AuthzPolicy, role_name: String, min_level: Int) -> AuthzResult`

Returns allowed if the role's numeric level meets or exceeds `min_level`. Useful for custom thresholds beyond the three built-in actions.

##### `authz_role_level(policy: AuthzPolicy, role_name: String) -> Int`

Returns the numeric privilege level for a role name. Returns `0` for unknown roles.

##### `authz_guard_update(policy: AuthzPolicy, role_name: String, resource: String, action: String) -> Int`

Convenience wrapper that calls `authz_check` and returns only the `allowed` integer — handy in `if` guards.

##### `authz_is_admin(policy: AuthzPolicy, role_name: String) -> Int`

Returns `1` if `role_name` matches the policy's admin role.

##### `authz_can_write(policy: AuthzPolicy, role_name: String) -> Int`

Returns `1` if the role has editor-level or higher privileges.

##### `authz_can_read(policy: AuthzPolicy, role_name: String) -> Int`

Returns `1` if the role has viewer-level or higher privileges.

## Capabilities

None. All functions are pure predicates with no side effects.

## Notes / Limitations

- The policy is a fixed three-role structure. Custom roles beyond admin/editor/viewer can be checked via `authz_require_role` with an explicit `min_level`.
- Role names are compared by string equality; casing matters.
- `resource` is passed through to the reason string but is not used in access-control logic — access depends solely on the action and role level.
