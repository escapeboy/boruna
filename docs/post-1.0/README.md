# Post-1.0 Execution Tracking

This directory hosts public-facing artifacts for work that lands
on top of `v1.0.0` GA. The candid roadmap and per-task execution
plan are kept out of the repo — they document trade-offs (e.g.
"this would break the LTS contract") that operator-facing docs
deliberately do not contain.

## Where the planning lives

| Source | Visibility | Purpose |
|--------|------------|---------|
| [`docs/lts.md`](../lts.md) | Public | LTS commitments operators rely on |
| [`docs/roadmap.md`](../roadmap.md) | Public | Operator-facing forward look |
| [`docs/branch-policy.md`](../branch-policy.md) | Public | `master` vs. `0.7.x` topology |
| `~/Obsidian/.../boruna/post-1.0-roadmap.md` | Internal | Candid lane breakdown |
| `~/Obsidian/.../boruna/post-1.0-execution-plan.md` | Internal | Per-task execution graph |

## How tasks are tracked on GitHub

Every post-1.0 task gets:

- A branch named `post1/T-<wave>.<n>-<slug>` (e.g.
  `post1/T-1.1-streaming-progress`).
- A pull request with the standard CHANGELOG entry under
  `## [Unreleased]` and an acceptance-criteria checklist in the body.
- The labels listed below applied for filterability.

### Labels

| Label | Meaning |
|-------|---------|
| `post1-execution` | Tracked by the post-1.0 execution plan |
| `wave-1` … `wave-4` | Which wave the task belongs to |
| `branch-master` | Lands on `master` (1.x LTS line) |
| `branch-0.7.x` | Lands on `0.7.x` (speculative parallel work) |

### Project board

A "Post-1.0 Execution" project (Projects v2) tracks task state
across columns `Wave 1`, `Wave 2`, `Wave 3`, `Wave 4`, `Done`.
Board creation requires the `project` token scope; if the board is
not yet linked, the labels above stand in.

## How to find current state

- Open PRs: `gh pr list --label post1-execution`
- Closed in a wave: `gh pr list --label wave-1 --state merged`
- 0.7.x work: `gh pr list --base 0.7.x`
