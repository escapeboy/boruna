# Test plan: `command` LLM provider

## Engine unit tests (`llm_providers.rs`, Unix-only where a script is executed)

| # | Case | Expected |
|---|---|---|
| C1 | `cat` as the command | reply equals the prompt (stdin delivered, stdout returned) |
| C2 | `sh -c 'printf %s "$1"' sh {model}` with model `x/m1` | reply `m1`; `{model}` inside a longer argument is replaced too |
| C3 | script exits 3 writing to stderr | error names exit code 3 and the stderr tail |
| C4 | `sleep 5` with `timeout_ms: 200` | error mentions the timeout; returns in well under 5 s; process killed |
| C5 | output larger than `max_output_bytes` | error, no unbounded memory |
| C6 | program not found | error "cannot run" with the program name |
| C7 | large prompt (2 MiB) with `cat` | full round trip (no pipe deadlock) |
| C8 | trailing newline in output | trimmed |
| V1 | `kind: command` without `command` / with `[]` / with `[""]` | load error |
| V2 | `command` on `kind: openai` | load error |
| V3 | `api_key_env` or `base_url` on `kind: command` | load error |
| V4 | `describe()` | shows the program, no arguments |
| R1 | non-http build: command-only config builds a router; mixed config errors | as stated |

## CLI integration (`crates/llmvm-cli/tests`)

| # | Case | Expected |
|---|---|---|
| I1 | `boruna run prog.ax --live --providers p.json` with a fake script provider | prints the script's reply |
| I2 | same without `--live` | mock reply, script not run (marker file absent) |
| I3 | `--record` then `boruna replay` | replay succeeds without running the script (marker counter unchanged) |
| I4 | policy denying `llm.call` | capability denied, script not run |

## Desktop

| # | Case | Expected |
|---|---|---|
| D1 | tokenizer: `claude -p --tools "" --model {model}` | 6 args incl. an empty one; quotes and escaped quotes |
| D2 | `ProvidersFile.render` for a preset | `kind: command`, `command` array, no key fields |
| D3 | presets | each preset's first element is the program, tools-off flags present |
| D4 | integration: generator via a fake `command` provider that returns a valid workflow JSON | workflow written and validated (first real end-to-end generation test) |

## Manual

- Real `claude` and `codex` through `boruna run --live` with a one-line prompt.
- Desktop: Settings → add Claude Code preset → write a workflow from a sentence.
