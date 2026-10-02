# Design: agent docs that cannot drift (`skills emit`, `skills pack`)

Status: sprint 1 of 2 from the agentlanguages.dev borrow review (2026-10-02).
Sprint 2 is [conformal gating](design-conformal-gating.md).

## Problem

`boruna skills get <name>` serves four hand-written markdown files embedded in the
binary (`crates/llmvm-cli/src/skills/*.md`). Nothing checks them against the real
CLI. The repo has already had this class of bug (crate renames, a removed HTTP layer
still documented, `ax-language.md` syntax drift fixed in PR #80). An agent that reads a
stale skill emits a command that does not exist.

Borrowed from: Vow (generate the agent skill from the compiled toolchain) and Lume
(token-budgeted retrieval over the docs).

## Decisions

| Question | Decision | Why |
|---|---|---|
| Who needs it? | Agents (Claude Code, Cursor) driving an installed `boruna` with no repo checkout | Same audience as the existing `skills` command |
| Narrowest MVP | (1) the `cli` skill's command reference is generated from the clap tree at runtime; (2) `skills emit <dir>` writes the skills as `SKILL.md` folders an agent can load; (3) `skills pack` returns only the sections relevant to a query within a token budget | Each is independently useful and testable |
| Rewrite the hand-written `cli.md`? | No. Keep its prose. Replace nothing; append a generated reference section | Prose explains tasks; the generated part carries exact flags. Rewriting prose is the gold-plating we want to avoid |
| Retrieval algorithm | Keyword overlap over heading-split chunks, ties broken by (score desc, skill name, heading) | Deterministic output is a Boruna invariant. No embeddings, no network |
| Token budget | `chars / 4`, documented as an estimate | A real tokenizer dependency is not worth it |

## Not in this sprint (and why)

- REPeL-style restart menu at gates (retry / substitute / skip). It changes approval
  semantics across four code paths in `runner.rs`. It deserves its own design; it is not
  documentation tooling.
- Stylesheet model-routing (Fabro). No demand signal in this repo.
- Type-guided token sampling (MoonBit). We sit downstream of generation.
- Behavioral certificates (Aver). Needs `verify` blocks and a Lean pipeline. Research.

## Acceptance

1. `boruna skills get cli` ends with a generated reference that lists every top-level
   subcommand and every flag of `skills`, `evidence`, `workflow`.
2. A test fails when a subcommand exists in clap but is absent from the generated
   reference (cannot happen by construction) AND when a subcommand named in the
   hand-written prose no longer exists (catches the real drift).
3. `boruna skills emit <dir>` creates `<dir>/<name>/SKILL.md` with `name` and
   `description` frontmatter, byte-identical on a second run.
4. `boruna skills pack "<query>" --budget N [--json]` returns at most N estimated
   tokens, same bytes on every run, exit 1 with a message when nothing matches.
