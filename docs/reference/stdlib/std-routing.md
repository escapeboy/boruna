# std-routing

> Declarative routing model

**Package:** `std.routing`  **Version:** `0.1.0`  **Capabilities required:** none

## Overview

`std-routing` gives framework apps a lightweight, deterministic routing layer. Routes are declared as `Route` records, and matching is done with pure functions — no parsing magic, no global history object. Define your routes once, call `route_match_first` in your `update` handler when the path changes, and use `route_is_active` in your `view` to highlight the current link.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.routing": "0.1.0"
```

## API Reference

### Types

#### `Route`

```
type Route { name: String, path: String, param_count: Int }
```

- `name` — a stable identifier used throughout the app (e.g. `"home"`, `"users"`)
- `path` — the URL path string to match against (e.g. `"/"`, `"/users"`)
- `param_count` — number of dynamic parameters in the path (0 for static routes)

#### `RouteMatch`

```
type RouteMatch { matched: Int, route_name: String, param1: String, param2: String }
```

- `matched` — `1` if a route matched, `0` otherwise
- `route_name` — the name of the matched route, `""` on no match
- `param1` / `param2` — extracted path parameters (up to two)

### Functions

##### `route_define(name: String, path: String) -> Route`

Declares a static route with no dynamic parameters.

**Example**
```
fn main() -> Int {
  let home: Route = route_define("home", "/")
  let users: Route = route_define("users", "/users")
  let settings: Route = route_define("settings", "/settings")
  let m: RouteMatch = route_match_first(home, users, settings, "/users")
  m.matched
}
```

##### `route_define_with_param(name: String, path: String, param_count: Int) -> Route`

Declares a route that expects `param_count` dynamic path segments.

##### `route_match_path(route: Route, path: String) -> RouteMatch`

Checks a single route against `path`. Returns a no-match result if `route.path != path`.

##### `route_match_first(r1: Route, r2: Route, r3: Route, path: String) -> RouteMatch`

Tries `r1`, `r2`, and `r3` in order and returns the first match. Returns the result of `r3` (which may be a no-match) if none of the first two matched. Use as a three-entry router table.

##### `route_no_match() -> RouteMatch`

Returns a zero-value no-match result. Use as a fallback or default.

##### `route_navigate(route_name: String) -> String`

Returns the route name as a navigation token. The framework dispatches this to the runtime to push the new path.

##### `route_navigate_with_param(route_name: String, param: String) -> String`

Like `route_navigate` but carries a single parameter.

##### `route_is_active(current_route: String, route_name: String) -> Int`

Returns `1` if `current_route == route_name`. Use in `view` to highlight active navigation links.

## Capabilities

None. All functions are pure.

## Notes / Limitations

- `route_match_path` uses exact string equality. Dynamic path segments (e.g. `/users/42`) require the calling app to extract parameters before matching, or use `param_count` as a hint to implement custom segment splitting.
- `route_match_first` handles exactly three routes. For larger route tables, nest calls or use a series of `route_match_path` checks in your `update` handler.
- `route_navigate` returns the route name string as a navigation token; the actual URL push is performed by the runtime, not the library.
