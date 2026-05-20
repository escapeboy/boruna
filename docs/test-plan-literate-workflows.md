# Test plan — Literate workflow extraction

For `boruna literate extract`. Architecture: `docs/architecture-literate-workflows.md`.

## Acceptance criteria

1. Extracting a markdown file with N `ax|boruna|quint <name> +=` fences produces N file
   contributions (where multiple fences may target the same file).
2. Re-running extraction is idempotent — byte-for-byte identical output.
3. Path traversal is rejected at parse time, not at filesystem-write time.
4. Diagnostics include the source markdown line number.
5. Non-Boruna fences (`rust`, `bash`, etc.) are silently ignored.
6. CLI exits 0 on success; non-zero on any error.

## Unit tests (in `tooling/tests/literate.rs`)

### Happy path

| ID | Input | Expected |
|---|---|---|
| L01 | one fence `ax foo.ax +=` with `let x = 1` | `gen/foo.ax` exists, contains `let x = 1\n` |
| L02 | two fences for `foo.ax`, second appending | `gen/foo.ax` = block1 + `\n` + block2 + `\n` |
| L03 | fences for `foo.ax` and `bar.ax` interleaved | both files exist, content per source order |
| L04 | re-extraction (same input twice) | second run leaves identical output |
| L05 | `quint`-tagged fence | extracted (the spec accepts quint, boruna, ax) |
| L06 | `boruna`-tagged fence | extracted |

### Ignored / no-op cases

| ID | Input | Expected |
|---|---|---|
| L07 | ` ```rust ... ``` ` fence | no file written, count = 0 |
| L08 | ` ```ax ` (no filename, no `+=`) | no file written, no error |
| L09 | code fence inside blockquote | extracted (markdown semantics) |

### Error cases

| ID | Input | Expected error_kind |
|---|---|---|
| L10 | ` ```ax ../etc.ax += ` | `literate.path_traversal` |
| L11 | ` ```ax /etc/foo += ` | `literate.absolute_path` |
| L12 | ` ```ax foo.ax = ` (missing `+=`) | `literate.invalid_fence` |
| L13 | `--out-dir` is an existing file (not dir) | `literate.invalid_out_dir` |
| L14 | input markdown not found | `literate.io` |

### Determinism / drift

| ID | Concern | Test |
|---|---|---|
| L15 | Path canonicalization | symlink in `out_dir` does not allow escape |
| L16 | UTF-8 in fence content | round-trips byte-for-byte |
| L17 | CRLF line endings in markdown | extracted content normalized to LF (or preserved — decision: preserve) |
| L18 | Empty code fence | written as empty file (creation, no content) |

## CLI tests (in `crates/llmvm-cli/tests/literate_cli.rs`)

### CLI happy path

| ID | Args | Expected |
|---|---|---|
| C01 | `boruna literate extract foo.md` | exit 0, files in `./gen/`, human report on stdout |
| C02 | `boruna literate extract foo.md --out-dir custom/` | files in `./custom/` |
| C03 | `boruna literate extract foo.md --json` | exit 0, valid JSON on stdout |
| C04 | `boruna literate extract foo.md --verbose` | exit 0, per-block listing on stdout |

### CLI error cases

| ID | Args | Expected |
|---|---|---|
| C05 | `boruna literate extract` (no file) | exit 2, clap-formatted usage |
| C06 | `boruna literate extract missing.md` | exit 1, error_kind: `literate.io` |
| C07 | `boruna literate extract bad-fence.md` (with `..` in filename) | exit 1, error_kind: `literate.path_traversal` |

## Integration test (golden)

Create `examples/compliance/soc2_audit_workflow.md` (or similar fixture) as a literate spec
that, when extracted, produces a `workflow validate`-passing workflow. Test asserts:

1. `boruna literate extract fixture.md --out-dir tmp/` exits 0
2. `boruna workflow validate tmp/` exits 0
3. `boruna workflow run tmp/ --policy deny-all --inputs '{...}' --record` produces an evidence
   bundle that `evidence verify` accepts

This catches "the extraction works mechanically but the resulting workflow is broken." Has the
side benefit of giving us the FIRST literate compliance fixture in `examples/`.

## Edge cases the parser MUST handle

1. Code fence with backtick-count > 3 (4+ backticks) — pulldown-cmark handles natively.
2. Fence info string with trailing spaces — trim before parsing the header.
3. Nested code fences inside markdown blockquotes — pulldown-cmark handles.
4. Multi-line fence body with embedded ` ``` ` fragments — pulldown-cmark handles.
5. CRLF vs LF in the markdown file — pulldown-cmark normalizes.

## Out of scope (deferred)

- Watch mode (`--watch`)
- Round-trip (`.ax` → markdown)
- HTML/PDF rendering
- LSP integration of literate diagnostics
