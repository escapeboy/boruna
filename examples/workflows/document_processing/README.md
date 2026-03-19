# Document Processing Workflow

**Pattern**: Fan-out / merge (5 steps)
**Demonstrates**: Parallel step execution, multi-input merge, DAG scheduling

## Use case

Your team ingests 200+ documents per day — contracts, invoices, reports — and needs each one classified, entity-extracted, and summarized before it enters a review queue. This workflow processes a document in parallel across three specialized steps (classify, extract, summarize), then merges the results into a single structured record.

The fan-out pattern means the three parallel steps run concurrently after ingestion, with the merge step waiting for all three to complete before producing the final output.

## Steps

```
ingest ──┬──► classify ──┐
         ├──► extract  ──┼──► merge
         └──► summarize ─┘
```

| Step | Role | Capability |
|------|------|-----------|
| `ingest` | Load document from storage | `fs.read` (live mode) |
| `classify` | Determine document type and priority | `llm.call` (live mode) |
| `extract` | Extract named entities (parties, dates, amounts) | `llm.call` (live mode) |
| `summarize` | Generate a concise summary | `llm.call` (live mode) |
| `merge` | Combine all results into a structured record | (pure) |

## How to run

```bash
# Validate the workflow DAG
cargo run --bin boruna -- workflow validate examples/workflows/document_processing

# Run in demo mode
cargo run --bin boruna -- workflow run examples/workflows/document_processing --policy allow-all

# Run and record evidence
cargo run --bin boruna -- workflow run examples/workflows/document_processing --policy allow-all --record
```

## Evidence produced

Each `--record` run writes a bundle to `evidence/run-document-processing-<timestamp>/` containing:

- `audit_log.json` — hash-chained log of all 5 steps (includes parallel step entries)
- `policy.json` — policy snapshot
- `outputs/` — per-step output values (all 5 steps)
- `env.json` — environment fingerprint

## Notes

- The `classify`, `extract`, and `summarize` steps are independent and scheduled concurrently by the workflow runner.
- The `merge` step declares `depends_on: ["classify", "extract", "summarize"]` in `workflow.json` to enforce the merge barrier.
- In demo mode, all three parallel steps return realistic hardcoded outputs for the sample contract document.
