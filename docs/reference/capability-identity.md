# Capability Identity & Caching Contract

Boruna binaries advertise a stable, hashable identity for their capability surface. Integrators use this identity to safely cache deterministic run results across binary upgrades.

**Stability:** stable from `0.3.0`. Implementation shipped in `0.2.x` for early integrators (FleetQ #3).

## Why this exists

Boruna is deterministic by construction: for a given `(source, inputs, policy, capability_contract)` the run result is identical every time. That means an integrator can memoize results indefinitely — *as long as the capability contract hasn't changed*.

Without a stable identity for the capability contract, integrators have two bad options:

1. Don't cache — leave free determinism on the table.
2. Cache by binary version string — invalidate on every patch release, even when contracts didn't change.

`capability_set_hash` solves this: it's a content-addressed identity over the capability surface that changes only when the contract changes.

## Surface

### CLI

```bash
$ boruna capability list --json
{
  "protocol_version": 1,
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
  "capability_set_hash": "sha256:b0ca1793a79656447d560092bae7af4b0ebee82023c6d2bea82bd80621bd9637"
}
```

The CLI conveys success via process exit code, so there is **no** `success` field.

### MCP

Tool `boruna_capability_list` (no parameters). Returns the same fields as the CLI `--json` output **plus** a leading `success: true` envelope (this server's universal convention for tool responses):

```json
{
  "success": true,
  "protocol_version": 1,
  "name": "boruna",
  "version": "0.2.0",
  "capabilities": [ ... ],
  "capability_set_hash": "sha256:..."
}
```

### Field semantics

| Field | Meaning | Stability |
|---|---|---|
| `protocol_version` | Wire-format version of this report. Bumped on **breaking** shape changes (field rename, removal, type change). Additive changes keep the version. | Frozen for `protocol_version: 1` going forward. |
| `name` | Binary identity. Defaults to `"boruna"` for upstream binaries. Downstream forks that rebrand may emit their own name. **Does NOT participate in `capability_set_hash`.** | Stable string. |
| `version` | Binary version (`Cargo.toml` package version of the calling crate). **Does NOT participate in `capability_set_hash`.** | Semver. |
| `capabilities[].name` | Capability identifier (e.g. `"net.fetch"`). | Stable; new caps appear, never rename. |
| `capabilities[].version` | Capability contract version. Bumped on argument/return/semantics changes. | Increments as integer string. |
| `capability_set_hash` | SHA-256 over canonical encoding of `(name, version)` pairs in sorted order. | Algorithm frozen — see below. |

## Hash algorithm

The `capability_set_hash` is computed byte-for-byte as follows:

1. Take all capabilities in **canonical order** (sorted ascending by `name`).
2. For each, encode the UTF-8 bytes of `"{name}\t{version}\n"`.
   - `\t` = ASCII 0x09 (tab)
   - `\n` = ASCII 0x0A (newline)
3. Concatenate all encodings into a single byte string (no separators between capabilities beyond the trailing `\n` of each).
4. SHA-256 of that byte string.
5. Lower-case hex, prefixed with `"sha256:"`.

### Worked example (current `0.2.0` surface)

The byte string fed to SHA-256 is exactly:

```
actor.send\t1\nactor.spawn\t1\ndb.query\t1\nfs.read\t1\nfs.write\t1\nllm.call\t1\nnet.fetch\t1\nrandom\t1\ntime.now\t1\nui.render\t1\n
```

(252 bytes, with literal tabs and newlines, not the escape sequences shown.)

Reproduce with shell:

```bash
$ printf 'actor.send\t1\nactor.spawn\t1\ndb.query\t1\nfs.read\t1\nfs.write\t1\nllm.call\t1\nnet.fetch\t1\nrandom\t1\ntime.now\t1\nui.render\t1\n' | shasum -a 256
b0ca1793a79656447d560092bae7af4b0ebee82023c6d2bea82bd80621bd9637  -
```

## Caching contract for integrators

Recommended cache key:

```
key = sha256(
  source_hash       ||  // sha256(.ax source bytes)
  policy_hash       ||  // sha256(canonical JSON of the Policy object)
  capability_set_hash || // from this endpoint
  policy_schema_version  // from the Policy.schema_version field, e.g. "1"
)
```

This guarantees:

- A `.ax` source change invalidates the entry.
- A policy change invalidates the entry.
- A capability contract change invalidates the entry (new capability added, or existing capability semantics changed).
- A policy schema change invalidates the entry (Boruna evolves the policy format).

Anything outside this set — Rust toolchain version, Boruna patch version that didn't touch capabilities, build host — does **not** invalidate cached results, because none of it can change the deterministic output.

## Per-capability `version` semantics

Each capability has its own `version`. We **bump it only when the contract changes** in a way that would make a downstream cached result invalid:

| Change | Bump? |
|---|---|
| Argument shape changes (new field, removed field, type change) | **Yes** |
| Return shape changes | **Yes** |
| Side-effect semantics change (e.g. `net.fetch` starts following redirects by default) | **Yes** |
| Performance improvement, internal refactor | No |
| Bug fix that brings behavior in line with documented contract | No |
| Stricter input validation that rejects previously-accepted-but-undefined inputs | Judgment call — usually **Yes** to be safe |

When you bump a capability version, you must also:

1. Update the match arm in `crates/llmbc/src/capability.rs::Capability::version()`.
2. Update the golden hash in `crates/llmbc/src/tests.rs::test_capability_set_hash_known_value`.
3. Add a `### Changed` entry under `[Unreleased]` in `CHANGELOG.md` referencing the capability.

## What does and does not affect the hash

| Affects `capability_set_hash`? | |
|---|---|
| ✅ Adding a new capability | Yes — extends the byte-string input. |
| ✅ Removing a capability | Yes — removes from input. |
| ✅ Bumping any capability's `version` field | Yes — that's the entire point. |
| ❌ Bumping the binary's `version` field | No — `binary_version` is metadata, not contract surface. |
| ❌ Forks emitting a different `name` | No — `binary_name` is metadata, not contract surface. |
| ❌ Bumping `protocol_version` | No — wire-format envelope is independent of capability contract. |

This separation is what makes the hash useful: a Boruna patch release that touches no capabilities produces an identical hash, so cached results stay valid.

## Stability guarantees

- The **algorithm** above is frozen. We will never change how the hash is computed without a major version bump (`protocol_version: 2`) and a clearly-documented migration path. An algorithm-level test (`test_compute_capability_set_hash_algorithm_known_value`) locks the encoding rule independently of the current capability set.
- The **JSON shape** of the report is locked at `protocol_version: 1`. Field additions are non-breaking and keep `protocol_version`; field removals, renames, or type changes bump `protocol_version`.
- The **per-capability versions** evolve independently of each other and of `protocol_version`. Today they are all `"1"`. Future bumps follow the rules above.

## Pairs with `Policy.schema_version`

The `Policy` object (see [policy schema](./policy-schema.md)) carries a `schema_version` field independently. Together they cover:

- `capability_set_hash` — does the binary still mean the same thing by `net.fetch`?
- `Policy.schema_version` — does the binary still parse my policy the same way?

Both must match for a cached result to be valid.

## See also

- [`docs/reference/policy-schema.md`](./policy-schema.md) — capability policy structure
- [`docs/concepts/determinism.md`](../concepts/determinism.md) — why Boruna can be cached at all
- FleetQ feedback letter — original ask ([issue #3](https://github.com/escapeboy/boruna/issues/3))
