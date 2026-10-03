# Frequently Asked Questions

## What problem does Boruna solve?

Most AI workflow orchestration tools are wrappers around LLM API calls with some retry logic and prompt templates. They work fine for demos. They break down when you need to answer questions like: "What exactly did the model see?", "Can I prove this workflow ran correctly?", "If I re-run this tomorrow, will I get the same result?"

Boruna is built for teams that need answers to those questions — because they operate in regulated environments, because their AI workflows make consequential decisions, or because they've been burned by non-reproducible outputs.

The core value proposition: every workflow run produces a tamper-evident evidence bundle. You can verify it independently, replay it exactly, and present it in an audit.

## How is Boruna different from LangChain, LlamaIndex, or similar frameworks?

LangChain and similar frameworks are excellent for building LLM-powered applications quickly. They are not designed for auditability or deterministic replay.

Boruna's design priorities are different:

| | LangChain | Boruna |
|---|---|---|
| Primary goal | LLM integration | Deterministic execution |
| Side effects | Implicit | Declared and gated |
| Audit trail | Not built-in | Hash-chained evidence bundle (`--record`) |
| Replay | Not supported | First-class |
| Policy enforcement | None | Capability-based |
| Language | Python | .ax (custom, Rust VM) |

These are different tools for different requirements. A team building a chatbot should probably use LangChain. A team running AI-assisted financial analysis in a regulated environment should look at Boruna.

## How is Boruna different from Temporal or similar workflow engines?

Temporal is a durable workflow engine with excellent reliability guarantees. It is not specifically designed for AI workflows or LLM governance.

Boruna focuses on:
- **Capability policy**: declaring and enforcing what LLM calls and network requests are permitted
- **Evidence bundles**: cryptographic proof of what ran and what the model returned
- **Determinism as a contract**: same inputs → same outputs, enforced by the VM
- **The `.ax` language**: pure, typed, auditable step code that compiles to bytecode

Temporal is a better choice if you need multi-step business processes with human tasks, timers, and at-least-once execution guarantees. Boruna is a better choice if governance and auditability of AI-specific workflows is the primary concern.

## Is .ax Turing-complete?

Yes. `.ax` supports recursion and is Turing-complete in the theoretical sense. In practice, the `--max-steps` flag on `boruna run` (default 10,000,000) bounds the number of VM steps, which prevents runaway execution.

## Can I call external APIs and LLMs?

Yes, but they must be declared as capabilities. A step that calls an LLM must annotate its function with `!{llm.call}`. A step that makes HTTP requests must declare `!{net.fetch}`.

In demo mode (default), capability calls are stubbed. In `--live` mode (with the `http` feature), real HTTP requests are made against the SSRF-protected handler.

## Does Boruna guarantee that LLM outputs are reproducible?

No. LLMs are probabilistic. Running the same prompt twice will produce different outputs. Boruna cannot change this.

What Boruna guarantees: the response is recorded. `boruna run --record <file>` writes the VM event log with every capability result, and `boruna replay` substitutes the recorded results, so the replay is deterministic even if the original call wasn't. For workflows, each step's output hash is in the hash-chained audit log of the evidence bundle.

This lets you verify that a workflow produced a specific output on a specific run, without requiring the LLM to reproduce the response.

## What is the evidence bundle format?

A directory containing:
- `manifest.json` — run metadata, per-file SHA-256 checksums, `bundle_hash`, optional ed25519 signature and encryption info
- `bundle.json` — bundle format version (`1.1`) and component list
- `workflow.json`, `policy.json` — the exact workflow definition and policy used
- `audit_log.json` — hash-chained log of workflow, step and approval events
- `env_fingerprint.json` — runtime environment snapshot
- `outputs/<step-id>/result.json` — per-step outputs (written by `boruna evidence create`)

`boruna evidence verify <dir>` checks it. See [Evidence Bundles](./concepts/evidence-bundles.md) for the full specification.

## Can I use Boruna without the .ax language?

Not currently. The VM, capability enforcement, and determinism guarantees are tied to `.ax` bytecode. Compiling other languages to Boruna bytecode is technically possible but not a current project goal.

## What Rust version does Boruna require?

Release binaries (installed with `install.sh` or `install.ps1`) need no Rust toolchain. To build from source you need stable Rust; no nightly features are required. CI builds with the current stable toolchain only, and the workspace does not declare a minimum Rust version, so older toolchains are untested.

## Is there a hosted version?

No. Since v3.0.0 Boruna is a local engine and CLI only. The HTTP server, coordinator, distributed workers and dashboards were removed in v3.0.0.

## How do I report a security vulnerability?

See [SECURITY.md](../SECURITY.md). Use GitHub Security Advisories for responsible disclosure. Do not file a public issue.

## Is Boruna production-ready?

Boruna is at 3.5.0. It is a local engine and CLI; there is no server component since v3.0.0. The core engine, workflow DAG, evidence bundles and capability enforcement are covered by the test suite and CI on Linux, macOS and Windows, and there are 14 stdlib packages. No external security audit has been completed yet. See [Stability](./stability.md) for the maturity assessment and [lts.md](./lts.md) for the support contract (written for the 1.x line).

## How do I contribute?

See [CONTRIBUTING.md](../CONTRIBUTING.md). The short version: open an issue, fork, implement, run `cargo test --workspace` + `cargo clippy` + `cargo fmt`, and open a PR with a CHANGELOG entry.
