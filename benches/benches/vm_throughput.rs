//! VM step-throughput benchmarks (sprint `W5-A`).
//!
//! Compiles each program once outside the timing loop and measures
//! `Vm::run()` per iteration. `Vm` is recreated each iteration because
//! it stores the post-execution stack/globals state.

use boruna_benches::{
    compile_or_panic, loop_program, loop_with_capability_program, loop_with_record_program,
};
use boruna_vm::{CapabilityGateway, Policy, Vm};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn run_compiled(module: &boruna_bytecode::Module) -> boruna_bytecode::Value {
    let gateway = CapabilityGateway::new(Policy::allow_all());
    let mut vm = Vm::new(module.clone(), gateway);
    vm.run().expect("bench fixture should run cleanly")
}

fn bench_pure_loop(c: &mut Criterion) {
    let mut group = c.benchmark_group("vm_pure_loop");
    for &iters in &[1_000_i64, 10_000, 100_000] {
        let module = compile_or_panic("pure_loop", &loop_program(iters));
        group.bench_function(format!("iters={iters}"), |b| {
            b.iter(|| {
                let v = run_compiled(black_box(&module));
                black_box(v);
            });
        });
    }
    group.finish();
}

fn bench_record_loop(c: &mut Criterion) {
    let mut group = c.benchmark_group("vm_record_loop");
    for &iters in &[1_000_i64, 10_000] {
        let module = compile_or_panic("record_loop", &loop_with_record_program(iters));
        group.bench_function(format!("iters={iters}"), |b| {
            b.iter(|| {
                let v = run_compiled(black_box(&module));
                black_box(v);
            });
        });
    }
    group.finish();
}

fn bench_call_dispatch_loop(c: &mut Criterion) {
    // Stand-in for capability-call workload: 4-deep call chain per
    // iteration. See `loop_with_capability_program` rationale.
    let mut group = c.benchmark_group("vm_call_dispatch_loop");
    for &iters in &[1_000_i64, 10_000] {
        let module = compile_or_panic("dispatch_loop", &loop_with_capability_program(iters));
        group.bench_function(format!("iters={iters}"), |b| {
            b.iter(|| {
                let v = run_compiled(black_box(&module));
                black_box(v);
            });
        });
    }
    group.finish();
}

criterion_group!(
    vm_benches,
    bench_pure_loop,
    bench_record_loop,
    bench_call_dispatch_loop
);
criterion_main!(vm_benches);
