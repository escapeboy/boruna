# Design — ITF (Informal Trace Format) trace export

## Status

Planned for v1.5.0. **Implemented this sprint** (per `/sprint-orchestrate full`).
Borrowed from Quint / Apalache. Source: research_quint_borrowable_ideas_2026-05-20.md, rec #4.

## Context

Boruna emits its own trace formats from two places:
1. `boruna trace2tests` produces regression tests + minimized failure traces in a custom JSON.
2. `boruna evidence inspect` emits the evidence bundle's audit log in another custom JSON shape.

Quint and Apalache emit traces in **ITF (Informal Trace Format)** — a tiny, documented schema
defined at [apalache-mc.org/docs/adr/015adr-trace.html](https://apalache-mc.org/docs/adr/015adr-trace.html).
It is the format consumed by the [ITF VS Code Trace Viewer](https://marketplace.visualstudio.com/items?itemName=informal.itf-trace-viewer)
and increasingly by other formal-methods tools.

ITF is *narrower* than a Boruna evidence bundle (no hash chains, no env fingerprint, no policy
snapshot). But it's an industry-standard envelope for "sequence of typed states." If Boruna
emits an ITF *export* view of its existing traces, every Quint/Apalache visualization tool works
out of the box.

## Why

This is the same play as the v1.4.0 agent-native `--json` outputs: instead of insisting on a
bespoke format, become interoperable with the surrounding ecosystem. Zero conceptual cost (ITF
is roughly a subset of what Boruna already records). One day of work. Compounding value: the
formal-methods VS Code extension Just Works on Boruna traces.

## Goals

1. `boruna trace2tests <bundle-dir> --out-itf=traces/` writes each minimized failure trace as
   `traces/out_<n>.itf.json` matching the ITF v0.15 schema.
2. `boruna evidence inspect <bundle-dir> --format=itf` writes the entire audit log as a single
   ITF document to stdout (or `--out`).
3. Boruna's internal formats stay unchanged. ITF is an EXPORT only; nothing reads ITF back.
4. Schema validation: a Rust unit test parses the emitted ITF and asserts conformance to the
   ADR 015 schema (using a vendored JSON Schema, not a runtime jsonschema dep).
5. Round-trip with `jq`: `boruna evidence inspect --format=itf … | jq '.states[0].vars'` returns
   the first state's variable bindings.

## Non-goals

- ITF *import* — Boruna does not consume ITF as input.
- Replacement of internal formats. Evidence bundles stay as-is.
- A general "trace transformation pipeline." Just ITF, just export, just two existing surfaces.

## Forcing questions

**Who needs this? What are they doing today?**
An engineer comparing a Boruna trace against the spec it was supposed to satisfy in Quint.
Today: hand-rolled translation between formats. With ITF export: the ITF Trace Viewer renders
both side-by-side.

**What's the narrowest MVP someone would pay for?**
`boruna evidence inspect <dir> --format=itf` writing a single valid ITF document. That's it.
`trace2tests --out-itf` is incremental.

**What would make someone say "whoa"?**
Opening a Boruna evidence bundle's ITF export in the official Quint ITF VS Code extension
and seeing the per-state diff highlighted.

**How does this compound over time?**
Any future Boruna trace-producing surface inherits the ITF export for free (single serializer
function). Adopting ITF also signals to the formal-methods community that Boruna participates
in their ecosystem.

## Scope (this sprint)

| In | Out |
|---|---|
| `boruna_tooling::trace::itf::TraceItf` serializer | ITF import / parser |
| `boruna evidence inspect --format=itf` | New `boruna trace export` top-level command |
| `boruna trace2tests --out-itf=<dir>` | Streaming output (whole-trace materialization is fine for v1.5) |
| Unit test asserting ITF v0.15 schema conformance | Custom ITF extensions beyond v0.15 |

## Decisions

1. **ITF schema version target:** v0.15 (the version Apalache emits as of 2026). Documented in
   a constant `ITF_FORMAT_VERSION = "0.15"` in the serializer module.
2. **Crate location:** New module `tooling/src/trace/itf.rs`. Sibling of (not coupled to)
   `tooling/trace2tests/`. The serializer takes a generic trace shape (`Vec<State>` where
   `State` is a `BTreeMap<String, ItfValue>`) so it's reusable.
3. **`ItfValue` type:** Mirrors Boruna's `Value` enum but flattened to ITF's restricted type
   set (int, str, bool, list, set, map/record, none, unserializable). `unserializable` is a
   marker per ITF spec for things like `FnRef`.
4. **Conversion from Boruna `Value`:** Implemented as `impl From<&boruna_bytecode::Value> for
   ItfValue`. `FnRef` → `unserializable`. `Result/Option` → record with `tag`/`value` fields.
5. **Output file naming:** `--out-itf=<dir>` writes `out_0.itf.json`, `out_1.itf.json`, ...
   matching Quint's `out_{seq}.itf.json` convention exactly.

## Risks

- **ITF schema drift.** Apalache could bump the schema. Mitigation: vendored JSON Schema in
  `tooling/src/trace/itf-v0.15.schema.json`; drift test compares emitted document against it.
  Per conventions §33, this is the cheap-and-effective approach.
- **`unserializable` proliferation.** Boruna's value enum has shapes ITF doesn't represent well
  (e.g., `Capability` references inside captured closures). Mitigation: document them clearly
  as `#unserializable("FnRef:42")` strings with the internal ID preserved; round-trip
  identification works even if the values themselves don't.

## Implementation effort estimate (verified during build phase)

- ITF schema + Rust types: ~80 lines
- Boruna `Value` → `ItfValue` conversion: ~60 lines
- CLI plumbing for `--out-itf` and `--format=itf`: ~50 lines
- Tests: ~150 lines
- Schema drift test: ~30 lines

**Total: ~370 lines of code + tests.** Small, additive, low-risk.
