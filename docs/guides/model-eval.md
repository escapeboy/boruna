# Model Evaluation Framework

`boruna workflow eval` runs the same workflow against two LLM provider
configurations and compares the resulting evidence bundles.

## Usage

```bash
boruna workflow eval examples/workflows/llm_code_review \
  --providers-a anthropic.json \
  --providers-b ollama.json \
  --runs 3 --live
```

`--live` makes each side call its own providers (the release binaries include the HTTP
support this needs). Without `--live`, both sides run on the mock handler and the comparison
says nothing about the models; the command prints a warning.

## Provider config format

Each file uses the same `{"providers": {...}}` format as `boruna run --providers`. The step
source picks the provider by name (`llm_call(prompt, "anthropic/claude-sonnet-4-6")`), so give
both files the same provider name and point it at different models or endpoints:

```json
{
  "providers": {
    "anthropic": { "kind": "anthropic", "api_key_env": "ANTHROPIC_API_KEY" }
  }
}
```

See [LLM integration](llm-integration.md#built-in-providers) for every provider kind and field.
The older capability-keyed format (`{"llm.call": {...}}`) is still accepted without `--live`
but cannot call a provider, so `--live` rejects it.

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
| `--live` | Call the real providers. Without it both sides use the mock |
| `--data-dir <dir>` | Directory for evidence bundles (default: `.boruna/data`) |
| `--json` | Machine-readable JSON output |

## Evidence bundles

Each run writes a full evidence bundle under `<data-dir>/model-eval/<name>/run_N/evidence/`.
`<name>` is the provider file's name without `.json`; when both files have the same name, the
sides are called `<name>-a` and `<name>-b`.
These can be inspected with `boruna evidence inspect` or diffed with `boruna evidence diff`.

## Notes on live LLM comparison

Provider replies are not deterministic, so two live runs of the same side can differ; that is
what the "identical" columns measure. Each run's evidence bundle keeps the step outputs, so you can see
what each model returned.
