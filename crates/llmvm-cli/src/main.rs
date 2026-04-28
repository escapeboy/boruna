use std::fs;
use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

use boruna_bytecode::Module;
use boruna_framework::runtime::AppMessage;
use boruna_framework::testing::TestHarness;
use boruna_framework::validate::AppValidator;
use boruna_tooling::diagnostics::collector::DiagnosticCollector;
use boruna_tooling::repair::{RepairStrategy, RepairTool};
use boruna_tooling::trace2tests;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy, ReplayHandler};
use boruna_vm::replay::EventLog;
use boruna_vm::vm::Vm;

#[cfg(feature = "serve")]
mod coordinator;
#[cfg(feature = "serve")]
mod dashboard;
mod format;
#[cfg(feature = "serve")]
mod serve;
#[cfg(feature = "serve")]
mod worker;

#[derive(Parser)]
#[command(
    name = "boruna",
    about = "Boruna — deterministic, capability-safe language"
)]
struct Cli {
    /// Environment namespace (sprint 0.4-S14). When set, the
    /// persistent `--data-dir` is namespaced to `<data-dir>/<env>/`,
    /// and Prometheus metrics carry an `env="<env>"` label. Use for
    /// dev/staging/prod separation. Falls back to `BORUNA_ENV` env
    /// var. When unset, the data dir and metrics are unscoped (the
    /// pre-0.4-S14 behavior).
    ///
    /// Names are restricted to `[a-zA-Z0-9_-]+` (1-64 chars) to
    /// keep them filesystem-safe and Prometheus-label-safe.
    #[arg(long, global = true, value_name = "NAME")]
    env: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a .ax source file to bytecode.
    Compile {
        /// Source file path (.ax)
        file: PathBuf,
        /// Output file path (.axbc). Defaults to same name with .axbc extension.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Run a .ax source file or bytecode file.
    Run {
        /// File path (.ax or .axbc)
        file: PathBuf,
        /// Capability policy: "allow-all", "deny-all", or a JSON policy file.
        #[arg(short, long, default_value = "allow-all")]
        policy: String,
        /// Maximum execution steps.
        #[arg(long, default_value = "10000000")]
        max_steps: u64,
        /// Record execution events to this file.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Use real HTTP handler for net.fetch (requires `http` feature).
        #[arg(long)]
        live: bool,
        /// Record net.fetch transactions to a tape file (requires --live).
        /// Mutually exclusive with --replay-net-from.
        /// See docs/design-net-record-replay.md.
        #[arg(long, conflicts_with = "replay_net_from")]
        record_net_to: Option<PathBuf>,
        /// Replay net.fetch transactions from a tape file. No real network
        /// access. Mutually exclusive with --record-net-to. If --live is
        /// also set, replay wins (no network calls happen).
        #[arg(long, conflicts_with = "record_net_to")]
        replay_net_from: Option<PathBuf>,
    },
    /// Run with execution tracing enabled.
    Trace {
        /// File path (.ax or .axbc)
        file: PathBuf,
    },
    /// Replay execution from a recorded event log.
    Replay {
        /// Bytecode file (.axbc)
        file: PathBuf,
        /// Event log file (JSON)
        log: PathBuf,
    },
    /// Inspect a bytecode file.
    Inspect {
        /// Bytecode file (.axbc)
        file: PathBuf,
    },
    /// Dump the AST of a .ax source file.
    Ast {
        /// Source file path (.ax)
        file: PathBuf,
    },
    /// Format a .ax source file (canonical pretty-print).
    ///
    /// Default: rewrite the file in place. With `--check`, exit 0 if the
    /// file is already formatted, exit 1 otherwise (CI gate). Exits 2 on
    /// parse errors so CI can distinguish "needs formatting" from
    /// "broken file". v1 strips comments — see `docs/design-boruna-fmt.md`.
    Fmt {
        /// Source file (.ax) to format.
        file: PathBuf,
        /// Check whether the file is already formatted; do not modify it.
        #[arg(long)]
        check: bool,
    },
    /// Framework commands.
    #[command(subcommand)]
    Framework(FrameworkCommand),
    /// Language tooling commands (diagnostics, repair).
    #[command(subcommand)]
    Lang(LangCommand),
    /// Trace-to-test tools (record, generate, run, minimize).
    #[command(subcommand)]
    Trace2tests(Trace2TestsCommand),
    /// Template tools (list, apply, validate).
    #[command(subcommand)]
    Template(TemplateCommand),
    /// Workflow execution and validation.
    #[command(subcommand)]
    Workflow(WorkflowCommand),
    /// Evidence bundle inspection and verification.
    #[command(subcommand)]
    Evidence(EvidenceCommand),
    /// Capability surface inspection (versioned identity for caching).
    #[command(subcommand)]
    Capability(CapabilityCommand),
    /// Prometheus metrics export from the persistent run store
    /// (sprint 0.4-S12). See `docs/design-prometheus-metrics.md`
    /// for the architectural decision and operator integration
    /// pattern (cron + node_exporter's textfile collector).
    #[command(subcommand)]
    Metrics(MetricsCommand),
    /// Policy file validation and inspection (sprint 0.4-S15).
    /// See `docs/design-policy-as-code.md` and
    /// `docs/reference/policy-schema.md` for the schema and the
    /// stable `error_kind` taxonomy.
    #[command(subcommand)]
    Policy(PolicyCommand),
    /// Workflow dashboard — read-only HTTP view over `runs.db`
    /// (sprint 0.4-S16). Requires `--features serve`. See
    /// `docs/design-workflow-dashboard.md` for the security
    /// posture (loopback by default, no auth).
    #[cfg(feature = "serve")]
    #[command(subcommand)]
    Dashboard(DashboardCommand),
    /// Distributed-execution coordinator — HTTP server that
    /// dispatches workflow steps to remote workers (sprint
    /// 0.5-S2b, ADR 002). Requires `--features serve`. Loopback
    /// default; **no authentication** — front with reverse
    /// proxy if exposed publicly.
    #[cfg(feature = "serve")]
    #[command(subcommand)]
    Coordinator(CoordinatorCommand),
    /// Distributed-execution worker — polls a coordinator for
    /// claimable steps, executes them, reports results (sprint
    /// 0.5-S2b, ADR 002). Requires `--features serve`.
    #[cfg(feature = "serve")]
    #[command(subcommand)]
    Worker(WorkerCommand),
    /// Migration tooling beta (sprint `W5-C`). Upgrades pre-1.0
    /// Boruna artifacts to the current on-disk format. See
    /// `docs/guides/migration.md` for the coverage matrix and
    /// recommended workflow.
    Migrate {
        /// Migration kind: `evidence-bundle` or `workflow-json`.
        kind: String,
        /// Path to the artifact (a directory for `evidence-bundle`,
        /// a file for `workflow-json`).
        path: PathBuf,
        /// Source version of the input. When absent the migrator
        /// infers from the file's contents.
        #[arg(long)]
        from: Option<String>,
        /// Target version. Defaults to "current" (latest stable).
        #[arg(long, default_value = "current")]
        to: String,
        /// Report the planned change without writing to disk.
        #[arg(long)]
        dry_run: bool,
        /// Modify the input artifact in place. Default: write a
        /// `<path>.migrated` sibling.
        #[arg(long)]
        in_place: bool,
    },
}

#[cfg(feature = "serve")]
#[derive(Subcommand)]
enum CoordinatorCommand {
    /// Serve the coordinator HTTP routes.
    Serve {
        /// Persistent data directory holding `runs.db`. Same
        /// fallback chain as `boruna workflow run`.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Listen port (default 8090).
        #[arg(long, default_value = "8090")]
        port: u16,
        /// Bind address. Defaults to `127.0.0.1`. Pass `0.0.0.0`
        /// to expose on all interfaces (you accept the
        /// no-auth-on-LAN consequences).
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Cap on lease TTL workers can request (default 5 min).
        #[arg(long, default_value = "300000")]
        max_lease_ttl_ms: u64,
        /// Long-poll wait timeout for `/api/work/claim`
        /// (default 30 s).
        #[arg(long, default_value = "30000")]
        poll_timeout_ms: u64,
        /// Background lease-expiry sweep interval in
        /// milliseconds (default 30 s). Lower = faster
        /// recovery from worker crashes; higher = less DB
        /// churn under steady-state. Minimum 100 ms (lower
        /// values are clamped + a warning is logged).
        #[arg(long, default_value = "30000")]
        sweep_interval_ms: u64,
        /// Shared-secret bearer token for HTTP authentication
        /// (sprint `0.5-S3`). When set, every coord HTTP route
        /// requires `Authorization: Bearer <secret>` header.
        /// Generate via `openssl rand -hex 32`. Falls back to
        /// `BORUNA_COORD_SECRET` env var. When unset, no auth
        /// is enforced — operators binding to a non-loopback
        /// address without a secret get a loud stderr warning
        /// (the no-auth posture remains backwards-compatible
        /// for loopback-only deployments).
        #[arg(long, env = "BORUNA_COORD_SECRET")]
        shared_secret: Option<String>,
    },
    /// Drive a submit-only workflow run to terminal status by
    /// computing downstream-ready successors as workers complete
    /// steps and writing fresh Pending checkpoints. Sprint
    /// `0.5-S2f`: client-side multi-wave advancement. Operates on
    /// the same `runs.db` the coordinator process uses; must run
    /// on a host with filesystem access to `--data-dir`.
    ///
    /// Idempotent on restart — kill and re-invoke at any point;
    /// the run continues from where it was left.
    ///
    /// Exit codes:
    /// - 0 — run reached `Completed` status.
    /// - 1 — run reached `Failed` status.
    /// - 2 — invalid arguments, run not found, missing
    ///   `workflow_def` in metadata, or unsupported step kind
    ///   (approval/external_trigger in non-first wave).
    /// - 3 — `--max-wait-secs` budget exceeded before terminal.
    Wait {
        /// Run id to drive to terminal status (returned by
        /// `boruna workflow run --submit-only`).
        run_id: String,
        /// Persistent data directory holding `runs.db`. Same
        /// fallback chain as `boruna workflow run`. Must match
        /// the coordinator process's `--data-dir`.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Polling interval in milliseconds. Minimum 100 ms
        /// (lower values are clamped + a warning is logged).
        #[arg(long, default_value = "500")]
        poll_interval_ms: u64,
        /// Maximum total wait duration in seconds. `0` =
        /// unlimited. Useful for CI test timeouts.
        #[arg(long, default_value = "0")]
        max_wait_secs: u64,
    },
}

#[cfg(feature = "serve")]
#[derive(Subcommand)]
enum WorkerCommand {
    /// Run a worker that polls the named coordinator for work.
    Run {
        /// Coordinator base URL, e.g.
        /// `http://coord.internal:8090`.
        #[arg(long)]
        coordinator: String,
        /// Optional worker id; auto-generated if absent.
        #[arg(long)]
        worker_id: Option<String>,
        /// Lease TTL the worker requests on each claim.
        /// Coordinator may cap this.
        #[arg(long, default_value = "300000")]
        lease_ttl_ms: u64,
        /// Long-poll timeout the worker tells the coordinator
        /// to wait before returning 204.
        #[arg(long, default_value = "30000")]
        poll_timeout_ms: u64,
        /// Shared-secret bearer token for HTTP authentication
        /// (sprint `0.5-S3`). MUST match the coordinator's
        /// `--shared-secret`. Falls back to `BORUNA_COORD_SECRET`
        /// env var. When unset, no `Authorization` header is
        /// sent — only works when the coord also has no secret.
        #[arg(long, env = "BORUNA_COORD_SECRET")]
        shared_secret: Option<String>,
    },
}

#[cfg(feature = "serve")]
#[derive(Subcommand)]
enum DashboardCommand {
    /// Serve a read-only dashboard over HTTP.
    ///
    /// Loopback (127.0.0.1) by default. Pass `--bind 0.0.0.0` to
    /// expose on the LAN; the dashboard ships no auth, so any
    /// public bind MUST be fronted by an auth-enforcing reverse
    /// proxy.
    Serve {
        /// Persistent data directory holding `runs.db`. Same
        /// fallback chain as `boruna workflow run` /
        /// `metrics export`.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Listen port (default 8080).
        #[arg(long, default_value = "8080")]
        port: u16,
        /// Bind address. Defaults to `127.0.0.1`. Pass `0.0.0.0`
        /// to expose on all interfaces (you accept the
        /// no-auth-on-LAN consequences).
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
    },
}

#[derive(Subcommand)]
enum PolicyCommand {
    /// Strict-validate a policy file. Exits 0 on ok, 2 on
    /// validation error, 1 on file IO error. Designed as a CI gate.
    Validate {
        /// Policy file path (.json).
        file: PathBuf,
        /// Emit machine-parseable JSON to stdout instead of the
        /// human-readable summary.
        #[arg(long)]
        json: bool,
    },
    /// Validate then print the effective policy in human-readable
    /// form: default behavior, denormalized rule list, net policy
    /// bounds.
    Show {
        /// Policy file path (.json).
        file: PathBuf,
    },
}

#[derive(Subcommand)]
enum MetricsCommand {
    /// Export current metrics in Prometheus text format to stdout.
    /// Pipe to a `.prom` file under `node_exporter`'s textfile
    /// collector directory:
    ///
    ///   `boruna metrics export --data-dir /var/lib/boruna \
    ///     > /var/lib/node_exporter/textfile_collector/boruna.prom`
    Export {
        /// Persistent data directory holding `runs.db`. Same fallback
        /// chain as `boruna workflow run` / `resume`.
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum CapabilityCommand {
    /// List all capabilities this binary exposes, with stable identity hash.
    /// Use `capability_set_hash` as part of a cache key to safely memoize
    /// deterministic results across binary upgrades.
    /// See docs/reference/capability-identity.md.
    List {
        /// Output as JSON (canonical machine surface).
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TemplateCommand {
    /// List available templates.
    List {
        /// Templates directory (defaults to ./templates).
        #[arg(long, default_value = "templates")]
        dir: PathBuf,
    },
    /// Apply a template with arguments.
    Apply {
        /// Template name.
        name: String,
        /// Templates directory (defaults to ./templates).
        #[arg(long, default_value = "templates")]
        dir: PathBuf,
        /// Template arguments as key=value pairs (comma-separated).
        #[arg(long)]
        args: String,
        /// Output file path.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Validate that output compiles.
        #[arg(long)]
        validate: bool,
    },
}

#[derive(Subcommand)]
enum LangCommand {
    /// Check a source file and report diagnostics.
    Check {
        /// Source file (.ax)
        file: PathBuf,
        /// Output diagnostics as JSON.
        #[arg(long)]
        json: bool,
        /// Write JSON diagnostics to this file.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Repair a source file using diagnostic suggestions.
    Repair {
        /// Source file (.ax) to repair.
        file: PathBuf,
        /// Diagnostics JSON file (if not provided, runs check first).
        #[arg(long)]
        from: Option<PathBuf>,
        /// Strategy: "best" (default), "all", or a specific patch ID.
        #[arg(long, default_value = "best")]
        apply: String,
    },
}

#[derive(Subcommand)]
enum Trace2TestsCommand {
    /// Record an execution trace from a framework app.
    Record {
        /// Source file (.ax).
        file: PathBuf,
        /// Messages to send as "tag:payload" pairs (comma-separated).
        #[arg(short, long)]
        messages: String,
        /// Output trace file (JSON).
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Generate a test spec from a recorded trace.
    Generate {
        /// Trace file (JSON).
        #[arg(long)]
        trace: PathBuf,
        /// Test name.
        #[arg(long, default_value = "regression_test")]
        name: String,
        /// Output test spec file (JSON).
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Run a test spec against source code.
    Run {
        /// Test spec file (JSON).
        #[arg(long)]
        spec: PathBuf,
        /// Source file (.ax). If not provided, uses source_file from spec.
        #[arg(long)]
        source: Option<PathBuf>,
    },
    /// Minimize a failing trace using delta debugging.
    Minimize {
        /// Trace file (JSON).
        #[arg(long)]
        trace: PathBuf,
        /// Source file (.ax).
        #[arg(long)]
        source: PathBuf,
        /// Predicate: "panic" (default) or external command.
        #[arg(long, default_value = "panic")]
        predicate: String,
        /// Output minimized trace file.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum WorkflowCommand {
    /// Validate a workflow definition directory.
    Validate {
        /// Workflow directory containing workflow.json.
        dir: PathBuf,
        /// After validation succeeds, emit `workflow_hash=<hex>` on
        /// stdout. Pair with `boruna workflow run/resume
        /// --expect-workflow-hash` to lock the def at deploy time:
        ///   $ HASH=$(boruna workflow validate ./wf --print-hash | cut -d= -f2)
        ///   $ boruna workflow run ./wf --expect-workflow-hash $HASH ...
        #[arg(long)]
        print_hash: bool,
    },
    /// Run a workflow.
    Run {
        /// Workflow directory containing workflow.json.
        dir: PathBuf,
        /// Capability policy: "allow-all", "deny-all", or a JSON policy file.
        #[arg(short, long, default_value = "allow-all")]
        policy: String,
        /// Record evidence bundle for this run.
        #[arg(long)]
        record: bool,
        /// Evidence output directory (defaults to <dir>/evidence/).
        #[arg(long)]
        evidence_dir: Option<PathBuf>,
        /// Use real HTTP handler for net.fetch (requires `http` feature).
        #[arg(long)]
        live: bool,
        /// Persistent data directory for `runs.db` and per-run output.
        /// Falls back to $BORUNA_DATA_DIR, then `./.boruna/data`. Pass
        /// `--ephemeral` to run without writing checkpoints.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Run without persistence — no checkpoints written, no
        /// `runs.db` created. Cannot be combined with `--data-dir`.
        #[arg(long, conflicts_with = "data_dir")]
        ephemeral: bool,
        /// Maximum steps to run concurrently within a topological
        /// level. `1` = sequential (default — preserves prior
        /// behavior). Higher values speed up fan-out workflows; the
        /// per-step output_hash is bit-identical to a sequential run.
        /// Persistent path only; ignored on `--ephemeral`.
        #[arg(long, default_value = "1")]
        concurrency: usize,
        /// Skip cleanly (exit 0) if a prior `Running` or `Paused` run
        /// of this workflow is still active in the data dir. Designed
        /// for cron-driven scheduled invocations:
        ///
        ///     0 2 * * * boruna workflow run /path/to/wf \
        ///               --skip-if-running --data-dir /var/lib/boruna
        ///
        /// Persistent path only; incompatible with `--ephemeral`.
        #[arg(long, conflicts_with = "ephemeral")]
        skip_if_running: bool,
        /// Submit the workflow to a distributed coord+workers
        /// cluster instead of executing in-process (sprint
        /// `0.5-S2e`). Validates + inserts the run row +
        /// initial wave's source-step Pending checkpoints +
        /// embeds step sources in metadata, then exits. The
        /// cluster picks up the steps via existing
        /// claim/dispatch mechanisms.
        ///
        /// Persistent path only; incompatible with
        /// `--ephemeral` and `--skip-if-running` (the
        /// skip-if-running path executes in-process and
        /// silently ignores submit-only — adversarial-review
        /// finding F1, fixed by making them mutually
        /// exclusive). Workflows using approval-gate /
        /// external-trigger steps in the first wave fail at
        /// submit time. Multi-wave automatic advancement is
        /// deferred to a future sprint.
        #[arg(long, conflicts_with_all = ["ephemeral", "skip_if_running"])]
        submit_only: bool,
        /// Submit the workflow to a remote coordinator over HTTP and
        /// poll for terminal status (sprint `0.5-S4`). The CI runner
        /// does NOT need filesystem access to the cluster's data-dir;
        /// the workflow.json + every Source-kind step's `.ax` body
        /// are inlined into the submit payload. Bearer token via
        /// `--coord-token` or the `BORUNA_TOKEN` env var when the
        /// cluster is auth-gated (0.5-S3).
        ///
        /// Mutually exclusive with `--data-dir` (different
        /// operational model entirely), `--ephemeral`,
        /// `--submit-only`, and `--skip-if-running`. Exit codes
        /// match `coordinator wait`: `0` Completed, `1` Failed,
        /// `2` timeout / submit-failed.
        #[arg(
            long,
            value_name = "URL",
            conflicts_with_all = ["data_dir", "ephemeral", "submit_only", "skip_if_running"]
        )]
        coordinator: Option<String>,
        /// Bearer token for the coordinator's auth middleware (sprint
        /// `0.5-S3`). Only meaningful with `--coordinator`. Falls back
        /// to the `BORUNA_TOKEN` env var. Omit if the cluster is
        /// loopback / unauthenticated.
        #[arg(long, value_name = "BEARER", env = "BORUNA_TOKEN")]
        coord_token: Option<String>,
        /// How often to poll `/api/runs/{run_id}/status` when running
        /// against `--coordinator`. Defaults to `1000` ms; clamped
        /// silently to a `500`-ms floor matching `coordinator wait`.
        /// Ignored without `--coordinator`.
        #[arg(long, default_value = "1000")]
        coord_poll_interval_ms: u64,
        /// Maximum total wall-clock time to wait for terminal status
        /// in `--coordinator` mode. `0` (default) means wait
        /// indefinitely — same posture as `coordinator wait`. On
        /// timeout the CLI exits with `2` and the run keeps going
        /// on the cluster.
        #[arg(long, default_value = "0")]
        coord_max_wait_secs: u64,
        /// CI/CD safety check: refuse to run if the on-disk def's
        /// workflow_hash doesn't match this value (case-insensitive
        /// 64-char SHA-256 hex). Capture via `boruna workflow
        /// validate <dir> --print-hash` at deploy time. Note: hashes
        /// the workflow.json structure only — `.ax` step source
        /// changes do NOT affect the hash. Use a tree hash for
        /// full-source coverage.
        #[arg(long, value_name = "HEX")]
        expect_workflow_hash: Option<String>,
    },
    /// Approve a paused approval-gate step. Records an approval sentinel
    /// in the run's metadata; the operator must run `boruna workflow
    /// resume <run-id>` afterward to advance the run past the gate.
    Approve {
        /// Run id (16-hex deterministic id from `boruna workflow run`).
        run_id: String,
        /// Step id of the approval gate to approve.
        step_id: String,
        #[arg(long, conflicts_with = "coordinator")]
        data_dir: Option<PathBuf>,
        /// Sprint 0.5-S6: drive a remote coordinator over HTTP instead
        /// of mutating a local data-dir. POSTs to
        /// `/api/runs/{run_id}/approve`. Mutually exclusive with
        /// `--data-dir`. Bearer token via `--coord-token` or
        /// `BORUNA_TOKEN` env var.
        #[arg(long, value_name = "URL")]
        coordinator: Option<String>,
        /// Bearer token for the coordinator's auth middleware. Falls
        /// back to the `BORUNA_TOKEN` env var. Only meaningful with
        /// `--coordinator`.
        #[arg(long, value_name = "BEARER", env = "BORUNA_TOKEN")]
        coord_token: Option<String>,
    },
    /// Reject a paused approval-gate step. Records a rejection sentinel;
    /// `boruna workflow resume <run-id>` will then halt the run as
    /// Failed with the optional reason as the error message.
    Reject {
        run_id: String,
        step_id: String,
        /// Optional human-readable rejection reason. Surfaces in the
        /// resumed run's step error_msg.
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, conflicts_with = "coordinator")]
        data_dir: Option<PathBuf>,
        /// Sprint 0.5-S6: drive a remote coordinator over HTTP. POSTs
        /// to `/api/runs/{run_id}/approve` with `decision: "rejected"`.
        #[arg(long, value_name = "URL")]
        coordinator: Option<String>,
        #[arg(long, value_name = "BEARER", env = "BORUNA_TOKEN")]
        coord_token: Option<String>,
    },
    /// Trigger a paused external_trigger step (sprint 0.3-S15). Records
    /// the supplied payload as the step's output and primes resume to
    /// advance past the gate. Operator must run `boruna workflow resume
    /// <run-id>` afterward to actually execute downstream steps.
    ///
    /// Designed for webhook-driven workflows: the operator's webhook
    /// receiver bridges the incoming HTTP body to this CLI. The payload
    /// becomes a JSON-encoded String value visible to downstream steps
    /// via the `step_input(name)` builtin (sprint 0.3-S14).
    ///
    /// `--token` is the value printed at pause-time when the runner
    /// entered the gate. The trigger CLI rejects mismatching tokens to
    /// prevent accidental cross-step triggers from a misrouted webhook.
    Trigger {
        /// Run id (16-hex deterministic id from `boruna workflow run`).
        run_id: String,
        /// Step id of the external_trigger step to advance.
        step_id: String,
        /// Token printed at pause-time. Required.
        #[arg(long)]
        token: String,
        /// JSON-encoded payload to record as the step's output. Mutually
        /// exclusive with `--payload-file`.
        #[arg(long, conflicts_with = "payload_file")]
        payload: Option<String>,
        /// Path to a file whose contents are the JSON payload. Mutually
        /// exclusive with `--payload`. Useful for large webhook bodies.
        #[arg(long)]
        payload_file: Option<PathBuf>,
        #[arg(long, conflicts_with = "coordinator")]
        data_dir: Option<PathBuf>,
        /// Sprint 0.5-S6: drive a remote coordinator over HTTP. POSTs
        /// to `/api/runs/{run_id}/trigger`. Mutually exclusive with
        /// `--data-dir`.
        #[arg(long, value_name = "URL")]
        coordinator: Option<String>,
        #[arg(long, value_name = "BEARER", env = "BORUNA_TOKEN")]
        coord_token: Option<String>,
    },
    /// Show the full state of a single run: row, step checkpoints, and
    /// approval-gate decisions. Use `--json` for machine-readable output
    /// (jq-friendly). Reads from the same `--data-dir` as `run`/`resume`.
    Show {
        /// Run id to inspect.
        run_id: String,
        /// Output as JSON (machine-readable).
        #[arg(long)]
        json: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// List runs in the persistent store. Optional --status filter.
    List {
        /// Filter by status: "running" | "paused" | "completed" | "failed".
        #[arg(long)]
        status: Option<String>,
        /// Output as JSON (machine-readable).
        #[arg(long)]
        json: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Resume a previously-paused or crashed workflow run by id.
    Resume {
        /// Run id (16-hex-character deterministic id from `boruna workflow run`).
        run_id: String,
        /// Persistent data directory holding `runs.db`. Same fallback
        /// chain as `boruna workflow run`.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Override the workflow definition directory. Defaults to the
        /// path stored in the run's metadata at the original run.
        #[arg(long)]
        workflow_dir: Option<PathBuf>,
        /// Capability policy override. Defaults to the policy from the
        /// original run.
        #[arg(short, long)]
        policy: Option<String>,
        /// Use real HTTP handler for net.fetch (requires `http` feature).
        #[arg(long)]
        live: bool,
        /// Maximum steps to run concurrently within a topological
        /// level on resumed waves. Default `1` = sequential.
        #[arg(long, default_value = "1")]
        concurrency: usize,
        /// CI/CD safety check: refuse to resume if the on-disk def's
        /// workflow_hash doesn't match this value. Operationally
        /// stricter than the implicit resume-time hash check (which
        /// compares against the persisted run's stored hash, not
        /// against an operator-supplied expected value).
        #[arg(long, value_name = "HEX")]
        expect_workflow_hash: Option<String>,
    },
}

#[derive(Subcommand)]
enum EvidenceCommand {
    /// Build an evidence bundle from a persisted run (sprint 0.4-S10).
    /// Reads the run's metadata, step checkpoints, and hash-chained
    /// audit log; writes a bundle directory containing workflow.json,
    /// policy.json, per-step outputs, audit_log.json, env_fingerprint.json,
    /// and a manifest.json with bundle hash + per-file checksums.
    ///
    /// The bundle is created post-hoc — the runner does NOT auto-create
    /// bundles during execution. Operators trigger bundle creation
    /// explicitly when needed (e.g., a compliance request months after
    /// the run completed).
    Create {
        /// Run id (16-hex deterministic id from `boruna workflow run`).
        run_id: String,
        /// Output directory. The bundle is written to
        /// `<output_dir>/<run_id>/` — the run_id subdirectory is
        /// created if it doesn't exist.
        #[arg(long)]
        output_dir: PathBuf,
        /// Persistent data directory holding `runs.db`. Same fallback
        /// chain as `boruna workflow run` / `resume`.
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Verify an evidence bundle for integrity.
    Verify {
        /// Evidence bundle directory.
        dir: PathBuf,
    },
    /// Inspect an evidence bundle's manifest.
    Inspect {
        /// Evidence bundle directory.
        dir: PathBuf,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FrameworkCommand {
    /// Create a new framework app from template.
    New {
        /// Application name
        name: String,
        /// Output directory (defaults to current dir)
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },
    /// Validate a .ax file conforms to the App protocol.
    Validate {
        /// Source file (.ax)
        file: PathBuf,
    },
    /// Run a framework app interactively with messages.
    Test {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages to send as "tag:payload" pairs (comma-separated).
        #[arg(short, long)]
        messages: Option<String>,
    },
    /// Inspect framework app state after running messages.
    InspectState {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages to send before inspecting.
        #[arg(short, long)]
        messages: Option<String>,
    },
    /// Simulate a sequence of messages and display state transitions.
    Simulate {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages as "tag:payload" pairs (comma-separated).
        messages: String,
    },
    /// Print App contract summary (State, Messages, Effects) — machine-readable.
    Inspect {
        /// Source file (.ax)
        file: PathBuf,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Structured diagnostics output (JSON).
    Diag {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages to send before diagnostics.
        #[arg(short, long)]
        messages: Option<String>,
    },
    /// Run with tracing and print a stable hash of the trace.
    TraceHash {
        /// Source file (.ax)
        file: PathBuf,
        /// Messages to send.
        #[arg(short, long)]
        messages: Option<String>,
    },
    /// Replay a recorded cycle log and verify determinism.
    Replay {
        /// Source file (.ax)
        file: PathBuf,
        /// Cycle log file (JSON).
        log: PathBuf,
    },
    /// Serve a framework app as a web page (requires --features serve).
    #[cfg(feature = "serve")]
    Serve {
        /// Source file (.ax)
        file: PathBuf,
        /// Port to listen on.
        #[arg(short, long, default_value = "3000")]
        port: u16,
    },
}

/// CLI entry point.
///
/// **Telemetry feature:** when built with `--features telemetry`, `main`
/// starts a tokio runtime (required by the OTel batch exporter) and calls
/// `boruna_vm::init_telemetry()` BEFORE parsing CLI args. The init function
/// reads `OTEL_EXPORTER_OTLP_ENDPOINT`; when unset the telemetry handle is
/// `Disabled` and behaves identically to a non-telemetry build (zero
/// allocations, zero spans exported). When set, capability spans
/// (`boruna.cap` with `cap.name`, `bytes_in`, `bytes_out`,
/// `cap.budget_remaining`, `error.kind` attributes) are exported via
/// OTLP-over-HTTP. The `TelemetryHandle` is dropped at the end of `main`
/// so pending spans flush before the binary exits.
///
/// **Without `telemetry` feature:** `main` is plain sync; capability spans
/// are still emitted (the `tracing` dep is non-optional in `boruna-vm`)
/// but go nowhere because no subscriber is installed. Zero overhead.
#[cfg(feature = "telemetry")]
fn main() {
    let runtime = tokio::runtime::Runtime::new()
        .expect("failed to start tokio runtime for telemetry feature");
    // Enter the runtime BEFORE init_telemetry — the OTel batch exporter
    // spawns its background task into the current runtime context.
    let _runtime_guard = runtime.enter();

    // Init may return Err on a hard config problem (malformed endpoint URL,
    // global subscriber already installed). Treat as warning, not fatal —
    // the rest of the CLI works fine without telemetry.
    let _telemetry_handle = match boruna_vm::init_telemetry() {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("warning: telemetry init failed: {e}");
            None
        }
    };

    let cli = Cli::parse();
    let result = run(cli);

    // Drop the telemetry handle BEFORE shutting down the runtime so
    // force_flush has somewhere to enqueue. The handle's Drop calls
    // `force_flush()` which returns immediately — actual HTTP POSTs run
    // on tokio tasks. We then shut down the runtime with a bounded
    // timeout so those tasks get a chance to complete; otherwise
    // `process::exit` below would kill them mid-flight and silently lose
    // the last batch of spans.
    drop(_telemetry_handle);
    drop(_runtime_guard);
    runtime.shutdown_timeout(std::time::Duration::from_secs(5));

    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

#[cfg(not(feature = "telemetry"))]
fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    // 0.4-S14: install the env namespace once at the top of run() so
    // every downstream call to `resolve_data_dir` and metrics export
    // sees the same value. The env flag wins over BORUNA_ENV; both
    // are validated against `[a-zA-Z0-9_-]+` (1–64 chars) to keep
    // names filesystem-safe and Prometheus-label-safe. An invalid
    // name produces a typed error rather than silently namespacing
    // to a name that the persistence layer would later reject.
    let env_name = match cli.env.as_deref() {
        Some(s) => Some(s.to_string()),
        None => std::env::var("BORUNA_ENV").ok().filter(|s| !s.is_empty()),
    };
    if let Some(name) = &env_name {
        validate_env_name(name)?;
    }
    // Threaded explicitly into resolve_data_dir() and metrics::export()
    // below — no env-var side channel.
    let env_arg = env_name.as_deref();

    match cli.command {
        Command::Compile { file, output } => {
            let source = fs::read_to_string(&file)?;
            let name = file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "module".into());
            let module = boruna_compiler::compile(&name, &source)?;
            let out_path = output.unwrap_or_else(|| file.with_extension("axbc"));
            let bytes = module.to_bytes()?;
            fs::write(&out_path, bytes)?;
            println!("compiled {} -> {}", file.display(), out_path.display());
            println!(
                "  {} functions, {} constants, {} types",
                module.functions.len(),
                module.constants.len(),
                module.types.len()
            );
        }
        Command::Run {
            file,
            policy,
            max_steps,
            record,
            live,
            record_net_to,
            replay_net_from,
        } => {
            let module = load_module(&file)?;
            let gateway = make_gateway(
                &policy,
                live,
                record_net_to.as_deref(),
                replay_net_from.as_deref(),
            )?;
            let mut vm = Vm::new(module, gateway);
            vm.set_max_steps(max_steps);

            match vm.run() {
                Ok(result) => {
                    println!("{result}");
                    if !vm.ui_output.is_empty() {
                        println!("\n--- UI Output ---");
                        for tree in &vm.ui_output {
                            let json = serde_json::to_string_pretty(tree)?;
                            println!("{json}");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("runtime error: {e}");
                    process::exit(1);
                }
            }

            if let Some(log_path) = record {
                let json = vm
                    .event_log()
                    .to_json()
                    .map_err(|e| format!("failed to serialize event log: {e}"))?;
                fs::write(&log_path, json)?;
                println!("events recorded to {}", log_path.display());
            }

            println!("steps: {}", vm.step_count());
        }
        Command::Trace { file } => {
            let module = load_module(&file)?;
            let gateway = CapabilityGateway::new(Policy::allow_all());
            let mut vm = Vm::new(module, gateway);
            vm.trace_enabled = true;

            match vm.run() {
                Ok(result) => println!("result: {result}"),
                Err(e) => eprintln!("runtime error: {e}"),
            }

            println!("\n--- Trace ({} steps) ---", vm.step_count());
            for entry in &vm.trace {
                println!("  {entry}");
            }
        }
        Command::Replay { file, log } => {
            let module = load_module(&file)?;
            let log_json = fs::read_to_string(&log)?;
            let original_log =
                EventLog::from_json(&log_json).map_err(|e| format!("invalid event log: {e}"))?;

            let results = original_log.capability_results();
            let handler = Box::new(ReplayHandler::new(results));
            let gateway = CapabilityGateway::with_handler(Policy::allow_all(), handler);
            let mut vm = Vm::new(module, gateway);

            match vm.run() {
                Ok(result) => println!("replay result: {result}"),
                Err(e) => eprintln!("replay error: {e}"),
            }

            let replay_result =
                boruna_vm::replay::ReplayEngine::verify(&original_log, vm.event_log());
            println!("replay verification: {replay_result:?}");
        }
        Command::Inspect { file } => {
            let module = load_module(&file)?;
            println!("Module: {}", module.name);
            println!("Version: {}", module.version);
            println!("Entry: function #{}", module.entry);
            println!("\nTypes ({}):", module.types.len());
            for (i, t) in module.types.iter().enumerate() {
                println!("  #{i}: {} {:?}", t.name, t.kind);
            }
            println!("\nConstants ({}):", module.constants.len());
            for (i, c) in module.constants.iter().enumerate() {
                println!("  #{i}: {c}");
            }
            println!("\nFunctions ({}):", module.functions.len());
            for (i, f) in module.functions.iter().enumerate() {
                println!(
                    "  #{i}: {}(arity={}, locals={}, ops={})",
                    f.name,
                    f.arity,
                    f.locals,
                    f.code.len()
                );
                if !f.capabilities.is_empty() {
                    let caps: Vec<_> = f.capabilities.iter().map(|c| c.name()).collect();
                    println!("      capabilities: {}", caps.join(", "));
                }
                for (j, op) in f.code.iter().enumerate() {
                    println!("      {j:04}: {op:?}");
                }
            }
        }
        Command::Ast { file } => {
            let source = fs::read_to_string(&file)?;
            let tokens = boruna_compiler::lexer::lex(&source)?;
            let program = boruna_compiler::parser::parse(tokens)?;
            let json = serde_json::to_string_pretty(&program)?;
            println!("{json}");
        }
        Command::Fmt { file, check } => format::run_fmt(&file, check)?,
        Command::Framework(fw) => run_framework(fw)?,
        Command::Lang(lang) => run_lang(lang)?,
        Command::Trace2tests(t2t) => run_trace2tests(t2t)?,
        Command::Template(tmpl) => run_template(tmpl)?,
        Command::Workflow(wf) => run_workflow(wf, env_arg)?,
        Command::Evidence(ev) => run_evidence(ev, env_arg)?,
        Command::Capability(cap) => run_capability(cap)?,
        Command::Metrics(m) => run_metrics(m, env_arg)?,
        Command::Policy(p) => {
            let code = run_policy(p);
            if code != 0 {
                process::exit(code);
            }
        }
        #[cfg(feature = "serve")]
        Command::Dashboard(d) => run_dashboard(d, env_arg)?,
        #[cfg(feature = "serve")]
        Command::Coordinator(c) => run_coordinator(c, env_arg)?,
        #[cfg(feature = "serve")]
        Command::Worker(w) => run_worker_cmd(w)?,
        Command::Migrate {
            kind,
            path,
            from,
            to,
            dry_run,
            in_place,
        } => run_migrate(kind, path, from, to, dry_run, in_place)?,
    }
    Ok(())
}

fn run_migrate(
    kind: String,
    path: PathBuf,
    _from: Option<String>,
    to: String,
    dry_run: bool,
    in_place: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if to != "current" && to != "1" && to != "0.6.0" && to != "1.0.0" {
        return Err(format!(
            "unsupported --to target {to:?} (beta supports: current, 1, 0.6.0, 1.0.0)"
        )
        .into());
    }

    let report = match kind.as_str() {
        "evidence-bundle" => boruna_tooling::migrations::evidence_bundle::migrate_bundle_dir(
            &path, dry_run, in_place,
        )?,
        "workflow-json" => boruna_tooling::migrations::workflow_json::migrate_workflow_json(
            &path, dry_run, in_place,
        )?,
        other => {
            return Err(format!(
                "unknown migration kind {other:?} (expected: evidence-bundle | workflow-json)"
            )
            .into());
        }
    };

    println!("{report}");
    Ok(())
}

#[cfg(feature = "serve")]
fn run_coordinator(
    cmd: CoordinatorCommand,
    env_arg: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        CoordinatorCommand::Serve {
            data_dir,
            port,
            bind,
            max_lease_ttl_ms,
            poll_timeout_ms,
            sweep_interval_ms,
            shared_secret,
        } => {
            #[cfg(feature = "persist-sqlite")]
            {
                let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                let bind_addr: std::net::IpAddr = bind
                    .parse()
                    .map_err(|e| format!("invalid --bind address {bind:?}: {e}"))?;
                coordinator::run_serve(
                    resolved,
                    port,
                    bind_addr,
                    max_lease_ttl_ms,
                    poll_timeout_ms,
                    sweep_interval_ms,
                    shared_secret,
                )?;
            }
            #[cfg(not(feature = "persist-sqlite"))]
            {
                let _ = (
                    data_dir,
                    port,
                    bind,
                    max_lease_ttl_ms,
                    poll_timeout_ms,
                    sweep_interval_ms,
                    shared_secret,
                );
                return Err("`coordinator serve` requires the `persist-sqlite` feature".into());
            }
        }
        CoordinatorCommand::Wait {
            run_id,
            data_dir,
            poll_interval_ms,
            max_wait_secs,
        } => {
            #[cfg(feature = "persist-sqlite")]
            {
                let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                let exit_code =
                    coordinator::run_wait(resolved, run_id, poll_interval_ms, max_wait_secs)?;
                if exit_code != 0 {
                    std::process::exit(exit_code);
                }
            }
            #[cfg(not(feature = "persist-sqlite"))]
            {
                let _ = (run_id, data_dir, poll_interval_ms, max_wait_secs);
                return Err("`coordinator wait` requires the `persist-sqlite` feature".into());
            }
        }
    }
    Ok(())
}

#[cfg(feature = "serve")]
fn run_worker_cmd(cmd: WorkerCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        WorkerCommand::Run {
            coordinator,
            worker_id,
            lease_ttl_ms,
            poll_timeout_ms,
            shared_secret,
        } => {
            worker::run_worker(
                coordinator,
                worker_id,
                lease_ttl_ms,
                poll_timeout_ms,
                shared_secret,
            )?;
        }
    }
    Ok(())
}

#[cfg(feature = "serve")]
fn run_dashboard(
    cmd: DashboardCommand,
    env_arg: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        DashboardCommand::Serve {
            data_dir,
            port,
            bind,
        } => {
            #[cfg(feature = "persist-sqlite")]
            {
                let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                let bind_addr: std::net::IpAddr = bind
                    .parse()
                    .map_err(|e| format!("invalid --bind address {bind:?}: {e}"))?;
                dashboard::run_serve(resolved, port, bind_addr)?;
            }
            #[cfg(not(feature = "persist-sqlite"))]
            {
                let _ = (data_dir, port, bind);
                return Err("`dashboard serve` requires the `persist-sqlite` feature".into());
            }
        }
    }
    Ok(())
}

fn run_policy(cmd: PolicyCommand) -> i32 {
    use boruna_vm::policy_validate;
    match cmd {
        PolicyCommand::Validate { file, json } => match policy_validate::parse_file(&file) {
            Ok(_p) => {
                if json {
                    println!(r#"{{"ok":true}}"#);
                } else {
                    println!("OK: {}", file.display());
                }
                0
            }
            Err(e) => {
                if json {
                    let payload = serde_json::json!({
                        "ok": false,
                        "errors": [policy_error_to_json(&e)],
                    });
                    println!("{payload}");
                } else {
                    eprintln!("error: {e}");
                }
                if matches!(e, policy_validate::PolicyParseError::Io { .. }) {
                    1
                } else {
                    2
                }
            }
        },
        PolicyCommand::Show { file } => match policy_validate::parse_file(&file) {
            Ok(p) => {
                print_policy_show(&p);
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                if matches!(e, policy_validate::PolicyParseError::Io { .. }) {
                    1
                } else {
                    2
                }
            }
        },
    }
}

fn policy_error_to_json(e: &boruna_vm::policy_validate::PolicyParseError) -> serde_json::Value {
    use boruna_vm::policy_validate::PolicyParseError as E;
    let mut obj = serde_json::Map::new();
    obj.insert("error_kind".into(), e.error_kind().into());
    obj.insert("message".into(), e.to_string().into());
    match e {
        E::UnknownField { path, .. } => {
            obj.insert("path".into(), path.clone().into());
        }
        E::InvalidCapability { found, hint } => {
            obj.insert("found".into(), found.clone().into());
            if let Some(h) = hint {
                obj.insert("hint".into(), h.clone().into());
            }
        }
        E::InvalidNetPolicy { field, .. } => {
            obj.insert("field".into(), (*field).into());
        }
        E::UnknownSchemaVersion(v) => {
            obj.insert("found".into(), serde_json::Value::Number((*v).into()));
        }
        _ => {}
    }
    serde_json::Value::Object(obj)
}

fn print_policy_show(p: &boruna_vm::Policy) {
    println!("Schema version: {}", p.schema_version);
    println!(
        "Default behavior: {}",
        if p.default_allow { "allow" } else { "deny" }
    );
    if p.rules.is_empty() {
        println!("Rules: (none)");
    } else {
        println!("Rules:");
        for (cap, rule) in &p.rules {
            let action = if rule.allow { "allow" } else { "deny" };
            let budget = if rule.budget == 0 {
                "unlimited".to_string()
            } else {
                rule.budget.to_string()
            };
            println!("  {cap:<14} {action:<5}  budget={budget}");
        }
    }
    match &p.net_policy {
        Some(np) => {
            println!("Net policy:");
            let domains = if np.allowed_domains.is_empty() {
                "(any)".to_string()
            } else {
                np.allowed_domains.join(", ")
            };
            let methods = if np.allowed_methods.is_empty() {
                "(any)".to_string()
            } else {
                np.allowed_methods.join(", ")
            };
            println!("  allowed_domains:    {domains}");
            println!("  allowed_methods:    {methods}");
            println!("  max_response_bytes: {}", np.max_response_bytes);
            println!("  timeout_ms:         {}", np.timeout_ms);
            println!("  allow_redirects:    {}", np.allow_redirects);
        }
        None => println!("Net policy: (default)"),
    }
}

fn run_metrics(
    cmd: MetricsCommand,
    env_arg: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        MetricsCommand::Export { data_dir } => {
            #[cfg(feature = "persist-sqlite")]
            {
                let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                let text = boruna_orchestrator::metrics::export(&resolved, env_arg)
                    .map_err(|e| format!("{e}"))?;
                // Write directly to stdout — no trailing newline
                // adjustment; format_prometheus already terminates
                // each line with \n. Operators redirect this to a
                // .prom file in node_exporter's textfile collector
                // directory.
                use std::io::Write;
                std::io::stdout().write_all(text.as_bytes())?;
            }
            #[cfg(not(feature = "persist-sqlite"))]
            {
                let _ = data_dir;
                return Err("`metrics export` requires the `persist-sqlite` feature".into());
            }
        }
    }
    Ok(())
}

fn run_capability(cmd: CapabilityCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        CapabilityCommand::List { json } => {
            let report =
                boruna_bytecode::capability_set_report("boruna", env!("CARGO_PKG_VERSION"));
            if json {
                let s = serde_json::to_string_pretty(&report)?;
                println!("{s}");
            } else {
                println!("{} {}", report.name, report.version);
                println!("capability_set_hash: {}", report.capability_set_hash);
                println!("protocol_version: {}", report.protocol_version);
                println!();
                println!("capabilities ({}):", report.capabilities.len());
                for cap in &report.capabilities {
                    println!("  {:<14} v{}", cap.name, cap.version);
                }
            }
        }
    }
    Ok(())
}

fn run_lang(cmd: LangCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        LangCommand::Check { file, json, output } => {
            let source = fs::read_to_string(&file)?;
            let file_str = file.display().to_string();
            let ds = DiagnosticCollector::new(&file_str, &source).collect();

            if json || output.is_some() {
                let json_str = ds.to_json();
                if let Some(out_path) = output {
                    fs::write(&out_path, &json_str)?;
                    println!("diagnostics written to {}", out_path.display());
                } else {
                    println!("{json_str}");
                }
            } else {
                let human = ds.to_human();
                if human.is_empty() {
                    println!("no diagnostics");
                } else {
                    print!("{human}");
                }
            }

            if ds.has_errors() {
                process::exit(1);
            }
        }
        LangCommand::Repair { file, from, apply } => {
            let source = fs::read_to_string(&file)?;
            let file_str = file.display().to_string();

            // Get diagnostics: from file or run check
            let ds = if let Some(diag_path) = from {
                let json = fs::read_to_string(&diag_path)?;
                serde_json::from_str(&json).map_err(|e| format!("invalid diagnostics JSON: {e}"))?
            } else {
                DiagnosticCollector::new(&file_str, &source).collect()
            };

            let (strategy, specific_id) = match apply.as_str() {
                "best" => (RepairStrategy::Best, None),
                "all" => (RepairStrategy::All, None),
                id => (RepairStrategy::ById, Some(id.to_string())),
            };

            let (repaired, result) =
                RepairTool::repair(&file_str, &source, &ds, strategy, specific_id.as_deref());

            if result.applied.is_empty() {
                println!("no patches applied");
                if !result.skipped.is_empty() {
                    for s in &result.skipped {
                        println!("  skipped {}: {}", s.diagnostic_id, s.reason);
                    }
                }
            } else {
                // Write repaired source
                fs::write(&file, &repaired)?;
                println!("applied {} patches:", result.applied.len());
                for a in &result.applied {
                    println!("  [{}] {}: {}", a.diagnostic_id, a.patch_id, a.description);
                }
                println!(
                    "diagnostics: {} -> {}",
                    result.diagnostics_before, result.diagnostics_after
                );
                if result.verify_passed {
                    println!("verify: PASS");
                } else {
                    println!("verify: FAIL (remaining issues)");
                }
            }
        }
    }
    Ok(())
}

fn run_trace2tests(cmd: Trace2TestsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        Trace2TestsCommand::Record {
            file,
            messages,
            out,
        } => {
            let source = fs::read_to_string(&file)?;
            let file_str = file.display().to_string();

            let msgs: Vec<AppMessage> = messages
                .split(',')
                .map(|s| {
                    let (tag, payload) = parse_message(s);
                    AppMessage::new(tag, payload)
                })
                .collect();

            let trace = trace2tests::record_trace(&source, &file_str, msgs)
                .map_err(|e| format!("record failed: {e}"))?;
            let json = serde_json::to_string_pretty(&trace)?;
            fs::write(&out, json)?;

            println!(
                "recorded {} cycles to {}",
                trace.cycles.len(),
                out.display()
            );
            println!("trace hash: {}", trace.trace_hash);
        }
        Trace2TestsCommand::Generate { trace, name, out } => {
            let trace_json = fs::read_to_string(&trace)?;
            let trace_file: trace2tests::TraceFile = serde_json::from_str(&trace_json)?;

            let spec = trace2tests::generate_test(&trace_file, &name);
            let json = serde_json::to_string_pretty(&spec)?;
            fs::write(&out, json)?;

            println!("generated test spec: {}", out.display());
            println!(
                "  {} messages, {} assertions",
                spec.messages.len(),
                spec.assertions.len()
            );
        }
        Trace2TestsCommand::Run { spec, source } => {
            let spec_json = fs::read_to_string(&spec)?;
            let test_spec: trace2tests::TestSpec = serde_json::from_str(&spec_json)?;

            let source_path = source.unwrap_or_else(|| PathBuf::from(&test_spec.source_file));
            let source_code = fs::read_to_string(&source_path)?;

            let result = trace2tests::run_test(&test_spec, &source_code);

            if result.passed {
                println!("PASS: {}", test_spec.name);
                for ar in &result.assertion_results {
                    println!("  [ok] {}", ar.kind);
                }
            } else {
                println!("FAIL: {}", test_spec.name);
                if let Some(err) = &result.error {
                    println!("  error: {err}");
                }
                for ar in &result.assertion_results {
                    let status = if ar.passed { "ok" } else { "FAIL" };
                    println!(
                        "  [{status}] {}: expected={}, actual={}",
                        ar.kind, ar.expected, ar.actual
                    );
                }
                process::exit(1);
            }
        }
        Trace2TestsCommand::Minimize {
            trace,
            source,
            predicate,
            out,
        } => {
            let trace_json = fs::read_to_string(&trace)?;
            let trace_file: trace2tests::TraceFile = serde_json::from_str(&trace_json)?;
            let source_code = fs::read_to_string(&source)?;

            let messages: Vec<trace2tests::TraceMessage> = trace_file
                .cycles
                .iter()
                .map(|c| c.message.clone())
                .collect();

            let original_len = messages.len();

            let minimal = match predicate.as_str() {
                "panic" => trace2tests::minimize_trace(
                    &source_code,
                    &messages,
                    &trace2tests::predicate_runtime_error,
                ),
                _ => {
                    // External command predicate
                    let cmd_str = predicate.clone();
                    let pred = move |_src: &str, msgs: &[trace2tests::TraceMessage]| {
                        external_predicate(&cmd_str, &trace_file.source_file, msgs)
                    };
                    trace2tests::minimize_trace(&source_code, &messages, &pred)
                }
            };

            println!("minimized: {} -> {} messages", original_len, minimal.len());

            // Record the minimal trace
            let app_msgs = trace2tests::messages_to_app(&minimal);
            let file_str = source.display().to_string();
            let minimal_trace = trace2tests::record_trace(&source_code, &file_str, app_msgs)
                .map_err(|e| format!("re-record failed: {e}"))?;
            let json = serde_json::to_string_pretty(&minimal_trace)?;

            let out_path = out.unwrap_or_else(|| {
                let stem = trace.with_extension("");
                PathBuf::from(format!("{}.min.json", stem.display()))
            });

            fs::write(&out_path, json)?;
            println!("minimized trace: {}", out_path.display());
            println!("trace hash: {}", minimal_trace.trace_hash);
        }
    }
    Ok(())
}

fn external_predicate(
    command: &str,
    source_file: &str,
    messages: &[trace2tests::TraceMessage],
) -> trace2tests::PredicateOutcome {
    let temp_file = match tempfile::NamedTempFile::new() {
        Ok(f) => f,
        Err(_) => return trace2tests::PredicateOutcome::Unresolved,
    };
    let temp_path = temp_file.path().to_path_buf();
    let trace_data = serde_json::json!({
        "source_file": source_file,
        "messages": messages,
    });
    if fs::write(
        &temp_path,
        serde_json::to_string(&trace_data).unwrap_or_default(),
    )
    .is_err()
    {
        return trace2tests::PredicateOutcome::Unresolved;
    }

    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        return trace2tests::PredicateOutcome::Unresolved;
    }

    let output = std::process::Command::new(parts[0])
        .args(&parts[1..])
        .arg(temp_path.to_string_lossy().as_ref())
        .output();

    match output {
        Ok(o) if o.status.success() => trace2tests::PredicateOutcome::Pass,
        Ok(_) => trace2tests::PredicateOutcome::Fail,
        Err(_) => trace2tests::PredicateOutcome::Unresolved,
    }
}

fn run_framework(cmd: FrameworkCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        FrameworkCommand::New { name, dir } => {
            let base = dir.unwrap_or_else(|| PathBuf::from("."));
            let app_dir = base.join(&name);
            fs::create_dir_all(&app_dir)?;

            let source = format!(
                r#"// {name} — Boruna Framework App

type State {{ value: Int }}
type Msg {{ tag: String, payload: Int }}
type Effect {{ kind: String, payload: String, callback_tag: String }}
type UpdateResult {{ state: State, effects: List<Effect> }}
type UINode {{ tag: String, text: String }}

fn init() -> State {{
    State {{ value: 0 }}
}}

fn update(state: State, msg: Msg) -> UpdateResult {{
    UpdateResult {{
        state: State {{ value: state.value + msg.payload }},
        effects: [],
    }}
}}

fn view(state: State) -> UINode {{
    UINode {{ tag: "text", text: "value" }}
}}

fn main() -> Int {{
    let s: State = init()
    s.value
}}
"#
            );

            let file_path = app_dir.join(format!("{name}.ax"));
            fs::write(&file_path, source)?;
            println!("created framework app: {}", file_path.display());
            println!("  run: boruna framework validate {}", file_path.display());
            println!("  run: boruna run {}", file_path.display());
        }
        FrameworkCommand::Validate { file } => {
            let source = fs::read_to_string(&file)?;
            let tokens = boruna_compiler::lexer::lex(&source)?;
            let program = boruna_compiler::parser::parse(tokens)?;

            match AppValidator::validate(&program) {
                Ok(result) => {
                    println!("valid App protocol");
                    println!(
                        "  init:     {}",
                        if result.has_init { "yes" } else { "MISSING" }
                    );
                    println!(
                        "  update:   {}",
                        if result.has_update { "yes" } else { "MISSING" }
                    );
                    println!(
                        "  view:     {}",
                        if result.has_view { "yes" } else { "MISSING" }
                    );
                    println!(
                        "  policies: {}",
                        if result.has_policies {
                            "yes"
                        } else {
                            "none (using defaults)"
                        }
                    );
                    if let Some(t) = &result.state_type {
                        println!("  state type: {t}");
                    }
                    if let Some(t) = &result.message_type {
                        println!("  message type: {t}");
                    }
                }
                Err(e) => {
                    eprintln!("validation failed: {e}");
                    process::exit(1);
                }
            }
        }
        FrameworkCommand::Test { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let mut harness = TestHarness::from_source(&source)?;

            println!("init state: {}", harness.state());

            if let Some(msgs) = messages {
                for msg_str in msgs.split(',') {
                    let (tag, payload) = parse_message(msg_str);
                    let msg = AppMessage::new(tag, payload);
                    match harness.send(msg) {
                        Ok((state, effects)) => {
                            println!("cycle {}: state={}", harness.cycle(), state);
                            if !effects.is_empty() {
                                println!("  effects: {}", effects.len());
                                for e in &effects {
                                    println!("    - {:?} -> {}", e.kind, e.callback_tag);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("error at cycle {}: {e}", harness.cycle());
                            process::exit(1);
                        }
                    }
                }
            }

            println!("\nfinal state: {}", harness.state());
            println!("cycles: {}", harness.cycle());

            // Show view
            match harness.view() {
                Ok(ui) => println!("view: {ui}"),
                Err(e) => eprintln!("view error: {e}"),
            }
        }
        FrameworkCommand::InspectState { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let mut harness = TestHarness::from_source(&source)?;

            if let Some(msgs) = messages {
                for msg_str in msgs.split(',') {
                    let (tag, payload) = parse_message(msg_str);
                    harness.send(AppMessage::new(tag, payload))?;
                }
            }

            println!("--- State Inspection ---");
            println!("cycle: {}", harness.cycle());
            println!("state: {}", harness.state());
            println!("\nJSON snapshot:");
            println!("{}", harness.snapshot());

            if harness.cycle() > 0 {
                println!("\n--- Diff from init ---");
                let log = harness.cycle_log();
                if let (Some(first), Some(last)) = (log.first(), log.last()) {
                    let diffs = boruna_framework::state::StateMachine::diff_values(
                        &first.state_before,
                        &last.state_after,
                    );
                    if diffs.is_empty() {
                        println!("  no changes");
                    } else {
                        for d in &diffs {
                            println!(
                                "  field[{}]: {} -> {}",
                                d.field_index, d.old_value, d.new_value
                            );
                        }
                    }
                }
            }
        }
        FrameworkCommand::Simulate { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let mut harness = TestHarness::from_source(&source)?;

            println!("=== Simulation ===");
            println!("init: {}", harness.state());
            println!();

            for (i, msg_str) in messages.split(',').enumerate() {
                let (tag, payload) = parse_message(msg_str);
                let msg = AppMessage::new(tag.clone(), payload);
                match harness.send(msg) {
                    Ok((state, effects)) => {
                        println!("step {}: msg=\"{tag}\"", i + 1);
                        println!("  state: {state}");
                        if !effects.is_empty() {
                            for e in &effects {
                                println!("  effect: {:?}", e.kind);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("step {} failed: {e}", i + 1);
                        process::exit(1);
                    }
                }
            }

            println!("\n=== Final ===");
            println!("state: {}", harness.state());
            println!("cycles: {}", harness.cycle());
        }
        FrameworkCommand::Inspect { file, json } => {
            let source = fs::read_to_string(&file)?;
            let tokens = boruna_compiler::lexer::lex(&source)?;
            let program = boruna_compiler::parser::parse(tokens)?;
            let result = AppValidator::validate(&program)?;

            if json {
                let output = serde_json::json!({
                    "file": file.display().to_string(),
                    "has_init": result.has_init,
                    "has_update": result.has_update,
                    "has_view": result.has_view,
                    "has_policies": result.has_policies,
                    "state_type": result.state_type,
                    "message_type": result.message_type,
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("=== App Contract ===");
                println!("file: {}", file.display());
                println!(
                    "init:     {}",
                    if result.has_init { "yes" } else { "MISSING" }
                );
                println!(
                    "update:   {}",
                    if result.has_update { "yes" } else { "MISSING" }
                );
                println!(
                    "view:     {}",
                    if result.has_view { "yes" } else { "MISSING" }
                );
                println!(
                    "policies: {}",
                    if result.has_policies {
                        "yes"
                    } else {
                        "none (defaults)"
                    }
                );
                if let Some(t) = &result.state_type {
                    println!("state type:   {t}");
                }
                if let Some(t) = &result.message_type {
                    println!("message type: {t}");
                }
            }
        }
        FrameworkCommand::Diag { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let harness_result = TestHarness::from_source(&source);

            let mut diag = serde_json::json!({
                "file": file.display().to_string(),
                "status": "ok",
            });

            match harness_result {
                Ok(mut harness) => {
                    diag["init_state"] = serde_json::json!(format!("{}", harness.state()));

                    let mut cycles = Vec::new();
                    let mut errors = Vec::new();

                    if let Some(msgs) = messages {
                        for msg_str in msgs.split(',') {
                            let (tag, payload) = parse_message(msg_str);
                            match harness.send(AppMessage::new(tag.clone(), payload)) {
                                Ok((state, effects)) => {
                                    let effect_kinds: Vec<&str> =
                                        effects.iter().map(|e| e.kind.as_str()).collect();
                                    cycles.push(serde_json::json!({
                                        "cycle": harness.cycle(),
                                        "message": tag,
                                        "state": format!("{state}"),
                                        "effects": effect_kinds,
                                    }));
                                }
                                Err(e) => {
                                    errors.push(boruna_framework::policy::error_to_json(&e));
                                    break;
                                }
                            }
                        }
                    }

                    diag["cycles"] = serde_json::json!(cycles);
                    diag["final_state"] = serde_json::json!(format!("{}", harness.state()));
                    diag["final_cycle"] = serde_json::json!(harness.cycle());
                    diag["snapshot"] = serde_json::json!(harness.snapshot());

                    if !errors.is_empty() {
                        diag["status"] = serde_json::json!("error");
                        diag["errors"] = serde_json::json!(errors);
                    }
                }
                Err(e) => {
                    diag["status"] = serde_json::json!("error");
                    diag["error"] = serde_json::json!(format!("{e}"));
                }
            }

            println!("{}", serde_json::to_string_pretty(&diag)?);
        }
        FrameworkCommand::TraceHash { file, messages } => {
            let source = fs::read_to_string(&file)?;
            let mut harness = TestHarness::from_source(&source)?;

            if let Some(msgs) = messages {
                for msg_str in msgs.split(',') {
                    let (tag, payload) = parse_message(msg_str);
                    harness.send(AppMessage::new(tag, payload))?;
                }
            }

            // Build a stable trace string from the cycle log
            let mut trace = String::new();
            for r in harness.cycle_log() {
                trace.push_str(&format!(
                    "c{}:{}:{},before={},after={},fx=[{}]\n",
                    r.cycle,
                    r.message.tag,
                    r.message.payload,
                    r.state_before,
                    r.state_after,
                    r.effects
                        .iter()
                        .map(|e| e.kind.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                ));
            }

            // SHA-256 hash (consistent with trace2tests)
            let hash = boruna_tooling::trace2tests::sha256_hex(&trace);

            println!("{}", hash);
            println!("cycles: {}", harness.cycle());
            println!("state: {}", harness.state());
        }
        #[cfg(feature = "serve")]
        FrameworkCommand::Serve { file, port } => {
            serve::run_serve(file, port)?;
        }
        FrameworkCommand::Replay { file, log } => {
            let source = fs::read_to_string(&file)?;
            let log_json = fs::read_to_string(&log)?;

            // Parse the cycle log
            let recorded: Vec<serde_json::Value> = serde_json::from_str(&log_json)
                .map_err(|e| format!("invalid cycle log JSON: {e}"))?;

            let mut harness = TestHarness::from_source(&source)?;
            let mut mismatches = Vec::new();

            for (i, entry) in recorded.iter().enumerate() {
                let tag = entry["message"].as_str().unwrap_or("unknown");
                let payload_str = entry.get("payload").and_then(|v| v.as_str()).unwrap_or("0");
                let (_, payload) = parse_message(&format!("{tag}:{payload_str}"));

                match harness.send(AppMessage::new(tag, payload)) {
                    Ok((state, _)) => {
                        let expected_state = entry
                            .get("state_after")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        let actual_state = format!("{state}");
                        if !expected_state.is_empty() && actual_state != expected_state {
                            mismatches.push(format!(
                                "cycle {}: expected state {}, got {}",
                                i + 1,
                                expected_state,
                                actual_state
                            ));
                        }
                    }
                    Err(e) => {
                        mismatches.push(format!("cycle {} failed: {e}", i + 1));
                        break;
                    }
                }
            }

            if mismatches.is_empty() {
                println!("replay: IDENTICAL");
                println!("cycles: {}", harness.cycle());
                println!("state: {}", harness.state());
            } else {
                println!("replay: DIVERGED");
                for m in &mismatches {
                    println!("  {m}");
                }
                process::exit(1);
            }
        }
    }
    Ok(())
}

/// Parse "tag:payload" into (tag, Value).
/// payload is parsed as Int if numeric, otherwise String.
fn parse_message(s: &str) -> (String, boruna_bytecode::Value) {
    let s = s.trim();
    if let Some(idx) = s.find(':') {
        let tag = s[..idx].to_string();
        let payload_str = s[idx + 1..].trim();
        let payload = if let Ok(n) = payload_str.parse::<i64>() {
            boruna_bytecode::Value::Int(n)
        } else {
            boruna_bytecode::Value::String(payload_str.to_string())
        };
        (tag, payload)
    } else {
        (s.to_string(), boruna_bytecode::Value::Int(0))
    }
}

fn load_module(path: &PathBuf) -> Result<Module, Box<dyn std::error::Error>> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    match ext {
        "axbc" => {
            let data = fs::read(path)?;
            Ok(Module::from_bytes(&data)?)
        }
        "ax" => {
            let source = fs::read_to_string(path)?;
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "module".into());
            Ok(boruna_compiler::compile(&name, &source)?)
        }
        _ => Err(format!("unknown file extension: {ext}").into()),
    }
}

fn make_gateway(
    policy_str: &str,
    live: bool,
    record_net_to: Option<&std::path::Path>,
    replay_net_from: Option<&std::path::Path>,
) -> Result<CapabilityGateway, Box<dyn std::error::Error>> {
    let policy = match policy_str {
        "allow-all" => Policy::allow_all(),
        "deny-all" => Policy::deny_all(),
        path => {
            // Strict validation: schema_version, deny-extra,
            // capability catalog, net_policy bounds. Failures
            // surface a stable `error_kind` string in the message
            // (`policy.unknown_field`, `policy.invalid_capability`,
            // etc.) — see `docs/design-policy-as-code.md`.
            boruna_vm::policy_validate::parse_file(std::path::Path::new(path))?
        }
    };

    // Replay takes precedence over both --live and --record-net-to. The clap
    // `conflicts_with` attribute already prevents --record-net-to and
    // --replay-net-from from coexisting; the additional check here protects
    // against future callers of `make_gateway` who bypass the CLI parser.
    if let Some(tape_path) = replay_net_from {
        if record_net_to.is_some() {
            return Err("--record-net-to and --replay-net-from are mutually exclusive".into());
        }
        #[cfg(feature = "http")]
        {
            let tape = boruna_vm::net_record_replay::NetTape::load(tape_path)?;
            if live {
                eprintln!(
                    "warning: --live is ignored when --replay-net-from is set \
                     (replay serves all net.fetch calls from the tape, no real network access)"
                );
            }
            return Ok(CapabilityGateway::with_handler(
                policy,
                Box::new(boruna_vm::net_record_replay::ReplayingHttpHandler::new(
                    tape,
                )),
            ));
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = tape_path;
            return Err(
                "--replay-net-from requires the `http` feature; rebuild with --features boruna-cli/http".into(),
            );
        }
    }

    if let Some(tape_path) = record_net_to {
        if !live {
            return Err(
                "--record-net-to requires --live (recording needs real HTTP calls to record)"
                    .into(),
            );
        }
        #[cfg(feature = "http")]
        {
            // Fail fast: probe write access on the tape path BEFORE running
            // the script. Save-on-drop logs but cannot signal failure to
            // the process exit code, so a CI pipeline like
            //   `boruna run ... --record-net-to fixtures/x && verify x`
            // would otherwise see a successful exit AND a missing/stale
            // tape file. Write an empty placeholder tape; the recorder
            // overwrites it on Drop with the real content.
            let placeholder = boruna_vm::net_record_replay::NetTape::new();
            placeholder.save(tape_path).map_err(|e| {
                format!(
                    "--record-net-to: cannot write to '{}': {e}",
                    tape_path.display()
                )
            })?;

            let net_policy = policy.net_policy.clone().unwrap_or_default();
            let inner = boruna_vm::http_handler::HttpHandler::new(net_policy);
            let recorder = boruna_vm::net_record_replay::RecordingHttpHandler::with_save_path(
                inner,
                tape_path.to_path_buf(),
            );
            return Ok(CapabilityGateway::with_handler(policy, Box::new(recorder)));
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = tape_path;
            return Err(
                "--record-net-to requires the `http` feature; rebuild with --features boruna-cli/http".into(),
            );
        }
    }

    if live {
        #[cfg(feature = "http")]
        {
            let net_policy = policy.net_policy.clone().unwrap_or_default();
            return Ok(CapabilityGateway::with_handler(
                policy,
                Box::new(boruna_vm::http_handler::HttpHandler::new(net_policy)),
            ));
        }
        #[cfg(not(feature = "http"))]
        {
            eprintln!("warning: --live requires the `http` feature; falling back to mock handler");
        }
    }

    Ok(CapabilityGateway::new(policy))
}

fn run_workflow(
    cmd: WorkflowCommand,
    env_arg: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    use boruna_orchestrator::audit::{AuditEvent, AuditLog, EvidenceBundleBuilder};
    use boruna_orchestrator::workflow::{
        RunOptions, WorkflowDef, WorkflowRunner, WorkflowValidator,
    };

    match cmd {
        WorkflowCommand::Validate { dir, print_hash } => {
            let def_path = dir.join("workflow.json");
            let json = fs::read_to_string(&def_path)
                .map_err(|e| format!("cannot read {}: {e}", def_path.display()))?;
            let def: WorkflowDef =
                serde_json::from_str(&json).map_err(|e| format!("invalid workflow.json: {e}"))?;

            match WorkflowValidator::validate(&def) {
                Ok(()) => {
                    let order = WorkflowValidator::topological_order(&def)?;
                    println!("workflow '{}' v{} is valid", def.name, def.version);
                    println!("  steps: {}", def.steps.len());
                    println!("  edges: {}", def.edges.len());
                    println!("  execution order: {}", order.join(" -> "));
                    // 0.3-S9: capture-friendly hash output for CI/CD.
                    // Format: `workflow_hash=<hex>` on its own line so
                    // `cut -d= -f2` extracts cleanly.
                    if print_hash {
                        let hash = WorkflowRunner::workflow_hash_from_def(&def);
                        println!("workflow_hash={hash}");
                    }
                }
                Err(errors) => {
                    eprintln!("validation failed:");
                    for err in &errors {
                        eprintln!("  {err}");
                    }
                    process::exit(1);
                }
            }
        }
        WorkflowCommand::Run {
            dir,
            policy,
            record,
            evidence_dir,
            live,
            data_dir,
            ephemeral,
            concurrency,
            skip_if_running,
            submit_only,
            coordinator,
            coord_token,
            coord_poll_interval_ms,
            coord_max_wait_secs,
            expect_workflow_hash,
        } => {
            if concurrency == 0 {
                return Err("--concurrency must be >= 1 (got 0); use 1 for sequential".into());
            }
            let def_path = dir.join("workflow.json");
            let json = fs::read_to_string(&def_path)
                .map_err(|e| format!("cannot read {}: {e}", def_path.display()))?;
            let def: WorkflowDef =
                serde_json::from_str(&json).map_err(|e| format!("invalid workflow.json: {e}"))?;

            // 0.3-S9: pre-flight hash check. Refuse before any
            // side effect (no run row, no checkpoints) if the
            // operator-supplied expected hash doesn't match the
            // on-disk def.
            if let Some(expected) = &expect_workflow_hash {
                check_workflow_hash_expectation(&def, expected)?;
            }

            let policy_obj = match policy.as_str() {
                "allow-all" => Policy::allow_all(),
                "deny-all" => Policy::deny_all(),
                path => {
                    // Sprint 0.4-S15: route through the strict
                    // validator so `workflow run` shares one parser
                    // with `boruna policy validate` and `boruna run`.
                    boruna_vm::policy_validate::parse_file(std::path::Path::new(path))?
                }
            };

            // Sprint 0.5-S4: --coordinator branches off the local-
            // run path entirely. Build the inline submit payload,
            // POST it, then poll status until terminal. Exit with
            // the conventional code (0/1/2) and skip the rest of
            // the local-run flow.
            if let Some(coord_url) = coordinator {
                #[cfg(feature = "serve")]
                {
                    let exit = crate::coordinator::run_remote(
                        &def,
                        &dir,
                        &policy_obj,
                        &coord_url,
                        coord_token.as_deref(),
                        coord_poll_interval_ms,
                        coord_max_wait_secs,
                    )?;
                    process::exit(exit);
                }
                #[cfg(not(feature = "serve"))]
                {
                    let _ = (
                        coord_url,
                        coord_token,
                        coord_poll_interval_ms,
                        coord_max_wait_secs,
                    );
                    return Err("`workflow run --coordinator` requires the `serve` feature".into());
                }
            }

            let options = RunOptions {
                policy: Some(policy_obj.clone()),
                record,
                workflow_dir: dir.display().to_string(),
                live,
                concurrency,
                submit_only,
            };

            let result = if ephemeral {
                WorkflowRunner::run(&def, &options).map_err(|e| format!("{e}"))?
            } else {
                #[cfg(feature = "persist-sqlite")]
                {
                    let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                    // 0.3-S7 → 0.3-S10: cron-friendly idempotent
                    // invocation. With --skip-if-running, the
                    // (check-in-flight + insert) sequence runs as a
                    // single SQL transaction so concurrent operators
                    // can't both pass the check and both insert.
                    // Returns Ok(None) on skip, Ok(Some(result)) on
                    // executed run.
                    if skip_if_running {
                        println!("  data_dir: {}", resolved.display());
                        match WorkflowRunner::run_persistent_or_skip(&def, &options, &resolved)
                            .map_err(|e| format!("{e}"))?
                        {
                            Some(result) => result,
                            None => {
                                // Look up the prior run for the skip
                                // message. This is a separate read
                                // outside the atomic write txn, but
                                // it's purely informational — by the
                                // time we get here, the writer has
                                // already chosen to skip, so even if
                                // the prior just terminated the
                                // operator's intent (don't double-
                                // run) was honored.
                                if let Some(prior) =
                                    boruna_orchestrator::workflow::find_in_flight_runs(
                                        &resolved, &def,
                                    )
                                    .map_err(|e| format!("{e}"))?
                                    .first()
                                {
                                    eprintln!(
                                        "skipped: workflow '{}' has {} run '{}' \
                                         (started_at_ms={}); not starting a new run",
                                        prior.workflow_name,
                                        prior.status.as_str(),
                                        prior.run_id,
                                        prior.started_at_ms,
                                    );
                                } else {
                                    eprintln!(
                                        "skipped: workflow '{}' had an in-flight prior at \
                                         insert-check time (since terminated); not starting a \
                                         new run",
                                        def.name
                                    );
                                }
                                return Ok(());
                            }
                        }
                    } else {
                        println!("  data_dir: {}", resolved.display());
                        WorkflowRunner::run_persistent(&def, &options, &resolved)
                            .map_err(|e| format!("{e}"))?
                    }
                }
                // Reject-at-parse per project-conventions §1: a binary
                // built without `persist-sqlite` cannot honor a
                // persistent run; rather than silently downgrading to
                // ephemeral (and creating no `runs.db`), surface the
                // contract mismatch as an error so the operator can
                // either rebuild with the feature or pass `--ephemeral`.
                #[cfg(not(feature = "persist-sqlite"))]
                {
                    let _ = (data_dir, skip_if_running);
                    return Err(format!(
                        "persistent runs require the `persist-sqlite` feature \
                         (rebuild with default features, or pass `--ephemeral`)"
                    )
                    .into());
                }
            };

            println!("workflow '{}' run: {:?}", def.name, result.status);
            println!("  run_id: {}", result.run_id);
            println!("  duration: {}ms", result.total_duration_ms);
            for (id, sr) in &result.step_results {
                println!("  step '{id}': {:?} ({}ms)", sr.status, sr.duration_ms);
                if let Some(err) = &sr.error {
                    println!("    error: {err}");
                }
            }

            if record {
                let ev_dir = evidence_dir.unwrap_or_else(|| dir.join("evidence"));
                let mut builder = EvidenceBundleBuilder::new(&ev_dir, &result.run_id, &def.name)?;

                builder.add_workflow_def(&json)?;
                let policy_json = serde_json::to_string_pretty(&policy_obj)?;
                builder.add_policy(&policy_json)?;

                // Build audit log from results
                let mut audit = AuditLog::new();
                audit.append(AuditEvent::WorkflowStarted {
                    workflow_hash: boruna_orchestrator::workflow::DataStore::hash_value(
                        &boruna_bytecode::Value::String(json.clone()),
                    ),
                    policy_hash: boruna_orchestrator::workflow::DataStore::hash_value(
                        &boruna_bytecode::Value::String(policy_json.clone()),
                    ),
                });

                for (id, sr) in &result.step_results {
                    match &sr.status {
                        boruna_orchestrator::workflow::StepStatus::Completed => {
                            audit.append(AuditEvent::StepCompleted {
                                step_id: id.clone(),
                                output_hash: sr.output_hash.clone().unwrap_or_default(),
                                duration_ms: sr.duration_ms,
                            });
                        }
                        boruna_orchestrator::workflow::StepStatus::Failed => {
                            audit.append(AuditEvent::StepFailed {
                                step_id: id.clone(),
                                error: sr.error.clone().unwrap_or_default(),
                            });
                        }
                        _ => {}
                    }
                }

                audit.append(AuditEvent::WorkflowCompleted {
                    result_hash: format!("{:?}", result.status),
                    total_duration_ms: result.total_duration_ms,
                });

                let manifest = builder.finalize(&audit)?;
                println!(
                    "\nevidence bundle: {}",
                    ev_dir.join(&result.run_id).display()
                );
                println!("  bundle_hash: {}", manifest.bundle_hash);
                println!("  audit_log_hash: {}", manifest.audit_log_hash);
                println!("  files: {}", manifest.file_checksums.len());
            }
        }
        WorkflowCommand::Resume {
            run_id,
            data_dir,
            workflow_dir,
            policy,
            live,
            concurrency,
            expect_workflow_hash,
        } => {
            if concurrency == 0 {
                return Err("--concurrency must be >= 1 (got 0); use 1 for sequential".into());
            }
            #[cfg(feature = "persist-sqlite")]
            {
                use boruna_orchestrator::workflow::ResumeOptions;
                let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                println!("resuming run '{run_id}' from {}", resolved.display());

                // 0.3-S9: pre-flight expected-hash check. The
                // resume function ALSO checks the workflow_hash
                // against the persisted run's stored hash; this
                // additional check verifies the operator's intent
                // (deploy-time captured hash) before either side-
                // effect path. Requires --workflow-dir to know
                // which on-disk def to hash; falls back to the
                // metadata-stored path if not supplied (matches
                // resume's normal behavior).
                if let Some(expected) = &expect_workflow_hash {
                    let wf_dir = workflow_dir.as_deref().ok_or_else(|| {
                        "--expect-workflow-hash on resume requires --workflow-dir to \
                             locate the on-disk def to hash"
                            .to_string()
                    })?;
                    let def_path = wf_dir.join("workflow.json");
                    let json = fs::read_to_string(&def_path)
                        .map_err(|e| format!("cannot read {}: {e}", def_path.display()))?;
                    let def: WorkflowDef = serde_json::from_str(&json)
                        .map_err(|e| format!("invalid workflow.json: {e}"))?;
                    check_workflow_hash_expectation(&def, expected)?;
                }

                let policy_obj = match policy.as_deref() {
                    None => None,
                    Some("allow-all") => Some(Policy::allow_all()),
                    Some("deny-all") => Some(Policy::deny_all()),
                    Some(path) => {
                        // Sprint 0.4-S15: route through the strict
                        // validator so `workflow resume` shares one
                        // parser with the other CLI policy paths.
                        Some(boruna_vm::policy_validate::parse_file(
                            std::path::Path::new(path),
                        )?)
                    }
                };

                let options = ResumeOptions {
                    policy: policy_obj,
                    record: false,
                    live,
                    workflow_dir_override: workflow_dir.map(|p| p.display().to_string()),
                    concurrency,
                };
                let result = WorkflowRunner::resume(&run_id, &resolved, &options)
                    .map_err(|e| format!("{e}"))?;
                println!(
                    "workflow '{}' resume: {:?}",
                    result.workflow_name, result.status
                );
                println!("  run_id: {}", result.run_id);
                println!("  duration: {}ms", result.total_duration_ms);
                for (id, sr) in &result.step_results {
                    println!("  step '{id}': {:?} ({}ms)", sr.status, sr.duration_ms);
                    if let Some(err) = &sr.error {
                        println!("    error: {err}");
                    }
                }
            }
            #[cfg(not(feature = "persist-sqlite"))]
            {
                let _ = (run_id, data_dir, workflow_dir, policy, live);
                return Err("`workflow resume` requires the `persist-sqlite` feature \
                            (on by default in boruna-orchestrator)"
                    .into());
            }
        }
        WorkflowCommand::Approve {
            run_id,
            step_id,
            data_dir,
            coordinator,
            coord_token,
        } => {
            if let Some(url) = coordinator {
                #[cfg(feature = "serve")]
                {
                    crate::coordinator::send_approve_remote(
                        &url,
                        coord_token.as_deref(),
                        &run_id,
                        &step_id,
                        "approved",
                        None,
                    )?;
                    println!(
                        "approval recorded for step '{step_id}' in run '{run_id}' \
                         via coordinator {url}."
                    );
                }
                #[cfg(not(feature = "serve"))]
                {
                    let _ = (url, coord_token);
                    return Err(
                        "`workflow approve --coordinator` requires the `serve` feature".into(),
                    );
                }
            } else {
                #[cfg(feature = "persist-sqlite")]
                {
                    use boruna_orchestrator::workflow::{record_approval_decision, ApprovalKind};
                    let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                    record_approval_decision(
                        &resolved,
                        &run_id,
                        &step_id,
                        ApprovalKind::Approved,
                        None,
                    )
                    .map_err(|e| format!("{e}"))?;
                    println!("approval recorded for step '{step_id}' in run '{run_id}'.");
                    println!(
                        "Run `boruna workflow resume {run_id} --data-dir {}` to advance.",
                        resolved.display()
                    );
                }
                #[cfg(not(feature = "persist-sqlite"))]
                {
                    let _ = (run_id, step_id, data_dir);
                    return Err("`workflow approve` requires the `persist-sqlite` feature".into());
                }
            }
        }
        WorkflowCommand::Reject {
            run_id,
            step_id,
            reason,
            data_dir,
            coordinator,
            coord_token,
        } => {
            if let Some(url) = coordinator {
                #[cfg(feature = "serve")]
                {
                    crate::coordinator::send_approve_remote(
                        &url,
                        coord_token.as_deref(),
                        &run_id,
                        &step_id,
                        "rejected",
                        reason.as_deref(),
                    )?;
                    println!(
                        "rejection recorded for step '{step_id}' in run '{run_id}' \
                         via coordinator {url}."
                    );
                }
                #[cfg(not(feature = "serve"))]
                {
                    let _ = (url, coord_token, reason);
                    return Err(
                        "`workflow reject --coordinator` requires the `serve` feature".into(),
                    );
                }
            } else {
                #[cfg(feature = "persist-sqlite")]
                {
                    use boruna_orchestrator::workflow::{record_approval_decision, ApprovalKind};
                    let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                    record_approval_decision(
                        &resolved,
                        &run_id,
                        &step_id,
                        ApprovalKind::Rejected,
                        reason,
                    )
                    .map_err(|e| format!("{e}"))?;
                    println!("rejection recorded for step '{step_id}' in run '{run_id}'.");
                    println!(
                        "Run `boruna workflow resume {run_id} --data-dir {}` to halt the run.",
                        resolved.display()
                    );
                }
                #[cfg(not(feature = "persist-sqlite"))]
                {
                    let _ = (run_id, step_id, reason, data_dir);
                    return Err("`workflow reject` requires the `persist-sqlite` feature".into());
                }
            }
        }
        WorkflowCommand::Trigger {
            run_id,
            step_id,
            token,
            payload,
            payload_file,
            data_dir,
            coordinator,
            coord_token,
        } => {
            let payload_str = match (payload, payload_file) {
                (Some(p), None) => p,
                (None, Some(path)) => std::fs::read_to_string(&path)
                    .map_err(|e| format!("cannot read --payload-file '{}': {e}", path.display()))?,
                (Some(_), Some(_)) => {
                    return Err("--payload and --payload-file are mutually exclusive".into());
                }
                (None, None) => {
                    return Err(
                        "either --payload or --payload-file is required for `workflow trigger`"
                            .into(),
                    );
                }
            };
            // Defense-in-depth: confirm the payload is well-formed JSON.
            // Same posture for both local and remote paths so operators
            // get the early failure regardless of mode.
            serde_json::from_str::<serde_json::Value>(&payload_str)
                .map_err(|e| format!("--payload is not valid JSON: {e}"))?;

            if let Some(url) = coordinator {
                #[cfg(feature = "serve")]
                {
                    crate::coordinator::send_trigger_remote(
                        &url,
                        coord_token.as_deref(),
                        &run_id,
                        &step_id,
                        &token,
                        &payload_str,
                    )?;
                    println!(
                        "trigger recorded for step '{step_id}' in run '{run_id}' \
                         via coordinator {url}."
                    );
                }
                #[cfg(not(feature = "serve"))]
                {
                    let _ = (url, coord_token);
                    return Err(
                        "`workflow trigger --coordinator` requires the `serve` feature".into(),
                    );
                }
            } else {
                #[cfg(feature = "persist-sqlite")]
                {
                    use boruna_orchestrator::workflow::record_external_trigger;
                    let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                    record_external_trigger(&resolved, &run_id, &step_id, &token, &payload_str)
                        .map_err(|e| format!("{e}"))?;
                    println!("trigger recorded for step '{step_id}' in run '{run_id}'.");
                    println!(
                        "Run `boruna workflow resume {run_id} --data-dir {}` to advance.",
                        resolved.display()
                    );
                }
                #[cfg(not(feature = "persist-sqlite"))]
                {
                    let _ = (run_id, step_id, token, data_dir);
                    return Err("`workflow trigger` requires the `persist-sqlite` feature".into());
                }
            }
        }
        WorkflowCommand::Show {
            run_id,
            json,
            data_dir,
        } => {
            #[cfg(feature = "persist-sqlite")]
            {
                let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                let detail = boruna_orchestrator::workflow::show_run(&resolved, &run_id)
                    .map_err(|e| format!("{e}"))?;
                if json {
                    // Hand-build the JSON shape so the field names are
                    // stable (do NOT serde-flatten internal types whose
                    // shape may drift). Sorted by step_id for
                    // deterministic output.
                    let mut steps: Vec<&_> = detail.checkpoints.iter().collect();
                    steps.sort_by(|a, b| a.step_id.cmp(&b.step_id));
                    let steps_json: Vec<serde_json::Value> = steps
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "step_id": c.step_id,
                                "status": c.status.as_str(),
                                "output_hash": c.output_hash,
                                "output_json_preview": c.output_json.as_ref().map(|s| {
                                    truncate_at_char_boundary(s, 200)
                                }),
                                "started_at_ms": c.started_at_ms,
                                "ended_at_ms": c.ended_at_ms,
                                "error_msg": c.error_msg,
                                // 0.3-S12: per-step retry attempt count
                                // (column added in 0.3-S11 schema v2).
                                // 1 = first-try; >1 = retry policy fired.
                                "attempt_count": c.attempt_count,
                            })
                        })
                        .collect();
                    let approvals_json: Vec<serde_json::Value> = detail
                        .approvals
                        .iter()
                        .map(|a| {
                            serde_json::json!({
                                "step_id": a.step_id,
                                "decision": match a.decision {
                                    boruna_orchestrator::workflow::ApprovalKind::Approved => "approved",
                                    boruna_orchestrator::workflow::ApprovalKind::Rejected => "rejected",
                                },
                                "decided_at_ms": a.decided_at_ms,
                                "reason": a.reason,
                            })
                        })
                        .collect();
                    let out = serde_json::json!({
                        "run": {
                            "run_id": detail.run.run_id,
                            "workflow_name": detail.run.workflow_name,
                            "workflow_hash": detail.run.workflow_hash,
                            "status": detail.run.status.as_str(),
                            "started_at_ms": detail.run.started_at_ms,
                            "updated_at_ms": detail.run.updated_at_ms,
                        },
                        "steps": steps_json,
                        "approvals": approvals_json,
                        // Reviewed 0.3-S3 H5: surface metadata parse
                        // failures programmatically so jq pipelines can
                        // detect corruption (stderr is dropped when
                        // stdout is piped).
                        "metadata_parse_error": detail.metadata_parse_error,
                    });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("=== Run ===");
                    println!("  run_id:        {}", detail.run.run_id);
                    println!("  workflow:      {}", detail.run.workflow_name);
                    println!("  workflow_hash: {}", detail.run.workflow_hash);
                    println!("  status:        {}", detail.run.status.as_str());
                    println!("  started_at_ms: {}", detail.run.started_at_ms);
                    println!("  updated_at_ms: {}", detail.run.updated_at_ms);
                    println!();
                    println!("=== Steps ===");
                    if detail.checkpoints.is_empty() {
                        println!("  (none)");
                    } else {
                        let mut steps: Vec<&_> = detail.checkpoints.iter().collect();
                        steps.sort_by(|a, b| a.step_id.cmp(&b.step_id));
                        // 0.3-S12: ATTEMPTS column surfaces the
                        // per-step retry count (1 = first-try, >1 =
                        // retry policy fired). Operator visibility
                        // signal for triage of flaky steps.
                        println!(
                            "  {:<24} {:<20} {:<8} {:<14} {:<14} {:<24}",
                            "STEP_ID",
                            "STATUS",
                            "ATTEMPTS",
                            "STARTED_AT",
                            "ENDED_AT",
                            "OUTPUT_HASH"
                        );
                        for c in &steps {
                            let hash_display = c
                                .output_hash
                                .as_deref()
                                .map(|h| if h.len() >= 16 { &h[..16] } else { h })
                                .unwrap_or("(none)");
                            println!(
                                "  {:<24} {:<20} {:<8} {:<14} {:<14} {}",
                                c.step_id,
                                c.status.as_str(),
                                c.attempt_count,
                                c.started_at_ms
                                    .map(|t| t.to_string())
                                    .unwrap_or_else(|| "-".into()),
                                c.ended_at_ms
                                    .map(|t| t.to_string())
                                    .unwrap_or_else(|| "-".into()),
                                hash_display,
                            );
                            if let Some(err) = &c.error_msg {
                                println!("    error: {err}");
                            }
                        }
                    }
                    println!();
                    println!("=== Approvals ===");
                    if detail.approvals.is_empty() {
                        println!("  (none)");
                    } else {
                        println!(
                            "  {:<24} {:<10} {:<14} REASON",
                            "STEP_ID", "DECISION", "DECIDED_AT"
                        );
                        for a in &detail.approvals {
                            let decision = match a.decision {
                                boruna_orchestrator::workflow::ApprovalKind::Approved => "approved",
                                boruna_orchestrator::workflow::ApprovalKind::Rejected => "rejected",
                            };
                            println!(
                                "  {:<24} {:<10} {:<14} {}",
                                a.step_id,
                                decision,
                                a.decided_at_ms,
                                a.reason.as_deref().unwrap_or(""),
                            );
                        }
                    }
                }
            }
            #[cfg(not(feature = "persist-sqlite"))]
            {
                let _ = (run_id, json, data_dir);
                return Err("`workflow show` requires the `persist-sqlite` feature".into());
            }
        }
        WorkflowCommand::List {
            status,
            json,
            data_dir,
        } => {
            #[cfg(feature = "persist-sqlite")]
            {
                use boruna_orchestrator::persistence::RunStatus as PersistRunStatus;
                let resolved = resolve_data_dir(data_dir.as_ref(), env_arg);
                let filter = match status.as_deref() {
                    None => None,
                    Some("running") => Some(PersistRunStatus::Running),
                    Some("paused") => Some(PersistRunStatus::Paused),
                    Some("completed") => Some(PersistRunStatus::Completed),
                    Some("failed") => Some(PersistRunStatus::Failed),
                    Some(other) => {
                        return Err(format!(
                            "unknown status '{other}' (expected: running | paused | completed | failed)"
                        )
                        .into())
                    }
                };
                let runs = boruna_orchestrator::workflow::list_runs(&resolved, filter)
                    .map_err(|e| format!("{e}"))?;
                if json {
                    let arr: Vec<serde_json::Value> = runs
                        .iter()
                        .map(|r| {
                            serde_json::json!({
                                "run_id": r.run_id,
                                "workflow_name": r.workflow_name,
                                "status": r.status.as_str(),
                                "started_at_ms": r.started_at_ms,
                                "updated_at_ms": r.updated_at_ms,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&arr)?);
                } else if runs.is_empty() {
                    println!("(no runs)");
                } else {
                    println!(
                        "{:<12} {:<20} {:<32} {:<14} {:<14}",
                        "STATUS", "RUN_ID", "WORKFLOW", "STARTED_AT", "UPDATED_AT"
                    );
                    for r in &runs {
                        println!(
                            "{:<12} {:<20} {:<32} {:<14} {:<14}",
                            r.status.as_str(),
                            r.run_id,
                            r.workflow_name,
                            r.started_at_ms,
                            r.updated_at_ms,
                        );
                    }
                }
            }
            #[cfg(not(feature = "persist-sqlite"))]
            {
                let _ = (status, json, data_dir);
                return Err("`workflow list` requires the `persist-sqlite` feature".into());
            }
        }
    }
    Ok(())
}

/// Truncate a UTF-8 string to at most `max_bytes` bytes, snapped to
/// the nearest character boundary at-or-below `max_bytes`. Appends an
/// ellipsis (`…`) when truncated. Reviewed 0.3-S3 (C1): naive
/// `&s[..max_bytes]` panics if `max_bytes` lands inside a multi-byte
/// codepoint. The `output_json_preview` field on `workflow show
/// --json` is exposed to arbitrary user-defined step output strings,
/// any of which can contain non-ASCII UTF-8.
#[cfg(feature = "persist-sqlite")]
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Verify the on-disk workflow def's hash matches the operator-supplied
/// expected value. Returns Err if there's a mismatch — the CLI bubbles
/// this up as exit code 1 with the formatted error message. Comparison
/// is case-insensitive so operators can paste hashes from any source
/// (`tr` pipelines, copy/paste from config, etc.).
///
/// Introduced in 0.3-S9 — the CI/CD safety primitive that catches
/// accidental or malicious workflow drift before any side effect.
fn check_workflow_hash_expectation(
    def: &boruna_orchestrator::workflow::WorkflowDef,
    expected: &str,
) -> Result<(), String> {
    let actual = boruna_orchestrator::workflow::WorkflowRunner::workflow_hash_from_def(def);
    let expected_norm = expected.trim().to_ascii_lowercase();
    let actual_norm = actual.to_ascii_lowercase();
    if expected_norm == actual_norm {
        Ok(())
    } else {
        Err(format!(
            "workflow_hash mismatch: expected={expected_norm}, actual={actual_norm}\n\
             (the on-disk workflow def differs from what was captured at deploy time; \
             refusing to run)"
        ))
    }
}

/// Resolve the persistent `--data-dir` argument with the documented
/// fallback chain: explicit flag → `BORUNA_DATA_DIR` env var → `./.boruna/data`
/// in the current working directory.
///
/// Sprint 0.4-S14: when `env_name` is `Some` (resolved from the
/// `--env` global flag or `BORUNA_ENV`), the resolved path is
/// further namespaced as `<base>/<env>/`. This lets operators run
/// the same workflow against different environments without manual
/// data-dir bookkeeping. The env name is threaded explicitly from
/// `run()` rather than mutating process state via `set_var`.
#[cfg(feature = "persist-sqlite")]
fn resolve_data_dir(flag: Option<&PathBuf>, env_name: Option<&str>) -> PathBuf {
    let base = if let Some(p) = flag {
        p.clone()
    } else if let Ok(env) = std::env::var("BORUNA_DATA_DIR") {
        if !env.is_empty() {
            PathBuf::from(env)
        } else {
            PathBuf::from("./.boruna/data")
        }
    } else {
        PathBuf::from("./.boruna/data")
    };
    match env_name.filter(|s| !s.is_empty()) {
        Some(name) => base.join(name),
        None => base,
    }
}

/// Validate an env name is filesystem-safe and Prometheus-label-safe
/// (sprint 0.4-S14). Allowed: ASCII alphanumerics, `_`, `-`. Length
/// 1-64. Rejecting unusual characters at the CLI boundary protects
/// downstream code (no path-traversal via `--env ../../etc/passwd`,
/// no broken Prom labels via `--env "with spaces"`).
fn validate_env_name(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if name.is_empty() {
        return Err("--env name must not be empty".into());
    }
    if name.len() > 64 {
        return Err(format!(
            "--env name '{}' exceeds 64-character limit",
            &name[..64.min(name.len())]
        )
        .into());
    }
    let invalid: Vec<char> = name
        .chars()
        .filter(|c| !c.is_ascii_alphanumeric() && *c != '_' && *c != '-')
        .collect();
    if !invalid.is_empty() {
        return Err(format!(
            "--env name '{name}' contains invalid characters {invalid:?}; \
             allowed: ASCII alphanumerics, '_', '-'"
        )
        .into());
    }
    Ok(())
}

fn run_evidence(
    cmd: EvidenceCommand,
    env_arg: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    use boruna_orchestrator::audit::{evidence::BundleManifest, verify::verify_bundle};

    match cmd {
        EvidenceCommand::Create {
            run_id,
            output_dir,
            data_dir,
        } => {
            #[cfg(feature = "persist-sqlite")]
            {
                let resolved_data = resolve_data_dir(data_dir.as_ref(), env_arg);
                let manifest = boruna_orchestrator::workflow::create_bundle(
                    &resolved_data,
                    &run_id,
                    &output_dir,
                )
                .map_err(|e| format!("{e}"))?;
                let bundle_path = output_dir.join(&run_id);
                println!("evidence bundle created at {}", bundle_path.display());
                println!("  bundle_hash: {}", manifest.bundle_hash);
                println!("  audit_hash:  {}", manifest.audit_log_hash);
                println!("  files:       {}", manifest.file_checksums.len());
            }
            #[cfg(not(feature = "persist-sqlite"))]
            {
                let _ = (run_id, output_dir, data_dir);
                return Err("`evidence create` requires the `persist-sqlite` feature".into());
            }
        }
        EvidenceCommand::Verify { dir } => {
            let result = verify_bundle(&dir);
            if result.valid {
                println!("evidence bundle is VALID");
            } else {
                eprintln!("evidence bundle INVALID:");
                for err in &result.errors {
                    eprintln!("  {err}");
                }
                process::exit(1);
            }
        }
        EvidenceCommand::Inspect { dir, json } => {
            let manifest_path = dir.join("manifest.json");
            let manifest_json = fs::read_to_string(&manifest_path)
                .map_err(|e| format!("cannot read manifest: {e}"))?;
            let manifest: BundleManifest = serde_json::from_str(&manifest_json)
                .map_err(|e| format!("invalid manifest: {e}"))?;

            if json {
                println!("{}", serde_json::to_string_pretty(&manifest)?);
            } else {
                println!("=== Evidence Bundle ===");
                println!("run_id:        {}", manifest.run_id);
                println!("workflow:      {}", manifest.workflow_name);
                println!("started_at:    {}", manifest.started_at);
                println!("completed_at:  {}", manifest.completed_at);
                println!("bundle_hash:   {}", manifest.bundle_hash);
                println!("workflow_hash: {}", manifest.workflow_hash);
                println!("policy_hash:   {}", manifest.policy_hash);
                println!("audit_hash:    {}", manifest.audit_log_hash);
                println!("files: {}", manifest.file_checksums.len());
                for (name, hash) in &manifest.file_checksums {
                    println!("  {name}: {}", &hash[..16]);
                }
                println!("\nenv:");
                println!("  boruna: {}", manifest.env_fingerprint.boruna_version);
                println!(
                    "  os: {}/{}",
                    manifest.env_fingerprint.os, manifest.env_fingerprint.arch
                );
            }
        }
    }
    Ok(())
}

fn run_template(cmd: TemplateCommand) -> Result<(), Box<dyn std::error::Error>> {
    use boruna_tooling::templates;

    match cmd {
        TemplateCommand::List { dir } => {
            let templates = templates::list_templates(&dir)
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            if templates.is_empty() {
                println!("no templates found in {}", dir.display());
            } else {
                println!("available templates:");
                for t in &templates {
                    println!("  {} v{} — {}", t.name, t.version, t.description);
                    if !t.dependencies.is_empty() {
                        println!("    deps: {}", t.dependencies.join(", "));
                    }
                    if !t.capabilities.is_empty() {
                        println!("    caps: {}", t.capabilities.join(", "));
                    }
                }
            }
        }
        TemplateCommand::Apply {
            name,
            dir,
            args,
            out,
            validate,
        } => {
            // Parse args from "key1=val1,key2=val2"
            let mut arg_map = std::collections::BTreeMap::new();
            for pair in args.split(',') {
                let pair = pair.trim();
                if pair.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = pair.splitn(2, '=').collect();
                if parts.len() != 2 {
                    return Err(format!("invalid arg format: '{pair}' (expected key=value)").into());
                }
                arg_map.insert(parts[0].to_string(), parts[1].to_string());
            }

            let result = templates::apply_template(&dir, &name, &arg_map)
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

            if validate {
                templates::validate_template_output(&result.source)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
                println!("template output validates OK");
            }

            let output_path = out.unwrap_or_else(|| PathBuf::from(&result.output_file));
            fs::write(&output_path, &result.source)?;
            println!(
                "generated {} from template '{}'",
                output_path.display(),
                result.template_name
            );
            println!("  deps: {}", result.dependencies.join(", "));
            println!("  caps: {}", result.capabilities.join(", "));
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "persist-sqlite"))]
mod tests {
    use super::truncate_at_char_boundary;

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate_at_char_boundary("hello", 200), "hello");
    }

    #[test]
    fn truncate_at_exact_boundary() {
        // 200-byte ASCII string is exactly the limit.
        let s = "a".repeat(200);
        assert_eq!(truncate_at_char_boundary(&s, 200), s);
    }

    #[test]
    fn truncate_does_not_panic_on_multibyte_at_boundary() {
        // 0.3-S3 C1 regression: prior code did `&s[..max_bytes]` which
        // panics when the slice index lands inside a multi-byte UTF-8
        // character. Construct a string where byte 200 lands inside a
        // multi-byte char and verify the truncate succeeds.
        // 'é' is 2 bytes (0xC3 0xA9) in UTF-8.
        // 199 ASCII 'a' chars (199 bytes) + 'é' (2 bytes) = 201 bytes.
        // truncate_at_char_boundary(s, 200) should snap down to byte
        // 199 (after the last 'a', before 'é') — NOT panic.
        let s = "a".repeat(199) + "é";
        assert!(s.len() > 200, "test setup: must exceed limit");
        // Pre-condition: byte index 200 is NOT a char boundary.
        assert!(
            !s.is_char_boundary(200),
            "test setup: byte 200 must be inside the multi-byte é"
        );
        let out = truncate_at_char_boundary(&s, 200);
        // Must not panic. Output must end at the 'a's, with '…' appended.
        assert_eq!(out, format!("{}…", "a".repeat(199)));
    }

    #[test]
    fn truncate_handles_long_multibyte_content() {
        // Pure non-ASCII content: 100 'é' = 200 bytes. At the limit.
        let s = "é".repeat(100);
        assert_eq!(s.len(), 200);
        assert_eq!(truncate_at_char_boundary(&s, 200), s);
        // 101 'é' = 202 bytes. Truncate must snap at a char boundary.
        let s = "é".repeat(101);
        let out = truncate_at_char_boundary(&s, 200);
        // The output is some truncation followed by '…'. The truncation
        // must contain only complete 'é' characters (each 2 bytes).
        let truncated = out.trim_end_matches('…');
        assert!(
            truncated.len().is_multiple_of(2),
            "truncated portion must end on a char boundary; got len {}",
            truncated.len()
        );
        assert!(truncated.chars().all(|c| c == 'é'));
    }

    // ── 0.3-S9: --expect-workflow-hash ──

    use super::check_workflow_hash_expectation;
    use boruna_orchestrator::workflow::{StepDef, StepKind, WorkflowDef, WorkflowRunner};
    use std::collections::BTreeMap;

    fn small_workflow() -> WorkflowDef {
        WorkflowDef {
            schema_version: 1,
            name: "test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([(
                "step1".into(),
                StepDef {
                    kind: StepKind::Source {
                        source: "steps/step1.ax".into(),
                    },
                    capabilities: vec![],
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    depends_on: vec![],
                    timeout_ms: None,
                    retry: None,
                    budget: None,
                },
            )]),
            edges: vec![],
        }
    }

    #[test]
    fn check_workflow_hash_match_returns_ok() {
        let def = small_workflow();
        let hash = WorkflowRunner::workflow_hash_from_def(&def);
        assert!(check_workflow_hash_expectation(&def, &hash).is_ok());
    }

    #[test]
    fn check_workflow_hash_mismatch_returns_typed_error() {
        let def = small_workflow();
        let result = check_workflow_hash_expectation(
            &def,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("workflow_hash mismatch"),
            "expected mismatch message; got: {err}"
        );
        assert!(err.contains("expected="), "should print expected hash");
        assert!(err.contains("actual="), "should print actual hash");
    }

    #[test]
    fn check_workflow_hash_is_case_insensitive() {
        let def = small_workflow();
        let hash = WorkflowRunner::workflow_hash_from_def(&def);
        // Operator pastes uppercase / mixed case — should still match.
        let upper = hash.to_uppercase();
        assert!(check_workflow_hash_expectation(&def, &upper).is_ok());
        // With surrounding whitespace (common in shell pipelines).
        let padded = format!("  {}\n", hash);
        assert!(check_workflow_hash_expectation(&def, &padded).is_ok());
    }
}
