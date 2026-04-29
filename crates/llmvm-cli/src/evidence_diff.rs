use boruna_orchestrator::audit::{evidence::BundleManifest, log::AuditLog, verify::verify_bundle};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct FieldDiff<T: Serialize> {
    a: T,
    b: T,
    same: bool,
}

impl<T: Serialize + PartialEq> FieldDiff<T> {
    fn new(a: T, b: T) -> Self {
        let same = a == b;
        FieldDiff { a, b, same }
    }
}

#[derive(Debug, Serialize)]
pub struct StepDiff {
    step: String,
    same: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    a: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    b: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiffReport {
    same: bool,
    workflow_name: FieldDiff<String>,
    step_count: FieldDiff<usize>,
    step_outputs: Vec<StepDiff>,
    audit_event_count: FieldDiff<usize>,
    verification: VerificationStatus,
}

#[derive(Debug, Serialize)]
pub struct VerificationStatus {
    a: String,
    b: String,
}

/// Load a bundle's manifest from `dir/manifest.json`.
fn load_manifest(dir: &Path) -> Result<BundleManifest, String> {
    let path = dir.join("manifest.json");
    let json = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&json).map_err(|e| format!("invalid manifest.json: {e}"))
}

/// Load a bundle's audit log from `dir/audit_log.json`.
fn load_audit_log(dir: &Path) -> Result<AuditLog, String> {
    let path = dir.join("audit_log.json");
    let json = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    AuditLog::from_json(&json).map_err(|e| format!("invalid audit_log.json: {e}"))
}

/// Read the step output files from `dir/outputs/` into a BTreeMap keyed by
/// step_id. Each value is the raw JSON string of the first output file found
/// under `outputs/<step_id>/`.
fn load_step_outputs(dir: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let outputs_dir = dir.join("outputs");
    let read_dir = match std::fs::read_dir(&outputs_dir) {
        Ok(rd) => rd,
        Err(_) => return map,
    };
    for step_entry in read_dir.flatten() {
        let step_id = step_entry.file_name().to_string_lossy().to_string();
        let step_path = step_entry.path();
        if let Ok(files) = std::fs::read_dir(&step_path) {
            for file_entry in files.flatten() {
                if let Ok(content) = std::fs::read_to_string(file_entry.path()) {
                    map.insert(step_id.clone(), content);
                    break; // first output file per step
                }
            }
        }
    }
    map
}

/// Build a diff report from two bundle directories, comparing both manifests
/// and on-disk step outputs.
pub fn build_diff(dir_a: &Path, dir_b: &Path) -> Result<DiffReport, Box<dyn std::error::Error>> {
    let manifest_a = load_manifest(dir_a).map_err(|e| e.as_str().to_string())?;
    let manifest_b = load_manifest(dir_b).map_err(|e| e.as_str().to_string())?;

    let audit_a = load_audit_log(dir_a).map_err(|e| e.as_str().to_string())?;
    let audit_b = load_audit_log(dir_b).map_err(|e| e.as_str().to_string())?;

    let outputs_a = load_step_outputs(dir_a);
    let outputs_b = load_step_outputs(dir_b);

    // Collect all step IDs from both bundles (file_checksums prefix "outputs/<id>/")
    let mut step_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for key in manifest_a.file_checksums.keys() {
        if let Some(rest) = key.strip_prefix("outputs/") {
            if let Some(step_id) = rest.split('/').next() {
                step_ids.insert(step_id.to_string());
            }
        }
    }
    for key in manifest_b.file_checksums.keys() {
        if let Some(rest) = key.strip_prefix("outputs/") {
            if let Some(step_id) = rest.split('/').next() {
                step_ids.insert(step_id.to_string());
            }
        }
    }

    let step_count_a = step_ids
        .iter()
        .filter(|s| {
            outputs_a.contains_key(*s)
                || manifest_a
                    .file_checksums
                    .keys()
                    .any(|k| k.starts_with(&format!("outputs/{s}/")))
        })
        .count();
    let step_count_b = step_ids
        .iter()
        .filter(|s| {
            outputs_b.contains_key(*s)
                || manifest_b
                    .file_checksums
                    .keys()
                    .any(|k| k.starts_with(&format!("outputs/{s}/")))
        })
        .count();

    let mut step_diffs: Vec<StepDiff> = Vec::new();
    for step_id in &step_ids {
        let val_a = outputs_a.get(step_id);
        let val_b = outputs_b.get(step_id);
        let same = val_a == val_b;
        step_diffs.push(StepDiff {
            step: step_id.clone(),
            same,
            a: if same {
                None
            } else {
                Some(val_a.cloned().unwrap_or_default())
            },
            b: if same {
                None
            } else {
                Some(val_b.cloned().unwrap_or_default())
            },
        });
    }

    let verify_a = verify_bundle(dir_a);
    let verify_b = verify_bundle(dir_b);

    let verification = VerificationStatus {
        a: if verify_a.valid {
            "pass".to_string()
        } else {
            "fail".to_string()
        },
        b: if verify_b.valid {
            "pass".to_string()
        } else {
            "fail".to_string()
        },
    };

    let workflow_diff = FieldDiff::new(
        manifest_a.workflow_name.clone(),
        manifest_b.workflow_name.clone(),
    );
    let step_count_diff = FieldDiff::new(step_count_a, step_count_b);
    let event_count_diff = FieldDiff::new(audit_a.entries().len(), audit_b.entries().len());

    let all_same = workflow_diff.same
        && step_count_diff.same
        && step_diffs.iter().all(|s| s.same)
        && event_count_diff.same;

    Ok(DiffReport {
        same: all_same,
        workflow_name: workflow_diff,
        step_count: step_count_diff,
        step_outputs: step_diffs,
        audit_event_count: event_count_diff,
        verification,
    })
}

/// Print a human-readable diff.
pub fn print_diff(dir_a: &Path, dir_b: &Path, report: &DiffReport) {
    println!("Bundle diff: {} vs {}", dir_a.display(), dir_b.display());
    println!();

    println!(
        "Workflow: {} ({})",
        report.workflow_name.a,
        if report.workflow_name.same {
            "same".to_string()
        } else {
            format!("CHANGED → {}", report.workflow_name.b)
        }
    );
    println!(
        "Steps: {} ({})",
        report.step_count.a,
        if report.step_count.same {
            "same".to_string()
        } else {
            format!("CHANGED — bundle-b has {}", report.step_count.b)
        }
    );
    println!();

    if report.step_outputs.is_empty() {
        println!("Step outputs: (none)");
    } else {
        println!("Step outputs:");
        for s in &report.step_outputs {
            if s.same {
                println!("  {} : SAME", s.step);
            } else {
                println!("  {} : CHANGED", s.step);
                if let Some(a) = &s.a {
                    for line in a.lines() {
                        println!("    - {line}");
                    }
                }
                if let Some(b) = &s.b {
                    for line in b.lines() {
                        println!("    + {line}");
                    }
                }
            }
        }
    }
    println!();

    let ea = report.audit_event_count.a;
    let eb = report.audit_event_count.b;
    let event_label = if report.audit_event_count.same {
        format!("{ea}")
    } else if eb > ea {
        format!(
            "{ea} vs {eb} (DIFFERENT — {} extra events in bundle-b)",
            eb - ea
        )
    } else {
        format!(
            "{ea} vs {eb} (DIFFERENT — {} fewer events in bundle-b)",
            ea - eb
        )
    };
    println!("Audit log:");
    println!("  Events: {event_label}");
    println!();

    println!("Verification:");
    println!("  bundle-a: {}", report.verification.a.to_uppercase());
    println!("  bundle-b: {}", report.verification.b.to_uppercase());
}

pub fn evidence_diff(
    dir_a: &Path,
    dir_b: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let report = build_diff(dir_a, dir_b)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_diff(dir_a, dir_b, &report);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use boruna_orchestrator::audit::{
        evidence::EvidenceBundleBuilder,
        log::{AuditEvent, AuditLog},
    };
    use std::path::PathBuf;

    fn make_bundle(
        base: &Path,
        run_id: &str,
        workflow_name: &str,
        step_output: &str,
        extra_event: bool,
    ) -> PathBuf {
        let mut builder = EvidenceBundleBuilder::new(base, run_id, workflow_name).unwrap();
        builder
            .add_workflow_def(&format!(r#"{{"name":"{workflow_name}"}}"#))
            .unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        builder
            .add_step_output("step1", "result", step_output)
            .unwrap();

        let mut log = AuditLog::new();
        log.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        log.append(AuditEvent::StepStarted {
            step_id: "step1".into(),
            input_hash: "in1".into(),
        });
        log.append(AuditEvent::StepCompleted {
            step_id: "step1".into(),
            output_hash: "out1".into(),
            duration_ms: 10,
        });
        if extra_event {
            log.append(AuditEvent::PolicyEvaluated {
                step_id: "step1".into(),
                rule: "allow-all".into(),
                decision: "allow".into(),
            });
        }
        log.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 50,
        });

        builder.finalize(&log).unwrap();
        base.join(run_id)
    }

    #[test]
    fn diff_identical_bundles_reports_same() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_bundle(dir.path(), "run-a", "my_workflow", r#"{"v":1}"#, false);
        let b = make_bundle(dir.path(), "run-b", "my_workflow", r#"{"v":1}"#, false);

        let report = build_diff(&a, &b).unwrap();
        assert!(report.same, "identical bundles must report same=true");
        assert!(report.workflow_name.same);
        assert!(report.audit_event_count.same);
        assert!(report.step_outputs.iter().all(|s| s.same));
    }

    #[test]
    fn diff_different_step_outputs_reports_changed() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_bundle(dir.path(), "run-a2", "wf", r#"{"v":1}"#, false);
        let b = make_bundle(dir.path(), "run-b2", "wf", r#"{"v":99}"#, false);

        let report = build_diff(&a, &b).unwrap();
        assert!(!report.same);
        let step1 = report
            .step_outputs
            .iter()
            .find(|s| s.step == "step1")
            .unwrap();
        assert!(!step1.same);
        assert!(step1.a.as_deref().unwrap().contains("1"));
        assert!(step1.b.as_deref().unwrap().contains("99"));
    }

    #[test]
    fn diff_different_event_counts_reports_different() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_bundle(dir.path(), "run-a3", "wf", r#"{"v":1}"#, false);
        let b = make_bundle(dir.path(), "run-b3", "wf", r#"{"v":1}"#, true);

        let report = build_diff(&a, &b).unwrap();
        assert!(!report.audit_event_count.same);
        assert_eq!(report.audit_event_count.b - report.audit_event_count.a, 1);
    }

    #[test]
    fn diff_json_output_is_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let a = make_bundle(dir.path(), "run-a4", "wf", r#"{"v":1}"#, false);
        let b = make_bundle(dir.path(), "run-b4", "wf", r#"{"v":2}"#, false);

        let report = build_diff(&a, &b).unwrap();
        let json_str = serde_json::to_string_pretty(&report).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.get("same").is_some());
        assert!(parsed.get("step_outputs").is_some());
        assert!(parsed.get("audit_event_count").is_some());
        assert!(parsed.get("verification").is_some());
    }
}
