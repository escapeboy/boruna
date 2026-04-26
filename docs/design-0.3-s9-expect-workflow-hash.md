# Design — 0.3-S9: `--expect-workflow-hash`

**Status:** 2026-04-26
**Theme:** Roadmap — "Workflow versioning."

## Scope

A pre-flight safety check on `boruna workflow run` and `boruna workflow resume` that refuses to proceed if the on-disk workflow def's `workflow_hash` doesn't match the operator-supplied expected value. CI/CD pattern: capture the hash at deploy time, pass it on every invocation in production. Catches accidental edits, malicious mutation, and stale-checkout-vs-config drift before a single step runs.

**In scope:**

1. New `--expect-workflow-hash <HEX>` flag on `boruna workflow run` and `boruna workflow resume`. When provided, the CLI computes `workflow_hash_from_def` over the on-disk def and compares.
2. On mismatch: print `error: workflow_hash mismatch: expected=<E>, actual=<A>` to stderr, exit non-zero. No new run row inserted, no resume side effects.
3. New `--print-hash` flag on `boruna workflow validate` that emits `workflow_hash=<hex>` on stdout (jq/grep-friendly). Operators capture this in their deployment pipeline.
4. Behaves the same on persistent and ephemeral paths (the check happens before either path forks).

**Out of scope:**

- Workflow registry / version-pinned URLs (`run @v1.2.3`). Bigger sprint.
- Persisting `workflow_version` (the human-readable string from `WorkflowDef.version`) on run rows. Cheap to add later; not needed for the safety check.
- Hash check on `--ephemeral` paths skipping the existing resume-time mismatch check. The new flag covers BOTH paths since both load the def from disk.

## Forcing questions (Think)

**Who needs this?** Anyone running Boruna in CI/CD or production where workflow defs are part of the deployable surface. Today: an unintended edit to `workflow.json` (or its referenced `.ax` step files) silently changes the workflow_hash; the operator has no pre-flight signal.

**Narrowest MVP?** A single CI invocation: `boruna workflow run /path/to/wf --expect-workflow-hash $WORKFLOW_HASH --policy allow-all --data-dir /var/lib/boruna --skip-if-running`. If anyone modified the workflow between deploy and run, the run refuses cleanly.

**Whoa moment?** A junior engineer accidentally edits `steps/classify.ax` in production; the next cron tick refuses cleanly with a clear hash-mismatch message instead of silently producing different outputs.

**Compounds?** Locks the `workflow_hash` as a first-class deployment artifact. Future sprints can build a workflow registry / version-pinned URLs on this primitive.

## Implementation

### Library

The hash function `WorkflowRunner::workflow_hash_from_def` already exists (sprint `0.3-S2b`). No new library work needed. The check happens at the CLI layer.

### CLI

```rust
WorkflowCommand::Run {
    ...
    /// Refuse to run if the on-disk def's workflow_hash doesn't
    /// match this value. CI/CD safety check — capture the expected
    /// hash via `boruna workflow validate <dir> --print-hash` at
    /// deploy time, pass it on every invocation.
    #[arg(long)]
    expect_workflow_hash: Option<String>,
}

WorkflowCommand::Resume {
    ...
    #[arg(long)]
    expect_workflow_hash: Option<String>,
}

WorkflowCommand::Validate {
    dir: PathBuf,
    /// Emit `workflow_hash=<hex>` on stdout for capturing into
    /// `$WORKFLOW_HASH` in CI pipelines. Pair with
    /// `boruna workflow run --expect-workflow-hash`.
    #[arg(long)]
    print_hash: bool,
}
```

### Behavior

`workflow run` / `workflow resume` — when `--expect-workflow-hash` is provided:
- After parsing the def, compute `workflow_hash_from_def(&def)`.
- Compare to the supplied hex. Case-insensitive (operators may pipe through `tr`).
- On mismatch: print error, exit 1. No store interaction, no run row, no side effects.
- On match: continue normally.

`workflow validate --print-hash` — after validation succeeds, emit:
```
workflow_hash=<64-char hex>
```
(Plain `key=value` format, jq-compatible if needed via `--json` later but keeping it simple for shell piping.)

### Hash format

Lowercase hex, 64 characters (SHA-256). The same string `workflow_hash_from_def` returns. Comparison is case-insensitive so operators can paste hashes from any source.

## Determinism

`workflow_hash_from_def` is already deterministic (canonical-JSON serialization + SHA-256). No change.

## Risks

- **`.ax` source changes don't affect the hash.** `workflow_hash_from_def` hashes the `WorkflowDef` (the JSON), which references step source paths but doesn't include their content. So an attacker who edits `steps/classify.ax` (without touching `workflow.json`) bypasses this check. Document this honestly in the help text.
- For full source-content protection, operators would need to hash the entire workflow_dir tree — that's a separate feature (`workflow seal` or similar).

## Acceptance criteria

- `cargo test --workspace` green including:
  - `cli_expect_workflow_hash_match_proceeds` (behaves normally)
  - `cli_expect_workflow_hash_mismatch_refuses` (exits with typed error)
  - `validate_print_hash_emits_known_format`
- `cargo clippy -D warnings` clean.
- `cargo fmt --check` clean.
- Manual demo:
  1. `boruna workflow validate examples/workflows/llm_code_review --print-hash` → captures hash.
  2. `boruna workflow run examples/workflows/llm_code_review --policy allow-all --data-dir /tmp/d --expect-workflow-hash <captured>` → completes.
  3. Edit `examples/workflows/llm_code_review/workflow.json` (e.g., change name).
  4. Same `run` invocation → refuses with mismatch error, exit 1.
