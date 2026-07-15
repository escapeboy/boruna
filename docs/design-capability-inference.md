# Design — Capability-row inference / over-declaration check (Theme D-lite, Sprint 4)

Branch (create AFTER v1.8.0 merges): `feat/capability-inference` off updated master (has Theme C's
call-graph analysis to build on). Borrowed from **AILANG** (effect-row inference — compiler computes
the minimal capability set and flags over-declaration). User chose the lite core — **no
information-flow / data-visibility typing** (large type-system addition; deferred).

## Idea
A function's declared `!{...}` capabilities SHOULD equal what it actually needs (least privilege).
Today nothing checks this: a function can declare `!{net.fetch}` and never use it (over-grant of
authority — a real least-privilege / audit smell). Compute each function's *needed* capability set
from its own effect uses + its callees', and flag capabilities it declares but doesn't need.

## Design
- **`Module::needed_capabilities(func_idx) -> BTreeSet<Capability>`** (bytecode): the set of caps a
  function actually requires =
  { caps referenced by its own `CapCall(cap_id, _)` ops }
  ∪ ⋃ needed_capabilities(callee) for each `Call`/`SpawnActor` callee.
  Cycle-safe (visited set); deterministic (BTreeSet). Reuses the call-graph traversal shape from
  Theme C's `reaches_cap`. NOTE: `CapCall` carries `cap_id: u32` → `Capability::from_id`.
- **Over-declaration = declared \ needed.** A helper `Module::over_declared_capabilities(func_idx)`
  returns the sorted caps a function declares but does not (transitively) need.
- **Surface**: a read-only CLI analysis `boruna lang caps <file.ax> [--json]` (or fold into
  `boruna lang check` as a Warning-severity diagnostic) listing per-function over-declarations.
  MVP: a dedicated analysis command that returns, per function, {declared, needed, over_declared}.
  Decide at implementation time based on how `boruna lang check` diagnostics are structured — if
  cheap, emit a stable Warning code; else a standalone `caps` subcommand.

## Why not enforce (error) or auto-fix
Over-declaration is a smell, not a correctness bug (the VM still gates at runtime). Report as a
warning/analysis, not a hard error — avoids breaking existing `.ax` that over-declares. Under-
declaration is already caught at runtime by the CapabilityGateway; the compiler need not duplicate.

## Determinism
Pure function of the module; BTreeSet ordering → deterministic output.

## Scope
In: `needed_capabilities` + `over_declared_capabilities` analysis + a read-only surface.
Out (deferred): information-flow/data-visibility typing (Plumbing); auto-repair of over-declarations;
making it a hard compile error.

## Tests
- needed_capabilities: direct CapCall; transitive via callee; cycle-safe; union across multiple callees.
- over_declared: declares net.fetch but never uses → flagged; declares exactly what it needs → empty.
- surface: CLI/diagnostic returns the expected per-function report.
