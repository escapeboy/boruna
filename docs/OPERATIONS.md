# Operations Guide

## Running Workflows

### Validate
```bash
boruna workflow validate <workflow-dir>
```
Checks DAG structure, step references, input/output wiring, and cycle detection.

### Run
```bash
boruna workflow run <workflow-dir> --policy allow-all
boruna workflow run <workflow-dir> --policy deny-all
boruna workflow run <workflow-dir> --policy path/to/policy.json
```

### Run with Evidence Recording
```bash
boruna workflow run <workflow-dir> --policy allow-all --record
boruna workflow run <workflow-dir> --record --evidence-dir ./evidence
```
Produces an evidence bundle directory containing:
- `manifest.json` — bundle metadata with checksums
- `workflow.json` — workflow definition snapshot
- `policy.json` — policy snapshot
- `audit_log.json` — hash-chained audit entries
- `env_fingerprint.json` — runtime environment info
- `outputs/<step_id>/result.json` — per-step output data

## Verifying Evidence

```bash
boruna evidence verify <bundle-dir>
```
Checks:
- File checksums match manifest
- Audit log chain integrity
- Required files present

```bash
boruna evidence inspect <bundle-dir>
boruna evidence inspect <bundle-dir> --json
```
Displays bundle manifest details.

## Replay

For single-file execution:
```bash
boruna run app.ax --record trace.json
boruna replay app.axbc trace.json
```

For workflow-level replay, re-run the workflow in mock mode using recorded outputs.

## Observability

### Current
- CLI output shows per-step status, duration, and errors
- Evidence bundles capture all execution details
- Audit log provides ordered event history

### Planned (Gap)
- Structured JSON logging via `tracing` crate
- Metrics (latency per step, cache hit rate, budget consumption)
- OpenTelemetry trace export

## CI Integration

```bash
# Validate all example workflows
for dir in examples/workflows/*/; do
  boruna workflow validate "$dir"
done

# Run in mock mode and verify
boruna workflow run examples/workflows/llm_code_review --record
boruna evidence verify examples/workflows/llm_code_review/evidence/<run-id>
```

## Deployment

Boruna is a statically-linked Rust binary. Deploy by copying the binary to the target system.

```bash
cargo build --release --bin boruna
# Binary at: target/release/boruna
```

Daemon/service mode is documented as a P2 gap in `ENTERPRISE_GAPS.md`.
