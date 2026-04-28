# Changelog

All notable changes to Boruna are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **CHANGELOG-driven GitHub Release notes** (sprint `W9-B`).
  The release pipeline now extracts the CHANGELOG section for
  the current tag and uses it as the GitHub Release body
  instead of auto-generating from commits. Operators MUST
  update `CHANGELOG.md` before tagging — empty section fails
  the release loudly. Improves release-page readability for
  integrators.

## [1.0.0-rc2] - 2026-04-28

**Theme: GA polish.** rc1 shipped the post-v0.5 sprint cycle
(W1-W6: versioned spec freezes, coord HA, mTLS, capability
tagging, blob GC, scaffold, perf baselines, LTS commitment,
migration tooling, bundle encryption). rc2 closes the security
review remainder before the v1.0 GA tag: explicit GA decision on
TLS 1.2 (kept, with rationale); spec amendment documenting the
optional `encryption` block in evidence bundles; canonical
`error_kind` taxonomy reference; mTLS guide updated with
revocation + non-ASCII CN limitations; `evidence inspect`
plaintext-leak gate test; algorithm-gate enforcement in
`Envelope::unwrap` matching the spec's reader contract;
stdlib version policy clarified.

### Added

- **Canonical `error_kind` taxonomy reference** at
  [`docs/reference/error-kinds.md`](./docs/reference/error-kinds.md)
  (sprint `W7`). All stable `error_kind` strings enumerated with
  HTTP status, sprint origin, and caller-facing meaning per LTS §B.6.
  Cross-linked from the policy-schema reference and the evidence
  bundle spec; integrators may switch on these strings.
- **Evidence bundle encryption block documented in spec** (sprint
  `W7`, finding `M-1`). [`docs/spec/evidence-bundle-1.0.md`](./docs/spec/evidence-bundle-1.0.md)
  now formally describes the optional `encryption` field added in
  `W6-B`: field shape, AES-256-GCM algorithm pin, per-file nonce
  derivation, replay-verified vs. operational classification, and the
  reader contract (1.x readers WITH KEK, 1.0 readers without). Additive
  to `format_version: 1.0`; no version bump.
- **mTLS limitations documented** (sprint `W7`, findings `M-4` /
  `M-5`). [`docs/guides/coord-mtls.md`](./docs/guides/coord-mtls.md)
  gains a "Limitations" section calling out the absence of CRL/OCSP
  revocation and recommending short-lived (≤24h) certs as the v1
  mitigation, plus a "CN comparison semantics" subsection documenting
  the ASCII-only `eq_ignore_ascii_case` fold (non-ASCII CNs are
  case-sensitive; no Unicode normalization).
- **Mutual TLS auth + per-worker client certificates** (sprint
  `W6-A`). Operators can now require X.509 client certs on the
  coord HTTP surface via `--tls-cert`, `--tls-key`, and
  `--tls-client-ca`. Workers present client certs via
  `--tls-cert` / `--tls-key` / `--tls-server-ca`. The cert
  subject CN drives worker identity; mismatch with a body
  `worker_id` returns `coord.identity_mismatch`. mTLS is
  additive: shared-secret bearer auth (sprint 0.5-S3)
  continues to work unchanged. Operator guide:
  [`docs/guides/coord-mtls.md`](./docs/guides/coord-mtls.md).
  New error_kind: `coord.identity_mismatch`.
- **Evidence bundle encryption** (sprint `W6-B`). Operators can
  now opt into AES-256-GCM envelope encryption for evidence
  bundles via `boruna workflow run --record --encrypt-bundle`
  with the KEK supplied via `--bundle-encryption-key <hex>` or
  `BORUNA_BUNDLE_KEK` env. Per-bundle data keys (DEK) are
  wrapped with the KEK; `bundle.json` carries the wrapped DEK
  and algorithm metadata. `verify_bundle` auto-detects
  encryption and decrypts before integrity check. Backwards-
  compat: unencrypted bundles continue to work. KEK lifecycle
  is the operator's responsibility — Boruna does not manage
  keys. New error_kinds: `evidence.encryption_key_required`,
  `evidence.encryption_key_mismatch`, `evidence.cipher_tag_invalid`.
  Threat model: `docs/design-bundle-encryption.md`.

### Decided

- **TLS 1.2 remains enabled in W6-A mTLS** (sprint `W7`). Decision:
  the default rustls 0.23 + `aws_lc_rs` configuration restricts TLS
  1.2 to AEAD-only ciphers (no CBC, no RC4, no export-grade), which
  is considered safe for 1.0 GA. Operators wanting TLS-1.3-only can
  build with a forked `rustls` feature set; the default ships TLS 1.2
  for compatibility with older HTTP load balancers and worker hosts.
  Rationale: the cryptographic surface (AEAD ciphers, ECDHE key
  exchange) is the same as TLS 1.3 for the practical attack model;
  forcing TLS-1.3-only would block deployment on systems with older
  client/proxy stacks. Re-evaluate at 2.0 if TLS 1.3 adoption is
  universal.

## [1.0.0-rc1] - 2026-04-28

**Theme: 1.0 release candidate.** This is the first 1.0
release candidate. Surfaces listed in
[`docs/lts.md`](./docs/lts.md) section B are now LTS-protected
under the long-term-support contract that takes effect at 1.0
GA. Three formal versioned specifications are published and
frozen at 1.0: the `.ax` language, the workflow DAG schema,
and the evidence bundle format. The distributed-execution
stack from v0.5.0 ships HA-ready (multi-coord active-active
behind a load balancer or via worker URL failover). Workers
can advertise capability subsets so heterogeneous fleets are
supported. Operators get the `boruna new` interactive
scaffold, `boruna migrate` for upgrading legacy artifacts,
and `boruna evidence gc-blobs` for blob storage cleanup.
Performance baselines are published with 1.x budget
commitments.

### Added

- **`boruna migrate` subcommand (beta)** (sprint `W5-C`).
  Migrators for evidence bundles (synthesize missing
  `bundle.json` for legacy v0.5.0-and-earlier bundles) and
  workflow.json (add `schema_version: 1` when missing).
  `--dry-run` previews; `--in-place` modifies the input

- **`boruna migrate` subcommand (beta)** (sprint `W5-C`).
  Migrators for evidence bundles (synthesize missing
  `bundle.json` for legacy v0.5.0-and-earlier bundles) and
  workflow.json (add `schema_version: 1` when missing).
  `--dry-run` previews; `--in-place` modifies the input
  directly; default writes a `.migrated` sibling. Beta status:
  the migrator coverage will expand in 1.x as breaking
  changes accumulate. Operator guide:
  [`docs/guides/migration.md`](./docs/guides/migration.md).
- **Performance benchmarks baseline** (sprint `W5-A`). New
  `benches/` workspace member with `criterion`-based benchmarks
  for compile time, VM throughput, and evidence bundle
  write/verify. Documented baseline + 1.x performance budget
  commitments at [`docs/PERFORMANCE.md`](./docs/PERFORMANCE.md).
  Benches are not gated in CI; run locally via
  `cargo bench -p boruna-benches`.
- **Long-term-support contract for 1.x** (sprint `W5-B`). New
  [`docs/lts.md`](docs/lts.md) documents the support windows (1.x active
  for 18 months from 1.0 GA, security-supported for 24 months; 0.x EOL on
  1.0 GA), the LTS-protected surface (.ax `language_version: "1.x"`,
  workflow DAG schema, evidence bundle format, MCP `protocol_version: 1`
  responses, CLI commands and flags, `error_kind` strings, HTTP API
  wire format), the deprecation policy (announce in 1.y → runtime warning
  → 6-month notice → migration tooling) for breaking changes in 2.x, the
  security-fix backport policy (CVSS v4, CRITICAL/HIGH within 7 days),
  and the 12-month end-of-life procedure. `docs/stability.md` cross-links
  to the LTS contract and clarifies which tiers are LTS-protected. The
  README gains an LTS line near the badges. `SECURITY.md` gains a
  backport-policy section. Doc-only sprint, no code changes.
- **Worker capability tagging** (sprint `W3-A`). Workers may
  advertise a SUBSET of the coord's capability set via
  `--advertise-caps net.fetch,db.query`; coord routes only
  steps whose policy-required capabilities are a subset of
  the worker's advertised set. Backwards-compatible:
  workers omitting the flag behave as before (full fleet).
  New `error_kind: "coord.unknown_capability"` rejects
  registration with unknown capability names. Operational
  metadata only — placement filter, not a security gate;
  the VM's capability gateway remains the authority.
- **Blob GC** (sprint `W3-B`). New `boruna evidence gc-blobs`
  command sweeps orphan content-addressed blobs from the
  data-dir's `blobs/` tree (output blobs no longer referenced
  by any step checkpoint). `--dry-run` reports without
  deleting; `--json` emits a structured report. Closes the
  0.5-S7 accepted limitation around manual blob cleanup.
  Library APIs `BlobStore::find_orphans`, `BlobStore::delete`,
  and `RunCheckpointStore::all_referenced_blob_hashes` are
  also exposed for future coord-side periodic-sweep wiring.
- **`boruna new` interactive scaffold** (sprint `W3-C`).
  Wraps the existing template engine with stdin-driven
  prompting. Walks the user through template selection,
  target dir, and per-template variables; confirms before
  writing. `--no-input` mode is CI-safe (errors on missing
  defaults rather than silently filling). Refuses to
  overwrite non-empty target dirs without `--force`.
- **Coordinator HA / failover** (sprint `W2`). Multiple
  `boruna coordinator serve` processes can run against the
  same SQLite data-dir for active-active HA. Workers accept
  comma-separated URLs in `--coordinator` and try them in
  order at registration time, sticking to the first reachable
  one. New `GET /api/health` endpoint returns
  `{status, boruna_version, capability_set_hash, uptime_ms}`
  and bypasses bearer auth so external load balancers can
  probe without holding the secret. Deployment topologies and
  failure-mode walkthroughs are documented at
  [`docs/guides/coord-ha.md`](./docs/guides/coord-ha.md).
- **Versioned workflow DAG schema** (sprint `W4`). New
  `schema_version: 1` field required on every `workflow.json`.
  Spec at [`docs/spec/workflow-dag-1.0.md`](./docs/spec/workflow-dag-1.0.md).
  `boruna_orchestrator::WORKFLOW_DAG_SCHEMA_VERSION = 1`
  exposed for compatible readers. Forward-compat: 1.x readers
  accept any 1.y workflow (additive fields ignored).
- **CI clippy gate now uses `--all-targets`** (sprint `W1-A`).
  All three clippy invocations in `.github/workflows/ci.yml` now
  include `--all-targets` so test-code lint regressions surface
  at PR time instead of only at release-runner time. Filed as a
  followup in the B-2 retro after the workspace `--all-targets`
  sweep landed.
- **Formal versioned `.ax` language specification** at
  `docs/spec/ax-language-1.0.md` (sprint `W1-B`).
  `language_version: "1.0"` exposed via
  `boruna_compiler::LANGUAGE_VERSION`. New `docs/spec/README.md`
  indexes versioned specs. The narrative reference
  (`docs/reference/ax-language.md`) cross-links the spec.
- **Versioned evidence bundle format** with `format_version: "1.0"`
  in `bundle.json` (sprint `W1-C`). Forward-compat reader gate
  rejects bundles from incompatible major versions; same-major
  bundles are accepted with unknown fields ignored. Spec:
  [`docs/spec/evidence-bundle-1.0.md`](./docs/spec/evidence-bundle-1.0.md).

### Changed

- **BREAKING:** Evidence bundles now require a top-level
  `bundle.json` manifest (sprint `W1-C`). Legacy bundles from
  v0.5.0 and earlier must be migrated (migration tool planned
  for sprint `W5-C`; until then, re-record against a current
  binary).
- **BREAKING:** `workflow.json` files without `schema_version`
  are now rejected (sprint `W4`). All bundled examples updated.
  Operator action: add `"schema_version": 1` to existing
  workflow definitions before upgrading.

### Decided

- **1.x is the long-term-support line.** At 1.0 GA, the surfaces listed
  in [`docs/lts.md`](docs/lts.md) section B are LTS-protected for the
  full 1.x line: every 1.0 `.ax` program, workflow.json, evidence bundle,
  MCP integration, and CLI invocation continues to work unchanged on every
  1.y. Active support runs 18 months from 1.0 GA, security support 24
  months. Breaking changes ride the 2.0 boat with at least 6 months of
  deprecation notice and migration tooling for any mechanically-derivable
  upgrade. Internal Rust APIs, default values, and logging output formats
  are explicitly out of scope — Boruna ships a CLI + binary, not a Rust
  library. See `docs/lts.md` for the full contract.

## [0.5.0] - 2026-04-28

**Theme: distributed execution.** Boruna can now run a fleet of
worker processes coordinated by a single HTTP coordinator,
drive workflows over the wire from CI runners that don't share
a data-dir, handle large LLM step outputs without bloating the
SQLite store, and serve human-in-the-loop and webhook-driven
gates against a remote cluster. Read paths are consistent across
in-process resume, evidence-bundle creation, dashboard
rendering, and the `step_input` builtin — every persistence
reader of step outputs goes through the same accessor.

The 0.5-S2a → 0.5-S2f sub-sprint cycle landed during 0.4.x and
is included in this tag for the first time as a versioned
release (the distributed-execution stack: claim/lease
persistence, coordinator/worker HTTP MVP, lease-expiry sweep,
coord+dashboard listener-merge, `workflow run --submit-only`,
`coordinator wait`).

### Added

- **Workspace clippy `--all-targets` is clean** (sprint `B-2`).
  Pre-existing test-code lints from rustc 1.91+ in 4 crates
  (boruna-bytecode, boruna-vm, boruna-framework, boruna-compiler)
  plus a few in production paths cleared in one sweep. Auto-fix
  handled `needless_borrows_for_generic_args`, `manual_contains`,
  `clone_on_copy`, `for_kv_map`, `manual_is_multiple_of`. Manual
  fixes for `module_inception` (4× tests.rs files,
  `#[allow]` on inner mod), `type_complexity` (3 sites in
  `llmvm/capability_gateway` tests, factored to a
  `RecordedCalls` type alias), `approx_constant` (test fixture
  used 3.14 for arbitrary roundtrip — replaced with 2.5),
  `await_holding_lock` (existing test had a `MutexGuard`
  whose binding scope spanned an `.await` across an explicit
  `drop()`; rebound inside a block scope so drop is automatic
  before any await).

- **Dashboard renders step outputs with blob-aware fallback**
  (sprint `0.5-S7b`). The per-run detail HTML page gains an
  `Output` column. Inline outputs render in a `<code>` block
  truncated to 256 chars; blob-stored outputs render
  `[blob: <hash[..16]>…]` linked to the S7
  `/api/runs/{run_id}/blobs/{hash}` route, without slurping the
  bytes into the dashboard render. Pending/Running/paused steps
  show `—`. Reads route through `RunCheckpointStore::read_step_output`
  for inline cases (the same accessor used by the resume and
  evidence-bundle paths). The JSON detail endpoint
  (`GET /api/runs/{id}`) is unchanged — `StepCheckpoint` already
  serializes both `output_json` and `output_blob_ref` fields, so
  programmatic consumers can branch on the shape directly.
  3 new HTML rendering tests. See
  `docs/design-dashboard-blob-render.md`.

- **Output blob references for large step outputs** (sprint
  `0.5-S7`). Step outputs whose JSON encoding exceeds 64 KiB are
  now offloaded to a content-addressed blob store at
  `<data-dir>/blobs/<aa>/<hash>`, keyed by SHA-256. The
  `step_checkpoints.output_blob_ref` column carries the hash; the
  inline `output_json` column is left NULL when the blob path is
  used. Mutually exclusive: at most one of the two columns is
  populated for any terminal-state row. Audit hashes are
  unchanged — the ref IS the existing `output_hash`, so
  evidence-bundle replay across pre-S7 and post-S7 runs produces
  byte-identical hash chains. New schema migration `v3 → v4`
  (additive `ALTER TABLE ADD COLUMN`, no table rewrite). New
  coordinator HTTP route `GET /api/runs/{run_id}/blobs/{hash}`,
  bearer-gated and run-scoped (the route only serves bytes if
  the requested hash is referenced by a checkpoint under the
  given run_id, preventing the route from acting as a generic
  blob server). New `error_kind` taxonomy: `coord.blobs.bad_hash`
  (400) and `coord.blobs.not_found` (404). Threshold is hard-coded
  for the sprint (no `Policy` knob); a future sprint may make it
  configurable. 27 new unit tests (15 blob_store, 12 persistence)
  + 5 new coord handler tests. See
  `docs/design-output-blob-refs.md` and
  `docs/architecture-output-blob-refs.md`.

- **Distributed approval-gate / external-trigger** (sprint
  `0.5-S6`). Two new operator-facing routes — `POST
  /api/runs/{run_id}/approve` and `POST /api/runs/{run_id}/trigger`
  — bearer-gated by the same auth middleware as worker endpoints.
  CLI flags `--coordinator <url>` + `--coord-token` added to
  `boruna workflow approve|reject|trigger` so CI runners can
  drive remote runs without shared `data-dir`. The wait driver
  (`advance_run_one_tick`) now opens approval / trigger gates
  when their dependencies complete, and closes them when the
  decision sentinel arrives in `metadata.approvals` /
  `metadata.triggers` — same synthesized output shape as the
  in-process resume sentinel pass so a run approved via either
  route hashes to the same evidence bundle. Five handler unit
  tests + three advance-loop unit tests + one end-to-end CLI
  integration test. New `error_kind` taxonomy entries:
  `coord.approve.invalid_state`, `coord.approve.bad_payload`,
  `coord.trigger.invalid_state`, `coord.trigger.bad_token`,
  `coord.trigger.bad_payload`. See
  `docs/design-0.5-s6-distributed-approval-trigger.md`.

- **`boruna workflow run --coordinator <url>`** (sprint
  `0.5-S4`). Submits a workflow over HTTP to a remote
  coordinator and polls for terminal status — eliminates the
  shared-`data-dir` requirement for CI workflows. Workflow
  definition + every Source-kind step's `.ax` body are inlined
  into the submit payload. Bearer token via `--coord-token` or
  the `BORUNA_TOKEN` env var. Exit codes match `coordinator
  wait`: `0` Completed, `1` Failed, `2` timeout / submit-failed.
  Two new coordinator HTTP routes: `POST /api/runs/submit` and
  `GET /api/runs/{run_id}/status`, both bearer-gated by the
  same auth middleware as worker endpoints. Status reads
  fold `advance_run_one_tick` into the request so the
  operator's poll IS the wait driver. Six handler unit tests +
  three end-to-end CLI integration tests. New `error_kind`
  taxonomy entries: `coord.submit.invalid_workflow`,
  `coord.submit.bad_payload`, `coord.runs.not_found`. See
  `docs/design-0.5-s4-coordinator-flag.md`.

- **Sprint A debt cleanup** (preceding commit `chore/0.5-debt-cleanup-2`).
  Five carried-forward debts cleared in one pass:
  eliminate `unsafe { env::set_var }` (production CLI + 2
  tests) by threading env name explicitly through
  `resolve_data_dir`/`metrics::export`; new
  `error_class::TRANSIENT_NETWORK` taxonomy entry detected
  from both `VmError::AssertionFailed` and wire-level
  `error_msg` strings; `AuditLog::from_entries_verified`
  called at evidence-bundle creation to catch direct sqlite3
  tamper of `metadata.audit_log`; new Prometheus
  `boruna_workflow_run_duration_seconds` histogram for p50/p95/p99
  dashboards; five drift-detection tests for
  `docs/reference/policy.schema.json` (caught real drift —
  schema was missing `step.input` capability, fixed).

- **Carried-debt cleanup pass** (preceding session). Three small
  fixes from earlier-sprint adversarial-review findings that
  hadn't been addressed:

  - **Audit chain wait-driven terminating event.** New
    `WorkflowRunner::append_wait_terminal_audit_event` emits
    `WorkflowCompleted` to the audit chain when the wait
    driver reaches Completed or Failed terminal status.
    Idempotent — re-invoked waits don't double-emit. Closes
    the gap from 0.5-S2f where submit-only emitted
    `WorkflowStarted` with no terminating entry. 3 new unit
    tests.

  - **Two-concurrent-waits integration test.** CORR-6 from
    0.5-S2f adversarial review. Locks the design intent: two
    `coordinator wait` processes against the same run_id both
    converge to exit 0 because the underlying
    `insert_pending_step_if_absent` and
    `requeue_failed_step_for_retry` primitives are
    `INSERT … ON CONFLICT DO NOTHING` (race-safe).

  - **Submit-only `--concurrency` warning.** Adversarial
    finding F3 from 0.5-S2e. `boruna workflow run --submit-only`
    silently ignored `--concurrency` because parallelism in
    distributed mode is controlled by the worker pool, not
    the in-process wave loop. Now emits a clear stderr
    warning at submit time so operators know.

- **Path-resolution failure-mode prevention** (this session).
  After the multi-sprint parallel-agent attempt that wrote
  files to the wrong worktree (because absolute paths in
  agent prompts bypass the `cwd` redirection of
  `isolation: "worktree"`), this session hardens the workflow:

  - New `docs/AGENT-PROMPT-TEMPLATE.md` — reusable skeleton
    for parallel worktree-agent prompts. Bakes in the
    relative-path discipline + a worktree-verification block
    that agents must run before any file edit.
  - New `CLAUDE.md` "Parallel-Agent Best Practices" section
    documenting the failure mode and the required prevention.
  - New project convention #31 — "Parallel worktree-agent
    prompts use RELATIVE paths only" — anchored in the
    convention memory.
  - New project convention #32 — "Strong gates absorb tooling
    failures" — locks the recovery posture.

- **Shared-secret bearer authentication for the coordinator**
  (sprint `0.5-S3`). Enables production deployment by gating
  every coord HTTP route on a bearer token. `coordinator serve
  --shared-secret <hex>` (or `BORUNA_COORD_SECRET` env var) and
  worker `--shared-secret <hex>` (same env var fallback)
  configure the symmetric secret. Mismatched or missing
  `Authorization: Bearer` header returns
  `401 + error_kind: coord.unauthorized`. When unset, no auth
  is enforced — the pre-0.5-S3 loopback-only behavior is
  preserved for backwards compatibility, with a loud stderr
  warning when the coord binds to a non-loopback address
  without a secret.

  Generate a secret via `openssl rand -hex 32`. mTLS,
  per-worker keys, and OAuth integration deferred to 0.6.x —
  shared-secret covers the common operator case (single
  trusted cluster, per-deployment secret rotation).

  Auth applies to merged dashboard routes too — operators
  who want a public read-only dashboard with auth-gated
  mutations should run a separate `boruna dashboard serve`
  process. The coordinator's merged listener is strictly
  all-or-nothing for auth.

  4 new CLI integration tests cover: missing bearer → 401,
  wrong bearer → 401, correct bearer → 200, no-secret legacy
  path → 200 (no regression).

- **Distributed retry policies** (sprint `0.5-S5`). Wires
  the existing `RetryPolicy` (`max_attempts`, `on_transient`,
  `retry_on`) through the wait driver so failed steps with
  retry budget transition `Failed → Pending` instead of
  permanent `Failed`. The coordinator stays dumb — all
  retry-decision logic lives in `WorkflowRunner::advance_run_one_tick`
  and a new persistence primitive
  `RunCheckpointStore::requeue_failed_step_for_retry`.

  The persistence primitive uses `BEGIN IMMEDIATE` + atomic
  status check inside the transaction; idempotent across
  concurrent wait clients (project convention §14). Returns
  a typed `RequeueOutcome` (`Requeued { new_attempt_count }`,
  `NotFailed { current_status }`, `NotFound`).

  `AdvanceResult` gains a `newly_requeued: Vec<String>` field
  (additive). The `coordinator wait` driver prints a
  distinct `step <id>: requeued (retry)` line for each
  requeued step before the generic transition print.

  Run-status derivation is updated: `Failed` is declared
  only when a step is `Failed` AND has no retry budget
  remaining. A `Failed`-with-budget step keeps the run
  `Running` and is requeued in the same tick.

  14 new orchestrator unit tests cover the retry pass:
  budget exhaustion, single-attempt rejection, error-class
  matching (`retry_on` vs. `on_transient` fallback),
  concurrent-wait race (idempotency), and policy-absent
  short-circuit. The pre-0.5-S5 wait limitation
  ("distributed retry not honored") is now resolved;
  `docs/design-coord-wait.md` updated accordingly.

- **`boruna fmt` auto-formatter for `.ax` files** (DX
  sprint, first item from the 0.2.x DX lane). Canonical
  pretty-printer that walks the existing compiler AST and
  emits formatted source.

  CLI: `boruna fmt <file>` rewrites in place; `boruna fmt
  --check <file>` exits 0 if the file is already formatted,
  exit 1 otherwise (CI gate). Exits 2 on parse errors so CI
  can distinguish "needs formatting" from "broken file".

  Style decisions: 4-space indent, trailing comma on
  multi-line records and match arms, blank line between
  top-level decls, same-line opening braces.

  **Known limitation (v1):** comments are stripped — the
  lexer drops them before they reach the parser, so the
  current AST has no comment positions. A token-aware
  comment-preserving formatter is future work. v1 is still
  useful as a CI gate for generated/scaffolded code or
  code reviews where comments are preserved manually.

  3 golden-fixture tests, 1 idempotency roundtrip, 1
  parse-failure error case, and 3 CLI integration tests
  (--check exit codes 0/1/2). New module
  `tooling/src/format/` with `format_source` and
  `check_source` public APIs.

- **`boruna coordinator wait <run-id>`** (sprint `0.5-S2f`).
  Multi-wave workflow advancement for distributed runs. After
  `workflow run --submit-only` writes the first wave's Pending
  checkpoints, `coordinator wait` polls runs.db, computes
  downstream-ready successors as workers complete steps, and
  writes Pending checkpoints for the next wave — repeating
  until the run reaches a terminal status.

  ```sh
  boruna coordinator serve --data-dir /var/lib/boruna &
  boruna worker run --coordinator http://127.0.0.1:8090 &
  boruna workflow run examples/workflows/document_processing \
      --submit-only --data-dir /var/lib/boruna
  # ↳ submitted run_id=...

  boruna coordinator wait <run-id> --data-dir /var/lib/boruna
  # ↳ polls every 500 ms, prints transitions per step,
  #    exits 0 on Completed / 1 on Failed
  ```

  **Coordinator gains zero new logic** — the "dumb transport"
  invariant from 0.5-S2c is preserved. All wave advancement is
  client-side. The wait driver is stateless: kill it at any
  point and re-invoke; the run continues from the persisted
  state.

  New flags on `coordinator wait`:
  - `--poll-interval-ms <ms>` (default 500, minimum 100; values
    below the floor are clamped with a warning).
  - `--max-wait-secs <s>` (default 0 = unlimited; useful for CI
    timeouts).

  Exit codes: `0` Completed, `1` Failed, `2` error (run not
  found, missing `workflow_def`, unsupported step kind in non-
  first wave), `3` `--max-wait-secs` exceeded.

  New persistence primitive
  `RunCheckpointStore::insert_pending_step_if_absent(run_id,
  step_id) -> bool` uses `INSERT ... ON CONFLICT DO NOTHING`
  so the wait client can safely write Pending checkpoints
  even when the coordinator is concurrently transitioning
  sibling steps. The legacy `upsert_step_checkpoint` (which
  hard-overwrites status on conflict) is unchanged; the new
  primitive is the race-safe variant for client-side
  advancement. Locked by
  `insert_pending_step_if_absent_preserves_running_row`.

  New field `PersistedRunMetadata::workflow_def:
  Option<WorkflowDef>` (with `#[serde(default)]` for
  back-compat). Embedded only when `submit_only=true`; capped
  at 1 MiB serialized JSON. In-process runs leave it `None` to
  keep metadata small.

  New `WorkflowRunner::compute_ready_steps(def, status_map)`
  (pure, deterministic-sort) and `advance_run_one_tick(store,
  run_id) -> AdvanceResult` (one polling tick).

  Tests: 14 new orchestrator unit tests covering the advance
  loop, the size cap, race-safe persistence, and idempotency.
  4 new CLI integration tests: marquee multi-wave end-to-end,
  kill-and-resume, fail-on-bad-step, immediate-exit-on-already-
  completed.

  **Known limitations** (deferred to 0.5-S3+):
  - Retry policies in distributed mode — a step that fails is
    terminal; the wait driver exits with status 1 even if a
    retry policy would have succeeded in-process. Distributed
    retry is a future sprint.
  - Audit-chain coverage — the wait driver does NOT append
    `WorkflowCompleted`/`WorkflowFailed` events. Submit-only
    emits `WorkflowStarted` but the chain has no terminating
    entry for distributed runs. Auditors should check
    persisted `runs.status` directly.
  - Concurrent wait clients — multiple `coordinator wait`
    processes against the same `run_id` are safe (the race-
    safe primitive ensures idempotency) but not specifically
    tested as an integration scenario.
  - HTTP-based remote wait — the wait client requires
    filesystem access to `--data-dir`. A truly remote
    `coordinator wait --coordinator <url>` mode is a future
    sprint.

- **`boruna workflow run --submit-only`** (sprint `0.5-S2e`).
  The first end-to-end path for dispatching a real workflow
  through a coord+workers cluster. Submit-only mode:
  validates + computes the DAG, embeds source-step bodies in
  `metadata_json.step_sources`, inserts the run row + initial
  wave's source-step Pending checkpoints, then **exits before
  spawning thread workers**. The cluster picks up the steps
  via existing claim/dispatch mechanisms.

  ```sh
  boruna coordinator serve --data-dir /var/lib/boruna &
  boruna worker run --coordinator http://127.0.0.1:8090 &
  boruna workflow run examples/workflows/llm_code_review \
      --submit-only --data-dir /var/lib/boruna
  ```

  Workflows using approval-gate / external-trigger steps in
  the first wave are rejected at submit time with a typed
  error (`submit-only mode does not support ... in the first
  wave`). Distributed mode for those features is deferred.

  Multi-wave automatic advancement is NOT done — operators
  monitor via the dashboard or `boruna workflow show
  <run-id>`. Wave loop integration becomes 0.5-S2f or later.

  Added field `RunOptions::submit_only: bool` and field
  `PersistedRunMetadata::step_sources: BTreeMap<String,
  String>` (with `#[serde(default)]` for back-compat). The
  `WorkflowStarted` audit event fires for submit-only runs
  matching the in-process `run_persistent` semantics.

  Tests: 3 new unit tests (insertion shape, metadata
  embedding, approval-gate rejection) + 1 new CLI integration
  test that runs `boruna workflow run --submit-only` against
  a real workflow.json + .ax file with a spawned coord+worker
  pair, asserts the step transitions through Pending →
  Running → Completed and the output_json matches the
  expected value.

- **Coordinator + dashboard listener-merge** (sprint
  `0.5-S2d`). The dashboard's read-only routes (`/`,
  `/runs/:id`, `/api/runs`, `/api/runs/:id`) are now served
  on the same listener as the coordinator's worker routes
  (`/api/workers/...`, `/api/work/...`). Operators get fleet
  visibility AND distributed dispatch from a single
  `boruna coordinator serve` invocation — one process, one
  port, one connection to runs.db.

  The merge is automatic — anyone running the coordinator
  gets the dashboard routes too. The standalone
  `boruna dashboard serve` keeps working unchanged for
  read-only deployments without the coordinator overhead.

  The coordinator's `bind_warning` flows into the dashboard
  builder so the red HTML banner correctly fires when the
  coordinator is bound to a non-loopback address. Operators
  can't accidentally expose the coordinator without the
  dashboard banner warning them.

  Refactor: `dashboard::dashboard_routes(store, bind_warning)`
  is now a `pub` route builder taking primitive args. The
  coordinator merges it onto its own router. Zero
  copy-paste; both the standalone dashboard and the
  coordinator use the same builder.

  3 new CLI integration tests cover the merged surface.

- **Coordinator background lease-expiry sweep** (sprint
  `0.5-S2c`). The coordinator now runs a tokio interval task
  that wakes up every `--sweep-interval-ms` (default 30 s)
  and calls `expire_leases_and_requeue`. Stale leases from
  worker crashes are now recovered without restarting the
  coordinator. Best-effort failure semantics: errors log +
  continue to the next tick.

  New CLI flag on `boruna coordinator serve`:
  `--sweep-interval-ms <ms>` (default 30000, minimum 100;
  values below the floor are clamped with a warning).

  New CLI integration tests:
  - `coord_bg_sweep_requeues_expired_lease` proves the sweep
    fires periodically and requeues stale leases without a
    coordinator restart.
  - `worker_completes_two_step_linear_dag` proves the
    protocol scales beyond a single step. (DAG advancement
    by the coordinator itself is deferred to 0.5-S2d; this
    test pre-populates both steps as Pending up front.)

  Architectural note documented in
  `docs/design-coord-bg-sweep.md`: in v0.5.x the coordinator
  is a "dumb transport" — it dispatches what's in Pending
  and persists what completes. Wave advancement (deciding
  which step is Pending after a successful completion based
  on DAG dependencies) lives in the client. The
  `boruna workflow run --coordinator <url>` client mode
  ships in 0.5-S2d.

- **Coordinator/worker HTTP MVP** (sprint `0.5-S2b`). The HTTP
  layer over the persistence-layer state machine from 0.5-S2a.
  Two new CLI subcommands behind the `serve` feature flag:
  - `boruna coordinator serve --data-dir <path> [--port 8090]
    [--bind 127.0.0.1] [--max-lease-ttl-ms 300000]
    [--poll-timeout-ms 30000]`
  - `boruna worker run --coordinator <url> [--worker-id <name>]
    [--lease-ttl-ms 300000] [--poll-timeout-ms 30000]`

  Six HTTP routes per ADR 002:
  `POST /api/workers/register`, `POST /api/workers/heartbeat`,
  `GET /api/work/claim` (long-poll), `POST /api/work/complete`,
  `POST /api/work/fail`, `POST /api/work/extend-lease`. Every
  response carries `protocol_version: 1`. Worker-side: register
  → long-poll claim → compile + execute the step's `.ax`
  source → POST result. Heartbeats every 10 s in a background
  task.

  Stable `coord.*` `error_kind` taxonomy, locked at this
  sprint's ship: `coord.lease_expired`, `coord.unknown_worker`,
  `coord.binary_mismatch`, `coord.invalid_request`,
  `coord.output_too_large`, `coord.step_not_found`. The HTTP
  layer maps the persistence-layer outcome enums (from 0.5-S2a)
  1:1 — no string-equality drift.

  Workers must match the coordinator's `capability_set_hash`
  per ADR 002's atomic-upgrade rule; mismatched workers get
  `409 + coord.binary_mismatch`. Output payload size capped at
  8 MiB per ADR 002; oversize bodies get `413 Payload Too
  Large` from Axum's `DefaultBodyLimit`.

  Workers parse policy via the strict validator from sprint
  `0.4-S15` (`boruna_vm::policy_validate::parse`), so workers
  reject the same shapes the CLI rejects with the same stable
  `error_kind` strings.

  On startup, the coordinator runs `expire_leases_and_requeue`
  to void any stale leases left over from a prior coordinator
  process (per ADR 002's "coordinator restart = all leases
  void" rule).

  Loopback (`127.0.0.1`) by default. Non-loopback bind emits a
  loud stderr warning. **No authentication** — operators
  exposing the coordinator MUST front it with an
  auth-enforcing reverse proxy.

  Tests: 9 coordinator handler unit tests (route shapes,
  error_kind strings, status codes, lease-cap enforcement) +
  4 worker unit tests (compile, execute, hash determinism,
  url-encoding) + 6 CLI integration tests including the
  flagship `worker_kill_mid_step_lease_expires_then_reclaim`
  regression that exercises the slow-but-not-dead worker race
  end-to-end at the wire level.

  New deps in the `serve` feature: `reqwest 0.12` (json +
  rustls-tls, no openssl) for the worker's HTTP client; `uuid
  1` for worker_id / session_token allocation.

  **Not in this sprint (deferred to 0.5-S2c):** workflow
  runner integration (`boruna workflow run --coordinator
  <url>`), wave-loop coordinator-side dispatcher, dashboard +
  coordinator listener-merge.

  See `docs/design-coordinator-worker-http.md`,
  `docs/architecture-coordinator-worker-http.md`,
  `docs/test-plan-coordinator-worker-http.md`.

- **Claim/lease persistence API** (sprint `0.5-S2a`). The
  persistence-layer half of ADR 002. Schema v3 adds three
  operational columns to `step_checkpoints`: `worker_id` (opaque
  worker handle), `lease_expires_at` (unix ms), and `claim_id`
  (monotonic per `(run_id, step_id)`, CAS key for terminal-state
  transitions). Five new methods on `RunCheckpointStore`:
  - `claim_step` — atomic Pending → Running transition with
    incremented `claim_id`.
  - `complete_step_cas` — CAS-protected completion. Rejects late
    writes from expired-lease workers without changing persisted
    state.
  - `fail_step_cas` — CAS-protected terminal failure.
  - `expire_leases_and_requeue` — sweep expired leases back to
    Pending. Idempotent.
  - `extend_lease_cas` — push out the lease deadline, CAS-protected
    against the original `claim_id`.
- New outcome enums: `ClaimOutcome`, `TerminalOutcome`,
  `ExtendOutcome`. Each carries a stable `kind() -> &'static str`
  per project convention #2 (`claim.*`, `terminal.*`, `extend.*`).
  These map to the wire-level `coord.*` `error_kind` strings the
  HTTP coordinator will lock in 0.5-S2b.
- Schema v2 → v3 migration via the existing migration runner.
  Idempotent — re-opens are no-ops; fresh databases get the
  full v3 schema directly from `SCHEMA_V1_SQL`.
- 32 new persistence tests including the load-bearing
  `slow_worker_race_late_completion_rejected` regression that
  exercises the slow-but-not-dead worker race the ADR's
  adversarial review caught: claim → expire → reclaim →
  original worker's late completion → `LeaseExpired` rejection
  → row state unchanged. If this test ever fails, the state
  machine is broken.
- The single-process `WorkflowRunner` path is unchanged.
  `upsert_step_checkpoint` does not write the new columns; they
  stay at their defaults (`None`, `0`) for steps that flow
  through the in-process scheduler.

### Decided

- **ADR 002 — Distributed step execution.** The 0.5.0 ("Scale")
  cycle's foundational architectural decision. Distributed mode
  uses an embedded HTTP coordinator + lightweight HTTP workers,
  all behind the existing `serve` feature flag. The coordinator
  remains the only writer of `runs.db`; workers long-poll for
  claimable steps and report results via JSON over HTTP. Lease-
  based claim with re-dispatch on expiry handles worker crashes.
  Determinism is preserved: which worker ran a step is
  operational state and never enters the audit/replay pipeline.
  The single-process path (`boruna workflow run`/`resume`) keeps
  working unchanged. Considered alternatives — shared-filesystem
  SQLite, external queue (Redis/RMQ/SQS), gRPC — were rejected
  for footgun risk, deployment-simplicity violation, and
  marginal benefit respectively. Implementation in `0.5-S2`.
  See [`docs/adr/002-distributed-step-execution.md`](docs/adr/002-distributed-step-execution.md).

## [0.4.0] — 2026-04-27

The **operations** release. Twelve sprints (0.4-S5 through
0.4-S16) ship the production-readiness layer on top of 0.3.0's
durability work: distributed-tracing observability, streaming
progress, multi-pause-per-level wave loops, per-error-class retry
classification, hash-chained audit decisions and lifecycle
events, post-hoc evidence-bundle creation, Prometheus metrics,
multi-provider LLM dispatch, multi-environment data separation,
strict-validated policy-as-code, and a read-only HTTP dashboard.

### Added

- **Workflow dashboard** (sprint `0.4-S16`). New `boruna dashboard
  serve` subcommand exposes a read-only HTTP view of `runs.db` so
  operators can triage at a glance without dropping into `sqlite3`.
  Loopback (`127.0.0.1`) by default; `--bind 0.0.0.0` is allowed
  but shouts a loud warning on stderr AND renders a red banner in
  the HTML, because the dashboard ships with **no authentication.**

  ```sh
  cargo build --release -p boruna-cli --features serve
  boruna dashboard serve --data-dir /var/lib/boruna
  ```

  Routes: `GET /` (HTML index), `GET /runs/:id` (HTML detail),
  `GET /api/runs` (JSON list), `GET /api/runs/:id` (JSON detail).
  Zero mutation routes — `POST`/`PUT`/`DELETE`/`PATCH` to any path
  return 405. Multi-env aware: when `--env` is set, the dashboard
  reads `<data-dir>/<env>/runs.db` per the 0.4-S14 contract.

  Builds behind the existing `serve` feature flag (already used by
  `boruna serve` for framework apps). Reuses the workspace
  `axum 0.8` + `tokio` deps.

- `boruna_orchestrator::persistence::{RunRow, RunRecord,
  RunOperational, StepCheckpoint}` now derive `Serialize` so
  read-only consumers can render rows directly. (Not
  `Deserialize` — there's no scenario where a dashboard consumer
  should be reconstructing a row.)

- 18 new unit tests in `dashboard::tests` covering every handler,
  HTML escaping (XSS regression), bind-warning banner, 404, and
  the date-format helper. 8 new CLI integration tests in
  `crates/llmvm-cli/tests/cli_dashboard.rs` covering end-to-end
  HTTP behavior, the read-only contract (POST → 405), and CLI
  error paths (missing data-dir, invalid bind address).

- New CI steps to build and test the `serve` feature
  (`cargo build/test/clippy -p boruna-cli --features serve`).

- New reference doc `docs/reference/dashboard.md` covering build,
  run, security posture, routes, stability tier.

- **Policy management as code** (sprint `0.4-S15`). Operators now
  treat `--policy` files as versioned, validated, code-reviewable
  artifacts. Two new CLI subcommands:
  - `boruna policy validate <file> [--json]` — strict-validate a
    policy file. Designed as a CI gate. Exits 0 on ok, 2 on
    validation error, 1 on file IO error.
  - `boruna policy show <file>` — validate then print the
    effective policy (default behavior, denormalized rule list,
    net_policy bounds).
  Plus a new MCP tool `boruna_policy_validate(policy_json)` that
  runs the same validator. The CLI, MCP, and `boruna run --policy
  <file>` paths now share **one parser** — passing validate but
  failing run is structurally impossible.
- New `boruna_vm::policy_validate::{parse, parse_file,
  PolicyParseError, POLICY_SCHEMA_VERSION}`. The validator
  enforces:
  - `schema_version` ∈ {`1`} (other values rejected — locks the
    contract for forwards-compat).
  - Top-level / `net_policy` / per-rule fields are an allow-list —
    unknown fields rejected (`policy.unknown_field`). Closes the
    silent-default footgun where `"default_alow": true` parsed as
    `default_allow: false`.
  - `rules` keys must be canonical capability names. Aliases
    (`"net"`, `"db"`, …) rejected with a hint to the canonical
    name (`"net.fetch"`, `"db.query"`). Aliases used to silently
    no-op at gateway-check time.
  - `net_policy.max_response_bytes > 0`, `timeout_ms > 0`,
    `allowed_methods` ⊆ `{GET, POST, PUT, DELETE, PATCH, HEAD,
    OPTIONS}` (canonical upper-case; lower-case rejected).
- Stable `error_kind` taxonomy — locked per project convention #2:
  `policy.io_error`, `policy.parse_error`,
  `policy.unknown_schema_version`, `policy.unknown_field`,
  `policy.invalid_capability`, `policy.invalid_net_policy`. Future
  validators can add new kinds; existing kinds never rename.
- 26 unit tests in `boruna_vm::policy_validate` + 11 CLI
  integration tests in `crates/llmvm-cli/tests/cli_policy.rs` + 7
  MCP tests + 3 protocol_version regression tests.
- Updated `docs/reference/policy-schema.md` with the strict-validator
  rules, error_kind taxonomy, and CLI tooling examples. Design
  rationale in `docs/design-policy-as-code.md`.

### Fixed

- `Policy::default()` now produces `schema_version: 1` (matching
  what the lenient deserializer's `#[serde(default = "...")]`
  produces for an empty input). The derived default leaked
  `schema_version: 0` into round-trips — invisible until the
  `0.4-S15` strict validator surfaced it. Affects `Policy::deny_all()`
  and any caller that started from `Policy::default()`.

- **Multi-environment support** (sprint `0.4-S14`). New global
  `--env <name>` flag (also from `BORUNA_ENV` env var). When set:
  - `--data-dir` is namespaced to `<data-dir>/<env>/` so each
    environment has its own runs.db, audit chains, and evidence
    bundles.
  - Every Prometheus metric gains an `env="<env>"` label so dashboards
    can filter / group by environment.
  ```sh
  boruna --env staging workflow run wf --data-dir /var/lib/boruna ...
  boruna --env prod workflow run wf --data-dir /var/lib/boruna ...
  # → /var/lib/boruna/staging/ and /var/lib/boruna/prod/ stay separate
  ```
  Operators get dev/staging/prod separation without external
  orchestration. Per-env policy is supplied via `--policy` per call.
- New `boruna_orchestrator::metrics::format_prometheus_with_env`
  variant. Backward compatible: `format_prometheus(snap)` continues
  to produce env-less output (calls `format_prometheus_with_env(snap,
  None)` internally).
- New CLI helper `validate_env_name` rejects names with characters
  outside `[a-zA-Z0-9_-]` (length 1-64). Protects against path
  traversal (`--env ../../etc/passwd` is rejected at the boundary)
  and broken Prometheus labels.
- 4 new tests in `metrics`: env label added to every series, env-less
  output is byte-identical to legacy, env label escapes, end-to-end
  `BORUNA_ENV` round-trip via the `export` entry.

#### Backward compatibility

When `--env` and `BORUNA_ENV` are both unset, behavior is exactly
as before: data goes to `<data-dir>/`, metrics carry no `env` label.
Operators upgrading from 0.4-S13 see no change unless they opt in.

- **`LlmRouterHandler` — multi-provider LLM dispatch helper** (sprint
  `0.4-S13`). Direct extension of the BYOH decision in `0.3-S8`.
  Integrators with multiple LLM providers (OpenAI + Anthropic +
  local Ollama / vLLM) no longer need to write their own dispatch
  logic — the router takes a registry of provider handlers and
  routes each `Capability::LlmCall` based on a `provider/model`
  prefix in `args[1]`:
  ```rust
  let mut providers: BTreeMap<String, Box<dyn CapabilityHandler>> = BTreeMap::new();
  providers.insert("openai".into(), Box::new(my_openai_handler));
  providers.insert("anthropic".into(), Box::new(my_anthropic_handler));
  let router = LlmRouterHandler::new(providers, Box::new(MockHandler));
  ```
  `.ax` callers then write `llm_call("Summarize:", "openai/gpt-4")` —
  the prefix selects the provider; the full model string (including
  the prefix) is forwarded unchanged so providers can use it for
  internal tagging.
- The router is pure routing logic — Boruna still ships **zero**
  provider HTTP code. Each provider's handler implementation,
  authentication, and response parsing belong to the integrator
  per the BYOH model.
- Non-LLM capability calls pass through to a fallback handler so
  the router composes with the existing `StepInputHandler` /
  `MockHandler` / `HttpHandler` stack.
- Typed errors for: missing model arg, non-string model arg,
  malformed model string (no `/`), empty provider prefix, unknown
  provider (error message includes the registered providers list).
- Late-registration support via `add_provider(name, handler)`
  returning the previously-registered handler.
- 11 unit tests covering routing, args forwarding, error variants,
  fallback delegation, late registration, and deterministic
  `registered_providers` ordering.
- Updated `docs/guides/llm-integration.md` with a new section
  walking through the router setup.

- **Prometheus metrics export CLI** (sprint `0.4-S12`). New
  `boruna metrics export --data-dir <DIR>` command reads the
  persistent run store and writes Prometheus text format to stdout.
  Operators integrate via cron + `node_exporter`'s textfile
  collector — the canonical Prometheus pattern for batch tools:
  ```
  */30 * * * * boruna metrics export --data-dir /var/lib/boruna \
                  > /var/lib/node_exporter/textfile_collector/boruna.prom
  ```
  Architectural decision documented in
  `docs/design-prometheus-metrics.md`: CLI-pulled (not embedded HTTP)
  to align with Boruna's CLI-only philosophy locked in `0.3-S15`
  (BYOH webhook pattern). No new long-running daemon process.
- Three metric families:
  - `boruna_workflow_runs_total{workflow,status}` — counter of runs
    by terminal/transient status.
  - `boruna_workflow_runs_in_flight{workflow}` — gauge of `running`
    or `paused` runs.
  - `boruna_workflow_step_completions_total{workflow,step,status}` —
    counter of step terminal transitions (`completed` / `failed`).
- New `boruna_orchestrator::metrics` module with `compute_snapshot`,
  `format_prometheus`, and `export` public entries. The snapshot is
  pure data so future exporters (JSON dashboard endpoint, etc.) can
  reuse it without re-querying the store.
- 8 unit tests covering: empty store emits HELP+TYPE only,
  aggregation by workflow/status, in-flight counting, terminal
  step transitions only (no Pending/Running noise), output is
  valid Prometheus textfile format with HELP/TYPE preceding data,
  determinism (BTreeMap iteration locked), label escaping
  (backslashes, quotes, newlines per the exposition spec),
  end-to-end realistic run set.

#### Counter semantics caveat

Counters are computed from current store state at sample time, not
maintained as deltas. If old runs are pruned from the DB, the
`_total` will decrease — Prometheus normally treats this as a
counter reset and handles it gracefully via `rate()`. Operators
running frequent pruning should be aware of this contract.

- **Full lifecycle audit events** (sprint `0.4-S11`). Closes the
  audit theme for 0.4.0. The audit chain now captures the complete
  run lifecycle, not just operator decisions:
  - `WorkflowStarted { workflow_hash, policy_hash }` — appended at
    `execute_after_insert`'s top, immediately after the run row
    inserts.
  - `StepCompleted { step_id, output_hash, duration_ms }` — appended
    after each step's terminal `Completed` checkpoint write.
  - `StepFailed { step_id, error }` — appended after each step's
    terminal `Failed` checkpoint write (including panic-failed
    workers in the concurrent path).
  - `WorkflowCompleted { result_hash, total_duration_ms }` —
    appended at terminal status only (Completed/Failed). Resume's
    terminal exit also appends it. Pause states leave the chain
    open for the next resume to extend.
- New `append_audit_event(store, run_id, event)` helper using the
  same CAS-retry pattern as `record_approval_decision` /
  `record_external_trigger`. Lifecycle appends are best-effort: a
  CAS budget exhaustion logs a warning and continues. Missed audit
  events are operationally annoying (chain has fewer step events
  than checkpoints) but never fail the run — the chain entries that
  DID commit remain valid, and an auditor at `verify` time sees the
  gap explicitly.
- `StepStarted` events are deliberately NOT emitted — the
  checkpoint's `started_at_ms` already captures per-step start
  operationally, and emitting an event-per-start would double the
  CAS-write count for limited compliance value.
- 2 new tests in `tests::evidence_bundle`:
  `lifecycle_events_emitted_in_order_for_multi_step_run` (4-entry
  chain in topological order: Started → 2× StepCompleted →
  Completed) and `step_failed_event_emitted_on_runtime_error`
  (chain captures the failed step + error message).
- 7 existing audit_decisions / evidence_bundle tests updated to
  match the new chain shape (lifecycle events + decisions).

#### Audit theme summary

Across `0.4-S9` (decisions), `0.4-S10` (bundle creation), and
`0.4-S11` (lifecycle events), the audit story is now end-to-end
complete: every persistent run produces a hash-chained audit log
of all lifecycle transitions and operator actions, the chain is
persisted atomically with the corresponding state changes, and
`boruna evidence create <run-id>` packages it with all reproducibility
artifacts for downstream verification via `boruna evidence verify`.

#### Performance impact

For a workflow with N steps, the chain now requires roughly N+2
additional CAS-protected metadata writes (1 WorkflowStarted, N
StepCompleted/Failed, 1 WorkflowCompleted). Each write is a
single SQLite UPDATE with a small JSON blob. For typical workflows
this is operationally negligible. High-throughput integrators can
disable lifecycle audit by deferring this sprint's wiring (no
disable flag ships in this sprint — file an issue if needed).

- **`boruna evidence create <run-id>`** (sprint `0.4-S10`). Builds an
  evidence bundle from a persisted run by reading the run's metadata,
  step checkpoints, and hash-chained audit log. Closes the
  audit-evidence loop end-to-end:
  ```
  $ boruna workflow run wf --data-dir .data --policy allow-all
  $ boruna workflow approve <run-id> <step-id> --data-dir .data
  $ boruna workflow resume <run-id> --data-dir .data
  $ boruna evidence create <run-id> --output-dir bundles --data-dir .data
  $ boruna evidence verify bundles/<run-id>      # VALID
  ```
- New `boruna_orchestrator::workflow::create_bundle(data_dir, run_id,
  output_dir)` public entry. Reads workflow.json from the run's
  recorded `workflow_dir`, policy from the persisted `policy_json`
  column, per-step outputs from `step_checkpoints.output_json`, and
  the full audit chain from `metadata.audit_log` (sprint 0.4-S9).
  Builds an `EvidenceBundleBuilder`, finalizes, returns the
  `BundleManifest`.
- 6 new tests in `tests::evidence_bundle`: complete artifact for a
  completed run, audit chain round-trip via JSON, end-to-end
  `verify_bundle()` passes on the produced bundle, trigger payload
  hash matches the synthesized step output_hash, unknown run id
  returns typed `RunNotFound`, runs without decisions produce an
  empty chain whose `audit_log_hash` is the all-zeros sentinel.

#### Post-hoc bundle creation

The runner does NOT auto-create bundles during execution — the hot
path stays free of bundle I/O. Operators trigger bundle creation
explicitly when needed (e.g., a compliance request months after the
run completed). Same model as the rest of the audit subsystem:
operator-driven, not runner-driven.

- **Audit-log integration of approval / trigger decisions** (sprint
  `0.4-S9`). Closes a 0.3.0 carried-forward debt. Operator actions
  (approval grants/denials, external trigger events) now produce
  hash-chained audit entries, persisted as `metadata.audit_log` and
  written atomically with the operator-facing decision via the
  existing CAS-protected metadata writes.
- New `AuditEvent::ExternalTriggerReceived { step_id, payload_hash }`
  variant. The `payload_hash` matches the synthesized step
  `output_hash` (since the trigger payload becomes the step's
  output value), so the chain links to the replay-verified
  output. Payload itself is hashed rather than logged verbatim —
  webhook bodies may contain operator PII.
- New `AuditLog::from_entries(Vec<AuditEntry>) -> Self` and
  `AuditLog::into_entries(self) -> Vec<AuditEntry>` for round-
  tripping the chain through a containing struct (e.g. the run's
  persisted metadata) without re-serializing to JSON.
- 7 new tests covering: approval-grant / approval-reject append the
  right event, trigger appends with payload_hash equal to
  output_hash, multi-decision chain integrity (prev_hash chains),
  legacy 0.3.x metadata round-trip without audit_log field, audit
  log persists unchanged across resume, first decision after legacy
  metadata starts a fresh genesis chain.
- Design doc: `docs/design-audit-decision-events.md`.

#### Tamper-evidence vs replay-verification

The audit chain's `prev_hash` linkage is **tamper-evident** — any
post-hoc mutation (direct sqlite3 surgery, bit-flip in storage)
surfaces when an auditor calls `AuditLog::verify()`. The chain is
**not** processed by the run's deterministic-execution replay
pipeline; replay verifies per-step `output_hash`, not the
operator-action chain. Documented prominently in the
`PersistedRunMetadata.audit_log` doc-comment to prevent confusion
with the replay-verification subsystem.

#### Backward compatibility

A 0.3.x metadata blob with no `audit_log` field deserializes via
`#[serde(default)]` to `Vec::new()`. The first decision recorded
by a 0.4-S9 binary on a 0.3.x run starts a fresh genesis chain
(sequence=0, prev_hash="0"*64). Locked by
`first_decision_after_legacy_metadata_starts_chain_at_sequence_zero`.

#### What this sprint does NOT ship

- Full lifecycle audit events (`WorkflowStarted`, `StepStarted`,
  `StepCompleted`, etc.) — separately scheduled. This sprint
  surgically closes the operator-action audit gap without touching
  the per-step hot path.
- Audit log in evidence bundles — `EvidenceBundleBuilder::finalize`
  already accepts an `AuditLog` parameter; wiring the in-metadata
  log into bundle construction is a small follow-on sprint.
- Operator identity capture — no auth subsystem yet. The `approver`
  field is empty string until a future identity sprint wires real
  auth. The field IS captured in the hash chain regardless so a
  future upgrade can fill it in without re-keying past entries.

- **Per-error-class retry classification** (sprint `0.4-S8`). The
  `RetryPolicy` schema gains an explicit `retry_on: Vec<String>`
  allowlist alongside the legacy binary `on_transient` gate. Operators
  who want "retry on transient timeouts but NOT on auth errors or
  bad code" now express it directly:
  ```json
  "retry": {
    "max_attempts": 3,
    "on_transient": false,
    "retry_on": ["wall_time_exceeded", "io_error"]
  }
  ```
- New `error_class` taxonomy with stable string constants:
  `wall_time_exceeded`, `step_limit_exceeded`, `capability_denied`,
  `capability_budget_exceeded`, `compile_error`, `runtime_error`,
  `io_error`, `input_resolution`. Forward-compatible — new classes
  add without breaking existing policies.
- New `classify_vm_error(&VmError) -> &'static str` maps every VM
  error variant to its taxonomy class. Catch-all is `runtime_error`
  (assertions, type errors, OOB, divisions, stack errors, bytecode
  errors all surface here).
- `should_retry_class(policy, class) -> bool` — central decision
  function. Resolution order: no policy / max_attempts ≤ 1 → false;
  non-empty `retry_on` → match in list; empty → fall back to
  `on_transient`.
- `retry_with_backoff` short-circuits on non-retry-eligible failures
  rather than running through the full backoff schedule. A compile
  error no longer waits 100+200+400ms before giving up.
- 17 new tests covering classification mappings, allowlist semantics,
  legacy fallback, unknown-class-ignored, retry_on takes precedence
  over on_transient=false, and serde round-trip for legacy 0.3.x
  workflow.json files (no `retry_on` field).

#### Backward compatibility

- A 0.3.x `workflow.json` with `retry: {max_attempts, on_transient}`
  (no `retry_on` field) deserializes with `retry_on = vec![]` via
  `#[serde(default)]`. The empty allowlist falls back to the legacy
  `on_transient` gate, so prior behavior is exactly preserved.
- `class` strings are case-sensitive. Use the lowercase snake_case
  forms documented in `error_class::*`. Unknown strings (typos like
  `"transient_netwrok"`) are silently ignored — they never match a
  real failure class, so the policy behaves as if the typo were
  absent (conservative-by-default).

- **Wave-loop multi-pause-per-level** (sprint `0.4-S7`). The
  concurrent execution path (`--concurrency >= 2`) now pauses ALL
  pause-steps in the same DAG level in a single execution pass —
  previously only the first was processed and remaining pauses were
  silently deferred to subsequent resumes. Enables "wait for payment
  AND fraud-check" webhook fan-in patterns where multiple
  `external_trigger` (or `approval_gate`) steps depend on a shared
  upstream and a downstream step depends on all of them. Each pause
  persists its own checkpoint and (for trigger steps) mints its own
  distinct token. The resume sentinel pass advances each pause
  independently as its decision/event arrives.
- New `persist_one_pause` helper isolates per-pause persistence
  errors. If one pause's `acquire_trigger_token` or
  `upsert_step_checkpoint` fails (transient `/dev/urandom` error,
  CAS retry exhaustion, disk error), the loop logs a warning and
  continues to the next pause. The run is marked `Paused` on the
  pauses that DID commit, leaving operators with a recoverable
  state. The next resume's wave loop is idempotent — `acquire_trigger_token`
  reuses existing tokens and `upsert_step_checkpoint` is re-write-safe
  — so the failed pauses retry cleanly. Reviewed in 0.4-S7 — earlier
  draft propagated the first per-pause error, terminally-failing the
  run and stranding pause #1's token with no recovery path.
- 5 new tests in `tests::multi_pause_per_wave`: 2-trigger parallel
  pause, partial trigger fire keeps other paused, full trigger fire
  advances downstream, mixed approval+trigger pauses, partial-pause
  failure recovery via direct-SQL state injection.

#### Asymmetry note

The sequential execution path (`--concurrency 1`) is unchanged: it
processes one step at a time and serializes parallel pauses across
multiple resumes. Operators expecting AND-fan-in webhook patterns
must use `--concurrency 2` or higher.

- **Streaming progress notifications from `boruna_run`** (sprint
  `0.4-S6`, closes [#4](https://github.com/escapeboy/boruna/issues/4)).
  When the MCP caller supplies a `progressToken` in the request `_meta`
  field (per the MCP spec), the server emits
  `notifications/progress` events with the cumulative VM step count
  every 100k opcodes. Long-running scripts no longer block the calling
  agent's UI behind a single final result blob. Backward compatible:
  callers without a progressToken see the legacy synchronous behavior
  unchanged.
- New `Vm::start_timer()` method — initializes the wall-clock timer
  used by `max_wall_ms` budgets. Callers driving the VM through
  `execute_bounded` should call it before `set_entry_function` to
  match `Vm::run`'s timing contract (the entry-frame allocation
  counts toward the budget).
- New `Vm::set_in_actor_context(bool)` flag — replaces the prior
  `budget.is_some()` heuristic for distinguishing actor-system
  scheduling from standalone bounded execution. `Op::ReceiveMsg` on
  an empty mailbox now blocks (rewind IP + `MailboxEmpty`) only when
  the flag is set; standalone bounded loops fall through with
  `Value::Unit`, matching `Vm::run`'s legacy semantics. Reviewed in
  0.4-S6 — without this fix, the streaming-progress and non-streaming
  paths of `boruna_run` would diverge for any program emitting
  `Op::ReceiveMsg` outside an actor system.
- `ActorSystem::run` sets `in_actor_context = true` on the root and
  every spawned child VM.

## [0.3.0] — 2026-04-26

**Theme: Real-use durability.** 0.3.0 makes Boruna usable for
long-running, durable, production workflows. Persistent state survives
process restarts; concurrent steps fan out within waves; transient
failures retry with backoff; webhook-driven steps wait for external
events. The full sprint stack (`0.3-S2a` through `0.3-S16`) closes
every big-rock theme on the original 0.3.0 plan and adds review-
driven safety work.

### Added

- **Persistent workflow state** (sprints `0.3-S2a`/`S2b`/`S3`/`S6`).
  Crash-resumable runs via SQLite-backed checkpoint store with
  `BEGIN IMMEDIATE` atomicity, `f_FULLFSYNC` on macOS for durability,
  and a `--data-dir` flag on `boruna workflow run` / `resume`.
- **Approval-gate operator UX** (sprint `0.3-S2c`). New
  `kind: "approval_gate"` step type pauses the run; operators advance
  via `boruna workflow approve <run-id> <step-id>` /
  `boruna workflow reject` with optional reason. Decisions persisted
  in run metadata.
- **Concurrent step execution within waves** (sprint `0.3-S4`).
  `--concurrency N` on `run` / `resume` parallelizes steps at the
  same DAG topological level. Determinism preserved: same
  `output_hash` regardless of concurrency.
- **Step retry policies** (sprint `0.3-S5`). Configurable per-step
  retry with exponential backoff (100ms × 2^N capped at 5s) for
  transient failures.
- **Idempotent invocation** (sprints `0.3-S7` + `0.3-S10`).
  `--skip-if-running` flag for cron-driven scheduling. Atomic
  skip-if-in-flight check + insert in a single transaction closes
  the prior race window.
- **LLM handler decision: Bring Your Own Handler** (sprint `0.3-S8`).
  No default LLM handler ships in core; integrators wire their
  provider via the `CapabilityHandler` trait. Reference OpenAI
  handler + integration contract in `docs/guides/llm-integration.md`.
- **Workflow versioning for CI/CD safety** (sprint `0.3-S9`).
  `--expect-workflow-hash` flag refuses runs / resumes when the
  on-disk definition's hash doesn't match.
- **Per-step `attempt_count` column** (sprints `0.3-S11`/`S12`/`S13`)
  with the project's first schema migration (v1→v2) via
  `column_exists` + `if v < N` pattern. `boruna workflow show`
  surfaces the column. Sequential failure path persists actual count.
- **Workflow step output piping via `step_input` builtin** (sprint
  `0.3-S14`). `let received: String = step_input("name")` returns the
  JSON-encoded upstream output. New `Capability::StepInput` (id=10).
  Both sequential and concurrent paths resolve inputs coordinator-
  side. Unknown input names error with the declared list (review-
  driven).
- **Async step execution via external trigger CLI** (sprint
  `0.3-S15`). New `external_trigger` step kind for webhook-driven
  workflows. `boruna workflow trigger <run-id> <step-id> --token <X>
  --payload <json>` records the payload as the step's output value.
  32-hex-char tokens from `/dev/urandom` (no fallback) prevent
  accidental cross-step triggers. Constant-time validation; webhook-
  replay rejected by `StepAlreadyTriggered`. Boruna stays a CLI tool
  — no in-binary HTTP server.
- **Real HTTP handler with SSRF protection** (added Feb 2026).
  Feature-gated `http` builds enable real network calls via `--live`.
  `NetPolicy` allowed_domains / methods / byte limits / timeout.
  Rejects private IPs, localhost, non-http schemes.
- 23 new typed errors covering approval-gate, trigger-gate, run-not-
  resumable, step-not-found, hash-mismatch, and CAS-budget-exhausted
  states.

### Fixed

- **Trigger-flow TOCTOU race** (sprint `0.3-S16`). The 0.3-S15 trigger
  flow split metadata writes (CAS) and step-checkpoint transitions
  (resume sentinel pass) across two separate SQL transactions. A
  concurrent `boruna workflow resume` calling
  `mark_step_running_clearing_output` between the trigger function's
  metadata-CAS and the next resume's sentinel pass could leave the
  payload silently logged-and-discarded. Fixed by wrapping the
  metadata CAS and the checkpoint transition in a single
  `BEGIN IMMEDIATE` SQL transaction (new
  `RunCheckpointStore::commit_external_trigger`). SQLite's
  write-locked transaction blocks concurrent writers, making the
  checkpoint state read inside the transaction authoritative.
- New `TriggerCommitOutcome` enum (`Committed | MetadataChanged |
  CheckpointStateMismatch { current_status }`) for callers that need
  to distinguish CAS-retry-eligible races from operator-error states.
- Resume sentinel pass remains in place as a defensive recovery for
  legacy 0.3-S15-format DBs (metadata.triggers populated with non-empty
  payload but checkpoint still in `awaiting_external_event`). New
  forward-compat test confirms the upgrade path.
- 5 new persistence-layer unit tests + 3 new runner-level integration
  tests cover the atomic-commit outcomes and the legacy upgrade
  scenario.

### Added

- **Async step execution via external trigger CLI** (sprint `0.3-S15`).
  New `external_trigger` step kind for webhook-driven workflows. The
  runner pauses at the gate; an operator (or webhook receiver) advances
  it with `boruna workflow trigger <run-id> <step-id> --token <X>
  --payload <json>`, and the payload becomes the step's output value
  (visible to downstream steps via `step_input`).
  ```json
  "webhook": {
    "kind": "external_trigger",
    "description": "Stripe payment.succeeded webhook",
    "depends_on": ["init"]
  }
  ```
  Pause-time prints a 32-hex-char trigger token (16 bytes from
  `/dev/urandom`); the CLI rejects mismatching tokens to prevent
  accidental cross-step triggers from a misrouted webhook. Boruna stays
  a CLI tool — no in-binary HTTP server. The operator's webhook
  receiver bridges to the CLI.
- New `StepKind::ExternalTrigger { description }` variant on workflow
  step definitions; new `StepStatus::AwaitingExternalEvent` (persisted
  as `"awaiting_external_event"`).
- Public entry `boruna_orchestrator::workflow::record_external_trigger`
  for programmatic embedders. Validates the run/step/state, validates
  the operator-supplied token in constant time, refuses replays of
  already-triggered steps (`StepAlreadyTriggered { prior_triggered_at_ms }`),
  and writes the payload via compare-and-swap.
- Resume sentinel pass advances paused trigger steps when a payload is
  recorded (mirrors the approval-decision pattern from sprint `0.3-S2c`).
  The payload is stored as `Value::String(payload)`; the audit hash
  chain captures the synthesized `output_hash`.
- Five new typed errors: `NotAnExternalTriggerStep`,
  `StepNotAtExternalTriggerGate`, `InvalidTriggerToken`,
  `StepAlreadyTriggered`, plus an empty-payload `Validation` guard.
- **Ephemeral runs reject external_trigger steps upfront**
  (review-driven). `WorkflowRunner::run` (no persistence) refuses
  workflows that contain trigger steps with a typed `Validation` error
  — earlier draft caught this at step-entry time, which silently
  allowed prior steps to execute before the typed error surfaced.
- **Trigger token reuse across resume** (review-driven). The token is
  acquired via `acquire_trigger_token`: if a previously-persisted
  token exists for the step, it's returned verbatim. Earlier draft
  generated a fresh token on every pause entry while
  persist-trigger-token's "leave existing" branch kept the original;
  the printed value would silently disconnect from the validated
  value, and operators copying the just-printed token would get
  `InvalidTriggerToken`.
- **No fallback for entropy failure** (review-driven). If
  `/dev/urandom` cannot be read, `generate_trigger_token` returns
  `Err`. Earlier draft fell back to a `SystemTime + pid + counter`
  hash, which gave low-entropy observer-predictable tokens silently.
- **Workflow step output piping via `step_input`** (sprint `0.3-S14`).
  New built-in function in `.ax`:
  ```
  let received: String = step_input("msg")
  ```
  Returns the JSON-encoded upstream output for the named input
  (declared in `workflow.json`'s `inputs: { msg: "upstream.result" }`).
  Steps that need typed access parse the JSON inline. Determinism
  preserved: same inputs → same per-step `output_hash` regardless
  of concurrency level.
- New `Capability::StepInput` (id=10, name="step.input", version="1").
  **Bumps `capability_set_hash`** — additive surface change.
  Integrators using the prior hash for cache keys MUST invalidate.
  Old: `sha256:b0ca1793...`. New: `sha256:980d017d...`.
- Compiler treats `step_input(name)` as a builtin (typeck arity 1;
  codegen emits `Op::CapCall(StepInput, 1)`). Auto-adds
  `Capability::StepInput` to the calling function's capability set so
  the VM's runtime function-cap check passes.
- New `boruna_vm::capability_gateway::StepInputHandler` — wraps an
  inner handler and intercepts `step.input` calls. Composes with both
  `MockHandler` and BYOH live handlers (sprint `0.3-S8`).
- `WorkflowRunner::build_step_policy` auto-allows `step.input` when
  the operator's policy is silent on it. `entry().or_insert()`
  preserves explicit denies for hardened workflows.
- Both sequential and concurrent execution paths resolve inputs
  coordinator-side and pass the snapshot to workers — workers hold
  no DataStore reference.
- **Unknown input names error** (review-driven, project-conventions
  §1). `step_input("undeclared_name")` returns a typed runtime error
  with the declared list for triage, instead of silently returning
  empty data and corrupting downstream output.

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

[Unreleased]: https://github.com/escapeboy/boruna/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/escapeboy/boruna/releases/tag/v0.5.0
[0.4.0]: https://github.com/escapeboy/boruna/releases/tag/v0.4.0
[0.3.0]: https://github.com/escapeboy/boruna/releases/tag/v0.3.0
[0.2.0]: https://github.com/escapeboy/boruna/releases/tag/v0.2.0
[0.1.0]: https://github.com/escapeboy/boruna/releases/tag/v0.1.0
