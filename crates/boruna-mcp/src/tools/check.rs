use boruna_tooling::diagnostics::collector::DiagnosticCollector;
use boruna_tooling::repair::{RepairStrategy, RepairTool};

/// Run diagnostics on source and return structured JSON.
pub fn check_source(source: &str, file_name: &str) -> String {
    let collector = DiagnosticCollector::new(file_name, source);
    let ds = collector.collect();

    let diagnostics: Vec<serde_json::Value> = ds
        .diagnostics
        .iter()
        .map(|d| {
            let mut diag = serde_json::json!({
                "id": d.id,
                "severity": d.severity,
                "message": d.message,
            });
            if let Some(loc) = &d.location {
                diag["location"] = serde_json::json!({
                    "file": loc.file,
                    "line": loc.line,
                    "col": loc.col,
                    "end_line": loc.end_line,
                    "end_col": loc.end_col,
                });
            }
            if !d.suggested_patches.is_empty() {
                diag["patches"] = serde_json::json!(d
                    .suggested_patches
                    .iter()
                    .map(|p| serde_json::json!({
                        "id": p.id,
                        "description": p.description,
                        "confidence": p.confidence,
                        "rationale": p.rationale,
                    }))
                    .collect::<Vec<_>>());
            }
            diag
        })
        .collect();

    serde_json::json!({
        "success": true,
        "file": ds.file,
        "diagnostics_count": diagnostics.len(),
        "diagnostics": diagnostics,
    })
    .to_string()
}

/// Auto-repair source using diagnostic suggestions, returning repaired source and report.
pub fn repair_source(
    source: &str,
    file_name: &str,
    strategy: &str,
    patch_id: Option<&str>,
) -> String {
    let collector = DiagnosticCollector::new(file_name, source);
    let ds = collector.collect();

    let repair_strategy = match strategy {
        "all" => RepairStrategy::All,
        _ => {
            if patch_id.is_some() {
                RepairStrategy::ById
            } else {
                RepairStrategy::Best
            }
        }
    };

    let (repaired_source, result) =
        RepairTool::repair(file_name, source, &ds, repair_strategy, patch_id);

    let applied: Vec<serde_json::Value> = result
        .applied
        .iter()
        .map(|a| {
            serde_json::json!({
                "diagnostic_id": a.diagnostic_id,
                "patch_id": a.patch_id,
                "description": a.description,
            })
        })
        .collect();

    let skipped: Vec<serde_json::Value> = result
        .skipped
        .iter()
        .map(|s| {
            serde_json::json!({
                "diagnostic_id": s.diagnostic_id,
                "reason": s.reason,
            })
        })
        .collect();

    serde_json::json!({
        "success": true,
        "repaired_source": repaired_source,
        "patches_applied": applied.len(),
        "patches_skipped": skipped.len(),
        "applied": applied,
        "skipped": skipped,
        "verify_passed": result.verify_passed,
        "diagnostics_before": result.diagnostics_before,
        "diagnostics_after": result.diagnostics_after,
    })
    .to_string()
}
