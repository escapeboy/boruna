# Sprint Retro — 2026-04-27 (multi-sprint: 0.5-S3 + 0.5-S5 + DX fmt)

**Sprints:** `0.5-S3` (auth), `0.5-S5` (distributed retry), DX (`boruna fmt`)
**Pipeline:** `/sprint-orchestrate full` with parallel worktree agents
**Outcome:** All three landed on `master` at `ede923b`, pushed to origin
**Driver:** First multi-sprint parallel session. Three independent
sprints attempted simultaneously: one driven sequentially by me
(auth), two delegated to background worktree agents (retry + fmt).

## What shipped

### 0.5-S3 — shared-secret bearer authentication
- `--shared-secret <hex>` flag on `coordinator serve` and
  `worker run` (env-var fallback `BORUNA_COORD_SECRET`).
- Axum `auth_middleware` with constant-time compare. 401 +
  `coord.unauthorized` on mismatch.
- Worker sends `Authorization: Bearer` on every HTTP call.
- 4 new CLI integration tests.
- Loopback-default no-auth posture preserved for backwards
  compatibility (loud stderr warning when bound non-loopback
  without secret).

### 0.5-S5 — distributed retry policies
- New `RunCheckpointStore::requeue_failed_step_for_retry`
  (race-safe persistence primitive, `BEGIN IMMEDIATE` envelope).
- `WorkflowRunner::advance_run_one_tick` retry pass — re-queues
  Failed steps with budget remaining and retry-eligible class.
- `AdvanceResult.newly_requeued` field (additive).
- `coordinator wait` driver prints `requeued (retry)` line.
- 14 new orchestrator unit tests.
- Resolves the "distributed retry not honored" limitation from
  0.5-S2f.

### DX — `boruna fmt` auto-formatter
- New `tooling/src/format/` module with AST-walking pretty-
  printer.
- `boruna fmt <file>` (in-place) and `boruna fmt --check <file>`
  (CI gate, exit 0/1/2).
- 5 unit tests + 6 CLI integration tests.
- Known v1 limitation: comments are stripped (lexer drops them
  before AST).

## What worked

1. **Strong test gates absorbed the parallelism failure.**
   The combined commit had 1961 lines from three separate
   concerns + agent contamination of my main worktree, but
   `cargo test --workspace`, `cargo clippy -D warnings`, and
   `cargo fmt --check` ALL passed cleanly. Without these
   gates, the contamination would have shipped silent bugs.

2. **The `prepare_persistent_run` and the wait-driver factor
   from 0.5-S2f scaled.** The retry sprint extended
   `advance_run_one_tick` with a retry pass at the START of
   the function (before the ready-set computation). No
   refactor of the wait driver's overall shape; just a new
   pre-pass.

3. **Race-safe persistence primitives are reusable.** The
   `insert_pending_step_if_absent` pattern from 0.5-S2f was
   the template for `requeue_failed_step_for_retry`. Both
   use `BEGIN IMMEDIATE` + status check inside tx + idempotent
   on conflict. The convention §13 + §14 combo is now a
   well-trodden path.

4. **The 14 retry unit tests included adversarial-coverage
   cases by design.** Per the agent's prompt, the tests
   covered budget exhaustion, single-attempt rejection,
   error-class matching (retry_on vs. on_transient fallback),
   and concurrent-wait race idempotency. This is the
   convention §29 pattern internalized — the agent treated
   adversarial review as part of the sprint scope, not a
   separate phase.

## What didn't work

### Parallel worktree isolation failure (the headline)

The Agent tool's `isolation: "worktree"` parameter creates a
git worktree under `.claude/worktrees/agent-<id>/`. Both
sub-agents I launched used `cwd: <agent-worktree-path>`, but
both ALSO accepted absolute paths in their tool calls (Edit,
Write, Read).

My agent prompts contained absolute-path references in the
"Reading order" sections (e.g.
`/Users/katsarov/htdocs/ai-lang/orchestrator/src/...`). The
agents used those absolute paths verbatim in their Edit calls.
Result: edits landed in the **main worktree** (where I was
working on auth), not in their isolated agent worktrees.

Symptoms:
- `git worktree list` showed both agent worktrees as expected.
- Agent worktrees' files (e.g.
  `.claude/worktrees/agent-X/orchestrator/src/workflow/runner.rs`)
  were UNCHANGED from their starting commit.
- My main worktree's same file had the agents' edits applied.
- My ongoing auth edits to Cargo.toml and main.rs were
  intermingled with retry-sprint and fmt-sprint edits.
- Build broke mid-flight when one agent's incomplete edit
  left a struct-init site missing a field.

Recovery: stopped both agents via `SendMessage`, ran the full
test/clippy/fmt gates against the merged-into-main-worktree
state, fixed one trailing struct-init site, and committed
all three sprints as a single decomposed commit.

### Cost of the failure

- **Time:** the parallel approach was supposed to overlap
  ~30-60 minutes of agent work with my auth sequential work.
  The recovery overhead (stop agents, take stock, fix the
  build break, re-decompose for the commit message) ate
  most of the parallelism win.
- **History:** a single 1961-line commit with three
  decomposed concerns vs. three clean per-sprint commits.
  Acceptable but not ideal for `git blame` / cherry-pick.
- **Reviewability:** the combined commit is harder to review
  in isolation per sprint. The CHANGELOG decomposition
  partially compensates.

### Why it still mostly worked

- **Tests as the safety net.** All three sprints had test
  coverage in their prompts. The combined test suite (310
  orch + 21 cli_coord + 6 fmt CLI + 108 tooling + ...) passed
  end-to-end without my intervention beyond fixing one
  struct-init site and one fmt issue.
- **Clippy + fmt as syntactic gates.** Both stayed clean
  through the contamination because each sprint individually
  produced clean code; the merge didn't introduce
  cross-sprint stylistic conflicts.
- **The branch model accommodated the fail-mode.** Even
  though the agents wrote to my main worktree (the wrong
  place), they were writing to MY current branch
  (`feat/0.5-s3-auth`). When I committed, all three
  sprints' work landed on that branch. Merging to master
  via `--no-ff` preserved the as-shipped state.

## Lessons for future parallel-agent work

1. **Brief agents with RELATIVE PATHS only.** The Reading
   order, file footprint, and any path references in the
   prompt should be relative to the project root. Agents
   inherit `cwd = worktree-path`, so relative paths resolve
   correctly. Absolute paths bypass the worktree.

2. **Or: commit between agent waves.** If absolute paths are
   unavoidable (for grep/research), commit my work to a
   branch FIRST, then launch agents in worktrees off that
   branch. Their writes to my main worktree wouldn't matter
   because I'd be working on a separate branch and could
   reset cleanly.

3. **Agents need to verify their worktree placement.** A
   simple `pwd` + `git rev-parse --show-toplevel` at agent
   startup would catch the misplacement before edits begin.

4. **Test gates do most of the heavy lifting.** Even when
   parallelism is broken, strong test/clippy/fmt gates make
   the merge tractable. This was the main reason the
   reconciliation succeeded despite the contamination.

5. **Multi-sprint single-commit is the honest representation
   when the parallelism fails.** Trying to retroactively
   split the commit via `git add -p` would have been
   busywork. The CHANGELOG decomposition + single commit is
   accurate to what happened.

## Tests / gates

- `cargo test --workspace --features boruna-cli/serve` —
  all green. Test count delta: +14 orchestrator (retry),
  +4 cli_coordinator_worker (auth), +5 tooling unit (fmt),
  +6 cli_format integration (fmt).
- `cargo clippy --workspace --features boruna-cli/serve -- -D
  warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `cargo build --workspace --features boruna-cli/serve` —
  clean.

## Roadmap impact

Updated `docs/roadmap.md` at the start of the session to
reflect post-0.5-S2f reality. After this session:

- 0.5-S3 ✅ (auth) — production-deployment unblocker.
- 0.5-S5 ✅ (retry) — distributed-execution correctness.
- DX `boruna fmt` ✅ — first DX-lane item delivered.

Still outstanding for v1 (per the refreshed roadmap):
- 0.5-S4 — `workflow run --coordinator <url>` (one-shot CI
  command).
- 0.5-S6 — distributed approval-gate / external-trigger.
- 0.5-S7 — output blob references for large LLM outputs.
- Coordinator HA / failover.
- Worker capability tagging / placement.
- Versioned `.ax` language specification (spec-freeze gate).
- Versioned workflow DAG schema, evidence bundle format.
- Migration tooling beta.
- Streaming output from `boruna_run`.
- LLM provider registry.
- DX: `boruna new`, `boruna fmt` comment-preserving v2,
  `boruna run --watch`, evidence bundle diff.
- 1.0.0 commitment work: security audit, performance
  benchmarks, LTS commitment.

## Recommended next session

**Stop here for the day.** Eight sprints in this session
(six earlier in 0.5-S2 cycle + 0.5-S2f closure + this
multi-sprint trio) is well past the diminishing-returns line.

When continuing:
- Pick **0.5-S6 (distributed approval-gate)** if production
  deployment workflows need that today.
- Pick **0.5-S7 (output blob refs)** if LLM output sizes
  are hitting the 8 MiB limit.
- Pick **versioned `.ax` spec** if planning the spec freeze
  for 0.5.0 ship.

Each is a 1-2 session sprint. Don't try parallel worktrees
again until the path-resolution issue is fixed in the agent
runtime.

## Conventions reinforced

- §1 (reject at parse) — auth 401, retry RequeueOutcome,
  fmt FormatError.
- §2 (typed error_kind) — `coord.unauthorized` added.
- §6 (default-features = false) — clap "env" feature added
  deliberately.
- §13 + §14 combo — `requeue_failed_step_for_retry` follows
  the same envelope as `claim_step` /
  `insert_pending_step_if_absent`.
- §15 (replay-verified vs. operational) — `newly_requeued`
  is operational only.
- §29 (adversarial review pays for itself) — retry agent
  baked adversarial cases into the unit tests directly.

## Conventions added (provisional)

- **Brief agents with relative paths only when using
  worktree isolation.** Absolute paths bypass the worktree.
  This will be added to project-conventions if it survives
  the next parallel-agent attempt.

- **Strong gates compensate for tooling failures.** When a
  parallel-agent strategy fails, the recovery cost scales
  inversely with how comprehensive the test/clippy/fmt
  gates are. Don't skip gates to save time on a sprint
  pipeline; they're load-bearing for failure recovery too.
