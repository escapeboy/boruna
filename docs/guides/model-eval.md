# Model Evaluation Framework

`boruna workflow eval` runs the same workflow against two LLM provider
configurations and compares the resulting evidence bundles.

## Usage

```bash
boruna workflow eval examples/workflows/llm_code_review \
  --providers-a anthropic.json \
  --providers-b ollama.json \
  --runs 3
```

## Provider config format

```json
{
  "llm.call": {
    "provider": "anthropic",
    "model": "claude-3-5-sonnet-20241022",
    "api_key_env": "ANTHROPIC_API_KEY"
  }
}
```

Supported providers: `anthropic`, `ollama`, `openai_compat`, `deny`, `passthrough`.

See `docs/guides/llm-integration.md` for full BYOH handler wiring.

## Output

The command prints a comparison table:

```
Provider A (anthropic): 3/3 runs succeeded (100%), mean 1420ms
Provider B (ollama):    3/3 runs succeeded (100%), mean 890ms

Step                     A identical   B identical   A vs B agree
----------------------------------------------------------------------
analyze                  yes           yes           no  (different)
report                   yes           yes           yes (identical)
```

Use `--json` for machine-readable output suitable for CI pipelines.

## Options

| Flag | Description |
|------|-------------|
| `--providers-a <file>` | First provider config JSON |
| `--providers-b <file>` | Second provider config JSON |
| `--runs N` | Number of runs per provider (default: 1) |
| `--data-dir <dir>` | Directory for evidence bundles (default: `.boruna/data`) |
| `--json` | Machine-readable JSON output |

## Evidence bundles

Each run writes a full evidence bundle under `<data-dir>/model-eval/<provider-name>/run_N/evidence/`.
These can be inspected with `boruna evidence inspect` or diffed with `boruna evidence diff`.

## Notes on live LLM comparison

The eval command validates the config and logs the provider that would be used, then runs
the workflow in demo mode. Live provider comparison (actual LLM API calls) requires the
`boruna-vm/http` feature and wired BYOH handlers — see `docs/guides/llm-integration.md`.
