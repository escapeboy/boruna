//! Smoke test for the benchmark fixtures (sprint `W5-A`, project §29).
//!
//! Criterion benches are compiled by `cargo bench --no-run` but never
//! exercised in CI. This test imports the same setup helpers the
//! benches use and runs ONE invocation of each so a typo or broken
//! fixture surfaces as a normal `cargo test` failure rather than as a
//! silent bench-only regression.

use boruna_benches::{
    build_evidence_bundle, compile_or_panic, loop_program, loop_with_capability_program,
    loop_with_record_program, render_crud_admin_template, run_module, MEDIUM_AX_SOURCE,
    SMALL_AX_SOURCE,
};
use boruna_orchestrator::audit::verify_bundle;

#[test]
fn bench_fixtures_compile_run_and_verify() {
    // Compile bench inputs.
    let _small = compile_or_panic("small", SMALL_AX_SOURCE);
    let _medium = compile_or_panic("medium", MEDIUM_AX_SOURCE);
    let template_src = render_crud_admin_template();
    let _template = compile_or_panic("crud_admin", &template_src);

    // VM bench inputs — keep iteration counts tiny so the smoke test
    // stays in the millisecond range.
    let pure = compile_or_panic("pure", &loop_program(100));
    let _ = run_module(pure);

    let record = compile_or_panic("record", &loop_with_record_program(100));
    let _ = run_module(record);

    let dispatch = compile_or_panic("dispatch", &loop_with_capability_program(100));
    let _ = run_module(dispatch);

    // Evidence bench inputs.
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = build_evidence_bundle(dir.path(), "smoke-run", 3);
    let result = verify_bundle(&bundle);
    assert!(result.valid, "smoke bundle should verify: {result:?}");
}
