use boruna_compiler::CompileError;

use super::*;
use super::analyzer::Analyzer;
use super::suggest;

/// Collects diagnostics by running the compiler and additional analysis passes.
pub struct DiagnosticCollector<'a> {
    file: &'a str,
    source: &'a str,
}

impl<'a> DiagnosticCollector<'a> {
    pub fn new(file: &'a str, source: &'a str) -> Self {
        DiagnosticCollector { file, source }
    }

    /// Run all diagnostic passes and return a complete DiagnosticSet.
    pub fn collect(&self) -> DiagnosticSet {
        let mut ds = DiagnosticSet::new(self.file);

        // Phase 1: Try lex
        let tokens = match boruna_compiler::lexer::lex(self.source) {
            Ok(tokens) => tokens,
            Err(e) => {
                ds.push(self.compile_error_to_diagnostic(&e));
                return ds;
            }
        };

        // Phase 2: Try parse
        let program = match boruna_compiler::parser::parse(tokens) {
            Ok(program) => program,
            Err(e) => {
                ds.push(self.compile_error_to_diagnostic(&e));
                return ds;
            }
        };

        // Phase 3: Try type check
        if let Err(e) = boruna_compiler::typeck::check(&program) {
            let mut diag = self.compile_error_to_diagnostic(&e);
            // Try to enhance with suggestions
            suggest::enhance_compiler_diagnostic(&mut diag, self.file, self.source, &program);
            ds.push(diag);
            // Don't return — still run analyzers for additional findings
        }

        // Phase 4: Run additional analyzers on the AST
        let analyzer = Analyzer::new(self.file, self.source, &program);
        let findings = analyzer.analyze();
        for diag in findings {
            ds.push(diag);
        }

        ds
    }

    /// Convert a CompileError into a Diagnostic.
    fn compile_error_to_diagnostic(&self, err: &CompileError) -> Diagnostic {
        match err {
            CompileError::Lexer { line, col, msg } => {
                Diagnostic::error(E001_LEXER, msg.clone())
                    .at(self.file, *line, Some(*col))
            }
            CompileError::Parse { line, msg } => {
                Diagnostic::error(E002_PARSE, msg.clone())
                    .at(self.file, *line, None)
            }
            CompileError::Type(msg) => {
                let (code, line) = classify_type_error(msg, self.source);
                let mut diag = Diagnostic::error(code, msg.clone());
                if let Some(l) = line {
                    diag = diag.at(self.file, l, None);
                }
                diag
            }
            CompileError::Codegen(msg) => {
                let line = find_codegen_error_line(msg, self.source);
                let mut diag = Diagnostic::error(E008_CODEGEN, msg.clone());
                if let Some(l) = line {
                    diag = diag.at(self.file, l, None);
                }
                diag
            }
        }
    }
}

/// Classify a type error string into a specific error code and try to find the line.
fn classify_type_error(msg: &str, source: &str) -> (&'static str, Option<usize>) {
    if msg.starts_with("undefined variable: ") {
        let name = msg.strip_prefix("undefined variable: ").unwrap_or("");
        let line = find_identifier_line(source, name);
        (E003_UNDEFINED_VAR, line)
    } else if msg.starts_with("undefined function: ") {
        let name = msg.strip_prefix("undefined function: ").unwrap_or("");
        let line = find_identifier_line(source, name);
        (E004_UNDEFINED_FN, line)
    } else {
        (E009_TYPE_ERROR, None)
    }
}

/// Find the first line containing an identifier (not in a comment or string).
fn find_identifier_line(source: &str, name: &str) -> Option<usize> {
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        // Skip comments
        if trimmed.starts_with("//") {
            continue;
        }
        // Skip type/fn definitions (the name is being defined, not used)
        if trimmed.starts_with("fn ") || trimmed.starts_with("type ") || trimmed.starts_with("enum ") {
            if trimmed.contains(&format!("fn {name}"))
                || trimmed.contains(&format!("type {name}"))
                || trimmed.contains(&format!("enum {name}"))
            {
                continue;
            }
        }
        // Look for the identifier as a word
        if contains_word(trimmed, name) {
            return Some(i + 1); // 1-indexed
        }
    }
    None
}

/// Find the line for a codegen error referencing an identifier.
fn find_codegen_error_line(msg: &str, source: &str) -> Option<usize> {
    // Extract identifier from common codegen error patterns
    if let Some(name) = msg.strip_prefix("undefined: ") {
        return find_identifier_line(source, name);
    }
    if let Some(name) = msg.strip_prefix("unknown function: ") {
        return find_identifier_line(source, name);
    }
    None
}

/// Check if a line contains a word (not part of a larger identifier).
fn contains_word(line: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = line[start..].find(word) {
        let abs_pos = start + pos;
        let before_ok = abs_pos == 0
            || !line.as_bytes()[abs_pos - 1].is_ascii_alphanumeric()
                && line.as_bytes()[abs_pos - 1] != b'_';
        let after_pos = abs_pos + word.len();
        let after_ok = after_pos >= line.len()
            || !line.as_bytes()[after_pos].is_ascii_alphanumeric()
                && line.as_bytes()[after_pos] != b'_';
        if before_ok && after_ok {
            return true;
        }
        start = abs_pos + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_word() {
        assert!(contains_word("let x = foo + bar", "foo"));
        assert!(!contains_word("let x = foobar", "foo"));
        assert!(contains_word("foo", "foo"));
        assert!(contains_word("(foo)", "foo"));
        assert!(!contains_word("_foo_bar", "foo"));
    }

    #[test]
    fn test_find_identifier_line() {
        let source = "fn main() -> Int {\n    let x = 10\n    y + x\n}\n";
        assert_eq!(find_identifier_line(source, "y"), Some(3));
    }

    #[test]
    fn test_classify_type_error() {
        let source = "fn main() -> Int {\n    foo\n}\n";
        let (code, line) = classify_type_error("undefined variable: foo", source);
        assert_eq!(code, E003_UNDEFINED_VAR);
        assert_eq!(line, Some(2));
    }

    #[test]
    fn test_collector_lexer_error() {
        let ds = DiagnosticCollector::new("test.ax", "fn main() { @ }").collect();
        assert!(ds.has_errors());
        assert_eq!(ds.diagnostics[0].id, E001_LEXER);
    }

    #[test]
    fn test_collector_parse_error() {
        let ds = DiagnosticCollector::new("test.ax", "fn { }").collect();
        assert!(ds.has_errors());
        assert_eq!(ds.diagnostics[0].id, E002_PARSE);
    }

    #[test]
    fn test_collector_undefined_variable() {
        let source = "fn main() -> Int {\n    undefined_var\n}\n";
        let ds = DiagnosticCollector::new("test.ax", source).collect();
        assert!(ds.has_errors());
        let err = ds.diagnostics.iter().find(|d| d.id == E003_UNDEFINED_VAR);
        assert!(err.is_some());
    }

    #[test]
    fn test_collector_valid_source() {
        let source = "fn main() -> Int {\n    42\n}\n";
        let ds = DiagnosticCollector::new("test.ax", source).collect();
        assert!(!ds.has_errors());
    }
}
