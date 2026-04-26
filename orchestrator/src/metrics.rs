//! Prometheus metrics export from the persistent run store
//! (sprint `0.4-S12`).
//!
//! See `docs/design-prometheus-metrics.md` for the architectural
//! decision (CLI-pulled, not embedded HTTP) and operator integration
//! pattern (cron + `node_exporter`'s textfile collector).
//!
//! # Surface
//!
//! Three metric families are emitted:
//!
//! - `boruna_workflow_runs_total{workflow,status}` — counter of runs
//!   currently in each terminal/transient status.
//! - `boruna_workflow_runs_in_flight{workflow}` — gauge of `running` or
//!   `paused` runs.
//! - `boruna_workflow_step_completions_total{workflow,step,status}` —
//!   counter of step transitions to a terminal status (`completed` or
//!   `failed`). Non-terminal statuses (pending/running/awaiting_*) are
//!   not surfaced — they're operationally noisy and don't fit a
//!   counter semantic.
//!
//! # Counter semantics caveat
//!
//! Counters are computed from current store state at sample time, not
//! maintained as deltas. If old runs are pruned from the DB, the
//! `_total` will decrease — Prometheus normally treats this as a
//! counter reset and handles it gracefully via `rate()`. Operators
//! who run frequent pruning should be aware of this contract; see
//! the design doc for details.

use std::collections::BTreeMap;
use std::path::Path;

use crate::persistence::{RunCheckpointStore, RunStatus, StepStatus};
use crate::workflow::WorkflowRunError;

/// Aggregated metrics snapshot computed from the persistence store.
/// Pure data — formatting to Prometheus text is a separate function
/// so the snapshot can be reused by other exporters (e.g. JSON for a
/// future dashboard endpoint).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// `(workflow_name, status) -> count` for `boruna_workflow_runs_total`.
    pub runs_total: BTreeMap<(String, String), u64>,
    /// `workflow_name -> count` of runs in `running` or `paused` state.
    pub runs_in_flight: BTreeMap<String, u64>,
    /// `(workflow_name, step_id, status) -> count` of step terminal
    /// transitions across all runs.
    pub step_completions: BTreeMap<(String, String, String), u64>,
}

/// Compute a metrics snapshot from the persistent run store. Reads
/// all rows; the persistent store is bounded by run history retention.
/// For pathologically large stores (>100k runs) operators should
/// consider periodic pruning — see the design doc.
pub fn compute_snapshot(data_dir: &Path) -> Result<MetricsSnapshot, WorkflowRunError> {
    let store_path = data_dir.join("runs.db");
    let store = RunCheckpointStore::open(&store_path)
        .map_err(|e| WorkflowRunError::Persistence(e.to_string()))?;

    let runs = store
        .list_runs()
        .map_err(|e| WorkflowRunError::Persistence(e.to_string()))?;

    let mut snapshot = MetricsSnapshot::default();

    for run in &runs {
        // runs_total: increment counter keyed by (workflow_name, status).
        let status_str = run.status.as_str().to_string();
        *snapshot
            .runs_total
            .entry((run.workflow_name.clone(), status_str))
            .or_insert(0) += 1;

        // runs_in_flight: only `running` and `paused` runs.
        if matches!(run.status, RunStatus::Running | RunStatus::Paused) {
            *snapshot
                .runs_in_flight
                .entry(run.workflow_name.clone())
                .or_insert(0) += 1;
        }

        // step_completions: walk the run's checkpoints; only count
        // terminal statuses. Non-terminal step statuses (Pending,
        // Running, AwaitingApproval, AwaitingExternalEvent) don't fit
        // a counter semantic — they're transient state, not events.
        let cps = store
            .list_step_checkpoints(&run.run_id)
            .map_err(|e| WorkflowRunError::Persistence(e.to_string()))?;
        for cp in &cps {
            let step_status_str = match cp.status {
                StepStatus::Completed => "completed",
                StepStatus::Failed => "failed",
                _ => continue,
            };
            *snapshot
                .step_completions
                .entry((
                    run.workflow_name.clone(),
                    cp.step_id.clone(),
                    step_status_str.to_string(),
                ))
                .or_insert(0) += 1;
        }
    }

    // Ensure operators see explicit zero values for runs_in_flight on
    // workflows that DO appear in runs_total — otherwise a workflow
    // with no in-flight runs would emit no in_flight series at all,
    // and Prometheus would treat the absence as "no data" rather than
    // "zero." We populate the zero by walking workflow names that
    // appear in runs_total but not in runs_in_flight.
    let workflow_names: std::collections::BTreeSet<&str> = snapshot
        .runs_total
        .keys()
        .map(|(w, _)| w.as_str())
        .collect();
    for w in workflow_names {
        snapshot.runs_in_flight.entry(w.to_string()).or_insert(0);
    }

    Ok(snapshot)
}

/// Format a metrics snapshot as Prometheus text format (the
/// canonical output of an exporter's `/metrics` endpoint or a
/// `node_exporter` textfile collector input).
///
/// Output is deterministic: BTreeMap iteration order on labels is
/// lexicographic, so two calls with the same snapshot produce
/// byte-identical strings.
pub fn format_prometheus(snapshot: &MetricsSnapshot) -> String {
    let mut out = String::new();

    // boruna_workflow_runs_total
    out.push_str(
        "# HELP boruna_workflow_runs_total Total workflow runs by status (counter \
         computed from current store state — see docs/design-prometheus-metrics.md \
         for the counter-semantics caveat).\n",
    );
    out.push_str("# TYPE boruna_workflow_runs_total counter\n");
    for ((workflow, status), count) in &snapshot.runs_total {
        out.push_str(&format!(
            "boruna_workflow_runs_total{{workflow=\"{}\",status=\"{}\"}} {}\n",
            escape_label(workflow),
            escape_label(status),
            count
        ));
    }

    // boruna_workflow_runs_in_flight
    out.push_str("# HELP boruna_workflow_runs_in_flight Currently running or paused runs.\n");
    out.push_str("# TYPE boruna_workflow_runs_in_flight gauge\n");
    for (workflow, count) in &snapshot.runs_in_flight {
        out.push_str(&format!(
            "boruna_workflow_runs_in_flight{{workflow=\"{}\"}} {}\n",
            escape_label(workflow),
            count
        ));
    }

    // boruna_workflow_step_completions_total
    out.push_str(
        "# HELP boruna_workflow_step_completions_total Total step terminal \
         transitions by status.\n",
    );
    out.push_str("# TYPE boruna_workflow_step_completions_total counter\n");
    for ((workflow, step, status), count) in &snapshot.step_completions {
        out.push_str(&format!(
            "boruna_workflow_step_completions_total\
             {{workflow=\"{}\",step=\"{}\",status=\"{}\"}} {}\n",
            escape_label(workflow),
            escape_label(step),
            escape_label(status),
            count
        ));
    }

    out
}

/// Escape a Prometheus label value per the exposition format spec:
/// backslashes, double-quotes, and newlines get escaped. All other
/// characters pass through verbatim.
fn escape_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// Public entry for the `boruna metrics export` CLI handler.
/// Reads the persistent store, computes a snapshot, and renders to
/// Prometheus text format.
pub fn export(data_dir: &Path) -> Result<String, WorkflowRunError> {
    let snapshot = compute_snapshot(data_dir)?;
    Ok(format_prometheus(&snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_data_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn empty_store_emits_only_help_and_type_lines() {
        let dir = empty_data_dir();
        // Open the store to create the schema.
        let _ = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
        let out = export(dir.path()).unwrap();
        assert!(out.contains("# HELP boruna_workflow_runs_total"));
        assert!(out.contains("# TYPE boruna_workflow_runs_total counter"));
        assert!(out.contains("# HELP boruna_workflow_runs_in_flight"));
        assert!(out.contains("# TYPE boruna_workflow_runs_in_flight gauge"));
        assert!(out.contains("# HELP boruna_workflow_step_completions_total"));
        assert!(out.contains("# TYPE boruna_workflow_step_completions_total counter"));
        // No data lines.
        assert!(!out.contains("boruna_workflow_runs_total{"));
        assert!(!out.contains("boruna_workflow_runs_in_flight{"));
        assert!(!out.contains("boruna_workflow_step_completions_total{"));
    }

    #[test]
    fn compute_snapshot_aggregates_runs_by_workflow_and_status() {
        use crate::persistence::RunRow;
        let dir = empty_data_dir();
        let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
        // Two runs of "wf-a", one Completed one Failed; one run of
        // "wf-b" Completed.
        for (run_id, workflow_name, status) in &[
            ("r1", "wf-a", RunStatus::Completed),
            ("r2", "wf-a", RunStatus::Failed),
            ("r3", "wf-b", RunStatus::Completed),
        ] {
            store
                .insert_run(&RunRow {
                    run_id: run_id.to_string(),
                    workflow_name: workflow_name.to_string(),
                    workflow_hash: "deadbeef".into(),
                    status: *status,
                    started_at_ms: 1_700_000_000_000,
                    updated_at_ms: 1_700_000_000_000,
                    policy_json: "{}".into(),
                    metadata_json: "{}".into(),
                })
                .unwrap();
        }
        drop(store);

        let snap = compute_snapshot(dir.path()).unwrap();
        assert_eq!(
            snap.runs_total
                .get(&("wf-a".to_string(), "completed".to_string())),
            Some(&1)
        );
        assert_eq!(
            snap.runs_total
                .get(&("wf-a".to_string(), "failed".to_string())),
            Some(&1)
        );
        assert_eq!(
            snap.runs_total
                .get(&("wf-b".to_string(), "completed".to_string())),
            Some(&1)
        );
        // No paused/running runs.
        assert_eq!(snap.runs_in_flight.get("wf-a"), Some(&0));
        assert_eq!(snap.runs_in_flight.get("wf-b"), Some(&0));
    }

    #[test]
    fn compute_snapshot_counts_in_flight_runs() {
        use crate::persistence::RunRow;
        let dir = empty_data_dir();
        let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
        for (run_id, status) in &[
            ("r1", RunStatus::Running),
            ("r2", RunStatus::Paused),
            ("r3", RunStatus::Completed),
        ] {
            store
                .insert_run(&RunRow {
                    run_id: run_id.to_string(),
                    workflow_name: "wf".into(),
                    workflow_hash: "x".into(),
                    status: *status,
                    started_at_ms: 1_700_000_000_000,
                    updated_at_ms: 1_700_000_000_000,
                    policy_json: "{}".into(),
                    metadata_json: "{}".into(),
                })
                .unwrap();
        }
        drop(store);

        let snap = compute_snapshot(dir.path()).unwrap();
        // 1 Running + 1 Paused = 2 in-flight.
        assert_eq!(snap.runs_in_flight.get("wf"), Some(&2));
    }

    #[test]
    fn compute_snapshot_counts_terminal_step_transitions() {
        use crate::persistence::{RunRow, StepCheckpoint};
        let dir = empty_data_dir();
        let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
        store
            .insert_run(&RunRow {
                run_id: "r1".into(),
                workflow_name: "wf".into(),
                workflow_hash: "x".into(),
                status: RunStatus::Completed,
                started_at_ms: 0,
                updated_at_ms: 0,
                policy_json: "{}".into(),
                metadata_json: "{}".into(),
            })
            .unwrap();
        for (step_id, status) in &[
            ("s1", StepStatus::Completed),
            ("s2", StepStatus::Failed),
            // Pending must be ignored — non-terminal.
            ("s3", StepStatus::Pending),
        ] {
            store
                .upsert_step_checkpoint(&StepCheckpoint {
                    run_id: "r1".into(),
                    step_id: step_id.to_string(),
                    status: *status,
                    output_json: None,
                    output_hash: None,
                    started_at_ms: Some(0),
                    ended_at_ms: None,
                    error_msg: None,
                    attempt_count: 1,
                })
                .unwrap();
        }
        drop(store);

        let snap = compute_snapshot(dir.path()).unwrap();
        assert_eq!(
            snap.step_completions.get(&(
                "wf".to_string(),
                "s1".to_string(),
                "completed".to_string()
            )),
            Some(&1)
        );
        assert_eq!(
            snap.step_completions
                .get(&("wf".to_string(), "s2".to_string(), "failed".to_string())),
            Some(&1)
        );
        // s3 (Pending) NOT in the map.
        assert!(!snap.step_completions.contains_key(&(
            "wf".to_string(),
            "s3".to_string(),
            "pending".to_string()
        )));
    }

    #[test]
    fn format_prometheus_output_is_valid_textfile_format() {
        let mut snap = MetricsSnapshot::default();
        snap.runs_total
            .insert(("wf".to_string(), "completed".to_string()), 5);
        snap.runs_in_flight.insert("wf".to_string(), 2);
        snap.step_completions.insert(
            ("wf".to_string(), "s1".to_string(), "completed".to_string()),
            10,
        );

        let out = format_prometheus(&snap);

        // Each metric must have HELP + TYPE before its data lines.
        let lines: Vec<&str> = out.lines().collect();
        let runs_help_idx = lines
            .iter()
            .position(|l| l.starts_with("# HELP boruna_workflow_runs_total"))
            .unwrap();
        let runs_type_idx = lines
            .iter()
            .position(|l| l.starts_with("# TYPE boruna_workflow_runs_total"))
            .unwrap();
        let runs_data_idx = lines
            .iter()
            .position(|l| l.starts_with("boruna_workflow_runs_total{"))
            .unwrap();
        assert!(runs_help_idx < runs_type_idx);
        assert!(runs_type_idx < runs_data_idx);

        // Data line shape.
        assert!(out.contains("boruna_workflow_runs_total{workflow=\"wf\",status=\"completed\"} 5"));
        assert!(out.contains("boruna_workflow_runs_in_flight{workflow=\"wf\"} 2"));
        assert!(out.contains(
            "boruna_workflow_step_completions_total{workflow=\"wf\",step=\"s1\",status=\"completed\"} 10"
        ));
    }

    #[test]
    fn format_prometheus_is_deterministic_across_calls() {
        // Two calls with the same snapshot must produce byte-identical
        // output. BTreeMap iteration guarantees this; we lock it in
        // a test so a future refactor to HashMap can't quietly
        // introduce non-determinism.
        let mut snap = MetricsSnapshot::default();
        snap.runs_total
            .insert(("wf-z".to_string(), "completed".to_string()), 1);
        snap.runs_total
            .insert(("wf-a".to_string(), "completed".to_string()), 1);
        snap.runs_total
            .insert(("wf-m".to_string(), "failed".to_string()), 1);

        let out1 = format_prometheus(&snap);
        let out2 = format_prometheus(&snap);
        assert_eq!(out1, out2);

        // Sort order is lexicographic by workflow then status.
        let pos_a = out1.find("workflow=\"wf-a\"").unwrap();
        let pos_m = out1.find("workflow=\"wf-m\"").unwrap();
        let pos_z = out1.find("workflow=\"wf-z\"").unwrap();
        assert!(pos_a < pos_m);
        assert!(pos_m < pos_z);
    }

    #[test]
    fn format_prometheus_escapes_label_values() {
        let mut snap = MetricsSnapshot::default();
        // A workflow name with a backslash and a quote — both must
        // be escaped per the exposition format spec.
        snap.runs_total.insert(
            (r#"weird\name"with""#.to_string(), "completed".to_string()),
            1,
        );
        let out = format_prometheus(&snap);
        // \\\\ in source = \\ on the wire (escaped backslash).
        // \\\" in source = \" on the wire (escaped quote).
        assert!(out.contains(
            r#"boruna_workflow_runs_total{workflow="weird\\name\"with\"",status="completed"} 1"#
        ));
    }

    #[test]
    fn export_returns_well_formed_prometheus_text_for_realistic_run_set() {
        // Integration: insert 2 runs + step checkpoints and confirm
        // the exported text contains all expected series.
        use crate::persistence::{RunRow, StepCheckpoint};
        let dir = empty_data_dir();
        let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
        store
            .insert_run(&RunRow {
                run_id: "r1".into(),
                workflow_name: "etl".into(),
                workflow_hash: "x".into(),
                status: RunStatus::Completed,
                started_at_ms: 0,
                updated_at_ms: 0,
                policy_json: "{}".into(),
                metadata_json: "{}".into(),
            })
            .unwrap();
        store
            .insert_run(&RunRow {
                run_id: "r2".into(),
                workflow_name: "etl".into(),
                workflow_hash: "x".into(),
                status: RunStatus::Paused,
                started_at_ms: 0,
                updated_at_ms: 0,
                policy_json: "{}".into(),
                metadata_json: "{}".into(),
            })
            .unwrap();
        store
            .upsert_step_checkpoint(&StepCheckpoint {
                run_id: "r1".into(),
                step_id: "extract".into(),
                status: StepStatus::Completed,
                output_json: None,
                output_hash: None,
                started_at_ms: Some(0),
                ended_at_ms: None,
                error_msg: None,
                attempt_count: 1,
            })
            .unwrap();
        drop(store);

        let out = export(dir.path()).unwrap();
        assert!(out.contains("boruna_workflow_runs_total{workflow=\"etl\",status=\"completed\"} 1"));
        assert!(out.contains("boruna_workflow_runs_total{workflow=\"etl\",status=\"paused\"} 1"));
        assert!(out.contains("boruna_workflow_runs_in_flight{workflow=\"etl\"} 1"));
        assert!(out.contains(
            "boruna_workflow_step_completions_total{workflow=\"etl\",step=\"extract\",status=\"completed\"} 1"
        ));
    }
}
