//! Compile-time benchmarks (sprint `W5-A`).
//!
//! Measures `boruna_compiler::compile()` end-to-end — lex + parse +
//! typeck + codegen — on three representative source sizes.

use boruna_benches::{render_crud_admin_template, MEDIUM_AX_SOURCE, SMALL_AX_SOURCE};
use boruna_compiler::compile;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_compile_small(c: &mut Criterion) {
    c.bench_function("compile_small_program", |b| {
        b.iter(|| {
            let module = compile(black_box("small"), black_box(SMALL_AX_SOURCE))
                .expect("small program compiles");
            black_box(module);
        });
    });
}

fn bench_compile_medium(c: &mut Criterion) {
    c.bench_function("compile_medium_program", |b| {
        b.iter(|| {
            let module = compile(black_box("medium"), black_box(MEDIUM_AX_SOURCE))
                .expect("medium program compiles");
            black_box(module);
        });
    });
}

fn bench_compile_template(c: &mut Criterion) {
    // Render once outside the timing loop — we measure compile cost,
    // not template substitution cost.
    let source = render_crud_admin_template();
    c.bench_function("compile_crud_admin_template", |b| {
        b.iter(|| {
            let module = compile(black_box("crud_admin"), black_box(&source))
                .expect("crud-admin template output compiles");
            black_box(module);
        });
    });
}

criterion_group!(
    compile_benches,
    bench_compile_small,
    bench_compile_medium,
    bench_compile_template
);
criterion_main!(compile_benches);
