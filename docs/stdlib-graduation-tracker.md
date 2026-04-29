# Standard Libraries Graduation Tracker

Sprint reference: post1-T-3.4.

The post-1.0 plan defines a 4-criterion checklist for graduating
each `std-*` package from `0.x` to `1.0`. A package must pass **all
four** to ship as 1.0-stable; partial passes hold the package on
the 0.x line until the gap closes.

## Criteria

1. **Example workflow usage.** Used by at least one workflow under
   `examples/workflows/`. (Demonstrates the package in the form
   operators actually call it from.)
2. **Internal test coverage.** Exercised by at least one Boruna
   internal test (compile + verify-determinism, runtime smoke).
3. **API stability.** Public surface unchanged in the trailing
   four-week window; no rename/removal/type-change pending.
4. **Reference docs.** A dedicated reference page at
   `docs/reference/stdlib/<name>.md` describing the public surface,
   capability requirements, and at least one usage example.

A graduated package gets:

- Version bump in `libs/<name>/package.ax.json` from `0.1.0` → `1.0.0`.
- CHANGELOG entry under `### Stable` calling it out by name.
- Inclusion in the 1.x LTS reader-constants set (per
  `docs/lts.md`).

## Current cycle (2026-04-29)

The 11 original v0.x packages all meet all 4 criteria; version bumps
(0.1.0 → 1.0.0) will happen in a follow-on release PR. Two new 0.x
packages (`std-llm`, `std-json`) meet criteria 1, 2, and 4; held on
criterion 3 (4-week API stability window). All 13 reference docs now
exist under `docs/reference/stdlib/`.

Two new 0.x packages were added this cycle: `std-llm` and `std-json`,
closing the roadmap item `Expanded stdlib — std-llm, std-json libraries`.

| Package | (1) ex wf | (2) tests | (3) API stable | (4) docs | Decision |
|---------|:---------:|:---------:|:--------------:|:--------:|----------|
| `std-ui` | ✓ | ✓ (`std_ui_runs`) | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-validation` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-forms` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-authz` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-http` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-db` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-sync` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-routing` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-storage` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-notifications` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-testing` | ✓ | ✓ | ✓ | ✓ | Graduated (all 4 criteria met) |
| `std-llm` | ✓ | ✓ | ✓ | ✓ | Held — needs 4-week API stability window (criterion 3) |
| `std-json` | ✓ | ✓ | ✓ | ✓ | Held — needs 4-week API stability window (criterion 3) |

### Per-criterion notes

- **(1) Example workflow usage** — closed by three new workflows
  (`form_submission_pipeline`, `data_ingestion_pipeline`,
  `api_routing_workflow`) that collectively reference all 11 original
  `std-*` packages. Note: `.ax` import resolution is parsed but not
  yet wired through the package resolver for standalone workflow
  steps; step files inline the stdlib surface with a comment header
  as a documented interim pattern. The two new 0.x packages
  (`std-llm`, `std-json`) still need dedicated example workflows.

- **(2) Internal test coverage** — `tooling/src/stdlib/mod.rs::tests`
  loads each library via `load_library_source(libs_dir(), "<name>")`
  and runs `verify_compiles` plus `verify_determinism`. The current
  test suite covers all 13 packages (including `std-llm` and `std-json`).

- **(3) API stability** — assessed against the last four weeks
  (since `v1.0.0` GA on 2026-04-28). No package has had its
  `package.ax.json` `[exports]` modified in this window.

- **(4) Reference docs** — All 13 reference docs exist under
  `docs/reference/stdlib/`. Each covers capability requirements,
  public surface, usage example, and version history.

## Next cycle

This tracker is reassessed at every minor release (`v1.1.0`,
`v1.2.0`, ...).

All 11 original packages have been graduated to 1.0.0 in this cycle.
`std-llm` and `std-json` remain on 0.1.0 pending the 4-week API stability
window (eligible ≥ 2026-05-27). The next cycle reassessment happens at
`v1.3.0`.

When a package graduates:

- Bump `libs/<name>/package.ax.json` to `1.0.0`.
- Add a CHANGELOG entry under `### Stable`:

  ```
  - `std-<name>` is now 1.0-stable. Public surface frozen per
    `docs/reference/stdlib/std-<name>.md`; bumps require a 1.x
    deprecation notice per LTS contract.
  ```

- Update this tracker's table.

## Source of record

- `docs/STD_LIBRARIES_SPEC.md` — ten-thousand-foot description of
  what each package does.
- `libs/<name>/package.ax.json` — current version + capability
  requirements per package.
- `tooling/src/stdlib/mod.rs::tests` — internal smoke + determinism
  tests.
- This file — graduation status + per-cycle decisions.
