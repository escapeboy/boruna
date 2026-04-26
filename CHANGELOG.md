# Changelog

All notable changes to Boruna are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- **Sequential failure path persists actual `attempt_count`** (sprint
  `0.3-S13`, closes carried-forward limitation from 0.3-S11). Prior
  to this, the sequential `execute_steps` failure branch defaulted
  to `attempt_count=1` even after retry exhaustion — so a step
  configured with `max_attempts: 3` that exhausted all 3 attempts
  showed up as `attempt_count=1` in the persisted SQL row and on
  `workflow show`. The error message correctly said "failed after 3
  attempts" but the column lied. Fix: `execute_source_step` now
  returns `Result<StepResult, (WorkflowRunError, u32)>` carrying
  the count on both branches; the caller threads it through to the
  Failed checkpoint upsert. Concurrent path was already correct.

### Added

- **`workflow show` surfaces `attempt_count`** (sprint `0.3-S12`).
  Plain mode adds an `ATTEMPTS` column to the steps table; `--json`
  mode adds `attempt_count` to each step's object. Closes the
  operator-visibility loop opened by 0.3-S11 — operators triaging
  flaky steps no longer need to query SQLite directly.

- **`step_checkpoints.attempt_count` column** (sprint `0.3-S11`).
  Tracks the number of attempts each step took to reach its terminal
  state — `1` for first-try success or single-attempt failure;
  `>1` when the retry policy fired (sprint `0.3-S5`). Operational
  only — wall-clock-keyed (depends on whether transient failures
  happened); never feeds an audit hash. Surfaced on `StepResult`,
  `StepCheckpoint`, and persisted in the SQL store. **First real
  schema migration**: bumps `SCHEMA_VERSION` to `2`; existing v1
  databases are migrated additively via `ALTER TABLE ADD COLUMN`
  with `DEFAULT 1` (no rewrite, instant). The migration runner is
  idempotent — fresh databases (where the canonical creation script
  already includes the column) skip the ALTER.
- New library API:
  - `RetryPolicy`-aware `retry_with_backoff` now returns
    `Result<(T, u32), (E, u32)>` so callers can persist the actual
    attempt count alongside success or failure.
  - `compile_and_run_step_with_retry` returns `(Value, u32)` /
    `(WorkflowRunError, u32)` — same change in the runner-level
    wrapper.
  - `StepResult.attempt_count: u32` (defaults to 1 for back-compat
    on older serialized JSON).
  - `StepCheckpoint.attempt_count: u32` matches the SQL column.
  - `persistence::SCHEMA_V1_TO_V2_SQL` and
    `persistence::column_exists` helpers exposed within the crate.

### Fixed

- **`--skip-if-running` race window closed** (sprint `0.3-S10`,
  carried-forward debt from 0.3-S7). Prior implementation's two-call
  flow (`find_in_flight_runs` then `run_persistent`) let two
  concurrent processes both pass the in-flight check and both insert
  new run rows. Now folded into a single `BEGIN IMMEDIATE` SQL
  transaction via the new
  `RunCheckpointStore::insert_run_with_derived_id_skip_if_in_flight`
  method: at most one of N concurrent invocations inserts; the rest
  cleanly Skip. Locked by an 8-thread regression test that asserts
  exactly 1 Inserted + 7 Skipped outcomes. New library API:
  `WorkflowRunner::run_persistent_or_skip` returning
  `Option<WorkflowRunResult>` (Some = ran, None = skipped). The CLI
  flow now uses this atomic path under `--skip-if-running`.

### Added

- **`--expect-workflow-hash <HEX>`** on `boruna workflow run` and
  `boruna workflow resume` (sprint `0.3-S9`). CI/CD safety primitive
  that refuses to start (or resume) if the on-disk workflow def's
  `workflow_hash` doesn't match the operator-supplied expected
  value. Catches accidental edits, malicious mutation, and stale-
  checkout-vs-config drift before any side effect.
- **`--print-hash`** on `boruna workflow validate`. After validation
  succeeds, emits `workflow_hash=<64-char hex>` on its own stdout
  line — cut-friendly for shell pipelines:
  ```
  HASH=$(boruna workflow validate ./wf --print-hash | grep ^workflow_hash | cut -d= -f2)
  boruna workflow run ./wf --expect-workflow-hash $HASH ...
  ```
  Hash comparison is case-insensitive + whitespace-trim-tolerant so
  operators can paste from any source.
- **Note:** the hash covers the `workflow.json` structure only —
  `.ax` step source changes do NOT affect the hash. For full-source
  coverage operators should hash the workflow_dir tree at the
  filesystem layer.

### Decided

- **LLM live handler model: Bring Your Own Handler (BYOH)** (sprint
  `0.3-S8`). Boruna does NOT ship a default LLM handler in core.
  Integrators implement the `CapabilityHandler` trait against their
  provider of choice (OpenAI, Anthropic, vLLM, Ollama, custom
  routers) and pass it to `CapabilityGateway::with_handler` at
  workflow run time. Rationale: provider churn shouldn't destabilize
  Boruna releases; API-key management belongs in the integrator's
  application; production integrators (FleetQ et al.) already have
  their own LLM clients. New guide:
  [`docs/guides/llm-integration.md`](docs/guides/llm-integration.md)
  covers the contract, provider variants, determinism notes, and
  testing patterns. Reference handler at
  [`examples/llm_handlers/openai/`](examples/llm_handlers/openai/).
  Closes the open question carried since the original 0.3.0 plan;
  `docs/roadmap.md` and `docs/limitations.md` updated accordingly.

### Added

- **`boruna workflow run --skip-if-running`** (sprint `0.3-S7`).
  Idempotent invocation primitive for cron-driven scheduled
  workflows. Before launching a new run, queries the persistent
  store for any in-flight (`Running` or `Paused`) run of the same
  workflow. If found, exits 0 cleanly with a stderr message
  identifying the prior run. Designed for the cron pattern:
  ```
  0 2 * * * boruna workflow run /path/to/wf \
            --skip-if-running --data-dir /var/lib/boruna
  ```
  Without this flag, overlapping invocations could race on the
  same `outputs/` directory and double-bill external API calls.
  Persistent path only; rejected at parse with `--ephemeral`.
- New library API: `boruna_orchestrator::workflow::find_in_flight_runs(data_dir, def)`,
  `boruna_orchestrator::persistence::RunCheckpointStore::list_in_flight_runs_for_workflow`.

### Fixed

- **Power-loss durability for `DataStore::store_output`** (sprint
  `0.3-S6`, closes H1/C3 deferral from 0.3-S3). After
  `tempfile::persist`, the parent directory is now opened and
  fsynced so the rename's directory entry is journaled to stable
  storage. Without this, POSIX permits the dirent to be lost on
  power loss even though the file's data blocks were flushed. On
  macOS uses `fcntl(F_FULLFSYNC)` for both file and directory syncs
  (review-driven 0.3-S6 finding) — plain `fsync(2)` on Darwin does
  NOT flush the drive's write cache to media, which would have
  silently undermined the durability claim on macOS deployments.
  SQLite, Postgres, and `git` all use F_FULLFSYNC for the same
  reason. Skipped on Windows (non-production target). NFS / fuse /
  network FS no longer claimed as covered — docstring downgraded
  to "use local FS for production durability claims" (review-driven
  finding: prior NFSv4 claim overstated; mount options + server
  semantics make the guarantee non-portable).

### Added

- **Retry policies with exponential backoff** (sprint `0.3-S5`).
  `RetryPolicy { max_attempts, on_transient }` on a step is now
  honored properly: the runner loops up to `max_attempts` total
  attempts with `100ms × 2^N` (capped at 5s) backoff between. Both
  sequential and concurrent execution paths share a single
  `retry_with_backoff` helper, so retry semantics don't drift between
  paths. Final-attempt failure surfaces as
  `"failed after N attempts: <reason>"` for operator triage.
- New library API: `boruna_orchestrator::workflow::retry_with_backoff`
  and `retry_backoff_ms` (pub(crate); used by tests).
- Operators see retry attempts logged to stderr (gated under
  `cfg(not(test))` so the unit suite stays silent).

### Fixed

- **Retry semantics no longer cap at "retry once."** Prior code
  (`should_retry = ... && r.max_attempts > 1`) re-attempted exactly
  once regardless of the configured `max_attempts`. Now honored as
  documented: a `max_attempts: 5` policy retries up to 4 times.
- **`retry_with_backoff`'s eprintln gated under `cfg(not(test))`**
  (review-driven 0.3-S5 finding #1). Prior unconditional eprintln
  polluted unit-test stderr and any embedder capturing process
  stderr.
- **Integration test `tests/retry_timing.rs` locks real wall-clock
  backoff** (review-driven 0.3-S5 finding #2). Unit tests skip
  sleeps under `cfg(test)`; this integration test runs in a context
  where `cfg(test)` is NOT set on the orchestrator lib build, so the
  real sleeps fire and the test asserts `elapsed >= 250ms` for a
  3-attempt retry. Catches future regressions that accidentally
  remove the sleep.

- **Concurrent step execution within a workflow run** (sprint `0.3-S4`).
  New `--concurrency <N>` flag on `boruna workflow run` and
  `boruna workflow resume`. Default `1` = sequential (preserves prior
  behavior); higher values parallelize fan-out workflows. The
  per-step `output_hash` is bit-identical across concurrency levels
  for successful runs — the determinism contract holds. Locked by a
  regression test that runs the same workflow at concurrency=1 and
  concurrency=4 and asserts every step's hash matches.
- Implementation: wave-based scheduler. `WorkflowValidator::topological_levels`
  partitions the DAG into "waves" where each level's steps have all
  dependencies in earlier levels. Within a wave, source steps fan out
  to short-lived `std::thread::spawn`'d workers (no tokio, no async
  runtime). Workers are pure compile+run paths returning a `Value`;
  the coordinator owns all DataStore + SQLite mutation.
- New library API: `RunOptions::concurrency: usize`,
  `ResumeOptions::concurrency: usize`,
  `WorkflowValidator::topological_levels`. `RunOptions::default()`
  and `ResumeOptions::default()` initialize concurrency to `1`.
- Persistent path only — `WorkflowRunner::run` (ephemeral) stays
  single-threaded. The CLI rejects `--concurrency 0` at parse.

### Fixed

- **Concurrent chunk halt no longer detaches sibling workers**
  (review-driven 0.3-S4 finding #1). Prior code used `?` inside the
  join loop, which dropped subsequent JoinHandles and detached their
  threads — those workers continued executing the workflow_dir even
  after `run_persistent` returned. Now the join loop collects all
  `JoinHandle::join()` results into a Vec before processing,
  guaranteeing no thread is left running once the function returns.
- **Pre-validate all chunk inputs before marking any Running**
  (review-driven 0.3-S4 finding #2). Prior code interleaved input
  validation with `mark_step_running_clearing_output`, so an input
  failure mid-chunk left earlier siblings Running on disk forever
  and the next resume re-executed them silently. Now a two-pass
  structure: pass 1 validates every chunk member's inputs (no side
  effects); pass 2 marks all Running atomically and dispatches.
- **Worker panics now produce attributed Failed checkpoints**
  (review-driven 0.3-S4 finding #3). Prior panic handler only
  matched `&'static str` payloads (so `panic!("step {} bad", id)`
  fell through to a generic message) and lost the step_id, leaving
  the panicked step at status=Running on disk. Now: tries `String`
  payloads first, carries the step_id alongside each JoinHandle, and
  records a Failed checkpoint with the panic message.

- **`boruna workflow show <run-id>` CLI** (sprint `0.3-S3`). Operator
  inspection of a single run's full state: row, step checkpoints with
  truncated output preview, and approval sentinels. Plain-mode tabular
  output mirrors `workflow list`'s aesthetic; `--json` emits a stable
  pipe-friendly document for `jq` consumers. Returns
  `RunNotFound` for unknown ids (project-conventions §1).
- New library API: `boruna_orchestrator::workflow::{show_run,
  RunDetail, ApprovalView}`. `RunDetail` carries a
  `metadata_parse_error: Option<String>` field so corrupt-metadata
  signals reach pipeline consumers (review-driven 0.3-S3 H5: stderr
  warnings are silently dropped when stdout is piped).

### Fixed

- **Atomic-rename in `DataStore::store_output`** (sprint `0.3-S3`,
  closes H4 deferral from 0.3-S2c). Replaces the previous
  `std::fs::write` (non-atomic) with
  `tempfile::NamedTempFile::persist`. Concurrent readers — including
  another resumed run process — see either the old contents or the
  new contents, never a partial torn write. Process-crash safe;
  full power-loss safety still requires a parent-directory fsync,
  documented honestly in the method docstring as the next hardening
  pass.
- **`output_hash` now equals `sha256sum result.json`** (review-driven
  0.3-S3 H2/H3). Previously `hash_value` used compact JSON while
  `store_output` wrote pretty-printed JSON, so an operator running
  `sha256sum runs/<id>/outputs/<step>/result.json` got a different
  hex than the persisted `output_hash` column — a UX footgun. All
  three (the hash input, the on-disk file bytes, and the
  `step_checkpoints.output_json` SQL column) are now the same compact
  serialization. Locked by a regression test that compares
  `sha256sum`-equivalent of the on-disk bytes against `hash_value`.
- **`workflow show --json` no longer panics on multi-byte UTF-8 in
  step output** (review-driven 0.3-S3 C1). Prior code did
  `&output_json[..200]` to truncate the preview field, which panicked
  if byte index 200 landed inside a multi-byte codepoint. New
  `truncate_at_char_boundary` helper snaps to the nearest char
  boundary at-or-below the byte budget. Locked by 4 regression
  tests covering pure ASCII, exact-boundary, multi-byte-at-boundary,
  and pure-multi-byte content.

- **Approval-gate completion CLI** (sprint `0.3-S2c`). Three new
  `boruna workflow` subcommands close the operator UX deferred from
  `0.3-S2b`:
  - `boruna workflow approve <run-id> <step-id> --data-dir <PATH>` —
    records an approval sentinel in the run's `metadata.approvals.<step>`.
  - `boruna workflow reject <run-id> <step-id> [--reason <STR>]
    --data-dir <PATH>` — records a rejection sentinel; the optional
    reason surfaces as the step's `error_msg` on resume.
  - `boruna workflow list [--status <STATUS>] [--json]
    --data-dir <PATH>` — lists runs ordered by `(workflow_name, run_id)`,
    optionally filtered by `running` / `paused` / `completed` / `failed`.
  After `approve`, the operator runs `boruna workflow resume <run-id>` to
  advance the gate to `Completed` (with a synthetic empty-record output
  whose hash is locked by a regression test) and execute downstream
  steps. After `reject`, resume halts the run as `Failed` with the
  recorded reason.
- **Approval sentinel mechanism on `metadata.approvals`**. The runner's
  `PersistedRunMetadata` now carries a `BTreeMap<step_id,
  ApprovalDecision>`. Each decision records `decision`
  (`approved`/`rejected`), `decided_at_ms` (operational only — does not
  feed any audit hash), and an optional human-readable `reason`.
  Backward compatible with `0.3-S2b` databases: the field defaults to
  empty if absent.
- New library API: `boruna_orchestrator::workflow::record_approval_decision`,
  `list_runs`, `ApprovalKind`, plus error variants `StepNotFound`,
  `StepNotAtApprovalGate { current_status }`, `StepAlreadyDecided
  { prior_decision }`, `NotAnApprovalGateStep`, `RunNotResumable
  { terminal_status }` (project-conventions §1).
- New `boruna_orchestrator::persistence::{get_run_metadata,
  update_run_metadata, compare_and_swap_metadata, list_runs}` methods.
  `compare_and_swap_metadata` is the atomicity primitive for the
  approve/reject flow's read-validate-write cycle.

### Fixed

- **Race in `record_approval_decision`** (review-driven, 0.3-S2c).
  Previous implementation's read+validate+write spanned three separate
  SQL transactions; two concurrent operators could both pass the
  in-memory prior-decision check and silently overwrite each other's
  decision. Now wrapped in a CAS retry loop via the new
  `compare_and_swap_metadata` primitive — exactly one writer succeeds;
  the others surface a clean `StepAlreadyDecided` error. Locked by a
  4-thread regression test asserting "exactly 1 ok, 3 already-decided."
- **Resume halt-cause attribution.** When both an independently-failed
  step (e.g. from a crashed prior run) and a rejected approval gate
  exist for the same run, the resume's `halt_with_failed_step` now
  preserves the FIRST failure (the actual root cause the operator
  should chase) rather than overwriting with the gate rejection.
- **Sentinel for non-`awaiting_approval` checkpoint** now emits an
  explicit `eprintln!` warning rather than silently no-op'ing, so
  operators see when their approval doesn't apply (e.g., pre-approval
  for a step the workflow hasn't reached, or stale sentinel for an
  already-terminal step).
- **Defense-in-depth `StepKind::ApprovalGate` re-validation in resume.**
  Synthetic empty-record output is now refused for non-gate steps even
  if a sentinel slipped past `record_approval_decision`'s validation
  (e.g. via a future code path bypass). Surfaces as
  `WorkflowRunError::Internal`.

- **Persistent workflow runs survive process restarts** (sprint `0.3-S2b`).
  Wires the SQLite-backed `RunCheckpointStore` shipped in `0.3-S2a` into
  `WorkflowRunner`. New `boruna workflow run --data-dir <PATH>` writes a
  `runs.db` and a checkpoint at every step transition. New
  `boruna workflow resume <run-id>` picks up where a crashed or paused run
  left off — already-`Completed` steps are restored from persisted output;
  `Running`-status checkpoints (mid-step crashes) are re-executed since
  the runner trusts only `Completed`. `Failed` step checkpoints in a
  non-terminal run halt the resume rather than silently advancing past
  them (review-driven regression). New `--ephemeral` flag opts out of
  persistence; `--data-dir` falls back to `$BORUNA_DATA_DIR` then
  `./.boruna/data`. Refuses to resume against a workflow whose hash has
  drifted (`error_kind: workflow_hash_mismatch`) and against a missing
  `run_id` (`run_not_found`). The `boruna workflow approve` CLI shipping
  in `0.3-S2c` will let operators advance approval gates; until then a
  paused approval-gate run resumes by re-pausing.
- **Deterministic `run_id` derivation**
  (project-conventions §16). Replaces the wall-clock-keyed
  `format!("run-{name}-{utc now}")` with
  `sha256(workflow_hash || ":" || inputs_hash || ":" || counter)[..16]`
  hex. The counter is `COUNT(*) FROM runs WHERE workflow_hash = ?` read
  inside an explicit `BEGIN IMMEDIATE` transaction (review-driven from
  the initial `unchecked_transaction` DEFERRED-default race) so concurrent
  writers either see distinct counter values or hit `BUSY` and retry.
  Locked by a multi-thread regression test that fans out 8 concurrent
  `insert_run_with_derived_id` calls and asserts all 8 produce distinct
  ids. Algorithm locked by a golden-vector test computed externally.
- **`RunRecord` and `RunOperational` view structs** on
  `RunCheckpointStore`. Replay-verified columns vs. operational metadata
  are now structurally distinct types: audit/replay code paths consume
  `RunRecord` (no `started_at`, no `updated_at`, terminal-only `Option<RunStatus>`);
  status dashboards consume `RunOperational`. Closes the H1 review finding
  from `0.3-S2a`. The original `RunRow` is retained for back-compat
  callers.
- New `WorkflowRunner` API: `run_persistent(def, options, data_dir)`,
  `resume(run_id, data_dir, options)`, and `ResumeOptions { policy,
  record, live, workflow_dir_override }`. `ResumeOptions::policy = None`
  defaults to the persisted policy from the original run (review-driven
  H2 fix; without this default the CLI's `--policy` omission silently
  collapsed to deny-all).
- New `boruna-cli` feature flag `persist-sqlite` (on by default) that
  forwards to `boruna-orchestrator/persist-sqlite`. CLI surfaces a typed
  error rather than silently downgrading when the flag is off and a
  persistent run is requested (project-conventions §1).

### Fixed

- **Reject-at-parse footgun on persistent runs without the SQLite feature.**
  Previously, `cargo build --no-default-features` produced a CLI that
  silently ran `boruna workflow run dir --data-dir /tmp/x` ephemerally,
  creating no `runs.db` and giving the operator no signal. Now the CLI
  errors with a clear "rebuild with default features, or pass `--ephemeral`"
  message.

### Added

- **Versioned capability identity** ([#3](https://github.com/escapeboy/boruna/issues/3),
  sprint `0.3-S11`). New `boruna capability list [--json]` CLI subcommand and
  `boruna_capability_list` MCP tool report a stable `capability_set_hash` over
  the binary's capability surface. Integrators use it as part of a cache key —
  `(source_hash, policy_hash, capability_set_hash, policy.schema_version)` — to
  safely memoize deterministic run results across binary upgrades. Algorithm,
  caching recipe, and per-capability versioning rules documented in
  `docs/reference/capability-identity.md`. All 10 shipped capabilities start at
  contract version `"1"`.
- New library API in `boruna-bytecode`:
  `Capability::ALL` (canonical sorted iteration), `Capability::version()`,
  `CapabilityIdentity`, `CapabilitySetReport`,
  `compute_capability_set_hash()`, `capability_set_report()`.
- **`protocol_version: 1` field on every `boruna-mcp` tool response**
  ([#6](https://github.com/escapeboy/boruna/issues/6), sprint `0.5-S4`,
  pulled forward from 0.5.0 because FleetQ blocked on it for their
  validate-on-save UX). Wire-format version of the response envelope; bumps
  only on breaking shape changes (additive changes keep the version).
  Locked by `crates/boruna-mcp/src/tools/mod.rs::TOOL_RESPONSE_PROTOCOL_VERSION`
  and a 16-case regression test asserting every tool's success and failure
  path carries it. Versioning policy and bump rules documented in
  `docs/reference/mcp-server.md` under "Stability". Pairs with
  `Policy.schema_version` shipped in 0.2.0.
- **MCP Server Tool Reference** documentation at `docs/reference/mcp-server.md` —
  wire contract for all 10 `boruna-mcp` tools: parameter names and types,
  return shapes, `error_kind` values, encoding rules, and limits. Driven by
  FleetQ implementer feedback (post-v0.2.0 follow-up): integrators previously
  had to read `crates/boruna-mcp/src/server.rs` to learn that `boruna_run`'s
  parameter is `source` (not `script`) and that there is no `input` parameter.
  Linked from `docs/README.md`.
- **Structured resource limits in `boruna_run`** ([#5](https://github.com/escapeboy/boruna/issues/5),
  sprint `0.3-S10`, FleetQ P1). New optional `limits` parameter on the MCP
  `boruna_run` tool accepting `max_wall_ms`, `max_output_bytes`, and
  `max_memory_mb`. Overruns return a typed
  `error_kind: "limit_exceeded"` with a `limit_kind` discriminator
  (`"wall_ms"` or `"output_bytes"`), the configured `limit`, and a
  human-readable `message` — so callers can surface clean per-limit UX
  instead of parsing error strings. `max_memory_mb` is accepted in the
  schema but **not enforced** in 0.3.x (documented as platform-best-effort
  pending Linux `setrlimit` work in a future sprint).
- New `boruna-vm::error::VmError::WallTimeExceeded(u64)` variant and
  `Vm::set_max_wall_ms(Option<u64>)` setter. Wall-clock checked every 1024
  steps inside the execute loop; uses `std::time::Instant` (not
  `chrono::Utc::now()` per ADR 001 determinism contract). Wall-time
  enforcement is wall-clock-keyed and therefore non-deterministic on
  overrun by construction — `max_steps` remains the deterministic
  ceiling; `max_wall_ms` is the operational guardrail.
- **Output JSON Schema validation gate in `boruna_run`**
  ([#8](https://github.com/escapeboy/boruna/issues/8), sprint `0.5-S6`,
  pulled forward from 0.5.0 because FleetQ wanted it in their pipeline).
  New optional `output_schema` parameter on the MCP `boruna_run` tool
  accepting any JSON Schema 2020-12 object. The script's `result` is
  validated post-execution; mismatches return
  `error_kind: "validation_failed", phase: "output_validation"` with
  per-path JSON Pointer errors. Malformed or oversized schemas (>256 KB)
  return `error_kind: "invalid_output_schema"`. Schemas declaring a
  non-2020-12 `$schema` are rejected (same "reject at parse, don't
  silently override" pattern as `0.3-S10`'s `unsupported_limit`). Error
  array capped at 100 entries with `truncated` and `total_errors`
  fields. **Known limitation:** records/enums emit as wrapper objects;
  schemas for the natural shape will fail. Best for primitive returns.
  See `docs/design-output-schema.md`.
- New `jsonschema = "0.30"` dependency in `boruna-mcp` (default features
  off — no `resolve-http` or `resolve-file`, so `$ref` to remote URLs
  cannot trigger SSRF or arbitrary file reads).
- **Record/replay for `net.fetch`** ([#7](https://github.com/escapeboy/boruna/issues/7),
  sprint `0.5-S7`, pulled forward from 0.5.0). Boruna scripts are
  deterministic by design; external HTTP is not. New CLI flags on
  `boruna run`:
  - `--record-net-to <FILE>` (requires `--live`) makes real HTTP calls and
    persists each `(method, url, request_body) → response_body`
    transaction to a sidecar JSON tape file.
  - `--replay-net-from <FILE>` serves responses from a loaded tape with
    no real network access. Strict ordered match on
    `(method, url, request_body)`; mismatch returns a typed error
    naming the position and differing field; tape exhaustion returns a
    typed error; under-consumption is silently OK.
  - Mutually exclusive (clap `conflicts_with`). If `--live` is set
    alongside `--replay-net-from`, replay wins (no real calls happen).
- New module `boruna_vm::net_record_replay` (feature-gated under
  `http`) exposing `NetTransaction`, `NetTape`, `RecordingHttpHandler`,
  `ReplayingHttpHandler`, and `TAPE_FORMAT_VERSION`.
- `RecordingHttpHandler::with_save_path()` arms save-on-drop; the CLI
  also probes write access on the tape path **before** the run starts
  so a CI pipeline like `record-net-to fixtures/x.tape && verify x.tape`
  fails fast on disk errors instead of silently producing a stale
  fixture (review-driven hardening).
- New shared parser `boruna_vm::http_handler::parse_net_fetch_args()`
  used by both the real handler and the recording layer so they can't
  silently drift in arg interpretation.
- Documentation: `docs/design-net-record-replay.md` (tape format, match
  strategy, CLI surface, known limitations).
- **Per-call OpenTelemetry observability** ([#9](https://github.com/escapeboy/boruna/issues/9),
  sprint `0.4-S5`, the LAST FleetQ ask). Always-on `tracing` instrumentation
  in `CapabilityGateway::call` emits `boruna.cap` spans with attributes
  `cap.name`, `bytes_in`, `bytes_out`, `cap.budget_remaining`, `error.kind`
  (set on the failure path: `denied` / `budget_exceeded` / `runtime_error`).
  When no subscriber is installed (the default), span macros are essentially
  no-ops — zero runtime cost.
- **`telemetry` Cargo feature** on `boruna-vm` (and mirror feature on
  `boruna-cli`) adds an OpenTelemetry OTLP-over-HTTP exporter
  (`opentelemetry 0.27` + `opentelemetry-otlp 0.27` + `tracing-opentelemetry
  0.28`). New helper `boruna_vm::init_telemetry()` reads
  `OTEL_EXPORTER_OTLP_ENDPOINT` (and optional `OTEL_SERVICE_NAME`,
  defaulting to `"boruna"`); returns a `Disabled` no-op handle when the
  endpoint is unset (Boruna behaves identically to a non-telemetry build),
  installs the exporter when set. Returns a `TelemetryHandle` whose `Drop`
  flushes pending spans.
- **CLI integration:** `boruna-cli` built with `--features telemetry` starts
  a tokio runtime in `main`, calls `init_telemetry()` BEFORE parsing CLI
  args, holds the handle for the binary lifetime, and on shutdown drops
  the handle THEN drains the runtime with a 5-second timeout (so
  in-flight OTel HTTP POSTs complete instead of being killed by
  `process::exit`).
- New documentation: `docs/design-otel.md` (span shape, attribute table,
  determinism contract, library-version pin set, BYO-subscriber fallback
  path).
- **`boruna_orchestrator::persistence::RunCheckpointStore`** — SQLite-backed
  workflow checkpoint store (sprint `0.3-S2a`). Implements ADR 001 step
  1–5: schema, Connection setup with mandatory PRAGMAs (`journal_mode=WAL`,
  `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`), CRUD
  operations (`insert_run`, `update_run_status`, `get_run`,
  `list_runs_by_status`, `upsert_step_checkpoint`,
  `list_step_checkpoints`), and a `BEGIN IMMEDIATE` retry policy that
  handles both `SQLITE_BUSY` and `SQLITE_LOCKED` with exponential
  backoff (10ms→50ms→250ms→1.25s) before failing with
  `PersistenceError::Busy`. **Not yet wired into `WorkflowRunner` —
  that integration lands in `0.3-S2b`** (along with `boruna workflow
  resume <run-id>` and `--data-dir`).
- New `persist-sqlite` Cargo feature on `boruna-orchestrator` (default-on).
  Adds `rusqlite = "0.32"` with the `bundled` feature so SQLite compiles
  from C source — preserves the musl-static-binary story per ADR 001.
- Schema embedded via `include_str!("schema_v1.sql")`. Single-row
  `schema_version` table with `CHECK (id = 1)` constraint structurally
  prevents stale-row accumulation across migration attempts.
- `PersistenceError::NotFound { entity, key }` returned by
  `update_run_status` when the target `run_id` does not exist (review-
  driven; silent-no-op was rejected as a footgun for the resume path).
- `upsert_step_checkpoint` uses `COALESCE(excluded.X, existing.X)` for
  `started_at`, `output_json`, `output_hash` so a partial upsert (e.g.
  step transition from Running to Completed without re-supplying
  started_at) preserves the original value rather than clobbering to
  NULL (review-driven; locked by two regression tests).
- `docs/design-persistence-store.md` — sprint scope split rationale,
  acceptance criteria, schema annotation conventions.

### Decided

- **ADR 001 — Persistence Backend** (`docs/adr/001-persistence-backend.md`).
  SQLite via `rusqlite/bundled` chosen as the workflow-checkpoint backend.
  No persistence-trait abstraction in v1 — direct concrete dependency.
  Includes a determinism contract for persisted state (operational vs.
  replay-verified columns), the writer serialization model, mandatory
  connection PRAGMAs (`journal_mode=WAL`, `foreign_keys=ON`,
  `busy_timeout=5000`), and an illustrative schema. Unblocks `0.3-S2`
  through `0.3-S9` — the entire 0.3.0 critical path. Sprint `0.3-S1`.
## [0.2.0] - 2026-04-25

Driven by [implementer feedback from FleetQ](https://github.com/escapeboy/boruna/issues?q=label%3Aenhancement) (production integrator). This release closes the two P0 adoption blockers; remaining P1/P2 asks are tracked as issues #3–#9.

### Added

- MCP `boruna_run` tool now accepts a structured `Policy` object for the `policy`
  parameter, in addition to the existing `"allow-all"` / `"deny-all"` string
  shorthands. This exposes the per-capability rules (`allow`, `budget`),
  `default_allow` mode (allowlist vs. denylist), and `net_policy` (allowed
  domains, methods, byte limits, timeout) that the VM has always supported.
  See `docs/reference/policy-schema.md` and `docs/reference/policy.schema.json`.
- New documentation: `docs/reference/policy-schema.md` (prose + examples) and
  `docs/reference/policy.schema.json` (machine-readable JSON Schema 2020-12)
  for integrators rendering capability matrices in their own UIs.
- The `boruna_run` MCP tool description now advertises the structured-policy
  capability so AI agents discover it from the tool list directly.
- Multi-target release workflow (`.github/workflows/release.yml`) that publishes
  static binaries on every `v*` tag for `x86_64-unknown-linux-musl`,
  `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`,
  plus a combined `SHA256SUMS` checksum file. Linux builds use musl so the
  binaries run on Alpine and other libc-minimal distributions.
- `docs/releasing.md` — release process, verification, and rationale for using
  GitHub-hosted runners (vs. the self-hosted runner used by `ci.yml`).
- README install section showing curl-and-verify install.

### Changed

- **Breaking (MCP only):** `boruna_run` now rejects unknown `policy` values
  (e.g. typo'd strings, numbers, arrays) with `success: false,
  error_kind: "invalid_policy"` instead of silently treating them as
  `"allow-all"`. The legacy strings `"allow-all"` and `"deny-all"` continue
  to behave identically.

## [0.1.0] - 2026-02-21

### Added

- Deterministic workflow execution engine with DAG validation and topological ordering
- Hash-chained audit logs (SHA-256) and self-contained evidence bundles for compliance
- Policy-gated capability system — 10 capabilities: `net.fetch`, `db.query`, `fs.read`,
  `fs.write`, `time.now`, `random`, `ui.render`, `llm.call`, `actor.spawn`, `actor.send`
- Replay engine for determinism verification via `EventLog` comparison
- Three reference workflow examples:
  - `llm_code_review` — linear 3-step pipeline demonstrating LLM capability and evidence recording
  - `document_processing` — fan-out/merge 5-step pipeline demonstrating parallel steps and DAG scheduling
  - `customer_support_triage` — approval-gate 4-step pipeline demonstrating human-in-the-loop and conditional pause
- MCP server (`boruna-mcp`) exposing 10 tools over JSON-RPC stdio for AI coding agent integration
- Actor system with `OneForOne` supervision and bounded execution scheduling (`Vm::execute_bounded`)
- `boruna-tooling`: diagnostics with source spans, auto-repair, trace-to-tests, stdlib test runner, 5 app templates
- `boruna-pkg`: deterministic package system with SHA-256 content hashing, dependency resolution, and lockfiles
- Real HTTP handler (feature-gated via `boruna-vm/http`) with SSRF protection for `net.fetch` capability
- CLI binary (`boruna`) with subcommands: `compile`, `run`, `trace`, `replay`, `inspect`, `ast`,
  `workflow`, `evidence`, `framework`, `lang`, `trace2tests`, `template`
- Standard library: 11 deterministic libraries — `std-ui`, `std-forms`, `std-authz`, `std-http`,
  `std-db`, `std-sync`, `std-validation`, `std-routing`, `std-storage`, `std-notifications`, `std-testing`
- 557+ tests across 9 crates

[Unreleased]: https://github.com/escapeboy/boruna/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/escapeboy/boruna/releases/tag/v0.2.0
[0.1.0]: https://github.com/escapeboy/boruna/releases/tag/v0.1.0
