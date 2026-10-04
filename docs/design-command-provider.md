# Design: `command` LLM provider

Date: 2026-10-04 · Phase: Think · Target: Boruna 4.2.0 + Boruna Desktop 0.2.0

## Problem

`llm_call` reaches models only through HTTP APIs (`openai`, `openai_compat`, `anthropic`, `ollama`,
`bedrock`), which need paid API credit. Many users already pay for a subscription that comes with a
command-line tool: Claude Code (`claude`), Codex (`codex`), Gemini CLI (`gemini`). On 2026-10-04
both API accounts on the maintainer's machine were empty while `claude` and `codex` worked, so
the desktop client could not write a single workflow.

## Who and what they do today

- Workflow authors with a subscription but no API key: they cannot use `llm_call` or the desktop
  client's generator at all.
- Developers running Boruna locally: they would like the model they already use in the terminal.

## Narrowest useful version

A provider kind `command`: `providers.json` names a program and its arguments; Boruna runs it
for each `llm.call`, writes the prompt to its stdin and takes its stdout as the reply. One
`{model}` placeholder in the arguments carries the model from `llm_call(prompt, "name/model")`.

## What makes it work well

- The desktop client offers ready presets (Claude Code, Codex, Gemini) with their tools switched
  off, so writing a workflow costs nothing beyond the subscription.
- Replies are recorded like any provider's: a recorded run replays without running the command,
  and the evidence shows the call.

## Safety (the part that matters)

These CLIs are agents: left to their defaults they can edit files and run shell commands, outside
Boruna's capability policy. Boruna cannot see or stop that. So:

1. Boruna never uses a shell: the command is an argument list, the prompt goes to stdin, not the
   arguments.
2. The documentation and the client's presets switch the tools off:
   `claude -p --tools "" --strict-mcp-config --no-session-persistence`,
   `codex exec --sandbox read-only --skip-git-repo-check --ephemeral -`,
   `gemini --approval-mode plan -p ""`.
3. Whoever can edit `providers.json` can run programs as the user. That file was already
   operator configuration (it chooses where prompts go); the docs now say so plainly.
4. Limits: a timeout (default 300 s, then the process is killed) and a maximum reply size
   (default 4 MiB). A non-zero exit fails the call with the end of stderr.
5. It only runs with `--live`, like every other provider. Without `--live` the mock answers.

## Decisions

| Question | Decision |
|---|---|
| Shell or argv | argv only, no shell |
| Prompt delivery | stdin (no argument length limit, no quoting) |
| Model | `{model}` substituted inside any argument |
| Environment | inherited (the CLIs need `HOME` and their own login) |
| `api_key_env` / `base_url` on a command provider | rejected at load |
| Needs the `http` build feature | no: works in any build |
| Output | stdout, trailing whitespace trimmed; stderr only in errors |

## Out of scope

Streaming, multi-turn sessions, parsing JSON output formats, Windows `.cmd` shims beyond
documenting them (`"command": ["claude.cmd", ...]`).
