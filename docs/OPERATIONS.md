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
Produces an evidence bundle directory `<evidence-dir>/<run-id>/` containing:
- `manifest.json` — bundle hash, audit log hash, policy/workflow hashes and per-file checksums
- `bundle.json` — bundle format version, Boruna version, run ID, workflow hash
- `workflow.json` — workflow definition snapshot
- `policy.json` — policy snapshot
- `audit_log.json` — hash-chained audit entries
- `env_fingerprint.json` — runtime environment info

Per-step outputs are not written by `workflow run --record`. `boruna evidence create` builds a bundle from a persisted run in `--data-dir` and does include per-step outputs.

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

Other `boruna evidence` subcommands (see `boruna evidence --help` and [CLI reference](reference/cli.md)):

| Command | Purpose |
|---------|---------|
| `create` | Build a bundle from a persisted run in `--data-dir` |
| `diff` | Compare two bundles |
| `redact` | Verifiably redact one audit-log entry (e.g. PII) without breaking verification |
| `attest` | Emit or verify an in-toto Statement + DSSE envelope |
| `anchor` | Anchor a signed bundle in a Sigstore Rekor transparency log |
| `report` | Compliance evidence-mapping report (not a certificate of compliance) |
| `otel` | Export the run as OpenTelemetry spans in OTLP/JSON |
| `rotate-kek` | Rotate the key-encryption key on encrypted bundles |
| `gc-blobs` | Remove orphan blobs from the data dir |

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
- `boruna metrics export --data-dir <dir>` prints Prometheus text metrics from the persistent run store: `boruna_workflow_runs_total{workflow,status}`, `boruna_workflow_runs_in_flight{workflow}`, `boruna_workflow_step_completions_total{workflow,step,status}`. There is no HTTP endpoint; write the output to a file for `node_exporter`'s textfile collector (see `docs/design-prometheus-metrics.md`)
- `boruna evidence otel <bundle-dir>` exports a recorded run as OpenTelemetry spans in OTLP/JSON. It makes no network calls; send the file to a collector yourself

### Planned (Gap)
- Structured JSON logging via `tracing` crate
- Metrics for per-step latency, cache hit rate and budget consumption

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

Daemon/service mode is documented as a P2 gap in [`archive/ENTERPRISE_GAPS.md`](archive/ENTERPRISE_GAPS.md).
