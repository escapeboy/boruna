//! Machine-readable registry of stable diagnostic codes.
//!
//! Every `E0NN` code emitted by the toolchain has exactly one entry here. The
//! registry is the agent-facing source of truth: `boruna lang codes` serves it
//! so an agent can resolve a code seen in `lang check --json` output without
//! reading compiler source. A drift test asserts the registry stays 1:1 with
//! the `E0NN` constants in `super`.

use serde::Serialize;

/// Documentation for one stable diagnostic code.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DiagnosticCodeInfo {
    /// Stable code string, e.g. `"E003"`. Never reused or renumbered.
    pub code: &'static str,
    /// Short human name, e.g. `"undefined-variable"`.
    pub name: &'static str,
    /// One-line summary of what the code means.
    pub summary: &'static str,
    /// Compiler phase that emits the code.
    pub category: &'static str,
}

/// All stable diagnostic codes, ordered by code.
pub const REGISTRY: &[DiagnosticCodeInfo] = &[
    DiagnosticCodeInfo {
        code: super::E001_LEXER,
        name: "lexer-error",
        summary: "The source could not be tokenized (invalid character or token).",
        category: "lexical",
    },
    DiagnosticCodeInfo {
        code: super::E002_PARSE,
        name: "parse-error",
        summary: "The token stream did not form a valid syntax tree.",
        category: "syntax",
    },
    DiagnosticCodeInfo {
        code: super::E003_UNDEFINED_VAR,
        name: "undefined-variable",
        summary: "A referenced variable is not defined in scope.",
        category: "name-resolution",
    },
    DiagnosticCodeInfo {
        code: super::E004_UNDEFINED_FN,
        name: "undefined-function",
        summary: "A called function is not defined in the module.",
        category: "name-resolution",
    },
    DiagnosticCodeInfo {
        code: super::E005_NON_EXHAUSTIVE_MATCH,
        name: "non-exhaustive-match",
        summary: "A match expression does not cover all possible cases.",
        category: "pattern-matching",
    },
    DiagnosticCodeInfo {
        code: super::E006_UNKNOWN_FIELD,
        name: "unknown-field",
        summary: "A record field access or construction references an unknown field.",
        category: "type",
    },
    DiagnosticCodeInfo {
        code: super::E007_CAPABILITY_VIOLATION,
        name: "capability-violation",
        summary: "A function performs an effect it does not declare in its capability set.",
        category: "capability",
    },
    DiagnosticCodeInfo {
        code: super::E008_CODEGEN,
        name: "codegen-error",
        summary: "The typechecked program could not be lowered to bytecode.",
        category: "codegen",
    },
    DiagnosticCodeInfo {
        code: super::E009_TYPE_ERROR,
        name: "type-error",
        summary: "An expression's type does not match the type required by its context.",
        category: "type",
    },
];

/// Returns the full diagnostic-code registry.
pub fn registry() -> &'static [DiagnosticCodeInfo] {
    REGISTRY
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract every `E0NN` code string from a `pub const ... = "E0NN";` line
    /// in `diagnostics/mod.rs`. Parsing the source — rather than a hand-kept
    /// list — means a constant added to `mod.rs` but not to the registry can
    /// never slip past `registry_matches_source_constants`.
    fn constants_declared_in_source() -> Vec<String> {
        let src = include_str!("mod.rs");
        let mut codes = Vec::new();
        for line in src.lines() {
            let line = line.trim();
            if !line.starts_with("pub const E") {
                continue;
            }
            // ... = "E001";  -> grab the quoted literal.
            if let Some(start) = line.find('"') {
                if let Some(end) = line[start + 1..].find('"') {
                    codes.push(line[start + 1..start + 1 + end].to_string());
                }
            }
        }
        codes
    }

    #[test]
    fn registry_matches_source_constants() {
        let declared = constants_declared_in_source();
        assert!(
            !declared.is_empty(),
            "found no `pub const E..` declarations in mod.rs — parser broke"
        );
        let registry_codes: Vec<&str> = REGISTRY.iter().map(|c| c.code).collect();
        for code in &declared {
            assert!(
                registry_codes.contains(&code.as_str()),
                "diagnostic constant {code} declared in mod.rs has no registry entry"
            );
        }
        for entry in REGISTRY {
            assert!(
                declared.iter().any(|c| c == entry.code),
                "registry entry {} has no backing `pub const` in mod.rs",
                entry.code
            );
        }
        assert_eq!(REGISTRY.len(), declared.len());
    }

    #[test]
    fn registry_codes_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in REGISTRY {
            assert!(seen.insert(entry.code), "duplicate code {}", entry.code);
        }
    }

    #[test]
    fn registry_entries_are_populated() {
        for entry in REGISTRY {
            assert!(!entry.name.is_empty());
            assert!(!entry.summary.is_empty());
            assert!(!entry.category.is_empty());
        }
    }
}
