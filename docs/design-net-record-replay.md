# Design: Record/Replay for `net.fetch`

**Sprint:** `0.5-S7` · **Issue:** [#7](https://github.com/escapeboy/boruna/issues/7) · **Status:** Think

## Who

Production integrators (canonical: FleetQ) running agent loops where the same script is invoked repeatedly during testing or CI but the underlying HTTP responses must be **deterministic across runs**. Today every run hits the network — flaky tests, rate-limit charges, and no reproducibility for debugging a regression that depends on a specific upstream response.

Also: anyone debugging a failed agent run from production logs who wants to replay the *exact* sequence of upstream responses against a local debugger without touching the production endpoint.

## What they're doing today

Mocking HTTP at the host-language layer (PHP/TS/Python `http_mock` etc.), maintaining brittle fixture files outside Boruna, or living with non-deterministic test failures. The host-language mocks don't compose with Boruna's `--live` mode at all — once you're inside the binary, you're hitting the network for real or you're not.

## MVP someone would pay for

```bash
# Record once against the real endpoint:
boruna run app.ax --policy allow-all --live --record-net-to fixtures/run-001.tape.json

# Replay forever after, with no network access:
boruna run app.ax --policy allow-all --replay-net-from fixtures/run-001.tape.json
```

The tape file is a single JSON document (sidecar):

```json
{
  "format_version": 1,
  "transactions": [
    {
      "method": "GET",
      "url": "https://api.example.com/users/42",
      "request_body": null,
      "response_body": "{\"id\": 42, \"name\": \"Alice\"}"
    },
    {
      "method": "POST",
      "url": "https://api.example.com/events",
      "request_body": "{\"event\": \"click\"}",
      "response_body": "{\"ok\": true}"
    }
  ]
}
```

Replay is **strict ordered**: the Nth `net.fetch` call must match the Nth tape entry by `(method, url, request_body)`. Mismatch returns a clear error (mismatch position + which field differs). Exhaustion returns a clear error (script asked for more calls than the tape has).

## What would make someone say "whoa"

> "I can record once against the real upstream, replay forever in CI without touching the network or paying rate limits, AND the replay is byte-identical to the original — including the agent's downstream branching behavior. My agent integration test takes 200 ms instead of 8 seconds."

That's the win. Pairs with `EventLog`-based replay (already shipped) — together they cover both VM-side determinism and external-side determinism.

## How this compounds

1. **Agent CI loops become deterministic.** Record one canonical run; replay across CI matrix. Same input → same output, no flakes.
2. **Bug repros become shippable.** "Here's the tape file from production, run `boruna run app.ax --replay-net-from prod.tape` to reproduce."
3. **Cost reduction.** Replay = $0. Production tapes can be promoted to test fixtures.
4. **Sets the pattern for other side-effecting capabilities.** `db.query` record/replay is the next obvious extension. `llm.call` (when the live handler ships) is the highest-leverage extension — LLM outputs are the most expensive non-deterministic side effect Boruna will ever face.
5. **Distinctive selling point.** Per FleetQ: "one of the few runtimes where record/replay would be ergonomic." The capability gateway already gives us the call site; the hard part is just persistence.

## CLI surface

| Flag | Meaning | Requires `--live`? | Mutually exclusive with |
|---|---|---|---|
| `--record-net-to <FILE>` | Make real HTTP calls; persist `(request, response)` pairs to FILE on exit | **Yes** (recording requires real calls to record) | `--replay-net-from` |
| `--replay-net-from <FILE>` | No real HTTP; serve responses from FILE in order | No (replay doesn't need live) | `--record-net-to` |

If both `--live` and `--replay-net-from` are specified, replay wins and `--live` is ignored (with a stderr warning) — replay is more specific.

The flags only affect `net.fetch`. Other capabilities (db.query, fs.read, etc.) are unaffected; they continue to use whatever handler the gateway is wired to.

## Match strategy: strict ordered, key on (method, url, body)

- **Why ordered:** Boruna scripts are deterministic by design. The nth call from the same script with the same inputs and the same policy will always be the nth call — there's no scheduler nondeterminism. So order-of-recording = order-of-replay.
- **Why these three fields:** `method` and `url` are the request identity. `request_body` covers POST/PUT/PATCH bodies — different bodies are different requests.
- **Why NOT headers:** auth tokens, user agents, content-length all change between recording and replay sessions. Including them in the match key creates false mismatches. Documented as a known limitation; future sprint adds opt-in header matching if asked.
- **On mismatch:** return a typed error on the next `net.fetch` call:
  ```
  net.fetch replay mismatch at position 3:
    expected: GET https://api.example.com/v1/users
    actual:   POST https://api.example.com/v1/users
    request_body differs: expected None, got Some(<14 bytes>)
  ```
- **On exhaustion:** return a typed error:
  ```
  net.fetch replay tape exhausted (5 transactions consumed, script asked for more)
  ```
- **On under-consumption (script makes fewer calls than tape has):** silently OK. The tape can have trailing transactions that the script didn't reach this run; those are unused.

## Tape file format

```jsonc
{
  "format_version": 1,         // bumped on breaking shape changes (additive ones keep version)
  "transactions": [
    {
      "method":         "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD",
      "url":            string,
      "request_body":   string | null,        // null when no body sent
      "response_body":  string                // the body string the handler returned
    }
  ]
}
```

The file is a **complete document** (not append-only NDJSON) — small enough that loading the whole thing on replay is trivial, and the JSON-array form is the natural fit for the strict-ordered match.

`response_body` is what the existing `HttpHandler::handle_net_fetch` returns today: a `String` (the response body, decoded as UTF-8). We don't record `(status, headers)` because the handler doesn't currently return them — adding that is a separate concern.

## What's recorded and what's not

| | Recorded? |
|---|---|
| Method, URL, request body | **Yes** |
| Response body (string) | **Yes** |
| Request headers (auth tokens, user-agent) | **No** — too noisy, false mismatches |
| Response headers, status code | **No** — handler returns body only today |
| Timing / latency | **No** — replay is instant; not a use case |
| Errors (network failure, 4xx/5xx) | **As error_kind on tape entry** in a future sprint; v1 only records successful responses |

If `--record-net-to` is set and a real HTTP call fails (network error, 4xx/5xx), the failure is propagated to the script as today (the script sees the error). The tape file does NOT record the failed transaction — re-recording is the user's responsibility. This is the simplest possible MVP; documented limitation.

## Determinism contract (per ADR 001)

- The **tape file itself** is deterministic given the same recording session: identical `transactions` array in identical order.
- A **replay** is deterministic by construction: every `net.fetch` returns the same `response_body` from the tape that the original recording put there.
- A **mismatch error** during replay is deterministic: the same script + same tape + same mismatch position always produces the same error message.

The only non-determinism is the **recording session itself** — which is fine because that's where the wall-clock-keyed network call happens. Replay is replay.

## Where it lives

- New file `crates/llmvm/src/net_record_replay.rs` (feature-gated under `http`, same as `http_handler.rs`).
- Defines `NetTransaction`, `NetTape`, `RecordingHttpHandler`, `ReplayingHttpHandler`.
- `RecordingHttpHandler` wraps `HttpHandler` (records every call, flushes on `save_tape`).
- `ReplayingHttpHandler` doesn't need `HttpHandler` at all — it just serves from the tape, falls back to `MockHandler` for non-`net.fetch` capabilities.
- New CLI flags on `boruna run` in `crates/llmvm-cli/src/main.rs`.

## Out of scope for v1

- **`boruna workflow run --record-net-to / --replay-net-from`** — same machinery applies, just plumbed through workflow runner. Defer to follow-up sprint.
- **`boruna_run` MCP parameter for replay** — adds a `replay_net: <tape JSON>` field. Defer; CLI is the primary use case for v1.
- **Recording response headers + status** — requires changes to `HttpHandler::handle_net_fetch` return type. Defer.
- **Recording failed transactions** — requires teaching the tape format about errors. Defer.
- **Header-aware matching** — opt-in `match_headers: ["x-tenant-id"]` config. Defer until asked.
- **`db.query` / `llm.call` record/replay** — each is its own sprint. The pattern (RecordingHandler wrapper + sidecar tape file) generalizes; this sprint establishes it.
- **Tape compaction / multi-script tapes** — out of scope; one tape per run.

## Acceptance criteria

1. New types `NetTransaction`, `NetTape`, `RecordingHttpHandler`, `ReplayingHttpHandler` in `crates/llmvm/src/net_record_replay.rs` (feature `http`).
2. Tape file format documented and stable: `{ format_version: 1, transactions: [...] }`.
3. `RecordingHttpHandler::save_tape(path)` writes a tape file from the in-memory transactions.
4. `ReplayingHttpHandler` consumes transactions in order; returns the recorded `response_body` for matching calls.
5. **Strict ordered matching on `(method, url, request_body)`** — a mismatch at position N returns a clear error naming the position and the differing field(s).
6. **Tape exhaustion** — returns a clear error.
7. **Tape under-consumption** — succeeds silently (trailing transactions unused).
8. New CLI flags `--record-net-to` and `--replay-net-from` on `boruna run`.
9. Mutually exclusive flag handling at parse time (clap level).
10. Integration test: record a tape against a `MockHandler`-backed setup, replay it from disk, verify byte-identical responses.
11. Test coverage: round-trip serde of tape, mismatch error, exhaustion error, under-consumption OK, position counter correctness.
12. CHANGELOG entry. Design doc (this file).
