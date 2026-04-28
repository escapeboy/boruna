# Contributing to Boruna

Thank you for your interest in Boruna. Contributions of all kinds are welcome — bug reports, documentation improvements, and code changes.

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). By participating, you agree to uphold these standards.

## What can I work on?

- Issues labeled [`good-first-issue`](https://github.com/escapeboy/boruna/labels/good-first-issue) — well-scoped bugs and documentation improvements, good for new contributors
- Issues labeled [`help-wanted`](https://github.com/escapeboy/boruna/labels/help-wanted) — features and improvements the maintainers want help with
- Bug reports — file an issue first, discuss the approach, then submit a fix
- Documentation — always welcome, no discussion needed before submitting

## Development Setup

**Prerequisites**: Rust stable toolchain (edition 2021)

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone the repository
git clone https://github.com/escapeboy/boruna.git
cd boruna

# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace
```

## Project Structure

Directory names use the original naming from the project's history. Crate names map to Boruna names:

| Directory | Crate |
|-----------|-------|
| `crates/llmbc/` | `boruna-bytecode` |
| `crates/llmc/` | `boruna-compiler` |
| `crates/llmvm/` | `boruna-vm` |
| `crates/llmvm-cli/` | `boruna-cli` |
| `crates/llmfw/` | `boruna-framework` |
| `crates/llm-effect/` | `boruna-effect` |
| `orchestrator/` | `boruna-orchestrator` |
| `packages/` | `boruna-pkg` |
| `tooling/` | `boruna-tooling` |

See [docs/RENAMING.md](docs/RENAMING.md) for the full naming history.

## Development Workflow

```bash
# Create a branch
git checkout -b feat/my-change

# Make your changes
# ...

# Run tests (must all pass)
cargo test --workspace

# Run linter (must be warning-free)
cargo clippy --workspace -- -D warnings

# Check formatting
cargo fmt --all -- --check

# Fix formatting
cargo fmt --all
```

## Critical Invariants

These must not be violated:

1. **Never break determinism** — use `BTreeMap`, never `HashMap` in deterministic code paths. No randomness, no time-dependent behavior in pure functions.
2. **Declare all capabilities** — functions with side effects must have `!{capability}` annotations.
3. **All tests must pass** — `cargo test --workspace` must pass with no failures.
4. **Zero clippy warnings** — `cargo clippy --workspace -- -D warnings` must be clean.

## Submitting a Pull Request

1. Fork the repository and create a branch from `master`
2. Make your changes following the invariants above
3. Add tests for any new behavior
4. Add a `CHANGELOG.md` entry under `## [Unreleased]` describing your change
5. Ensure `cargo test --workspace`, `cargo clippy`, and `cargo fmt` all pass
6. Submit a pull request with a clear description of what and why

**PR checklist:**
- [ ] Tests pass (`cargo test --workspace`)
- [ ] No clippy warnings (`cargo clippy --workspace -- -D warnings`)
- [ ] Code is formatted (`cargo fmt --all`)
- [ ] `CHANGELOG.md` updated under `[Unreleased]`
- [ ] New behavior has test coverage

## Reading the bench-compare PR comment

Every PR that touches non-doc code triggers a `Bench compare` job
that runs the criterion bench harness on the PR base and on the
PR head, then posts a sticky comment with per-benchmark deltas.

The comment is a simple table:

| Benchmark | Mean change | 99% CI |
|-----------|-------------|--------|
| `compile/small_program` | `-1.20%` | `[-3.10%, +0.50%]` |
| `vm_throughput/loop_1k` | `+0.45%` | `[-1.20%, +2.10%]` |

How to read it:

- **Mean change**: positive means slower, negative means faster.
  Roughly: `+5%` is "noticeable", `+10%` is the regression
  threshold, `-10%` is "you sped something up — say so in the
  PR body."
- **99% CI**: the confidence interval on the mean. If the CI
  spans zero, the change is not statistically distinguishable
  from noise.
- **Threshold**: 10% slower mean fails the job. The job is
  intentionally NOT in the required-status-checks set, so a
  failing `Bench compare` does not block merge by default.
  Reviewers should ask "is this regression intentional?" and
  expect a documented answer in the PR body.
- **CI runner**: bench-compare runs on `self-hosted`. Hosted
  GitHub runners are noisy enough that the deltas would not be
  trustworthy; the self-hosted box is a stable reference.

If a `Bench compare` job fails on a PR that is genuinely not
perf-relevant (a docs-only change misclassified, a test-only
change), trigger a re-run from the Actions UI. If the
regression reproduces on a docs-only change, the path filter
in `.github/workflows/bench-compare.yml` may need tightening —
file a follow-up issue.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
