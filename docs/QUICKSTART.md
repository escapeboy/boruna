# Quickstart

Get from zero to a running workflow with an evidence bundle in about 10 minutes.

## Prerequisites

- Rust 1.75+ (`rustup update stable`)
- Git

## 1. Build

```bash
git clone https://github.com/escapeboy/boruna
cd boruna
cargo build --workspace
```

This builds all 9 crates. Expect 1-2 minutes on first build.

## 2. Run hello world

```bash
cargo run --bin boruna -- run examples/hello.ax
```

Expected output:

```
Hello, Boruna!
```

This compiles `hello.ax` to bytecode and runs it on the VM. No capabilities are needed.

## 3. Run a workflow

Boruna workflows are DAGs — directed acyclic graphs of steps. Each step is a `.ax` file that compiles independently and runs in isolation.

Run the LLM code review workflow (in demo mode — no real LLM calls):

```bash
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review \
  --policy allow-all
```

Expected output:

```
Running workflow: llm_code_review

  [1/3] fetch_diff    → ok
  [2/3] analyze       → ok
  [3/3] report        → ok

Workflow completed in 0.03s
```

Three steps ran in topological order: fetch a diff, analyze it, produce a report. In demo mode the steps return representative data without calling any external services.

## 4. Record an evidence bundle

Add `--record` to capture a tamper-evident log of the run:

```bash
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review \
  --policy allow-all --record
```

Expected output:

```
Running workflow: llm_code_review

  [1/3] fetch_diff    → ok
  [2/3] analyze       → ok
  [3/3] report        → ok

Workflow completed in 0.03s
Bundle written to: .boruna/runs/20260319-120000-abc12/
```

## 5. Inspect the evidence bundle

```bash
cargo run --bin boruna -- evidence inspect .boruna/runs/20260319-120000-abc12/
```

Expected output:

```
Run ID:     20260319-120000-abc12
Workflow:   llm_code_review
Started:    2026-03-19T12:00:00Z
Completed:  2026-03-19T12:00:00Z
Policy:     allow-all
Steps:      3 completed, 0 failed

Step Results:
  fetch_diff   → ok  (0.0s)
  analyze      → ok  (0.0s)
  report       → ok  (0.0s)

Chain:      valid (3 entries, no gaps)
```

## 6. Verify the evidence bundle

```bash
cargo run --bin boruna -- evidence verify .boruna/runs/20260319-120000-abc12/
```

Expected output:

```
Chain integrity: VALID
All step hashes: MATCH
Environment fingerprint: PRESENT
Verification: PASSED
```

The hash chain is unbroken. No step output was modified. This is what makes Boruna useful for audit: you can present this bundle, and anyone with the `boruna` binary can verify it independently.

## What you just saw

- **Deterministic execution**: same workflow definition → same outputs, every time
- **Capability policy**: `--policy allow-all` controls what side effects are permitted
- **Evidence bundle**: a tamper-evident directory written alongside every recorded run
- **Independent verification**: `evidence verify` needs no network access, no central server

## Try the other workflows

```bash
# Document processing with fan-out parallelism
cargo run --bin boruna -- workflow run examples/workflows/document_processing \
  --policy allow-all --record

# Customer support triage with an approval gate
cargo run --bin boruna -- workflow run examples/workflows/customer_support_triage \
  --policy allow-all --record
```

## Run the tests

```bash
cargo test --workspace
```

557+ tests across all crates. All should pass.

## Next steps

- **Write a workflow**: [Your First Workflow](./guides/first-workflow.md)
- **Understand the language**: [.ax Language Reference](./reference/ax-language.md)
- **Understand determinism**: [Determinism](./concepts/determinism.md)
- **Understand capabilities**: [Capabilities](./concepts/capabilities.md)
- **See all CLI commands**: [CLI Reference](./reference/cli.md)
- **Check maturity status**: [Stability](./stability.md)
