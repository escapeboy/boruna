# Architecture: agent docs emit / pack

All changes are in `crates/llmvm-cli`. No other crate is touched.

## Components

```
main.rs        SkillsCommand gains Emit{dir} and Pack{query,budget,json}
skills.rs      + render(skill)      -> body, with generated reference for "cli"
               + emit(dir)          -> writes <dir>/<name>/SKILL.md
               + chunks()           -> heading-split chunks of rendered skills
               + pack(query,budget) -> ordered chunks within budget
```

### Generated CLI reference

`clap::CommandFactory::command()` on the `Cli` struct gives the full tree. A function
walks it depth-first in declaration order and prints one line per (sub)command:
`boruna evidence verify <DIR>  - <about>` plus its flags. `Cli` lives in `main.rs`, so
`skills::render` takes a `&clap::Command` argument instead of importing `Cli`.

`skills get cli` and `skills emit` both call `render`, so they print the same bytes.

### `skills emit`

For each skill: create `<dir>/<name>/SKILL.md`:

```
---
name: boruna-<name>
description: <summary>
---

<rendered body>
```

Overwrites existing files (the command's stated purpose). Refuses a `<dir>` that is an
existing regular file. Prints the list of written paths.

### `skills pack`

1. Render every skill, split on lines starting with `## ` (a chunk keeps its heading).
2. Tokenize query and chunk into lowercase alphanumeric words, drop words under 3 chars.
3. Score = number of distinct query words found in the chunk, heading hits count double.
4. Sort by (score desc, skill name asc, chunk index asc). Drop score 0.
5. Emit chunks until the estimated budget (`len/4`) would be exceeded. The first chunk is
   always emitted truncated at a line boundary if it alone exceeds the budget.

Output is plain markdown, or `--json` with `{query, budget, tokens_estimated, chunks:[{skill,heading,text}]}`.

## Data flow / invariants

- No I/O except writing under `<dir>`; no clock, no randomness, no HashMap iteration.
- Skill bodies are compile-time constants (`include_str!`), unchanged.
