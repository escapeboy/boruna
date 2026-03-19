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

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
