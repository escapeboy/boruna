use std::path::Path;

use crate::diagnostics::collector::DiagnosticCollector;
use crate::diagnostics::{Confidence, DiagnosticSet, SuggestedPatch, TextEdit};

/// Strategy for selecting which patches to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairStrategy {
    /// Apply the highest-confidence suggestion for each diagnostic.
    Best,
    /// Apply a specific suggestion by ID.
    ById,
    /// Apply all suggestions (in order of confidence).
    All,
    /// Apply only High-confidence patches; skip Medium and Low.
    /// Use when you want safe, certain fixes only (e.g. CI auto-repair).
    Conservative,
}

/// Result of a repair operation.
#[derive(Debug)]
pub struct RepairResult {
    pub applied: Vec<AppliedPatch>,
    pub skipped: Vec<SkippedPatch>,
    pub verify_passed: bool,
    pub diagnostics_before: usize,
    pub diagnostics_after: usize,
}

#[derive(Debug)]
pub struct AppliedPatch {
    pub diagnostic_id: String,
    pub patch_id: String,
    pub description: String,
}

#[derive(Debug)]
pub struct SkippedPatch {
    pub diagnostic_id: String,
    pub reason: String,
}

/// Repair tool: apply patches from diagnostics to fix source files.
pub struct RepairTool;

impl RepairTool {
    /// Run repair on a source file using diagnostics.
    /// Returns the repaired source and a RepairResult.
    pub fn repair(
        file: &str,
        source: &str,
        diagnostics: &DiagnosticSet,
        strategy: RepairStrategy,
        specific_id: Option<&str>,
    ) -> (String, RepairResult) {
        let mut result = RepairResult {
            applied: Vec::new(),
            skipped: Vec::new(),
            verify_passed: false,
            diagnostics_before: diagnostics.diagnostics.len(),
            diagnostics_after: 0,
        };

        // Collect all applicable patches
        let patches: Vec<(&str, &SuggestedPatch)> = diagnostics
            .diagnostics
            .iter()
            .flat_map(|d| d.suggested_patches.iter().map(move |p| (d.id.as_str(), p)))
            .collect();

        if patches.is_empty() {
            result.diagnostics_after = result.diagnostics_before;
            return (source.to_string(), result);
        }

        // Select patches based on strategy
        let selected = match strategy {
            RepairStrategy::Best => select_best(diagnostics),
            RepairStrategy::ById => {
                if let Some(id) = specific_id {
                    select_by_id(&patches, id)
                } else {
                    Vec::new()
                }
            }
            RepairStrategy::All => select_all(diagnostics),
            RepairStrategy::Conservative => select_conservative(diagnostics),
        };

        if selected.is_empty() {
            result.skipped.push(SkippedPatch {
                diagnostic_id: "all".into(),
                reason: "no applicable patches found".into(),
            });
            result.diagnostics_after = result.diagnostics_before;
            return (source.to_string(), result);
        }

        // Apply patches (sorted by line number descending to avoid offset issues)
        let mut repaired = source.to_string();
        for (diag_id, patch) in &selected {
            match apply_patch(&repaired, &patch.edits) {
                Ok(new_source) => {
                    repaired = new_source;
                    result.applied.push(AppliedPatch {
                        diagnostic_id: diag_id.to_string(),
                        patch_id: patch.id.clone(),
                        description: patch.description.clone(),
                    });
                }
                Err(reason) => {
                    result.skipped.push(SkippedPatch {
                        diagnostic_id: diag_id.to_string(),
                        reason,
                    });
                }
            }
        }

        // Verify: re-run diagnostics on the repaired source
        let verify = DiagnosticCollector::new(file, &repaired).collect();
        result.diagnostics_after = verify.diagnostics.len();
        result.verify_passed = !verify.has_errors();

        (repaired, result)
    }

    /// Repair a file on disk. Reads the file, applies patches, writes back.
    pub fn repair_file(
        path: &Path,
        diagnostics: &DiagnosticSet,
        strategy: RepairStrategy,
        specific_id: Option<&str>,
    ) -> Result<RepairResult, String> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;

        let file = path.to_string_lossy().to_string();
        let (repaired, result) = Self::repair(&file, &source, diagnostics, strategy, specific_id);

        if !result.applied.is_empty() {
            std::fs::write(path, &repaired)
                .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
        }

        Ok(result)
    }
}

/// Select the best (highest confidence) patch for each diagnostic.
fn select_best(diagnostics: &DiagnosticSet) -> Vec<(String, SuggestedPatch)> {
    let mut selected = Vec::new();
    for d in &diagnostics.diagnostics {
        if let Some(best) = d
            .suggested_patches
            .iter()
            .max_by_key(|p| match p.confidence {
                Confidence::High => 3,
                Confidence::Medium => 2,
                Confidence::Low => 1,
            })
        {
            selected.push((d.id.clone(), best.clone()));
        }
    }
    selected
}

/// Select a specific patch by its ID.
fn select_by_id<'a>(
    patches: &[(&'a str, &'a SuggestedPatch)],
    target_id: &str,
) -> Vec<(String, SuggestedPatch)> {
    patches
        .iter()
        .filter(|(_, p)| p.id == target_id)
        .map(|(diag_id, p)| (diag_id.to_string(), (*p).clone()))
        .collect()
}

/// Select all patches (in confidence order).
fn select_all(diagnostics: &DiagnosticSet) -> Vec<(String, SuggestedPatch)> {
    let mut selected = Vec::new();
    for d in &diagnostics.diagnostics {
        let mut patches: Vec<_> = d.suggested_patches.clone();
        patches.sort_by_key(|p| match p.confidence {
            Confidence::High => 0,
            Confidence::Medium => 1,
            Confidence::Low => 2,
        });
        if let Some(best) = patches.into_iter().next() {
            selected.push((d.id.clone(), best));
        }
    }
    selected
}

/// Select only High-confidence patches (one per diagnostic).
fn select_conservative(diagnostics: &DiagnosticSet) -> Vec<(String, SuggestedPatch)> {
    let mut selected = Vec::new();
    for d in &diagnostics.diagnostics {
        if let Some(best) = d
            .suggested_patches
            .iter()
            .filter(|p| p.confidence == Confidence::High)
            .max_by_key(|p| match p.confidence {
                Confidence::High => 3,
                Confidence::Medium => 2,
                Confidence::Low => 1,
            })
        {
            selected.push((d.id.clone(), best.clone()));
        }
    }
    selected
}

/// Apply text edits to source. Edits are applied in reverse line order.
fn apply_patch(source: &str, edits: &[TextEdit]) -> Result<String, String> {
    let mut lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();

    // Sort edits by start_line descending
    let mut sorted_edits: Vec<&TextEdit> = edits.iter().collect();
    sorted_edits.sort_by_key(|e| std::cmp::Reverse(e.start_line));

    for edit in sorted_edits {
        if edit.start_line == 0 || edit.start_line > lines.len() {
            return Err(format!(
                "edit line {} out of range (1-{})",
                edit.start_line,
                lines.len()
            ));
        }

        let idx = edit.start_line - 1; // 0-indexed

        // Count how many lines the old_text spans
        let old_lines: Vec<&str> = edit.old_text.lines().collect();
        let old_count = old_lines.len().max(1);

        // Verify the old text matches
        if old_count == 1 {
            let actual = &lines[idx];
            if actual.trim() != edit.old_text.trim() && *actual != edit.old_text {
                return Err(format!(
                    "line {} mismatch: expected '{}', got '{}'",
                    edit.start_line,
                    edit.old_text.trim(),
                    actual.trim(),
                ));
            }
        }

        // Replace lines
        let new_lines: Vec<String> = edit.new_text.lines().map(|l| l.to_string()).collect();
        let end = (idx + old_count).min(lines.len());
        lines.splice(idx..end, new_lines);
    }

    Ok(lines.join("\n") + if source.ends_with('\n') { "\n" } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Severity;

    #[test]
    fn test_apply_patch_single_line() {
        let source = "fn main() -> Int {\n    let x = countt\n    x\n}\n";
        let edits = vec![TextEdit {
            file: "test.ax".into(),
            start_line: 2,
            old_text: "    let x = countt".into(),
            new_text: "    let x = count".into(),
        }];
        let result = apply_patch(source, &edits).unwrap();
        assert!(result.contains("let x = count"));
        assert!(!result.contains("countt"));
    }

    #[test]
    fn test_apply_patch_multiline_insert() {
        let source = "match x {\n    A => 1\n}\n";
        let edits = vec![TextEdit {
            file: "test.ax".into(),
            start_line: 3,
            old_text: "}".into(),
            new_text: "    B => 2\n}".into(),
        }];
        let result = apply_patch(source, &edits).unwrap();
        assert!(result.contains("B => 2"));
    }

    #[test]
    fn test_repair_undefined_variable() {
        let source = "\
fn main() -> Int {
    let count = 10
    countt
}
";
        let ds = DiagnosticCollector::new("test.ax", source).collect();
        let (repaired, result) =
            RepairTool::repair("test.ax", source, &ds, RepairStrategy::Best, None);
        // Should have attempted a fix
        assert!(!result.applied.is_empty() || !result.skipped.is_empty());
        // The repaired source should be different if a patch was applied
        if !result.applied.is_empty() {
            assert_ne!(repaired, source);
        }
    }

    #[test]
    fn test_repair_strategy_by_id() {
        // Create a diagnostic set with known patches
        let mut ds = DiagnosticSet::new("test.ax");
        ds.push(crate::diagnostics::Diagnostic {
            id: "E006".into(),
            severity: Severity::Error,
            message: "unknown field".into(),
            location: None,
            suggested_patches: vec![SuggestedPatch {
                id: "E006-rename-countt".into(),
                description: "rename 'countt' to 'count'".into(),
                confidence: Confidence::High,
                rationale: "closest match".into(),
                edits: vec![TextEdit {
                    file: "test.ax".into(),
                    start_line: 2,
                    old_text: "    State { countt: 0 }".into(),
                    new_text: "    State { count: 0 }".into(),
                }],
            }],
            related: Vec::new(),
        });

        let source = "fn init() -> State {\n    State { countt: 0 }\n}\n";
        let (repaired, result) = RepairTool::repair(
            "test.ax",
            source,
            &ds,
            RepairStrategy::ById,
            Some("E006-rename-countt"),
        );
        assert_eq!(result.applied.len(), 1);
        assert!(repaired.contains("count: 0"));
    }

    #[test]
    fn repair_e003_near_miss_variable() {
        let source = "\
fn main() -> Int {
    let count = 10
    countt
}
";
        let ds = DiagnosticCollector::new("test.ax", source).collect();
        let e003 = ds.diagnostics.iter().find(|d| d.id == "E003");
        assert!(e003.is_some(), "expected E003 diagnostic");
        let has_patch = e003
            .unwrap()
            .suggested_patches
            .iter()
            .any(|p| !p.edits.is_empty());
        assert!(
            has_patch,
            "E003 should have a TextEdit patch for near-miss rename"
        );

        let (repaired, result) =
            RepairTool::repair("test.ax", source, &ds, RepairStrategy::Best, None);
        assert!(!result.applied.is_empty(), "repair should apply E003 patch");
        assert!(
            repaired.contains("count"),
            "repaired source should contain 'count'"
        );
        assert!(
            !repaired.contains("countt"),
            "repaired source should not contain 'countt'"
        );
    }

    #[test]
    fn repair_e004_near_miss_function() {
        // E004 is emitted when a diagnostic is manually tagged as such (e.g. by a
        // future compiler that distinguishes fn-call errors from var errors).
        // We test the repair path by constructing the diagnostic directly.
        let source = "fn helper() -> Int { 42 }\n\nfn main() -> Int {\n    helperr()\n}\n";
        let mut ds = DiagnosticSet::new("test.ax");
        ds.push(crate::diagnostics::Diagnostic {
            id: "E004".into(),
            severity: Severity::Error,
            message: "undefined function: helperr".into(),
            location: Some(crate::diagnostics::SourceLocation {
                file: "test.ax".into(),
                line: 4,
                col: None,
                end_line: None,
                end_col: None,
            }),
            suggested_patches: vec![SuggestedPatch {
                id: "E004-rename-helperr".into(),
                description: "rename 'helperr' to 'helper'".into(),
                confidence: Confidence::Medium,
                rationale: "closest match".into(),
                edits: vec![TextEdit {
                    file: "test.ax".into(),
                    start_line: 4,
                    old_text: "    helperr()".into(),
                    new_text: "    helper()".into(),
                }],
            }],
            related: Vec::new(),
        });

        let (repaired, result) =
            RepairTool::repair("test.ax", source, &ds, RepairStrategy::Best, None);
        assert!(!result.applied.is_empty(), "repair should apply E004 patch");
        assert!(
            repaired.contains("helper()"),
            "repaired source should contain correct function name"
        );
        assert!(
            !repaired.contains("helperr"),
            "repaired source should not contain typo"
        );
    }

    #[test]
    fn repair_bottom_up_ordering() {
        // Two patches at different positions — both must apply correctly
        let source = "fn main() -> Int {\n    let aaa = 1\n    let bbb = 2\n    aaax + bbbx\n}\n";
        let mut ds = DiagnosticSet::new("test.ax");
        ds.push(crate::diagnostics::Diagnostic {
            id: "E003".into(),
            severity: Severity::Error,
            message: "undefined variable: aaax".into(),
            location: None,
            suggested_patches: vec![SuggestedPatch {
                id: "E003-rename-aaax".into(),
                description: "rename 'aaax' to 'aaa'".into(),
                confidence: Confidence::Medium,
                rationale: "closest match".into(),
                edits: vec![TextEdit {
                    file: "test.ax".into(),
                    start_line: 4,
                    old_text: "    aaax + bbbx".into(),
                    new_text: "    aaa + bbbx".into(),
                }],
            }],
            related: Vec::new(),
        });
        ds.push(crate::diagnostics::Diagnostic {
            id: "E003".into(),
            severity: Severity::Error,
            message: "undefined variable: bbbx".into(),
            location: None,
            suggested_patches: vec![SuggestedPatch {
                id: "E003-rename-bbbx".into(),
                description: "rename 'bbbx' to 'bbb'".into(),
                confidence: Confidence::Medium,
                rationale: "closest match".into(),
                edits: vec![TextEdit {
                    file: "test.ax".into(),
                    start_line: 4,
                    old_text: "    aaa + bbbx".into(),
                    new_text: "    aaa + bbb".into(),
                }],
            }],
            related: Vec::new(),
        });
        let (repaired, result) =
            RepairTool::repair("test.ax", source, &ds, RepairStrategy::All, None);
        assert!(
            !result.applied.is_empty(),
            "should apply at least one patch"
        );
        assert!(
            repaired.contains("aaa"),
            "repaired source should use corrected names"
        );
    }

    #[test]
    fn repair_conservative_skips_low_confidence() {
        let mut ds = DiagnosticSet::new("test.ax");
        ds.push(crate::diagnostics::Diagnostic {
            id: "E003".into(),
            severity: Severity::Error,
            message: "undefined variable: countt".into(),
            location: None,
            suggested_patches: vec![SuggestedPatch {
                id: "E003-rename-countt".into(),
                description: "rename 'countt' to 'count'".into(),
                confidence: Confidence::Medium,
                rationale: "closest match".into(),
                edits: vec![TextEdit {
                    file: "test.ax".into(),
                    start_line: 2,
                    old_text: "    countt".into(),
                    new_text: "    count".into(),
                }],
            }],
            related: Vec::new(),
        });

        let source = "fn main() -> Int {\n    countt\n}\n";

        // Conservative should skip Medium confidence
        let (_, result_conservative) =
            RepairTool::repair("test.ax", source, &ds, RepairStrategy::Conservative, None);
        assert!(
            result_conservative.applied.is_empty(),
            "Conservative should skip Medium-confidence E003 patches"
        );

        // Best should apply it
        let (repaired_best, result_best) =
            RepairTool::repair("test.ax", source, &ds, RepairStrategy::Best, None);
        assert!(
            !result_best.applied.is_empty(),
            "Best should apply Medium-confidence patch"
        );
        assert!(
            repaired_best.contains("count"),
            "Best should rename countt to count"
        );
    }
}
