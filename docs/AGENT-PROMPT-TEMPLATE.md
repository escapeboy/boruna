# Parallel Worktree-Agent Prompt Template

Use this template when launching a sub-agent via the `Agent` tool with `isolation: "worktree"`. It bakes in the relative-path discipline and cwd verification that prevent the path-resolution failure mode documented in `CLAUDE.md` (see "Parallel-Agent Best Practices").

## When to use parallel worktree agents

- Sprint scope is **truly disjoint** from other concurrent work (different crates, different modules).
- Sprint is **well-scoped** (clear acceptance criteria, ~500-1000 LOC, 1-3 testable changes).
- You have **bandwidth to brief thoroughly** — agents start cold and need full context.
- You **cannot directly observe** the agent's progress; the agent must self-verify.

## When NOT to use parallel worktree agents

- Sprint touches the same crate as your sequential work or another agent's sprint.
- Sprint requires architectural judgment that would benefit from your in-the-loop review.
- Sprint is small enough that briefing overhead exceeds execution time.
- The path-resolution issue in the agent runtime has not been independently verified as fixed.

## Template skeleton

Replace `[BRACKETED]` placeholders. Keep the verification block verbatim.

```markdown
You are implementing **sprint [SPRINT-ID] — [TOPIC]** in the Boruna repo.
You are running in an isolated git worktree off `master`.

## Worktree verification (RUN FIRST, NO EXCEPTIONS)

Before any file edits, run these three commands and check the output:

```sh
pwd
git rev-parse --show-toplevel
git branch --show-current
```

Expected:
- `pwd` and `git rev-parse --show-toplevel` print the SAME path
  (or root + suffix) — your assigned worktree at
  `.claude/worktrees/agent-<id>`.
- `git branch --show-current` prints YOUR assigned branch
  (typically `feat/[SPRINT-ID]` or `worktree-agent-<id>`).

If ANY of these don't match the expectation, STOP and report
"worktree verification failed: <output>". Your file-edit tools
will land on the wrong path otherwise. Do not proceed.

## Path discipline (NON-NEGOTIABLE)

Use **relative paths only** in every `Edit`, `Write`, `Read`, and
`Bash` tool call. Examples:

- ✅ Edit `orchestrator/src/workflow/runner.rs`
- ✅ Read `crates/llmvm-cli/src/main.rs`
- ❌ Edit `/Users/.../ai-lang/orchestrator/src/workflow/runner.rs`

Absolute paths bypass the worktree's `cwd` and write to the
calling agent's main worktree. This is a known runtime
limitation; relative paths are the only safe form.

For `Bash` tool calls, prefer commands that work with relative
paths:
- ✅ `cargo test -p boruna-orchestrator`
- ✅ `grep -rn "fn foo" orchestrator/src`
- ❌ `cargo test --manifest-path /Users/.../Cargo.toml`

## Context — what's already shipped

[3-5 sentences on prior sprints relevant to this one. Cite
file:line references for the most relevant code, but use
RELATIVE paths only.]

## What you ship

[1-2 paragraphs on the deliverable.]

## File footprint (allowed paths)

You may edit ONLY these paths. Other paths belong to concurrent
sprints; touching them risks merge conflicts:

- [list of relative paths]

## Critical constraints (DO NOT TOUCH)

- [list of paths owned by other concurrent sprints, e.g.
  "DO NOT TOUCH: tooling/ — fmt sprint is in parallel"]
- [list of crates/modules off-limits per the architecture]

## Project conventions you MUST follow

- §[N] — [convention summary] — see `docs/project-conventions.md`
  or the `project-conventions-2026-04` Serena memory.
- [...]

## Acceptance criteria — all must pass

- `cargo build --workspace` clean.
- `cargo test -p [your-crate]` — all existing tests pass + your
  new ones (N+ unit tests, M+ integration tests).
- `cargo clippy --workspace -- -D warnings` clean.
- `cargo fmt --all -- --check` clean.

## Documentation

- New `docs/design-[topic].md` — design doc.
- Update `CHANGELOG.md` `[Unreleased]` `### Added`.
- [other docs]

## Final commit + branch

When done:
1. Run all gates from "Acceptance criteria".
2. `git add` your files (RELATIVE paths).
3. Commit with subject: `feat([sprint-id]): [short description]`.
4. Leave the branch in your worktree. Do NOT push. Do NOT merge.

Report back with:
- Branch name + commit hash.
- File list (relative paths).
- Test count delta.
- Any HIGH adversarial-review findings you triaged.
- Under 500 words.

## Reading order (start here)

[3-5 reading-order entries. RELATIVE PATHS ONLY.]
1. `CLAUDE.md` — project conventions, parallel-agent rules
2. [other relevant files]

You have full file-edit permissions for the allowed file
footprint. Don't ask for confirmation; just ship.
```

## Why this template exists

On 2026-04-27, a multi-sprint parallel attempt (auth + retry + fmt) failed worktree isolation because the agent prompts contained absolute paths. Both sub-agents wrote to the main worktree instead of their isolated worktrees, causing three sprints to converge onto a single branch. Recovery worked (test/clippy/fmt gates absorbed the contamination), but the friction was significant — see `retro/retro-2026-04-27-multi-sprint-auth-retry-fmt.md`.

This template hardens the workflow against recurrence. Future parallel-agent prompts should start from this skeleton and replace placeholders rather than writing prompts from scratch.

## Pre-launch checklist

Before invoking the `Agent` tool with this prompt:

- [ ] Replaced every `[BRACKETED]` placeholder.
- [ ] Confirmed every path in the prompt is relative (not absolute).
- [ ] Verified the file footprint is **disjoint** from concurrent sprints.
- [ ] Listed concurrent sprints' paths under "DO NOT TOUCH".
- [ ] Set `isolation: "worktree"` and `run_in_background: true`.

## Post-completion checklist

When the agent reports back:

- [ ] Verify the agent's branch exists locally: `git branch | grep <branch>`.
- [ ] Verify the commit landed on the branch: `git log --oneline <branch> ^master`.
- [ ] Cherry-pick or merge the branch into master via `--no-ff`.
- [ ] Run full gates on master one more time before pushing.
- [ ] Clean up the agent worktree: `git worktree remove --force .claude/worktrees/agent-<id>`.
