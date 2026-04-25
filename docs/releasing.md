# Releasing Boruna

Boruna ships pre-built binaries on every Git tag matching `v*` via `.github/workflows/release.yml`.

## Cutting a release

1. Update `CHANGELOG.md`: move entries from `[Unreleased]` to a new `[X.Y.Z] - YYYY-MM-DD` section. Add the comparison link at the bottom.
2. Bump `version` in the workspace `Cargo.toml` if appropriate (workspace package version is inherited by all member crates).
3. Commit on `master`:
   ```bash
   git commit -am "release: vX.Y.Z"
   ```
4. Tag and push:
   ```bash
   git tag -a vX.Y.Z -m "Boruna vX.Y.Z"
   git push origin master
   git push origin vX.Y.Z
   ```
5. The `Release` workflow runs automatically. After ~5–10 minutes a GitHub Release will be published containing:
   - `boruna-X.Y.Z-x86_64-unknown-linux-musl.tar.gz`
   - `boruna-X.Y.Z-aarch64-unknown-linux-musl.tar.gz`
   - `boruna-X.Y.Z-aarch64-apple-darwin.tar.gz`
   - `SHA256SUMS` (combined checksums)

> **Intel Mac (`x86_64-apple-darwin`) is not shipped as a release artifact.** Apple Silicon (`aarch64-apple-darwin`) covers ~all current macOS users. Intel Mac users build from source. Re-add the target to `.github/workflows/release.yml` if real demand emerges.

Each tarball contains:

```
boruna-X.Y.Z-<target>/
  ├── boruna           # main CLI
  ├── boruna-mcp       # MCP server for AI agents
  ├── boruna-pkg       # package manager
  ├── boruna-orch      # standalone orchestrator
  ├── LICENSE
  ├── README.md
  └── CHANGELOG.md
```

The Linux binaries are statically linked against musl libc — they run on any modern Linux distribution (Alpine, Ubuntu, Debian, etc.) without a glibc dependency. This is what FleetQ-style integrators use to drop `boruna-mcp` into their `php-fpm-alpine` containers without needing the Rust toolchain.

## Verification

Consumers should verify checksums:

```bash
# Download the tarball + SHA256SUMS for your target
curl -fsSLO https://github.com/escapeboy/boruna/releases/download/vX.Y.Z/boruna-X.Y.Z-x86_64-unknown-linux-musl.tar.gz
curl -fsSLO https://github.com/escapeboy/boruna/releases/download/vX.Y.Z/SHA256SUMS

# Verify
grep boruna-X.Y.Z-x86_64-unknown-linux-musl.tar.gz SHA256SUMS | sha256sum -c -

# Extract and run
tar -xzf boruna-X.Y.Z-x86_64-unknown-linux-musl.tar.gz
./boruna-X.Y.Z-x86_64-unknown-linux-musl/boruna --version
```

## Manual / dry-run

To rehearse the workflow without tagging, trigger it manually:

```bash
gh workflow run release.yml --field tag=vX.Y.Z-rc1
```

This builds and publishes a Release named `vX.Y.Z-rc1`. Delete the rc release afterwards if you don't want it visible.

## Cross-compilation notes

- **Linux musl** targets use [`cross`](https://github.com/cross-rs/cross) so the workflow doesn't need to install per-target system libraries.
- **macOS arm64** target compiles natively on the GitHub-hosted `macos-14` runner. No `cross` needed.
- The `http` feature is **not** enabled in release builds — releases are the deterministic, no-network default. Integrators who want real HTTP rebuild from source with `--features boruna-vm/http`.

### Hybrid runner architecture

| Target | Runner | Why |
|---|---|---|
| `x86_64-unknown-linux-musl` | `self-hosted` (boruna-runner, Linux X64) | Same machine as `ci.yml` — warm cargo cache, no GitHub queue wait |
| `aarch64-unknown-linux-musl` | `self-hosted` (boruna-runner, Linux X64) | `cross` cross-compiles to aarch64 from x64 |
| `aarch64-apple-darwin` | `macos-14` (GitHub-hosted) | macOS targets cannot run on Linux self-hosted; we don't have an Apple Silicon self-hosted runner |

If GitHub `macos-14` runner queues become a problem, the answer is to register a self-hosted Apple Silicon runner with label `self-hosted` + `macOS` + `ARM64` and switch the matrix to it. Don't try to cross-compile macOS from Linux — Apple's signing tooling and the macOS SDK make that fragile.

## What to do if the release workflow fails

- **`cross` build fails on aarch64-musl**: usually a transient cargo registry issue on the self-hosted runner — re-run the failed job.
- **macos arm64 runner unavailable**: GitHub occasionally throttles `macos-14`. Wait 30 minutes and re-run.
- **`self-hosted` runner offline**: check that `boruna-runner` is up on the host (`systemctl --user status actions.runner.escapeboy-boruna.boruna-runner.service` or equivalent). Recent Rust toolchain updates on the host can break clippy with new lints — fix the code, don't pin Rust.
- **`gh release create` collision**: a release for that tag already exists. Either delete it via the GitHub UI or pick a new tag.
