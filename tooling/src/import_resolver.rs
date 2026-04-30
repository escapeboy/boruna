// Resolves `import "std-name"` statements by inlining the library source.
// This is a source-level preprocessor; the compiler sees a single merged source.

use std::path::Path;

#[derive(Debug)]
pub enum ImportError {
    LibraryNotFound(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::LibraryNotFound(name) => {
                write!(f, "import error: library '{name}' not found")
            }
            ImportError::Io(e) => write!(f, "import error: {e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        ImportError::Io(e)
    }
}

/// Resolve `import "name"` statements in `source` by inlining library source from `libs_dir`.
///
/// Returns all imported library sources (with `fn main()` stripped) concatenated,
/// followed by the original source with import lines removed.
pub fn resolve_imports(source: &str, libs_dir: &Path) -> Result<String, ImportError> {
    let mut lib_sources: Vec<String> = Vec::new();
    let mut user_lines: Vec<&str> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(name) = parse_import_line(trimmed) {
            if seen.contains(&name) {
                // Duplicate import — drop the line.
                continue;
            }
            seen.insert(name.clone());

            let lib_path = libs_dir.join(&name).join("src/core.ax");
            let lib_src = std::fs::read_to_string(&lib_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ImportError::LibraryNotFound(name.clone())
                } else {
                    ImportError::Io(e)
                }
            })?;
            lib_sources.push(strip_main(&lib_src));
        } else {
            user_lines.push(line);
        }
    }

    if lib_sources.is_empty() {
        return Ok(source.to_owned());
    }

    let mut out = lib_sources.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&user_lines.join("\n"));
    Ok(out)
}

/// Parse a line of the form `import "name"` or `import name`.
/// Returns the library name, or `None` if it is not an import statement.
fn parse_import_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix("import")?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    // Quoted: import "std-ui"
    if let Some(inner) = rest.strip_prefix('"') {
        let name = inner.trim_end_matches('"');
        if !name.is_empty() {
            return Some(name.to_owned());
        }
    } else {
        // Bare identifier: import std-ui
        let name = rest.split_whitespace().next()?;
        if !name.is_empty() {
            return Some(name.to_owned());
        }
    }
    None
}

/// Strip the `fn main() -> Int { … }` test stub from a library source.
fn strip_main(src: &str) -> String {
    if !src.contains("fn main(") {
        return src.to_owned();
    }

    let main_start = match src.find("fn main(") {
        Some(pos) => pos,
        None => return src.to_owned(),
    };

    // Walk back to the start of the line containing `fn main(`.
    let line_start = src[..main_start].rfind('\n').map(|p| p + 1).unwrap_or(0);

    // Walk forward past the closing brace (track brace depth).
    let after_main = &src[main_start..];
    let mut depth: i32 = 0;
    let mut found_open = false;
    let mut rel_end = 0;
    for (i, ch) in after_main.char_indices() {
        match ch {
            '{' => {
                depth += 1;
                found_open = true;
            }
            '}' => {
                depth -= 1;
                if found_open && depth == 0 {
                    rel_end = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }

    let end_offset = main_start + rel_end;
    // Consume trailing newline so we don't leave a blank line.
    let end_offset = if src.as_bytes().get(end_offset) == Some(&b'\n') {
        end_offset + 1
    } else {
        end_offset
    };

    let mut out = String::with_capacity(src.len());
    out.push_str(&src[..line_start]);
    out.push_str(&src[end_offset..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn libs_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../libs")
    }

    #[test]
    fn test_import_resolves_std_ui() {
        let source =
            "import \"std-ui\"\n\nfn main() -> Int {\n    let n: UINode = empty_node()\n    0\n}\n";
        let result = resolve_imports(source, &libs_dir()).expect("resolve_imports failed");
        assert!(result.contains("fn button("), "expected std-ui fn button");
        assert!(result.contains("fn text("), "expected std-ui fn text");
        assert!(
            !result.contains("import \"std-ui\""),
            "import line should be removed"
        );
        assert!(result.contains("fn main() -> Int"), "user main must remain");
    }

    #[test]
    fn test_import_missing_library() {
        let source = "import \"std-nonexistent\"\nfn main() -> Int { 0 }\n";
        let err = resolve_imports(source, &libs_dir()).expect_err("should fail");
        match err {
            ImportError::LibraryNotFound(name) => assert_eq!(name, "std-nonexistent"),
            other => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn test_no_imports() {
        let source = "fn main() -> Int { 0 }\n";
        let result = resolve_imports(source, &libs_dir()).expect("should succeed");
        assert_eq!(result, source);
    }

    #[test]
    fn test_main_stripped_from_library() {
        let lib_with_main = "fn helper() -> Int { 1 }\nfn main() -> Int { 0 }\n";
        let result = strip_main(lib_with_main);
        assert!(!result.contains("fn main("), "main should be stripped");
        assert!(result.contains("fn helper("), "helper must remain");
    }
}
