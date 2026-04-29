# std-http

> Safe typed HTTP effect wrappers

**Package:** `std.http`  **Version:** `0.1.0`  **Capabilities required:** `net.fetch`

## Overview

`std-http` wraps outbound HTTP calls as typed `Effect` values. Your `update` handler returns these effects; the Boruna runtime dispatches them through the `net.fetch` capability and delivers responses back via the named `callback_tag`. This keeps network I/O out of pure logic and makes every request auditable in the evidence bundle. The library also provides retry configuration helpers and status-code predicates.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.http": "0.1.0"
```

Your workflow or app policy must grant `net.fetch` to the step that uses this library.

## API Reference

### Types

#### `Effect`

```
type Effect { kind: String, payload: String, callback_tag: String }
```

Returned by all request-building functions. Pass it back from `update` to trigger the HTTP call.

#### `HttpRequest`

```
type HttpRequest { method: String, url: String, body: String, content_type: String }
```

A fully described request for use with `http_request`.

#### `HttpResponse`

```
type HttpResponse { status: Int, body: String }
```

Shape of the response delivered to the `callback_tag` handler.

#### `RetryConfig`

```
type RetryConfig { max_retries: Int, backoff_ms: Int, multiplier: Int }
```

Exponential backoff parameters used with `http_next_backoff`.

### Functions

#### Request builders

##### `http_get(url: String, callback_tag: String) -> Effect`

Produces a GET request effect.

**Example**
```
fn main() -> Int {
  let eff: Effect = http_get("https://api.example.com/items", "items_loaded")
  0
}
```

##### `http_post(url: String, body: String, callback_tag: String) -> Effect`

Produces a POST request effect with the given body.

##### `http_put(url: String, body: String, callback_tag: String) -> Effect`

Produces a PUT request effect.

##### `http_delete(url: String, callback_tag: String) -> Effect`

Produces a DELETE request effect.

##### `http_request(req: HttpRequest, callback_tag: String) -> Effect`

Produces an effect from a fully specified `HttpRequest` — use when you need to set `content_type` or other fields explicitly.

#### Retry helpers

##### `http_default_retry() -> RetryConfig`

Returns a sensible default: 3 retries, 1000 ms base backoff, multiplier 2.

##### `http_retry_config(max_retries: Int, backoff_ms: Int, multiplier: Int) -> RetryConfig`

Constructs a custom retry configuration.

##### `http_next_backoff(config: RetryConfig, attempt: Int) -> Int`

Returns the wait duration in milliseconds before the next retry for a given `attempt` index (0-based). Uses fixed exponential steps up to `attempt == 2`; subsequent attempts cap at `backoff_ms * multiplier^2`.

#### Status-code predicates

##### `http_is_success(status: Int) -> Int`

Returns `1` for 2xx status codes.

##### `http_is_error(status: Int) -> Int`

Returns `1` for 4xx and 5xx status codes.

##### `http_should_retry(status: Int) -> Int`

Returns `1` for status `429` (Too Many Requests) or any 5xx — the conditions under which retrying is appropriate.

##### `http_parse_status(payload: String) -> Int`

Parses a status code string to `Int`. Returns `0` if parsing fails.

## Capabilities

Requires `net.fetch`. The VM's `CapabilityGateway` enforces this at runtime; the call is rejected if the active policy does not include `net.fetch`.

## Notes / Limitations

- All functions produce `Effect` values — they do not perform any I/O themselves. Actual network calls happen in the runtime after the `update` function returns.
- `http_next_backoff` implements three-step exponential backoff only. If `attempt >= 3` the backoff is not further increased in the current implementation.
- `http_parse_status` is a stub that returns `0`; the runtime is expected to set the status field directly on the response record delivered to the callback.
