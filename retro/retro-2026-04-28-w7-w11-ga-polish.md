# Retro: W7 → W11 — v1.0 GA polish cycle

**Cycle window:** 2026-04-28 (single-day push)
**Commits in cycle:** ~25 (W7 → e79b254)
**Tags cut:** `v1.0.0-rc1`, `v1.0.0-rc2` (both pushed; release pipeline shipped 4 artifacts each)
**Test count:** 1175 (start of W7) → 1183 (end of W11)
**Sprints landed:** W7 (security follow-up + LTS polish), W8 (bench gate), W9-A/B/C/D (specs + release notes + examples gate + smoke report), W10 (flake hardening + H1/H2 doc fixes), W11-A (pre-release validation script).

This retro covers the entire post-rc1 GA-polish cycle.

## What worked

### Adversarial-review-driven sprint scope (§29 reinforced)

The cycle entered W7 with a concrete list from /security-review on W6:
6 MEDIUMs to close. Sprint scope was unambiguous: "fix exactly these 6
items." All 6 closed cleanly. Then /security-review ran AGAIN on W7's
output and found 2 NEW MEDIUMs (algorithm gap + taxonomy completeness).
Both closed in the same cycle. Net: zero security findings outstanding
at GA-readiness.

**Pattern reinforced:** review → fix → re-review on the same diff. The
second pass costs ~5 min of agent time and consistently finds residual
issues the first pass missed. Bake into the standard sprint flow.

### Smoke testing the published artifact (W9-C) caught a real bug

W9-C downloaded `boruna-1.0.0-rc2-aarch64-apple-darwin.tar.gz` from
GitHub Releases, verified SHA256, ran an example workflow end-to-end,
and verified the bundle. The act of running `boruna capability list`
against the actual binary surfaced that the `random` capability's wire
name was `random`, but my earlier /content-review had typed it as
`random.next` in three docs. Local tests didn't catch it because they
don't compare binary output to docs.

**Pattern:** for releases, smoke-test the SHIPPED artifact, not just
the source-built one. The wire name was visible only when the binary
self-reported its capability list.

### Worktree-isolated parallel agents on disjoint surfaces (§31)

W9 ran 3 agents in parallel: W9-A (bytecode spec), W9-B (release notes),
W9-D (examples gate). Surfaces were disjoint. All three landed cleanly.
Conflicts on merge were limited to CHANGELOG.md (each added an
`[Unreleased] ### Added` entry) and resolved trivially.

The W7 docs sprint (M-1+M-2+M-4+M-5) was also run as an agent in
parallel with my direct work on M-3+M-6. Same disjoint-surface pattern,
same clean merge.

**Pattern:** for additive doc/CI sprints, parallel agents are reliably
faster than sequential without paying merge tax. Production code
parallelism remains higher-risk per §31's original lesson.

### Best-practice "stop before GA" enforcement

When the user explicitly asked me to "keep working without asking,"
I had to repeatedly assess whether the next item was strictly additive
(safe) or production-code-changing (would invalidate the soak window).
The discipline held: every commit in W7-W11 was either docs, CI, tests,
or scripts. Zero production code touched after W6 closed.

**Pattern:** during a soak window, the test "is this additive?" is the
right gate. Not "is this small?" — small production changes still
invalidate soak. Additive doc/CI/test work is GA-period-safe.

## What didn't work

### Misframing /content-review H-1 + H-2 as "post-1.0 work"

Initially classified `INTEGRATION_GUIDE.md` v0.1.0 references and
`FRAMEWORK_API.md` version label as "1.0.x patch lane (full sprint)."
On second look, both were 5-line edits, not 700-line rewrites. I had
mis-scoped them.

**Lesson:** for content-review HIGH findings, look at the actual delta
needed before deferring. "Full rewrite" was speculation; the actual
work was 5 lines per finding.

### awk parsing artifact in test-count display

Twice in this cycle, the post-test count line said `pass: 141 fail: 1`
when the real numbers were `pass: 1183 fail: 0`. Cause: my awk one-liner
treated a result line containing `1 ignored` as `1 failed`. Took me ~5
min each time to realize it was a parser bug, not a real failure.

**Fix for future cycles:** use `cargo test --workspace --features
boruna-cli/serve 2>&1 | grep "test result" | awk '{p+=$4; f+=$6} END {…}'`
where `$4` and `$6` are the right columns. Already corrected mid-cycle;
captured here so future retros don't repeat the diagnostic loop.

### The W4 agent stalled mid-sprint (recurring failure mode)

The W4 (versioned workflow DAG schema) agent stalled on the runtime's
600s no-progress watchdog. Recovery required salvaging the agent's
uncommitted dirty changes from its worktree, reviewing for correctness,
and committing manually. Cost ~15 min of recovery time.

**Lesson:** for parallel agents with substantial scope (>500 LOC), the
stall risk is real even on well-bounded surfaces. The mitigation is
already in place (per project §32 — strong gates absorb tooling
failures). The salvage worked because the agent's dirty changes were
visible and small enough to read.

**Open question:** is there a way to ping the agent for status before
the 600s watchdog fires? If so, a 300s `SendMessage` might unstick
without requiring a full salvage path. Not pursued this cycle.

### Disk space hit ENOSPC mid-cycle

Around W6 the host hit 99% disk usage from accumulated `target/`
artifacts in 4 worktrees. Cleared by removing the worktrees once
their merges landed. Not a code issue, but worth noting:

**Lesson:** for parallel-worktree sprints, plan for ~5GB of `target/`
per worktree. Clean up promptly. Alternatively, share the target dir
across worktrees via `CARGO_TARGET_DIR=$(git rev-parse --show-toplevel)/target`
in the agent's environment — but that introduces its own race risks.

## Patterns added or reinforced

### §42 (NEW) — Smoke-test the published artifact, not the source build

When cutting a release candidate, smoke-test the actual GitHub Releases
artifact (download → verify SHA256 → extract → run → verify bundle), not
just `cargo run` against source. The shipped artifact may surface bugs
invisible to source-builds (wire-name mismatches, library packaging
issues, runtime path assumptions).

Anchor: `docs/release-smoke-tests/v1.0.0-rc2.md` for the macOS arm64
target. Linux musl targets remain operator smoke tests on real hardware
per [`v1.0-GA-checklist.md`](../../barsy/obsidian/Barsy/Projects/boruna/v1.0-GA-checklist.md).

### §43 (NEW) — Pre-release-check script as a GA-cut gate

A single script that the operator runs BEFORE `git tag` confirms:
clean tree, correct branch, tag uniqueness, Cargo.toml/README/CHANGELOG
version alignment, all spec constants in place, every CI gate green,
every example workflow runs and verifies. Dry-run during script
development against the previous rc to catch script bugs.

Anchor: `scripts/pre-release-check.sh`. Replaces ad-hoc operator
checklists.

### §44 (NEW) — Re-run /security-review on the diff that closed earlier findings

The W7 follow-up review found 2 NEW MEDIUMs in the W7 fix to the W6
findings. Without the second-pass review, these would have shipped to
GA. Bake "re-review on the diff that fixes the prior review" into the
sprint template.

### §31 reinforced — Parallel agents on truly disjoint surfaces work; on overlapping CHANGELOG/main.rs sections they need merge-resolution

The W3, W5, W7-Docs, and W9-A/B/D agent runs all proved this. The
common conflict surface is CHANGELOG.md (each agent adds an Unreleased
entry). Manageable; just budget ~1 min per merge for the trivial
combine.

### §40 reinforced — Audit retro premises, including for THIS retro

Some items in this retro contradict assumptions made earlier in the
cycle. E.g. the "post-1.0 roadmap" classification of H-1 as a "full
sprint" was wrong; it was a 5-line edit. Capturing the correction in
THIS retro is the §40 application: the next cycle's planning should
not assume "if I called it a full sprint last cycle, it really is."

## Test count delta

| Sprint | New tests | Cumulative |
|--------|----------:|-----------:|
| W7 | +0 (doc-only) | 1175 |
| W7 NEW-1 fix | +1 (`bundle_verify_rejects_unknown_algorithm`) | 1176 |
| W7 M-6 | +2 (inspect plaintext-leak gate) | 1178 |
| W8 | +0 (bench compile gate, no new test asserts) | 1178 |
| W9-A | +1 (`test_bytecode_version_is_1_0`) | 1179 |
| W9-B | +4 (release-notes extraction tests) | 1183 |
| W9-C | +0 (smoke report — no source change) | 1183 |
| W9-D | +0 (CI gate — no Rust test) | 1183 |
| W10 | +0 (existing-test hardening) | 1183 |
| W10-H1 | +0 (doc fix) | 1183 |
| W10-H2 | +0 (doc fix) | 1183 |
| W11-A | +0 (script + ci.sh refresh) | 1183 |

**Net: +8 tests across the cycle. All green; clippy `--all-targets` clean
across 3 feature combinations; fmt clean; CI green for the last 7
consecutive runs.**

## Open items at end of cycle (operator-blocked)

1. **Soak window for rc2** — calendar; 2-4 weeks typical.
2. **Linux musl artifact smoke tests on real Alpine + Pi/Graviton** —
   the host running this retro is macOS-only with an offline OrbStack
   daemon. Must be operator-run on real Linux hardware.
3. **External security audit booking** — Q4 2026 deadline per
   `docs/lts.md` to land Q2 2027.
4. **Cut `v1.0.0` tag** — operator decides when soak closes; the
   process is a 5-min coding step + `bash scripts/pre-release-check.sh
   1.0.0`.

## Recommended for next cycle (post-GA)

The Obsidian post-1.0 roadmap at `~/htdocs/barsy/obsidian/Barsy/Projects/
boruna/post-1.0-roadmap.md` carries the canonical list. Highlights:

- **1.0.x rolling**: bench-on-PR comparison gate, Linux musl smoke
  reports, occasional doc clarifications.
- **1.1.0**: streaming output from `boruna_run`, LLM provider registry,
  version-aware capability tagging, `boruna run --watch`,
  error-message suggestions.
- **1.2.0**: bundle storage adapters (S3/GCS), KEK rotation tooling,
  `boruna fmt` v2.
- **0.7.x parallel branch** (LTS-incompatible): rolling upgrades, mTLS
  CRL/OCSP, web evidence inspector.
- **2.0** (deprecate in 1.x first): mutable `.ax` locals,
  default-streaming `boruna_run`, post-quantum AEAD.

LTS calendar puts 1.x active support running 18 months past GA, so
post-GA work has a clear runway.
