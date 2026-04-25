# Design: Versioned Capability Identity

**Sprint:** `0.3-S11`
**Issue:** [escapeboy/boruna#3](https://github.com/escapeboy/boruna/issues/3)
**Date:** 2026-04-25
**Status:** Think

## Who needs this

**Production integrators** who embed Boruna as a deterministic compute lane and want to cache results across binary upgrades. Today the canonical example is **FleetQ** (Laravel-based AI agent platform), which runs `.ax` scripts via the MCP `boruna_run` tool and cannot safely memoize results without a stable identity for "what capabilities did this binary expose, and with what semantics".

## What they're doing today

Without `capability_set_hash`, an integrator's cache key looks like `(source_hash, input_hash, policy_hash)`. The moment Boruna upgrades — even a patch release — the integrator has no way to detect that, say, `net.fetch`'s response shape changed. They have two bad options:

1. **Don't cache** — leave free determinism on the table.
2. **Cache by binary version string** — invalidates on every patch release even when contracts didn't change.

## MVP someone would pay for

A single CLI subcommand and matching MCP tool that returns:

```json
{
  "name": "boruna",
  "version": "0.2.0",
  "capabilities": [
    { "name": "actor.send",   "version": "1" },
    { "name": "actor.spawn",  "version": "1" },
    { "name": "db.query",     "version": "1" },
    { "name": "fs.read",      "version": "1" },
    { "name": "fs.write",     "version": "1" },
    { "name": "llm.call",     "version": "1" },
    { "name": "net.fetch",    "version": "1" },
    { "name": "random",       "version": "1" },
    { "name": "time.now",     "version": "1" },
    { "name": "ui.render",    "version": "1" }
  ],
  "capability_set_hash": "sha256:<hex>"
}
```

Hash is SHA-256 over the canonical UTF-8 encoding of sorted `(name, version)` pairs joined by `\n`. Per-capability `version` bumps **only** when the capability *contract* (argument shape, return shape, side-effect semantics) changes — not on every binary release.

## What would make someone say "whoa"

> "Wait — so as long as the hash matches, I can cache the output of any deterministic Boruna run *forever*, even across years of upgrades?"

Yes. Cache key becomes `(source_hash, policy_hash, capability_set_hash, policy.schema_version)`. Hit rate stays high across releases that don't touch capability contracts. **Free deterministic caching** is a genuine selling point — no other workflow runtime offers this.

## How this compounds over time

1. **Every integrator** reuses the same caching contract — no per-vendor rediscovery.
2. **Adding a new capability** (e.g. `0.3.0` adds `time.sleep`) bumps the hash automatically. Old cached results still keyed under old hash → still valid.
3. **Changing an existing capability contract** (e.g. `net.fetch` adds a `redirects` array to the response) requires a deliberate `version` bump — caught in code review, not silently broken in the field.
4. **Documentation contract** in `docs/reference/capability-identity.md` becomes the canonical reference for downstream caching.
5. **Pairs with `Policy.schema_version`** (already shipped in 0.2.0) → completes the full versioned identity story.

## Out of scope

- Cache implementation in Boruna itself — this is a contract for *callers*.
- Per-capability documentation pages — separate sprint if needed.
- `--capability-list` flag on existing subcommands — only `boruna capability list` (new subcommand) and `boruna_capability_list` (MCP tool).

## Acceptance criteria

1. `boruna capability list --json` prints the JSON above to stdout, exit 0.
2. `boruna_capability_list` MCP tool returns the same shape (with `success: true` wrapper).
3. Hash is deterministic across runs (same binary → same hash, always).
4. Hash changes if any `(name, version)` pair changes; documented test asserts this.
5. `docs/reference/capability-identity.md` documents the contract and the caching recipe.
6. Per-capability `version` defaults to `"1"` for all 10 existing capabilities.
