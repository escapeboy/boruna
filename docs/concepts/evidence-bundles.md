# Evidence Bundles

An evidence bundle is the tamper-evident record of a workflow execution. It contains everything needed to prove what ran, when it ran, what inputs were used, what capabilities were invoked, and what outputs were produced.

Evidence bundles are the mechanism through which Boruna supports compliance, audit, and replay.

## What an evidence bundle contains

```
.boruna/runs/<run-id>/
├── manifest.json          # Run metadata: workflow ID, start time, policy, step list
├── audit_log.json         # Hash-chained log of every step execution
├── events/
│   └── event_log.json     # Full CapCall/CapResult/actor event stream
├── steps/
│   ├── <step-id>.input    # Step input values (serialized)
│   └── <step-id>.output   # Step output values (serialized)
└── env_fingerprint.json   # Runtime environment: OS, Boruna version, hash of workflow def
```

## Hash-chained audit log

The audit log is hash-chained: each entry includes the SHA-256 hash of the previous entry. This makes it impossible to insert, delete, or modify a log entry without breaking the chain.

Each log entry records:
- Step ID and source file hash
- Start time and end time
- Policy in effect
- Capability calls made
- Output value hash
- Previous entry hash

The chain starts with a genesis entry that includes the workflow definition hash and the environment fingerprint.

## Generating a bundle

Pass `--record` to `workflow run`:

```bash
boruna workflow run examples/workflows/llm_code_review \
  --policy allow-all \
  --record

# Output:
# Bundle written to: .boruna/runs/20260315-143022-abc4d/
```

## Inspecting a bundle

```bash
# Summary view
boruna evidence inspect .boruna/runs/20260315-143022-abc4d/

# Full JSON output
boruna evidence inspect .boruna/runs/20260315-143022-abc4d/ --json
```

Example output:

```
Run ID:     20260315-143022-abc4d
Workflow:   llm_code_review
Started:    2026-03-15T14:30:22Z
Completed:  2026-03-15T14:30:31Z
Policy:     allow-all
Steps:      3 completed, 0 failed

Step Results:
  fetch_diff   → ok  (0.1s)
  analyze      → ok  (6.8s)  [llm.call: 1 invocation, 312 tokens]
  report       → ok  (0.0s)

Chain:      valid (3 entries, no gaps)
```

## Verifying a bundle

```bash
boruna evidence verify .boruna/runs/20260315-143022-abc4d/

# Output:
# Chain integrity: VALID
# All step hashes: MATCH
# Environment fingerprint: PRESENT
# Verification: PASSED
```

Verification checks:
1. Hash chain is unbroken from genesis to final entry
2. Step output hashes match recorded values
3. Workflow definition hash matches the definition on disk
4. Environment fingerprint is present and well-formed

A failed verification means either the bundle was tampered with, or the workflow definition changed since the run.

## Replaying from a bundle

The evidence bundle contains everything needed to re-execute the workflow with the same capability responses:

```bash
boruna workflow run examples/workflows/llm_code_review \
  --replay .boruna/runs/20260315-143022-abc4d/ \
  --verify
```

In replay mode, LLM calls, HTTP requests, and all other effects return their recorded responses instead of hitting real services. The `--verify` flag checks that the replayed execution produces the same output hashes.

## Compliance relevance

Evidence bundles address several audit requirements directly:

- **What ran**: Workflow definition hash is recorded
- **What model was called**: LLM capability calls logged with model identifier
- **What the model returned**: CapResult entries preserve full responses
- **Who approved it**: Approval gate decisions are logged as step transitions
- **Was it tampered with**: Hash chain verification detects any modification

For regulated workflows (financial, healthcare, legal), evidence bundles can be exported and stored in a document management system alongside the artifacts they describe.

See also: [Replay](../DETERMINISM_CONTRACT.md), [Compliance](../COMPLIANCE_EVIDENCE.md)
