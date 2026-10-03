# Boruna

[![CI](https://github.com/escapeboy/boruna/actions/workflows/ci.yml/badge.svg)](https://github.com/escapeboy/boruna/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-3.0.0-blue.svg)](CHANGELOG.md)
[![Status: Stable](https://img.shields.io/badge/status-stable-green.svg)](docs/stability.md)

> **v3.0.0 is the current release.** The 1.x line remains under long-term support — active through 2027-11-15, security through 2028-05-15. See [`docs/lts.md`](./docs/lts.md) for support windows, deprecation policy, and security-backport SLAs.

**Deterministic, policy-gated workflow execution for AI systems that must be auditable.**

---

## The problem

Most AI orchestration tools run workflows and return outputs. When something goes wrong — or when a regulator asks — there is no reliable way to answer: *What exactly ran? What did the model see? What did it return? Can you prove it?*

Boruna answers those questions by design.

Every Boruna workflow run can produce a **tamper-evident evidence bundle**: a hash-chained audit log that records, for every step, the hash of the inputs it ran on, every capability it called (allowed or denied) and the hash of its output, plus every failure, every approval or rejection, and every confidence-gate decision. Anyone can inspect and verify the bundle offline, without a central server and without trusting anyone's word.

This makes Boruna suited for teams building AI workflows that touch regulated data, make consequential decisions, or need a defensible audit trail.

## What Boruna provides

- **DAG workflow execution** — steps are `.ax` source files; the workflow is a `workflow.json` DAG definition (`schema_version: 1` frozen at 1.0)
- **Capability enforcement** — every side effect (LLM calls, HTTP, database, filesystem) is declared and policy-gated at the VM level
- **Evidence bundles** — hash-chained tamper-evident logs, written automatically with `--record`. Optional **AES-256-GCM envelope encryption** for compliance-sensitive deployments. `evidence inspect` shows step output content for plaintext bundles.
- **Deterministic replay** — re-execute any recorded workflow with identical outputs, verified by the VM
- **Approval gates** — pause workflow execution for human review or external triggers before continuing
- **Diagnostics, auto-repair, and migration** — `boruna lang check`, `boruna lang repair`, `boruna migrate` for `.ax` files and bundle/workflow upgrades
- **`boruna new`** — interactive scaffold for new workflows from templates
- **33 built-in functions** — string (12), list (7), and map (7) operations plus type conversions and debug builtins (`__builtin_string_*`, `__builtin_list_*`, `__builtin_map_*`, …) available in every `.ax` file without imports
- **Import resolution** — `import "std-name"` inlines `libs/<name>/src/core.ax` at compile time; 14 stdlib packages (the original 13 are 1.0-stable)
- **Four formal versioned specifications** — `.ax` language 1.0, bytecode 1.0, evidence bundle format 1.0, workflow DAG schema 1.0 (all under [`docs/spec/`](./docs/spec/))
- **MCP server** — exposes 14 tools for AI coding agent integration (Claude Code, Cursor, Codex)

## What Boruna is not

- Not a general-purpose language or runtime (use Rust, Python, Go for that)
- Not an LLM framework (use LangChain, LCEL, etc. for prompt engineering)
- Not a cloud service (Boruna runs wherever you deploy it)
- Not a key-management system (operators wire HSM / KMS integration themselves; bundle-encryption KEK lifecycle is operator-owned)

## Install

Linux and macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/escapeboy/boruna/master/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/escapeboy/boruna/master/install.ps1 | iex
```

Both scripts pick the right build for your OS and CPU, check it against the release's `SHA256SUMS`
and refuse to install on a mismatch, then put `boruna`, `boruna-mcp`, `boruna-pkg` and
`boruna-orch` in `~/.local/bin` (Windows: `%LOCALAPPDATA%\Programs\boruna\bin`, added to your
user PATH). Set `BORUNA_VERSION=v3.4.0` to pin a version or `BORUNA_INSTALL_DIR` to choose the
folder. Read [`install.sh`](install.sh) / [`install.ps1`](install.ps1) first if you prefer not to
pipe a script into your shell.

Check the installation with `boruna --version` and `boruna doctor`.

<details>
<summary>Manual download with checksum check</summary>

```bash
# Linux x86_64 (musl — works on Alpine, Ubuntu, Debian, ...)
curl -fsSL https://github.com/escapeboy/boruna/releases/latest/download/SHA256SUMS -o SHA256SUMS
TARGET=x86_64-unknown-linux-musl
TAR=$(grep "$TARGET" SHA256SUMS | awk '{print $2}')
curl -fsSLO "https://github.com/escapeboy/boruna/releases/latest/download/$TAR"
grep "$TAR" SHA256SUMS | sha256sum -c -
tar -xzf "$TAR"
./boruna-*-${TARGET}/boruna --version
```

Windows (PowerShell):

```powershell
$base = "https://github.com/escapeboy/boruna/releases/latest/download"
Invoke-WebRequest "$base/SHA256SUMS" -OutFile SHA256SUMS
$target = "x86_64-pc-windows-msvc"   # or aarch64-pc-windows-msvc on Windows on Arm
$zip = ((Select-String -Path SHA256SUMS -Pattern $target).Line -split "\s+")[1].TrimStart("*")
Invoke-WebRequest "$base/$zip" -OutFile $zip
# Compare this hash with the line for $zip in SHA256SUMS:
(Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
Expand-Archive $zip -DestinationPath .
.\boruna-*-$target\boruna.exe --version
```

</details>

### Supported platforms

Every platform below is built and its full test suite is run natively on a real machine of that
type in CI (no emulation), together with an example workflow and `evidence verify`.

| Platform | Release asset | Tested natively in CI |
|---|---|---|
| Linux x86_64 (musl) | `x86_64-unknown-linux-musl` `.tar.gz` | yes (tests run on a glibc build; the musl release binary is cross-built) |
| Linux arm64 (musl) | `aarch64-unknown-linux-musl` `.tar.gz` | yes (glibc build; the musl release binary is cross-built) |
| macOS Apple Silicon | `aarch64-apple-darwin` `.tar.gz` | yes |
| macOS Intel | `x86_64-apple-darwin` `.tar.gz` | yes |
| Windows x64 | `x86_64-pc-windows-msvc` `.zip` | yes |
| Windows on Arm | `aarch64-pc-windows-msvc` `.zip` | yes |

Other targets (FreeBSD, 32-bit, RISC-V and so on) are not built or tested; build from source and run
`cargo test --workspace` to see whether they work for you. See [`docs/releasing.md`](docs/releasing.md).

Or build from source:

```bash
git clone https://github.com/escapeboy/boruna
cd boruna
cargo build --workspace --release
```

## Quickstart

```bash
# Run a workflow
boruna workflow run examples/workflows/llm_code_review --policy allow-all --record

# Verify the evidence bundle
boruna evidence verify .boruna/runs/<run-id>/
```

**→ [Full Quickstart](docs/QUICKSTART.md)** — 10 minutes, ends with a verified evidence bundle.

## Example workflows

| Workflow | Pattern | What it shows |
|----------|---------|---------------|
| [LLM Code Review](examples/workflows/llm_code_review/) | Linear, 3 steps | LLM capability, data flow, evidence recording |
| [Document Processing](examples/workflows/document_processing/) | Fan-out, 5 steps | Parallel steps, multi-input merge |
| [Customer Support Triage](examples/workflows/customer_support_triage/) | Approval gate | Human-in-the-loop, conditional pause, audit trail |

Each example runs in demo mode (no external services) and produces a verifiable evidence bundle.

## How the evidence guarantee works

```
workflow.json  →  DAG Validator  →  Step Runner
                                        ↓
                                   .ax source
                                        ↓
                                   Compiler → Bytecode
                                        ↓
                                   VM (capability gateway)
                                        ↓
                                   EventLog entry (CapCall + CapResult)
                                        ↓
                              Hash-chained audit log

boruna evidence verify <bundle>
  → Chain integrity: VALID
  → All step hashes: MATCH
  → Verification: PASSED
```

Every `CapCall` (including LLM calls) is logged with its full response. The log is SHA-256 hash-chained from a genesis entry containing the workflow definition hash. Modification of any entry breaks the chain.

## Architecture

Boruna is a Rust workspace with 10 production crates plus a `benches/` member:

| Crate | Purpose |
|-------|---------|
| `boruna-orchestrator` | Workflow engine, DAG execution, evidence bundles |
| `boruna-vm` | Bytecode VM, capability gateway, actor system, replay |
| `boruna-compiler` | Lexer, parser, type checker, code generator |
| `boruna-bytecode` | Opcodes, Module, Value, Capability definitions |
| `boruna-framework` | Elm-architecture runtime, test harness |
| `boruna-effect` | LLM integration, prompt management, caching |
| `boruna-cli` | CLI binary (`boruna`) |
| `boruna-tooling` | Diagnostics, repair, trace-to-tests, templates |
| `boruna-pkg` | Package registry, resolver, lockfiles |

1175+ tests across 11 workspace members. `cargo test --workspace` — all pass.

## Documentation

| | |
|---|---|
| [Quickstart](docs/QUICKSTART.md) | Build, run a workflow, inspect evidence |
| [Concepts: Determinism](docs/concepts/determinism.md) | Why and how determinism is enforced |
| [Concepts: Capabilities](docs/concepts/capabilities.md) | Side effect declaration and policy gating |
| [Concepts: Evidence Bundles](docs/concepts/evidence-bundles.md) | Hash-chained audit logs and replay |
| [Guide: First Workflow](docs/guides/first-workflow.md) | Build a workflow from scratch |
| [Guide: Migration](docs/guides/migration.md) | Upgrade legacy bundles and workflow files |
| [Spec: `.ax` Language 1.0](docs/spec/ax-language-1.0.md) | Formal language specification |
| [Spec: Workflow DAG 1.0](docs/spec/workflow-dag-1.0.md) | `workflow.json` schema |
| [Spec: Evidence Bundle 1.0](docs/spec/evidence-bundle-1.0.md) | Bundle format + encryption envelope |
| [Reference: CLI](docs/reference/cli.md) | All `boruna` commands |
| [LTS contract](docs/lts.md) | Support windows + deprecation policy for 1.x |
| [Performance](docs/PERFORMANCE.md) | Baseline numbers + 1.x performance budget |
| [Stability](docs/stability.md) | What is stable, experimental, and planned |
| [Roadmap](docs/roadmap.md) | 0.2.0 → 1.0.0 → 1.x |
| [Limitations](docs/limitations.md) | Real constraints, stated honestly |
| [FAQ](docs/faq.md) | Common questions |
| [All docs →](docs/README.md) | Full documentation index |

## Status

Boruna is at **v3.5.0**: a **local deterministic engine and CLI**. Since v3.0.0 there is no HTTP server, coordinator, dashboard or `serve` feature. Recent releases added signed, redactable and Rekor-anchorable evidence bundles with compliance reports (3.1–3.2), calibrated confidence gates for approvals (3.3), native builds and tests for macOS, Windows and Linux on x86_64 and Arm (3.4), and one-line installers, language 1.1 and several `match` fixes (3.5). See the [CHANGELOG](CHANGELOG.md).

The project is suited for evaluation, internal tooling, and audit-sensitive AI pipelines. **Operator action**: validate the [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) budget against your workload, and review [`docs/limitations.md`](docs/limitations.md) for known constraints. External security audit booking is the Q4 2026 commitment in `lts.md`.

See [docs/stability.md](docs/stability.md) for the stability tier breakdown.

## For coding agents

Boruna exposes an MCP server for AI coding agent integration. See [AGENTS.md](AGENTS.md) for integration instructions and the tool reference.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version: open an issue, implement with tests, run `cargo test --workspace` + `cargo clippy` + `cargo fmt`, add a CHANGELOG entry, open a PR.

## License

[MIT](LICENSE) — Copyright 2026 Boruna Contributors
