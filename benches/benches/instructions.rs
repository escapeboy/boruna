//! Instruction-count benchmarks (Gungraun / Valgrind Callgrind).
//!
//! The criterion benches in this crate measure wall-clock time, which on a shared CI runner
//! moves by 10–40% between two runs of identical code. These count executed instructions
//! instead: the same code gives the same count on any machine, so a change in the count is a
//! change in the code. `bench-compare.yml` runs this file on the PR base and head and fails when
//! a benchmark executes noticeably more instructions.
//!
//! Setup (compiling the program, writing the bundle) runs outside the measured function.
//! Linux only: needs `valgrind` and `gungraun-runner` of the same version as `gungraun`.

use std::hint::black_box;
use std::path::PathBuf;

use boruna_benches::{
    build_evidence_bundle, compile_or_panic, loop_program, loop_with_capability_program,
    loop_with_record_program, render_crud_admin_template, MEDIUM_AX_SOURCE, SMALL_AX_SOURCE,
};
use boruna_bytecode::{Module, Value};
use boruna_compiler::compile;
use boruna_orchestrator::audit::verify_bundle;
use boruna_vm::{CapabilityGateway, Policy, Vm};
use gungraun::prelude::*;

#[library_benchmark]
#[bench::small(SMALL_AX_SOURCE.to_string())]
#[bench::medium(MEDIUM_AX_SOURCE.to_string())]
#[bench::crud_admin_template(render_crud_admin_template())]
fn compile_source(source: String) -> Module {
    black_box(compile("bench", black_box(&source)).expect("bench fixture should compile"))
}

fn pure_loop(iters: i64) -> Module {
    compile_or_panic("pure_loop", &loop_program(iters))
}

fn record_loop(iters: i64) -> Module {
    compile_or_panic("record_loop", &loop_with_record_program(iters))
}

fn call_dispatch_loop(iters: i64) -> Module {
    compile_or_panic("dispatch_loop", &loop_with_capability_program(iters))
}

#[library_benchmark]
#[benches::pure_loop(args = [1_000, 10_000], setup = pure_loop)]
#[benches::record_loop(args = [1_000], setup = record_loop)]
#[benches::call_dispatch_loop(args = [1_000], setup = call_dispatch_loop)]
fn vm_run(module: Module) -> Value {
    let mut vm = Vm::new(module, CapabilityGateway::new(Policy::allow_all()));
    black_box(vm.run().expect("bench fixture should run cleanly"))
}

fn bundle_with_steps(steps: usize) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = build_evidence_bundle(dir.path(), "run", steps);
    (dir, path)
}

#[library_benchmark]
#[benches::steps(args = [5, 10], setup = bundle_with_steps)]
fn evidence_verify(bundle: (tempfile::TempDir, PathBuf)) {
    let (dir, path) = bundle;
    black_box(verify_bundle(black_box(&path)));
    drop(dir);
}

library_benchmark_group!(name = compiler, benchmarks = compile_source);
library_benchmark_group!(name = vm, benchmarks = vm_run);
library_benchmark_group!(name = evidence, benchmarks = evidence_verify);

fn main() {
    main!(library_benchmark_groups = compiler, vm, evidence);
}
