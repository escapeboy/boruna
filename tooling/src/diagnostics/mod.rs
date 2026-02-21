pub mod collector;
pub mod analyzer;
pub mod suggest;

use serde::{Deserialize, Serialize};

/// Stable error codes.
pub const E001_LEXER: &str = "E001";
pub const E002_PARSE: &str = "E002";
pub const E003_UNDEFINED_VAR: &str = "E003";
pub const E004_UNDEFINED_FN: &str = "E004";
pub const E005_NON_EXHAUSTIVE_MATCH: &str = "E005";
pub const E006_UNKNOWN_FIELD: &str = "E006";
pub const E007_CAPABILITY_VIOLATION: &str = "E007";
pub const E008_CODEGEN: &str = "E008";
pub const E009_TYPE_ERROR: &str = "E009";

/// A structured, machine-readable diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub id: String,
    pub severity: Severity,
    pub message: String,
    pub location: Option<SourceLocation>,
    pub suggested_patches: Vec<SuggestedPatch>,
    pub related: Vec<RelatedInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// Source location in a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: String,
    pub line: usize,
    pub col: Option<usize>,
    pub end_line: Option<usize>,
    pub end_col: Option<usize>,
}

/// A suggested fix for a diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedPatch {
    pub id: String,
    pub description: String,
    pub confidence: Confidence,
    pub rationale: String,
    pub edits: Vec<TextEdit>,
}

/// Confidence level for a suggested patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// A text edit (compatible with PatchBundle Hunk format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEdit {
    pub file: String,
    pub start_line: usize,
    pub old_text: String,
    pub new_text: String,
}

/// Related information for a diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedInfo {
    pub message: String,
    pub location: Option<SourceLocation>,
}

/// Top-level diagnostics output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticSet {
    pub version: u32,
    pub file: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl DiagnosticSet {
    pub fn new(file: &str) -> Self {
        DiagnosticSet {
            version: 1,
            file: file.to_string(),
            diagnostics: Vec::new(),
        }
    }

    pub fn push(&mut self, diag: Diagnostic) {
        self.diagnostics.push(diag);
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn to_human(&self) -> String {
        let mut out = String::new();
        for d in &self.diagnostics {
            let sev = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
                Severity::Hint => "hint",
            };
            if let Some(loc) = &d.location {
                out.push_str(&format!(
                    "[{}] {}:{}:{}: {}\n",
                    d.id,
                    loc.file,
                    loc.line,
                    loc.col.unwrap_or(0),
                    d.message,
                ));
            } else {
                out.push_str(&format!("[{}] {}: {}\n", d.id, sev, d.message));
            }
            for patch in &d.suggested_patches {
                out.push_str(&format!(
                    "  fix({}): {} [confidence: {:?}]\n",
                    patch.id, patch.description, patch.confidence,
                ));
            }
        }
        out
    }
}

impl Diagnostic {
    pub fn error(id: &str, message: String) -> Self {
        Diagnostic {
            id: id.to_string(),
            severity: Severity::Error,
            message,
            location: None,
            suggested_patches: Vec::new(),
            related: Vec::new(),
        }
    }

    pub fn warning(id: &str, message: String) -> Self {
        Diagnostic {
            id: id.to_string(),
            severity: Severity::Warning,
            message,
            location: None,
            suggested_patches: Vec::new(),
            related: Vec::new(),
        }
    }

    pub fn at(mut self, file: &str, line: usize, col: Option<usize>) -> Self {
        self.location = Some(SourceLocation {
            file: file.to_string(),
            line,
            col,
            end_line: None,
            end_col: None,
        });
        self
    }

    pub fn with_suggestion(mut self, patch: SuggestedPatch) -> Self {
        self.suggested_patches.push(patch);
        self
    }

    pub fn with_related(mut self, info: RelatedInfo) -> Self {
        self.related.push(info);
        self
    }
}
