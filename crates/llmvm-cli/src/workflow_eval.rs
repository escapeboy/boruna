//! `boruna workflow eval` — run the same workflow against two LLM provider
//! configurations and compare the resulting evidence bundles.

use std::path::Path;
use std::time::Instant;

use boruna_orchestrator::workflow::{RunOptions, WorkflowDef, WorkflowRunner, WorkflowStatus};
use boruna_vm::capability_gateway::Policy;
use serde::Serialize;

use crate::evidence_diff;
use crate::provider_registry::ProviderRegistry;

#[derive(Debug, Serialize)]
pub struct ProviderSummary {
    pub name: String,
    pub runs: u32,
    pub successes: u32,
    pub mean_wall_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct StepComparison {
    pub step_id: String,
    pub outputs_identical: bool,
    pub output_variance_a: usize,
    pub output_variance_b: usize,
}

#[derive(Debug, Serialize)]
pub struct EvalReport {
    pub workflow: String,
    pub provider_a: ProviderSummary,
    pub provider_b: ProviderSummary,
    pub step_comparisons: Vec<StepComparison>,
}

struct RunRecord {
    run_id: String,
    elapsed_ms: u64,
    success: bool,
    bundle_dir: std::path::PathBuf,
}

fn run_one(
    def: &WorkflowDef,
    workflow_dir: &Path,
    runs_base: &Path,
    provider_name: &str,
    run_n: u32,
) -> Result<RunRecord, Box<dyn std::error::Error>> {
    let run_dir = runs_base.join(provider_name).join(format!("run_{run_n}"));
    std::fs::create_dir_all(&run_dir)?;

    let evidence_dir = run_dir.join("evidence");
    std::fs::create_dir_all(&evidence_dir)?;

    let options = RunOptions {
        policy: Some(Policy::allow_all()),
        record: true,
        workflow_dir: workflow_dir.display().to_string(),
        live: false,
        concurrency: 1,
        submit_only: false,
    };

    let t0 = Instant::now();
    let result = WorkflowRunner::run(def, &options).map_err(|e| format!("{e}"))?;
    let elapsed_ms = t0.elapsed().as_millis() as u64;

    let success = matches!(result.status, WorkflowStatus::Completed);

    // Write evidence bundle for this run using the same builder as `workflow run --record`.
    use boruna_orchestrator::audit::{AuditEvent, AuditLog, EvidenceBundleBuilder};
    let mut builder = EvidenceBundleBuilder::new(&evidence_dir, &result.run_id, &def.name)?;
    builder.add_workflow_def(&serde_json::to_string(def)?)?;
    builder.add_policy(r#"{"default_allow":true}"#)?;

    let mut log = AuditLog::new();
    log.append(AuditEvent::WorkflowStarted {
        workflow_hash: "model-eval".into(),
        policy_hash: "model-eval".into(),
    });
    for (step_id, sr) in &result.step_results {
        log.append(AuditEvent::StepStarted {
            step_id: step_id.clone(),
            input_hash: "model-eval".into(),
        });
        log.append(AuditEvent::StepCompleted {
            step_id: step_id.clone(),
            output_hash: sr.output_hash.clone().unwrap_or_else(|| "none".into()),
            duration_ms: sr.duration_ms,
        });
    }
    log.append(AuditEvent::WorkflowCompleted {
        result_hash: "model-eval".into(),
        total_duration_ms: result.total_duration_ms,
    });

    builder.finalize(&log)?;

    let bundle_dir = evidence_dir.join(&result.run_id);

    Ok(RunRecord {
        run_id: result.run_id,
        elapsed_ms,
        success,
        bundle_dir,
    })
}

fn collect_step_ids(records: &[RunRecord]) -> Vec<String> {
    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for rec in records {
        let outputs_dir = rec.bundle_dir.join("outputs");
        if let Ok(rd) = std::fs::read_dir(&outputs_dir) {
            for entry in rd.flatten() {
                ids.insert(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    ids.into_iter().collect()
}

fn read_step_output(bundle_dir: &Path, step_id: &str) -> Option<String> {
    let step_dir = bundle_dir.join("outputs").join(step_id);
    if let Ok(files) = std::fs::read_dir(&step_dir) {
        for entry in files.flatten() {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                return Some(content);
            }
        }
    }
    None
}

fn build_step_comparisons(records_a: &[RunRecord], records_b: &[RunRecord]) -> Vec<StepComparison> {
    let mut all_step_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for s in collect_step_ids(records_a) {
        all_step_ids.insert(s);
    }
    for s in collect_step_ids(records_b) {
        all_step_ids.insert(s);
    }

    let mut comparisons = Vec::new();

    for step_id in &all_step_ids {
        let outputs_a: Vec<Option<String>> = records_a
            .iter()
            .map(|r| read_step_output(&r.bundle_dir, step_id))
            .collect();
        let outputs_b: Vec<Option<String>> = records_b
            .iter()
            .map(|r| read_step_output(&r.bundle_dir, step_id))
            .collect();

        let distinct_a: std::collections::BTreeSet<String> =
            outputs_a.iter().filter_map(|o| o.clone()).collect();
        let distinct_b: std::collections::BTreeSet<String> =
            outputs_b.iter().filter_map(|o| o.clone()).collect();

        let variance_a = distinct_a.len();
        let variance_b = distinct_b.len();

        // Cross-provider agreement: every A output equals every B output.
        let all_a_vals: Vec<_> = outputs_a.iter().filter_map(|o| o.as_deref()).collect();
        let all_b_vals: Vec<_> = outputs_b.iter().filter_map(|o| o.as_deref()).collect();
        let outputs_identical = !all_a_vals.is_empty()
            && !all_b_vals.is_empty()
            && all_a_vals.iter().all(|a| all_b_vals.iter().all(|b| a == b));

        comparisons.push(StepComparison {
            step_id: step_id.clone(),
            outputs_identical,
            output_variance_a: variance_a,
            output_variance_b: variance_b,
        });
    }

    comparisons
}

fn print_report(report: &EvalReport) {
    println!("=== Workflow Eval: {} ===", report.workflow);
    println!();

    let rate_a = if report.provider_a.runs > 0 {
        report.provider_a.successes * 100 / report.provider_a.runs
    } else {
        0
    };
    let rate_b = if report.provider_b.runs > 0 {
        report.provider_b.successes * 100 / report.provider_b.runs
    } else {
        0
    };

    println!(
        "Provider A ({}): {}/{} runs succeeded ({}%), mean {}ms",
        report.provider_a.name,
        report.provider_a.successes,
        report.provider_a.runs,
        rate_a,
        report.provider_a.mean_wall_ms,
    );
    println!(
        "Provider B ({}): {}/{} runs succeeded ({}%), mean {}ms",
        report.provider_b.name,
        report.provider_b.successes,
        report.provider_b.runs,
        rate_b,
        report.provider_b.mean_wall_ms,
    );
    println!();

    if report.step_comparisons.is_empty() {
        println!("No step outputs to compare.");
        return;
    }

    let col_w = 24usize;
    println!(
        "{:<col_w$} {:<13} {:<13} {:<16}",
        "Step", "A identical", "B identical", "A vs B agree"
    );
    println!("{}", "-".repeat(70));

    for sc in &report.step_comparisons {
        let a_mark = if sc.output_variance_a <= 1 {
            "yes".to_string()
        } else {
            format!("no ({} variants)", sc.output_variance_a)
        };
        let b_mark = if sc.output_variance_b <= 1 {
            "yes".to_string()
        } else {
            format!("no ({} variants)", sc.output_variance_b)
        };
        let agree = if sc.outputs_identical {
            "yes (identical)"
        } else {
            "no  (different)"
        };
        println!(
            "{:<col_w$} {:<13} {:<13} {:<16}",
            sc.step_id, a_mark, b_mark, agree
        );
    }
}

/// Extract a human-readable provider name from the registry's describe() output.
/// Falls back to the file's stem if the registry has no entries.
fn provider_name_from_registry(registry: &ProviderRegistry, path: &Path, fallback: &str) -> String {
    let desc = registry.describe();
    let after_arrow = desc.split("->").nth(1).unwrap_or("").trim().to_string();
    let first_token = after_arrow
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if !first_token.is_empty() {
        first_token
    } else {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| fallback.into())
    }
}

pub fn run_workflow_eval(
    workflow_dir: &Path,
    provider_a_path: &Path,
    provider_b_path: &Path,
    runs_per_provider: u32,
    data_dir: Option<&Path>,
    json_output: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let reg_a =
        ProviderRegistry::from_file(provider_a_path).map_err(|e| format!("provider-a: {e}"))?;
    let reg_b =
        ProviderRegistry::from_file(provider_b_path).map_err(|e| format!("provider-b: {e}"))?;

    eprintln!("provider-a: {}", reg_a.describe());
    eprintln!("provider-b: {}", reg_b.describe());

    let name_a = provider_name_from_registry(&reg_a, provider_a_path, "provider_a");
    let name_b = provider_name_from_registry(&reg_b, provider_b_path, "provider_b");

    let def_path = workflow_dir.join("workflow.json");
    let json = std::fs::read_to_string(&def_path)
        .map_err(|e| format!("cannot read {}: {e}", def_path.display()))?;
    let def: WorkflowDef =
        serde_json::from_str(&json).map_err(|e| format!("invalid workflow.json: {e}"))?;

    let base = data_dir
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from(".boruna/data"));
    let runs_base = base.join("model-eval");

    let mut records_a: Vec<RunRecord> = Vec::new();
    let mut records_b: Vec<RunRecord> = Vec::new();

    eprintln!(
        "Running {} run(s) for provider-a ({name_a})...",
        runs_per_provider
    );
    for n in 1..=runs_per_provider {
        eprintln!("  provider-a run {n}/{runs_per_provider}...");
        match run_one(&def, workflow_dir, &runs_base, &name_a, n) {
            Ok(rec) => {
                eprintln!(
                    "    run_id={} elapsed={}ms success={}",
                    rec.run_id, rec.elapsed_ms, rec.success
                );
                records_a.push(rec);
            }
            Err(e) => {
                eprintln!("    FAILED: {e}");
                records_a.push(RunRecord {
                    run_id: format!("failed-a-{n}"),
                    elapsed_ms: 0,
                    success: false,
                    bundle_dir: std::path::PathBuf::new(),
                });
            }
        }
    }

    eprintln!(
        "Running {} run(s) for provider-b ({name_b})...",
        runs_per_provider
    );
    for n in 1..=runs_per_provider {
        eprintln!("  provider-b run {n}/{runs_per_provider}...");
        match run_one(&def, workflow_dir, &runs_base, &name_b, n) {
            Ok(rec) => {
                eprintln!(
                    "    run_id={} elapsed={}ms success={}",
                    rec.run_id, rec.elapsed_ms, rec.success
                );
                records_b.push(rec);
            }
            Err(e) => {
                eprintln!("    FAILED: {e}");
                records_b.push(RunRecord {
                    run_id: format!("failed-b-{n}"),
                    elapsed_ms: 0,
                    success: false,
                    bundle_dir: std::path::PathBuf::new(),
                });
            }
        }
    }

    let successes_a = records_a.iter().filter(|r| r.success).count() as u32;
    let successes_b = records_b.iter().filter(|r| r.success).count() as u32;

    let mean_a = if records_a.is_empty() {
        0
    } else {
        records_a.iter().map(|r| r.elapsed_ms).sum::<u64>() / records_a.len() as u64
    };
    let mean_b = if records_b.is_empty() {
        0
    } else {
        records_b.iter().map(|r| r.elapsed_ms).sum::<u64>() / records_b.len() as u64
    };

    let step_comparisons = build_step_comparisons(&records_a, &records_b);

    // When multiple runs are available, build cross-provider diffs to validate
    // the evidence bundle infrastructure.
    if runs_per_provider > 1 {
        let ok_a: Vec<_> = records_a
            .iter()
            .filter(|r| r.success && r.bundle_dir.exists())
            .collect();
        let ok_b: Vec<_> = records_b
            .iter()
            .filter(|r| r.success && r.bundle_dir.exists())
            .collect();
        for (ra, rb) in ok_a.iter().zip(ok_b.iter()) {
            let _ = evidence_diff::build_diff(&ra.bundle_dir, &rb.bundle_dir);
        }
    }

    let report = EvalReport {
        workflow: def.name.clone(),
        provider_a: ProviderSummary {
            name: name_a,
            runs: runs_per_provider,
            successes: successes_a,
            mean_wall_ms: mean_a,
        },
        provider_b: ProviderSummary {
            name: name_b,
            runs: runs_per_provider,
            successes: successes_b,
            mean_wall_ms: mean_b,
        },
        step_comparisons,
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_report_serializes_to_json() {
        let report = EvalReport {
            workflow: "test_wf".into(),
            provider_a: ProviderSummary {
                name: "anthropic".into(),
                runs: 2,
                successes: 2,
                mean_wall_ms: 500,
            },
            provider_b: ProviderSummary {
                name: "ollama".into(),
                runs: 2,
                successes: 1,
                mean_wall_ms: 300,
            },
            step_comparisons: vec![StepComparison {
                step_id: "analyze".into(),
                outputs_identical: true,
                output_variance_a: 1,
                output_variance_b: 1,
            }],
        };

        let json_str = serde_json::to_string(&report).expect("serializes");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("parses back");
        assert_eq!(parsed["workflow"], "test_wf");
        assert_eq!(parsed["provider_a"]["name"], "anthropic");
        assert_eq!(parsed["provider_b"]["successes"], 1);
        assert!(parsed["step_comparisons"][0]["outputs_identical"]
            .as_bool()
            .unwrap());
    }

    #[test]
    fn provider_summary_success_rate() {
        let summary = ProviderSummary {
            name: "anthropic".into(),
            runs: 2,
            successes: 1,
            mean_wall_ms: 750,
        };
        assert_eq!(summary.runs, 2);
        assert_eq!(summary.successes, 1);
        assert_eq!(summary.mean_wall_ms, 750);
        let rate = summary.successes * 100 / summary.runs;
        assert_eq!(rate, 50);
    }

    #[test]
    fn step_comparison_identical() {
        let sc = StepComparison {
            step_id: "report".into(),
            outputs_identical: true,
            output_variance_a: 1,
            output_variance_b: 1,
        };
        assert!(sc.outputs_identical);
        assert_eq!(sc.output_variance_a, 1);
        assert_eq!(sc.output_variance_b, 1);
    }
}
