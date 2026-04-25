# Design: Per-call OpenTelemetry Observability

**Sprint:** `0.4-S5` · **Issue:** [#9](https://github.com/escapeboy/boruna/issues/9) · **Status:** Think

## Who

Production integrators (canonical: FleetQ) running Boruna inside an existing OpenTelemetry stack — they already operate a tenant-scoped OTLP collector for their LLM/agent spans and want Boruna's per-capability resource usage to surface right next to those spans without writing glue code.

Also: anyone debugging a slow agent run who wants per-capability timing breakdown (which `net.fetch` took 7s of the 10s wall time?).

## What they're doing today

Wrapping `boruna run` in their host language's tracing instrumentation, which only sees "Boruna started/ended" — no per-capability detail. Resource attribution to specific capabilities (LLM call cost, HTTP latency, DB query count) is invisible.

## MVP someone would pay for

Set the OpenTelemetry standard env var, run Boruna, observe spans:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://collector.internal:4318
export OTEL_SERVICE_NAME=my-agent
boruna run app.ax --policy allow-all --live
```

Each capability call emits one span:

```
boruna.cap.net.fetch    {cap.name="net.fetch", bytes_in=247, bytes_out=1834}    142ms
boruna.cap.llm.call     {cap.name="llm.call", bytes_in=890, bytes_out=2103}     7.4s
boruna.cap.fs.read      {cap.name="fs.read",  bytes_in=64,  bytes_out=12834}    8ms
```

If `OTEL_EXPORTER_OTLP_ENDPOINT` is unset, Boruna runs exactly as today — zero overhead, no allocations beyond what the existing `tracing` macros do (which is essentially zero when no subscriber is installed).

## What would make someone say "whoa"

> "I exported one env var and now Boruna's per-capability timing shows up in my OTel dashboard alongside everything else, with zero glue code, AND the existing CLI invocation works unchanged."

That's the win. The OTel standard means every observability tool in the ecosystem (Grafana, Honeycomb, Datadog, your homegrown Jaeger, etc.) accepts the spans without further config.

## How this compounds

1. **Bills the right capability.** When an agent run is slow, the bottleneck is visible per-capability — no more "Boruna took 10s" without explanation.
2. **Pairs with the just-shipped `limits`** (`0.3-S10`). When a wall-time limit fires, the trace shows exactly which capability burned the time.
3. **Pairs with `net.fetch` record/replay** (`0.5-S7`). Recorded and replayed runs both emit identical span shapes — telemetry is replay-stable.
4. **Sets the pattern** for future per-capability instrumentation. When `db.query` and `llm.call` get their own handlers, the same span machinery covers them automatically.
5. **Distinguishing for adoption.** "OTel-native, zero glue" is a real adoption advantage over runtimes that require per-tool integration code.

## Architecture

### Two-layer split

- **`tracing` crate (always-on, non-optional dep).** `CapabilityGateway::call` always emits a `tracing::info_span!` per capability call. When no subscriber is installed (the default), this is essentially a no-op (single atomic check + a few stack-allocated structs).
- **`telemetry` Cargo feature (opt-in).** Adds the OpenTelemetry SDK + OTLP exporter + a `boruna_vm::telemetry::init()` helper that wires the OTel pipeline into `tracing`'s subscriber registry.

This split means:
- Library consumers who want their own subscriber (e.g. `tracing-subscriber` for stderr logs) can use the spans without paying for OTel.
- Integrators who want OTel-out-of-the-box enable the feature and set the env var.
- Users who want zero observability pay zero overhead at runtime AND zero binary-size cost (when feature off).

### Span shape (locked v1)

| Field | Value | Notes |
|---|---|---|
| Name | `boruna.cap.<name>` | e.g. `boruna.cap.net.fetch`, `boruna.cap.llm.call` |
| Level | `INFO` | Below WARN/ERROR for runtime issues; integrators can filter |
| `cap.name` (string) | The capability name | Same as the name suffix; redundant but cleaner attribute querying |
| `bytes_in` (u64) | Sum of UTF-8 byte length of String args | Approximation of "how big was the request" |
| `bytes_out` (u64) | UTF-8 byte length of returned String, or 0 | Approximation of "how big was the response" |
| `cap.budget_remaining` (u64 or "unlimited") | If a budget rule is set: budget − usage; else absent or "unlimited" | Helps debug "why did this fail?" with `0` budget remaining |
| `error.kind` (string, on error path only) | `denied` / `budget_exceeded` / `runtime_error` | Set via `Span::record` if the call errors |

Duration is implicit (recorded by the SDK from span open/close).

### What is NOT in spans (per ADR 001 determinism contract)

- Wall-clock timestamps in attributes (the SDK records its own; we don't put them in our hash chain)
- PIDs, hostnames, process IDs (operational metadata, not contract state)
- Capability *args* themselves (these can be huge, secret, or tenant-private; integrators who want this can write their own subscriber)
- `EventLog` content (separate concern; spans are operational, EventLog is replay-verified)

The decision to NOT include args means span attributes are safe to ship to a multi-tenant collector without leaking customer data.

### Activation

- **Env var:** `OTEL_EXPORTER_OTLP_ENDPOINT` is the OpenTelemetry standard. We honor it. Empty string or unset → no OTel, but spans still emit (consumed by any other subscriber the user installs, or dropped silently).
- **CLI:** the binary's `main` (in `boruna-cli`) calls `boruna_vm::telemetry::init()` at startup if the `telemetry` feature is on. The init function reads the env var and either sets up the OTel pipeline OR returns a no-op handle.
- **No CLI flag** — env var is the single signal, matching the issue's recommendation. `OTEL_EXPORTER_OTLP_ENDPOINT` is a well-known variable; integrators expect it.

## Library choices

| Need | Crate | Version (lockstep) |
|---|---|---|
| Tracing facade | `tracing` | `0.1` |
| OTel SDK | `opentelemetry` | `0.27` (or whatever resolves cleanly with the bridge) |
| OTLP exporter | `opentelemetry-otlp` | matching `0.27` |
| `tracing` ↔ OTel bridge | `tracing-opentelemetry` | matching SDK version |
| Subscriber init | `tracing-subscriber` | `0.3` |

The OTel Rust ecosystem versions move in lockstep — picking a coherent set is the only real complexity. Lock to whatever resolves cleanly today; document the version pin.

If versions don't resolve cleanly under our existing transitive deps (`reqwest`, `tonic`, `prost`, etc.), fall back to a smaller scope: emit `tracing` spans only, and document the OTel bridge as the user's responsibility (they install their own `tracing-opentelemetry` subscriber). That still satisfies the issue (per-call observability) — it just shifts the SDK plumbing onto the integrator. Clearly mark which path was taken in the PR description.

## Where it lives

- `crates/llmvm/src/capability_gateway.rs` — wrap `call()` in a span (always; tracing dep is non-optional).
- `crates/llmvm/src/telemetry.rs` (new file, feature `telemetry`) — `init() -> TelemetryHandle` + Drop-flush.
- `crates/llmvm-cli/src/main.rs` — call `boruna_vm::telemetry::init()` at startup if feature on; hold the handle for the binary's lifetime.

## Out of scope for v1

- **CLI `--telemetry` flag.** Env var is the activation signal per OTel convention.
- **Span context propagation in/out of Boruna.** Boruna doesn't accept incoming traceparents today (no CLI/MCP surface for it). Future sprint when there's an ask.
- **Metrics (counters, histograms).** OTel spans are the v1 surface. `--metrics-addr :9090` is a separate sprint (`0.4-S4`).
- **Logs export.** `tracing` events at WARN/ERROR could ship over OTLP-logs, but this sprint focuses on spans only.
- **Args / capability payloads in attributes.** Privacy/size concerns; integrators can write a custom subscriber if needed.
- **`boruna-mcp` / `boruna-orch` binaries getting telemetry.** Same env var works once the `telemetry` feature is enabled in those crates' deps; defer until asked.

## Determinism contract (per ADR 001)

Spans are **operational metadata**. They never feed an `EventLog`, `AuditLog`, or `EvidenceBundle`. Their content (durations, byte counts derived from args/results) is not part of any replay-verified hash. A replayed run produces identical replay state but may produce different span durations on a faster/slower host — by design.

Hard invariant for downstream sprints: **never wire a span attribute or duration into an audit-hash chain.** Documented loudly in `boruna_vm::telemetry::init()`'s doc comment.

## Acceptance criteria

1. `tracing` is a non-optional dep of `boruna-vm`. `CapabilityGateway::call` always emits `boruna.cap.<name>` spans with attributes per the shape table above.
2. New `telemetry` Cargo feature in `boruna-vm` adds the OTel SDK deps + `init()` helper.
3. `init()` reads `OTEL_EXPORTER_OTLP_ENDPOINT`; returns no-op handle when unset, OTel pipeline when set.
4. `init()` returns a `TelemetryHandle` whose `Drop` flushes pending spans.
5. With `--features telemetry` and the env var unset → CLI behaves identically to today (no OTel allocations, no panics).
6. With `--features telemetry` and the env var set → CLI emits OTLP spans for each capability call.
7. Without `--features telemetry` → CLI compiles and runs as today; tracing spans still emit but go nowhere (zero subscriber).
8. Spans never include capability args or `EventLog` content.
9. Tests: span emission via test exporter (with feature on), no-op when env var unset, no-op when feature off.
10. `docs/design-otel.md` (this file). CHANGELOG entry. PR description names the version-pin set.
11. **Fallback acceptable:** if OTel deps don't resolve cleanly with our transitive deps, ship `tracing`-only spans + document BYO subscriber as the integration path. PR description must clearly state which path landed.
