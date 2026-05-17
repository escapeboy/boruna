# Test Plan — Agent-Native CLI surfaces

Companion to `docs/design-agent-native-cli.md`. Acceptance criteria + edge cases.

## 1. `lang codes`

- `boruna lang codes` → human table lists all 9 codes.
- `boruna lang codes --json` → valid JSON, `codes.len() == 9`, each has code/name/summary/category.
- **Drift test** (`tooling`): every `E0NN` `pub const` in `diagnostics/mod.rs` appears exactly
  once in `REGISTRY`; no registry entry lacks a backing const. Fails loudly on future drift.
- Registry codes are unique (no duplicate `code` values).

## 2. `doctor`

- `boruna doctor` → human report; exit 0 in a healthy checkout.
- `boruna doctor --json` → valid JSON with `ok`, `boruna_version`, `checks[]`.
- `boruna_version` equals `CARGO_PKG_VERSION`.
- Every check has a `status` in {ok,warn,error}; `ok == true` iff no `error` check.
- Missing `rustc` produces a `warn`, not an `error`, and does not abort.

## 3. `workflow graph`

- `boruna workflow graph examples/workflows/llm_code_review` → human summary.
- `--json` → nodes match the workflow's steps; edges match `WorkflowDef.edges`.
- `topological_order` is a valid topo order (every edge `(a,b)` has `a` before `b`).
- `roots` = steps with no dependencies; `leaves` = steps no step depends on.
- A workflow with a cycle → graph reports the cycle / non-DAG, exit non-zero.
- Missing directory → clean error, exit 1.

## 4. `size`

- `boruna size examples/hello.ax` → human table with per-function rows + totals.
- `--json` → `functions[]`, `totals`, `bytecode_bytes > 0`, `bytecode_format` set.
- `totals.total_ops` == sum of per-function `op_count`.
- A file with a compile error → error emitted, exit 1, no panic.
- Missing file → clean error, exit 1.

## 5. `skills`

- `boruna skills list` → all embedded skills with summaries.
- `boruna skills list --json` → JSON array, length == number of embedded docs.
- `boruna skills get ax-language` → prints non-empty markdown body.
- `boruna skills get ax-language --json` → `{ name, summary, content }`, content non-empty.
- `boruna skills get nonexistent` → exit 1, lists available skill names.
- Every embedded skill body is non-empty (compile-time `include_str!` guarantees existence).

## Regression gates (convention §30, §32)

- `cargo test --workspace` — all 557+ existing tests still pass.
- `cargo clippy --workspace --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --all -- --check` — clean.
- `cargo build --workspace` — clean.

## Test placement

- Registry drift test → `tooling/src/diagnostics/` test module (or `tooling/src/tests.rs`).
- CLI surface tests → `crates/llmvm-cli/tests/cli_agent_native.rs` (new integration test file,
  follows the existing `cli_*.rs` pattern), invoking the built `boruna` binary.
