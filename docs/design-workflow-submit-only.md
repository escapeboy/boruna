# Design — Workflow `--submit-only` flag (sprint 0.5-S2e)

## Premise

Sprints 0.5-S1 through 0.5-S2d shipped the entire distributed
execution stack: ADR + persistence state machine + HTTP
coordinator + worker + background sweep + dashboard merge. The
last gap is wiring the existing `boruna workflow run` so
operators can dispatch real workflows through the coordinator
+ workers cluster.

Full integration (`--coordinator <url>` mode with client-side
wave loop, polling, retry, multi-pause) is genuinely a big
refactor of `WorkflowRunner::run_persistent` (~1200 lines).
This sprint ships the **smallest possible step**: a
`--submit-only` flag that runs the existing `prepare_persistent_run`
path (validate, insert run row, embed step sources in
metadata, insert initial-wave Pending checkpoints) and then
**exits before spawning thread workers**. The coordinator's
existing claim/dispatch flow handles the rest.

For multi-wave workflows the operator gets the FIRST wave
executed (via remote workers) but no automatic advancement —
0.5-S2f or later adds the wave loop. For single-wave
workflows the integration is end-to-end.

## Who needs this

- **Operators running coord + workers in production.** Today
  there's no way to feed real workflows into the cluster.
  `--submit-only` closes the most basic path: "I have a
  workflow.json, please run it on my workers."
- **The 0.5-S2f implementer** (next sprint). Once the
  metadata format includes `step_sources` and the
  early-return path is in place, adding the wave loop is
  pure plumbing on top.

## Narrowest MVP

```sh
boruna coordinator serve --data-dir /var/lib/boruna &
boruna worker run --coordinator http://127.0.0.1:8090 &
boruna workflow run examples/workflows/llm_code_review \
    --submit-only --data-dir /var/lib/boruna
```

The `workflow run --submit-only` command:
1. Validates + computes DAG.
2. Reads each source step's `.ax` file and embeds them in
   `metadata_json.step_sources` (the field the coordinator
   already reads since 0.5-S2b).
3. Inserts the run row + initial wave's source-step Pending
   checkpoints.
4. Prints `submitted run_id=<id>` and exits 0.
5. The cluster (coord + workers) picks up the Pending steps
   via existing mechanisms.

Operators monitor via `boruna workflow show <run-id>` or the
dashboard at the coordinator's port.

## What would make someone say "whoa"

- **Deterministic `run_id`.** The existing
  `insert_run_with_derived_id` produces `sha256(workflow_hash
  + inputs + counter)` — the same operator submitting the same
  workflow gets a different `run_id` only because the counter
  increments. This is unchanged.
- **Step sources travel with the run.** Workers on remote
  hosts don't need filesystem access to `workflow_dir`; the
  source is served by the coordinator from
  `metadata_json.step_sources`.
- **The existing `boruna workflow show` and the dashboard
  pick up the new field automatically.** They render
  whatever's in `metadata_json` opaquely.

## How this compounds

- 0.5-S2f's wave loop just needs to: poll runs.db for step
  terminal states, write Pending checkpoints for downstream-
  ready successors, repeat. The submit-only path proves the
  initial wave works; the wave loop iterates.
- Once submit-only is stable, future modes (`--wait`,
  `--coordinator <url>`, `--follow`) layer on cleanly.
- Existing audit / lifecycle integration just keeps working
  — submit-only emits a `WorkflowStarted` audit event the
  same way `run_persistent` does today.

## Scope (what this sprint changes)

- New: `RunOptions::submit_only: bool` field.
- New: `PersistedRunMetadata::step_sources: BTreeMap<String,
  String>` field (with `#[serde(default)]` for back-compat).
- New: `boruna workflow run --submit-only` CLI flag.
- Wire-up: `prepare_persistent_run` reads source files from
  `workflow_dir` for each source-kind step and embeds them in
  `metadata_json.step_sources` BEFORE the run insert.
- Wire-up: `run_persistent` checks `options.submit_only` AFTER
  `prepare_persistent_run`; if true, writes initial-wave
  Pending checkpoints + returns a partial `WorkflowRunResult`
  with status `Submitted` (a new variant, OR we reuse
  `Running` and document that submit-only's result is
  semantically "in flight"). Reusing `Running` is simpler.
- Test: 1 CLI integration test that submits a 1-step workflow
  to a spawned coord+worker pair and asserts the step
  completes.

## Non-goals (deferred)

- **No client-side wave loop.** Multi-wave workflows submit
  their first wave; subsequent waves require manual
  intervention until 0.5-S2f.
- **No `--wait` / `--follow` mode.** Operator polls via
  `boruna workflow show <run-id>` or the dashboard.
- **No approval gates / external triggers in submit-only.**
  Workflows that use those return a typed error.
- **No retry policy in submit-only.** The first attempt is
  the only attempt; a failed step stays Failed.
- **No source-file streaming for huge workflows.** All step
  sources are read into memory and embedded in metadata.
  8 MiB combined is the soft limit per ADR 002's
  `coord.output_too_large` precedent; not enforced this sprint.

## Stable contract

- `--submit-only` flag name and CLI surface.
- `metadata_json.step_sources` field name (was already used
  in 0.5-S2b's coordinator code path; this sprint formalizes
  it as a real field on `PersistedRunMetadata`).
- The early-return semantics: submit-only returns BEFORE any
  step executes; the run shows `Running` status with steps
  in `Pending`.

## Stability tier

Per `docs/stability.md`: **experimental**. The `submit_only`
field on `RunOptions` and `step_sources` on
`PersistedRunMetadata` are additive (`#[serde(default)]`).

## Test plan

| # | Test | Expectation |
|---|---|---|
| 1 | `submit_only_inserts_run_and_initial_pending_checkpoints` (unit) | runs.db has the run row + step1 status=Pending after submit-only run |
| 2 | `submit_only_embeds_step_sources_in_metadata` (unit) | `metadata_json.step_sources` contains the source for step1 |
| 3 | `cli_workflow_run_submit_only_then_worker_completes` (integration) | spawn coord+worker, run `boruna workflow run --submit-only` against a 1-step workflow, assert step transitions to Completed via the worker |

## Adversarial review focus areas

1. **Audit lifecycle event** — does `prepare_persistent_run`
   emit `WorkflowStarted` BEFORE the early return? Or only
   inside `execute_after_insert`?
2. **Approval gates / external triggers in submit-only** —
   should we reject these workflows at the CLI boundary, or
   silently ship them as-is? If the cluster doesn't yet handle
   them in distributed mode, what happens?
3. **`step_sources` size cap** — embedding all source bodies
   in `metadata_json` means `metadata_json` grows. Is there an
   existing cap?
4. **Back-compat** — old binaries reading new metadata see
   `step_sources` as an unknown field; with
   `#[serde(default)]` they ignore it. Verify.
5. **Deterministic `run_id` invariance** — `step_sources`
   should NOT feed the run_id derivation (it's
   workflow-content-derived already via `workflow_hash`).
   Confirm.
