# Standard Libraries Specification

Official, batteries-included standard libraries for the Boruna VM + Framework.

## Design Principles

1. **Deterministic** — All functions produce identical output for identical input. No hidden randomness or host-dependent behavior.
2. **Capability-gated** — Libraries that need side effects declare capabilities explicitly. Pure libraries require none.
3. **Replay-compatible** — All state transitions integrate with the framework's record/replay system.
4. **Minimal surface** — Small, focused APIs. One way to do each thing.
5. **LLM-friendly** — Predictable patterns, small function signatures, no magic.
6. **No runtime dependency cycles** — Libraries form a DAG. UI depends on nothing. Forms depends on validation. Testing depends on framework internals.

## Versioning Policy

- All std libraries start at `0.1.0`
- Semver: MAJOR.MINOR.PATCH
- Breaking changes bump MAJOR
- New features bump MINOR
- Bug fixes bump PATCH
- All libraries in a release are tested together

## Stability Guarantees

- Public API types and function signatures are stable within a MAJOR version
- JSON serialization format is stable within a MAJOR version
- All libraries pass determinism tests on every release

---

## Libraries

### 1. std.ui (UI Primitives)

**Package:** `std.ui` v0.1.0
**Capabilities:** none (pure)
**Depends on:** nothing

Public API:

```
// Layout
fn row(children: List<UINode>) -> UINode
fn column(children: List<UINode>) -> UINode
fn stack(children: List<UINode>) -> UINode

// Containers
fn container(content: UINode) -> UINode
fn card(title: String, content: UINode) -> UINode
fn section(heading: String, content: UINode) -> UINode

// Controls
fn button(label: String, on_click: String) -> UINode
fn input(name: String, value: String, on_change: String) -> UINode
fn select(name: String, options: List<String>, selected: String, on_change: String) -> UINode
fn checkbox(name: String, checked: Int, on_change: String) -> UINode

// Data display
fn table(headers: List<String>, rows: List<List<String>>) -> UINode
fn error_display(message: String) -> UINode
fn text(content: String) -> UINode
fn badge(label: String, variant: String) -> UINode
```

### 2. std.forms (Form Engine)

**Package:** `std.forms` v0.1.0
**Capabilities:** none (pure)
**Depends on:** std.validation

Public API:

```
type FieldState { value: String, touched: Int, dirty: Int, error: String }
type FormState { fields: List<FieldState>, field_names: List<String>, submitted: Int, valid: Int }

fn form_init(field_names: List<String>) -> FormState
fn form_set_field(form: FormState, name: String, value: String) -> FormState
fn form_touch_field(form: FormState, name: String) -> FormState
fn form_validate(form: FormState, rules: List<ValidationRule>) -> FormState
fn form_submit(form: FormState) -> FormState
fn form_reset(form: FormState) -> FormState
fn form_get_field(form: FormState, name: String) -> FieldState
fn form_is_valid(form: FormState) -> Int
fn form_to_record(form: FormState) -> List<String>
```

### 3. std.authz (Authorization)

**Package:** `std.authz` v0.1.0
**Capabilities:** none (pure)
**Depends on:** nothing

Public API:

```
type Role { name: String, level: Int }
type Permission { resource: String, action: String }
type AuthzPolicy { roles: List<Role>, permissions: List<Permission>, role_permissions: List<RolePermission> }
type RolePermission { role_name: String, resource: String, action: String }
type AuthzResult { allowed: Int, reason: String }

fn authz_check(policy: AuthzPolicy, role_name: String, resource: String, action: String) -> AuthzResult
fn authz_require_role(policy: AuthzPolicy, role_name: String, min_level: Int) -> AuthzResult
fn authz_require_permission(policy: AuthzPolicy, role_name: String, resource: String, action: String) -> AuthzResult
fn authz_guard_update(policy: AuthzPolicy, role_name: String, resource: String, action: String) -> Int
```

### 4. std.http (HTTP Abstractions)

**Package:** `std.http` v0.1.0
**Capabilities:** net.fetch
**Depends on:** nothing

Public API:

```
type HttpRequest { method: String, url: String, headers: List<String>, body: String }
type HttpResponse { status: Int, body: String, headers: List<String> }
type RetryConfig { max_retries: Int, backoff_ms: Int, backoff_multiplier: Int }

fn http_get(url: String) -> Effect
fn http_post(url: String, body: String) -> Effect
fn http_put(url: String, body: String) -> Effect
fn http_delete(url: String) -> Effect
fn http_request(req: HttpRequest) -> Effect
fn http_with_retry(req: HttpRequest, config: RetryConfig) -> List<Effect>
fn http_parse_response(payload: String) -> HttpResponse
```

### 5. std.db (Database Helpers)

**Package:** `std.db` v0.1.0
**Capabilities:** db.query
**Depends on:** nothing

Public API:

```
type Query { operation: String, table: String, columns: List<String>, conditions: List<String>, order_by: String, limit: Int, offset: Int }
type Pagination { page: Int, per_page: Int, total: Int }

fn db_select(table: String, columns: List<String>) -> Query
fn db_insert(table: String, columns: List<String>, values: List<String>) -> Query
fn db_update(table: String, sets: List<String>, conditions: List<String>) -> Query
fn db_delete(table: String, conditions: List<String>) -> Query
fn db_where(query: Query, condition: String) -> Query
fn db_order(query: Query, column: String) -> Query
fn db_paginate(query: Query, page: Int, per_page: Int) -> Query
fn db_to_effect(query: Query, callback_tag: String) -> Effect
fn pagination_info(page: Int, per_page: Int, total: Int) -> Pagination
fn pagination_has_next(p: Pagination) -> Int
fn pagination_has_prev(p: Pagination) -> Int
```

### 6. std.sync (Offline + Sync Helpers)

**Package:** `std.sync` v0.1.0
**Capabilities:** net.fetch
**Depends on:** nothing

Public API:

```
type SyncState { online: Int, pending_count: Int, synced_count: Int, status: String, local_version: Int, remote_version: Int, conflicts: Int }
type ConflictResolution { strategy: String, winner: String }

fn sync_init() -> SyncState
fn sync_queue_edit(state: SyncState) -> SyncState
fn sync_mark_synced(state: SyncState) -> SyncState
fn sync_go_offline(state: SyncState) -> SyncState
fn sync_go_online(state: SyncState) -> SyncState
fn sync_detect_conflict(state: SyncState) -> SyncState
fn sync_resolve_conflict(state: SyncState, strategy: String) -> SyncState
fn sync_effect(state: SyncState, endpoint: String, callback_tag: String) -> Effect
fn sync_needs_push(state: SyncState) -> Int
```

### 7. std.validation (Reusable Validation Rules)

**Package:** `std.validation` v0.1.0
**Capabilities:** none (pure)
**Depends on:** nothing

Public API:

```
type ValidationRule { field: String, rule_type: String, param: String, message: String }
type ValidationResult { valid: Int, errors: List<ValidationError> }
type ValidationError { field: String, message: String }

fn validate_required(field: String, value: String) -> ValidationResult
fn validate_min_length(field: String, value: String, min: Int) -> ValidationResult
fn validate_max_length(field: String, value: String, max: Int) -> ValidationResult
fn validate_numeric(field: String, value: String) -> ValidationResult
fn validate_min_value(field: String, value: Int, min: Int) -> ValidationResult
fn validate_max_value(field: String, value: Int, max: Int) -> ValidationResult
fn validate_email(field: String, value: String) -> ValidationResult
fn validate_all(rules: List<ValidationRule>, values: List<String>) -> ValidationResult
fn validation_merge(a: ValidationResult, b: ValidationResult) -> ValidationResult
```

### 8. std.routing (Declarative Routing)

**Package:** `std.routing` v0.1.0
**Capabilities:** none (pure)
**Depends on:** nothing

Public API:

```
type Route { path: String, name: String, params: List<String> }
type RouteMatch { matched: Int, route_name: String, params: List<String> }
type NavMsg { tag: String, payload: String }

fn route_define(name: String, path: String) -> Route
fn route_match(routes: List<Route>, path: String) -> RouteMatch
fn route_navigate(route_name: String) -> NavMsg
fn route_navigate_with_params(route_name: String, params: List<String>) -> NavMsg
fn route_extract_param(match_result: RouteMatch, index: Int) -> String
fn route_is_active(routes: List<Route>, current_path: String, route_name: String) -> Int
```

### 9. std.storage (Local Persistence)

**Package:** `std.storage` v0.1.0
**Capabilities:** fs.read, fs.write
**Depends on:** nothing

Public API:

```
type StorageKey { namespace: String, key: String }
type StorageEntry { key: StorageKey, value: String, version: Int }

fn storage_get(namespace: String, key: String) -> Effect
fn storage_set(namespace: String, key: String, value: String) -> Effect
fn storage_delete(namespace: String, key: String) -> Effect
fn storage_list(namespace: String) -> Effect
fn storage_key(namespace: String, key: String) -> StorageKey
fn storage_make_entry(key: StorageKey, value: String, version: Int) -> StorageEntry
```

### 10. std.notifications (Notification Helpers)

**Package:** `std.notifications` v0.1.0
**Capabilities:** time.now
**Depends on:** nothing

Public API:

```
type Notification { id: Int, level: String, message: String, auto_dismiss_ms: Int }
type NotificationQueue { items: List<Notification>, next_id: Int, max_visible: Int }

fn notification_init(max_visible: Int) -> NotificationQueue
fn notification_push(queue: NotificationQueue, level: String, message: String, dismiss_ms: Int) -> NotificationQueue
fn notification_dismiss(queue: NotificationQueue, id: Int) -> NotificationQueue
fn notification_dismiss_effect(id: Int, delay_ms: Int) -> Effect
fn notification_visible(queue: NotificationQueue) -> List<Notification>
fn notification_count(queue: NotificationQueue) -> Int
```

### 11. std.testing (Test Helpers)

**Package:** `std.testing` v0.1.0
**Capabilities:** none (pure)
**Depends on:** nothing

Public API:

```
type TestCase { name: String, messages: List<Msg>, expected_state_fields: List<String>, expected_values: List<String> }
type TestResult { name: String, passed: Int, failures: List<String> }
type TestSuite { name: String, cases: List<TestCase>, results: List<TestResult> }

fn test_case(name: String, messages: List<Msg>) -> TestCase
fn test_expect_field(tc: TestCase, field: String, expected: String) -> TestCase
fn test_run_case(tc: TestCase, init_state: State, update_fn: String) -> TestResult
fn test_suite(name: String, cases: List<TestCase>) -> TestSuite
fn test_all_passed(suite: TestSuite) -> Int
fn test_summary(suite: TestSuite) -> String
fn assert_eq(actual: String, expected: String, label: String) -> TestResult
fn assert_true(value: Int, label: String) -> TestResult
```

---

## Templates

Templates generate deterministic source code using std libraries.

### crud-admin
Full CRUD admin panel with authz guards, db effects, form validation.
Uses: std.ui, std.forms, std.authz, std.db, std.validation

### form-basic
Simple validated form with field binding and submission.
Uses: std.ui, std.forms, std.validation

### auth-app
Authentication flow with role management.
Uses: std.authz, std.http, std.storage

### realtime-feed
Live event feed with polling, rate limiting, notifications.
Uses: std.ui, std.http, std.notifications

### offline-sync
Offline-first app with sync queue and conflict resolution.
Uses: std.sync, std.storage, std.http

---

## Directory Structure

```
libs/
  std-ui/
    package.ax.json
    src/core.ax
  std-forms/
    package.ax.json
    src/core.ax
  std-authz/
    package.ax.json
    src/core.ax
  std-http/
    package.ax.json
    src/core.ax
  std-db/
    package.ax.json
    src/core.ax
  std-sync/
    package.ax.json
    src/core.ax
  std-validation/
    package.ax.json
    src/core.ax
  std-routing/
    package.ax.json
    src/core.ax
  std-storage/
    package.ax.json
    src/core.ax
  std-notifications/
    package.ax.json
    src/core.ax
  std-testing/
    package.ax.json
    src/core.ax

templates/
  crud-admin/
    template.json
    app.ax.template
  form-basic/
    template.json
    app.ax.template
  auth-app/
    template.json
    app.ax.template
  realtime-feed/
    template.json
    app.ax.template
  offline-sync/
    template.json
    app.ax.template
```

## Integration

- Libraries are published to the local package registry via `boruna-pkg publish`
- Apps declare dependencies in `package.ax.json`
- Capability aggregation validates total capability set
- Templates produce patchbundles and run `lang check` post-generation
- All libraries pass determinism verification via `trace2tests`
