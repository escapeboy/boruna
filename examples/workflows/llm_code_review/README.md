# LLM Code Review Workflow

**Pattern**: Linear pipeline (3 steps)
**Demonstrates**: LLM capability gating, data flow between steps, evidence recording

## Use case

Your team reviews 50+ pull requests per day. This workflow automates pre-screening: it fetches the diff, sends it through an LLM analyzer for security and style review, and formats the findings as a structured report — with a full audit trail showing which model analyzed which diff under which policy.

Every run produces a verifiable evidence bundle that can be attached to your PR review record.

## Steps

| Step | Role | Capability |
|------|------|-----------|
| `fetch_diff` | Retrieves the PR diff from a source | `net.fetch` (live mode) |
| `analyze` | Sends diff to LLM for security and style analysis | `llm.call` (live mode) |
| `report` | Formats findings as a structured review report | (pure) |

## How to run

```bash
# Validate the workflow definition
cargo run --bin boruna -- workflow validate examples/workflows/llm_code_review

# Run in demo mode (no network calls)
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review --policy allow-all

# Run and record an evidence bundle
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review --policy allow-all --record

# Verify the evidence bundle
cargo run --bin boruna -- evidence verify examples/workflows/llm_code_review/evidence/<run-id>
```

## Evidence produced

Each `--record` run writes a self-contained bundle to `evidence/run-llm-code-review-<timestamp>/`:

- `audit_log.json` — hash-chained log of all 3 step executions
- `policy.json` — snapshot of the policy applied during this run
- `workflow.json` — snapshot of the workflow definition
- `outputs/` — per-step output values
- `env.json` — environment fingerprint (platform, binary hash)

The bundle can be verified at any time:

```
boruna evidence verify evidence/run-llm-code-review-20260319T143200Z/
# Integrity: VERIFIED (3/3 steps, SHA-256 chain valid)
```

## Notes

- In demo mode, `fetch_diff` returns a representative diff and `analyze` returns deterministic findings.
- In live mode (`--live` flag, requires `boruna-cli/http` feature), steps call real endpoints.
- All capability declarations (`!{net.fetch}`, `!{llm.call}`) are enforced by the policy gateway even in demo mode — calls are blocked unless the policy allows them.
