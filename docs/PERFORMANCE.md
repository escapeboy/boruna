# Performance Baseline

This document is the published baseline for Boruna's performance budget
(roadmap milestone for 1.0.0). It captures **reproducible benchmarks**,
not portable absolute numbers — your machine will measure differently,
and that's fine. The goal is a stable harness so we can detect
regressions over time.

## Benchmark suite

The suite lives in [`benches/`](../benches) at the workspace root and
uses [criterion](https://docs.rs/criterion/) 0.5. It covers three
areas:

- **`compile.rs`** — `boruna_compiler::compile()` end-to-end (lex →
  parse → typeck → codegen). Three sizes: ~20-line program,
  ~200-line program with records / pattern matching / loops, and
  the post-substitution `crud-admin` template.
- **`vm_throughput.rs`** — `Vm::run()` on tight loops at 1k / 10k /
  100k iterations. Variants: pure arithmetic, per-iteration record
  allocation, and a 4-deep call-chain stand-in for capability
  dispatch (the surface language only generates `CapCall` from
  `step_input`, which needs a workflow context — pure call dispatch
  shares the same hot opcode loop).
- **`evidence.rs`** — `EvidenceBundleBuilder::finalize()` and
  `verify_bundle()` round-trips at 0 / 5 / 10 steps.

## How to run

```bash
# Build the harness without running (fast, used in CI)
cargo bench -p boruna-benches --no-run

# Run all benches with criterion's defaults (~3 minutes)
cargo bench -p boruna-benches

# Run a single bench file with a quick configuration
cargo bench -p boruna-benches --bench compile -- \
    --sample-size 10 --warm-up-time 1 --measurement-time 3
```

Criterion writes detailed JSON + (optionally) HTML reports under
`target/criterion/`. Compare two runs with `criterion --baseline
<name>` after saving a baseline (`--save-baseline <name>`).

## Baseline (recorded 2026-04-28)

Captured with `--sample-size 10 --warm-up-time 1 --measurement-time 3`
on a developer laptop (macOS, x86_64, dev profile of dependencies but
release profile for the bench binary — criterion's default). The
**median** column is what we compare against; the bracket shows
criterion's lo/hi bound for the central tendency.

| Benchmark                              | Median   | Range            |
| -------------------------------------- | -------- | ---------------- |
| `compile_small_program`                | 32 µs    | 26 – 44 µs       |
| `compile_medium_program`               | 346 µs   | 273 – 511 µs     |
| `compile_crud_admin_template`          | 397 µs   | 318 – 452 µs     |
| `vm_pure_loop/iters=1000`              | 361 µs   | 332 – 397 µs     |
| `vm_pure_loop/iters=10000`             | 3.97 ms  | 3.58 – 4.66 ms   |
| `vm_pure_loop/iters=100000`            | 36.0 ms  | 32.2 – 41.6 ms   |
| `vm_record_loop/iters=1000`            | 1.47 ms  | 1.29 – 1.84 ms   |
| `vm_record_loop/iters=10000`           | 32.5 ms  | 27.5 – 36.2 ms   |
| `vm_call_dispatch_loop/iters=1000`     | 5.42 ms  | 3.80 – 6.62 ms   |
| `vm_call_dispatch_loop/iters=10000`    | 37.7 ms  | 32.2 – 46.1 ms   |
| `evidence_build_empty`                 | 2.16 ms  | 1.93 – 2.67 ms   |
| `evidence_build_5_steps`               | 7.22 ms  | 6.37 – 8.44 ms   |
| `evidence_verify_5_steps`              | 3.95 ms  | 3.23 – 4.65 ms   |
| `evidence_verify_10_steps`             | 5.71 ms  | 3.76 – 7.48 ms   |

Roughly: a small program compiles in tens of microseconds, a medium
program in hundreds of microseconds; the VM sustains ~2.7 M
arithmetic-loop iterations per second; an evidence bundle round-trip
(build + verify, 5 steps) takes ~11 ms total.

## 1.x performance budget commitments

These are conservative ceilings — roughly 2x the baseline median plus
headroom — that we commit to NOT regressing past on hardware in the
same class as the baseline machine. CI does not enforce them today
(see "CI" below); they're a contract for human review of perf-
sensitive PRs.

| Budget                                | Ceiling   | Source bench                        |
| ------------------------------------- | --------- | ----------------------------------- |
| `compile_small_program`               | < 5 ms    | `compile_small_program` (median 32 µs) |
| `compile_medium_program`              | < 5 ms    | `compile_medium_program` (median 346 µs) |
| `compile_crud_admin_template`         | < 5 ms    | `compile_crud_admin_template` (median 397 µs) |
| `vm_pure_loop` per 100k iters         | < 100 ms  | `vm_pure_loop/iters=100000` (median 36 ms) |
| `vm_record_loop` per 10k iters        | < 80 ms   | `vm_record_loop/iters=10000` (median 32 ms) |
| `evidence_build_5_steps`              | < 25 ms   | `evidence_build_5_steps` (median 7.2 ms) |
| `evidence_verify_10_steps`            | < 50 ms   | `evidence_verify_10_steps` (median 5.7 ms) |

If a PR drops a number more than 2x past the ceiling, treat it as a
regression — bisect, profile, and either fix or document the cause
before merging.

## Interpreting regressions

1. Save a baseline before changing anything:
   ```bash
   cargo bench -p boruna-benches -- --save-baseline before
   ```
2. Apply your change.
3. Compare:
   ```bash
   cargo bench -p boruna-benches -- --baseline before
   ```
   Criterion prints a per-benchmark verdict (`Improved`, `Regressed`,
   `No change`).
4. If `Regressed` shows up:
   - Re-run on a quiet machine (close browsers, stop background
     processes — criterion is sensitive to scheduling jitter).
   - If still regressed, look at flamegraphs (`cargo flamegraph
     --bench <name>`) before guessing.
   - A 5–10 % wobble between runs is normal; flag only consistent
     regressions of 25 %+ relative to the budget headroom.

## CI

Benches are **not** gated in CI today — `cargo bench` is slow (each
benchmark needs ≥3 s of measurement time × ≥10 samples for stable
numbers) and criterion's stdout is noisy. The smoke test in
`benches/tests/smoke.rs` runs under `cargo test --workspace` and
guarantees the bench fixtures still compile and execute, which catches
the most common breakage (a lib refactor stranding a bench call).

A future sprint may add a perf-comparison gate via
[`criterion-compare-action`](https://github.com/boa-dev/criterion-compare-action)
or [`cargo-codspeed`](https://github.com/CodSpeedHQ/codspeed-rust),
both of which run on hosted runners with bare-metal stability and
post a PR comment with the diff. The decision is gated on actual
CI flake rate — regression detection is worse than no detection if
random green builds get reported as 30 % regressions.

For now, the workflow is: **run locally before merging
perf-sensitive PRs**.

## Source layout

```
benches/
  Cargo.toml          # workspace member, depends on criterion 0.5
  src/lib.rs          # shared fixtures (.ax sources, bundle builder)
  benches/
    compile.rs        # compile time benchmarks
    vm_throughput.rs  # vm step throughput benchmarks
    evidence.rs       # evidence bundle write/verify benchmarks
  tests/
    smoke.rs          # CI-gated smoke test exercising every fixture
```

Sprint reference: `W5-A` (1.0.0 roadmap entry "Performance benchmarks").
