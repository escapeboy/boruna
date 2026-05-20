# Architecture — Literate workflow specs (`boruna literate extract`)

Companion to `docs/design-literate-workflows.md`. **Implemented this sprint.**

## Component map

| Component | Location | Role |
|---|---|---|
| `literate` module | `tooling/src/literate/mod.rs` (new) | Markdown parser + extractor |
| Public API | `boruna_tooling::literate::extract(input, out_dir, opts)` | Library entry point |
| CLI command | `crates/llmvm-cli/src/main.rs` (new `Literate` Subcommand) | `boruna literate extract <file>` |
| Diagnostics | reuses `boruna_tooling::diagnostics::Diagnostic` | Line/col attribution |

## Data flow

```
boruna literate extract foo.md --out-dir gen/
  ↓
fs::read_to_string("foo.md")
  ↓
literate::extract(markdown_str, &out_dir, opts) -> Result<ExtractReport, ExtractError>:
  pulldown_cmark::Parser parses markdown
  walk events, collect CodeBlock { lang, content, line }
  for each block whose lang matches "ax|boruna|quint <filename> +=":
    parse fence header → { filename, mode: Append }
    validate filename (no .., not absolute)
    canonicalize against out_dir
    classify: first-seen or subsequent
    write/append to <out_dir>/<filename>
  return ExtractReport {
    files_written: Vec<PathBuf>,
    blocks_extracted: usize,
    blocks_skipped: usize,
  }
  ↓
human report to stdout, or --json
```

## Public API (library shape)

```rust
// tooling/src/literate/mod.rs

pub struct ExtractOptions {
    pub overwrite_on_first: bool,  // default true (decision #5)
    pub accept_langs: Vec<String>, // default ["ax","boruna","quint"]
}

pub struct ExtractReport {
    pub files_written: Vec<PathBuf>,
    pub blocks_extracted: usize,
    pub blocks_skipped: usize,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid fence header on line {line}: {detail}")]
    InvalidFence { line: usize, detail: String },
    #[error("path traversal rejected on line {line}: filename {0:?}")]
    PathTraversal(String, usize),
    #[error("absolute path rejected on line {line}: filename {0:?}")]
    AbsolutePath(String, usize),
}

pub fn extract(
    markdown: &str,
    out_dir: &Path,
    opts: &ExtractOptions,
) -> Result<ExtractReport, ExtractError>;
```

## Fence header grammar

Acceptable code-fence info strings (the `xxx` in ` ```xxx `):

```
<lang> <filename> +=
```

- `<lang>` ∈ {`ax`, `boruna`, `quint`}
- `<filename>` is a relative path; rejected if it contains `..` or starts with `/`
- `+=` is literal; missing it = ExtractError::InvalidFence
- Whitespace separation: any run of ASCII whitespace

Code fences with other language tags (`rust`, `bash`, etc.) are silently ignored — they're
"prose code samples," not extraction targets. Same goes for ` ```ax ` without a filename
(that's a syntax-highlighted snippet for the reader).

## File-write semantics (decision #5 from design doc)

- **First `+=` block for a filename:** truncates the file (or creates it). Content is written
  followed by a newline.
- **Subsequent `+=` blocks for the same filename:** append. A separator newline between blocks.
- **Across runs (re-invoking `boruna literate extract`):** same input file → same output. The
  first block always truncates. Idempotent.

This diverges from Quint (Quint pure-appends, requiring users to delete files between runs).
The divergence is intentional and documented in the design doc.

## CLI surface

```
boruna literate <SUBCOMMAND>

Subcommands:
  extract  Extract code fences from a literate markdown file

boruna literate extract <FILE>
  Extract `ax|boruna|quint <filename> +=` code fences into per-file outputs.

  FILE  Markdown file to extract from.

Options:
  --out-dir <PATH>   Output directory [default: ./gen]
  --json             Machine-readable report on stdout
  --verbose          List each block extracted
```

## CLI handler

`crates/llmvm-cli/src/literate.rs` (new file, ~80 lines):

```rust
pub fn run(file: &Path, out_dir: &Path, json: bool, verbose: bool) -> ExitCode {
    let md = fs::read_to_string(file)?;
    let opts = ExtractOptions::default();
    let report = literate::extract(&md, out_dir, &opts)?;
    if json {
        serde_json::to_writer(stdout(), &report)?;
    } else {
        print_human_report(&report, verbose);
    }
    ExitCode::SUCCESS
}
```

Wired into `main.rs`:

```rust
Command::Literate { sub } => match sub {
    LiterateCommand::Extract { file, out_dir, json, verbose } =>
        literate::run(&file, &out_dir, json, verbose),
}
```

## Dependencies

- `pulldown-cmark = { version = "0.10", default-features = false }` — pure-Rust CommonMark
  parser. Already widely used in the Rust ecosystem (mdbook, rustdoc).
- `serde_json` — already present in tooling.
- `thiserror` — already present.

## File map (new files this sprint)

| File | LoC est. |
|---|---|
| `tooling/src/literate/mod.rs` | ~200 |
| `tooling/src/literate/parse.rs` | ~120 |
| `tooling/src/literate/write.rs` | ~80 |
| `tooling/src/lib.rs` (add `pub mod literate;`) | +1 |
| `tooling/Cargo.toml` (add `pulldown-cmark`) | +1 |
| `tooling/tests/literate.rs` | ~200 |
| `crates/llmvm-cli/src/literate.rs` | ~80 |
| `crates/llmvm-cli/src/main.rs` (subcommand wiring) | +30 |
| `crates/llmvm-cli/tests/literate_cli.rs` | ~80 |

**Total: ~790 lines including tests.**

## Diagnostic shape

Per conventions §2 (typed `error_kind` strings):

| `error_kind` | When |
|---|---|
| `literate.io` | File read/write failure |
| `literate.invalid_fence` | Fence header malformed (e.g., missing `+=`) |
| `literate.path_traversal` | Filename contains `..` |
| `literate.absolute_path` | Filename is absolute |
| `literate.invalid_out_dir` | `--out-dir` not writable or invalid |

## Test plan reference

Test plan: `docs/test-plan-literate-workflows.md` (written this sprint).
