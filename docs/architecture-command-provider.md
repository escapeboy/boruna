# Architecture: `command` LLM provider

Reads: `design-command-provider.md`. Target runs on the user's machine; nothing remote to probe.
CLI behaviour checked on 2026-10-04: `claude 2.1.289` and `codex-cli 0.160.0` print only the reply
on stdout with the flags below; `gemini 0.33.1` was not logged in here.

## Engine (`crates/llmvm/src/llm_providers.rs`)

```json
{
  "providers": {
    "claude": {
      "kind": "command",
      "command": ["claude", "-p", "--tools", "", "--strict-mcp-config",
                  "--no-session-persistence", "--model", "{model}"],
      "timeout_ms": 300000
    }
  }
}
```

- `ProviderKind::Command`; `ProviderConfig.command: Option<Vec<String>>`,
  `max_output_bytes: Option<u64>` (default 4 MiB).
- `from_json` validation: `command` required and non-empty (first element non-empty) for
  `kind: command`; `command` rejected on other kinds; `api_key_env`, `base_url`, `region` rejected
  on `command`.
- New module `command_provider` (not behind the `http` feature): `CommandHandler` implements
  `CapabilityHandler` for `llm.call`:
  1. prompt = args[0], model = part after `/` of args[1] (same parsing as HTTP providers);
  2. argv = `command` with every `{model}` replaced;
  3. spawn with stdin/stdout/stderr piped, no shell; write the prompt to stdin on a thread and
     close it; read stdout and stderr on threads, stopping at `max_output_bytes`;
  4. wait with the timeout (poll `try_wait`), kill on timeout;
  5. exit 0 → stdout with trailing whitespace trimmed; non-zero → error with the exit code and
     the last 500 characters of stderr; spawn failure → "cannot run <program>: <error>".
- `build_router`: command providers get a `CommandHandler` in both builds; other kinds need
  the `http` feature as before (non-http build errors only when a non-command provider exists).
- `describe()`: `name -> Command, program=claude`.

Recording and replay need nothing new: the router is a `CapabilityHandler`, and the VM's event
log already records each `llm.call` result.

## Desktop client (`boruna-desktop`)

- `ProviderConfig` gains `command: [String]?`, `timeoutMs: Int?`; `ProvidersFile.render` writes
  `command` and `timeout_ms`; `requiredEnvironment` is empty for `command`.
- Presets: `claude` (Claude Code), `codex` (Codex), `gemini` (Gemini CLI), each with tools off,
  and a free-form command.
- Settings: kind `command` shows the command as one editable line (shell-like quoting: `""` is
  an empty argument), parsed by a small tokenizer; a note on what the program may do.
- Default model for a fresh install stays `anthropic/...`; choosing a preset suggests
  `claude/sonnet`, `codex/gpt-5.4-mini`, `gemini/gemini-2.5-flash`.
- The live-run confirmation lists command providers with their program.

## Docs

LLM integration guide (new section), CLI reference for `--providers`, SECURITY (providers.json can
run programs), CHANGELOG 4.2.0, desktop README.
