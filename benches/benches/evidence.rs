//! Evidence-bundle benchmarks (sprint `W5-A`).
//!
//! Measures the cost of building a bundle on disk and verifying its
//! hash chain. Each iteration uses a fresh tempdir so file-system
//! caches don't blur successive runs.

use boruna_benches::build_evidence_bundle;
use boruna_orchestrator::audit::{verify_bundle, AuditLog, EvidenceBundleBuilder};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_build_empty(c: &mut Criterion) {
    c.bench_function("evidence_build_empty", |b| {
        b.iter_with_setup(
            || tempfile::tempdir().expect("tempdir"),
            |dir| {
                let mut builder =
                    EvidenceBundleBuilder::new(dir.path(), "run-empty", "bench-workflow")
                        .expect("create builder");
                builder
                    .add_workflow_def(r#"{"name":"bench-workflow"}"#)
                    .expect("workflow def");
                builder
                    .add_policy(r#"{"schema_version":1,"rules":{},"default_allow":true}"#)
                    .expect("policy");
                let audit = AuditLog::new();
                let manifest = builder.finalize(&audit).expect("finalize");
                black_box(manifest);
            },
        );
    });
}

fn bench_build_5_steps(c: &mut Criterion) {
    c.bench_function("evidence_build_5_steps", |b| {
        b.iter_with_setup(
            || tempfile::tempdir().expect("tempdir"),
            |dir| {
                let path = build_evidence_bundle(dir.path(), "run-5", 5);
                black_box(path);
            },
        );
    });
}

fn bench_verify_5_steps(c: &mut Criterion) {
    // Round-trip: build the bundle inside the setup closure (NOT
    // timed), then verify in the routine (timed).
    c.bench_function("evidence_verify_5_steps", |b| {
        b.iter_with_setup(
            || {
                let dir = tempfile::tempdir().expect("tempdir");
                let bundle_path = build_evidence_bundle(dir.path(), "run-5", 5);
                (dir, bundle_path)
            },
            |(dir, bundle_path)| {
                let result = verify_bundle(&bundle_path);
                black_box(&result);
                // Hold `dir` until verify completes so the tempdir
                // lives long enough; drop it at the end of this
                // iteration.
                drop(dir);
            },
        );
    });
}

fn bench_verify_10_steps(c: &mut Criterion) {
    c.bench_function("evidence_verify_10_steps", |b| {
        b.iter_with_setup(
            || {
                let dir = tempfile::tempdir().expect("tempdir");
                let bundle_path = build_evidence_bundle(dir.path(), "run-10", 10);
                (dir, bundle_path)
            },
            |(dir, bundle_path)| {
                let result = verify_bundle(&bundle_path);
                black_box(&result);
                drop(dir);
            },
        );
    });
}

criterion_group!(
    evidence_benches,
    bench_build_empty,
    bench_build_5_steps,
    bench_verify_5_steps,
    bench_verify_10_steps
);
criterion_main!(evidence_benches);
