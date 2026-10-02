# Test plan: agent docs emit / pack

Unit tests in `crates/llmvm-cli/src/skills.rs`, integration tests in
`crates/llmvm-cli/tests/cli_skills_emit.rs` (runs the real binary).

| # | Case | Expected |
|---|---|---|
| 1 | `skills get cli` | contains every top-level subcommand reported by `boruna --help` |
| 2 | Drift guard | every `boruna <word>` command named in the four hand-written skills resolves to a real clap (sub)command; test fails with the offending skill and line otherwise |
| 3 | `skills emit <tmp>` | four `SKILL.md` files, each starting with `---\nname: boruna-` and a `description:` line |
| 4 | `skills emit` twice | second run leaves bytes identical |
| 5 | `skills emit` onto an existing regular file | exit 1, no panic |
| 6 | `skills pack "approval gate"` | first chunk is the generated `boruna workflow` section (it names approve/reject); output has at most budget tokens. Found during build: with one big reference section the best hit ate the whole budget, so the reference is split per top-level command |
| 7 | `skills pack` twice | identical bytes |
| 8 | `skills pack "zzzqqq"` | exit 1, message names the query |
| 9 | `--budget 1` on a matching query | still returns one truncated chunk, never empty |
| 10 | `--json` | parses, `tokens_estimated <= budget` unless the case-9 exception applies |

Edge cases: query of only short words (all dropped, treated as no match); a chunk with
no heading (preamble) is named `(intro)`; multibyte characters in truncation must cut on a
char boundary.

Gates: `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`,
`cargo test --workspace`.
