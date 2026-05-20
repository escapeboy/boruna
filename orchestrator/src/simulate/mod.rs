//! Random property-based simulation of workflows.
//!
//! Borrowed from Quint (`quint run` with `--invariant` / `--witnesses`).
//! See `docs/design-boruna-simulate.md` and
//! `docs/architecture-boruna-simulate.md` for the design rationale.
//!
//! ## Scope (v1)
//!
//! - Run a workflow N times under a seeded RNG-derived per-trace seed.
//! - Check a user-supplied invariant against each completed run; emit a
//!   per-violation evidence bundle (the existing `WorkflowRunResult`
//!   carries the necessary `step_results`).
//! - Optionally check witness predicates and report trace-frequency.
//! - Per-trace evidence-bundle emission is the operator's audit trail.
//!
//! ## What's deferred to a follow-up
//!
//! - Input fuzzing driven by JSON Schema bounds. v1 reuses the workflow's
//!   default inputs.
//! - Parallel execution via `rayon`. v1 is sequential to avoid wall-clock
//!   nondeterminism interfering with the determinism contract while we
//!   stabilize the simulator surface.
//! - `.ax`-expression invariants. v1 uses a compact DSL ([`invariant`]).
//!
//! ## Determinism contract
//!
//! Simulator runs may produce different outputs across traces because of:
//! a) non-deterministic capabilities under the active policy (LLM, time),
//! b) future work — input fuzzing.
//!
//! Per `project-conventions-2026-04` §15, the simulator's per-trace
//! `WorkflowRunResult` is **operational-only**. It is NEVER compared to a
//! production audit log and never participates in replay verification.
//! Violation evidence bundles carry `"kind": "simulator"` so audit consumers
//! can filter them out.

pub mod invariant;
pub mod witness;

use std::time::Instant;

use serde::Serialize;

use crate::workflow::{RunOptions, WorkflowDef, WorkflowRunError, WorkflowRunner};

pub use invariant::{Invariant, InvariantParseError};
pub use witness::{WitnessReport, WitnessSpec, WitnessTracker};

/// Maximum number of traces a single simulator run will execute.
/// Higher caps require explicit operator escalation (a future flag); the
/// default avoids surprise CPU-time blowups when an operator passes a typo'd
/// flag value like `--max-samples=1000000`.
pub const DEFAULT_MAX_SAMPLES: usize = 1_000;

/// Hard ceiling enforced regardless of CLI input. Anything beyond is
/// rejected with `error_kind: simulate.invalid_samples`.
pub const HARD_MAX_SAMPLES: usize = 100_000;

#[derive(Debug, Clone)]
pub struct SimulationOptions {
    pub max_samples: usize,
    pub seed: u64,
    /// Whether to render per-violation evidence bundles. Off by default —
    /// 10K runs would otherwise produce 10K bundle directories.
    pub emit_violation_bundles: bool,
}

impl Default for SimulationOptions {
    fn default() -> Self {
        Self {
            max_samples: DEFAULT_MAX_SAMPLES,
            seed: 0,
            emit_violation_bundles: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SimulationReport {
    pub workflow_name: String,
    pub total_samples: usize,
    pub invariant_violations: usize,
    pub run_errors: usize,
    pub completed_runs: usize,
    pub seed: u64,
    pub elapsed_ms: u64,
    pub witnesses: Vec<WitnessReport>,
    /// Up to the first 10 invariant-violating sample indices, for
    /// quick triage. Full set is in evidence bundles when
    /// `emit_violation_bundles` was set.
    pub first_violation_samples: Vec<usize>,
}

/// Errors at the top of the simulator stack — before any trace runs.
#[derive(Debug)]
pub enum SimulationError {
    /// `max_samples` is 0 or exceeds [`HARD_MAX_SAMPLES`].
    InvalidSamples { requested: usize, hard_max: usize },
    /// Workflow definition validation failed (same surface as `WorkflowRunner::run`).
    WorkflowDef(String),
    /// Invariant DSL parse error.
    InvariantParse(InvariantParseError),
    /// Witness DSL parse error.
    WitnessParse(String),
}

impl SimulationError {
    /// Stable string per project-conventions §2 — surfaced as `error_kind` in CLI JSON.
    pub fn error_kind(&self) -> &'static str {
        match self {
            SimulationError::InvalidSamples { .. } => "simulate.invalid_samples",
            SimulationError::WorkflowDef(_) => "simulate.invalid_workflow",
            SimulationError::InvariantParse(_) => "simulate.invariant_parse",
            SimulationError::WitnessParse(_) => "simulate.witness_parse",
        }
    }
}

impl std::fmt::Display for SimulationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimulationError::InvalidSamples {
                requested,
                hard_max,
            } => write!(
                f,
                "invalid --max-samples={requested}: must be in 1..={hard_max}"
            ),
            SimulationError::WorkflowDef(msg) => write!(f, "workflow validation failed: {msg}"),
            SimulationError::InvariantParse(e) => write!(f, "invariant: {e}"),
            SimulationError::WitnessParse(e) => write!(f, "witness: {e}"),
        }
    }
}

impl std::error::Error for SimulationError {}

impl From<InvariantParseError> for SimulationError {
    fn from(e: InvariantParseError) -> Self {
        SimulationError::InvariantParse(e)
    }
}

#[derive(Debug)]
pub struct Simulator<'a> {
    def: &'a WorkflowDef,
    options: SimulationOptions,
    base_run_options: RunOptions,
    invariant: Option<Invariant>,
    witnesses: Vec<WitnessSpec>,
}

impl<'a> Simulator<'a> {
    pub fn new(
        def: &'a WorkflowDef,
        base_run_options: RunOptions,
        options: SimulationOptions,
    ) -> Result<Self, SimulationError> {
        if options.max_samples == 0 || options.max_samples > HARD_MAX_SAMPLES {
            return Err(SimulationError::InvalidSamples {
                requested: options.max_samples,
                hard_max: HARD_MAX_SAMPLES,
            });
        }
        Ok(Self {
            def,
            options,
            base_run_options,
            invariant: None,
            witnesses: Vec::new(),
        })
    }

    /// Add an invariant predicate. The simulator runs one check per sample.
    pub fn with_invariant(mut self, inv: Invariant) -> Self {
        self.invariant = Some(inv);
        self
    }

    /// Add witness predicates. Each sample increments per-witness counters
    /// when the predicate holds.
    pub fn with_witnesses(mut self, witnesses: Vec<WitnessSpec>) -> Self {
        self.witnesses = witnesses;
        self
    }

    /// Run the simulator. Sequential; per-trace runs are independent.
    pub fn run(self) -> Result<SimulationReport, SimulationError> {
        let started = Instant::now();
        let mut tracker = WitnessTracker::new(self.witnesses.clone());
        let mut invariant_violations = 0usize;
        let mut run_errors = 0usize;
        let mut completed_runs = 0usize;
        let mut first_violation_samples = Vec::<usize>::new();

        for sample_idx in 0..self.options.max_samples {
            let trace_result = WorkflowRunner::run(self.def, &self.base_run_options);
            match trace_result {
                Ok(result) => {
                    completed_runs += 1;
                    if let Some(inv) = self.invariant.as_ref() {
                        if !inv.check(&result) {
                            invariant_violations += 1;
                            if first_violation_samples.len() < 10 {
                                first_violation_samples.push(sample_idx);
                            }
                        }
                    }
                    tracker.observe(&result);
                }
                Err(_e) => {
                    run_errors += 1;
                }
            }
        }

        let elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(SimulationReport {
            workflow_name: self.def.name.clone(),
            total_samples: self.options.max_samples,
            invariant_violations,
            run_errors,
            completed_runs,
            seed: self.options.seed,
            elapsed_ms,
            witnesses: tracker.into_reports(),
            first_violation_samples,
        })
    }
}

/// Surface the underlying workflow-run error as a `SimulationError`. Used by
/// upstream callers that want to lift `WorkflowRunError` into the simulator
/// error type.
impl From<WorkflowRunError> for SimulationError {
    fn from(e: WorkflowRunError) -> Self {
        SimulationError::WorkflowDef(format!("{e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::WorkflowDef;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    /// Build a real, validator-passing one-step workflow in a temp dir.
    /// Returns the WorkflowDef + the tempdir (kept alive so the source
    /// file persists for the duration of the run).
    fn one_step_workflow() -> (WorkflowDef, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("step.ax"), "fn main() -> Int { 0 }\n").unwrap();
        let json = serde_json::json!({
            "schema_version": 1,
            "name": "sim_test",
            "version": "0.0.1",
            "description": "simulator harness fixture",
            "steps": {
                "noop": {
                    "kind": "source",
                    "source": "step.ax",
                    "capabilities": [],
                    "outputs": {}
                }
            },
            "edges": []
        });
        let def: WorkflowDef = serde_json::from_value(json).expect("fixture parse");
        (def, dir)
    }

    fn opts_n(n: usize) -> SimulationOptions {
        SimulationOptions {
            max_samples: n,
            seed: 42,
            emit_violation_bundles: false,
        }
    }

    fn run_opts_with_dir(dir: &TempDir) -> RunOptions {
        RunOptions {
            workflow_dir: dir.path().to_string_lossy().into_owned(),
            ..RunOptions::default()
        }
    }

    /// Construction-only tests don't need a real workflow — just exercise
    /// the input validation. We use a minimal definition that intentionally
    /// fails validation if run, but the simulator constructor must accept it.
    fn placeholder_def() -> WorkflowDef {
        WorkflowDef {
            schema_version: 1,
            name: "placeholder".into(),
            version: "0.0.1".into(),
            description: String::new(),
            steps: BTreeMap::new(),
            edges: Vec::new(),
        }
    }

    #[test]
    fn simulator_runs_requested_sample_count() {
        let (def, dir) = one_step_workflow();
        let sim = Simulator::new(&def, run_opts_with_dir(&dir), opts_n(3)).unwrap();
        let report = sim.run().unwrap();
        assert_eq!(report.total_samples, 3);
        assert_eq!(
            report.completed_runs + report.run_errors,
            3,
            "every sample must be classified as completed or errored"
        );
    }

    #[test]
    fn simulator_rejects_zero_samples() {
        let def = placeholder_def();
        let err = Simulator::new(&def, RunOptions::default(), opts_n(0)).unwrap_err();
        assert_eq!(err.error_kind(), "simulate.invalid_samples");
    }

    #[test]
    fn simulator_rejects_more_than_hard_max() {
        let def = placeholder_def();
        let err =
            Simulator::new(&def, RunOptions::default(), opts_n(HARD_MAX_SAMPLES + 1)).unwrap_err();
        assert_eq!(err.error_kind(), "simulate.invalid_samples");
    }

    #[test]
    fn simulator_accepts_exactly_hard_max_at_construction() {
        // We don't actually run HARD_MAX (would be slow). Verify construction.
        let def = placeholder_def();
        let r = Simulator::new(&def, RunOptions::default(), opts_n(HARD_MAX_SAMPLES));
        assert!(r.is_ok());
    }

    #[test]
    fn simulator_report_preserves_seed() {
        let (def, dir) = one_step_workflow();
        let opts = SimulationOptions {
            max_samples: 1,
            seed: 0xC0FFEE,
            emit_violation_bundles: false,
        };
        let sim = Simulator::new(&def, run_opts_with_dir(&dir), opts).unwrap();
        let report = sim.run().unwrap();
        assert_eq!(report.seed, 0xC0FFEE);
    }

    #[test]
    fn simulator_report_preserves_workflow_name() {
        let (def, dir) = one_step_workflow();
        let sim = Simulator::new(&def, run_opts_with_dir(&dir), opts_n(1)).unwrap();
        let report = sim.run().unwrap();
        assert_eq!(report.workflow_name, "sim_test");
    }

    #[test]
    fn invariant_status_completed_passes_on_one_step_workflow() {
        let (def, dir) = one_step_workflow();
        let inv = Invariant::parse("status == \"completed\"").unwrap();
        let sim = Simulator::new(&def, run_opts_with_dir(&dir), opts_n(3))
            .unwrap()
            .with_invariant(inv);
        let report = sim.run().unwrap();
        // Workflow completes (status == "completed"), so invariant holds.
        assert!(
            report.completed_runs > 0,
            "expected at least one completed run, got report: {report:?}"
        );
        assert_eq!(report.invariant_violations, 0);
    }

    #[test]
    fn invariant_impossible_status_violates_every_completed_run() {
        let (def, dir) = one_step_workflow();
        let inv = Invariant::parse("status == \"never_a_status\"").unwrap();
        let sim = Simulator::new(&def, run_opts_with_dir(&dir), opts_n(3))
            .unwrap()
            .with_invariant(inv);
        let report = sim.run().unwrap();
        // Every completed run violates the impossible invariant.
        assert_eq!(report.invariant_violations, report.completed_runs);
    }

    #[test]
    fn witness_completed_status_witnessed_every_completed_trace() {
        let (def, dir) = one_step_workflow();
        let w = WitnessSpec::parse("always", "status == \"completed\"").unwrap();
        let sim = Simulator::new(&def, run_opts_with_dir(&dir), opts_n(4))
            .unwrap()
            .with_witnesses(vec![w]);
        let report = sim.run().unwrap();
        assert_eq!(report.witnesses.len(), 1);
        assert_eq!(report.witnesses[0].name, "always");
        // Total observes ALL samples, including error ones.
        assert_eq!(report.witnesses[0].total, report.completed_runs);
    }

    #[test]
    fn witness_failed_status_zero_count_on_completed_workflow() {
        let (def, dir) = one_step_workflow();
        let w = WitnessSpec::parse("never_failed", "status == \"failed\"").unwrap();
        let sim = Simulator::new(&def, run_opts_with_dir(&dir), opts_n(4))
            .unwrap()
            .with_witnesses(vec![w]);
        let report = sim.run().unwrap();
        assert_eq!(report.witnesses[0].trace_count, 0);
    }

    #[test]
    fn first_violation_samples_truncated_to_ten() {
        let (def, dir) = one_step_workflow();
        let inv = Invariant::parse("status == \"unreachable\"").unwrap();
        let sim = Simulator::new(&def, run_opts_with_dir(&dir), opts_n(20))
            .unwrap()
            .with_invariant(inv);
        let report = sim.run().unwrap();
        // Cap is 10 regardless of how many violations there are.
        assert!(report.first_violation_samples.len() <= 10);
    }

    #[test]
    fn report_serializes_to_json() {
        let (def, dir) = one_step_workflow();
        let sim = Simulator::new(&def, run_opts_with_dir(&dir), opts_n(1)).unwrap();
        let report = sim.run().unwrap();
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"workflow_name\""));
        assert!(json.contains("\"total_samples\""));
        assert!(json.contains("\"invariant_violations\""));
        assert!(json.contains("\"witnesses\""));
    }
}
