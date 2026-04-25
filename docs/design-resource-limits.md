# Design: Structured Resource Limits

**Sprint:** `0.3-S10` · **Issue:** [#5](https://github.com/escapeboy/boruna/issues/5) · **Status:** Think

## Who

Production integrators (canonical: FleetQ) embedding `boruna_run` in a multi-tenant request path. Today they wrap our binary in a PHP-side timeout shim because we have no internal wall-clock limit. A buggy or adversarial `.ax` script can still exhaust memory or burn CPU before the PHP timeout fires — ugly UX and no typed signal for the integrator's UI.

## What they're doing today

- Wrap the binary call in a `set_time_limit(N)` / `proc_open()`-with-timeout from PHP. SIGKILL on overrun → exit status, no structured error.
- No way to set per-tenant output size limits — a tenant generating a 50 MB output blob slows their whole server's response queue.

## MVP someone would pay for

```json
boruna_run({
  source: "...",
  policy: ...,
  limits: {
    "max_wall_ms":      30000,
    "max_output_bytes": 1048576
  }
})
```

Returns on overrun:

```json
{
  "success": false,
  "protocol_version": 1,
  "error_kind": "limit_exceeded",
  "limit_kind": "wall_ms" | "output_bytes",
  "limit": 30000,
  "message": "wall-clock limit of 30000 ms exceeded"
}
```

Two limits cover the actual pain. Skip what's harder than it's worth in v1.

## What would make someone say "whoa"

> "Wait — the typed `error_kind: 'limit_exceeded'` with `limit_kind` discriminator means I can write one `catch` branch per limit, surface the right user-facing message ('your script took too long' vs. 'your script returned too much data'), and bill differently per limit class — without parsing error strings."

That's the win. Every other workflow runtime returns "process killed" with no machine-readable detail.

## How this compounds

1. Sets the **error_kind shape pattern** (typed + discriminator) for every future `boruna_*` tool that needs limit enforcement (`boruna_workflow_run` next; `boruna_framework_test` after).
2. Pairs with the existing `policy.rules.<cap>.budget` (per-cap call quotas) — together they form the **complete sandbox surface**: capability quotas + execution time + output size.
3. The `limit_kind` discriminator extends without bumping `protocol_version` — adding `memory_mb`, `syscalls` later is purely additive.
4. Removes a class of "boruna ate my server" support tickets for every integrator.

## Out of scope for v1

- **`max_memory_mb`** — Rust process-level rlimits (`setrlimit(RLIMIT_AS)`) are Linux-specific and have surprising failure modes (allocator metadata, JIT pages). Documented in the `limits` schema as `"reserved for a future release; not enforced in 0.3.x"`. Integrators continue to use process-level cgroups/ulimits at their orchestration layer.
- **`max_syscalls`** — would require a `seccomp` filter; out of scope. Capability gating already covers the "no surprise filesystem/network calls" need.
- **Per-step wall-clock budgets** — orthogonal concern; belongs to `0.3-S6` (workflow step timeouts).
- **`boruna_run --limits` CLI flag** — MCP only for v1. CLI flag follows in a 0.2.x patch if asked.

## Determinism trade-off (explicit, per ADR 001 lesson)

`max_wall_ms` is **wall-clock-keyed**. A script that completes within the limit produces identical output on every machine — the deterministic guarantee holds. A script that hits the limit may complete on a fast machine and time out on a slow one — the **failure path is non-deterministic by construction**. This is the same trade-off `max_steps` already has (step counts are deterministic, but a different host could be faster/slower at reaching them in wall time).

**Recommendation in docs:** use `max_steps` (deterministic) as the reproducibility guardrail; use `max_wall_ms` as the operational ceiling against runaway scripts. Don't depend on `max_wall_ms` for replay.

## Acceptance criteria

1. `boruna_run` accepts a `limits: { max_wall_ms?, max_output_bytes?, max_memory_mb? }` parameter; all sub-fields optional.
2. `max_wall_ms` enforced inside the VM execute loop, checked every 1024 steps. Wall-clock measurement uses `std::time::Instant` (not `chrono::Utc::now()` — the latter is replay-tainted per ADR 001).
3. `max_output_bytes` enforced during result serialization in `tools/run.rs::format_value`. Cumulative tracking; abort once exceeded.
4. New `error_kind: "limit_exceeded"` with `limit_kind` discriminator (one of `"wall_ms"`, `"output_bytes"`) and `limit` (the configured value) and `message` (human-readable).
5. `max_memory_mb` accepted in the schema but ignored at runtime; documented as best-effort/deferred.
6. Adding `limits` is **additive** — keeps `protocol_version: 1`, doesn't change the existing success or `error_kind: "runtime_error"` shapes.
7. Test coverage: each limit hit, no-limits-set (default), both-set-but-not-hit (default), unrelated-VmError-still-runtime_error.
8. `docs/reference/mcp-server.md` updated with the `limits` parameter and the new error shape.
