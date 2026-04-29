# std-db

> Typed database query helpers

**Package:** `std.db`  **Version:** `0.1.0`  **Capabilities required:** `db.query`

## Overview

`std-db` provides a builder API for constructing typed database queries and converting them to `Effect` values for execution. Build a `Query` with the CRUD helpers, refine it with `db_where`, `db_order`, and `db_paginate`, then call `db_to_effect` to hand it off to the runtime. The library also includes a `Pagination` type and helpers for navigating paged result sets.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.db": "0.1.0"
```

Your policy must grant `db.query`.

## API Reference

### Types

#### `Effect`

```
type Effect { kind: String, payload: String, callback_tag: String }
```

#### `Query`

```
type Query {
    operation: String,
    table: String,
    columns: String,
    conditions: String,
    order_by: String,
    limit_val: Int,
    offset_val: Int
}
```

A portable query descriptor. `columns` is a comma-separated list; `conditions` is a raw filter expression string.

#### `Pagination`

```
type Pagination { page: Int, per_page: Int, total: Int, total_pages: Int }
```

Computed pagination metadata. Use with `pagination_has_next` / `pagination_has_prev` in your view.

### Functions

#### Query builders

##### `db_select(table: String, columns: String) -> Query`

Creates a SELECT query for the specified columns.

**Example**
```
fn main() -> Int {
  let q: Query = db_select("users", "id, name, email")
  let q2: Query = db_where(q, "active = 1")
  let q3: Query = db_order(q2, "name")
  let q4: Query = db_paginate(q3, 1, 20)
  let eff: Effect = db_to_effect(q4, "users_loaded")
  0
}
```

##### `db_insert(table: String, columns: String, values: String) -> Query`

Creates an INSERT query. `columns` and `values` are comma-separated strings corresponding to each other positionally.

##### `db_update(table: String, sets: String, conditions: String) -> Query`

Creates an UPDATE query. `sets` is a comma-separated list of `column = value` assignments.

##### `db_delete(table: String, conditions: String) -> Query`

Creates a DELETE query filtered by `conditions`.

#### Query modifiers

##### `db_where(query: Query, condition: String) -> Query`

Replaces the query's condition expression. Call after the initial builder.

##### `db_order(query: Query, column: String) -> Query`

Sets the ORDER BY column.

##### `db_limit(query: Query, limit_val: Int) -> Query`

Sets a row limit.

##### `db_offset(query: Query, offset_val: Int) -> Query`

Sets a row offset.

##### `db_paginate(query: Query, page: Int, per_page: Int) -> Query`

Sets `limit_val` and `offset_val` from a 1-based page number and page size. Equivalent to calling `db_limit` and `db_offset` with the computed values.

#### Effect dispatch

##### `db_to_effect(query: Query, callback_tag: String) -> Effect`

Converts a `Query` to an `Effect` with `kind: "db_query"`. Return this from `update` to execute the query.

#### Pagination helpers

##### `pagination_info(page: Int, per_page: Int, total: Int) -> Pagination`

Computes the `Pagination` record from a known total row count.

##### `pagination_has_next(p: Pagination) -> Int`

Returns `1` if there is a next page.

##### `pagination_has_prev(p: Pagination) -> Int`

Returns `1` if there is a previous page.

##### `pagination_next_page(p: Pagination) -> Int`

Returns the next page number, or the current page if already at the last.

##### `pagination_prev_page(p: Pagination) -> Int`

Returns the previous page number, or `1` if already at the first.

## Capabilities

Requires `db.query`. Queries are executed by the runtime's capability handler; the library itself produces only data structures.

## Notes / Limitations

- Condition strings (`conditions`, `sets`) are passed through verbatim to the runtime. The library does not perform sanitization; callers are responsible for safe parameterization.
- `db_paginate` uses 1-based page numbers. Page `0` or negative values produce a negative offset.
- `total_pages` is computed as `ceil(total / per_page)`; when `total == 0` the result is `1`.
