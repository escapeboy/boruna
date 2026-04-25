//! Integration test for the retry policy's real wall-clock backoff.
//!
//! This file lives in `orchestrator/tests/` rather than inside the
//! orchestrator crate's source tree because `cfg(test)` is NOT set on
//! the orchestrator's library build when integration tests compile —
//! so `retry_with_backoff`'s sleep is real here, not skipped. The
//! orchestrator's own unit tests skip the sleep for fast iteration;
//! this file locks the real-timing contract operators see in
//! production.
//!
//! Reviewed 0.3-S5 (finding #2): prior to this test, all retry
//! coverage skipped the sleep, so a future regression silently
//! removing the `std::thread::sleep` would not have been caught.

use std::collections::BTreeMap;
use std::time::Instant;

use boruna_orchestrator::workflow::{
    RetryPolicy, RunOptions, StepDef, StepKind, WorkflowDef, WorkflowRunner, WorkflowStatus,
};
use boruna_vm::capability_gateway::Policy;

#[test]
fn retry_actually_sleeps_between_attempts() {
    // 3 attempts × 100ms+200ms = 300ms minimum wall-clock for the
    // sleeps alone (plus negligible compile-and-fail time per attempt).
    // The bad step is a syntax error, so every attempt fails fast at
    // compile time.
    let dir = tempfile::tempdir().unwrap();
    let steps_dir = dir.path().join("steps");
    std::fs::create_dir_all(&steps_dir).unwrap();
    std::fs::write(steps_dir.join("bad.ax"), "fn main( { }").unwrap();

    let bad = StepDef {
        kind: StepKind::Source {
            source: "steps/bad.ax".into(),
        },
        capabilities: vec![],
        inputs: BTreeMap::new(),
        outputs: BTreeMap::new(),
        depends_on: vec![],
        timeout_ms: None,
        retry: Some(RetryPolicy {
            max_attempts: 3,
            on_transient: true,
        }),
        budget: None,
    };
    let def = WorkflowDef {
        schema_version: 1,
        name: "retry-timing".into(),
        version: "1.0.0".into(),
        description: String::new(),
        steps: BTreeMap::from([("bad".into(), bad)]),
        edges: vec![],
    };
    let options = RunOptions {
        policy: Some(Policy::allow_all()),
        record: false,
        workflow_dir: dir.path().to_string_lossy().to_string(),
        live: false,
        concurrency: 1,
    };

    let start = Instant::now();
    let result = WorkflowRunner::run(&def, &options).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(result.status, WorkflowStatus::Failed);
    // Lower bound: 100ms (before attempt 2) + 200ms (before attempt 3)
    // = 300ms minimum. Generous lower bound at 250ms to allow for
    // CI clock-resolution skew while still failing if sleeps are
    // accidentally skipped (skipped sleeps would complete in <50ms).
    assert!(
        elapsed.as_millis() >= 250,
        "retry sleeps must actually fire under integration-test build; \
         elapsed only {elapsed:?} (expected >= 300ms = 100ms+200ms backoff)"
    );
    // Upper bound: keep the test fast. 3-attempt backoff is at most
    // 100+200 = 300ms of sleeps + tiny compile time. 5s is generous.
    assert!(
        elapsed.as_secs() < 5,
        "retry took {elapsed:?} which is way over the expected 300ms; \
         is the backoff cap broken?"
    );
}
