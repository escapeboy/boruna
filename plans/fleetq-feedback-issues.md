# FleetQ Feedback — Issue Drafts

Source: implementer feedback letter from FleetQ (Nikola Katsarov, 2026-04-25).
Sprint A (#2) and Sprint B (#1) are already addressed on local branches `feat/fine-grained-policy` and `feat/release-distribution`. The 7 issues below cover the remaining P1/P2 asks.

Existing repo labels: `bug`, `documentation`, `duplicate`, `enhancement`, `good first issue`, `help wanted`, `invalid`, `question`, `wontfix`.

Suggested approach: file all 7 with `enhancement` label. Optionally also create new labels `p1` and `p2` to mark priority — let me know.

---

## Issue 1 — Versioned capability identity from `boruna_capability_list`

**Labels:** `enhancement`

**Body:**

Reported by FleetQ (production integrator). When a team upgrades the Boruna binary, downstream callers cannot detect that capability semantics changed. This blocks aggressive caching: Boruna is deterministic by design, so for `(script_hash, input_hash, policy_hash, binary_capability_hash)` the result is identical — but without a stable identity from the binary, callers can't safely memoize across versions.

### Ask

Have `boruna_capability_list` (or an equivalent `boruna --capability-list --json` CLI subcommand) return a versioned, hashable identity:

```json
{
  "name": "boruna",
  "version": "0.7.1",
  "capabilities": [
    { "name": "net.fetch", "version": "1" },
    { "name": "fs.read",   "version": "1" }
  ],
  "capability_set_hash": "sha256:..."
}
```

Document this as the canonical contract. Once stable, integrators can land result caching that is essentially free at runtime — a meaningful selling point for Boruna ("deterministic AND cached").

### Notes for implementers

- The 10 capabilities live in `boruna_bytecode::Capability::name()`. Per-capability `version` should bump only when the *contract* changes (argument shape, return shape, side-effect semantics), not on every binary release.
- `capability_set_hash` should be a SHA-256 over the canonical (sorted) `(name, version)` tuples so it's stable across builds with the same contract surface.
- Pair this with the `Policy.schema_version` field already in the `Policy` struct so a full cache key becomes `(source_hash, policy_hash, capability_set_hash, policy.schema_version)`.

---

## Issue 2 — Streaming output from `boruna_run`

**Labels:** `enhancement`

**Body:**

Reported by FleetQ. For long-running scripts the calling agent loses visibility — no progress, no early failure detection. Today only the final string blob is surfaced.

### Ask

MCP streaming responses on `boruna_run`, or a separate `boruna_run_stream` tool. Even a periodic `progress` event or `log_line` event would be enough for downstream UIs to show liveness.

### Notes for implementers

- `rmcp` v0.16 supports incremental tool result chunks.
- The simplest first version: emit a `progress` event every N opcodes (e.g. every 100k steps) with `{ steps_executed, max_steps }`. No structural change to the VM needed — `Vm::execute_bounded` already provides the hook.
- A more ambitious version: stream `EventLog` entries (CapCall, ActorSpawn, MessageSend) as they happen, giving the caller a structured live view of what the script is doing.
- Consider a `--stream` flag on the CLI as well so the same machinery is reachable outside MCP.

---

## Issue 3 — Structured resource limits with typed errors

**Labels:** `enhancement`

**Body:**

Reported by FleetQ. Today `boruna_run` accepts `max_steps` (and via the upstream VM, `policy.net_policy.timeout_ms`). A buggy or adversarial `.ax` script can still exhaust memory or fork-bomb the host process before any step or wall-clock limit fires.

### Ask

```jsonc
boruna_run({
  script,
  policy,
  limits: {
    "max_memory_mb":    256,
    "max_wall_ms":      30000,
    "max_syscalls":     100000,
    "max_output_bytes": 1048576
  }
})
```

Returning typed errors:

```json
{ "success": false, "error_kind": "limit_exceeded", "limit": "memory", "actual_mb": 312, "max_mb": 256 }
```

…so callers can surface a clean UX rather than a generic non-zero exit.

### Notes for implementers

- `max_wall_ms` is straightforward via a `tokio::time::timeout` wrapper around the `spawn_blocking` call in `crates/boruna-mcp/src/server.rs`.
- `max_memory_mb` is harder — Rust process-level rlimits (`setrlimit` on Linux) work but are platform-specific. A simpler MVP: ship `max_wall_ms` first and document `max_memory_mb` as platform-best-effort.
- `max_output_bytes` can be enforced at the `format_value` step in `tools/run.rs` by tracking serialized size and aborting once exceeded.
- `max_syscalls` would require a `seccomp` filter on Linux — out of scope for v1.

---

## Issue 4 — Stable, documented `boruna_validate` response schema

**Labels:** `enhancement`, `documentation`

**Body:**

Reported by FleetQ. Integrators want to call the validation tool from skill-creation forms so users see syntax / capability errors **on save**, not on first run. Today the response shape isn't formally documented, so they're afraid of coupling their UI to it.

### Ask

Lock down and document:

```jsonc
{
  "ok": false,
  "protocol_version": 1,
  "diagnostics": [
    { "level": "error", "line": 12, "col": 4, "code": "E0451", "message": "..." }
  ]
}
```

Even a versioned `protocol_version: 1` field on the response would be enough to commit to it.

### Notes for implementers

- The diagnostics layer already exists in `boruna-tooling::diagnostics` with structured spans, severity, and suggested patches.
- This issue is mainly: (a) add `protocol_version: 1` to the JSON output of `boruna_check` and any "validate" tool, (b) document the shape in `docs/reference/mcp-tools.md` (new file) or extend `docs/DIAGNOSTICS_AND_REPAIR.md`.
- Pair this with the `policy.schema_version` from Sprint A for a uniform versioning story across the MCP surface.

---

## Issue 5 — Record/replay for `net.fetch`

**Labels:** `enhancement`

**Body:**

Reported by FleetQ as a P2 power feature. Determinism + external HTTP are fundamentally at odds. Boruna is one of the few runtimes where record/replay would be **ergonomic**, because the capability gate already has the call site.

### Ask

Optional recording mode dumps `(request, response)` pairs to a sidecar file; replay mode reads from it instead of making real calls. This makes agent loops genuinely reproducible — a strong, distinctive selling point.

### Notes for implementers

- The capability gateway (`crates/llmvm/src/capability_gateway.rs`) already records every call to the `EventLog`; the `ReplayHandler` already exists for replaying recorded outputs.
- The gap: `EventLog` only stores result `Value`s, not request shapes. A net-fetch-specific recorder would persist `(method, url, headers, body)` → `(status, headers, body)` to a JSON sidecar.
- The existing `crates/llmvm/src/http_handler.rs` (`http` feature) is the right place to insert a `RecordingHandler` wrapper.
- CLI flag suggestion: `--record-net-to <file>` and `--replay-net-from <file>`. Mutually exclusive with `--live`.

---

## Issue 6 — Output schema validation as a first-class gate

**Labels:** `enhancement`

**Body:**

Reported by FleetQ as a P2 power feature. Allow scripts to declare an output JSON Schema; Boruna validates the return value before yielding it. Saves every integrator from re-implementing this in their host language.

### Ask

```ax
fn main() -> Map<String, String> !{out_schema=schema.json} {
    ...
}
```

Or, less invasive: a `boruna_run` parameter `output_schema: object` that runs the validator after execution and returns a structured `validation_failed` error if the result doesn't match.

### Notes for implementers

- The simpler MCP-parameter form does not require any compiler change.
- Use `jsonschema` crate (Rust) for validation; behavior matches the JSON Schema 2020-12 spec used in `docs/reference/policy.schema.json`.
- The compiler-annotation form is a larger language design question — defer until the runtime mechanism exists.

---

## Issue 7 — Per-call observability hooks (OpenTelemetry)

**Labels:** `enhancement`

**Body:**

Reported by FleetQ as a P2 power feature. A `--telemetry-otlp <endpoint>` flag (or env var) emitting OpenTelemetry spans for each capability call (`net.fetch`, `fs.read`, …) with timing/byte counts.

### Ask

Per-capability OpenTelemetry spans, exportable to any OTLP collector. FleetQ already runs a tenant-scoped collector and would surface Boruna's resource usage right next to LLM/agent spans in their UI without writing glue code.

### Notes for implementers

- The `opentelemetry` and `opentelemetry-otlp` crates are stable. Wire them at the `CapabilityGateway::call` boundary in `crates/llmvm/src/capability_gateway.rs`.
- Suggested span names: `boruna.cap.<name>` (e.g. `boruna.cap.net.fetch`).
- Suggested attributes: `cap.name`, `cap.budget_remaining`, `bytes_in`, `bytes_out`, `error`.
- Activation: env var `OTEL_EXPORTER_OTLP_ENDPOINT` (the OpenTelemetry standard) — no Boruna-specific flag needed.
- Make this a Cargo feature (`telemetry`) so the dependency is opt-in for integrators who don't need it.

---

## Filing plan

When you OK it, I'll run:

```bash
gh issue create --repo escapeboy/boruna --title "..." --label enhancement --body-file <each-body>
```

…seven times. I'll capture the resulting URLs and post them back in this conversation.
