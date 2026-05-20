//! Witness predicates for `boruna simulate --witnesses name=expr,...`.
//!
//! A witness is an invariant we WANT to be possible (not required). The
//! simulator reports how many traces witnessed the predicate. Borrowed from
//! Quint's `quint run --witnesses=`.
//!
//! Witness expressions reuse the [`Invariant`](super::invariant::Invariant)
//! DSL — same grammar, different semantics. Where invariants are
//! "must hold or violation", witnesses are "frequency counter."
//!
//! See `docs/design-boruna-witnesses.md`.

use serde::Serialize;

use super::invariant::{Invariant, InvariantParseError};
use crate::workflow::WorkflowRunResult;

#[derive(Debug, Clone)]
pub struct WitnessSpec {
    pub name: String,
    invariant: Invariant,
}

impl WitnessSpec {
    /// Parse a single witness with an explicit name. Use [`WitnessSpec::parse_csv`]
    /// when accepting a `name=expr,name=expr` list from the CLI.
    pub fn parse(name: &str, expr: &str) -> Result<Self, String> {
        if name.is_empty() {
            return Err("witness name must be non-empty".into());
        }
        if name.contains('=') {
            return Err(format!(
                "witness name `{name}` must not contain `=` — split first"
            ));
        }
        let invariant = Invariant::parse(expr).map_err(|e| format!("{name}: {e}"))?;
        Ok(Self {
            name: name.to_string(),
            invariant,
        })
    }

    /// Parse a CSV of `name=expr,name=expr,...`. Commas inside string
    /// literals are NOT supported (this is a v1 limitation — operators
    /// needing complex expressions can use multiple `--witnesses` flag
    /// invocations). Whitespace around commas and `=` is tolerated.
    pub fn parse_csv(spec: &str) -> Result<Vec<WitnessSpec>, String> {
        let mut out = Vec::new();
        for chunk in spec.split(',') {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }
            let (name, expr) = chunk.split_once('=').ok_or_else(|| {
                format!("witness `{chunk}` is missing `=` (expected name=expression)")
            })?;
            out.push(WitnessSpec::parse(name.trim(), expr.trim())?);
        }
        if out.is_empty() {
            return Err("no witnesses parsed from empty list".into());
        }
        Ok(out)
    }

    /// Underlying parsed expression — exposed for testing.
    pub fn check(&self, result: &WorkflowRunResult) -> bool {
        self.invariant.check(result)
    }

    pub fn source(&self) -> &str {
        self.invariant.source()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WitnessReport {
    pub name: String,
    pub source: String,
    pub trace_count: usize,
    pub total: usize,
    pub frequency: f64,
}

/// Aggregates per-witness hits across simulator traces.
pub struct WitnessTracker {
    specs: Vec<WitnessSpec>,
    counts: Vec<usize>,
    total: usize,
}

impl WitnessTracker {
    pub fn new(specs: Vec<WitnessSpec>) -> Self {
        let counts = vec![0usize; specs.len()];
        Self {
            specs,
            counts,
            total: 0,
        }
    }

    pub fn observe(&mut self, result: &WorkflowRunResult) {
        self.total += 1;
        for (i, spec) in self.specs.iter().enumerate() {
            if spec.check(result) {
                self.counts[i] += 1;
            }
        }
    }

    pub fn into_reports(self) -> Vec<WitnessReport> {
        let WitnessTracker {
            specs,
            counts,
            total,
        } = self;
        specs
            .into_iter()
            .zip(counts)
            .map(|(spec, count)| {
                let frequency = if total == 0 {
                    0.0
                } else {
                    count as f64 / total as f64
                };
                WitnessReport {
                    name: spec.name,
                    source: spec.invariant.source().to_string(),
                    trace_count: count,
                    total,
                    frequency,
                }
            })
            .collect()
    }
}

// From-impl for the SimulationError type — converts witness parse-failures
// at CLI-arg-parsing time into the top-level error.
impl From<InvariantParseError> for String {
    fn from(_: InvariantParseError) -> Self {
        // Unreachable: WitnessSpec::parse already wraps InvariantParseError
        // into String. This impl exists only to avoid a coherence headache
        // elsewhere — never called in practice.
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::definition::WorkflowStatus;
    use crate::workflow::WorkflowRunResult;
    use std::collections::BTreeMap;

    fn fixture(status: WorkflowStatus) -> WorkflowRunResult {
        WorkflowRunResult {
            run_id: "t".into(),
            workflow_name: "wt".into(),
            status,
            step_results: BTreeMap::new(),
            total_duration_ms: 0,
        }
    }

    #[test]
    fn parse_single_witness_ok() {
        let w = WitnessSpec::parse("happy_path", "status == \"completed\"").unwrap();
        assert_eq!(w.name, "happy_path");
        assert!(w.check(&fixture(WorkflowStatus::Completed)));
        assert!(!w.check(&fixture(WorkflowStatus::Failed)));
    }

    #[test]
    fn parse_witness_rejects_empty_name() {
        assert!(WitnessSpec::parse("", "status == \"completed\"").is_err());
    }

    #[test]
    fn parse_witness_rejects_name_with_equals() {
        assert!(WitnessSpec::parse("bad=name", "status == \"completed\"").is_err());
    }

    #[test]
    fn parse_witness_propagates_invariant_parse_error() {
        let err = WitnessSpec::parse("bad", "garbage").unwrap_err();
        assert!(err.contains("bad"));
    }

    #[test]
    fn parse_csv_two_witnesses() {
        let ws =
            WitnessSpec::parse_csv("ok=status == \"completed\",fail=status == \"failed\"").unwrap();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].name, "ok");
        assert_eq!(ws[1].name, "fail");
    }

    #[test]
    fn parse_csv_tolerates_whitespace() {
        let ws = WitnessSpec::parse_csv(" a = status == \"completed\" , b = status == \"failed\" ")
            .unwrap();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].name, "a");
        assert_eq!(ws[1].name, "b");
    }

    #[test]
    fn parse_csv_rejects_empty_input() {
        assert!(WitnessSpec::parse_csv("").is_err());
        assert!(WitnessSpec::parse_csv("   ").is_err());
    }

    #[test]
    fn parse_csv_rejects_chunk_without_equals() {
        assert!(WitnessSpec::parse_csv("just_a_name").is_err());
    }

    #[test]
    fn tracker_aggregates_counts_and_frequencies() {
        let w1 = WitnessSpec::parse("ok", "status == \"completed\"").unwrap();
        let w2 = WitnessSpec::parse("nope", "status == \"failed\"").unwrap();
        let mut t = WitnessTracker::new(vec![w1, w2]);
        t.observe(&fixture(WorkflowStatus::Completed));
        t.observe(&fixture(WorkflowStatus::Completed));
        t.observe(&fixture(WorkflowStatus::Failed));
        let reports = t.into_reports();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].name, "ok");
        assert_eq!(reports[0].trace_count, 2);
        assert_eq!(reports[0].total, 3);
        assert!((reports[0].frequency - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(reports[1].name, "nope");
        assert_eq!(reports[1].trace_count, 1);
        assert!((reports[1].frequency - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn tracker_zero_observations_yields_zero_frequency() {
        let w = WitnessSpec::parse("ok", "status == \"completed\"").unwrap();
        let t = WitnessTracker::new(vec![w]);
        let reports = t.into_reports();
        assert_eq!(reports[0].trace_count, 0);
        assert_eq!(reports[0].total, 0);
        assert_eq!(reports[0].frequency, 0.0);
    }

    #[test]
    fn report_serializes_to_json() {
        let w = WitnessSpec::parse("ok", "status == \"completed\"").unwrap();
        let mut t = WitnessTracker::new(vec![w]);
        t.observe(&fixture(WorkflowStatus::Completed));
        let reports = t.into_reports();
        let json = serde_json::to_string(&reports).unwrap();
        assert!(json.contains("\"name\":\"ok\""));
        assert!(json.contains("\"trace_count\":1"));
        assert!(json.contains("\"frequency\":1"));
    }
}
