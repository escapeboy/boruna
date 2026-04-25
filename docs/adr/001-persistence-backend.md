# ADR 001: Persistence Backend

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-04-25 |
| **Sprint** | `0.3-S1` (unblocks `0.3-S2` through `0.3-S9`) |
| **Deciders** | Boruna maintainers (driven by FleetQ implementer feedback) |
| **Supersedes** | — |

## Context

Boruna's workflow runner today persists **nothing across process restarts**. `WorkflowRunner::run` creates a `tempfile::tempdir()` for the run's `DataStore`, executes steps in topological order, then drops the directory when the process exits. If the host kills the process — crash, OOM, deploy, scheduled reboot — the in-flight workflow state is gone. This is documented in [`docs/limitations.md`](../limitations.md):

> **No persistent state across restarts.** If the `boruna workflow run` process exits (crash, kill, timeout), the workflow run cannot be resumed. Checkpoint-and-resume is on the roadmap for 0.3.0.

Sprint `0.3-S2` (Persistent workflow state — MVP) needs to write a checkpoint after each step and let `boruna workflow resume <run-id>` pick up from the last successful checkpoint. Subsequent sprints (`0.3-S3` through `0.3-S9` — async steps, retries, schedules, versioning, etc.) all build on top of that checkpoint store. **The persistence backend choice gates every other 0.3.0 sprint.**

There is also an existing JSON-file storage layer at `orchestrator/src/storage/mod.rs` (`Store` — graphs, locks, gates, bundles), but it serves the multi-agent orchestration engine (`engine::WorkGraph`), not the workflow runner. Whether to unify the two is **explicitly out of scope** for this ADR; see [Open questions](#open-questions).

### Constraints (non-negotiable)

1. **Single-binary distribution.** Boruna ships `boruna-X.Y.Z-<target>.tar.gz` static binaries for `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, and `aarch64-apple-darwin`. FleetQ extracts these into `php-fpm-alpine` containers. Anything that requires a separately-installed daemon (postgres, mysql, even a sidecar process) breaks the install story.
2. **Determinism.** Boruna's whole value proposition is reproducible execution + replay. The persistence layer must not introduce nondeterminism via wall-clock-keyed indexes, autoincrement PKs that depend on insertion order, or anything else that diverges between original run and replay.
3. **Embeddable inside other processes.** FleetQ runs Boruna binaries per-script; some integrators will eventually link the orchestrator as a library. The backend cannot require global state, network ports, or root permissions.
4. **Local-first by default.** Boruna's primary deployment is local execution (a single binary on a developer's laptop, a CI runner, or a VPS). Multi-tenant SaaS-scale storage is a future variant, not the default.

### Decision criteria (weighted)

| Weight | Criterion |
|---|---|
| **Critical** | Works in a single static binary (musl + macOS arm64) |
| **Critical** | Survives crashes (durable writes, transactional checkpoints) |
| **Critical** | Doesn't introduce nondeterminism into Boruna's deterministic guarantee |
| High | Operationally trivial (no separate daemon, no schema migration ceremony for v1) |
| High | Inspectable with off-the-shelf tooling (CLI, GUI) when debugging a stuck run |
| High | Cargo build-time impact on the existing 9-crate workspace |
| Medium | Scales to ~10⁵ runs per database file before splitting becomes necessary |
| Medium | Concurrent reader story (CLI inspection during a run) |
| Low | Multi-writer (one orchestrator process is the only writer; no real need yet) |

## Decision

**SQLite, embedded via `rusqlite` with the `bundled` feature.** No persistence-backend abstraction layer in v1.

```toml
# orchestrator/Cargo.toml (when 0.3-S2 lands — not in this ADR)
[features]
default = ["persist-sqlite"]
persist-sqlite = ["dep:rusqlite"]

[dependencies]
rusqlite = { version = "0.32", features = ["bundled"], optional = true }
```

The feature gate keeps the SQLite C source out of the **`boruna-mcp`** and **`boruna-pkg`** binaries, which don't need persistence — only `boruna` (CLI) and `boruna-orch` link the orchestrator with `persist-sqlite` enabled. Saves ~1.5 MB × 2 unrelated binaries in the release tarball.

Concretely:

- One database file per Boruna data directory: `<data-dir>/runs.db`. The data directory is configurable via `--data-dir` (default: `./.boruna/`).
- WAL journal mode (`PRAGMA journal_mode = WAL`) — concurrent reads while the orchestrator writes. WAL files (`runs.db-wal`, `runs.db-shm`) live alongside the main db.
- Schema is hand-rolled SQL migrations in code (`include_str!` + version table). No migration framework dependency in v1.
- All checkpoint writes happen inside a single `BEGIN IMMEDIATE / COMMIT` transaction so partial-step failures roll back cleanly.
- Writer model is **serial-by-process-lifecycle**, not "single process" — see [Writer serialization model](#writer-serialization-model) below.

**No abstraction trait.** `0.3-S2` writes against `rusqlite::Connection` directly. A `PersistenceBackend` trait gets extracted **only** when a second backend (e.g. postgres for SaaS) is genuinely needed — likely the SaaS workstream after `0.5.0`. Premature abstraction over a single implementation is rejected (see [Alternatives considered → 5](#5-pluggable-trait-abstraction-from-day-one)).

### Determinism contract for persisted state

The replay engine guarantees that re-executing a recorded run reproduces the same `EventLog` — same capability calls in the same order producing the same outputs. The persistence layer must not weaken this. The contract:

- **Replay-verified state** (participates in any audit hash, replay comparison, or evidence bundle): `step_checkpoints.output_json`, `step_checkpoints.output_hash`, `step_checkpoints.status` (terminal values only — `completed`, `failed`).
- **Operational metadata** (recorded for human/CLI observability, **never** fed into a hash, **never** compared during replay): `runs.started_at`, `runs.updated_at`, `step_checkpoints.started_at`, `step_checkpoints.ended_at`, transient `status` values like `running`/`pending`.
- **`run_id`** is derived from the workflow name + a deterministic counter or content hash, **not** from `chrono::Utc::now()`. The current `runner.rs:49-53` formula (`run-{name}-{utc now}`) violates this and must change in `0.3-S2`.

Reviewers of `0.3-S2` and downstream sprints must reject any code path that:
- Feeds a timestamp column into a SHA-256 chain or evidence bundle
- Uses `ORDER BY started_at` instead of `ORDER BY (run_id, step_id)` for replay-relevant queries
- Cargo-cults `chrono::Utc::now()` from existing code without checking whether the result lands in replay-verified state

### Writer serialization model

SQLite WAL allows one writer at a time. The orchestrator's process model already serializes writers naturally — but it's worth making explicit because the approval-gate flow (and future scheduler daemon) make this less obvious than "one orchestrator per data dir":

- `boruna workflow run` writes checkpoints, exits at end-of-run or at an approval gate.
- `boruna workflow approve <run-id> <step-id>` writes the approval result, exits.
- `boruna workflow resume <run-id>` writes subsequent checkpoints, exits.

These are **disjoint in time** by process lifecycle — never concurrent. A future `boruna scheduler run` daemon (`0.3-S7`) running alongside an interactive `approve` invocation **does** introduce concurrent-writer risk. Policy:

- All writes use `BEGIN IMMEDIATE` so the transaction acquires the writer lock up-front (not at first write).
- On `SQLITE_BUSY`, retry with exponential backoff: 10 ms, 50 ms, 250 ms, 1.25 s, then fail with `error_kind: "persistence_busy"`. Documented user-facing — long-running writers are an operational issue, not a silent data-loss surface.
- If concurrent writers become a frequent operational pain (rather than a rare collision), that is the trigger to extract the persistence trait and add a postgres backend — same trigger as the SaaS variant.

## Consequences

### Positive

- **Single-binary story preserved.** `rusqlite`'s `bundled` feature compiles SQLite from source as part of the crate, statically linking the C code into the Rust binary. No `libsqlite3.so` runtime dependency, no Alpine `apk add sqlite-dev` step, no separate daemon. Static-musl release binaries continue to work as drop-in installs.
- **Crash safety for free.** SQLite has been the durability gold standard for embedded use for two decades. WAL mode + transactional checkpoints means a SIGKILL mid-step rolls back to the last committed checkpoint without corruption.
- **Operationally trivial.** `sqlite3 .boruna/runs.db ".tables"` works on every laptop, in every CI runner, in every Alpine container. Any DBA, integrator, or curious user can inspect a stuck run with tools they already have. The DB file can be `tar`'d and emailed for support tickets.
- **Replay-compatible.** SQLite produces deterministic ordering when queries use explicit `ORDER BY`. We avoid implicit `rowid` ordering and timestamp-keyed indexes in the schema.
- **No new external dependencies.** `rusqlite` is one new crate (and its native SQLite source). No tokio-runtime adapter required for synchronous use.
- **Aligned with FleetQ's deployment model.** They already pin our binaries by SHA256 and extract into Alpine containers. A SQLite file alongside the binary fits that pattern; a postgres dependency would force them to provision and manage a database.

### Negative

- **Binary size increases.** Bundled SQLite C source compiles to ~1.5–2.5 MB of additional code per target. For comparison, our v0.2.0 musl binary is ~10 MB; the post-0.3.0 binary will land around 12–13 MB. Not a free lunch — but well within the budget for a single-binary deploy story. Validated empirically in [Validation](#validation) below.
- **Native code in the build.** `rusqlite/bundled` requires a C compiler (`cc`) at build time. Already true for our macOS arm64 release builds. The musl runner already has `gcc-musl` for other crates' C deps; verify after first 0.3.x release attempt.
- **Schema migrations are our problem.** No magic: when the schema changes between 0.3.0 and 0.4.0, we ship hand-written SQL migration scripts and a tiny runner. v1 ships with one schema and a `schema_version = 1` row; future bumps are explicit.
- **Single-writer constraint.** SQLite WAL allows one writer at a time. For one orchestrator process per data directory this is sufficient. **If a future SaaS variant needs multiple orchestrator processes against shared state, that's the trigger for the postgres backend** — and the trigger to extract the abstraction.
- **No built-in replication.** Backups are file-copies. For most use cases this is fine; for compliance-sensitive deployments where evidence bundles already cover audit, the `runs.db` is operational state, not the audit trail.

### Neutral / accepted

- **Multi-tenant isolation will need careful schema design later.** v1 has no concept of tenant; runs are scoped per data directory. When SaaS arrives, we either give each tenant their own SQLite file or migrate to postgres with a `tenant_id` column.
- **The legacy `orchestrator::storage::Store` (JSON files) is not migrated by this ADR.** It serves a different consumer (multi-agent engine) and uses different shapes. Folding it into SQLite is a separate sprint when there's a reason to.

## Alternatives considered

### 1. Postgres (or any external SQL daemon)

**Pros:** Battle-tested at scale, mature replication, multi-writer, real connection pooling.
**Cons:** Breaks the single-binary install story. Forces every Boruna user — including the developer trying it out for the first time, and FleetQ's per-script binary invocations — to provision and manage a database server. Boruna's primary mode is *local-first*; postgres optimizes for the SaaS variant we explicitly do not need yet.
**Verdict:** Rejected for v1. Re-evaluate when SaaS deployment is the primary use case.

### 2. Pure-Rust embedded KV store (`sled`, `redb`, `fjall`)

**Pros:** Smaller binary delta. No C dependency. Pure-Rust supply chain.
**Cons:**
- `sled` has been "1.0 beta" for years and the maintainer has publicly stated [the project is on hold](https://github.com/spacejam/sled/issues/1390); not a safe long-term bet for a load-bearing dependency.
- `redb` is well-engineered but young; production users are sparse.
- All KV stores force us to hand-build secondary indexes, pagination, joins for queries like "list runs that hit step X in the last 24 hours." We'd be reinventing a sliver of SQL with worse tooling.
- No DBA in the world has `sled` debugging instincts; SQLite is the lingua franca.
**Verdict:** Rejected. The off-the-shelf-tooling and maturity advantages of SQLite outweigh the smaller binary footprint.

### 3. Continue with JSON files (extend existing `orchestrator::storage::Store`)

**Pros:** Already in the codebase. Zero new dependencies. Trivial to reason about.
**Cons:**
- No transactions: a checkpoint write that fans out across multiple files (graph state + lock state + step output) cannot atomically succeed-or-fail. Crash mid-write leaves inconsistent state.
- No concurrent-reader story: a CLI process listing runs while the orchestrator writes risks reading half-flushed JSON.
- File proliferation: a workflow run with 50 steps, 3 retries each, becomes hundreds of small files. Fine at toy scale, ugly at real scale.
- Manual schema versioning: every file shape needs its own `schema_version` field handled by every reader.
- Querying "all runs in state X" means `read_dir + filter` — O(all-runs) for every list operation.
**Verdict:** Rejected. Acceptable for the multi-agent engine's existing usage (low-volume, mostly write-once), but inadequate for high-frequency checkpoint writes during workflow execution.

### 4. SQLite via dynamic linking (`rusqlite` without `bundled`)

**Pros:** Smaller binary. System SQLite gets security updates from the distro.
**Cons:** Forces every Boruna deployment to ship `libsqlite3` at the right ABI version. Alpine ships SQLite ABI versions that don't always match what `rusqlite` was built against. We'd be back to dynamic-library-hunt-and-peck — exactly what musl static linking was supposed to eliminate.
**Verdict:** Rejected. The whole point of musl static binaries is portability; reintroducing a runtime ABI dependency throws that away.

### 5. Pluggable trait abstraction from day one

**Pros:** Future-proof; postgres can slot in without refactor. Cleaner test seams (mock backend).
**Cons:**
- Premature: we have one backend today, not two. Trait shapes designed against a single implementation almost always need rework when the second arrives — the abstraction encodes assumptions of the first impl, not the union of both. Better to write SQLite-direct code first, then extract the trait when postgres is concrete enough to inform the boundary.
- Adds indirection cost (vtable, generic-over-backend, error type harmonization) for zero current benefit.
- Test-seam argument is weak: SQLite in-memory (`:memory:`) is trivially fast for unit tests; we don't need mock backends.
- Violates [project CLAUDE.md "Code Discipline"](../../CLAUDE.md): "Don't design for hypothetical future requirements. Three similar lines is better than a premature abstraction."
**Verdict:** Rejected for v1. Extract the trait when the second backend (postgres for SaaS) is a real ask, not a hypothetical one. Every consumer of the persistence layer in `0.3-S2` through `0.3-S9` will land against `rusqlite::Connection` directly; abstracting later is a mechanical refactor, abstracting now risks designing the wrong trait.

### 6. Cloud-native: S3 + DynamoDB or similar

**Pros:** Infinite scale; managed; backups built-in.
**Cons:** Same as postgres but worse: requires AWS credentials at runtime, requires network access, makes local development a nightmare, and pegs Boruna to one cloud provider's API surface.
**Verdict:** Rejected. Boruna is local-first. Cloud-state is an integrator concern, not a runtime concern.

## Validation

### Done in this sprint (host build, macOS)

Built a throwaway probe at `/tmp/rusqlite-musl-probe/` depending on `rusqlite = { version = "0.32", features = ["bundled"] }` with one `Connection::open_in_memory()` round-trip. Results on `x86_64-apple-darwin`:

- Build succeeded in 31 s clean. Pulled in `libsqlite3-sys`, `cc`, `hashbrown`, `hashlink`, `ahash`, `smallvec`, `fallible-iterator` as transitive deps. No system dependencies hunted.
- `otool -L probe` showed only `/usr/lib/libSystem.B.dylib` — **no `libsqlite3` dynamic link.** SQLite is statically embedded as expected.
- Release binary: **1.2 MB total** (probe code + rusqlite + bundled SQLite + transitive crates, default release profile). For a from-zero project that's the upper bound; the delta added on top of the existing `boruna-orch` binary will be smaller (transitive deps already present).

This validates the bundled-link approach in principle.

### Deferred to first `0.3.x` release attempt (musl cross-compile)

The host build proves `bundled` works; it does **not** prove `cross build --target x86_64-unknown-linux-musl` succeeds. The release pipeline (`.github/workflows/release.yml:71-93`) uses `cargo install cross` and `cross build`, which runs the build inside a Docker image with its own sysroot. Two known integration risks:

1. **`rusqlite/bundled` invokes `cc-rs`** which respects `CC_<target>` env vars set by `cross`'s default image. Default `cross` images for `aarch64-unknown-linux-musl` have historically had gaps in the C cross-toolchain when bundled SQLite triggers `-fPIE`/`-fstack-protector` defaults.
2. The bundled SQLite C source compiles ~150 K lines of C; build time per target jumps by ~30–60 s.

**If musl cross-compile fails:** the fix is a custom `Cross.toml` with a pinned image (e.g. `ghcr.io/cross-rs/aarch64-unknown-linux-musl:edge` plus a `pre-build` step that installs `gcc-musl-dev`). It is **not** a backend-swap trigger. Backend swap is reserved for a fundamental incompatibility (e.g. SQLite's runtime sqlite3_threadsafe assumptions break under musl's threading model — currently no evidence of this).

**Validation execution order in `0.3-S2`:**

1. Add `rusqlite` dep to `orchestrator/Cargo.toml` behind `persist-sqlite` feature.
2. Push to a throwaway branch; trigger CI to verify clippy + tests pass on the self-hosted runner.
3. Manually trigger the release workflow against the throwaway tag to verify musl + macOS arm64 builds. **If a target fails, fix the `Cross.toml` before merging the schema work.**
4. Measure binary-size delta on each target; record in the `0.3-S2` PR description.

### Inside `0.3-S2`

- **WAL mode works under single-orchestrator concurrent-reader use** — spike: open two `Connection`s, one writes a checkpoint, the other reads concurrently; expect reader to see either the pre- or post-state, never a partial write.
- **`BEGIN IMMEDIATE` retry policy survives a contrived two-writer race** — spike: spawn two threads, both attempt `BEGIN IMMEDIATE` simultaneously, verify the loser retries with backoff and eventually wins (or fails cleanly with `persistence_busy`).
- **`PRAGMA foreign_keys = ON` + `ON DELETE CASCADE` actually cascades** — trivial test; SQLite default is OFF, so this PRAGMA is mandatory on every `Connection` open. Easy to forget.

A failure of the **musl cross-compile** is the only finding from this validation list that triggers re-evaluation of the backend choice. Failures of the WAL or PRAGMA spikes inside S2 trigger schema or pragma changes, not a backend swap.

## Open questions

- **Legacy `orchestrator::storage::Store` (JSON files) consolidation.** The two stores describe different domain objects (multi-agent `WorkGraph` vs. `WorkflowRun`) and never need to be queried together — until `0.4-S7` (workflow dashboard backend), which will want a unified `list_runs` view across both. **That sprint is the explicit consolidation trigger.** Until then, both stores coexist with no required interop, and the legacy JSON store keeps its filesystem-mtime-based ordering quirk (`storage/mod.rs::latest_graph()` uses `metadata.modified()` — also a determinism smell, not addressed here). When `0.4-S7` arrives, decide between (a) migrating the JSON store into SQLite under a `work_graphs` table, or (b) building a read-side view layer that abstracts over both. Don't pre-commit.
- **Where does the database file live?** `--data-dir` defaulting to `./.boruna/` follows the local-first convention. SaaS variant will need an env-var override and probably a per-tenant subdirectory. Defer until SaaS.
- **Backup tooling.** `boruna data backup <path>` is plausible as a follow-up sprint; for v1 users can `cp .boruna/runs.db .boruna/runs.db.bak` while the orchestrator is stopped, or use SQLite's `.backup` command online.
- **Encryption at rest.** Not in scope. Compliance-sensitive deployments use disk-level encryption (LUKS, FileVault, EBS encryption) today; we will revisit when there's a customer ask.

## Implementation notes for `0.3-S2`

These are **non-binding guidance** for the next sprint, captured here so the implementer doesn't have to rediscover the gotchas this ADR already identified. The schema sketch is illustrative; refine it in the S2 PR.

### Sizing flag

The ADR sprint plan (`plans/sprints-to-v1.md:44`) sizes `0.3-S2` as **L (1 week)**. After this ADR's review surfaced the determinism contract, the writer-serialization model, the foreign-keys PRAGMA, the schema indexes, and the cross-compile validation step, the realistic scope is **L+ to 2× L**. Concretely the sprint must deliver: schema + migration runner + `Connection` lifecycle + `RunCheckpointStore` API + `rusqlite::Error → WorkflowRunError` mapping (currently 4 plain variants in `runner.rs`, no `Database(_)` variant) + replacing `tempfile::tempdir()` in `WorkflowRunner::run` + `--data-dir` plumbing through CLI + `boruna workflow resume` end-to-end + the three validation spikes from the [Validation](#inside-03-s2) section + tests including a SIGKILL-mid-step crash-recovery test. **The sprint plan should re-size 0.3-S2 to 2 weeks before starting**, and the maintainer should consider splitting it: schema + checkpoint write in one PR, `resume` subcommand + crash-recovery test in a follow-up.

### Steps

1. **Cargo:** add `rusqlite = { version = "0.32", features = ["bundled"], optional = true }` to `orchestrator/Cargo.toml`. Wire the `persist-sqlite` feature (default-enabled in `orchestrator`, **off** in `boruna-mcp` and `boruna-pkg` to keep their binaries lean).
2. **Module:** `orchestrator/src/persistence/mod.rs` with one public type, `RunCheckpointStore`, owning a `rusqlite::Connection`. **Do not** introduce a `PersistenceBackend` trait. Direct concrete dependency.
3. **Connection PRAGMAs (all four mandatory on every open):**
   - `PRAGMA journal_mode = WAL;`
   - `PRAGMA synchronous = NORMAL;` — SAFE under WAL: durable to commit, may lose last-uncommitted on power loss
   - `PRAGMA foreign_keys = ON;` — **default is OFF.** Without this, `ON DELETE CASCADE` is silently inert and orphan rows accumulate forever. High-confidence gotcha; cover with a unit test that inserts a parent + child, deletes parent, asserts child gone.
   - `PRAGMA busy_timeout = 5000;` — combined with `BEGIN IMMEDIATE` retry policy below
4. **Writer policy:** wrap each step's checkpoint write (status update + output insertion) in `BEGIN IMMEDIATE; ...; COMMIT;`. On `SQLITE_BUSY`, retry with exponential backoff (10 ms, 50 ms, 250 ms, 1.25 s) before failing with a typed `error_kind: "persistence_busy"`.
5. **Schema sketch (illustrative — refine in S2):**
   ```sql
   CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
   INSERT INTO schema_version VALUES (1);

   CREATE TABLE runs (
     run_id        TEXT PRIMARY KEY,           -- DETERMINISTIC: derive from
                                               --   sha256(workflow_hash + input_hash + counter)
                                               -- NOT from chrono::Utc::now()
     workflow_name TEXT NOT NULL,
     workflow_hash TEXT NOT NULL,              -- sha256 of the workflow.json
     status        TEXT NOT NULL,              -- 'running' | 'paused' | 'completed' | 'failed'
     started_at    INTEGER NOT NULL,           -- unix ms — OPERATIONAL ONLY,
                                               --   never feeds an audit hash or replay comparison
     updated_at    INTEGER NOT NULL,           -- OPERATIONAL ONLY
     policy_json   TEXT NOT NULL,
     metadata_json TEXT NOT NULL DEFAULT '{}'
   );

   CREATE TABLE step_checkpoints (
     run_id      TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
     step_id     TEXT NOT NULL,
     status      TEXT NOT NULL,                -- terminal values feed replay; transient ones do not
     output_json TEXT,                         -- step output value, JSON-encoded — REPLAY-VERIFIED
     output_hash TEXT,                         -- sha256(output_json) — REPLAY-VERIFIED
     started_at  INTEGER,                      -- OPERATIONAL ONLY
     ended_at    INTEGER,                      -- OPERATIONAL ONLY
     error_msg   TEXT,
     PRIMARY KEY (run_id, step_id)
   );

   CREATE INDEX idx_runs_status ON runs(status);

   -- Partial index for "what is blocked / running across all runs?" queries that
   -- the dashboard (0.4-S7) and the scheduler (0.3-S7) will need. Keeps the index
   -- small (most rows have terminal status and don't need a global view).
   CREATE INDEX idx_step_checkpoints_active
     ON step_checkpoints(status)
     WHERE status IN ('awaiting_approval', 'running');
   ```
6. **Replace** `tempfile::tempdir()` in `WorkflowRunner::run` with the data-directory-rooted `runs.db`. The `DataStore` (output values) becomes a SQLite-backed lookup over `step_checkpoints.output_json` for the `from: "step1.output.field"` resolver in `0.3-S9`.
7. **Add** `boruna workflow resume <run-id>` CLI subcommand that reads the run's checkpoint and continues from the next pending step. Must verify the on-disk `workflow.json` hash matches the row's `workflow_hash` before resuming — refuse to resume against a modified workflow definition (force user to start a new run, or supply `--allow-workflow-drift`).
8. **Replace** `runner.rs:49-53` `run_id` derivation. Current formula `format!("run-{name}-{utc now}")` violates the determinism contract. Replace with `format!("run-{name}-{}", &short_hash[..12])` where `short_hash = sha256(workflow_hash + serialized_inputs + run_counter)`. The counter is the count of existing rows in `runs` with the same workflow_hash, queried at submission time.

## References

- FleetQ implementer feedback letter (2026-04-25) — drove `0.3-S10` resource limits and `0.3-S11` capability hash; persistence is the foundation that lets future asks like async-on-webhook (`0.3-S4`) work at all.
- [`docs/limitations.md`](../limitations.md) — explicit promise to ship persistence in 0.3.0.
- [`plans/sprints-to-v1.md`](../../plans/sprints-to-v1.md) — sprint dependency graph; `0.3-S1 → 0.3-S2 → 0.3-S3 → ...` is the longest chain in the 0.3.0 milestone.
- [`orchestrator/src/workflow/runner.rs`](../../orchestrator/src/workflow/runner.rs) — current `tempfile::tempdir()`-based runner (the code being replaced).
- [`orchestrator/src/storage/mod.rs`](../../orchestrator/src/storage/mod.rs) — pre-existing JSON file store for the multi-agent engine (not changed by this ADR).
- [`docs/concepts/determinism.md`](../concepts/determinism.md) — determinism contract this ADR must not violate.
- [rusqlite crate](https://docs.rs/rusqlite) — chosen Rust binding for SQLite.
- [SQLite WAL mode docs](https://www.sqlite.org/wal.html) — single-writer, concurrent-readers semantics.
