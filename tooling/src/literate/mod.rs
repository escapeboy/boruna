//! Literate workflow specs — markdown with embedded `.ax` code fences.
//!
//! A literate workflow file is a Markdown document. Code fences whose info
//! string matches `<lang> <filename> +=` (where `<lang>` ∈ {`ax`, `boruna`,
//! `quint`}) are extracted into per-file outputs, allowing one auditor-facing
//! document to be both human narrative AND executable Boruna source.
//!
//! Fences in any other language (`rust`, `bash`, `mermaid`, …) are silently
//! ignored — they're prose code samples, not extraction targets.
//!
//! See `docs/design-literate-workflows.md` and
//! `docs/architecture-literate-workflows.md` for the design rationale.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use serde::Serialize;

/// Languages whose fences are considered for extraction. Other languages are
/// silently ignored (treated as prose code samples).
pub const EXTRACTION_LANGS: &[&str] = &["ax", "boruna", "quint"];

/// Options controlling extraction behavior.
#[derive(Debug, Clone)]
pub struct ExtractOptions {
    /// When the FIRST `+=` block for a filename is encountered, truncate
    /// the file (or create it). Subsequent `+=` blocks append.
    /// Default: `true` — gives idempotent re-extraction.
    pub overwrite_on_first: bool,
    /// Language tags whose code fences are extracted.
    /// Default: `["ax", "boruna", "quint"]`.
    pub accept_langs: Vec<String>,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            overwrite_on_first: true,
            accept_langs: EXTRACTION_LANGS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Summary of what was extracted.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractReport {
    /// Files (relative paths under `out_dir`) that received content.
    pub files_written: Vec<PathBuf>,
    /// Number of fenced blocks extracted across all files.
    pub blocks_extracted: usize,
    /// Number of fenced blocks whose language was not in `accept_langs`,
    /// or that had no filename header (`+=`-less fences).
    pub blocks_skipped: usize,
}

/// Errors returned by [`extract`].
#[derive(Debug)]
pub enum ExtractError {
    Io {
        kind: io::ErrorKind,
        detail: String,
    },
    /// Fence info string parses neither as `<lang> <filename> +=` nor as
    /// a recognized "ignore this" shape (e.g. plain `<lang>`).
    InvalidFence {
        line: usize,
        info: String,
        detail: &'static str,
    },
    /// Filename contained `..` — path traversal attempt.
    PathTraversal {
        line: usize,
        filename: String,
    },
    /// Filename was absolute — must be relative.
    AbsolutePath {
        line: usize,
        filename: String,
    },
    /// `out_dir` is not a directory or could not be created.
    InvalidOutDir {
        detail: String,
    },
}

impl ExtractError {
    /// Stable string per project-conventions §2. Used by CLI integration to
    /// emit `error_kind` in JSON output.
    pub fn error_kind(&self) -> &'static str {
        match self {
            ExtractError::Io { .. } => "literate.io",
            ExtractError::InvalidFence { .. } => "literate.invalid_fence",
            ExtractError::PathTraversal { .. } => "literate.path_traversal",
            ExtractError::AbsolutePath { .. } => "literate.absolute_path",
            ExtractError::InvalidOutDir { .. } => "literate.invalid_out_dir",
        }
    }
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Io { kind, detail } => write!(f, "io error ({kind:?}): {detail}"),
            ExtractError::InvalidFence { line, info, detail } => {
                write!(
                    f,
                    "invalid fence header on line {line}: {detail} — saw `{info}`"
                )
            }
            ExtractError::PathTraversal { line, filename } => {
                write!(
                    f,
                    "path traversal rejected on line {line}: filename `{filename}` contains `..`"
                )
            }
            ExtractError::AbsolutePath { line, filename } => {
                write!(
                    f,
                    "absolute path rejected on line {line}: filename `{filename}` must be relative"
                )
            }
            ExtractError::InvalidOutDir { detail } => write!(f, "invalid --out-dir: {detail}"),
        }
    }
}

impl std::error::Error for ExtractError {}

impl From<io::Error> for ExtractError {
    fn from(e: io::Error) -> Self {
        ExtractError::Io {
            kind: e.kind(),
            detail: e.to_string(),
        }
    }
}

/// Parse the markdown source and extract every block whose info string is of
/// the form `<lang> <filename> +=` into `out_dir`.
///
/// The first `+=` block for each filename truncates / creates the file when
/// `opts.overwrite_on_first` is `true` (the default). Subsequent `+=` blocks
/// append. Each block is followed by a single trailing newline.
pub fn extract(
    markdown: &str,
    out_dir: &Path,
    opts: &ExtractOptions,
) -> Result<ExtractReport, ExtractError> {
    ensure_out_dir(out_dir)?;

    let blocks = parse_blocks(markdown);

    let mut blocks_extracted = 0usize;
    let mut blocks_skipped = 0usize;
    let mut files_written = Vec::<PathBuf>::new();
    let mut seen_filenames: HashSet<PathBuf> = HashSet::new();

    for block in blocks {
        match classify_block(&block, &opts.accept_langs)? {
            BlockKind::Ignored => {
                blocks_skipped += 1;
            }
            BlockKind::Extract { filename } => {
                let safe_filename = validate_filename(&filename, block.line)?;
                let target = out_dir.join(&safe_filename);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }

                let already_seen = !seen_filenames.insert(safe_filename.clone());
                let should_truncate = !already_seen && opts.overwrite_on_first;

                if should_truncate {
                    let mut content = block.content.clone();
                    if !content.ends_with('\n') {
                        content.push('\n');
                    }
                    fs::write(&target, content)?;
                    files_written.push(safe_filename);
                } else {
                    let mut existing = if target.exists() {
                        fs::read_to_string(&target)?
                    } else {
                        String::new()
                    };
                    // Ensure a separator newline between blocks when both
                    // sides are non-empty.
                    if !existing.is_empty() && !existing.ends_with('\n') {
                        existing.push('\n');
                    }
                    existing.push_str(&block.content);
                    if !existing.ends_with('\n') {
                        existing.push('\n');
                    }
                    fs::write(&target, existing)?;
                    if !files_written.contains(&safe_filename) {
                        files_written.push(safe_filename);
                    }
                }
                blocks_extracted += 1;
            }
        }
    }

    Ok(ExtractReport {
        files_written,
        blocks_extracted,
        blocks_skipped,
    })
}

fn ensure_out_dir(out_dir: &Path) -> Result<(), ExtractError> {
    if out_dir.exists() {
        if !out_dir.is_dir() {
            return Err(ExtractError::InvalidOutDir {
                detail: format!("{:?} exists but is not a directory", out_dir),
            });
        }
    } else {
        fs::create_dir_all(out_dir).map_err(|e| ExtractError::InvalidOutDir {
            detail: format!("cannot create {:?}: {e}", out_dir),
        })?;
    }
    Ok(())
}

/// Reject `..` and absolute paths. Returns a normalized relative `PathBuf`
/// safe to join under `out_dir`.
fn validate_filename(filename: &str, line: usize) -> Result<PathBuf, ExtractError> {
    let p = Path::new(filename);
    if p.is_absolute() {
        return Err(ExtractError::AbsolutePath {
            line,
            filename: filename.to_string(),
        });
    }
    for comp in p.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return Err(ExtractError::PathTraversal {
                line,
                filename: filename.to_string(),
            });
        }
        if matches!(comp, std::path::Component::RootDir) {
            return Err(ExtractError::AbsolutePath {
                line,
                filename: filename.to_string(),
            });
        }
    }
    Ok(p.to_path_buf())
}

#[derive(Debug)]
struct ParsedBlock {
    info: String,
    content: String,
    /// Approximate 1-based line in the source markdown — extracted from
    /// the running newline count in `parse_blocks`.
    line: usize,
}

#[derive(Debug)]
enum BlockKind {
    Ignored,
    Extract { filename: String },
}

fn parse_blocks(markdown: &str) -> Vec<ParsedBlock> {
    let parser = Parser::new(markdown);
    let mut blocks = Vec::new();
    let mut inside: Option<(String, String)> = None; // (info, content)
    let mut byte_offset = 0usize;
    let mut current_line = 1usize;

    for (event, range) in parser.into_offset_iter() {
        // Track the line where this event starts.
        let start = range.start;
        if start >= byte_offset {
            let delta = &markdown[byte_offset..start];
            current_line += delta.bytes().filter(|b| *b == b'\n').count();
            byte_offset = start;
        }

        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                inside = Some((info.into_string(), String::new()));
            }
            Event::Text(t) => {
                if let Some((_, content)) = inside.as_mut() {
                    content.push_str(&t);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((info, content)) = inside.take() {
                    blocks.push(ParsedBlock {
                        info,
                        content,
                        line: current_line,
                    });
                }
            }
            _ => {}
        }
    }
    blocks
}

fn classify_block(block: &ParsedBlock, accept_langs: &[String]) -> Result<BlockKind, ExtractError> {
    let info = block.info.trim();
    if info.is_empty() {
        return Ok(BlockKind::Ignored);
    }
    // Tokenize info string: <lang> <filename> +=
    let tokens: Vec<&str> = info.split_whitespace().collect();
    let lang = tokens[0];
    if !accept_langs.iter().any(|l| l == lang) {
        // A language we don't recognize → silently ignored (matches Quint).
        return Ok(BlockKind::Ignored);
    }
    match tokens.len() {
        1 => {
            // Bare `<lang>` fence — syntax-highlighted prose snippet, not
            // an extraction target.
            Ok(BlockKind::Ignored)
        }
        3 => {
            let filename = tokens[1];
            let op = tokens[2];
            if op != "+=" {
                return Err(ExtractError::InvalidFence {
                    line: block.line,
                    info: info.to_string(),
                    detail: "operator must be `+=`",
                });
            }
            Ok(BlockKind::Extract {
                filename: filename.to_string(),
            })
        }
        _ => Err(ExtractError::InvalidFence {
            line: block.line,
            info: info.to_string(),
            detail: "expected `<lang> <filename> +=`",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn extract_in_tmp(md: &str) -> (TempDir, ExtractReport) {
        let tmp = TempDir::new().unwrap();
        let report = extract(md, tmp.path(), &ExtractOptions::default()).unwrap();
        (tmp, report)
    }

    #[test]
    fn single_fence_writes_file() {
        let md = "# Hi\n```ax foo.ax +=\nlet x = 1\n```\n";
        let (tmp, report) = extract_in_tmp(md);
        assert_eq!(report.blocks_extracted, 1);
        assert_eq!(report.files_written.len(), 1);
        let content = fs::read_to_string(tmp.path().join("foo.ax")).unwrap();
        assert_eq!(content, "let x = 1\n");
    }

    #[test]
    fn two_fences_same_file_append_in_order() {
        let md = "\
```ax foo.ax +=
a
```

```ax foo.ax +=
b
```
";
        let (tmp, report) = extract_in_tmp(md);
        assert_eq!(report.blocks_extracted, 2);
        let content = fs::read_to_string(tmp.path().join("foo.ax")).unwrap();
        assert!(content.contains("a\n"));
        assert!(content.contains("b\n"));
        // Order preserved
        let a_pos = content.find('a').unwrap();
        let b_pos = content.find('b').unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn idempotent_re_extraction() {
        let md = "\
```ax foo.ax +=
hello
```
";
        let tmp = TempDir::new().unwrap();
        extract(md, tmp.path(), &ExtractOptions::default()).unwrap();
        let first = fs::read_to_string(tmp.path().join("foo.ax")).unwrap();
        extract(md, tmp.path(), &ExtractOptions::default()).unwrap();
        let second = fs::read_to_string(tmp.path().join("foo.ax")).unwrap();
        assert_eq!(first, second, "re-extraction must be byte-identical");
    }

    #[test]
    fn quint_tag_is_accepted() {
        let md = "```quint foo.qnt +=\nval x = 1\n```\n";
        let (tmp, report) = extract_in_tmp(md);
        assert_eq!(report.blocks_extracted, 1);
        assert!(tmp.path().join("foo.qnt").exists());
    }

    #[test]
    fn boruna_tag_is_accepted() {
        let md = "```boruna foo.ax +=\nlet x = 1\n```\n";
        let (tmp, report) = extract_in_tmp(md);
        assert_eq!(report.blocks_extracted, 1);
        assert!(tmp.path().join("foo.ax").exists());
    }

    #[test]
    fn rust_fence_is_ignored() {
        let md = "```rust\nfn main() {}\n```\n";
        let (tmp, report) = extract_in_tmp(md);
        assert_eq!(report.blocks_extracted, 0);
        assert_eq!(report.blocks_skipped, 1);
        assert!(fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    #[test]
    fn bare_ax_fence_without_filename_is_ignored() {
        // Prose snippet — syntax-highlighted but not extracted.
        let md = "```ax\nlet x = 1\n```\n";
        let (tmp, report) = extract_in_tmp(md);
        assert_eq!(report.blocks_extracted, 0);
        assert_eq!(report.blocks_skipped, 1);
        assert!(fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    #[test]
    fn path_traversal_is_rejected() {
        let md = "```ax ../etc/foo.ax +=\nlet x = 1\n```\n";
        let tmp = TempDir::new().unwrap();
        let err = extract(md, tmp.path(), &ExtractOptions::default()).unwrap_err();
        assert_eq!(err.error_kind(), "literate.path_traversal");
    }

    #[test]
    fn absolute_path_is_rejected() {
        let md = "```ax /etc/foo.ax +=\nlet x = 1\n```\n";
        let tmp = TempDir::new().unwrap();
        let err = extract(md, tmp.path(), &ExtractOptions::default()).unwrap_err();
        assert_eq!(err.error_kind(), "literate.absolute_path");
    }

    #[test]
    fn invalid_operator_is_rejected() {
        let md = "```ax foo.ax =\nlet x = 1\n```\n";
        let tmp = TempDir::new().unwrap();
        let err = extract(md, tmp.path(), &ExtractOptions::default()).unwrap_err();
        assert_eq!(err.error_kind(), "literate.invalid_fence");
    }

    #[test]
    fn invalid_out_dir_when_path_is_a_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("not_a_dir");
        fs::write(&file_path, "junk").unwrap();
        let err = extract("", &file_path, &ExtractOptions::default()).unwrap_err();
        assert_eq!(err.error_kind(), "literate.invalid_out_dir");
    }

    #[test]
    fn nested_subdir_is_created() {
        let md = "```ax sub/nested.ax +=\nlet x = 1\n```\n";
        let (tmp, _) = extract_in_tmp(md);
        assert!(tmp.path().join("sub/nested.ax").exists());
    }

    #[test]
    fn interleaved_files_preserve_per_file_order() {
        let md = "\
```ax a.ax +=
A1
```

```ax b.ax +=
B1
```

```ax a.ax +=
A2
```
";
        let (tmp, report) = extract_in_tmp(md);
        assert_eq!(report.blocks_extracted, 3);
        let a = fs::read_to_string(tmp.path().join("a.ax")).unwrap();
        let b = fs::read_to_string(tmp.path().join("b.ax")).unwrap();
        assert!(a.contains("A1"));
        assert!(a.contains("A2"));
        assert!(a.find("A1").unwrap() < a.find("A2").unwrap());
        assert_eq!(b.trim_end(), "B1");
    }

    #[test]
    fn fences_with_no_blocks_yields_zero_report() {
        let md = "# Just text\n\nNo fences here.\n";
        let (_, report) = extract_in_tmp(md);
        assert_eq!(report.blocks_extracted, 0);
        assert_eq!(report.blocks_skipped, 0);
        assert!(report.files_written.is_empty());
    }

    #[test]
    fn empty_markdown_is_ok() {
        let (_, report) = extract_in_tmp("");
        assert_eq!(report.blocks_extracted, 0);
    }

    #[test]
    fn unicode_in_content_round_trips() {
        let md = "```ax greet.ax +=\nlet greeting = \"Здравей, свят!\"\n```\n";
        let (tmp, _) = extract_in_tmp(md);
        let content = fs::read_to_string(tmp.path().join("greet.ax")).unwrap();
        assert!(content.contains("Здравей, свят!"));
    }
}
