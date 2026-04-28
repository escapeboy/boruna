# Migration tooling (beta)

Status: **beta** (sprint `W5-C`). API and on-disk shapes may shift in
the 0.6 line; the public CLI invocation will remain stable.

`boruna migrate` upgrades pre-1.0 artifacts to the format the current
release expects. It is the supported way to bring legacy evidence
bundles and workflow definitions forward as Boruna's on-disk
contracts evolve toward 1.0.

## When to use it

| Symptom | Migrator | Notes |
|---|---|---|
| `boruna evidence inspect` rejects a v0.5.0-or-earlier bundle for missing `bundle.json` | `evidence-bundle` | Synthesizes a `bundle.json` summary; cannot reconstruct missing checksums (see "What it does NOT do" below). |
| `boruna workflow validate` rejects a workflow.json with "missing `schema_version`" | `workflow-json` | Adds `"schema_version": 1`. |

If you are starting fresh with 0.6.0+, you do not need this tool —
new artifacts are produced in the current format.

## CLI shape

```
boruna migrate <kind> <path> [--from <version>] [--to <version>] [--dry-run] [--in-place]
```

| Flag | Purpose |
|---|---|
| `<kind>` | `evidence-bundle` or `workflow-json`. |
| `<path>` | Bundle directory (for `evidence-bundle`) or `workflow.json` file (for `workflow-json`). |
| `--from` | Source version of the input. Optional; the migrator infers from contents when absent. |
| `--to` | Target version. Defaults to `current` (latest stable). The beta also accepts `1`, `0.6.0`, `1.0.0`. |
| `--dry-run` | Report what WOULD change without touching disk. Always run this first. |
| `--in-place` | Rewrite the input artifact directly. Default is to write a `<path>.migrated` sibling. |

## Recommended workflow

Always do this before shipping a migrated artifact into a downstream
pipeline:

```bash
# 1. Take a backup. The migrator does not snapshot for you.
cp -R bundles/run-2026-01-01-abc bundles/run-2026-01-01-abc.bak

# 2. Dry-run to see the planned change.
boruna migrate evidence-bundle bundles/run-2026-01-01-abc --dry-run

# 3. Apply, writing a sibling file you can diff.
boruna migrate evidence-bundle bundles/run-2026-01-01-abc

# 4. Review bundle.json.migrated; once happy, swap in place.
boruna migrate evidence-bundle bundles/run-2026-01-01-abc --in-place
```

For workflow.json the same pattern applies:

```bash
boruna migrate workflow-json examples/workflows/legacy/workflow.json --dry-run
boruna migrate workflow-json examples/workflows/legacy/workflow.json --in-place
```

## What each migrator does

### `evidence-bundle`

For a bundle directory:

- If `bundle.json` already exists, the migrator reports a no-op.
- Otherwise, it synthesizes a `bundle.json` summary containing
  `format_version`, `boruna_version` (default `0.6.0-pre` when no
  embedded metadata pins it down), `created_at` (mtime of
  `audit_log.json` if present, otherwise current time), `run_id`
  (derived from the bundle directory name), `workflow_hash` (extracted
  from the first `WorkflowStarted` event in the audit log when readable;
  empty string when not), and `components` (sorted relative paths of
  every file in the bundle).
- The synthesized object includes `synthesized: true` so downstream
  tooling can distinguish a reconstructed summary from a runner-native
  one.
- When the bundle ALSO contains `manifest.json` (the canonical
  per-file checksum manifest from `boruna-orchestrator`), the migrator
  cross-checks it via `verify_bundle` and reports the result.

### `workflow-json`

For a single `workflow.json` file:

- If `schema_version: 1` is present, the migrator reports a no-op and
  the file is byte-identical afterwards.
- If `schema_version` is missing, the migrator adds it as `1`.
- If `schema_version` is present and greater than `1`, the migrator
  errors out — downgrading from a future schema is out of scope for
  the beta.
- If `schema_version` is some other value (negative, non-integer,
  string), the migrator rejects the input as malformed.

Note on field order: `serde_json::Map` in this workspace is a
`BTreeMap`, so the migrated file's keys land in lexicographic order.
The semantic content is preserved; comments (which JSON does not
support) are unaffected.

## What the migrators do NOT do

- **Reconstruct missing checksums.** A legacy evidence bundle that
  never had `manifest.json` cannot grow one with hashes that match
  the original artifacts after the fact — that would manufacture a
  false integrity guarantee. The synthesized `bundle.json` is a
  summary, not a checksum manifest.
- **Downgrade from a future schema.** `--to` is forward-only in the
  beta.
- **Touch persistence stores (`runs.db`).** Database schema migrations
  are deferred to a follow-up sprint that will land alongside the
  next breaking persistence change.
- **Edit `examples/workflows/*/workflow.json` retroactively.** Those
  ship at the current schema with each release.

## Coverage matrix (beta)

| From | To | `evidence-bundle` | `workflow-json` |
|---|---|---|---|
| 0.5.0 (and earlier) | 0.6.0 / 1.0.0 / current | yes (best-effort summary) | yes |
| 0.6.0+ artifacts | current | no-op | no-op |
| 1.x+ artifacts | downgrade | not supported | not supported |

The matrix will expand in 1.x as additional breaking changes
accumulate. Each future migrator will land with its own line in this
table and a sprint reference in `docs/roadmap.md`.

## Reporting issues

If `boruna migrate` rejects a bundle or workflow that you believe is
valid 0.5.0 output, file an issue at
<https://github.com/escapeboy/boruna/issues> with:

- The full migrator output (run with `--dry-run` first).
- A redacted copy of the bundle directory listing or workflow.json.
- The Boruna version that produced the artifact.
