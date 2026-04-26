# Design: Prometheus metrics export

**Sprint**: `0.4-S12`. Companion to `0.4-S5` (OTel observability —
already shipped) for SLI dashboards.

## Decision: CLI-pulled, not embedded HTTP

Three possible architectures were considered:

| Approach | Pros | Cons |
| -------- | ---- | ---- |
| Embedded HTTP `/metrics` endpoint | Standard Prometheus pull model | Contradicts the CLI-only philosophy locked in `0.3-S15` (no in-binary HTTP server). Long-running daemon process model for what is otherwise a CLI tool. |
| OTel Collector exporter | Zero new code in Boruna | Operators must deploy + configure an OTel Collector. Heavy ops burden for small teams. Indirect path to Prom backend. |
| **CLI-pulled metrics from `runs.db`** | Pure CLI. Stateless. Reads existing persistent store. Standard Prometheus pattern for batch jobs. | Sampling cadence is operator-controlled (cron); not real-time. |

**We pick CLI-pulled.** It aligns with Boruna's CLI-only stance,
requires no new infrastructure, and uses the persistence store the
runner already maintains. Operators integrate via cron +
`node_exporter`'s textfile collector — the canonical Prometheus
pattern for batch tools.

## Operator integration

```sh
# Cron entry on each Boruna host:
*/30 * * * * boruna metrics export --data-dir /var/lib/boruna \
                > /var/lib/node_exporter/textfile_collector/boruna.prom

# node_exporter scrapes the .prom file every scrape interval
# (typically 15s) and exposes the metrics on its own /metrics
# endpoint, which Prometheus pulls.
```

Sampling cadence (30 min default) is a tradeoff between freshness
and SQL-aggregation cost. Operators with high-volume Boruna
deployments can drop to 1-min cadence cheaply (the queries below
are bounded by total run count × 1 SELECT).

## Metrics surface (MVP)

Three families, all label-rich for dashboarding:

```
# HELP boruna_workflow_runs_total Total workflow runs by terminal status.
# TYPE boruna_workflow_runs_total counter
boruna_workflow_runs_total{workflow="...",status="completed"} N
boruna_workflow_runs_total{workflow="...",status="failed"} N
boruna_workflow_runs_total{workflow="...",status="paused"} N
boruna_workflow_runs_total{workflow="...",status="running"} N

# HELP boruna_workflow_runs_in_flight Currently-executing or paused runs.
# TYPE boruna_workflow_runs_in_flight gauge
boruna_workflow_runs_in_flight{workflow="..."} N

# HELP boruna_workflow_step_completions_total Total step completions by status.
# TYPE boruna_workflow_step_completions_total counter
boruna_workflow_step_completions_total{workflow="...",step="...",status="completed"} N
boruna_workflow_step_completions_total{workflow="...",step="...",status="failed"} N
```

`workflow` label is the workflow's `name` field. `step` is the
step_id within the workflow's DAG.

## Counter semantics

Counters are computed from the persistence store at sample time,
not maintained as deltas. So a `_total` metric represents the
*current count of runs/steps in that state* in the store, not a
monotonically-increasing-since-process-start counter. This matches
the Prometheus textfile-collector convention for batch-job metrics
and keeps the implementation stateless.

A consequence: if old runs are pruned from the DB, the `_total`
will decrease. Operators handling pruning should either capture
historical counts at prune time or accept the recompute-from-state
contract. Documented in the metric `# HELP` comment when noticed
in the field.

## Out of scope (this sprint)

- Histograms / summaries for run duration. Adds output complexity
  for a single sprint.
- Per-step retry counts. The `attempt_count` column is there, but
  exposing it adds 1+ more series per step. Defer.
- Memory / CPU / disk metrics for the Boruna binary itself —
  those belong to the host's `node_exporter`, not this command.
- Push gateway integration. `node_exporter` textfile is the
  recommended path; pushgateway is for ephemeral jobs that finish
  before scrape.
