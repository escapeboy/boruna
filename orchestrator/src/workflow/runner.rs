use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use boruna_vm::capability_gateway::{CapabilityGateway, Policy, PolicyRule};
use boruna_vm::error::VmError;
use boruna_vm::Vm;

use crate::workflow::data_flow::DataStore;
use crate::workflow::definition::*;
use crate::workflow::validator::WorkflowValidator;

#[cfg(feature = "persist-sqlite")]
use std::path::PathBuf;

#[cfg(feature = "persist-sqlite")]
use crate::persistence::{
    derive_run_id, PersistenceError, RunCheckpointStore, RunStatus as PersistRunStatus,
    StepCheckpoint, StepStatus as PersistStepStatus,
};

/// Options for a workflow run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Policy to apply (None = deny-all).
    pub policy: Option<Policy>,
    /// Whether to record evidence.
    pub record: bool,
    /// Base directory for the workflow definition files.
    pub workflow_dir: String,
    /// Use real HTTP handler instead of mock (requires `http` feature).
    pub live: bool,
    /// Maximum steps to run concurrently within a single wave (a
    /// "wave" is a topological level in the workflow DAG). `1` means
    /// sequential — preserves the pre-`0.3-S4` behavior. Higher values
    /// fan out fan-out workflows; speedup is proportional to the
    /// number of steps in the largest wave that exceed concurrency.
    /// Honored only on the persistent path (`run_persistent` /
    /// `resume`); the ephemeral `run` path stays single-threaded.
    /// Default `1`.
    #[cfg_attr(not(feature = "persist-sqlite"), allow(dead_code))]
    pub concurrency: usize,
    /// Submit-only mode (sprint `0.5-S2e`). When `true`,
    /// `run_persistent` validates the workflow, embeds step
    /// sources in metadata, inserts the run row + initial
    /// wave's source-step Pending checkpoints, then
    /// **returns BEFORE spawning thread workers**. The
    /// distributed cluster (coord + workers) picks up the
    /// Pending steps via existing mechanisms.
    ///
    /// Honored only on the persistent path. Workflows using
    /// approval gates / external triggers in their FIRST wave
    /// fail at submit time — distributed mode for those step
    /// kinds is deferred to a future sprint.
    ///
    /// Sprint `0.5-S5`: per-step `RetryPolicy` IS honored in
    /// distributed mode via the `coordinator wait` driver.
    /// A worker-reported failure on a step with retry budget
    /// remaining is requeued (Failed → Pending,
    /// `attempt_count += 1`); the next worker re-runs.
    #[cfg_attr(not(feature = "persist-sqlite"), allow(dead_code))]
    pub submit_only: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            policy: None,
            record: false,
            workflow_dir: String::new(),
            live: false,
            concurrency: 1,
            submit_only: false,
        }
    }
}

/// Options for resuming a previously-paused or crashed workflow run.
///
/// Carries the runtime knobs that may legitimately vary between the
/// original run and the resume — `policy` (operator may have widened or
/// narrowed it), `live` (may have toggled record/replay mode), `record`.
/// `workflow_dir` is read from persisted metadata by default but can be
/// overridden via `workflow_dir_override` for relocated checkouts.
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone)]
pub struct ResumeOptions {
    pub policy: Option<Policy>,
    pub record: bool,
    pub live: bool,
    pub workflow_dir_override: Option<String>,
    /// Maximum steps to run concurrently per wave. Default `1` =
    /// sequential. See [`RunOptions::concurrency`].
    pub concurrency: usize,
}

#[cfg(feature = "persist-sqlite")]
impl Default for ResumeOptions {
    fn default() -> Self {
        Self {
            policy: None,
            record: false,
            live: false,
            workflow_dir_override: None,
            concurrency: 1,
        }
    }
}

/// Persisted metadata blob stored in the `runs.metadata_json` column. Owned
/// by the runner — not by the `RunCheckpointStore`, which treats it as
/// opaque caller-defined JSON. Captures the operational context needed to
/// resume the run on the same host.
///
/// `workflow_dir` is **OPERATIONAL ONLY** — different machines may store
/// the same workflow at different absolute paths; the path does not feed
/// any audit hash. `inputs_hash` is **REPLAY-VERIFIED** — it is part of
/// the `run_id` derivation and a resume must produce identical output for
/// the same inputs hash.
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct PersistedRunMetadata {
    workflow_dir: String,
    inputs_hash: String,
    boruna_version: String,
    /// Per-step approval decisions. **OPERATIONAL ONLY** — captures who
    /// decided what and when so dashboards and audit trails can render
    /// the operator action; the run's audit hash chain only sees that
    /// the gate transitioned to `Completed`/`Failed`. Defaulted so
    /// existing 0.3-S2b databases (which have no `approvals` key) parse
    /// cleanly.
    #[serde(default)]
    approvals: BTreeMap<String, ApprovalDecision>,
    /// Per-step external-trigger payloads recorded by
    /// `boruna workflow trigger` (sprint 0.3-S15). Defaulted so
    /// pre-0.3-S15 databases parse cleanly. Operational only.
    #[serde(default)]
    triggers: BTreeMap<String, TriggerRecord>,
    /// Hash-chained audit log of operator actions (sprint 0.4-S9).
    /// Captures `ApprovalGranted` / `ApprovalDenied` /
    /// `ExternalTriggerReceived` events at the moment the operator
    /// authorizes a transition. Defaulted so pre-0.4-S9 databases
    /// parse cleanly with an empty chain.
    ///
    /// **Tamper-evident, NOT replay-verified.** The chain's
    /// `prev_hash` linkage detects any post-hoc mutation when an
    /// auditor calls [`crate::audit::AuditLog::verify`]. The chain
    /// is NOT processed by the run's deterministic-execution
    /// replay pipeline — replay verifies `output_hash` for each
    /// step, not the operator-action chain. Each entry IS
    /// committed atomically with the operator-facing decision in
    /// the same CAS-protected metadata write, so the chain stays
    /// consistent with the persisted decision state.
    ///
    /// Chain order reflects CAS-commit order, not operator-decision
    /// wall-clock order. Two concurrent operators acting on
    /// different steps produce a chain whose order is determined
    /// by which CAS won the SQLite write lock first; per-decision
    /// `decided_at_ms` (in the approvals/triggers blobs) preserves
    /// wall-clock ordering for callers that need it.
    #[serde(default)]
    audit_log: Vec<crate::audit::AuditEntry>,
    /// Step source bodies indexed by `step_id`. Populated by
    /// `prepare_persistent_run` for source-kind steps (sprint
    /// `0.5-S2e`); read by the coordinator's
    /// `extract_step_source` to serve workers a work item
    /// without requiring filesystem access to `workflow_dir`.
    /// Defaulted so pre-0.5-S2e databases (no `step_sources`
    /// key) parse cleanly.
    ///
    /// **OPERATIONAL ONLY.** Source content is already
    /// captured in `workflow_hash` for replay/audit purposes;
    /// embedding the bodies here is a transport convenience,
    /// not a replay-verified record.
    #[serde(default)]
    step_sources: BTreeMap<String, String>,
    /// Full workflow DAG embedded for client-side multi-wave
    /// advancement (sprint `0.5-S2f`). Populated only when
    /// `RunOptions::submit_only` is true; in-process
    /// `run_persistent` keeps this `None` to avoid bloating
    /// metadata. Read by `WorkflowRunner::advance_run_one_tick`
    /// (the `boruna coordinator wait` driver) to compute
    /// downstream-ready successors as steps complete.
    /// Defaulted so pre-0.5-S2f databases parse cleanly.
    ///
    /// **OPERATIONAL ONLY.** `workflow_hash` is the
    /// replay-verified record of workflow content; this field
    /// is a transport convenience for the wait client, identical
    /// to the on-disk `workflow.json` at submit time. Capped at
    /// 1 MiB serialized JSON; oversize fails at submit time
    /// with a typed `Validation` error.
    #[serde(default)]
    workflow_def: Option<crate::workflow::definition::WorkflowDef>,
}

/// Per-tick result of [`WorkflowRunner::advance_run_one_tick`].
/// Sprint `0.5-S2f`: returned to `boruna coordinator wait` for
/// progress display + terminal-status decision.
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone)]
pub struct AdvanceResult {
    /// Step IDs that transitioned Unknown → Pending in this tick.
    /// Sorted ascending. Empty when no new steps became ready.
    pub newly_pending: Vec<String>,
    /// Step IDs that transitioned Failed → Pending in this tick via
    /// the per-step `RetryPolicy` (sprint `0.5-S5`). Sorted ascending.
    /// Empty when no Failed-with-retry-budget steps were observed.
    /// Distinguished from [`Self::newly_pending`] so the wait driver
    /// can print a different log line ("requeued (attempt N)" vs
    /// just "pending"). Operational only; both kinds are reflected
    /// in [`Self::all_step_statuses`].
    pub newly_requeued: Vec<String>,
    /// Overall run status as derived from the checkpoint set:
    /// - `Failed` if any step is in `Failed` status AND has no
    ///   retry budget remaining (sprint `0.5-S5`). A Failed-with-
    ///   budget step keeps the run `Running` and is requeued in
    ///   this same tick.
    /// - `Completed` if all steps in the workflow def are in
    ///   `Completed` status (terminal).
    /// - `Running` otherwise (transient).
    pub run_status: AdvanceRunStatus,
    /// Per-step status map after the tick. Keys are step IDs from
    /// the workflow def; values are the persisted status. Steps
    /// with no checkpoint row are absent from the map (Unknown).
    pub all_step_statuses: BTreeMap<String, PersistStepStatus>,
}

/// Terminal-vs-transient run status surfaced to wait clients.
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvanceRunStatus {
    Running,
    Completed,
    Failed,
}

/// Per-step approval decision recorded by `boruna workflow approve` /
/// `reject`. Stored in the run's `metadata_json.approvals.<step_id>`
/// blob; round-tripped through SQLite as part of the opaque caller JSON.
///
/// **OPERATIONAL ONLY.** None of these fields feed any audit hash. The
/// run's audit chain records only that the approval-gate step
/// transitioned to a terminal state — the WHO/WHEN/WHY belong to the
/// operator metadata, not the replay-verified record.
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ApprovalDecision {
    decision: ApprovalKind,
    /// Unix epoch ms of when the operator ran `approve`/`reject`.
    /// Wall-clock-keyed; not in any hash chain.
    decided_at_ms: i64,
    /// Optional rejection reason. None for approvals.
    #[serde(default)]
    reason: Option<String>,
}

/// Approval-gate decision kind. Public so the CLI handler can pass it
/// into [`record_approval_decision`].
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Approved,
    Rejected,
}

#[cfg(feature = "persist-sqlite")]
impl ApprovalKind {
    fn as_str(self) -> &'static str {
        match self {
            ApprovalKind::Approved => "approved",
            ApprovalKind::Rejected => "rejected",
        }
    }
}

/// Per-step external-trigger record (sprint 0.3-S15). Stored in the
/// run's `metadata_json.triggers.<step_id>`. The trigger payload
/// becomes the step's output value when resume processes the
/// sentinel — downstream steps read it via `step_input` (sprint
/// 0.3-S14).
///
/// **OPERATIONAL ONLY** for the timestamp; the **payload** itself
/// becomes the step's output (which IS replay-verified). Cross-
/// machine replay of an externally-triggered run requires the same
/// payload to arrive — typically captured via the operator's webhook
/// receiver and replayed from a tape (similar to net record/replay).
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TriggerRecord {
    /// Token issued at pause-time, validated at trigger-time. Random
    /// 32-hex-char (16 bytes) generated when the runner enters an
    /// `awaiting_external_event` state. The CLI requires the operator
    /// to pass this token to prevent accidental cross-step triggers.
    token: String,
    /// JSON-encoded payload supplied by the operator. Becomes the
    /// step's output value: `Value::String(payload)` so downstream
    /// steps read it via `step_input` and parse the JSON inline.
    payload: String,
    /// Wall-clock ms of when the trigger arrived. Operational only.
    triggered_at_ms: i64,
}

/// Executes a validated workflow definition step by step.
pub struct WorkflowRunner;

impl WorkflowRunner {
    /// Run a workflow to completion (or until an approval gate is hit).
    ///
    /// **Ephemeral path** — runs in a `tempfile::tempdir()` and writes no
    /// checkpoints. Use [`run_persistent`](Self::run_persistent) for runs
    /// that must survive process restarts.
    pub fn run(
        def: &WorkflowDef,
        options: &RunOptions,
    ) -> Result<WorkflowRunResult, WorkflowRunError> {
        // Validate first
        WorkflowValidator::validate(def).map_err(|errors| {
            WorkflowRunError::Validation(
                errors
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;

        // 0.3-S15: external_trigger steps require persistent metadata
        // to stash the trigger token; the ephemeral path has no store,
        // so a paused trigger could never be advanced. Refuse upfront
        // rather than executing prior steps and then erroring at the
        // pause. Reviewed in 0.3-S15 — earlier draft caught this at
        // step-entry time, which silently allowed prior steps to
        // execute before the typed error surfaced.
        for (step_id, step) in &def.steps {
            if matches!(step.kind, StepKind::ExternalTrigger { .. }) {
                return Err(WorkflowRunError::Validation(format!(
                    "step '{step_id}' is an external_trigger step; \
                     external triggers require persistent runs \
                     (use `run_persistent`, not `run`)"
                )));
            }
        }

        let order =
            WorkflowValidator::topological_order(def).map_err(WorkflowRunError::Validation)?;

        // Deterministic run_id even on the ephemeral path. Counter is
        // always 0 because there is no persistent counter to consult; this
        // means two calls to `run` with identical inputs produce the same
        // run_id, which is fine for an ephemeral path that never inserts
        // into a store.
        let workflow_hash = Self::workflow_hash_from_def(def);
        let inputs_hash = Self::ephemeral_inputs_hash();

        #[cfg(feature = "persist-sqlite")]
        let run_id = derive_run_id(&workflow_hash, &inputs_hash, 0);
        #[cfg(not(feature = "persist-sqlite"))]
        let run_id = {
            // Fallback when the persistence feature is off: use a hash-only
            // derivation that doesn't require the helper from the
            // persistence module.
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(workflow_hash.as_bytes());
            h.update(b":");
            h.update(inputs_hash.as_bytes());
            h.update(b":");
            h.update(0i64.to_le_bytes());
            let digest = h.finalize();
            let mut s = String::with_capacity(16);
            for b in &digest[..8] {
                use std::fmt::Write;
                let _ = write!(s, "{b:02x}");
            }
            s
        };

        let run_dir = tempfile::tempdir().map_err(|e| WorkflowRunError::Io(e.to_string()))?;
        let mut data_store =
            DataStore::new(run_dir.path()).map_err(|e| WorkflowRunError::Io(e.to_string()))?;

        let result = Self::execute_steps(
            def,
            &order,
            options,
            &run_id,
            &mut data_store,
            BTreeSet::new(),
            &BTreeMap::new(),
            #[cfg(feature = "persist-sqlite")]
            None,
        )?;
        Ok(result)
    }

    /// Run with persistence. Opens (or creates) a `RunCheckpointStore` at
    /// `data_dir/runs.db`, inserts a row at run-start, writes a checkpoint
    /// at every step transition, and updates the terminal run status when
    /// execution finishes or pauses.
    ///
    /// `run_id` is deterministically derived per ADR 001 and
    /// project-conventions §16: `sha256(workflow_hash || ":" || inputs_hash
    /// || ":" || counter)[..16]` where the counter is the per-workflow
    /// existing-runs count read inside a `BEGIN IMMEDIATE` transaction.
    #[cfg(feature = "persist-sqlite")]
    pub fn run_persistent(
        def: &WorkflowDef,
        options: &RunOptions,
        data_dir: &Path,
    ) -> Result<WorkflowRunResult, WorkflowRunError> {
        let (store, run_id) = Self::prepare_persistent_run(def, options, data_dir)?;
        if options.submit_only {
            // Sprint 0.5-S2e: submit-only mode. Insert the
            // initial wave's source-step Pending checkpoints and
            // emit the WorkflowStarted audit event, then return
            // a partial result. The distributed cluster (coord +
            // workers) picks up the Pending steps via existing
            // mechanisms.
            //
            // Adversarial-review F3 from 0.5-S2e: warn explicitly
            // when `--concurrency` is set on submit-only runs.
            // Concurrency is enforced inside the in-process wave
            // loop (`execute_steps_concurrent`), which submit-only
            // never invokes — the coordinator's worker pool
            // controls parallelism in distributed mode. Silent
            // ignore was the original posture; this surfaces it.
            if options.concurrency > 1 {
                eprintln!(
                    "[WARNING] submit-only mode ignores --concurrency={} — \
                     parallelism in distributed mode is controlled by the \
                     number of workers connected to the coordinator, not by \
                     this flag.",
                    options.concurrency
                );
            }
            Self::insert_initial_wave_pending_checkpoints(def, &store, &run_id)?;
            // Best-effort WorkflowStarted audit event, matching
            // execute_after_insert's behavior so the audit chain
            // has its genesis entry.
            let policy_hash_seed = serde_json::to_string(&options.policy).unwrap_or_default();
            if let Err(e) = append_audit_event(
                &store,
                &run_id,
                crate::audit::AuditEvent::WorkflowStarted {
                    workflow_hash: Self::workflow_hash_from_def(def),
                    policy_hash: sha256_hex(&policy_hash_seed),
                },
            ) {
                eprintln!(
                    "warning: failed to append WorkflowStarted audit event for submit-only run \
                     '{run_id}': {e}"
                );
            }
            return Ok(WorkflowRunResult {
                run_id,
                workflow_name: def.name.clone(),
                status: WorkflowStatus::Running,
                step_results: BTreeMap::new(),
                total_duration_ms: 0,
            });
        }
        Self::execute_after_insert(def, options, data_dir, &store, run_id)
    }

    /// Atomic variant of [`run_persistent`] for the cron-driven
    /// scheduling pattern. Wraps the (check-in-flight + insert) sequence
    /// in a single SQL transaction so concurrent processes can no
    /// longer both pass the check and both insert.
    ///
    /// Returns:
    /// - `Ok(Some(WorkflowRunResult))` — a new run was inserted and
    ///   executed.
    /// - `Ok(None)` — a prior in-flight run exists; this invocation
    ///   skipped cleanly without inserting or executing anything.
    /// - `Err(...)` — validation or persistence failure.
    ///
    /// Replaces the racy 2-call flow from sprint 0.3-S7
    /// (`find_in_flight_runs` then `run_persistent`). Reviewed in
    /// 0.3-S10. The previous flow remains correct in single-operator
    /// cron invocations but could double-insert under concurrent
    /// operators.
    #[cfg(feature = "persist-sqlite")]
    pub fn run_persistent_or_skip(
        def: &WorkflowDef,
        options: &RunOptions,
        data_dir: &Path,
    ) -> Result<Option<WorkflowRunResult>, WorkflowRunError> {
        WorkflowValidator::validate(def).map_err(|errors| {
            WorkflowRunError::Validation(
                errors
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        WorkflowValidator::topological_order(def).map_err(WorkflowRunError::Validation)?;

        let store = open_store(data_dir)?;
        let workflow_hash = Self::workflow_hash_from_def(def);
        let inputs_hash = Self::ephemeral_inputs_hash();
        let policy_json = serde_json::to_string(&options.policy)
            .map_err(|e| WorkflowRunError::Internal(format!("policy serialize: {e}")))?;
        let metadata = PersistedRunMetadata {
            workflow_dir: options.workflow_dir.clone(),
            inputs_hash: inputs_hash.clone(),
            boruna_version: env!("CARGO_PKG_VERSION").to_string(),
            approvals: BTreeMap::new(),
            triggers: BTreeMap::new(),
            audit_log: Vec::new(),
            step_sources: Self::collect_step_sources(def, &options.workflow_dir)?,
            workflow_def: Self::embed_workflow_def_for_metadata(def, options)?,
        };
        let metadata_json = serde_json::to_string(&metadata)
            .map_err(|e| WorkflowRunError::Internal(format!("metadata serialize: {e}")))?;

        let outcome = store
            .insert_run_with_derived_id_skip_if_in_flight(
                &def.name,
                &workflow_hash,
                &inputs_hash,
                &policy_json,
                &metadata_json,
                now_unix_ms(),
            )
            .map_err(WorkflowRunError::from)?;
        match outcome {
            crate::persistence::InsertOrSkip::Skipped(_prior) => Ok(None),
            crate::persistence::InsertOrSkip::Inserted(run_id) => {
                Self::execute_after_insert(def, options, data_dir, &store, run_id).map(Some)
            }
        }
    }

    /// Embed the full `WorkflowDef` into metadata when running in
    /// submit-only mode. Sprint `0.5-S2f`: read by
    /// `advance_run_one_tick` (the `boruna coordinator wait` driver)
    /// to compute downstream-ready successors as steps complete.
    ///
    /// Returns `None` when `options.submit_only` is false (in-process
    /// runs don't need the embedded def — the runner already holds it
    /// in memory). When `submit_only` is true, serializes the def to
    /// estimate size and rejects defs whose serialized JSON exceeds
    /// `MAX_WORKFLOW_DEF_BYTES`. The cap mirrors the per-step
    /// `MAX_PER_STEP_BYTES` pattern from 0.5-S2e: variable-size
    /// embedded payloads must have an explicit ceiling so a
    /// pathological workflow can't blow up `metadata_json`.
    #[cfg(feature = "persist-sqlite")]
    fn embed_workflow_def_for_metadata(
        def: &WorkflowDef,
        options: &RunOptions,
    ) -> Result<Option<WorkflowDef>, WorkflowRunError> {
        if !options.submit_only {
            return Ok(None);
        }
        // 1 MiB ceiling on serialized def. Real workflows top out at
        // a few hundred KiB even with verbose `with_input` blocks;
        // this cap is generous but bounded.
        const MAX_WORKFLOW_DEF_BYTES: usize = 1024 * 1024;
        let bytes = serde_json::to_string(def)
            .map_err(|e| WorkflowRunError::Internal(format!("workflow_def serialize: {e}")))?
            .len();
        if bytes > MAX_WORKFLOW_DEF_BYTES {
            return Err(WorkflowRunError::Validation(format!(
                "workflow_def serialized to {bytes} bytes; exceeds cap of \
                 {MAX_WORKFLOW_DEF_BYTES} bytes (submit-only mode embeds the \
                 def in metadata for the coordinator wait driver)"
            )));
        }
        Ok(Some(def.clone()))
    }

    /// Read the `.ax` source for every `Source`-kind step in
    /// `def` and return a `step_id → source` map. Sprint
    /// `0.5-S2e`: populated into `metadata_json.step_sources` so
    /// the distributed coordinator can serve workers without
    /// requiring filesystem access to `workflow_dir`.
    ///
    /// Approval-gate and external-trigger steps don't have an
    /// `.ax` source — they're skipped here (the coordinator only
    /// dispatches source steps to workers).
    #[cfg(feature = "persist-sqlite")]
    fn collect_step_sources(
        def: &WorkflowDef,
        workflow_dir: &str,
    ) -> Result<BTreeMap<String, String>, WorkflowRunError> {
        use crate::workflow::definition::StepKind;
        // Size caps (adversarial-review F2): metadata_json must
        // round-trip through SQLite and be embedded in HTTP
        // responses; oversized metadata blocks the dispatch
        // path. 256 KiB per step covers comfortably-large `.ax`
        // sources; 4 MiB aggregate covers ~16 large steps.
        const MAX_PER_STEP_BYTES: usize = 256 * 1024;
        const MAX_AGGREGATE_BYTES: usize = 4 * 1024 * 1024;
        let mut out = BTreeMap::new();
        let mut total_bytes: usize = 0;
        let base = std::path::PathBuf::from(workflow_dir);
        for (step_id, step_def) in &def.steps {
            if let StepKind::Source { source } = &step_def.kind {
                let path = base.join(source);
                let body = std::fs::read_to_string(&path).map_err(|e| {
                    WorkflowRunError::Io(format!(
                        "step '{step_id}' source '{}' read failed: {e}",
                        path.display()
                    ))
                })?;
                if body.len() > MAX_PER_STEP_BYTES {
                    return Err(WorkflowRunError::Validation(format!(
                        "step '{step_id}' source is {} bytes; exceeds per-step cap of {} bytes",
                        body.len(),
                        MAX_PER_STEP_BYTES
                    )));
                }
                total_bytes = total_bytes.saturating_add(body.len());
                if total_bytes > MAX_AGGREGATE_BYTES {
                    return Err(WorkflowRunError::Validation(format!(
                        "aggregate step_sources size exceeds cap of {} bytes \
                         (after step '{step_id}')",
                        MAX_AGGREGATE_BYTES
                    )));
                }
                out.insert(step_id.clone(), body);
            }
        }
        Ok(out)
    }

    /// Submit a workflow with inline step sources to the persistent
    /// store, returning the assigned `run_id`. Sprint `0.5-S4`:
    /// powers the coordinator's `POST /api/runs/submit` endpoint, so
    /// CI runners can drive remote clusters without sharing a
    /// `data-dir`.
    ///
    /// Behaviorally identical to `prepare_persistent_run` + the
    /// `submit_only` branch of [`Self::run_persistent`], with two
    /// differences:
    ///
    /// 1. **Step sources come from the caller, not disk.** The HTTP
    ///    submit payload inlines every Source-kind step's `.ax`
    ///    body. We validate per-step (256 KiB) and aggregate (4 MiB)
    ///    size caps that match [`Self::collect_step_sources`].
    /// 2. **`workflow_dir` is empty in metadata.** The remote
    ///    cluster never reads from operator-side paths; downstream
    ///    code that touches `workflow_dir` is gated on
    ///    `step_sources` being populated, which it always is here.
    ///
    /// Validation, the WorkflowStarted audit-event genesis, and the
    /// initial-wave Pending checkpoint insertion all match
    /// submit-only mode so the coordinator can dispatch work to
    /// connected workers immediately.
    #[cfg(feature = "persist-sqlite")]
    pub fn submit_with_inline_sources(
        def: &WorkflowDef,
        step_sources: BTreeMap<String, String>,
        policy: &boruna_vm::Policy,
        store: &RunCheckpointStore,
    ) -> Result<String, WorkflowRunError> {
        use crate::workflow::definition::StepKind;

        WorkflowValidator::validate(def).map_err(|errors| {
            WorkflowRunError::Validation(
                errors
                    .into_iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;

        // Every Source-kind step in the def must be covered by an
        // entry in `step_sources`; missing entries would mean the
        // remote cluster has no .ax to compile when a worker claims
        // the step. Symmetric to collect_step_sources reading every
        // such step from disk.
        for (step_id, step_def) in &def.steps {
            if let StepKind::Source { .. } = &step_def.kind {
                if !step_sources.contains_key(step_id) {
                    return Err(WorkflowRunError::Validation(format!(
                        "submit-with-inline-sources: missing inline source for \
                         Source-kind step '{step_id}'"
                    )));
                }
            }
        }

        // Caps mirror collect_step_sources / embed_workflow_def_for_metadata.
        const MAX_PER_STEP_BYTES: usize = 256 * 1024;
        const MAX_AGGREGATE_BYTES: usize = 4 * 1024 * 1024;
        const MAX_WORKFLOW_DEF_BYTES: usize = 1024 * 1024;
        let mut total_bytes: usize = 0;
        for (step_id, body) in &step_sources {
            if body.len() > MAX_PER_STEP_BYTES {
                return Err(WorkflowRunError::Validation(format!(
                    "step '{step_id}' inline source is {} bytes; exceeds per-step cap of {} bytes",
                    body.len(),
                    MAX_PER_STEP_BYTES
                )));
            }
            total_bytes = total_bytes.saturating_add(body.len());
            if total_bytes > MAX_AGGREGATE_BYTES {
                return Err(WorkflowRunError::Validation(format!(
                    "aggregate inline step_sources size exceeds cap of {} bytes \
                     (after step '{step_id}')",
                    MAX_AGGREGATE_BYTES
                )));
            }
        }
        let def_bytes = serde_json::to_string(def)
            .map_err(|e| WorkflowRunError::Internal(format!("workflow_def serialize: {e}")))?
            .len();
        if def_bytes > MAX_WORKFLOW_DEF_BYTES {
            return Err(WorkflowRunError::Validation(format!(
                "workflow_def serialized to {def_bytes} bytes; exceeds cap of \
                 {MAX_WORKFLOW_DEF_BYTES} bytes"
            )));
        }

        let workflow_hash = Self::workflow_hash_from_def(def);
        let inputs_hash = Self::ephemeral_inputs_hash();
        let policy_json = serde_json::to_string(policy)
            .map_err(|e| WorkflowRunError::Internal(format!("policy serialize: {e}")))?;
        let metadata = PersistedRunMetadata {
            // Empty: the cluster never reads from operator-side paths
            // because step_sources covers every dispatchable step.
            workflow_dir: String::new(),
            inputs_hash: inputs_hash.clone(),
            boruna_version: env!("CARGO_PKG_VERSION").to_string(),
            approvals: BTreeMap::new(),
            triggers: BTreeMap::new(),
            audit_log: Vec::new(),
            step_sources,
            workflow_def: Some(def.clone()),
        };
        let metadata_json = serde_json::to_string(&metadata)
            .map_err(|e| WorkflowRunError::Internal(format!("metadata serialize: {e}")))?;

        let run_id = store
            .insert_run_with_derived_id(
                &def.name,
                &workflow_hash,
                &inputs_hash,
                &policy_json,
                &metadata_json,
                now_unix_ms(),
            )
            .map_err(WorkflowRunError::from)?;

        Self::insert_initial_wave_pending_checkpoints(def, store, &run_id)?;

        // Best-effort genesis audit event — same posture as
        // submit-only and execute_after_insert. CAS exhaustion logs
        // and continues; the run still proceeds.
        let policy_hash_seed = serde_json::to_string(&Some(policy.clone())).unwrap_or_default();
        if let Err(e) = append_audit_event(
            store,
            &run_id,
            crate::audit::AuditEvent::WorkflowStarted {
                workflow_hash: Self::workflow_hash_from_def(def),
                policy_hash: sha256_hex(&policy_hash_seed),
            },
        ) {
            eprintln!(
                "warning: failed to append WorkflowStarted audit event for inline-submit run \
                 '{run_id}': {e}"
            );
        }

        Ok(run_id)
    }

    /// Submit-only initial-wave Pending checkpoint insertion
    /// (sprint `0.5-S2e`). For the first topological level, write
    /// a Pending step checkpoint for every `Source`-kind step.
    /// Approval-gate and external-trigger steps are NOT supported
    /// in submit-only mode — they require runner-side
    /// orchestration that distributed mode doesn't yet handle.
    #[cfg(feature = "persist-sqlite")]
    fn insert_initial_wave_pending_checkpoints(
        def: &WorkflowDef,
        store: &RunCheckpointStore,
        run_id: &str,
    ) -> Result<(), WorkflowRunError> {
        use crate::persistence::StepCheckpoint;
        use crate::workflow::definition::StepKind;
        let levels =
            WorkflowValidator::topological_levels(def).map_err(WorkflowRunError::Validation)?;
        let Some(first_level) = levels.first() else {
            return Ok(());
        };
        for step_id in first_level {
            let step_def = def.steps.get(step_id).ok_or_else(|| {
                WorkflowRunError::Internal(format!("step not found in def: {step_id}"))
            })?;
            match &step_def.kind {
                StepKind::Source { .. } => {
                    store
                        .upsert_step_checkpoint(&StepCheckpoint {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                            status: PersistStepStatus::Pending,
                            output_json: None,
                            output_hash: None,
                            started_at_ms: None,
                            ended_at_ms: None,
                            error_msg: None,
                            attempt_count: 1,
                            worker_id: None,
                            lease_expires_at_ms: None,
                            claim_id: 0,
                            output_blob_ref: None,
                        })
                        .map_err(WorkflowRunError::from)?;
                }
                StepKind::ApprovalGate { .. } | StepKind::ExternalTrigger { .. } => {
                    return Err(WorkflowRunError::Validation(format!(
                        "submit-only mode does not support {:?}-kind steps in the first wave \
                         (step '{step_id}'); use the in-process runner for workflows with \
                         approval gates or external triggers",
                        step_def.kind
                    )));
                }
            }
        }
        Ok(())
    }

    /// Compute the set of steps that are ready to dispatch given the
    /// workflow DAG and the current per-step status map. Sprint
    /// `0.5-S2f`: pure helper used by [`advance_run_one_tick`] for
    /// client-side multi-wave advancement.
    ///
    /// A step is "ready" iff:
    /// - It has no checkpoint row yet (i.e. its status is Unknown,
    ///   represented here by absence from `status_map`).
    /// - Every upstream dependency (via explicit `edges` or
    ///   `depends_on`) has status `Completed` in `status_map`.
    ///
    /// Returns step IDs sorted ascending for deterministic dispatch
    /// order. Pure function; no side effects.
    #[cfg(feature = "persist-sqlite")]
    pub fn compute_ready_steps(
        def: &WorkflowDef,
        status_map: &BTreeMap<String, PersistStepStatus>,
    ) -> Vec<String> {
        let mut upstream: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for id in def.steps.keys() {
            upstream.insert(id.clone(), BTreeSet::new());
        }
        for (from, to) in &def.edges {
            upstream.entry(to.clone()).or_default().insert(from.clone());
        }
        for (id, step) in &def.steps {
            for dep in &step.depends_on {
                upstream.entry(id.clone()).or_default().insert(dep.clone());
            }
        }
        let mut ready: Vec<String> = Vec::new();
        for step_id in def.steps.keys() {
            if status_map.contains_key(step_id) {
                continue;
            }
            let deps = match upstream.get(step_id) {
                Some(d) => d,
                None => continue,
            };
            let all_completed = deps
                .iter()
                .all(|d| matches!(status_map.get(d), Some(PersistStepStatus::Completed)));
            if all_completed {
                ready.push(step_id.clone());
            }
        }
        ready.sort();
        ready
    }

    /// One polling tick of the client-side wave-advancement driver.
    /// Sprint `0.5-S2f`: invoked repeatedly by `boruna coordinator wait`.
    ///
    /// Reads the run's metadata + checkpoints, computes ready steps via
    /// [`Self::compute_ready_steps`], and writes a Pending checkpoint for
    /// each ready step using [`RunCheckpointStore::insert_pending_step_if_absent`]
    /// (the race-safe primitive — it leaves Running rows untouched if
    /// the coordinator concurrently claimed a sibling step).
    ///
    /// Returns an [`AdvanceResult`] summarizing the tick.
    ///
    /// Errors:
    /// - The run is not found in the store.
    /// - The run's metadata has no embedded `workflow_def` (was
    ///   submitted before 0.5-S2f or via in-process mode that didn't
    ///   embed the def).
    /// - A non-first-wave step is `ApprovalGate` or `ExternalTrigger`
    ///   kind — distributed mode does not yet support those step kinds.
    #[cfg(feature = "persist-sqlite")]
    pub fn advance_run_one_tick(
        store: &RunCheckpointStore,
        run_id: &str,
    ) -> Result<AdvanceResult, WorkflowRunError> {
        use crate::persistence::RequeueOutcome;
        let row = store
            .get_run(run_id)
            .map_err(WorkflowRunError::from)?
            .ok_or_else(|| WorkflowRunError::Validation(format!("run not found: {run_id}")))?;
        let metadata: PersistedRunMetadata = serde_json::from_str(&row.metadata_json)
            .map_err(|e| WorkflowRunError::Internal(format!("metadata parse: {e}")))?;
        let def = metadata.workflow_def.as_ref().ok_or_else(|| {
            WorkflowRunError::Validation(format!(
                "run '{run_id}' has no embedded workflow_def in metadata; \
                 coordinator wait requires a submit-only-mode run \
                 (run was submitted before 0.5-S2f or via in-process mode)"
            ))
        })?;

        // Sprint 0.5-S6: close resolved approval / trigger gates *first*.
        // Doing this before the checkpoint read and ready-step computation
        // means the gate's transition (e.g. AwaitingApproval → Completed)
        // is visible to compute_ready_steps in the same tick — downstream
        // steps unblock immediately rather than after another poll.
        // Idempotent: if no sentinels match, this is a cheap metadata
        // read + bail.
        let _gate_closures = Self::close_resolved_gates(store, run_id, def)?;

        let checkpoints = store
            .list_step_checkpoints(run_id)
            .map_err(WorkflowRunError::from)?;
        let mut status_map: BTreeMap<String, PersistStepStatus> = BTreeMap::new();
        // Capture full per-step view; we need attempt_count + error_msg
        // for the retry pass below.
        let mut checkpoint_by_id: BTreeMap<String, &StepCheckpoint> = BTreeMap::new();
        for cp in &checkpoints {
            status_map.insert(cp.step_id.clone(), cp.status);
            checkpoint_by_id.insert(cp.step_id.clone(), cp);
        }

        // 0.5-S5: Failed-step retry pass. For each Failed step whose
        // RetryPolicy still has budget AND whose recorded error class
        // is retry-eligible per the policy, atomically requeue
        // (Failed → Pending, attempt_count += 1). The status_map entry
        // flips so the downstream "any_failed" check below sees Pending,
        // not Failed.
        //
        // Iteration is over def.steps so order is BTreeMap-sorted —
        // deterministic. The persistence primitive is idempotent
        // against concurrent wait clients (convention §14).
        let mut newly_requeued: Vec<String> = Vec::new();
        for step_id in def.steps.keys() {
            if !matches!(status_map.get(step_id), Some(PersistStepStatus::Failed)) {
                continue;
            }
            let cp = match checkpoint_by_id.get(step_id) {
                Some(cp) => cp,
                None => continue,
            };
            let step_def = match def.steps.get(step_id) {
                Some(s) => s,
                None => continue,
            };
            let policy = match step_def.retry.as_ref() {
                Some(p) => p,
                None => continue,
            };
            // Budget gate: attempt_count is the number of attempts
            // already used. max_attempts <= 1 (or budget exhausted)
            // means no retry. Also handled by should_retry_class but
            // short-circuited here so we don't hit the persistence
            // layer for an exhausted policy.
            if policy.max_attempts <= 1 || cp.attempt_count >= policy.max_attempts {
                continue;
            }
            let class = classify_failure_message(cp.error_msg.as_deref().unwrap_or(""));
            if !should_retry_class(Some(policy), class) {
                continue;
            }
            let outcome = store
                .requeue_failed_step_for_retry(run_id, step_id)
                .map_err(WorkflowRunError::from)?;
            match outcome {
                RequeueOutcome::Requeued { .. } => {
                    newly_requeued.push(step_id.clone());
                    status_map.insert(step_id.clone(), PersistStepStatus::Pending);
                }
                // Race: a concurrent wait client already moved the row
                // off Failed. Idempotent no-op. Refresh status_map to
                // reflect on-disk truth.
                RequeueOutcome::NotFailed { current_status } => {
                    status_map.insert(step_id.clone(), current_status);
                }
                // Defensive: no code path produces NotFound today (no
                // DELETE on step_checkpoints), but don't panic if a
                // future migration changes that.
                RequeueOutcome::NotFound => {}
            }
        }

        let ready = Self::compute_ready_steps(def, &status_map);
        let mut newly_pending: Vec<String> = Vec::new();
        for step_id in &ready {
            let step_def = def.steps.get(step_id).ok_or_else(|| {
                WorkflowRunError::Internal(format!(
                    "step '{step_id}' in ready set but missing from workflow_def"
                ))
            })?;
            match &step_def.kind {
                StepKind::Source { .. } => {
                    let inserted = store
                        .insert_pending_step_if_absent(run_id, step_id)
                        .map_err(WorkflowRunError::from)?;
                    if inserted {
                        newly_pending.push(step_id.clone());
                        status_map.insert(step_id.clone(), PersistStepStatus::Pending);
                    }
                }
                StepKind::ApprovalGate { .. } => {
                    // Sprint 0.5-S6: open the approval gate by writing
                    // an AwaitingApproval checkpoint. The operator
                    // closes the gate via `boruna workflow approve|reject`
                    // (local data-dir or remote `--coordinator`); the
                    // sentinel-pass below picks up the decision on a
                    // later tick. Idempotent: `upsert_step_checkpoint`
                    // is a no-op if the checkpoint already exists with
                    // the same status.
                    let exists = checkpoint_by_id.contains_key(step_id);
                    if !exists {
                        store
                            .upsert_step_checkpoint(&StepCheckpoint {
                                run_id: run_id.to_string(),
                                step_id: step_id.clone(),
                                status: PersistStepStatus::AwaitingApproval,
                                output_json: None,
                                output_hash: None,
                                started_at_ms: Some(now_unix_ms()),
                                ended_at_ms: None,
                                error_msg: None,
                                attempt_count: 1,
                                worker_id: None,
                                lease_expires_at_ms: None,
                                claim_id: 0,
                                output_blob_ref: None,
                            })
                            .map_err(WorkflowRunError::from)?;
                        status_map.insert(step_id.clone(), PersistStepStatus::AwaitingApproval);
                    }
                }
                StepKind::ExternalTrigger { .. } => {
                    // Sprint 0.5-S6: open the trigger gate. Acquire (or
                    // recover) the per-step trigger token, then write
                    // the AwaitingExternalEvent checkpoint. The
                    // sentinel-pass below advances the step once the
                    // operator (or webhook bridge) calls
                    // `boruna workflow trigger` with a non-empty
                    // payload.
                    let exists = checkpoint_by_id.contains_key(step_id);
                    if !exists {
                        let _token = acquire_trigger_token(store, run_id, step_id)?;
                        store
                            .upsert_step_checkpoint(&StepCheckpoint {
                                run_id: run_id.to_string(),
                                step_id: step_id.clone(),
                                status: PersistStepStatus::AwaitingExternalEvent,
                                output_json: None,
                                output_hash: None,
                                started_at_ms: Some(now_unix_ms()),
                                ended_at_ms: None,
                                error_msg: None,
                                attempt_count: 1,
                                worker_id: None,
                                lease_expires_at_ms: None,
                                claim_id: 0,
                                output_blob_ref: None,
                            })
                            .map_err(WorkflowRunError::from)?;
                        status_map
                            .insert(step_id.clone(), PersistStepStatus::AwaitingExternalEvent);
                    }
                }
            }
        }

        // 0.5-S5: Failed → run_status=Failed only if at least one Failed
        // step has no retry budget remaining. A Failed-with-budget step
        // was requeued above (status_map flipped to Pending), so the
        // any_failed scan here only sees genuinely-terminal failures.
        let any_failed = status_map
            .values()
            .any(|s| matches!(s, PersistStepStatus::Failed));
        let all_completed = def
            .steps
            .keys()
            .all(|s| matches!(status_map.get(s), Some(PersistStepStatus::Completed)));
        let run_status = if any_failed {
            AdvanceRunStatus::Failed
        } else if all_completed {
            AdvanceRunStatus::Completed
        } else {
            AdvanceRunStatus::Running
        };
        Ok(AdvanceResult {
            newly_pending,
            newly_requeued,
            run_status,
            all_step_statuses: status_map,
        })
    }

    /// Sprint 0.5-S6: close approval / external-trigger gates whose
    /// sentinel has been written to `metadata.approvals` /
    /// `metadata.triggers`. Synthesizes `Completed` (approved or
    /// triggered) or `Failed` (rejected) checkpoints, mirroring the
    /// in-process resume sentinel pass — but lives here so the
    /// distributed wait driver advances past gates without requiring
    /// a separate `boruna workflow resume` call.
    ///
    /// Returns the (step_id, new_status) pairs that were transitioned
    /// so the caller can update its in-memory `status_map`.
    ///
    /// **Idempotent** — re-running on a chain that already has the
    /// terminal checkpoint + audit event is a no-op. Synthetic
    /// approval output matches the in-process path: a JSON-encoded
    /// empty `Map`. Synthetic trigger output is the operator's
    /// payload as the step's `result`, again matching in-process
    /// resume semantics.
    #[cfg(feature = "persist-sqlite")]
    fn close_resolved_gates(
        store: &RunCheckpointStore,
        run_id: &str,
        def: &WorkflowDef,
    ) -> Result<Vec<(String, PersistStepStatus)>, WorkflowRunError> {
        use crate::workflow::definition::StepKind;

        let metadata_json = match store
            .get_run_metadata(run_id)
            .map_err(WorkflowRunError::from)?
        {
            Some(j) => j,
            None => return Ok(Vec::new()),
        };
        let metadata: PersistedRunMetadata = serde_json::from_str(&metadata_json)
            .map_err(|e| WorkflowRunError::Internal(format!("metadata parse: {e}")))?;
        let checkpoints = store
            .list_step_checkpoints(run_id)
            .map_err(WorkflowRunError::from)?;
        let mut transitions: Vec<(String, PersistStepStatus)> = Vec::new();

        // Approval sentinels: each metadata.approvals entry advances
        // its AwaitingApproval checkpoint to Completed (approved) or
        // Failed (rejected). Already-terminal checkpoints (Completed
        // or Failed) are no-ops. Out-of-state checkpoints (Pending /
        // Running etc.) are skipped with a warning — same posture as
        // resume's sentinel pass.
        for (step_id, approval) in &metadata.approvals {
            let cp = match checkpoints.iter().find(|c| &c.step_id == step_id) {
                Some(cp) => cp,
                None => continue,
            };
            if matches!(
                cp.status,
                PersistStepStatus::Completed | PersistStepStatus::Failed
            ) {
                continue;
            }
            if cp.status != PersistStepStatus::AwaitingApproval {
                continue;
            }
            let step_def = def.steps.get(step_id).ok_or_else(|| {
                WorkflowRunError::Internal(format!(
                    "approval sentinel for step '{step_id}' but step not in def"
                ))
            })?;
            if !matches!(step_def.kind, StepKind::ApprovalGate { .. }) {
                return Err(WorkflowRunError::Internal(format!(
                    "approval sentinel for non-ApprovalGate step '{step_id}'"
                )));
            }

            let now = now_unix_ms();
            match approval.decision {
                ApprovalKind::Approved => {
                    // Synthetic empty-map output, byte-identical to
                    // the in-process resume sentinel path so a run
                    // approved via either route hashes to the same
                    // bundle.
                    let synthetic = boruna_bytecode::Value::Map(BTreeMap::new());
                    let output_json = serde_json::to_string(&synthetic).map_err(|e| {
                        WorkflowRunError::Internal(format!("synthetic output serialize: {e}"))
                    })?;
                    let output_hash = DataStore::hash_value(&synthetic);
                    let (routed_json, routed_blob_ref) =
                        route_output(output_json, store.blob_store());
                    store
                        .upsert_step_checkpoint(&StepCheckpoint {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                            status: PersistStepStatus::Completed,
                            output_json: routed_json,
                            output_hash: Some(output_hash.clone()),
                            started_at_ms: None, // COALESCE preserves
                            ended_at_ms: Some(now),
                            error_msg: None,
                            attempt_count: 1,
                            worker_id: None,
                            lease_expires_at_ms: None,
                            claim_id: 0,
                            output_blob_ref: routed_blob_ref,
                        })
                        .map_err(WorkflowRunError::from)?;
                    transitions.push((step_id.clone(), PersistStepStatus::Completed));
                }
                ApprovalKind::Rejected => {
                    let reason = approval
                        .reason
                        .clone()
                        .unwrap_or_else(|| "rejected".to_string());
                    store
                        .upsert_step_checkpoint(&StepCheckpoint {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                            status: PersistStepStatus::Failed,
                            output_json: None,
                            output_hash: None,
                            started_at_ms: None,
                            ended_at_ms: Some(now),
                            error_msg: Some(format!("rejected: {reason}")),
                            attempt_count: 1,
                            worker_id: None,
                            lease_expires_at_ms: None,
                            claim_id: 0,
                            output_blob_ref: None,
                        })
                        .map_err(WorkflowRunError::from)?;
                    transitions.push((step_id.clone(), PersistStepStatus::Failed));
                }
            }
        }

        // Trigger sentinels: AwaitingExternalEvent + non-empty payload
        // in metadata.triggers → advance Completed with payload as the
        // step's `result` output value. Pause-time placeholder
        // (empty payload) is skipped — gate stays open.
        for (step_id, trigger) in &metadata.triggers {
            if trigger.payload.is_empty() {
                continue;
            }
            let cp = match checkpoints.iter().find(|c| &c.step_id == step_id) {
                Some(cp) => cp,
                None => continue,
            };
            if matches!(
                cp.status,
                PersistStepStatus::Completed | PersistStepStatus::Failed
            ) {
                continue;
            }
            if cp.status != PersistStepStatus::AwaitingExternalEvent {
                continue;
            }
            let step_def = def.steps.get(step_id).ok_or_else(|| {
                WorkflowRunError::Internal(format!(
                    "trigger sentinel for step '{step_id}' but step not in def"
                ))
            })?;
            if !matches!(step_def.kind, StepKind::ExternalTrigger { .. }) {
                return Err(WorkflowRunError::Internal(format!(
                    "trigger sentinel for non-ExternalTrigger step '{step_id}'"
                )));
            }

            // Output is the trigger payload as a String value, same as
            // the in-process resume path's synthesis.
            let synthetic = boruna_bytecode::Value::String(trigger.payload.clone());
            let output_json = serde_json::to_string(&synthetic).map_err(|e| {
                WorkflowRunError::Internal(format!("trigger output serialize: {e}"))
            })?;
            let output_hash = DataStore::hash_value(&synthetic);
            let (routed_json, routed_blob_ref) = route_output(output_json, store.blob_store());
            store
                .upsert_step_checkpoint(&StepCheckpoint {
                    run_id: run_id.to_string(),
                    step_id: step_id.clone(),
                    status: PersistStepStatus::Completed,
                    output_json: routed_json,
                    output_hash: Some(output_hash),
                    started_at_ms: None,
                    ended_at_ms: Some(now_unix_ms()),
                    error_msg: None,
                    attempt_count: 1,
                    worker_id: None,
                    lease_expires_at_ms: None,
                    claim_id: 0,
                    output_blob_ref: routed_blob_ref,
                })
                .map_err(WorkflowRunError::from)?;
            transitions.push((step_id.clone(), PersistStepStatus::Completed));
        }

        Ok(transitions)
    }

    /// Append a `WorkflowCompleted` audit event to the run's
    /// hash-chained `metadata.audit_log` when a wait-driven run
    /// reaches a terminal status. Sprint follow-up to `0.5-S2f`:
    /// the wait driver's terminating event was missing from the
    /// audit chain, leaving distributed runs with a `WorkflowStarted`
    /// genesis but no closing entry.
    ///
    /// **Idempotent.** If the chain already contains a
    /// `WorkflowCompleted` event (e.g. the wait driver was
    /// re-invoked against an already-Completed run, or the run
    /// originally finished in-process and a wait was attached
    /// later), this is a no-op and returns `Ok(())`.
    ///
    /// **Result hash convention** mirrors the in-process runner
    /// (`runner.rs` line ~1041 / ~1593): the `result_hash` is the
    /// `output_hash` of the LAST Completed step (sorted by
    /// step_id, the same iteration order as
    /// `list_step_checkpoints`). When no step has a hash (run
    /// failed before any step completed), falls back to a
    /// 64-zero hex string. `total_duration_ms` is `0` because the
    /// wait driver does not track per-run wall-clock duration —
    /// operators read `runs.started_at` / `updated_at` columns
    /// for that, both of which are operational-only per
    /// project convention §15.
    ///
    /// **Best-effort failure mode.** Same posture as the in-process
    /// `WorkflowCompleted` emit: callers in the wait driver SHOULD
    /// log + continue rather than failing the run if the CAS budget
    /// exhausts. A missed terminating event is operationally
    /// annoying but not a correctness failure; the existing chain
    /// entries remain valid.
    #[cfg(feature = "persist-sqlite")]
    pub fn append_wait_terminal_audit_event(
        store: &RunCheckpointStore,
        run_id: &str,
    ) -> Result<(), WorkflowRunError> {
        let metadata_json = store
            .get_run_metadata(run_id)
            .map_err(WorkflowRunError::from)?
            .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;
        let metadata: PersistedRunMetadata = serde_json::from_str(&metadata_json).map_err(|e| {
            WorkflowRunError::Internal(format!("corrupt metadata_json for run '{run_id}': {e}"))
        })?;
        // Idempotent: skip if a WorkflowCompleted entry already
        // exists in the chain. Two concurrent wait clients reaching
        // terminal at the same moment would otherwise emit two
        // entries; this guard collapses to one.
        let already_terminal = metadata.audit_log.iter().any(|entry| {
            matches!(
                entry.event,
                crate::audit::AuditEvent::WorkflowCompleted { .. }
            )
        });
        if already_terminal {
            return Ok(());
        }
        let checkpoints = store
            .list_step_checkpoints(run_id)
            .map_err(WorkflowRunError::from)?;
        let result_hash = checkpoints
            .iter()
            .rfind(|c| matches!(c.status, PersistStepStatus::Completed))
            .and_then(|c| c.output_hash.clone())
            .unwrap_or_else(|| "0".repeat(64));
        append_audit_event(
            store,
            run_id,
            crate::audit::AuditEvent::WorkflowCompleted {
                result_hash,
                total_duration_ms: 0,
            },
        )
    }

    /// Internal shared body for `run_persistent` and
    /// `run_persistent_or_skip`. Validates, opens the store, and
    /// inserts a new run row via `insert_run_with_derived_id`.
    /// Returns the opened store + the new run_id.
    #[cfg(feature = "persist-sqlite")]
    fn prepare_persistent_run(
        def: &WorkflowDef,
        options: &RunOptions,
        data_dir: &Path,
    ) -> Result<(RunCheckpointStore, String), WorkflowRunError> {
        WorkflowValidator::validate(def).map_err(|errors| {
            WorkflowRunError::Validation(
                errors
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        WorkflowValidator::topological_order(def).map_err(WorkflowRunError::Validation)?;

        let store = open_store(data_dir)?;
        let workflow_hash = Self::workflow_hash_from_def(def);
        let inputs_hash = Self::ephemeral_inputs_hash();
        let policy_json = serde_json::to_string(&options.policy)
            .map_err(|e| WorkflowRunError::Internal(format!("policy serialize: {e}")))?;
        let metadata = PersistedRunMetadata {
            workflow_dir: options.workflow_dir.clone(),
            inputs_hash: inputs_hash.clone(),
            boruna_version: env!("CARGO_PKG_VERSION").to_string(),
            approvals: BTreeMap::new(),
            triggers: BTreeMap::new(),
            audit_log: Vec::new(),
            step_sources: Self::collect_step_sources(def, &options.workflow_dir)?,
            workflow_def: Self::embed_workflow_def_for_metadata(def, options)?,
        };
        let metadata_json = serde_json::to_string(&metadata)
            .map_err(|e| WorkflowRunError::Internal(format!("metadata serialize: {e}")))?;

        let run_id = store
            .insert_run_with_derived_id(
                &def.name,
                &workflow_hash,
                &inputs_hash,
                &policy_json,
                &metadata_json,
                now_unix_ms(),
            )
            .map_err(WorkflowRunError::from)?;
        Ok((store, run_id))
    }

    /// Internal shared body — execute steps + persist terminal status —
    /// after a fresh run row has been inserted by either the
    /// non-atomic or the atomic check+insert path.
    #[cfg(feature = "persist-sqlite")]
    fn execute_after_insert(
        def: &WorkflowDef,
        options: &RunOptions,
        data_dir: &Path,
        store: &RunCheckpointStore,
        run_id: String,
    ) -> Result<WorkflowRunResult, WorkflowRunError> {
        let order =
            WorkflowValidator::topological_order(def).map_err(WorkflowRunError::Validation)?;

        // Persistent runs use a stable per-run subdir, not a tempdir, so
        // the data store survives a crash. Caller controls the parent;
        // each run gets its own folder keyed by run_id.
        let run_data_dir = data_dir.join("runs").join(&run_id);
        let mut data_store =
            DataStore::new(&run_data_dir).map_err(|e| WorkflowRunError::Io(e.to_string()))?;

        // 0.4-S11: append WorkflowStarted to the audit chain. The
        // run row was just inserted with an empty audit_log, so
        // this is the chain's genesis entry.
        let policy_hash_seed = serde_json::to_string(&options.policy).unwrap_or_default();
        if let Err(e) = append_audit_event(
            store,
            &run_id,
            crate::audit::AuditEvent::WorkflowStarted {
                workflow_hash: Self::workflow_hash_from_def(def),
                policy_hash: sha256_hex(&policy_hash_seed),
            },
        ) {
            // Best-effort logging; don't shadow the workflow's result.
            eprintln!(
                "warning: failed to append WorkflowStarted audit event for run '{run_id}': {e}"
            );
        }

        let result = if options.concurrency > 1 {
            // Wave-based concurrent execution. Compute levels once;
            // the wave loop handles dispatch + halt semantics.
            let levels =
                WorkflowValidator::topological_levels(def).map_err(WorkflowRunError::Validation)?;
            Self::execute_steps_concurrent(
                def,
                &levels,
                options,
                &run_id,
                &mut data_store,
                BTreeSet::new(),
                &BTreeMap::new(),
                store,
            )
        } else {
            Self::execute_steps(
                def,
                &order,
                options,
                &run_id,
                &mut data_store,
                BTreeSet::new(),
                &BTreeMap::new(),
                Some(store),
            )
        };

        // Persist terminal run status whether the body returned Ok or Err.
        // On Err we treat the run as Failed for status purposes; the typed
        // error still propagates to the caller.
        //
        // The trailing status write is operational metadata; if it fails
        // we log and return the workflow's actual outcome rather than
        // shadowing it with a persistence error. Audit-relevant state
        // (step_checkpoints, run row, policy_json, workflow_hash) was
        // already written before this point; only the final transient
        // status update can be lost, and the next resume reconciles it
        // by walking step_checkpoints.
        let final_status = match &result {
            Ok(r) => persist_status_from_workflow(&r.status),
            Err(_) => PersistRunStatus::Failed,
        };
        if let Err(e) = store.update_run_status(&run_id, final_status, now_unix_ms()) {
            eprintln!(
                "warning: failed to persist terminal status for run '{run_id}': {e} \
                 (workflow result is still authoritative; resume will reconcile)"
            );
        }

        // 0.4-S11: append WorkflowCompleted at terminal status only.
        // Pause states (Paused) leave the chain open — the next
        // resume continues appending. result_hash is the hash of the
        // last step's persisted output (deterministic over the
        // run's outputs).
        if let Ok(r) = &result {
            if matches!(r.status, WorkflowStatus::Completed | WorkflowStatus::Failed) {
                let result_hash = r
                    .step_results
                    .values()
                    .next_back()
                    .and_then(|sr| sr.output_hash.clone())
                    .unwrap_or_else(|| "0".repeat(64));
                if let Err(e) = append_audit_event(
                    store,
                    &run_id,
                    crate::audit::AuditEvent::WorkflowCompleted {
                        result_hash,
                        total_duration_ms: r.total_duration_ms,
                    },
                ) {
                    eprintln!(
                        "warning: failed to append WorkflowCompleted audit event for \
                         run '{run_id}': {e}"
                    );
                }
            }
        }

        result
    }

    /// Resume a previously-paused or crashed run.
    ///
    /// **Resume rules** (the headline determinism contract):
    ///
    /// 1. **Already-`Completed` steps** are skipped — their persisted
    ///    `output_json` is restored into the in-memory data store and
    ///    downstream steps consume it as if the step had just run.
    /// 2. **`running`-status steps at resume-time are RE-EXECUTED.** A
    ///    crash mid-step can leave any state on disk; the only safe rule
    ///    is "trust only `completed`." Re-execution is idempotent because
    ///    Boruna steps are pure functions of their inputs.
    /// 3. **`failed` steps are NOT silently re-executed.** A previously-
    ///    failed run resumes to its existing terminal state; explicit
    ///    re-runs require a fresh `run` invocation.
    /// 4. **`awaiting_approval` steps re-pause** unless the persisted
    ///    metadata's `approvals.<step_id>` sentinel is set. The CLI to
    ///    set that sentinel ships in `0.3-S2c`.
    /// 5. **`workflow_hash` mismatch refuses to resume.** Editing a step
    ///    file between run and resume invalidates the original audit
    ///    chain; the runner returns `WorkflowRunError::WorkflowHashMismatch`.
    #[cfg(feature = "persist-sqlite")]
    pub fn resume(
        run_id: &str,
        data_dir: &Path,
        options: &ResumeOptions,
    ) -> Result<WorkflowRunResult, WorkflowRunError> {
        let store = open_store(data_dir)?;

        let record = store
            .get_run_record(run_id)
            .map_err(WorkflowRunError::from)?
            .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;

        // Refuse to resume a Completed/Failed run — return the persisted
        // result rather than re-executing. Callers that want a fresh run
        // should use `run_persistent` not `resume`.
        if let Some(terminal) = record.terminal_status {
            return Self::reconstruct_terminal_result(&store, run_id, &record, terminal);
        }

        let metadata: PersistedRunMetadata =
            serde_json::from_str(&record.metadata_json).map_err(|e| {
                WorkflowRunError::Internal(format!("corrupt metadata_json for run '{run_id}': {e}"))
            })?;

        let workflow_dir = options
            .workflow_dir_override
            .clone()
            .unwrap_or(metadata.workflow_dir.clone());

        let def_path = Path::new(&workflow_dir).join("workflow.json");
        let def_json = std::fs::read_to_string(&def_path).map_err(|e| {
            WorkflowRunError::Io(format!("cannot read {}: {e}", def_path.display()))
        })?;
        let def: WorkflowDef = serde_json::from_str(&def_json)
            .map_err(|e| WorkflowRunError::Internal(format!("invalid workflow.json: {e}")))?;

        let actual_hash = Self::workflow_hash_from_def(&def);
        if actual_hash != record.workflow_hash {
            return Err(WorkflowRunError::WorkflowHashMismatch {
                run_id: run_id.to_string(),
                expected: record.workflow_hash.clone(),
                actual: actual_hash,
            });
        }

        WorkflowValidator::validate(&def).map_err(|errors| {
            WorkflowRunError::Validation(
                errors
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        let order =
            WorkflowValidator::topological_order(&def).map_err(WorkflowRunError::Validation)?;

        // Restore data store. Persistent runs keep their step output JSONs
        // alongside the runs.db file under data_dir/runs/<run_id>/outputs.
        let run_data_dir = data_dir.join("runs").join(run_id);
        let mut data_store =
            DataStore::new(&run_data_dir).map_err(|e| WorkflowRunError::Io(e.to_string()))?;

        // Walk persisted checkpoints. Build:
        //   - already_completed: step ids whose output should be restored
        //   - prior_results: their persisted StepResult shape (status,
        //     output_hash, capabilities) so the returned WorkflowRunResult
        //     reflects them
        // Restore output JSON into the data store so downstream step
        // input resolution works.
        let mut already_completed: BTreeSet<String> = BTreeSet::new();
        let mut prior_results: BTreeMap<String, StepResult> = BTreeMap::new();
        let mut halt_with_failed_step: Option<String> = None;
        let checkpoints = store
            .list_step_checkpoints(run_id)
            .map_err(WorkflowRunError::from)?;
        for cp in &checkpoints {
            match cp.status {
                PersistStepStatus::Completed => {
                    already_completed.insert(cp.step_id.clone());
                    // Sprint 0.5-S7: large outputs live in the blob
                    // store; the row's output_json is None but the row
                    // has output_blob_ref set. read_step_output
                    // resolves either source.
                    if let Some(output_json) = store
                        .read_step_output(run_id, &cp.step_id)
                        .map_err(WorkflowRunError::from)?
                    {
                        let value: boruna_bytecode::Value = serde_json::from_str(&output_json)
                            .map_err(|e| {
                                WorkflowRunError::Internal(format!(
                                    "corrupt output_json for step '{}': {e}",
                                    cp.step_id
                                ))
                            })?;
                        data_store
                            .store_output(&cp.step_id, "result", &value)
                            .map_err(|e| WorkflowRunError::Io(e.to_string()))?;
                    }
                    prior_results.insert(
                        cp.step_id.clone(),
                        StepResult {
                            step_id: cp.step_id.clone(),
                            status: StepStatus::Completed,
                            output_hash: cp.output_hash.clone(),
                            duration_ms: 0,
                            capabilities_used: vec![],
                            error: None,
                            attempt_count: 1,
                        },
                    );
                }
                PersistStepStatus::Failed => {
                    // A failed step in a non-terminal run row is unusual
                    // — the runner would normally have set the run to
                    // Failed when execute_steps returned with a failed
                    // step. Most likely path here: the original run
                    // crashed AFTER persisting the step's failure but
                    // BEFORE its trailing update_run_status. Honour it as
                    // terminal: don't re-execute the failed step or any
                    // downstream step. Halt with the persisted Failed
                    // status, equivalent to how a fresh run would treat
                    // a step failure.
                    prior_results.insert(
                        cp.step_id.clone(),
                        StepResult {
                            step_id: cp.step_id.clone(),
                            status: StepStatus::Failed,
                            output_hash: cp.output_hash.clone(),
                            duration_ms: 0,
                            capabilities_used: vec![],
                            error: cp.error_msg.clone(),
                            attempt_count: 1,
                        },
                    );
                    halt_with_failed_step = Some(cp.step_id.clone());
                }
                // Pending / Running / AwaitingApproval: re-execute. The
                // running-on-resume case is the crash-mid-step scenario:
                // we do NOT trust any partial output. Don't seed
                // already_completed; let execute_steps run them fresh.
                //
                // AwaitingApproval steps with a metadata.approvals
                // sentinel are handled by the post-loop pass below —
                // approved gates are upgraded to already_completed,
                // rejected gates set halt_with_failed_step. Without a
                // sentinel, AwaitingApproval falls through here and
                // execute_steps re-encounters the gate, re-pausing it.
                _ => {}
            }
        }

        // 0.3-S2c: honor approval-gate sentinels recorded by
        // `record_approval_decision`. Iterate `metadata.approvals` and
        // process each only if the corresponding step is still
        // `awaiting_approval`. Two failure modes worth surfacing:
        //
        // - Sentinel for a step whose checkpoint is already terminal
        //   (Completed/Failed): the sentinel was decoration after the
        //   fact. No-op silently — the run already advanced past the
        //   gate via a prior resume.
        //
        // - Sentinel for a step that's missing a checkpoint, or whose
        //   checkpoint is Pending/Running: the operator's intent
        //   doesn't apply. Print a warning so the operator notices their
        //   approval isn't taking effect (rather than silently ignoring
        //   it). Reviewed in 0.3-S2c (correctness #3): the prior
        //   silent-no-op contradicted the sibling doc comment.
        //
        // Defense-in-depth: re-validate that the step is actually a
        // StepKind::ApprovalGate in the workflow def before applying the
        // sentinel. The workflow_hash check at resume entry refuses
        // mismatches, so this guard is paranoid; it catches a future bug
        // where validation gets bypassed (integrity H3).
        for (step_id, approval) in &metadata.approvals {
            let cp = checkpoints.iter().find(|c| &c.step_id == step_id);
            let cp_status = cp.map(|c| c.status);
            match cp_status {
                Some(PersistStepStatus::AwaitingApproval) => {
                    // Eligible — fall through.
                }
                Some(PersistStepStatus::Completed | PersistStepStatus::Failed) => {
                    // Decoration after the fact. Common no-op.
                    continue;
                }
                Some(other) => {
                    eprintln!(
                        "warning: approval sentinel for step '{step_id}' in run '{run_id}' \
                         ignored: checkpoint is in state '{}' (expected awaiting_approval)",
                        other.as_str()
                    );
                    continue;
                }
                None => {
                    eprintln!(
                        "warning: approval sentinel for step '{step_id}' in run '{run_id}' \
                         ignored: no checkpoint exists (workflow has not reached the gate)"
                    );
                    continue;
                }
            }

            // Defense-in-depth: refuse to apply a sentinel to a step
            // whose StepDef.kind is not ApprovalGate. Should never
            // happen given workflow_hash protection + record_approval_
            // decision validation, but the worst-case if it slipped
            // through would be silently overwriting a real step's output
            // with a synthetic empty record.
            let step_def = def.steps.get(step_id);
            if !matches!(
                step_def.map(|s| &s.kind),
                Some(StepKind::ApprovalGate { .. })
            ) {
                return Err(WorkflowRunError::Internal(format!(
                    "approval sentinel for step '{step_id}' in run '{run_id}' targets a \
                     non-ApprovalGate step (workflow_hash check should have prevented this)"
                )));
            }

            match approval.decision {
                ApprovalKind::Approved => {
                    // Advance: persist a Completed checkpoint with a
                    // synthetic empty-record output (`{}` JSON of an
                    // empty Map). Add to already_completed so
                    // execute_steps skips it; add a prior_result entry
                    // so the returned WorkflowRunResult reports it.
                    let synthetic = boruna_bytecode::Value::Map(BTreeMap::new());
                    let output_json = serde_json::to_string(&synthetic).map_err(|e| {
                        WorkflowRunError::Internal(format!("synthetic output serialize: {e}"))
                    })?;
                    let output_hash = DataStore::hash_value(&synthetic);
                    let (routed_json, routed_blob_ref) =
                        route_output(output_json, store.blob_store());
                    store
                        .upsert_step_checkpoint(&StepCheckpoint {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                            status: PersistStepStatus::Completed,
                            output_json: routed_json,
                            output_hash: Some(output_hash.clone()),
                            started_at_ms: None, // COALESCE preserves
                            ended_at_ms: Some(now_unix_ms()),
                            error_msg: None,
                            attempt_count: 1,
                            worker_id: None,
                            lease_expires_at_ms: None,
                            claim_id: 0,
                            output_blob_ref: routed_blob_ref,
                        })
                        .map_err(WorkflowRunError::from)?;
                    data_store
                        .store_output(step_id, "result", &synthetic)
                        .map_err(|e| WorkflowRunError::Io(e.to_string()))?;
                    already_completed.insert(step_id.clone());
                    prior_results.insert(
                        step_id.clone(),
                        StepResult {
                            step_id: step_id.clone(),
                            status: StepStatus::Completed,
                            output_hash: Some(output_hash),
                            duration_ms: 0,
                            capabilities_used: vec![],
                            error: None,
                            attempt_count: 1,
                        },
                    );
                }
                ApprovalKind::Rejected => {
                    let err_msg = approval
                        .reason
                        .clone()
                        .unwrap_or_else(|| "rejected by operator".to_string());
                    store
                        .upsert_step_checkpoint(&StepCheckpoint {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                            status: PersistStepStatus::Failed,
                            output_json: None,
                            output_hash: None,
                            started_at_ms: None,
                            ended_at_ms: Some(now_unix_ms()),
                            error_msg: Some(err_msg.clone()),
                            attempt_count: 1,
                            worker_id: None,
                            lease_expires_at_ms: None,
                            claim_id: 0,
                            output_blob_ref: None,
                        })
                        .map_err(WorkflowRunError::from)?;
                    prior_results.insert(
                        step_id.clone(),
                        StepResult {
                            step_id: step_id.clone(),
                            status: StepStatus::Failed,
                            output_hash: None,
                            duration_ms: 0,
                            capabilities_used: vec![],
                            error: Some(err_msg),
                            attempt_count: 1,
                        },
                    );
                    // get_or_insert: preserve the FIRST failure as the
                    // halt cause. If a step in the original run already
                    // failed (set in the prior loop above), that's the
                    // operator's actual failure to chase, not a later
                    // rejection sentinel. Reviewed in 0.3-S2c
                    // (correctness #2: prior `= Some(...)` overwrote
                    // an earlier independent failure).
                    halt_with_failed_step.get_or_insert(step_id.clone());
                }
            }
        }

        // 0.3-S15: honor external-trigger sentinels recorded by
        // `record_external_trigger`. Mirrors the approval-sentinel pass
        // above. A trigger sentinel is identified by a non-empty
        // `payload` field on the `TriggerRecord` — pause-time inserts a
        // placeholder with `payload: ""` to stash the token; trigger-
        // time fills in the payload.
        //
        // For each trigger with a non-empty payload whose checkpoint is
        // still `awaiting_external_event`: persist a Completed
        // checkpoint, store the payload as the step's output (as a JSON
        // String value so downstream `step_input(...)` returns it
        // verbatim), add to already_completed, and seed prior_results.
        //
        // Same warning surface for misaligned sentinels as approvals
        // (see comments above), and same defense-in-depth check that
        // the StepDef.kind is actually ExternalTrigger.
        for (step_id, trigger) in &metadata.triggers {
            if trigger.payload.is_empty() {
                // Pause-time placeholder; no actual trigger arrived yet.
                continue;
            }
            let cp = checkpoints.iter().find(|c| &c.step_id == step_id);
            let cp_status = cp.map(|c| c.status);
            match cp_status {
                Some(PersistStepStatus::AwaitingExternalEvent) => {
                    // Eligible — fall through.
                }
                Some(PersistStepStatus::Completed | PersistStepStatus::Failed) => {
                    continue;
                }
                Some(other) => {
                    eprintln!(
                        "warning: trigger sentinel for step '{step_id}' in run '{run_id}' \
                         ignored: checkpoint is in state '{}' (expected awaiting_external_event)",
                        other.as_str()
                    );
                    continue;
                }
                None => {
                    eprintln!(
                        "warning: trigger sentinel for step '{step_id}' in run '{run_id}' \
                         ignored: no checkpoint exists (workflow has not reached the gate)"
                    );
                    continue;
                }
            }

            let step_def = def.steps.get(step_id);
            if !matches!(
                step_def.map(|s| &s.kind),
                Some(StepKind::ExternalTrigger { .. })
            ) {
                return Err(WorkflowRunError::Internal(format!(
                    "trigger sentinel for step '{step_id}' in run '{run_id}' targets a \
                     non-ExternalTrigger step (workflow_hash check should have prevented this)"
                )));
            }

            // Payload is treated as an opaque JSON-encoded string. The
            // step's output Value is `Value::String(payload)` — downstream
            // steps read it via `step_input(name)` (sprint 0.3-S14) and
            // parse the JSON inline if they want typed access. This
            // mirrors the BYOH net-handler pattern: the receiver bridges
            // raw bytes; the .ax program decodes.
            let synthetic = boruna_bytecode::Value::String(trigger.payload.clone());
            let output_json = serde_json::to_string(&synthetic).map_err(|e| {
                WorkflowRunError::Internal(format!("trigger output serialize: {e}"))
            })?;
            let output_hash = DataStore::hash_value(&synthetic);
            let (routed_json, routed_blob_ref) = route_output(output_json, store.blob_store());
            store
                .upsert_step_checkpoint(&StepCheckpoint {
                    run_id: run_id.to_string(),
                    step_id: step_id.clone(),
                    status: PersistStepStatus::Completed,
                    output_json: routed_json,
                    output_hash: Some(output_hash.clone()),
                    started_at_ms: None,
                    ended_at_ms: Some(now_unix_ms()),
                    error_msg: None,
                    attempt_count: 1,
                    worker_id: None,
                    lease_expires_at_ms: None,
                    claim_id: 0,
                    output_blob_ref: routed_blob_ref,
                })
                .map_err(WorkflowRunError::from)?;
            data_store
                .store_output(step_id, "result", &synthetic)
                .map_err(|e| WorkflowRunError::Io(e.to_string()))?;
            already_completed.insert(step_id.clone());
            prior_results.insert(
                step_id.clone(),
                StepResult {
                    step_id: step_id.clone(),
                    status: StepStatus::Completed,
                    output_hash: Some(output_hash),
                    duration_ms: 0,
                    capabilities_used: vec![],
                    error: None,
                    attempt_count: 1,
                },
            );
        }

        if halt_with_failed_step.is_some() {
            // Refuse to advance past a previously-failed step. Set the
            // run to Failed and return — without re-executing anything
            // or touching the in-memory data store further. The CLI
            // surfaces this exactly as a fresh failed-run does.
            store
                .update_run_status(run_id, PersistRunStatus::Failed, now_unix_ms())
                .map_err(WorkflowRunError::from)?;
            return Ok(WorkflowRunResult {
                run_id: run_id.to_string(),
                workflow_name: def.name.clone(),
                status: WorkflowStatus::Failed,
                step_results: prior_results,
                total_duration_ms: 0,
            });
        }

        // Resume policy resolution: per the documented contract on
        // ResumeOptions, policy=None means "use the original run's
        // persisted policy." Without this, every step would silently fall
        // through to Policy::deny_all and fail any capability check.
        let resume_policy = match options.policy.clone() {
            Some(p) => Some(p),
            None => {
                if record.policy_json.is_empty() || record.policy_json == "null" {
                    None
                } else {
                    serde_json::from_str(&record.policy_json).map_err(|e| {
                        WorkflowRunError::Internal(format!(
                            "corrupt persisted policy_json for run '{run_id}': {e}"
                        ))
                    })?
                }
            }
        };

        let synthesized_options = RunOptions {
            policy: resume_policy,
            record: options.record,
            workflow_dir,
            live: options.live,
            concurrency: options.concurrency.max(1),
            // Resume always executes in-process; submit-only is
            // a fresh-run-only mode (sprint 0.5-S2e).
            submit_only: false,
        };

        // Reset run status to Running for the resume window.
        store
            .update_run_status(run_id, PersistRunStatus::Running, now_unix_ms())
            .map_err(WorkflowRunError::from)?;

        let result = if synthesized_options.concurrency > 1 {
            let levels = WorkflowValidator::topological_levels(&def)
                .map_err(WorkflowRunError::Validation)?;
            Self::execute_steps_concurrent(
                &def,
                &levels,
                &synthesized_options,
                run_id,
                &mut data_store,
                already_completed,
                &prior_results,
                &store,
            )
        } else {
            Self::execute_steps(
                &def,
                &order,
                &synthesized_options,
                run_id,
                &mut data_store,
                already_completed,
                &prior_results,
                Some(&store),
            )
        };

        // Persist terminal run status. If this update fails, log it and
        // return the actual workflow result anyway — the trailing status
        // write is operational metadata; losing it must not lose the
        // run's outcome (a Persistence error here would otherwise mask
        // a step failure or a successful workflow result, breaking
        // observability for the caller). The next resume will recover
        // the row state by walking step_checkpoints.
        let final_status = match &result {
            Ok(r) => persist_status_from_workflow(&r.status),
            Err(_) => PersistRunStatus::Failed,
        };
        if let Err(e) = store.update_run_status(run_id, final_status, now_unix_ms()) {
            eprintln!(
                "warning: failed to persist terminal status for run '{run_id}': {e} \
                 (workflow result is still authoritative; resume will reconcile)"
            );
        }

        // 0.4-S11: append WorkflowCompleted on terminal status. Same
        // logic as `execute_after_insert` — resume terminates the
        // run in the same way, so the audit chain closes here.
        if let Ok(r) = &result {
            if matches!(r.status, WorkflowStatus::Completed | WorkflowStatus::Failed) {
                let result_hash = r
                    .step_results
                    .values()
                    .next_back()
                    .and_then(|sr| sr.output_hash.clone())
                    .unwrap_or_else(|| "0".repeat(64));
                if let Err(e) = append_audit_event(
                    &store,
                    run_id,
                    crate::audit::AuditEvent::WorkflowCompleted {
                        result_hash,
                        total_duration_ms: r.total_duration_ms,
                    },
                ) {
                    eprintln!(
                        "warning: failed to append WorkflowCompleted audit event for \
                         resume of run '{run_id}': {e}"
                    );
                }
            }
        }

        result
    }

    /// Stable workflow-hash derivation. Hashes the *canonical JSON* form
    /// of the definition (sorted via BTreeMap), not the on-disk bytes —
    /// so cosmetic file edits (whitespace, key order) don't change the
    /// hash.
    pub fn workflow_hash_from_def(def: &WorkflowDef) -> String {
        let canonical = serde_json::to_string(def).unwrap_or_default();
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Inputs hash for the runner's current "no external workflow inputs"
    /// shape. A future sprint adding workflow-level params will replace
    /// this with a real serialization.
    fn ephemeral_inputs_hash() -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"{}");
        format!("{:x}", hasher.finalize())
    }

    /// Reconstruct a `WorkflowRunResult` from persisted state for a run
    /// that has already reached a terminal status. Used when `resume` is
    /// called on a `Completed`/`Failed` run.
    #[cfg(feature = "persist-sqlite")]
    fn reconstruct_terminal_result(
        store: &RunCheckpointStore,
        run_id: &str,
        record: &crate::persistence::RunRecord,
        terminal: PersistRunStatus,
    ) -> Result<WorkflowRunResult, WorkflowRunError> {
        let checkpoints = store
            .list_step_checkpoints(run_id)
            .map_err(WorkflowRunError::from)?;
        let mut step_results = BTreeMap::new();
        for cp in checkpoints {
            let status = match cp.status {
                PersistStepStatus::Completed => StepStatus::Completed,
                PersistStepStatus::Failed => StepStatus::Failed,
                PersistStepStatus::AwaitingApproval => StepStatus::AwaitingApproval,
                PersistStepStatus::AwaitingExternalEvent => StepStatus::AwaitingExternalEvent,
                PersistStepStatus::Running => StepStatus::Running,
                PersistStepStatus::Pending => StepStatus::Pending,
            };
            step_results.insert(
                cp.step_id.clone(),
                StepResult {
                    step_id: cp.step_id,
                    status,
                    output_hash: cp.output_hash,
                    duration_ms: 0,
                    capabilities_used: vec![],
                    error: cp.error_msg,
                    attempt_count: 1,
                },
            );
        }
        let workflow_status = match terminal {
            PersistRunStatus::Completed => WorkflowStatus::Completed,
            PersistRunStatus::Failed => WorkflowStatus::Failed,
            // Unreachable per the caller's terminal_status branch, but
            // keep the match exhaustive instead of unwrap-style panic.
            _ => WorkflowStatus::Failed,
        };
        Ok(WorkflowRunResult {
            run_id: run_id.to_string(),
            workflow_name: record.workflow_name.clone(),
            status: workflow_status,
            step_results,
            total_duration_ms: 0,
        })
    }

    /// Shared step-execution loop. Used by both `run` (no store) and
    /// `run_persistent` / `resume` (with store).
    ///
    /// `already_completed` is the set of step ids whose output is already
    /// persisted and should NOT be re-executed (resume's skip set).
    /// `prior_results` carries the StepResult shape for those skipped
    /// steps so the returned WorkflowRunResult reports them correctly.
    #[allow(clippy::too_many_arguments)]
    /// Wave-based concurrent step execution. Persistent path only —
    /// the ephemeral [`run`](Self::run) path stays sequential.
    ///
    /// Each topological level (a "wave") is processed in parallel up
    /// to `options.concurrency` workers. Within a wave, source steps
    /// are dispatched to short-lived `std::thread::spawn`'d workers
    /// that compile + run a fresh VM and return the resulting `Value`.
    /// The coordinator (this function, on the calling thread) owns
    /// all SQLite + DataStore mutation: it writes the `Running`
    /// checkpoint before dispatch and the `Completed`/`Failed`
    /// checkpoint after collecting the worker's result. Workers hold
    /// no shared mutable state — every Value flows back via the
    /// `JoinHandle`.
    ///
    /// Approval gates inside a wave are NOT dispatched to workers —
    /// the coordinator detects them inline and pauses the run.
    ///
    /// **Determinism contract:** for any concurrency level ≥ 1, the
    /// per-step `output_hash` and the persisted `output_json` are
    /// bit-identical to a sequential run. Wall-clock fields
    /// (`started_at_ms`, `ended_at_ms`, run-level `total_duration_ms`)
    /// vary — they're operational-only per project-conventions §15.
    ///
    /// Introduced in `0.3-S4`.
    #[cfg(feature = "persist-sqlite")]
    #[allow(clippy::too_many_arguments)]
    fn execute_steps_concurrent(
        def: &WorkflowDef,
        levels: &[Vec<String>],
        options: &RunOptions,
        run_id: &str,
        data_store: &mut DataStore,
        already_completed: BTreeSet<String>,
        prior_results: &BTreeMap<String, StepResult>,
        store: &RunCheckpointStore,
    ) -> Result<WorkflowRunResult, WorkflowRunError> {
        let run_start = Instant::now();
        let mut step_results: BTreeMap<String, StepResult> = prior_results.clone();
        let mut workflow_status = WorkflowStatus::Running;
        let max_concurrency = options.concurrency.max(1);

        'outer: for level in levels {
            // Filter out skip-on-resume steps and partition into
            // pause-causing kinds (approval gates, external triggers)
            // vs source steps. Pauses are sequential — they halt the
            // wave for an external decision/event.
            let mut pauses: Vec<&str> = Vec::new();
            let mut sources: Vec<&str> = Vec::new();
            for id in level {
                if already_completed.contains(id) {
                    continue;
                }
                let step_def = def
                    .steps
                    .get(id)
                    .ok_or_else(|| WorkflowRunError::Internal(format!("step not found: {id}")))?;
                match &step_def.kind {
                    StepKind::ApprovalGate { .. } | StepKind::ExternalTrigger { .. } => {
                        pauses.push(id.as_str())
                    }
                    StepKind::Source { .. } => sources.push(id.as_str()),
                }
            }

            // 0.4-S7: process ALL pause-steps in this wave, not just
            // the first. Multiple pauses at the same DAG level enables
            // "wait for payment AND fraud-check" webhook fan-in
            // patterns. Each pause persists its own checkpoint and
            // (for ExternalTrigger) mints its own token; the resume
            // sentinel pass iterates approvals and triggers
            // independently and advances each pause as its decision
            // arrives.
            //
            // Source steps in the same wave are left for resume to
            // discover — pause-causing steps still halt the wave.
            // Reviewed 0.3-S15 (residual risk) — earlier behavior
            // processed only `pauses.first()`, silently serializing
            // parallel pauses across multiple resumes.
            // **Per-pause errors are isolated.** If acquiring a token
            // or persisting a checkpoint fails for one pause (e.g.,
            // transient `/dev/urandom` error, CAS retry exhaustion),
            // the loop logs the error and continues to the next pause.
            // The run is still marked Paused on the pauses that DID
            // commit, leaving operators with a recoverable state. The
            // next resume's wave loop is idempotent —
            // `acquire_trigger_token` reuses existing tokens and
            // `upsert_step_checkpoint` is re-write-safe — so the
            // failed pauses retry cleanly.
            //
            // Reviewed 0.4-S7 — earlier draft propagated the first
            // per-pause error via `?`, which terminally-failed the run
            // via `run_persistent`'s trailing `update_run_status`,
            // stranding any pause #1 token with no recovery path
            // (resume short-circuits on terminal status; record_*
            // entry points refuse RunNotResumable). Isolating per-pause
            // errors preserves the invariant that a partial wave
            // commit is always advanced via subsequent resume.
            if !pauses.is_empty() {
                let now = now_unix_ms();
                for pause_id in &pauses {
                    match persist_one_pause(def, store, run_id, pause_id, now) {
                        Ok(ax_status) => {
                            step_results.insert(
                                pause_id.to_string(),
                                StepResult {
                                    step_id: pause_id.to_string(),
                                    status: ax_status,
                                    output_hash: None,
                                    duration_ms: 0,
                                    capabilities_used: vec![],
                                    error: None,
                                    attempt_count: 1,
                                },
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "warning: failed to persist pause for step '{pause_id}' in run \
                                 '{run_id}': {e}; will retry on next resume"
                            );
                        }
                    }
                }
                workflow_status = WorkflowStatus::Paused;
                break 'outer;
            }

            // Source steps: pre-validate ALL chunk inputs, then mark
            // every chunk member Running atomically (no half-marked
            // state on input failure), then dispatch + join + process.
            //
            // Reviewed 0.3-S4 (correctness #1, #2, #3):
            // - #2: prior code marked steps Running interleaved with
            //   per-step input validation, so an input failure mid-
            //   chunk left earlier members Running on disk forever.
            //   Fixed via two-pass validation.
            // - #1: prior `?`-on-error inside the join loop dropped
            //   subsequent JoinHandles, detaching their threads.
            //   Fixed by collecting all join results into a Vec first.
            // - #3: panic handler now tries `String` payloads first
            //   (the common shape from `panic!("{}", ...)`), and we
            //   carry the step_id alongside each JoinHandle so a
            //   panic produces a Failed checkpoint instead of leaving
            //   the step Running forever.
            for chunk in sources.chunks(max_concurrency) {
                // Pass 1: validate inputs for EVERY chunk member
                // before any side effect. If any fails, halt without
                // marking anyone Running.
                let mut input_failures: Vec<(String, String)> = Vec::new();
                for &step_id in chunk {
                    let step_def = &def.steps[step_id];
                    if let Err(e) = data_store.resolve_step_inputs(&step_def.inputs) {
                        input_failures
                            .push((step_id.to_string(), format!("input resolution: {e}")));
                    }
                }
                if !input_failures.is_empty() {
                    // Persist Failed for every step that failed input
                    // validation; record the FIRST failure as the run's
                    // failure cause. Don't touch other chunk members:
                    // they remain unstarted (no Running checkpoint).
                    for (failed_id, err_msg) in &input_failures {
                        store
                            .upsert_step_checkpoint(&StepCheckpoint {
                                run_id: run_id.to_string(),
                                step_id: failed_id.clone(),
                                status: PersistStepStatus::Failed,
                                output_json: None,
                                output_hash: None,
                                started_at_ms: None,
                                ended_at_ms: Some(now_unix_ms()),
                                error_msg: Some(err_msg.clone()),
                                attempt_count: 1,
                                worker_id: None,
                                lease_expires_at_ms: None,
                                claim_id: 0,
                                output_blob_ref: None,
                            })
                            .map_err(WorkflowRunError::from)?;
                        step_results.insert(
                            failed_id.clone(),
                            StepResult {
                                step_id: failed_id.clone(),
                                status: StepStatus::Failed,
                                output_hash: None,
                                duration_ms: 0,
                                capabilities_used: vec![],
                                error: Some(err_msg.clone()),
                                attempt_count: 1,
                            },
                        );
                    }
                    workflow_status = WorkflowStatus::Failed;
                    break 'outer;
                }

                // Pass 2: mark every chunk member Running. Build the
                // dispatch list. After this pass every step is in the
                // Running state on disk; the post-join loop is
                // responsible for transitioning EACH ONE to a terminal
                // state.
                //
                // 0.3-S14: also resolve each step's inputs from the
                // coordinator-side data store and pass into the
                // worker. The worker holds no DataStore; inputs are
                // a value-typed snapshot taken at dispatch time.
                // Pass 1 already proved each step's inputs resolve,
                // so re-resolving here is guaranteed to succeed
                // (same coordinator, no concurrent mutation between
                // passes).
                #[allow(clippy::type_complexity)]
                let mut dispatches: Vec<(
                    String,
                    StepDef,
                    String,
                    BTreeMap<String, boruna_bytecode::Value>,
                )> = Vec::new();
                for &step_id in chunk {
                    let step_def = def.steps[step_id].clone();
                    let started_at_ms = now_unix_ms();
                    store
                        .mark_step_running_clearing_output(run_id, step_id, started_at_ms)
                        .map_err(WorkflowRunError::from)?;
                    let source_path = match &step_def.kind {
                        StepKind::Source { source } => source.clone(),
                        _ => unreachable!(),
                    };
                    let resolved_inputs = data_store
                        .resolve_step_inputs(&step_def.inputs)
                        .map_err(|e| {
                            WorkflowRunError::Internal(format!(
                                "input resolution drift between pass 1 and pass 2 \
                                 for step '{step_id}': {e}"
                            ))
                        })?;
                    dispatches.push((step_id.to_string(), step_def, source_path, resolved_inputs));
                }

                // Spawn workers, tracking step_id alongside each
                // handle so a panicking thread can be attributed
                // back to its step. Workers hold no shared state.
                let workflow_dir = options.workflow_dir.clone();
                let policy = options.policy.clone();
                let live = options.live;
                let handles: Vec<(String, StepDef, std::thread::JoinHandle<_>)> = dispatches
                    .into_iter()
                    .map(|(step_id, step_def, source, resolved_inputs)| {
                        let workflow_dir = workflow_dir.clone();
                        let policy = policy.clone();
                        let id_for_thread = step_id.clone();
                        let def_for_thread = step_def.clone();
                        let start = Instant::now();
                        let h = std::thread::spawn(move || {
                            // Workers honor the same RetryPolicy as
                            // sequential execution. The retry happens
                            // INSIDE the worker thread; the chunk
                            // wave waits for ALL workers (including
                            // ones still retrying) before moving on.
                            // Wall-clock backoff is bounded by the
                            // policy's max_attempts.
                            let result = Self::compile_and_run_step_with_retry(
                                &id_for_thread,
                                &source,
                                &def_for_thread,
                                &workflow_dir,
                                &policy,
                                live,
                                resolved_inputs,
                            );
                            (result, start.elapsed().as_millis() as u64)
                        });
                        (step_id, step_def, h)
                    })
                    .collect();

                // Join EVERY handle into a results Vec before
                // processing. This guarantees no thread is left
                // detached on early-exit paths and that the
                // coordinator never returns to its caller while
                // workers are still touching the workflow_dir.
                // 0.3-S11: worker now returns (Result<(Value, u32),
                // (WorkflowRunError, u32)>, duration_ms). The inner
                // Result carries the attempt count alongside the
                // value (success) or error (failure) so we can
                // persist it in the step's checkpoint row.
                #[allow(clippy::type_complexity)]
                let joined: Vec<(
                    String,
                    StepDef,
                    std::thread::Result<(
                        Result<(boruna_bytecode::Value, u32), (WorkflowRunError, u32)>,
                        u64,
                    )>,
                )> = handles
                    .into_iter()
                    .map(|(step_id, step_def, h)| (step_id, step_def, h.join()))
                    .collect();

                // Process results. First failure (panic, runtime, or
                // worker error) sets chunk_failed; others continue to
                // be persisted in the same chunk so the on-disk state
                // honestly reflects what actually ran. Sequential
                // execution would have stopped at the first failure
                // and produced a smaller step_results map; that
                // divergence is documented in the design doc as
                // expected behavior for failed runs at concurrency >
                // 1 (review-driven 0.3-S4 finding #4).
                let mut chunk_failed = false;
                for (step_id, step_def, join_res) in joined {
                    match join_res {
                        Ok((Ok((value, attempt_count)), duration_ms)) => {
                            let output_hash = DataStore::hash_value(&value);
                            data_store
                                .store_output(&step_id, "result", &value)
                                .map_err(|e| {
                                    WorkflowRunError::StepFailed(step_id.clone(), e.to_string())
                                })?;
                            let output_json = serde_json::to_string(&value).map_err(|e| {
                                WorkflowRunError::Internal(format!("output serialize: {e}"))
                            })?;
                            let (routed_json, routed_blob_ref) =
                                route_output(output_json, store.blob_store());
                            store
                                .upsert_step_checkpoint(&StepCheckpoint {
                                    run_id: run_id.to_string(),
                                    step_id: step_id.clone(),
                                    status: PersistStepStatus::Completed,
                                    output_json: routed_json,
                                    output_hash: Some(output_hash.clone()),
                                    started_at_ms: None,
                                    ended_at_ms: Some(now_unix_ms()),
                                    error_msg: None,
                                    attempt_count,
                                    worker_id: None,
                                    lease_expires_at_ms: None,
                                    claim_id: 0,
                                    output_blob_ref: routed_blob_ref,
                                })
                                .map_err(WorkflowRunError::from)?;
                            emit_step_terminal_audit(
                                store,
                                run_id,
                                &step_id,
                                StepStatus::Completed,
                                Some(&output_hash),
                                None,
                                duration_ms,
                            );
                            step_results.insert(
                                step_id.clone(),
                                StepResult {
                                    step_id,
                                    status: StepStatus::Completed,
                                    output_hash: Some(output_hash),
                                    duration_ms,
                                    capabilities_used: step_def.capabilities.clone(),
                                    error: None,
                                    attempt_count,
                                },
                            );
                        }
                        Ok((Err((e, attempt_count)), duration_ms)) => {
                            let err_msg = e.to_string();
                            store
                                .upsert_step_checkpoint(&StepCheckpoint {
                                    run_id: run_id.to_string(),
                                    step_id: step_id.clone(),
                                    status: PersistStepStatus::Failed,
                                    output_json: None,
                                    output_hash: None,
                                    started_at_ms: None,
                                    ended_at_ms: Some(now_unix_ms()),
                                    error_msg: Some(err_msg.clone()),
                                    attempt_count,
                                    worker_id: None,
                                    lease_expires_at_ms: None,
                                    claim_id: 0,
                                    output_blob_ref: None,
                                })
                                .map_err(WorkflowRunError::from)?;
                            emit_step_terminal_audit(
                                store,
                                run_id,
                                &step_id,
                                StepStatus::Failed,
                                None,
                                Some(&err_msg),
                                duration_ms,
                            );
                            step_results.insert(
                                step_id.clone(),
                                StepResult {
                                    step_id,
                                    status: StepStatus::Failed,
                                    output_hash: None,
                                    duration_ms,
                                    capabilities_used: vec![],
                                    error: Some(err_msg),
                                    attempt_count,
                                },
                            );
                            chunk_failed = true;
                        }
                        Err(panic_payload) => {
                            // Try String first (the common shape from
                            // `panic!("...{var}...")`), then &'static
                            // str (the literal-only path), then
                            // fallback. We KNOW the step_id here via
                            // the carried tuple, so the failed
                            // checkpoint is correctly attributed.
                            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<String>()
                            {
                                s.clone()
                            } else if let Some(s) = panic_payload.downcast_ref::<&'static str>() {
                                s.to_string()
                            } else {
                                "<non-string panic>".to_string()
                            };
                            let err_msg = format!("worker panicked: {panic_msg}");
                            store
                                .upsert_step_checkpoint(&StepCheckpoint {
                                    run_id: run_id.to_string(),
                                    step_id: step_id.clone(),
                                    status: PersistStepStatus::Failed,
                                    output_json: None,
                                    output_hash: None,
                                    started_at_ms: None,
                                    ended_at_ms: Some(now_unix_ms()),
                                    error_msg: Some(err_msg.clone()),
                                    attempt_count: 1,
                                    worker_id: None,
                                    lease_expires_at_ms: None,
                                    claim_id: 0,
                                    output_blob_ref: None,
                                })
                                .map_err(WorkflowRunError::from)?;
                            emit_step_terminal_audit(
                                store,
                                run_id,
                                &step_id,
                                StepStatus::Failed,
                                None,
                                Some(&err_msg),
                                0,
                            );
                            step_results.insert(
                                step_id.clone(),
                                StepResult {
                                    step_id,
                                    status: StepStatus::Failed,
                                    output_hash: None,
                                    duration_ms: 0,
                                    capabilities_used: vec![],
                                    error: Some(err_msg),
                                    attempt_count: 1,
                                },
                            );
                            chunk_failed = true;
                        }
                    }
                }
                if chunk_failed {
                    workflow_status = WorkflowStatus::Failed;
                    break 'outer;
                }
            }
        }

        if workflow_status == WorkflowStatus::Running {
            workflow_status = WorkflowStatus::Completed;
        }

        Ok(WorkflowRunResult {
            run_id: run_id.to_string(),
            workflow_name: def.name.clone(),
            status: workflow_status,
            step_results,
            total_duration_ms: run_start.elapsed().as_millis() as u64,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_steps(
        def: &WorkflowDef,
        order: &[String],
        options: &RunOptions,
        run_id: &str,
        data_store: &mut DataStore,
        already_completed: BTreeSet<String>,
        prior_results: &BTreeMap<String, StepResult>,
        #[cfg(feature = "persist-sqlite")] store: Option<&RunCheckpointStore>,
    ) -> Result<WorkflowRunResult, WorkflowRunError> {
        let run_start = Instant::now();
        let mut step_results: BTreeMap<String, StepResult> = prior_results.clone();
        let mut workflow_status = WorkflowStatus::Running;

        for step_id in order {
            // Skip already-completed steps on resume.
            if already_completed.contains(step_id) {
                continue;
            }

            let step_def = def
                .steps
                .get(step_id)
                .ok_or_else(|| WorkflowRunError::Internal(format!("step not found: {step_id}")))?;

            let step_start = Instant::now();
            let step_started_at_ms = now_unix_ms();

            #[cfg(feature = "persist-sqlite")]
            if let Some(s) = store {
                // Use the clearing variant rather than the COALESCE-based
                // upsert: when re-executing on resume, a stale
                // output_json from the prior crashed attempt would
                // otherwise survive the transition to Running and
                // produce a contradictory on-disk row (status=Running
                // with non-null output_json that doesn't correspond to
                // any complete execution). Reviewed in 0.3-S2b.
                s.mark_step_running_clearing_output(run_id, step_id, step_started_at_ms)
                    .map_err(WorkflowRunError::from)?;
            }

            match &step_def.kind {
                StepKind::ApprovalGate { required_role, .. } => {
                    let cp = StepResult {
                        step_id: step_id.clone(),
                        status: StepStatus::AwaitingApproval,
                        output_hash: None,
                        duration_ms: 0,
                        capabilities_used: vec![],
                        error: None,
                        attempt_count: 1,
                    };
                    step_results.insert(step_id.clone(), cp);
                    workflow_status = WorkflowStatus::Paused;
                    eprintln!(
                        "Awaiting approval for step '{}' (role: {}). \
                         Run: boruna workflow approve {} {}",
                        step_id, required_role, run_id, step_id
                    );

                    #[cfg(feature = "persist-sqlite")]
                    if let Some(s) = store {
                        s.upsert_step_checkpoint(&StepCheckpoint {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                            status: PersistStepStatus::AwaitingApproval,
                            output_json: None,
                            output_hash: None,
                            started_at_ms: Some(step_started_at_ms),
                            ended_at_ms: None,
                            error_msg: None,
                            attempt_count: 1,
                            worker_id: None,
                            lease_expires_at_ms: None,
                            claim_id: 0,
                            output_blob_ref: None,
                        })
                        .map_err(WorkflowRunError::from)?;
                    }
                    break;
                }
                StepKind::ExternalTrigger { .. } => {
                    // 0.3-S15: external trigger pauses the run identically
                    // to an approval gate, but the resume mechanism is
                    // `boruna workflow trigger` instead of `approve`. The
                    // trigger payload becomes the step's output value.
                    //
                    // Ephemeral path (`store: None`) doesn't support
                    // triggers — there's no persistent metadata to stash
                    // a token in, so an operator could never advance the
                    // run. Surface a typed validation error rather than
                    // silently hanging.
                    #[cfg(feature = "persist-sqlite")]
                    let s = match store {
                        Some(s) => s,
                        None => {
                            return Err(WorkflowRunError::Validation(format!(
                                "step '{step_id}' is an external_trigger step; \
                                 external triggers require persistent runs \
                                 (use `run_persistent`, not `run`)"
                            )));
                        }
                    };
                    #[cfg(not(feature = "persist-sqlite"))]
                    {
                        return Err(WorkflowRunError::Validation(format!(
                            "step '{step_id}' is an external_trigger step; \
                             requires the `persist-sqlite` feature"
                        )));
                    }
                    #[cfg(feature = "persist-sqlite")]
                    {
                        let token = acquire_trigger_token(s, run_id, step_id)?;
                        let cp = StepResult {
                            step_id: step_id.clone(),
                            status: StepStatus::AwaitingExternalEvent,
                            output_hash: None,
                            duration_ms: 0,
                            capabilities_used: vec![],
                            error: None,
                            attempt_count: 1,
                        };
                        step_results.insert(step_id.clone(), cp);
                        workflow_status = WorkflowStatus::Paused;
                        eprintln!(
                            "Awaiting external event for step '{}'. \
                             Run: boruna workflow trigger {} {} \
                             --token {} --payload '<json>'",
                            step_id, run_id, step_id, token
                        );
                        s.upsert_step_checkpoint(&StepCheckpoint {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                            status: PersistStepStatus::AwaitingExternalEvent,
                            output_json: None,
                            output_hash: None,
                            started_at_ms: Some(step_started_at_ms),
                            ended_at_ms: None,
                            error_msg: None,
                            attempt_count: 1,
                            worker_id: None,
                            lease_expires_at_ms: None,
                            claim_id: 0,
                            output_blob_ref: None,
                        })
                        .map_err(WorkflowRunError::from)?;
                        break;
                    }
                }
                StepKind::Source { source } => {
                    let result = Self::execute_source_step(
                        step_id,
                        source,
                        step_def,
                        &options.workflow_dir,
                        &options.policy,
                        data_store,
                        options.live,
                    );
                    let duration_ms = step_start.elapsed().as_millis() as u64;

                    // Retry semantics live inside `execute_source_step`
                    // → `compile_and_run_step_with_retry`. As of
                    // sprint 0.3-S13, the failure path also carries
                    // the actual attempt count so the persisted
                    // checkpoint reflects retry exhaustion accurately
                    // (was: defaulted to 1 in the failure path).
                    let outcome: Result<StepResult, (WorkflowRunError, u32)> = match result {
                        Ok(sr) => Ok(StepResult { duration_ms, ..sr }),
                        Err((e, attempts)) => Err((
                            WorkflowRunError::StepFailed(step_id.clone(), e.to_string()),
                            attempts,
                        )),
                    };

                    match outcome {
                        Ok(sr) => {
                            #[cfg(feature = "persist-sqlite")]
                            if let Some(s) = store {
                                let raw_output_json =
                                    Self::lookup_output_json(data_store, step_id, "result")?;
                                let (routed_json, routed_blob_ref) = match raw_output_json {
                                    Some(j) => route_output(j, s.blob_store()),
                                    None => (None, None),
                                };
                                s.upsert_step_checkpoint(&StepCheckpoint {
                                    run_id: run_id.to_string(),
                                    step_id: step_id.clone(),
                                    status: PersistStepStatus::Completed,
                                    output_json: routed_json,
                                    output_hash: sr.output_hash.clone(),
                                    started_at_ms: None, // COALESCE preserves
                                    ended_at_ms: Some(now_unix_ms()),
                                    error_msg: None,
                                    // 0.3-S11: persist the actual
                                    // attempt count from the StepResult
                                    // (set by compile_and_run_step_with_retry).
                                    attempt_count: sr.attempt_count,
                                    worker_id: None,
                                    lease_expires_at_ms: None,
                                    claim_id: 0,
                                    output_blob_ref: routed_blob_ref,
                                })
                                .map_err(WorkflowRunError::from)?;
                                emit_step_terminal_audit(
                                    s,
                                    run_id,
                                    step_id,
                                    StepStatus::Completed,
                                    sr.output_hash.as_deref(),
                                    None,
                                    sr.duration_ms,
                                );
                            }
                            step_results.insert(step_id.clone(), sr);
                        }
                        Err((e, attempt_count)) => {
                            let err_msg = e.to_string();
                            step_results.insert(
                                step_id.clone(),
                                StepResult {
                                    step_id: step_id.clone(),
                                    status: StepStatus::Failed,
                                    output_hash: None,
                                    duration_ms,
                                    capabilities_used: vec![],
                                    error: Some(err_msg.clone()),
                                    attempt_count,
                                },
                            );
                            workflow_status = WorkflowStatus::Failed;

                            #[cfg(feature = "persist-sqlite")]
                            if let Some(s) = store {
                                s.upsert_step_checkpoint(&StepCheckpoint {
                                    run_id: run_id.to_string(),
                                    step_id: step_id.clone(),
                                    status: PersistStepStatus::Failed,
                                    output_json: None,
                                    output_hash: None,
                                    started_at_ms: None,
                                    ended_at_ms: Some(now_unix_ms()),
                                    error_msg: Some(err_msg.clone()),
                                    attempt_count,
                                    worker_id: None,
                                    lease_expires_at_ms: None,
                                    claim_id: 0,
                                    output_blob_ref: None,
                                })
                                .map_err(WorkflowRunError::from)?;
                                emit_step_terminal_audit(
                                    s,
                                    run_id,
                                    step_id,
                                    StepStatus::Failed,
                                    None,
                                    Some(&err_msg),
                                    duration_ms,
                                );
                            }
                            break;
                        }
                    }
                }
            }
        }

        if workflow_status == WorkflowStatus::Running {
            workflow_status = WorkflowStatus::Completed;
        }

        Ok(WorkflowRunResult {
            run_id: run_id.to_string(),
            workflow_name: def.name.clone(),
            status: workflow_status,
            step_results,
            total_duration_ms: run_start.elapsed().as_millis() as u64,
        })
    }

    /// Read the persisted JSON for a step's output from the in-memory data
    /// store. Returns `Ok(None)` if the step did not produce a "result"
    /// output. This is the bridge from VM-Value-shape outputs (held in
    /// memory by `DataStore`) to the JSON column in `step_checkpoints`.
    #[cfg(feature = "persist-sqlite")]
    fn lookup_output_json(
        data_store: &DataStore,
        step_id: &str,
        output_name: &str,
    ) -> Result<Option<String>, WorkflowRunError> {
        let key = format!("{step_id}.{output_name}");
        match data_store.resolve_input(&key) {
            Ok(value) => serde_json::to_string(&value)
                .map(Some)
                .map_err(|e| WorkflowRunError::Internal(format!("output serialize: {e}"))),
            Err(_) => Ok(None),
        }
    }

    fn execute_source_step(
        step_id: &str,
        source: &str,
        step_def: &StepDef,
        workflow_dir: &str,
        policy: &Option<Policy>,
        data_store: &mut DataStore,
        live: bool,
    ) -> Result<StepResult, (WorkflowRunError, u32)> {
        // 0.3-S14: resolve inputs ONCE up front, then pass the
        // resolved map to the compute path. The .ax step's
        // `step_input("name")` calls dispatch through the gateway's
        // StepInputHandler which serves from this map. Input-
        // resolution failures count as a single attempt (we can't
        // retry a missing upstream output).
        let resolved_inputs = data_store
            .resolve_step_inputs(&step_def.inputs)
            .map_err(|e| (WorkflowRunError::StepFailed(step_id.to_string(), e), 1))?;

        // Compute path is wrapped in retry. On retry success, the
        // returned Value is stored once (idempotent for the on-disk
        // file, since `store_output` overwrites atomically per 0.3-S3).
        // 0.3-S11: helper returns (Value, attempts) on success and
        // (err, attempts) on failure.
        // 0.3-S13: surface the attempt count on the failure path too
        // so the sequential terminal-failure upsert can persist the
        // accurate count instead of defaulting to 1.
        let (value, attempt_count) = Self::compile_and_run_step_with_retry(
            step_id,
            source,
            step_def,
            workflow_dir,
            policy,
            live,
            resolved_inputs,
        )?;

        let output_hash = DataStore::hash_value(&value);
        data_store
            .store_output(step_id, "result", &value)
            .map_err(|e| {
                (
                    WorkflowRunError::StepFailed(step_id.to_string(), e.to_string()),
                    attempt_count,
                )
            })?;

        Ok(StepResult {
            step_id: step_id.to_string(),
            status: StepStatus::Completed,
            output_hash: Some(output_hash),
            duration_ms: 0, // filled in by caller
            capabilities_used: step_def.capabilities.clone(),
            error: None,
            attempt_count,
        })
    }

    /// Wrap [`Self::compile_and_run_step`] in the step's `RetryPolicy`.
    /// Implements the contract documented in
    /// [`retry_with_backoff`]:
    ///
    /// - `step_def.retry == None` OR `on_transient == false` OR
    ///   `max_attempts <= 1` → single attempt, no retry.
    /// - Otherwise: up to `max_attempts` total attempts with
    ///   exponential backoff between (100ms * 2^N capped at 5s).
    /// - On final exhaustion, the returned `WorkflowRunError` is the
    ///   last attempt's error wrapped to include the attempt count
    ///   in the user-facing message.
    ///
    /// Shared between sequential `execute_source_step` and the
    /// concurrent worker closure inside [`Self::execute_steps_concurrent`].
    /// Introduced in `0.3-S5` (closes the prior "retry once
    /// regardless of max_attempts" primitive).
    fn compile_and_run_step_with_retry(
        step_id: &str,
        source: &str,
        step_def: &StepDef,
        workflow_dir: &str,
        policy: &Option<Policy>,
        live: bool,
        resolved_inputs: BTreeMap<String, boruna_bytecode::Value>,
    ) -> Result<(boruna_bytecode::Value, u32), (WorkflowRunError, u32)> {
        retry_with_backoff(step_def.retry.as_ref(), step_id, |_attempt| {
            // Each retry attempt gets its own clone of the inputs
            // (the underlying compile+run path takes ownership).
            // Inputs are bounded — typically a handful of small
            // strings — so the clone cost is negligible.
            Self::compile_and_run_step(
                step_id,
                source,
                step_def,
                workflow_dir,
                policy,
                live,
                resolved_inputs.clone(),
            )
        })
    }

    /// Compile + run a single step's `.ax` source and return the
    /// resulting `Value`. Pure compute path: no DataStore mutation, no
    /// SQLite writes. Shared between the sequential `execute_source_step`
    /// (which then stores the output) and the concurrent worker thread
    /// (which returns the Value to the coordinator for storage).
    ///
    /// Extracted in `0.3-S4` so worker threads can call into the same
    /// compile+run path without holding any shared mutable state.
    ///
    /// Sprint `0.4-S8`: the error type is now
    /// `(WorkflowRunError, &'static str)` where the second element is
    /// the [`error_class`] string. The retry loop consults the class
    /// to decide whether to retry per the policy's `retry_on`
    /// allowlist.
    fn compile_and_run_step(
        step_id: &str,
        source: &str,
        step_def: &StepDef,
        workflow_dir: &str,
        policy: &Option<Policy>,
        live: bool,
        resolved_inputs: BTreeMap<String, boruna_bytecode::Value>,
    ) -> Result<boruna_bytecode::Value, (WorkflowRunError, &'static str)> {
        let source_path = Path::new(workflow_dir).join(source);
        let source_code = std::fs::read_to_string(&source_path).map_err(|e| {
            (
                WorkflowRunError::StepFailed(
                    step_id.to_string(),
                    format!("cannot read {}: {e}", source_path.display()),
                ),
                error_class::IO_ERROR,
            )
        })?;

        let module = boruna_compiler::compile(step_id, &source_code).map_err(|e| {
            (
                WorkflowRunError::StepFailed(step_id.to_string(), format!("compile error: {e}")),
                error_class::COMPILE_ERROR,
            )
        })?;

        let step_policy = Self::build_step_policy(policy, step_def);

        // Each call builds its own gateway. In the concurrent path,
        // workers each construct their own gateway/VM — no shared
        // gateway state.
        //
        // 0.3-S14: wrap the chosen handler in `StepInputHandler` so
        // `step_input("name")` calls in the .ax source dispatch to
        // the runner-resolved upstream outputs. The wrapper
        // delegates non-StepInput calls to the inner handler, so
        // this composes with both the mock handler and (under
        // `--live`) the real HTTP handler.
        let inner_handler: Box<dyn boruna_vm::capability_gateway::CapabilityHandler> = if live {
            #[cfg(feature = "http")]
            {
                let net_policy = step_policy.net_policy.clone().unwrap_or_default();
                Box::new(boruna_vm::http_handler::HttpHandler::new(net_policy))
            }
            #[cfg(not(feature = "http"))]
            {
                eprintln!(
                    "warning: --live requires the `http` feature; falling back to mock handler"
                );
                Box::new(boruna_vm::capability_gateway::MockHandler)
            }
        } else {
            Box::new(boruna_vm::capability_gateway::MockHandler)
        };
        let handler = Box::new(boruna_vm::capability_gateway::StepInputHandler::new(
            resolved_inputs,
            inner_handler,
        ));
        let gateway = CapabilityGateway::with_handler(step_policy, handler);
        let mut vm = Vm::new(module, gateway);
        vm.run().map_err(|e| {
            let class = classify_vm_error(&e);
            (
                WorkflowRunError::StepFailed(step_id.to_string(), format!("runtime error: {e}")),
                class,
            )
        })
    }

    fn build_step_policy(base_policy: &Option<Policy>, step_def: &StepDef) -> Policy {
        match base_policy {
            Some(p) => {
                let mut policy = p.clone();
                if let Some(budget) = &step_def.budget {
                    if let Some(max_calls) = budget.max_calls {
                        for cap in &step_def.capabilities {
                            policy.rules.entry(cap.clone()).or_insert(PolicyRule {
                                allow: true,
                                budget: max_calls,
                            });
                        }
                    }
                }
                // 0.3-S14: `step.input` is structurally a
                // workflow-internal capability — it reads data the
                // workflow itself produced, not an external side
                // effect. Auto-allow when the operator hasn't
                // expressed a contrary policy, so steps don't need
                // to redundantly declare it in
                // `workflow.json::capabilities`.
                //
                // `entry().or_insert()` PRESERVES an operator's
                // explicit `step.input` rule (allow=false to deny,
                // budget=N to cap). The auto-allow only fires when
                // the policy is silent on `step.input`. Operators
                // who want to deny step.input on a hardened
                // workflow (e.g. an air-gapped batch) can do so via
                // the policy file. Reviewed 0.3-S14.
                policy
                    .rules
                    .entry("step.input".to_string())
                    .or_insert(PolicyRule {
                        allow: true,
                        budget: u64::MAX,
                    });
                policy
            }
            None => Policy::deny_all(),
        }
    }
}

/// Documented taxonomy of step-failure classes (sprint `0.4-S8`).
/// Operators reference these strings in [`RetryPolicy::retry_on`] to
/// allowlist which failures should retry. The taxonomy is **forward-
/// compatible**: strings are stable; new classes can be added without
/// breaking existing policies; unknown strings in `retry_on` are
/// silently ignored (conservative-by-default — typo means "do not
/// retry," never a panic).
///
/// All values are snake_case, matching the surrounding error_kind
/// convention in `boruna_run`'s MCP responses.
pub mod error_class {
    /// VM hit `set_max_wall_ms` (operational, wall-clock-keyed).
    /// Recommended for retry: yes — typically transient.
    pub const WALL_TIME_EXCEEDED: &str = "wall_time_exceeded";
    /// VM hit `set_max_steps` (deterministic ceiling).
    /// Recommended for retry: no — same input → same step count.
    pub const STEP_LIMIT_EXCEEDED: &str = "step_limit_exceeded";
    /// Capability denied by policy (`Op::CapCall` against a denied
    /// capability or one outside the policy allowlist).
    /// Recommended for retry: no — policy is deterministic.
    pub const CAPABILITY_DENIED: &str = "capability_denied";
    /// Per-capability budget (`PolicyRule::budget`) exhausted.
    /// Recommended for retry: no — budget is per-run.
    pub const CAPABILITY_BUDGET_EXCEEDED: &str = "capability_budget_exceeded";
    /// Compilation failure (`boruna_compiler::compile`).
    /// Recommended for retry: no — source is deterministic.
    pub const COMPILE_ERROR: &str = "compile_error";
    /// Runtime VM error not covered by another class — assertions,
    /// type errors, list-index OOB, division by zero, match
    /// exhaustion, stack errors, bytecode errors.
    /// Recommended for retry: no — usually deterministic.
    pub const RUNTIME_ERROR: &str = "runtime_error";
    /// IO error reading the step's source file.
    /// Recommended for retry: maybe — transient FS issues do happen.
    pub const IO_ERROR: &str = "io_error";
    /// Step input resolution failed (e.g., upstream output missing).
    /// Recommended for retry: no — DAG-level concern, not transient.
    pub const INPUT_RESOLUTION: &str = "input_resolution";
    /// Network-level transient failure from a `net.fetch` capability
    /// call: timeout, connection refused, DNS resolution failure,
    /// connection reset mid-stream, etc. Detected by string-matching
    /// the `ureq` error wrapper emitted by `http_handler`. SSRF
    /// blocks and policy-allowlist denials are *not* this class —
    /// those are deterministic configuration errors and surface as
    /// `RUNTIME_ERROR` (not retry-eligible by default).
    /// Recommended for retry: yes — typically transient.
    pub const TRANSIENT_NETWORK: &str = "transient_network";
}

/// Classify a [`VmError`] into one of the strings in [`error_class`]
/// (sprint `0.4-S8`). Used by the retry loop to decide whether the
/// step's failure matches the operator's `retry_on` allowlist.
fn classify_vm_error(e: &VmError) -> &'static str {
    match e {
        VmError::WallTimeExceeded(_) => error_class::WALL_TIME_EXCEEDED,
        VmError::ExecutionLimitExceeded(_) => error_class::STEP_LIMIT_EXCEEDED,
        VmError::CapabilityDenied(_) => error_class::CAPABILITY_DENIED,
        VmError::CapabilityBudgetExceeded(_) => error_class::CAPABILITY_BUDGET_EXCEEDED,
        // Capability errors surface as AssertionFailed wrapping the
        // handler's `Err(String)` (see capability_gateway::invoke).
        // Distinguish transient network failures (retry-eligible) from
        // deterministic policy/config errors (not retry-eligible) by
        // matching the http_handler's error wrappers.
        VmError::AssertionFailed(msg) if is_transient_network_error(msg) => {
            error_class::TRANSIENT_NETWORK
        }
        // All other VmError variants — including assertion failures,
        // type errors, index-out-of-bounds, division by zero, match
        // exhaustion, stack errors, invalid IP / function / constant /
        // local / global, unknown capability, actor-related errors
        // (ActorNotFound, MailboxEmpty, Deadlock, MaxRoundsExceeded),
        // Halt, BudgetExhausted, Bytecode — surface as RUNTIME_ERROR.
        // Operators wanting per-class retry on assertions vs deadlocks
        // would need a finer taxonomy; deferred until a real use case
        // demands it.
        _ => error_class::RUNTIME_ERROR,
    }
}

/// Detect transient-network markers in an error message string.
/// Used by both [`classify_vm_error`] (in-process retry path) and
/// [`classify_failure_message`] (distributed wait-driver path) so
/// the two paths agree on what counts as TRANSIENT_NETWORK.
///
/// Matches the wrappers emitted by `crates/llmvm/src/http_handler.rs`:
/// `"HTTP request failed: ..."` and
/// `"failed to read response body: ..."`. Both indicate the network
/// call reached the wire but the transport / peer / DNS broke.
/// SSRF blocks (`"blocked request to ..."`) and allowlist denials
/// (`"domain '..' not in allowlist"`) are intentionally NOT matched —
/// they're deterministic config errors, not transient.
fn is_transient_network_error(msg: &str) -> bool {
    msg.contains("HTTP request failed:") || msg.contains("failed to read response body:")
}

/// Classify a wire-level `error_msg` string into one of the
/// [`error_class`] strings (sprint `0.5-S5`). Used by
/// [`WorkflowRunner::advance_run_one_tick`] to decide whether a
/// distributed-mode step failure is retry-eligible per its
/// [`RetryPolicy`].
///
/// **Why a separate classifier from [`classify_vm_error`]:** the
/// in-process retry path holds the live [`VmError`] and classifies
/// it directly. The wait driver only sees the persisted `error_msg`
/// string — the worker stripped the typed enum and emitted a
/// human-readable prefix (`"compile: …"`, `"runtime: …"`,
/// `"policy parse: …"`, `"report_complete rejected …"`). Mapping
/// those prefixes back to error_class strings is best-effort. An
/// unknown prefix falls back to [`error_class::RUNTIME_ERROR`] —
/// the most-conservative default given the existing taxonomy
/// (RUNTIME_ERROR is "not recommended for retry" per its docs, so a
/// retry_on=[] + on_transient=true policy still retries it; an
/// allowlisted policy will only retry it if RUNTIME_ERROR is
/// explicitly listed).
///
/// **Operational only.** The classifier doesn't need to be perfect;
/// it just needs to be consistent with what the worker produces.
/// Convention §15: attempt_count and retry decisions don't feed any
/// audit hash.
pub(crate) fn classify_failure_message(error_msg: &str) -> &'static str {
    let trimmed = error_msg.trim_start();
    if trimmed.starts_with("compile:") || trimmed.starts_with("compile error") {
        error_class::COMPILE_ERROR
    } else if trimmed.starts_with("policy parse") {
        // Policy parse failures are deterministic — same JSON in,
        // same parse error out. Treat as RUNTIME_ERROR so a
        // permissive retry_on=[]+on_transient=true policy retries
        // it (operator may have updated the policy in the
        // meantime), but an allowlisted policy won't unless
        // explicitly listed.
        error_class::RUNTIME_ERROR
    } else if trimmed.starts_with("runtime:") {
        // Worker-emitted "runtime: <VmError display>". Most variants
        // collapse to RUNTIME_ERROR (we can't recover wall-time vs
        // capability-denied vs assertion from a free-form string),
        // but transient-network failures carry a distinctive
        // wrapper from http_handler that survives Display, so we
        // can recover that one class. A future sprint adds a typed
        // error_kind field on the FailRequest so the wait driver
        // sees the original class for all variants.
        if is_transient_network_error(trimmed) {
            error_class::TRANSIENT_NETWORK
        } else {
            error_class::RUNTIME_ERROR
        }
    } else if trimmed.contains("cannot read") {
        error_class::IO_ERROR
    } else {
        error_class::RUNTIME_ERROR
    }
}

/// Decide whether a step failure with the given class should retry
/// per the supplied policy (sprint `0.4-S8`).
///
/// Resolution order:
/// 1. `policy = None` or `max_attempts <= 1` → no retry.
/// 2. `policy.retry_on` non-empty → retry IFF `class` is in the list.
/// 3. `policy.retry_on` empty → fall back to legacy `on_transient`
///    gate.
fn should_retry_class(policy: Option<&RetryPolicy>, class: &str) -> bool {
    let p = match policy {
        Some(p) => p,
        None => return false,
    };
    if p.max_attempts <= 1 {
        return false;
    }
    if !p.retry_on.is_empty() {
        return p.retry_on.iter().any(|c| c == class);
    }
    p.on_transient
}

/// Base for the exponential-backoff schedule: 100ms × 2^N. Pulled out
/// as a constant so tests can verify the formula without sleeping.
const RETRY_BASE_BACKOFF_MS: u64 = 100;
/// Cap for the backoff schedule. Without a cap, attempt 7 would sleep
/// 12.8s and attempt 10 would sleep 102.4s — way past operator
/// tolerance. 5s gives ~6 retries before saturating.
const RETRY_MAX_BACKOFF_MS: u64 = 5_000;

/// Pre-attempt sleep duration in ms for `attempt` (0-indexed: the
/// sleep that happens BEFORE attempt N+1). Defined as a free function
/// so tests can verify the curve without invoking the closure.
pub(crate) fn retry_backoff_ms(prev_attempt: u32) -> u64 {
    RETRY_BASE_BACKOFF_MS
        .saturating_mul(2u64.saturating_pow(prev_attempt))
        .min(RETRY_MAX_BACKOFF_MS)
}

/// Run `attempt_fn` up to `policy.max_attempts` times with exponential
/// backoff. Returns the first success, or the final attempt's `Err`
/// (wrapped to include the attempt count) on exhaustion.
///
/// # Retry eligibility (sprint `0.4-S8`)
///
/// `policy = None` OR `policy.max_attempts <= 1` → single attempt.
///
/// Otherwise, after each per-attempt failure, [`should_retry_class`]
/// decides whether to loop again:
/// - `policy.retry_on` non-empty → retry IFF the failure's class is
///   in the allowlist.
/// - `policy.retry_on` empty → fall back to `policy.on_transient`
///   (legacy gate).
///
/// A failure with a non-retry-eligible class short-circuits the loop
/// at the first attempt — no exponential backoff for "compile error
/// will not improve on retry."
///
/// # Backoff
///
/// Pre-attempt sleeps: 100ms before attempt 2, 200ms before attempt 3,
/// 400ms before attempt 4, ... capped at 5s. See [`retry_backoff_ms`].
///
/// # Determinism
///
/// Backoff is wall-clock-keyed (project-conventions §17). A successful
/// retry's `output_hash` is bit-identical to a successful first
/// attempt — the determinism contract holds. The number of attempts
/// and per-attempt durations are operational only.
///
/// # Test ergonomics
///
/// Sleeps are skipped entirely under `cfg(test)` so the unit suite
/// runs fast. Real backoff is exercised by integration tests on
/// demand.
pub(crate) fn retry_with_backoff<T, F>(
    policy: Option<&RetryPolicy>,
    step_id: &str,
    mut attempt_fn: F,
) -> Result<(T, u32), (WorkflowRunError, u32)>
where
    F: FnMut(u32) -> Result<T, (WorkflowRunError, &'static str)>,
{
    // 0.4-S8: max_attempts is always honored as the upper bound; the
    // per-attempt should_retry_class check decides whether to actually
    // loop. Legacy `on_transient = false` semantics ("single attempt,
    // no retry") falls out naturally because should_retry_class
    // returns false for that policy shape.
    let max_attempts = policy.map(|p| p.max_attempts.max(1)).unwrap_or(1);
    let mut last_err: Option<WorkflowRunError> = None;
    let mut attempts_used: u32 = 0;
    for attempt in 1..=max_attempts {
        attempts_used = attempt;
        if attempt > 1 {
            let prev_attempt = attempt - 2; // attempt is 1-indexed
            let sleep_ms = retry_backoff_ms(prev_attempt);
            // Skip real sleeps under cfg(test) so the unit suite is
            // fast. The backoff curve is independently tested via
            // `retry_backoff_ms`; real wall-clock backoff is locked
            // by an integration test in `orchestrator/tests/` where
            // cfg(test) is NOT set on the orchestrator lib build.
            #[cfg(not(test))]
            std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
            #[cfg(test)]
            let _ = sleep_ms;
            // Operator-facing retry log. Gated under `cfg(not(test))`
            // so the unit test suite stays silent — embedders capturing
            // stderr (e.g. the MCP server speaking JSON-RPC over
            // stdio) shouldn't see test-suite noise either way, but
            // production embedders DO want this log line so they can
            // see retries happening. Reviewed 0.3-S5 (finding #1):
            // prior unconditional eprintln polluted unit-test output.
            #[cfg(not(test))]
            eprintln!(
                "step '{step_id}' attempt {attempt}/{max_attempts} \
                 (retrying after {sleep_ms}ms backoff)"
            );
            #[cfg(test)]
            let _ = (step_id, attempt, max_attempts);
        }
        match attempt_fn(attempt) {
            // Sprint 0.3-S11: surface the actual attempt count
            // alongside the value so the caller can persist it in
            // the step's checkpoint row. `attempt` is 1-indexed
            // (1 = first try succeeded; >1 = retry succeeded).
            Ok(value) => return Ok((value, attempt)),
            Err((e, class)) => {
                last_err = Some(e);
                // 0.4-S8: short-circuit if this failure's class is
                // not in the operator's `retry_on` allowlist (or, in
                // legacy mode, if `on_transient` is false). Skipping
                // the backoff sleep on a non-retry-eligible failure
                // is the whole point — operators set narrower retry
                // policies precisely to avoid wasting time on
                // deterministic failures (compile errors, runtime
                // errors with bad inputs, etc.).
                if !should_retry_class(policy, class) {
                    break;
                }
            }
        }
    }
    // Exhausted (or short-circuited). Wrap the last error to include
    // the attempt count when more than one attempt actually ran; for
    // single-attempt paths preserve the original error shape so
    // existing operator scripts that match on error strings don't
    // break.
    let final_err = last_err.expect("loop runs at least once");
    if attempts_used > 1 {
        Err((
            WorkflowRunError::StepFailed(
                step_id.to_string(),
                format!("failed after {attempts_used} attempts: {final_err}"),
            ),
            attempts_used,
        ))
    } else {
        Err((final_err, 1))
    }
}

#[cfg(feature = "persist-sqlite")]
fn open_store(data_dir: &Path) -> Result<RunCheckpointStore, WorkflowRunError> {
    if data_dir.as_os_str().is_empty() {
        return Err(WorkflowRunError::Internal(
            "--data-dir must be a non-empty path".to_string(),
        ));
    }
    if data_dir == Path::new("/") {
        return Err(WorkflowRunError::Internal(
            "--data-dir must not be the system root '/'".to_string(),
        ));
    }
    std::fs::create_dir_all(data_dir).map_err(|e| {
        WorkflowRunError::Io(format!(
            "cannot create data dir '{}': {e}",
            data_dir.display()
        ))
    })?;
    let db_path: PathBuf = data_dir.join("runs.db");
    RunCheckpointStore::open(&db_path).map_err(WorkflowRunError::from)
}

#[cfg(feature = "persist-sqlite")]
fn persist_status_from_workflow(s: &WorkflowStatus) -> PersistRunStatus {
    match s {
        WorkflowStatus::Running => PersistRunStatus::Running,
        WorkflowStatus::Paused => PersistRunStatus::Paused,
        WorkflowStatus::Completed => PersistRunStatus::Completed,
        WorkflowStatus::Failed => PersistRunStatus::Failed,
    }
}

#[cfg(feature = "persist-sqlite")]
fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Generate a fresh 16-byte trigger token from `/dev/urandom`, hex-
/// encoded (sprint 0.3-S15). The token is the operator-facing security
/// boundary that binds a `boruna workflow trigger` invocation to a
/// specific pause instance.
///
/// **Why `/dev/urandom` and not the `rand` crate?** Avoids a dep for
/// one helper. Posix-only by design — the persistence feature already
/// requires file-backed SQLite, so the orchestrator never runs where
/// `/dev/urandom` is absent.
///
/// **No fallback.** If `/dev/urandom` cannot be read, the function
/// returns `Err`. Reviewed in 0.3-S15 — a prior version degraded to a
/// `SystemTime + pid + counter` hash, which gave low-entropy,
/// observer-predictable tokens silently. The trigger token IS the
/// security boundary, so entropy failure is not a graceful-degradation
/// concern. A `/dev/urandom` failure on a real Unix system signals a
/// fundamentally misconfigured environment (chroot/cgroup denying the
/// device, or filesystem corruption); the trigger flow refusing to
/// pause loudly is the right response.
#[cfg(feature = "persist-sqlite")]
fn generate_trigger_token() -> Result<String, WorkflowRunError> {
    use std::io::Read;
    let mut buf = [0u8; 16];
    let mut f = std::fs::File::open("/dev/urandom").map_err(|e| {
        WorkflowRunError::Io(format!(
            "trigger token entropy unavailable: cannot open /dev/urandom: {e}"
        ))
    })?;
    f.read_exact(&mut buf).map_err(|e| {
        WorkflowRunError::Io(format!(
            "trigger token entropy unavailable: short read from /dev/urandom: {e}"
        ))
    })?;
    Ok(hex_lower(&buf))
}

#[cfg(feature = "persist-sqlite")]
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Acquire the trigger token for a paused step at pause-time (sprint
/// 0.3-S15). On first entry, generates a fresh token and persists a
/// placeholder `TriggerRecord` (with empty `payload`) via CAS. On
/// re-entry (resume after crash before the operator triggered), reads
/// and returns the existing token — does NOT rotate it, otherwise the
/// token printed in the original pause's stderr would silently stop
/// validating.
///
/// Returns the **persisted** token, which the caller MUST use in the
/// "Run: boruna workflow trigger ... --token <X>" message it prints to
/// the operator. Reviewed in 0.3-S15 — a prior version separated
/// generate-then-persist, which printed the freshly-generated token
/// while persist's "leave existing" branch kept the original. Operators
/// would copy the just-printed token from the resume's stderr and get
/// `InvalidTriggerToken`.
///
/// **Concurrency:** at pause-time the runner is the only writer for
/// THIS step's pause flow, but a contemporaneous `boruna workflow
/// approve` against another step in the same run could race the read.
/// CAS retries handle that — same pattern as
/// [`record_approval_decision`].
#[cfg(feature = "persist-sqlite")]
fn acquire_trigger_token(
    store: &RunCheckpointStore,
    run_id: &str,
    step_id: &str,
) -> Result<String, WorkflowRunError> {
    const CAS_RETRY_BUDGET: usize = 5;
    for _ in 0..CAS_RETRY_BUDGET {
        let metadata_json = store
            .get_run_metadata(run_id)
            .map_err(WorkflowRunError::from)?
            .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;
        let mut metadata: PersistedRunMetadata =
            serde_json::from_str(&metadata_json).map_err(|e| {
                WorkflowRunError::Internal(format!("corrupt metadata_json for run '{run_id}': {e}"))
            })?;
        if let Some(existing) = metadata.triggers.get(step_id) {
            // Re-entry path: pause was previously taken and the token
            // is already persisted. Return it verbatim. We don't even
            // CAS — there's nothing to write.
            return Ok(existing.token.clone());
        }
        // First entry: mint a fresh token and persist as a placeholder.
        let token = generate_trigger_token()?;
        metadata.triggers.insert(
            step_id.to_string(),
            TriggerRecord {
                token: token.clone(),
                payload: String::new(),
                triggered_at_ms: 0,
            },
        );
        let updated_metadata = serde_json::to_string(&metadata)
            .map_err(|e| WorkflowRunError::Internal(format!("metadata serialize: {e}")))?;
        let swapped = store
            .compare_and_swap_metadata(run_id, &metadata_json, &updated_metadata, now_unix_ms())
            .map_err(WorkflowRunError::from)?;
        if swapped {
            return Ok(token);
        }
        // CAS lost — another writer touched metadata. Re-read; the
        // re-read may now reveal an existing token (if the other
        // writer was a concurrent pause), in which case we'll return
        // that one rather than minting a third.
    }
    Err(WorkflowRunError::Internal(format!(
        "CAS retry budget exhausted acquiring trigger token for step '{step_id}' in run '{run_id}'"
    )))
}

/// Append a single audit event to the run's hash-chained
/// `metadata.audit_log` (sprint `0.4-S11`). Uses the same CAS-retry
/// pattern as [`record_approval_decision`] / [`record_external_trigger`]
/// — concurrent operator decisions and runner-side lifecycle appends
/// converge after at most one CAS retry.
///
/// **Persistent path only.** Ephemeral runs (`WorkflowRunner::run`)
/// have no store, so this is never called from that path.
///
/// **Best-effort failure mode.** If the CAS budget exhausts (heavily
/// contended runs with N concurrent operator decisions racing the
/// runner), the append returns `Internal`. Callers in the lifecycle
/// path SHOULD log + continue rather than failing the run — a missed
/// audit event is operationally annoying but not a correctness
/// failure. The existing chain entries remain valid; the gap is
/// detectable by an auditor as "fewer step events than checkpoints"
/// at verify time.
#[cfg(feature = "persist-sqlite")]
fn append_audit_event(
    store: &RunCheckpointStore,
    run_id: &str,
    event: crate::audit::AuditEvent,
) -> Result<(), WorkflowRunError> {
    const CAS_RETRY_BUDGET: usize = 5;
    for _ in 0..CAS_RETRY_BUDGET {
        let metadata_json = store
            .get_run_metadata(run_id)
            .map_err(WorkflowRunError::from)?
            .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;
        let mut metadata: PersistedRunMetadata =
            serde_json::from_str(&metadata_json).map_err(|e| {
                WorkflowRunError::Internal(format!("corrupt metadata_json for run '{run_id}': {e}"))
            })?;
        let mut audit = crate::audit::AuditLog::from_entries(metadata.audit_log);
        audit.append(event.clone());
        metadata.audit_log = audit.into_entries();
        let updated_metadata = serde_json::to_string(&metadata)
            .map_err(|e| WorkflowRunError::Internal(format!("metadata serialize: {e}")))?;
        let swapped = store
            .compare_and_swap_metadata(run_id, &metadata_json, &updated_metadata, now_unix_ms())
            .map_err(WorkflowRunError::from)?;
        if swapped {
            return Ok(());
        }
        // CAS lost — concurrent writer touched metadata. Re-read and
        // rebuild the chain on top of whatever they committed; our
        // event will be appended last.
    }
    Err(WorkflowRunError::Internal(format!(
        "CAS retry budget exhausted appending audit event to run '{run_id}'"
    )))
}

/// Compute a SHA-256 hash of a string for audit-event fields like
/// `policy_hash` (sprint `0.4-S11`). Hex-encoded, lowercase.
#[cfg(feature = "persist-sqlite")]
fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Route a step output to inline storage or the blob store based on size.
///
/// Sprint 0.5-S7. Returns `(output_json, output_blob_ref)`. Exactly one of
/// the two will be `Some`; the other will be `None` — satisfying the
/// mutual-exclusion invariant enforced by `upsert_step_checkpoint`.
///
/// When `blob_store` is `None` or the payload is at or below
/// [`crate::persistence::BLOB_THRESHOLD`], the output stays inline.
/// On blob-write failure the function silently falls back to inline so no
/// output is ever lost.
#[cfg(feature = "persist-sqlite")]
fn route_output(
    output_json: String,
    blob_store: Option<&crate::persistence::BlobStore>,
) -> (Option<String>, Option<String>) {
    use crate::persistence::BLOB_THRESHOLD;
    match blob_store {
        Some(bs) if output_json.len() > BLOB_THRESHOLD => {
            let hash = sha256_hex(&output_json);
            match bs.write(&hash, output_json.as_bytes()) {
                Ok(()) => (None, Some(hash)),
                Err(_) => (Some(output_json), None),
            }
        }
        _ => (Some(output_json), None),
    }
}

/// Emit a step-terminal audit event (Completed or Failed) based on
/// the step result's status (sprint `0.4-S11`). Best-effort: a CAS
/// budget exhaustion logs and continues, never propagates.
///
/// Called by both the sequential `execute_steps` path and the
/// concurrent `execute_steps_concurrent` worker-result handler. The
/// helper centralizes the Completed/Failed mapping so the call sites
/// stay terse.
#[cfg(feature = "persist-sqlite")]
fn emit_step_terminal_audit(
    store: &RunCheckpointStore,
    run_id: &str,
    step_id: &str,
    status: StepStatus,
    output_hash: Option<&str>,
    error: Option<&str>,
    duration_ms: u64,
) {
    use crate::audit::AuditEvent;
    let event = match status {
        StepStatus::Completed => AuditEvent::StepCompleted {
            step_id: step_id.to_string(),
            // Synthesize an all-zeros hash for steps without an
            // output (approval gates that advance via sentinel
            // produce empty Map values, which still hash). Should
            // never be None for Completed in practice.
            output_hash: output_hash
                .map(|s| s.to_string())
                .unwrap_or_else(|| "0".repeat(64)),
            duration_ms,
        },
        StepStatus::Failed => AuditEvent::StepFailed {
            step_id: step_id.to_string(),
            error: error.unwrap_or("(no error message)").to_string(),
        },
        // Other statuses (Pending, Running, AwaitingApproval,
        // AwaitingExternalEvent, Skipped) are not terminal — no
        // audit event for them at this point. AwaitingApproval /
        // AwaitingExternalEvent get their own ApprovalGranted /
        // ExternalTriggerReceived events from the decision
        // entrypoints.
        _ => return,
    };
    if let Err(e) = append_audit_event(store, run_id, event) {
        eprintln!(
            "warning: failed to append step-terminal audit event for \
             step '{step_id}' in run '{run_id}': {e}"
        );
    }
}

/// Persist a single pause-step's checkpoint and (for triggers) its
/// token, printing the operator-facing pause message (sprint `0.4-S7`).
/// Returns the corresponding `StepStatus` on success so the caller can
/// build a `StepResult` for the in-memory result map.
///
/// Extracted from the wave-loop's per-pause body so the loop can call
/// it inside a per-pause `match` and isolate failures: a transient
/// error on pause #N must not strand pauses #1..N-1 in a half-committed
/// state, nor terminally-fail the run. See the call site comment for
/// the full rationale.
#[cfg(feature = "persist-sqlite")]
fn persist_one_pause(
    def: &WorkflowDef,
    store: &RunCheckpointStore,
    run_id: &str,
    pause_id: &str,
    now: i64,
) -> Result<StepStatus, WorkflowRunError> {
    let step_def = &def.steps[pause_id];
    let (persist_status, ax_status, message) = match &step_def.kind {
        StepKind::ApprovalGate { required_role, .. } => (
            PersistStepStatus::AwaitingApproval,
            StepStatus::AwaitingApproval,
            format!(
                "Awaiting approval for step '{pause_id}' (role: {required_role}). \
                 Run: boruna workflow approve {run_id} {pause_id}"
            ),
        ),
        StepKind::ExternalTrigger { .. } => {
            // 0.3-S15: acquire (or recover) the trigger token. On
            // first entry mints fresh + persists; on re-entry returns
            // the previously-persisted token unchanged so the printed
            // value always matches the value the trigger CLI
            // validates against.
            let token = acquire_trigger_token(store, run_id, pause_id)?;
            (
                PersistStepStatus::AwaitingExternalEvent,
                StepStatus::AwaitingExternalEvent,
                format!(
                    "Awaiting external event for step '{pause_id}'. \
                     Run: boruna workflow trigger {run_id} {pause_id} \
                     --token {token} --payload '<json>'"
                ),
            )
        }
        _ => unreachable!("persist_one_pause called with non-pause StepKind"),
    };
    store
        .upsert_step_checkpoint(&StepCheckpoint {
            run_id: run_id.to_string(),
            step_id: pause_id.to_string(),
            status: persist_status,
            output_json: None,
            output_hash: None,
            started_at_ms: Some(now),
            ended_at_ms: None,
            error_msg: None,
            attempt_count: 1,
            worker_id: None,
            lease_expires_at_ms: None,
            claim_id: 0,
            output_blob_ref: None,
        })
        .map_err(WorkflowRunError::from)?;
    eprintln!("{message}");
    Ok(ax_status)
}

/// Load a `WorkflowDef` for a run by inspecting its persisted metadata.
/// Sprint `0.5-S6`: prefers `metadata.workflow_def` (always populated for
/// remote-submit + submit-only runs) over the `<workflow_dir>/workflow.json`
/// disk path (the original pre-0.5-S2f source). Falling back to disk
/// keeps in-process runs working — they don't carry an embedded def.
///
/// The `run_id` is threaded through purely for error-message context.
#[cfg(feature = "persist-sqlite")]
fn load_def_from_metadata(
    metadata: &PersistedRunMetadata,
    run_id: &str,
) -> Result<WorkflowDef, WorkflowRunError> {
    if let Some(def) = &metadata.workflow_def {
        return Ok(def.clone());
    }
    if metadata.workflow_dir.is_empty() {
        return Err(WorkflowRunError::Internal(format!(
            "run '{run_id}' has neither embedded workflow_def nor a \
             workflow_dir in its metadata; this is a corrupt run record"
        )));
    }
    let def_path = Path::new(&metadata.workflow_dir).join("workflow.json");
    let def_json = std::fs::read_to_string(&def_path).map_err(|e| {
        WorkflowRunError::Io(format!(
            "cannot read {} (workflow_dir from run '{run_id}' metadata): {e}",
            def_path.display()
        ))
    })?;
    serde_json::from_str(&def_json)
        .map_err(|e| WorkflowRunError::Internal(format!("invalid workflow.json: {e}")))
}

/// Record an approval-gate decision for a paused workflow run.
///
/// Public entry for the `boruna workflow approve` / `reject` CLI handlers.
/// Validates the run + step state, mutates the run's `metadata.approvals`
/// blob, and writes back via a compare-and-swap update. Does **not**
/// advance the run — the operator must run `boruna workflow resume
/// <run-id>` to pick up past the gate.
///
/// ## Validation order (each surfaces a distinct typed error)
///
/// 1. Run must exist (`RunNotFound`)
/// 2. Run must not be in a terminal state (`RunNotResumable`)
/// 3. Workflow def must be loadable from the persisted metadata's
///    `workflow_dir` (or surface as `Internal("corrupt metadata_json")`)
/// 4. Step must exist in the workflow def (`StepNotFound`)
/// 5. Step must be a `StepKind::ApprovalGate` (`NotAnApprovalGateStep`)
/// 6. Step's persisted checkpoint must be `awaiting_approval`
///    (`StepNotAtApprovalGate { current_status }` if not)
/// 7. Step must not already have a decision in metadata.approvals
///    (`StepAlreadyDecided { prior_decision }`)
///
/// ## Concurrency
///
/// Read + validate + write run inside a compare-and-swap loop via
/// [`RunCheckpointStore::compare_and_swap_metadata`]: the function
/// reads the on-disk metadata, validates, then attempts to UPDATE the
/// row WHERE its `metadata_json` still equals the snapshot we read.
/// If a concurrent writer mutated the metadata in between, the UPDATE
/// matches 0 rows and this function loops, re-reading and re-validating
/// — where the re-read picks up the other writer's recorded decision and
/// surfaces a clean `StepAlreadyDecided` error. A bounded retry budget
/// prevents an infinite ping-pong (in practice the second iteration
/// always converges). Reviewed in 0.3-S2c (correctness #1 / integrity
/// H1: prior implementation's read+validate+write spanned 3 separate
/// SQL transactions and silently overwrote concurrent decisions).
#[cfg(feature = "persist-sqlite")]
pub fn record_approval_decision(
    data_dir: &Path,
    run_id: &str,
    step_id: &str,
    decision: ApprovalKind,
    reason: Option<String>,
) -> Result<(), WorkflowRunError> {
    let store = open_store(data_dir)?;
    record_approval_decision_in_store(&store, run_id, step_id, decision, reason)
}

/// Store-scoped variant of [`record_approval_decision`] for callers
/// that hold an existing `RunCheckpointStore` lock (sprint `0.5-S6`:
/// the coordinator's `POST /api/runs/{run_id}/approve` HTTP handler).
/// Behaviorally identical to [`record_approval_decision`]; the only
/// difference is the store source.
///
/// Loads the workflow def from `metadata.workflow_def` when present
/// (covers remote-submit + submit-only modes), falling back to
/// reading `<workflow_dir>/workflow.json` for in-process runs that
/// paused at an approval gate.
#[cfg(feature = "persist-sqlite")]
pub fn record_approval_decision_in_store(
    store: &RunCheckpointStore,
    run_id: &str,
    step_id: &str,
    decision: ApprovalKind,
    reason: Option<String>,
) -> Result<(), WorkflowRunError> {
    // Bounded CAS retry budget. Happy path is 1 iteration; second iteration
    // fires only on a race. After the second re-read, either we surface
    // StepAlreadyDecided or our CAS succeeds.
    const CAS_RETRY_BUDGET: usize = 5;
    for _ in 0..CAS_RETRY_BUDGET {
        let record = store
            .get_run_record(run_id)
            .map_err(WorkflowRunError::from)?
            .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;
        if let Some(terminal) = record.terminal_status {
            return Err(WorkflowRunError::RunNotResumable {
                run_id: run_id.to_string(),
                terminal_status: terminal.as_str().to_string(),
            });
        }

        let metadata_json = store
            .get_run_metadata(run_id)
            .map_err(WorkflowRunError::from)?
            .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;
        let mut metadata: PersistedRunMetadata =
            serde_json::from_str(&metadata_json).map_err(|e| {
                WorkflowRunError::Internal(format!("corrupt metadata_json for run '{run_id}': {e}"))
            })?;

        let def = load_def_from_metadata(&metadata, run_id)?;

        let step_def = def
            .steps
            .get(step_id)
            .ok_or_else(|| WorkflowRunError::StepNotFound {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
            })?;
        if !matches!(step_def.kind, StepKind::ApprovalGate { .. }) {
            return Err(WorkflowRunError::NotAnApprovalGateStep {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
            });
        }

        if let Some(prior) = metadata.approvals.get(step_id) {
            return Err(WorkflowRunError::StepAlreadyDecided {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
                prior_decision: prior.decision.as_str().to_string(),
            });
        }

        let checkpoints = store
            .list_step_checkpoints(run_id)
            .map_err(WorkflowRunError::from)?;
        let cp = checkpoints
            .iter()
            .find(|c| c.step_id == step_id)
            .ok_or_else(|| WorkflowRunError::StepNotFound {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
            })?;
        if cp.status != PersistStepStatus::AwaitingApproval {
            return Err(WorkflowRunError::StepNotAtApprovalGate {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
                current_status: cp.status.as_str().to_string(),
            });
        }

        metadata.approvals.insert(
            step_id.to_string(),
            ApprovalDecision {
                decision,
                decided_at_ms: now_unix_ms(),
                reason: reason.clone(),
            },
        );

        // 0.4-S9: append the decision to the run's hash-chained
        // audit log. The append is committed atomically with the
        // approvals-blob write below — both live inside the same
        // metadata_json column and the CAS protects them as a unit.
        // The hash chain entry's format is dictated by the AuditLog
        // contract (sha256 of sequence || prev_hash || event_json).
        let mut audit = crate::audit::AuditLog::from_entries(metadata.audit_log.clone());
        let audit_event = match decision {
            ApprovalKind::Approved => crate::audit::AuditEvent::ApprovalGranted {
                step_id: step_id.to_string(),
                // Approver identity is operator-supplied via the CLI;
                // 0.4-S9 surfaces an empty string until a future
                // identity sprint wires real auth. The field is
                // captured in the hash chain regardless so a future
                // upgrade can fill it in without re-keying past
                // entries.
                approver: String::new(),
            },
            ApprovalKind::Rejected => crate::audit::AuditEvent::ApprovalDenied {
                step_id: step_id.to_string(),
                reason: reason.clone().unwrap_or_default(),
            },
        };
        audit.append(audit_event);
        metadata.audit_log = audit.into_entries();

        let updated_metadata = serde_json::to_string(&metadata)
            .map_err(|e| WorkflowRunError::Internal(format!("metadata serialize: {e}")))?;

        let swapped = store
            .compare_and_swap_metadata(run_id, &metadata_json, &updated_metadata, now_unix_ms())
            .map_err(WorkflowRunError::from)?;
        if swapped {
            return Ok(());
        }
        // CAS lost — concurrent writer modified metadata. Loop and
        // re-validate; the re-read either reveals their decision (we
        // surface StepAlreadyDecided) or just bumped some unrelated
        // field (we re-attempt our CAS).
    }
    Err(WorkflowRunError::Internal(format!(
        "CAS retry budget exhausted recording approval for step '{step_id}' in run '{run_id}' \
         (likely a contended hot run; try again)"
    )))
}

/// Record an external-trigger event for a paused workflow run (sprint
/// 0.3-S15).
///
/// Public entry for the `boruna workflow trigger` CLI handler. Validates
/// the run + step state, validates the operator-supplied token against
/// the one stashed at pause-time, mutates the run's `metadata.triggers`
/// blob with the payload, and writes back via compare-and-swap.
///
/// **Does not advance the run.** Like the approval-gate flow, the
/// operator must subsequently invoke `boruna workflow resume <run-id>`.
/// Resume notices the trigger sentinel (a `TriggerRecord` with non-empty
/// `payload`) and advances the paused step to `Completed`, with the
/// payload as its output value.
///
/// ## Validation order
///
/// 1. Run must exist (`RunNotFound`)
/// 2. Run must not be in a terminal state (`RunNotResumable`)
/// 3. Workflow def must be loadable from persisted metadata
/// 4. Step must exist in the workflow def (`StepNotFound`)
/// 5. Step must be a `StepKind::ExternalTrigger` (`NotAnExternalTriggerStep`)
/// 6. Step's persisted checkpoint must be `awaiting_external_event`
///    (`StepNotAtExternalTriggerGate { current_status }` if not)
/// 7. Step's stashed trigger token must match the supplied `token`
///    (`InvalidTriggerToken` if not)
/// 8. Step must not already have a recorded payload
///    (`StepAlreadyTriggered { prior_triggered_at_ms }` if so) —
///    idempotency guard against webhook replay.
///
/// ## Concurrency
///
/// Read + validate + write run inside the same compare-and-swap loop
/// pattern as [`record_approval_decision`]: contended writes converge
/// after a re-read, where the re-read either reveals the prior trigger
/// (we surface `StepAlreadyTriggered`) or just bumped some unrelated
/// metadata key (we re-attempt the CAS).
#[cfg(feature = "persist-sqlite")]
pub fn record_external_trigger(
    data_dir: &Path,
    run_id: &str,
    step_id: &str,
    token: &str,
    payload: &str,
) -> Result<(), WorkflowRunError> {
    let store = open_store(data_dir)?;
    record_external_trigger_in_store(&store, run_id, step_id, token, payload)
}

/// Store-scoped variant of [`record_external_trigger`] for callers
/// that hold an existing `RunCheckpointStore` lock (sprint `0.5-S6`:
/// the coordinator's `POST /api/runs/{run_id}/trigger` HTTP handler).
/// Behaviorally identical to [`record_external_trigger`]; only the
/// store source differs.
///
/// Loads the workflow def from `metadata.workflow_def` when present
/// (covers remote-submit + submit-only modes), falling back to
/// reading `<workflow_dir>/workflow.json` for in-process runs.
#[cfg(feature = "persist-sqlite")]
pub fn record_external_trigger_in_store(
    store: &RunCheckpointStore,
    run_id: &str,
    step_id: &str,
    token: &str,
    payload: &str,
) -> Result<(), WorkflowRunError> {
    // Defense-in-depth contract: the resume sentinel pass uses
    // `payload.is_empty()` to discriminate "pause-time placeholder"
    // from "trigger arrived." An empty payload here would leave the
    // trigger record indistinguishable from the placeholder and the
    // step would never advance. The CLI's serde_json validation also
    // catches empty input ("EOF while parsing"), but enforcing the
    // invariant locally protects future callers (e.g. a programmatic
    // embedder).
    if payload.is_empty() {
        return Err(WorkflowRunError::Validation(
            "trigger payload must not be empty".into(),
        ));
    }

    const CAS_RETRY_BUDGET: usize = 5;
    for _ in 0..CAS_RETRY_BUDGET {
        let record = store
            .get_run_record(run_id)
            .map_err(WorkflowRunError::from)?
            .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;
        if let Some(terminal) = record.terminal_status {
            return Err(WorkflowRunError::RunNotResumable {
                run_id: run_id.to_string(),
                terminal_status: terminal.as_str().to_string(),
            });
        }

        let metadata_json = store
            .get_run_metadata(run_id)
            .map_err(WorkflowRunError::from)?
            .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;
        let mut metadata: PersistedRunMetadata =
            serde_json::from_str(&metadata_json).map_err(|e| {
                WorkflowRunError::Internal(format!("corrupt metadata_json for run '{run_id}': {e}"))
            })?;

        let def = load_def_from_metadata(&metadata, run_id)?;

        let step_def = def
            .steps
            .get(step_id)
            .ok_or_else(|| WorkflowRunError::StepNotFound {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
            })?;
        if !matches!(step_def.kind, StepKind::ExternalTrigger { .. }) {
            return Err(WorkflowRunError::NotAnExternalTriggerStep {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
            });
        }

        let existing = metadata.triggers.get(step_id).cloned();
        let stashed_token = match &existing {
            Some(rec) => rec.token.clone(),
            None => {
                // No stashed token means the runner never reached this
                // step's pause — the operator triggered too early. Surface
                // as the gate-state error (cleanest fit) so the CLI
                // distinguishes "wrong gate state" from "wrong token."
                let checkpoints = store
                    .list_step_checkpoints(run_id)
                    .map_err(WorkflowRunError::from)?;
                let cp = checkpoints
                    .iter()
                    .find(|c| c.step_id == step_id)
                    .ok_or_else(|| WorkflowRunError::StepNotFound {
                        run_id: run_id.to_string(),
                        step_id: step_id.to_string(),
                    })?;
                return Err(WorkflowRunError::StepNotAtExternalTriggerGate {
                    run_id: run_id.to_string(),
                    step_id: step_id.to_string(),
                    current_status: cp.status.as_str().to_string(),
                });
            }
        };

        // Idempotency guard: if a payload already exists for this step,
        // refuse rather than overwriting. Webhook delivery often retries;
        // we surface the prior timestamp so the caller can detect this is
        // a duplicate delivery, not a fresh trigger.
        if let Some(rec) = &existing {
            if !rec.payload.is_empty() {
                return Err(WorkflowRunError::StepAlreadyTriggered {
                    run_id: run_id.to_string(),
                    step_id: step_id.to_string(),
                    prior_triggered_at_ms: rec.triggered_at_ms,
                });
            }
        }

        // Token validation. Constant-time compare avoids the (extremely
        // narrow) timing-leak where short-circuit `==` could disclose
        // the prefix of a token byte-by-byte. Tokens are 32 hex chars
        // either way, so `subtle::ConstantTimeEq` is overkill — the
        // minimum-length-equality + xor-fold is enough.
        let supplied = token.as_bytes();
        let stashed = stashed_token.as_bytes();
        let token_match = supplied.len() == stashed.len() && {
            let mut acc: u8 = 0;
            for (a, b) in supplied.iter().zip(stashed.iter()) {
                acc |= a ^ b;
            }
            acc == 0
        };
        if !token_match {
            return Err(WorkflowRunError::InvalidTriggerToken {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
            });
        }

        // Verify the persisted checkpoint is still in the gate state. The
        // metadata-side token check is necessary but not sufficient: a
        // crashed-mid-resume run might have advanced the checkpoint past
        // the gate while leaving the metadata triggers blob behind. Re-
        // checking the checkpoint is the authoritative gate.
        let checkpoints = store
            .list_step_checkpoints(run_id)
            .map_err(WorkflowRunError::from)?;
        let cp = checkpoints
            .iter()
            .find(|c| c.step_id == step_id)
            .ok_or_else(|| WorkflowRunError::StepNotFound {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
            })?;
        if cp.status != PersistStepStatus::AwaitingExternalEvent {
            return Err(WorkflowRunError::StepNotAtExternalTriggerGate {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
                current_status: cp.status.as_str().to_string(),
            });
        }

        let triggered_at_ms = now_unix_ms();
        metadata.triggers.insert(
            step_id.to_string(),
            TriggerRecord {
                token: stashed_token,
                payload: payload.to_string(),
                triggered_at_ms,
            },
        );

        // 0.3-S16: atomic commit. The persistence layer transitions
        // the step's checkpoint to Completed (with the synthesized
        // output) AND CAS-updates the metadata in a single SQL
        // transaction under BEGIN IMMEDIATE. This closes the TOCTOU
        // race where a concurrent `boruna workflow resume` could mark
        // the step Running between separate metadata-CAS and
        // checkpoint-write paths — leaving a non-empty payload + a
        // non-AwaitingExternalEvent checkpoint that the next resume's
        // sentinel pass would silently log-and-discard.
        //
        // The synthesized output value is `Value::String(payload)` —
        // downstream steps read it via `step_input(name)` (sprint
        // 0.3-S14) and parse the JSON inline if they want typed access.
        let synthetic = boruna_bytecode::Value::String(payload.to_string());
        let output_json = serde_json::to_string(&synthetic)
            .map_err(|e| WorkflowRunError::Internal(format!("trigger output serialize: {e}")))?;
        let output_hash = DataStore::hash_value(&synthetic);

        // 0.4-S9: append the trigger to the run's hash-chained audit
        // log. The chain captures `payload_hash` (matches the
        // synthesized output_hash above) — the payload itself is
        // operator-supplied and may contain PII, so we hash rather
        // than log it verbatim. The append is committed atomically
        // with the metadata + checkpoint write via
        // `commit_external_trigger`; if the CAS loses, the loop
        // re-reads metadata (now with whatever event the racing
        // writer appended) and rebuilds the chain on top.
        let mut audit = crate::audit::AuditLog::from_entries(metadata.audit_log.clone());
        audit.append(crate::audit::AuditEvent::ExternalTriggerReceived {
            step_id: step_id.to_string(),
            payload_hash: output_hash.clone(),
        });
        metadata.audit_log = audit.into_entries();

        let updated_metadata = serde_json::to_string(&metadata)
            .map_err(|e| WorkflowRunError::Internal(format!("metadata serialize: {e}")))?;

        match store
            .commit_external_trigger(
                run_id,
                step_id,
                &metadata_json,
                &updated_metadata,
                &output_json,
                &output_hash,
                triggered_at_ms,
            )
            .map_err(WorkflowRunError::from)?
        {
            crate::persistence::TriggerCommitOutcome::Committed => return Ok(()),
            crate::persistence::TriggerCommitOutcome::CheckpointStateMismatch {
                current_status,
            } => {
                return Err(WorkflowRunError::StepNotAtExternalTriggerGate {
                    run_id: run_id.to_string(),
                    step_id: step_id.to_string(),
                    current_status,
                });
            }
            crate::persistence::TriggerCommitOutcome::MetadataChanged => {
                // Concurrent writer touched metadata. Loop and
                // re-validate. The re-read either reveals their
                // trigger (we surface StepAlreadyTriggered) or just
                // bumped an unrelated field (we re-attempt our CAS).
                continue;
            }
        }
    }
    Err(WorkflowRunError::Internal(format!(
        "CAS retry budget exhausted recording trigger for step '{step_id}' in run '{run_id}'"
    )))
}

/// Operator-facing view of a single approval decision, surfaced by
/// [`show_run`]. Mirrors the internal `ApprovalDecision` struct but is
/// the public DTO for callers (CLI, future API). Stable across the
/// 0.3.x line.
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApprovalView {
    pub step_id: String,
    pub decision: ApprovalKind,
    /// Unix epoch ms — operational only; not in any audit hash.
    pub decided_at_ms: i64,
    pub reason: Option<String>,
}

/// Operator-facing detail view of one workflow run. Returned by
/// [`show_run`]; consumed by `boruna workflow show`.
///
/// Fields are clearly labeled by their determinism contract: `run` and
/// `checkpoints` carry both replay-verified columns (`workflow_hash`,
/// `output_hash`, terminal `status`) and operational columns
/// (`*_at_ms`); `approvals` is operational-only by design.
///
/// `metadata_parse_error` surfaces metadata-parse corruption directly
/// in the structured output rather than relying solely on a stderr
/// warning that gets dropped when the CLI is piped to `jq`. When
/// `Some`, `approvals` is empty (the parse failed); operators see the
/// corruption programmatically. Reviewed 0.3-S3 (H5/C2): prior contract
/// was stderr-only, which made corrupt-metadata indistinguishable from
/// no-decisions in pipeline consumers.
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone)]
pub struct RunDetail {
    pub run: crate::persistence::RunRow,
    /// Step checkpoints sorted by `step_id` for deterministic output.
    pub checkpoints: Vec<crate::persistence::StepCheckpoint>,
    /// Approval-gate decisions sorted by `step_id`. Empty for runs
    /// with no recorded decisions, for older 0.3-S2b databases, OR
    /// when `metadata_parse_error` is `Some` — see that field for
    /// disambiguation.
    pub approvals: Vec<ApprovalView>,
    /// `Some(<error>)` when the run's `metadata_json` failed to parse.
    /// Operators piping `workflow show --json | jq` get a structured
    /// signal of corruption that stderr doesn't deliver.
    pub metadata_parse_error: Option<String>,
}

/// Fetch the full state of a single run for operator inspection.
///
/// Returns [`WorkflowRunError::RunNotFound`] when the run_id doesn't
/// exist (project-conventions §1).
///
/// Public CLI entry for `boruna workflow show`. Corrupt
/// `metadata_json` does NOT cause this function to fail — operators
/// often need to inspect a corrupt run to triage. The error is
/// surfaced through `RunDetail::metadata_parse_error` (and as a
/// stderr warning); `run` and `checkpoints` are still authoritative.
#[cfg(feature = "persist-sqlite")]
pub fn show_run(data_dir: &Path, run_id: &str) -> Result<RunDetail, WorkflowRunError> {
    let store = open_store(data_dir)?;
    let run = store
        .get_run(run_id)
        .map_err(WorkflowRunError::from)?
        .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;
    let checkpoints = store
        .list_step_checkpoints(run_id)
        .map_err(WorkflowRunError::from)?;
    let (approvals, metadata_parse_error) =
        match serde_json::from_str::<PersistedRunMetadata>(&run.metadata_json) {
            Ok(meta) => (
                meta.approvals
                    .into_iter()
                    .map(|(step_id, d)| ApprovalView {
                        step_id,
                        decision: d.decision,
                        decided_at_ms: d.decided_at_ms,
                        reason: d.reason,
                    })
                    .collect::<Vec<_>>(),
                None,
            ),
            Err(e) => {
                eprintln!(
                    "warning: could not parse metadata_json for run '{run_id}' \
                     (showing run + checkpoints anyway): {e}"
                );
                (Vec::new(), Some(e.to_string()))
            }
        };
    Ok(RunDetail {
        run,
        checkpoints,
        approvals,
        metadata_parse_error,
    })
}

/// Build an evidence bundle from a persisted run (sprint `0.4-S10`).
///
/// Public entry for the `boruna evidence create` CLI handler. Reads
/// the run's row, step checkpoints, and metadata blob; constructs an
/// [`EvidenceBundleBuilder`]; populates it with the workflow
/// definition, policy snapshot, per-step output JSON, and the
/// hash-chained audit log persisted in `metadata.audit_log`. Returns
/// the finalized [`BundleManifest`].
///
/// **Post-hoc bundle creation.** The runner does NOT auto-create
/// bundles during execution; this function is invoked explicitly by
/// the operator on a completed (or paused) run. That keeps the hot
/// path free of bundle I/O and lets operators re-bundle on demand
/// (e.g., after a compliance request months later).
///
/// **What goes in the bundle:**
/// - `workflow.json` — read from the run's persisted `workflow_dir`.
///   Operator-supplied; same source as `record_approval_decision`'s
///   workflow_hash check.
/// - `policy.json` — read from the run's persisted `policy_json`
///   column.
/// - `outputs/<step_id>/result.json` — read from each step
///   checkpoint's `output_json` column. Steps without an output
///   (failed, skipped, in-flight) contribute no file.
/// - `audit_log.json` — the full chain from `metadata.audit_log`.
///   Empty `[]` for runs with no recorded operator decisions.
/// - `env_fingerprint.json` — captured at bundle-finalize time
///   (operational; reflects bundling-host environment, not run-host).
/// - `manifest.json` — bundle hash + per-file checksums.
///
/// **Determinism:** the bundle hash incorporates `started_at` /
/// `completed_at` timestamps from `EvidenceBundleBuilder`, so two
/// bundles built from the same run at different times will have
/// different `bundle_hash`. Per-file checksums (workflow_hash,
/// policy_hash, audit_log_hash, output checksums) ARE deterministic
/// across re-bundlings.
#[cfg(feature = "persist-sqlite")]
pub fn create_bundle(
    data_dir: &Path,
    run_id: &str,
    output_dir: &Path,
) -> Result<crate::audit::BundleManifest, WorkflowRunError> {
    use crate::audit::{AuditLog, EvidenceBundleBuilder};

    let store = open_store(data_dir)?;
    let run = store
        .get_run(run_id)
        .map_err(WorkflowRunError::from)?
        .ok_or_else(|| WorkflowRunError::RunNotFound(run_id.to_string()))?;
    let checkpoints = store
        .list_step_checkpoints(run_id)
        .map_err(WorkflowRunError::from)?;
    let metadata: PersistedRunMetadata = serde_json::from_str(&run.metadata_json).map_err(|e| {
        WorkflowRunError::Internal(format!("corrupt metadata_json for run '{run_id}': {e}"))
    })?;

    let mut builder = EvidenceBundleBuilder::new(output_dir, run_id, &run.workflow_name)
        .map_err(|e| WorkflowRunError::Io(format!("bundle directory: {e}")))?;

    // Workflow definition snapshot. Source of truth is the on-disk
    // workflow.json at the run's recorded workflow_dir — same path
    // record_approval_decision validates against. If it's gone (the
    // operator moved or deleted the workflow), the bundle write
    // fails loudly rather than silently producing an incomplete
    // bundle.
    let workflow_path = Path::new(&metadata.workflow_dir).join("workflow.json");
    let workflow_json = std::fs::read_to_string(&workflow_path).map_err(|e| {
        WorkflowRunError::Io(format!(
            "cannot read workflow.json from {} (recorded workflow_dir): {e}",
            workflow_path.display()
        ))
    })?;
    builder
        .add_workflow_def(&workflow_json)
        .map_err(|e| WorkflowRunError::Io(format!("bundle add_workflow_def: {e}")))?;

    // Policy snapshot — directly from the run's `policy_json`
    // column, no parse / re-serialize round-trip (so the bundle
    // captures bit-identical bytes the runner saw).
    builder
        .add_policy(&run.policy_json)
        .map_err(|e| WorkflowRunError::Io(format!("bundle add_policy: {e}")))?;

    // Per-step outputs. Steps with no output (failed before producing
    // output, still pending, or paused at a gate) contribute nothing.
    // Each output is added as `outputs/<step_id>/result.json` — matches
    // `WorkflowRunner::execute_*`'s "result"-named output convention.
    //
    // Sprint 0.5-S7: outputs that exceeded `BLOB_THRESHOLD` live in the
    // blob store, not in the row's `output_json` column. We MUST resolve
    // via the new `read_step_output` accessor — reading `cp.output_json`
    // directly would silently omit large outputs from the evidence
    // bundle, which is a compliance regression (the bundle would verify
    // but be incomplete).
    for cp in &checkpoints {
        if let Some(output_json) = store
            .read_step_output(run_id, &cp.step_id)
            .map_err(WorkflowRunError::from)?
        {
            builder
                .add_step_output(&cp.step_id, "result", &output_json)
                .map_err(|e| {
                    WorkflowRunError::Io(format!(
                        "bundle add_step_output for '{}': {e}",
                        cp.step_id
                    ))
                })?;
        }
    }

    // Hash-chained audit log from metadata. We verify the chain at
    // bundle-creation time so that direct sqlite3 tamper of
    // `metadata.audit_log` surfaces *here* (operator gets a clear
    // error and an unwritten bundle) rather than propagating into a
    // bundle that downstream `boruna evidence verify` would later
    // flag — the latter is too late to prevent a tampered bundle
    // from being shipped to a compliance reviewer. This catches
    // chain-link corruption (mutation without re-hashing); a fully
    // forged chain rebuilt with consistent hashes is out of scope
    // for this defense and requires an out-of-band hash root.
    let audit = AuditLog::from_entries_verified(metadata.audit_log).map_err(|bad_seq| {
        WorkflowRunError::Io(format!(
            "audit chain integrity check failed at sequence {bad_seq}: \
             metadata.audit_log appears to have been tampered with — \
             refusing to build evidence bundle"
        ))
    })?;

    let manifest = builder
        .finalize(&audit)
        .map_err(|e| WorkflowRunError::Io(format!("bundle finalize: {e}")))?;
    Ok(manifest)
}

/// List all paused/running/completed/failed runs in a `data_dir`.
/// Returns `RunRow`s ordered by `(workflow_name, run_id)`. Optional
/// `status_filter` reuses `list_runs_by_status`.
///
/// Public CLI entry for `boruna workflow list`.
#[cfg(feature = "persist-sqlite")]
pub fn list_runs(
    data_dir: &Path,
    status_filter: Option<crate::persistence::RunStatus>,
) -> Result<Vec<crate::persistence::RunRow>, WorkflowRunError> {
    let store = open_store(data_dir)?;
    let rows = match status_filter {
        Some(s) => store.list_runs_by_status(s),
        None => store.list_runs(),
    };
    rows.map_err(WorkflowRunError::from)
}

/// List in-flight (`Running` or `Paused`) runs for a workflow def.
///
/// Used by the `--skip-if-running` CLI flag (sprint `0.3-S7`) to
/// decide whether a cron-triggered invocation should skip a new run
/// because a prior one is still active. Computes the canonical
/// `workflow_hash` from the def, then queries the persistent store.
///
/// Returns an empty `Vec` if no run is currently in flight for this
/// workflow.
#[cfg(feature = "persist-sqlite")]
pub fn find_in_flight_runs(
    data_dir: &Path,
    def: &WorkflowDef,
) -> Result<Vec<crate::persistence::RunRow>, WorkflowRunError> {
    let store = open_store(data_dir)?;
    let workflow_hash = WorkflowRunner::workflow_hash_from_def(def);
    store
        .list_in_flight_runs_for_workflow(&workflow_hash)
        .map_err(WorkflowRunError::from)
}

/// Errors that can occur during a workflow run.
#[derive(Debug, Clone)]
pub enum WorkflowRunError {
    Validation(String),
    StepFailed(String, String),
    Io(String),
    Internal(String),
    /// Resume target run_id does not exist in the store. Surfaced from
    /// `WorkflowRunner::resume`. Aligned with project-conventions §1: a
    /// typo'd run_id MUST surface as a typed error rather than silent no-op.
    #[cfg(feature = "persist-sqlite")]
    RunNotFound(String),
    /// Resume refused because the on-disk workflow definition's hash does
    /// not match the persisted run's `workflow_hash`. Editing a step's
    /// source file between run and resume invalidates the audit chain.
    #[cfg(feature = "persist-sqlite")]
    WorkflowHashMismatch {
        run_id: String,
        expected: String,
        actual: String,
    },
    /// Wrapped persistence-layer error. Preserves the typed `error_kind`
    /// of the underlying store operation for CLI surfacing.
    #[cfg(feature = "persist-sqlite")]
    Persistence(String),
    /// `boruna workflow approve`/`reject` named a step that doesn't exist
    /// in the run's checkpoints OR in the workflow definition. Sprint
    /// `0.3-S2c`.
    #[cfg(feature = "persist-sqlite")]
    StepNotFound {
        run_id: String,
        step_id: String,
    },
    /// `approve`/`reject` named a step whose persisted checkpoint is not
    /// in `awaiting_approval` state. Distinguished from
    /// [`StepAlreadyDecided`] so operators see the actual current state.
    #[cfg(feature = "persist-sqlite")]
    StepNotAtApprovalGate {
        run_id: String,
        step_id: String,
        current_status: String,
    },
    /// `approve`/`reject` named a step that already has a recorded
    /// decision in `metadata.approvals`. Caller intent is ambiguous —
    /// did they mean to override? — so we refuse rather than silently
    /// overwriting.
    #[cfg(feature = "persist-sqlite")]
    StepAlreadyDecided {
        run_id: String,
        step_id: String,
        prior_decision: String,
    },
    /// `approve`/`reject` named a step whose `StepDef` in the workflow
    /// definition is NOT a `StepKind::ApprovalGate`. Distinguishes
    /// "approving a non-gate" from "approving a gate that's not paused."
    #[cfg(feature = "persist-sqlite")]
    NotAnApprovalGateStep {
        run_id: String,
        step_id: String,
    },
    /// `approve`/`reject` against a run whose persisted status is
    /// already terminal (`Completed`/`Failed`). Mutating the metadata of
    /// a finished run is a footgun — refuse.
    #[cfg(feature = "persist-sqlite")]
    RunNotResumable {
        run_id: String,
        terminal_status: String,
    },
    /// `boruna workflow trigger` named a step whose `StepDef` is NOT a
    /// `StepKind::ExternalTrigger` (sprint 0.3-S15). Distinguishes
    /// "triggering a non-trigger" from "triggering a step that's not
    /// paused."
    #[cfg(feature = "persist-sqlite")]
    NotAnExternalTriggerStep {
        run_id: String,
        step_id: String,
    },
    /// `boruna workflow trigger` named a step whose persisted checkpoint
    /// is not in `awaiting_external_event` state (sprint 0.3-S15).
    /// Distinguishes "trigger arrived too late" (step already advanced)
    /// from "trigger arrived too early" (step not yet paused).
    #[cfg(feature = "persist-sqlite")]
    StepNotAtExternalTriggerGate {
        run_id: String,
        step_id: String,
        current_status: String,
    },
    /// `boruna workflow trigger` was invoked with a `--token` value that
    /// does not match the token stashed at pause-time (sprint 0.3-S15).
    /// Prevents accidental cross-step triggers in the "operator pasted
    /// the wrong webhook payload" footgun.
    #[cfg(feature = "persist-sqlite")]
    InvalidTriggerToken {
        run_id: String,
        step_id: String,
    },
    /// `boruna workflow trigger` named a step that already has a
    /// recorded payload (sprint 0.3-S15). Idempotency guard: replay of
    /// a webhook should not re-trigger the same step. Caller can detect
    /// the prior trigger via this typed error.
    #[cfg(feature = "persist-sqlite")]
    StepAlreadyTriggered {
        run_id: String,
        step_id: String,
        prior_triggered_at_ms: i64,
    },
}

#[cfg(feature = "persist-sqlite")]
impl From<PersistenceError> for WorkflowRunError {
    fn from(e: PersistenceError) -> Self {
        WorkflowRunError::Persistence(e.to_string())
    }
}

impl std::fmt::Display for WorkflowRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "validation error: {msg}"),
            Self::StepFailed(step, msg) => write!(f, "step '{step}' failed: {msg}"),
            Self::Io(msg) => write!(f, "IO error: {msg}"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
            #[cfg(feature = "persist-sqlite")]
            Self::RunNotFound(run_id) => write!(f, "run not found: '{run_id}'"),
            #[cfg(feature = "persist-sqlite")]
            Self::WorkflowHashMismatch {
                run_id,
                expected,
                actual,
            } => write!(
                f,
                "workflow hash mismatch for run '{run_id}': persisted={expected}, on-disk={actual}"
            ),
            #[cfg(feature = "persist-sqlite")]
            Self::Persistence(msg) => write!(f, "persistence error: {msg}"),
            #[cfg(feature = "persist-sqlite")]
            Self::StepNotFound { run_id, step_id } => write!(
                f,
                "step '{step_id}' not found in run '{run_id}'"
            ),
            #[cfg(feature = "persist-sqlite")]
            Self::StepNotAtApprovalGate {
                run_id,
                step_id,
                current_status,
            } => write!(
                f,
                "step '{step_id}' in run '{run_id}' is in state '{current_status}', not awaiting_approval"
            ),
            #[cfg(feature = "persist-sqlite")]
            Self::StepAlreadyDecided {
                run_id,
                step_id,
                prior_decision,
            } => write!(
                f,
                "step '{step_id}' in run '{run_id}' was already {prior_decision}"
            ),
            #[cfg(feature = "persist-sqlite")]
            Self::NotAnApprovalGateStep { run_id, step_id } => write!(
                f,
                "step '{step_id}' in run '{run_id}' is not an approval gate"
            ),
            #[cfg(feature = "persist-sqlite")]
            Self::RunNotResumable {
                run_id,
                terminal_status,
            } => write!(
                f,
                "run '{run_id}' is {terminal_status}; cannot mutate"
            ),
            #[cfg(feature = "persist-sqlite")]
            Self::NotAnExternalTriggerStep { run_id, step_id } => write!(
                f,
                "step '{step_id}' in run '{run_id}' is not an external_trigger step"
            ),
            #[cfg(feature = "persist-sqlite")]
            Self::StepNotAtExternalTriggerGate {
                run_id,
                step_id,
                current_status,
            } => write!(
                f,
                "step '{step_id}' in run '{run_id}' is in state '{current_status}', \
                 not awaiting_external_event"
            ),
            #[cfg(feature = "persist-sqlite")]
            Self::InvalidTriggerToken { run_id, step_id } => write!(
                f,
                "trigger token mismatch for step '{step_id}' in run '{run_id}'"
            ),
            #[cfg(feature = "persist-sqlite")]
            Self::StepAlreadyTriggered {
                run_id,
                step_id,
                prior_triggered_at_ms,
            } => write!(
                f,
                "step '{step_id}' in run '{run_id}' was already triggered \
                 at unix_ms={prior_triggered_at_ms}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_workflow_with_steps(step_sources: &[(&str, &str)]) -> (WorkflowDef, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();

        let mut steps = BTreeMap::new();
        let mut edges = Vec::new();
        let mut prev: Option<String> = None;

        for (id, source_code) in step_sources {
            let filename = format!("steps/{id}.ax");
            std::fs::write(dir.path().join(&filename), source_code).unwrap();

            steps.insert(
                id.to_string(),
                StepDef {
                    kind: StepKind::Source { source: filename },
                    capabilities: vec![],
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    depends_on: vec![],
                    timeout_ms: None,
                    retry: None,
                    budget: None,
                    required_capability_versions: Default::default(),
                },
            );

            if let Some(prev_id) = &prev {
                edges.push((prev_id.clone(), id.to_string()));
            }
            prev = Some(id.to_string());
        }

        let def = WorkflowDef {
            schema_version: 1,
            name: "test-workflow".into(),
            version: "1.0.0".into(),
            description: "test".into(),
            steps,
            edges,
        };

        (def, dir)
    }

    // ── Submit-only mode (sprint 0.5-S2e) ──

    #[test]
    fn submit_only_inserts_run_and_initial_pending_checkpoints() {
        let (def, dir) = make_workflow_with_steps(&[
            ("step1", "fn main() -> Int { 1 }"),
            ("step2", "fn main() -> Int { 2 }"),
        ]);
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        // Submit-only returns an in-flight result.
        assert_eq!(result.status, WorkflowStatus::Running);
        assert!(result.step_results.is_empty());

        // The run row exists; step1 (initial wave) is Pending.
        let store_path = data_dir.path().join("runs.db");
        let store = crate::persistence::RunCheckpointStore::open(&store_path).unwrap();
        let cps = store.list_step_checkpoints(&result.run_id).unwrap();
        assert_eq!(cps.len(), 1, "only initial-wave step should be inserted");
        assert_eq!(cps[0].step_id, "step1");
        assert_eq!(cps[0].status, crate::persistence::StepStatus::Pending);
    }

    #[test]
    fn submit_only_embeds_step_sources_in_metadata() {
        let (def, dir) = make_workflow_with_steps(&[
            ("step1", "fn main() -> Int { 99 }"),
            ("step2", "fn main() -> Int { 100 }"),
        ]);
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        let metadata_json = store
            .get_run_metadata(&result.run_id)
            .unwrap()
            .expect("metadata present");
        let parsed: serde_json::Value = serde_json::from_str(&metadata_json).unwrap();
        let sources = parsed
            .get("step_sources")
            .expect("step_sources key present")
            .as_object()
            .expect("step_sources is object");
        assert_eq!(sources.len(), 2);
        assert!(sources["step1"].as_str().unwrap().contains("99"));
        assert!(sources["step2"].as_str().unwrap().contains("100"));
    }

    // ── Multi-wave advancement (sprint 0.5-S2f) ──

    /// Fan-out / fan-in DAG used by the advance-loop tests:
    ///
    /// ```text
    ///   s1 ──┐
    ///   s2 ──┼──▶ s4 ──▶ s5
    ///   s3 ──┘
    /// ```
    ///
    /// Wave 1: {s1, s2, s3} (no upstream).
    /// Wave 2: {s4} (depends on s1, s2, s3).
    /// Wave 3: {s5} (depends on s4).
    fn make_fan_in_workflow() -> (WorkflowDef, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();
        let mut steps = BTreeMap::new();
        for (id, body) in [
            ("s1", "fn main() -> Int { 1 }"),
            ("s2", "fn main() -> Int { 2 }"),
            ("s3", "fn main() -> Int { 3 }"),
            ("s4", "fn main() -> Int { 4 }"),
            ("s5", "fn main() -> Int { 5 }"),
        ] {
            let filename = format!("steps/{id}.ax");
            std::fs::write(dir.path().join(&filename), body).unwrap();
            steps.insert(
                id.to_string(),
                StepDef {
                    kind: StepKind::Source { source: filename },
                    capabilities: vec![],
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    depends_on: vec![],
                    timeout_ms: None,
                    retry: None,
                    budget: None,
                    required_capability_versions: Default::default(),
                },
            );
        }
        let edges = vec![
            ("s1".into(), "s4".into()),
            ("s2".into(), "s4".into()),
            ("s3".into(), "s4".into()),
            ("s4".into(), "s5".into()),
        ];
        let def = WorkflowDef {
            schema_version: 1,
            name: "fan-in".into(),
            version: "1.0.0".into(),
            description: "fan-in test".into(),
            steps,
            edges,
        };
        (def, dir)
    }

    #[test]
    fn compute_ready_steps_initial_state_returns_source_steps() {
        let (def, _dir) = make_fan_in_workflow();
        let status_map = BTreeMap::new();
        let ready = WorkflowRunner::compute_ready_steps(&def, &status_map);
        assert_eq!(
            ready,
            vec!["s1".to_string(), "s2".to_string(), "s3".to_string()]
        );
    }

    #[test]
    fn compute_ready_steps_after_first_wave_returns_downstream() {
        let (def, _dir) = make_fan_in_workflow();
        let mut status_map = BTreeMap::new();
        status_map.insert("s1".to_string(), PersistStepStatus::Completed);
        status_map.insert("s2".to_string(), PersistStepStatus::Completed);
        status_map.insert("s3".to_string(), PersistStepStatus::Completed);
        let ready = WorkflowRunner::compute_ready_steps(&def, &status_map);
        assert_eq!(ready, vec!["s4".to_string()]);
    }

    #[test]
    fn compute_ready_steps_partial_completion_returns_empty() {
        let (def, _dir) = make_fan_in_workflow();
        let mut status_map = BTreeMap::new();
        status_map.insert("s1".to_string(), PersistStepStatus::Completed);
        status_map.insert("s2".to_string(), PersistStepStatus::Pending);
        status_map.insert("s3".to_string(), PersistStepStatus::Completed);
        let ready = WorkflowRunner::compute_ready_steps(&def, &status_map);
        assert!(
            ready.is_empty(),
            "s4 should NOT be ready while s2 is still Pending; got {ready:?}"
        );
    }

    #[test]
    fn compute_ready_steps_skips_already_pending_steps() {
        let (def, _dir) = make_fan_in_workflow();
        let mut status_map = BTreeMap::new();
        // First wave already Pending — wait client wrote them last tick.
        status_map.insert("s1".to_string(), PersistStepStatus::Pending);
        status_map.insert("s2".to_string(), PersistStepStatus::Pending);
        status_map.insert("s3".to_string(), PersistStepStatus::Pending);
        let ready = WorkflowRunner::compute_ready_steps(&def, &status_map);
        assert!(
            ready.is_empty(),
            "Pending steps should not be re-marked ready; got {ready:?}"
        );
    }

    #[test]
    fn compute_ready_steps_sort_is_deterministic() {
        // Build a workflow where step IDs would naturally enumerate
        // out of order if we relied on insertion order. BTreeMap
        // already iterates sorted; this test locks the contract.
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();
        let mut steps = BTreeMap::new();
        for id in ["zeta", "alpha", "mu"] {
            let filename = format!("steps/{id}.ax");
            std::fs::write(dir.path().join(&filename), "fn main() -> Int { 0 }").unwrap();
            steps.insert(
                id.to_string(),
                StepDef {
                    kind: StepKind::Source { source: filename },
                    capabilities: vec![],
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    depends_on: vec![],
                    timeout_ms: None,
                    retry: None,
                    budget: None,
                    required_capability_versions: Default::default(),
                },
            );
        }
        let def = WorkflowDef {
            schema_version: 1,
            name: "sort-test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps,
            edges: vec![],
        };
        let status_map = BTreeMap::new();
        let ready = WorkflowRunner::compute_ready_steps(&def, &status_map);
        assert_eq!(ready, vec!["alpha", "mu", "zeta"]);
    }

    #[test]
    fn advance_run_one_tick_inserts_pending_checkpoints() {
        let (def, dir) = make_fan_in_workflow();
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();

        // First tick: s1, s2, s3 are already Pending (submit-only). No new
        // Pending should be added; run is Running.
        let r0 = WorkflowRunner::advance_run_one_tick(&store, &result.run_id).unwrap();
        assert!(r0.newly_pending.is_empty());
        assert_eq!(r0.run_status, AdvanceRunStatus::Running);

        // Simulate workers completing s1, s2, s3.
        for sid in ["s1", "s2", "s3"] {
            let claim = store
                .claim_step(&result.run_id, sid, "worker-A", 1_000_000_000, 0)
                .unwrap();
            let claim_id = match claim {
                crate::persistence::ClaimOutcome::Claimed { claim_id } => claim_id,
                other => panic!("expected Claimed, got {other:?}"),
            };
            let _ = store
                .complete_step_cas(&result.run_id, sid, claim_id, "{}", "0", 1, 1)
                .unwrap();
        }

        // Second tick: s4 becomes Pending.
        let r1 = WorkflowRunner::advance_run_one_tick(&store, &result.run_id).unwrap();
        assert_eq!(r1.newly_pending, vec!["s4".to_string()]);
        assert_eq!(r1.run_status, AdvanceRunStatus::Running);
    }

    #[test]
    fn advance_run_one_tick_is_idempotent() {
        let (def, dir) = make_fan_in_workflow();
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();

        // No worker activity between ticks → both ticks return zero new pending.
        let r0 = WorkflowRunner::advance_run_one_tick(&store, &result.run_id).unwrap();
        let r1 = WorkflowRunner::advance_run_one_tick(&store, &result.run_id).unwrap();
        assert!(r0.newly_pending.is_empty());
        assert!(r1.newly_pending.is_empty());
    }

    #[test]
    fn advance_run_one_tick_preserves_running_status_on_concurrent_claim() {
        // Race: between ticks, the coordinator claims a Pending step.
        // The next tick's compute_ready_steps must NOT include that step
        // (it's no longer Unknown — it's Running), so no clobbering write.
        let (def, dir) = make_fan_in_workflow();
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        // Coordinator claims s1.
        let claim = store
            .claim_step(&result.run_id, "s1", "worker-A", 1_000_000_000, 0)
            .unwrap();
        assert!(matches!(
            claim,
            crate::persistence::ClaimOutcome::Claimed { .. }
        ));
        // Wait tick runs.
        let r = WorkflowRunner::advance_run_one_tick(&store, &result.run_id).unwrap();
        assert!(r.newly_pending.is_empty());
        // s1 is still Running with worker-A.
        let cps = store.list_step_checkpoints(&result.run_id).unwrap();
        let s1 = cps.iter().find(|c| c.step_id == "s1").unwrap();
        assert_eq!(s1.status, PersistStepStatus::Running);
        assert_eq!(s1.worker_id.as_deref(), Some("worker-A"));
    }

    #[test]
    fn advance_run_one_tick_returns_completed_when_all_done() {
        let (def, dir) = make_fan_in_workflow();
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        // Drive all 5 steps to Completed.
        for sid in ["s1", "s2", "s3", "s4", "s5"] {
            // Ensure it has a Pending row (advance ticks fill in waves 2/3 incrementally).
            loop {
                let cps = store.list_step_checkpoints(&result.run_id).unwrap();
                if cps.iter().any(|c| c.step_id == sid) {
                    break;
                }
                WorkflowRunner::advance_run_one_tick(&store, &result.run_id).unwrap();
            }
            let claim = store
                .claim_step(&result.run_id, sid, "w", 1_000_000_000, 0)
                .unwrap();
            let claim_id = match claim {
                crate::persistence::ClaimOutcome::Claimed { claim_id } => claim_id,
                other => panic!("claim {sid}: {other:?}"),
            };
            store
                .complete_step_cas(&result.run_id, sid, claim_id, "{}", "0", 1, 1)
                .unwrap();
        }
        let r = WorkflowRunner::advance_run_one_tick(&store, &result.run_id).unwrap();
        assert_eq!(r.run_status, AdvanceRunStatus::Completed);
    }

    #[test]
    fn advance_run_one_tick_opens_approval_gate_when_deps_complete() {
        // Sprint 0.5-S6: when an approval-gate step's deps complete,
        // advance must transition the gate from "no checkpoint" →
        // AwaitingApproval (was: rejected with non-first-wave error).
        let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
        let data_dir = tempfile::tempdir().unwrap();
        let r = WorkflowRunner::run_persistent(
            &def,
            &RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: true,
            },
            data_dir.path(),
        )
        .unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        // Drive `analyze` (the only first-wave source step) to Completed.
        let claim = store
            .claim_step(&r.run_id, "analyze", "w", 1_000_000_000, 0)
            .unwrap();
        let claim_id = match claim {
            crate::persistence::ClaimOutcome::Claimed { claim_id } => claim_id,
            other => panic!("{other:?}"),
        };
        store
            .complete_step_cas(&r.run_id, "analyze", claim_id, "{}", "0", 1, 1)
            .unwrap();
        // Now tick — the gate must open.
        let advanced = WorkflowRunner::advance_run_one_tick(&store, &r.run_id).unwrap();
        assert_eq!(advanced.run_status, AdvanceRunStatus::Running);
        let cps = store.list_step_checkpoints(&r.run_id).unwrap();
        let gate_cp = cps
            .iter()
            .find(|c| c.step_id == "human_review")
            .expect("human_review checkpoint inserted");
        assert_eq!(gate_cp.status, PersistStepStatus::AwaitingApproval);
    }

    #[test]
    fn advance_run_one_tick_closes_approved_gate() {
        // After `record_approval_decision_in_store` writes the
        // sentinel, the next tick must transition the gate
        // checkpoint from AwaitingApproval → Completed and unblock
        // downstream steps.
        let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
        let data_dir = tempfile::tempdir().unwrap();
        let r = WorkflowRunner::run_persistent(
            &def,
            &RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: true,
            },
            data_dir.path(),
        )
        .unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        // Wave 1: analyze.
        let claim = store
            .claim_step(&r.run_id, "analyze", "w", 1_000_000_000, 0)
            .unwrap();
        let claim_id = match claim {
            crate::persistence::ClaimOutcome::Claimed { claim_id } => claim_id,
            other => panic!("{other:?}"),
        };
        store
            .complete_step_cas(&r.run_id, "analyze", claim_id, "{}", "0", 1, 1)
            .unwrap();
        WorkflowRunner::advance_run_one_tick(&store, &r.run_id).unwrap();

        // Operator approves.
        record_approval_decision_in_store(
            &store,
            &r.run_id,
            "human_review",
            ApprovalKind::Approved,
            None,
        )
        .unwrap();

        // Next tick: gate closes, publish becomes Pending.
        WorkflowRunner::advance_run_one_tick(&store, &r.run_id).unwrap();
        let cps = store.list_step_checkpoints(&r.run_id).unwrap();
        let gate = cps.iter().find(|c| c.step_id == "human_review").unwrap();
        assert_eq!(gate.status, PersistStepStatus::Completed);
        assert!(gate.output_hash.is_some());
        let publish = cps.iter().find(|c| c.step_id == "publish").unwrap();
        assert_eq!(publish.status, PersistStepStatus::Pending);
    }

    #[test]
    fn advance_run_one_tick_closes_rejected_gate_to_failed() {
        // Symmetric to the approved case: rejection sentinel
        // transitions the gate to Failed and the run reaches Failed.
        let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
        let data_dir = tempfile::tempdir().unwrap();
        let r = WorkflowRunner::run_persistent(
            &def,
            &RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: true,
            },
            data_dir.path(),
        )
        .unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        let claim = store
            .claim_step(&r.run_id, "analyze", "w", 1_000_000_000, 0)
            .unwrap();
        let claim_id = match claim {
            crate::persistence::ClaimOutcome::Claimed { claim_id } => claim_id,
            other => panic!("{other:?}"),
        };
        store
            .complete_step_cas(&r.run_id, "analyze", claim_id, "{}", "0", 1, 1)
            .unwrap();
        WorkflowRunner::advance_run_one_tick(&store, &r.run_id).unwrap();
        record_approval_decision_in_store(
            &store,
            &r.run_id,
            "human_review",
            ApprovalKind::Rejected,
            Some("compliance issue".into()),
        )
        .unwrap();
        let advanced = WorkflowRunner::advance_run_one_tick(&store, &r.run_id).unwrap();
        assert_eq!(advanced.run_status, AdvanceRunStatus::Failed);
        let cps = store.list_step_checkpoints(&r.run_id).unwrap();
        let gate = cps.iter().find(|c| c.step_id == "human_review").unwrap();
        assert_eq!(gate.status, PersistStepStatus::Failed);
        assert!(gate
            .error_msg
            .as_deref()
            .unwrap_or("")
            .contains("compliance issue"));
    }

    #[test]
    fn advance_run_one_tick_returns_failed_when_any_failed() {
        let (def, dir) = make_fan_in_workflow();
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        // s1 fails.
        let claim = store
            .claim_step(&result.run_id, "s1", "w", 1_000_000_000, 0)
            .unwrap();
        let claim_id = match claim {
            crate::persistence::ClaimOutcome::Claimed { claim_id } => claim_id,
            other => panic!("{other:?}"),
        };
        store
            .fail_step_cas(&result.run_id, "s1", claim_id, "boom", 1, 1)
            .unwrap();
        let r = WorkflowRunner::advance_run_one_tick(&store, &result.run_id).unwrap();
        assert_eq!(r.run_status, AdvanceRunStatus::Failed);
    }

    // ── 0.5-S5: distributed retry policies ──

    /// Build a 1-step workflow with a configurable RetryPolicy on s1.
    /// Returns (def, workflow_dir tempdir).
    fn make_retry_policy_workflow(retry: Option<RetryPolicy>) -> (WorkflowDef, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();
        std::fs::write(dir.path().join("steps/s1.ax"), "fn main() -> Int { 1 }").unwrap();
        let mut steps = BTreeMap::new();
        steps.insert(
            "s1".to_string(),
            StepDef {
                kind: StepKind::Source {
                    source: "steps/s1.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry,
                budget: None,
                required_capability_versions: Default::default(),
            },
        );
        let def = WorkflowDef {
            schema_version: 1,
            name: "retry-test".into(),
            version: "1.0.0".into(),
            description: "retry-test".into(),
            steps,
            edges: vec![],
        };
        (def, dir)
    }

    /// Run submit_only on `def` and fail s1 once with the supplied error_msg.
    /// Returns (store, run_id).
    fn submit_and_fail_s1(
        def: &WorkflowDef,
        workflow_dir: &Path,
        error_msg: &str,
    ) -> (RunCheckpointStore, String) {
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: workflow_dir.to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(def, &options, data_dir.path()).unwrap();
        // Reopen the store rather than let the tempdir drop. We
        // leak data_dir intentionally — tests are short-lived and
        // the OS reclaims tempdirs on process exit.
        let db_path = data_dir.path().join("runs.db");
        // Persist the path so the store survives the closure.
        let store = RunCheckpointStore::open(&db_path).unwrap();
        let claim = store
            .claim_step(&result.run_id, "s1", "w", 1_000_000_000, 0)
            .unwrap();
        let claim_id = match claim {
            crate::persistence::ClaimOutcome::Claimed { claim_id } => claim_id,
            other => panic!("{other:?}"),
        };
        store
            .fail_step_cas(&result.run_id, "s1", claim_id, error_msg, 1, 1)
            .unwrap();
        // Move ownership of the store out of the closure; data_dir's
        // tempdir is leaked here so the file stays valid for the
        // caller's subsequent operations on the store.
        std::mem::forget(data_dir);
        (store, result.run_id)
    }

    #[test]
    fn advance_run_one_tick_requeues_failed_step_with_retry_budget() {
        // Adversarial case 1: retry=3, attempt_count=1, on_transient=true.
        // Failed → requeued; run stays Running.
        let (def, dir) = make_retry_policy_workflow(Some(RetryPolicy {
            max_attempts: 3,
            on_transient: true,
            retry_on: vec![],
        }));
        let (store, run_id) = submit_and_fail_s1(&def, dir.path(), "runtime: boom");
        let r = WorkflowRunner::advance_run_one_tick(&store, &run_id).unwrap();
        assert_eq!(r.run_status, AdvanceRunStatus::Running);
        assert_eq!(r.newly_requeued, vec!["s1".to_string()]);
        assert_eq!(
            r.all_step_statuses.get("s1"),
            Some(&PersistStepStatus::Pending),
        );
        let cps = store.list_step_checkpoints(&run_id).unwrap();
        assert_eq!(cps[0].status, PersistStepStatus::Pending);
        assert_eq!(cps[0].attempt_count, 2);
        assert_eq!(cps[0].error_msg, None);
    }

    #[test]
    fn advance_run_one_tick_does_not_requeue_when_budget_exhausted() {
        // Adversarial case 2: retry=2 with attempt_count=2 already burned.
        // No requeue → run = Failed.
        let (def, dir) = make_retry_policy_workflow(Some(RetryPolicy {
            max_attempts: 2,
            on_transient: true,
            retry_on: vec![],
        }));
        let (store, run_id) = submit_and_fail_s1(&def, dir.path(), "runtime: boom");
        // Manually bump attempt_count to 2 to simulate prior retries.
        store
            .upsert_step_checkpoint(&StepCheckpoint {
                run_id: run_id.clone(),
                step_id: "s1".into(),
                status: PersistStepStatus::Failed,
                output_json: None,
                output_hash: None,
                started_at_ms: Some(0),
                ended_at_ms: Some(1),
                error_msg: Some("runtime: boom".into()),
                attempt_count: 2,
                worker_id: None,
                lease_expires_at_ms: None,
                claim_id: 1,
                output_blob_ref: None,
            })
            .unwrap();
        let r = WorkflowRunner::advance_run_one_tick(&store, &run_id).unwrap();
        assert_eq!(r.run_status, AdvanceRunStatus::Failed);
        assert!(r.newly_requeued.is_empty());
    }

    #[test]
    fn advance_run_one_tick_does_not_requeue_for_single_attempt_policy() {
        // Adversarial case 3: max_attempts=1 → no retry. Run = Failed.
        let (def, dir) = make_retry_policy_workflow(Some(RetryPolicy {
            max_attempts: 1,
            on_transient: true,
            retry_on: vec![],
        }));
        let (store, run_id) = submit_and_fail_s1(&def, dir.path(), "runtime: boom");
        let r = WorkflowRunner::advance_run_one_tick(&store, &run_id).unwrap();
        assert_eq!(r.run_status, AdvanceRunStatus::Failed);
        assert!(r.newly_requeued.is_empty());
    }

    #[test]
    fn advance_run_one_tick_does_not_requeue_when_class_not_in_allowlist() {
        // Adversarial case 4: retry_on=["wall_time_exceeded"] but
        // failure class is "runtime_error". No requeue.
        let (def, dir) = make_retry_policy_workflow(Some(RetryPolicy {
            max_attempts: 5,
            on_transient: false,
            retry_on: vec![error_class::WALL_TIME_EXCEEDED.to_string()],
        }));
        let (store, run_id) = submit_and_fail_s1(&def, dir.path(), "runtime: boom");
        let r = WorkflowRunner::advance_run_one_tick(&store, &run_id).unwrap();
        assert_eq!(r.run_status, AdvanceRunStatus::Failed);
        assert!(r.newly_requeued.is_empty());
    }

    #[test]
    fn advance_run_one_tick_requeues_with_empty_retry_on_and_on_transient() {
        // Adversarial case 5: retry_on=[], on_transient=true → retry any class.
        let (def, dir) = make_retry_policy_workflow(Some(RetryPolicy {
            max_attempts: 3,
            on_transient: true,
            retry_on: vec![],
        }));
        let (store, run_id) = submit_and_fail_s1(&def, dir.path(), "compile: bad syntax");
        let r = WorkflowRunner::advance_run_one_tick(&store, &run_id).unwrap();
        assert_eq!(r.run_status, AdvanceRunStatus::Running);
        assert_eq!(r.newly_requeued, vec!["s1".to_string()]);
    }

    #[test]
    fn advance_run_one_tick_does_not_requeue_when_no_retry_policy() {
        // No retry policy at all → Failed remains Failed.
        let (def, dir) = make_retry_policy_workflow(None);
        let (store, run_id) = submit_and_fail_s1(&def, dir.path(), "runtime: boom");
        let r = WorkflowRunner::advance_run_one_tick(&store, &run_id).unwrap();
        assert_eq!(r.run_status, AdvanceRunStatus::Failed);
        assert!(r.newly_requeued.is_empty());
    }

    #[test]
    fn classify_failure_message_known_prefixes() {
        assert_eq!(
            classify_failure_message("compile: bad syntax"),
            error_class::COMPILE_ERROR,
        );
        assert_eq!(
            classify_failure_message("runtime: assertion failed"),
            error_class::RUNTIME_ERROR,
        );
        assert_eq!(
            classify_failure_message("policy parse: invalid JSON"),
            error_class::RUNTIME_ERROR,
        );
        assert_eq!(
            classify_failure_message("cannot read steps/foo.ax: NotFound"),
            error_class::IO_ERROR,
        );
        // Unknown prefix → conservative fallback.
        assert_eq!(
            classify_failure_message("something completely unexpected"),
            error_class::RUNTIME_ERROR,
        );
        assert_eq!(classify_failure_message(""), error_class::RUNTIME_ERROR);
    }

    #[test]
    fn classify_failure_message_detects_transient_network() {
        // Worker-emitted "runtime: <Display of AssertionFailed wrapping
        // a net.fetch handler error>" must classify as transient_network
        // so retry_on=["transient_network"] policies engage.
        assert_eq!(
            classify_failure_message(
                "runtime: assertion failed: capability error: HTTP request failed: dns error"
            ),
            error_class::TRANSIENT_NETWORK,
        );
        assert_eq!(
            classify_failure_message(
                "runtime: assertion failed: capability error: failed to read response body: connection reset"
            ),
            error_class::TRANSIENT_NETWORK,
        );
        // SSRF blocks and allowlist denials are deterministic config
        // errors — must NOT classify as transient_network (operators
        // shouldn't retry a misconfigured policy).
        assert_eq!(
            classify_failure_message(
                "runtime: assertion failed: capability error: blocked request to localhost (localhost)"
            ),
            error_class::RUNTIME_ERROR,
        );
        assert_eq!(
            classify_failure_message(
                "runtime: assertion failed: capability error: domain 'example.com' not in allowlist: []"
            ),
            error_class::RUNTIME_ERROR,
        );
    }

    #[test]
    fn classify_vm_error_detects_transient_network() {
        use boruna_vm::VmError;
        // AssertionFailed wrapping the http_handler's wrappers must
        // map to transient_network on the in-process retry path.
        assert_eq!(
            classify_vm_error(&VmError::AssertionFailed(
                "capability error: HTTP request failed: timeout".into(),
            )),
            error_class::TRANSIENT_NETWORK,
        );
        // Plain assertion failures stay as RUNTIME_ERROR.
        assert_eq!(
            classify_vm_error(&VmError::AssertionFailed("user assertion: x != 0".into())),
            error_class::RUNTIME_ERROR,
        );
        // SSRF block surfaces inside AssertionFailed — must remain
        // RUNTIME_ERROR (deterministic config error, not transient).
        assert_eq!(
            classify_vm_error(&VmError::AssertionFailed(
                "capability error: blocked request to private IP (10.0.0.1)".into(),
            )),
            error_class::RUNTIME_ERROR,
        );
    }

    #[test]
    fn submit_only_embeds_workflow_def_in_metadata() {
        let (def, dir) = make_fan_in_workflow();
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        let metadata_json = store.get_run_metadata(&result.run_id).unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&metadata_json).unwrap();
        let wf = parsed
            .get("workflow_def")
            .expect("workflow_def key present")
            .as_object()
            .expect("workflow_def is object");
        assert_eq!(wf.get("name").unwrap().as_str().unwrap(), "fan-in");
        let steps = wf.get("steps").unwrap().as_object().unwrap();
        assert_eq!(steps.len(), 5);
    }

    #[test]
    fn non_submit_only_does_not_embed_workflow_def() {
        let (def, dir) = make_workflow_with_steps(&[("step1", "fn main() -> Int { 1 }")]);
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: false,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        let metadata_json = store.get_run_metadata(&result.run_id).unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&metadata_json).unwrap();
        // workflow_def should be absent or null.
        let wf = parsed.get("workflow_def");
        assert!(
            matches!(wf, None | Some(&serde_json::Value::Null)),
            "in-process runs should not embed workflow_def; got: {wf:?}"
        );
    }

    #[test]
    fn submit_only_rejects_oversized_workflow_def() {
        // Build a workflow whose serialized def exceeds 1 MiB by stuffing
        // a giant description.
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();
        let filename = "steps/s1.ax";
        std::fs::write(dir.path().join(filename), "fn main() -> Int { 1 }").unwrap();
        let mut steps = BTreeMap::new();
        steps.insert(
            "s1".to_string(),
            StepDef {
                kind: StepKind::Source {
                    source: filename.to_string(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            },
        );
        let def = WorkflowDef {
            schema_version: 1,
            name: "huge".into(),
            version: "1.0.0".into(),
            description: "x".repeat(2 * 1024 * 1024), // 2 MiB description
            steps,
            edges: vec![],
        };
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let err = WorkflowRunner::run_persistent(&def, &options, data_dir.path())
            .expect_err("expected oversize rejection");
        let msg = format!("{err}");
        assert!(msg.contains("workflow_def"), "msg: {msg}");
        assert!(msg.contains("1048576"), "msg: {msg}");
    }

    #[test]
    fn advance_run_one_tick_rejects_run_without_workflow_def() {
        // A run that pre-dates 0.5-S2f (no workflow_def in metadata)
        // should error out clearly.
        let (def, dir) = make_workflow_with_steps(&[("step1", "fn main() -> Int { 1 }")]);
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: false, // does NOT embed workflow_def
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        let err = WorkflowRunner::advance_run_one_tick(&store, &result.run_id)
            .expect_err("expected error for run without embedded workflow_def");
        let msg = format!("{err}");
        assert!(msg.contains("workflow_def"), "msg: {msg}");
        assert!(msg.contains("submit-only"), "msg: {msg}");
    }

    #[test]
    fn submit_only_with_concurrency_gt_one_still_succeeds() {
        // Regression: the --concurrency warning at submit time
        // (adversarial-review F3 from 0.5-S2e) must NOT cause
        // the run to fail. The warning is informational; the
        // submit-only path proceeds normally.
        let (def, dir) = make_workflow_with_steps(&[("step1", "fn main() -> Int { 7 }")]);
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 4, // triggers the warning
            submit_only: true,
        };
        let result =
            WorkflowRunner::run_persistent(&def, &options, data_dir.path()).expect("submit ok");
        assert_eq!(result.status, WorkflowStatus::Running);
        assert!(result.step_results.is_empty());
    }

    // ── append_wait_terminal_audit_event ──

    #[test]
    fn append_wait_terminal_audit_event_appends_workflow_completed() {
        let (def, dir) = make_fan_in_workflow();
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        WorkflowRunner::append_wait_terminal_audit_event(&store, &result.run_id).unwrap();
        let metadata_json = store
            .get_run_metadata(&result.run_id)
            .unwrap()
            .expect("metadata present");
        let parsed: serde_json::Value = serde_json::from_str(&metadata_json).unwrap();
        let log = parsed
            .get("audit_log")
            .expect("audit_log present")
            .as_array()
            .expect("audit_log is array");
        // submit-only emits WorkflowStarted; we just appended
        // WorkflowCompleted.
        let kinds: Vec<&str> = log
            .iter()
            .filter_map(|e| {
                let event = e.get("event")?;
                if let Some(obj) = event.as_object() {
                    obj.keys().next().map(String::as_str)
                } else {
                    None
                }
            })
            .collect();
        assert!(
            kinds.contains(&"WorkflowStarted"),
            "expected WorkflowStarted in chain; got {kinds:?}"
        );
        assert!(
            kinds.contains(&"WorkflowCompleted"),
            "expected WorkflowCompleted in chain; got {kinds:?}"
        );
    }

    #[test]
    fn append_wait_terminal_audit_event_is_idempotent() {
        let (def, dir) = make_fan_in_workflow();
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        // First append — adds the entry.
        WorkflowRunner::append_wait_terminal_audit_event(&store, &result.run_id).unwrap();
        // Second append — must be a no-op (no second WorkflowCompleted).
        WorkflowRunner::append_wait_terminal_audit_event(&store, &result.run_id).unwrap();
        let metadata_json = store.get_run_metadata(&result.run_id).unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&metadata_json).unwrap();
        let log = parsed.get("audit_log").unwrap().as_array().unwrap();
        let completed_count = log
            .iter()
            .filter(|e| {
                e.get("event")
                    .and_then(|v| v.as_object())
                    .map(|obj| obj.contains_key("WorkflowCompleted"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(
            completed_count, 1,
            "expected exactly 1 WorkflowCompleted entry; got {completed_count}"
        );
    }

    #[test]
    fn append_wait_terminal_audit_event_uses_zero_hash_when_no_step_completed() {
        let (def, dir) = make_fan_in_workflow();
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
        let store =
            crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
        // No step completed — append at terminal time anyway.
        WorkflowRunner::append_wait_terminal_audit_event(&store, &result.run_id).unwrap();
        let metadata_json = store.get_run_metadata(&result.run_id).unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&metadata_json).unwrap();
        let log = parsed.get("audit_log").unwrap().as_array().unwrap();
        let completed_entry = log
            .iter()
            .find(|e| {
                e.get("event")
                    .and_then(|v| v.as_object())
                    .map(|obj| obj.contains_key("WorkflowCompleted"))
                    .unwrap_or(false)
            })
            .expect("WorkflowCompleted entry");
        let hash = completed_entry["event"]["WorkflowCompleted"]["result_hash"]
            .as_str()
            .unwrap();
        assert_eq!(hash, "0".repeat(64), "expected zero-hash fallback");
    }

    #[test]
    fn submit_only_rejects_approval_gate_in_first_wave() {
        // Build a workflow whose first wave contains an
        // approval gate. Submit-only should reject it.
        let (def, dir) = make_workflow_with_steps(&[(
            "approve",
            "fn main() -> Int { 1 }", // body unused for approval gate
        )]);
        // Mutate the def to make the only step an approval gate.
        let mut mutated = def.clone();
        for step in mutated.steps.values_mut() {
            step.kind = StepKind::ApprovalGate {
                required_role: "ops".into(),
                condition: None,
            };
        }
        let data_dir = tempfile::tempdir().unwrap();
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: true,
        };
        let err = WorkflowRunner::run_persistent(&mutated, &options, data_dir.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("submit-only"), "msg: {msg}");
        assert!(msg.contains("approval"), "msg: {msg}");
    }

    #[test]
    fn test_run_linear_workflow() {
        let (def, dir) = make_workflow_with_steps(&[
            ("step1", "fn main() -> Int { 1 }"),
            ("step2", "fn main() -> Int { 2 }"),
            ("step3", "fn main() -> Int { 3 }"),
        ]);

        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: false,
        };

        let result = WorkflowRunner::run(&def, &options).unwrap();
        assert_eq!(result.status, WorkflowStatus::Completed);
        assert_eq!(result.step_results.len(), 3);
        for sr in result.step_results.values() {
            assert_eq!(sr.status, StepStatus::Completed);
        }
    }

    #[test]
    fn test_run_with_compile_error() {
        let (def, dir) = make_workflow_with_steps(&[
            ("good", "fn main() -> Int { 1 }"),
            ("bad", "fn main( { }"), // syntax error
        ]);

        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: false,
        };

        let result = WorkflowRunner::run(&def, &options).unwrap();
        assert_eq!(result.status, WorkflowStatus::Failed);
        assert_eq!(result.step_results["good"].status, StepStatus::Completed);
        assert_eq!(result.step_results["bad"].status, StepStatus::Failed);
    }

    #[test]
    fn test_run_with_policy_deny() {
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();
        std::fs::write(
            steps_dir.join("fetch.ax"),
            "fn fetch(url: String) -> String !{net.fetch} { url }\nfn main() -> Int { 0 }",
        )
        .unwrap();

        let def = WorkflowDef {
            schema_version: 1,
            name: "deny-test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([(
                "fetch".into(),
                StepDef {
                    kind: StepKind::Source {
                        source: "steps/fetch.ax".into(),
                    },
                    capabilities: vec!["net.fetch".into()],
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    depends_on: vec![],
                    timeout_ms: None,
                    retry: None,
                    budget: None,
                    required_capability_versions: Default::default(),
                },
            )]),
            edges: vec![],
        };

        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: false,
        };

        // With allow_all, should succeed
        let result = WorkflowRunner::run(&def, &options).unwrap();
        assert_eq!(result.status, WorkflowStatus::Completed);
    }

    #[test]
    fn test_run_approval_gate_pauses() {
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = dir.path().join("steps");
        std::fs::create_dir_all(&steps_dir).unwrap();
        std::fs::write(steps_dir.join("analyze.ax"), "fn main() -> Int { 42 }").unwrap();

        let def = WorkflowDef {
            schema_version: 1,
            name: "approval-test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                (
                    "analyze".into(),
                    StepDef {
                        kind: StepKind::Source {
                            source: "steps/analyze.ax".into(),
                        },
                        capabilities: vec![],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec![],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                        required_capability_versions: Default::default(),
                    },
                ),
                (
                    "approve".into(),
                    StepDef {
                        kind: StepKind::ApprovalGate {
                            required_role: "reviewer".into(),
                            condition: None,
                        },
                        capabilities: vec![],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec!["analyze".into()],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                        required_capability_versions: Default::default(),
                    },
                ),
                (
                    "store".into(),
                    StepDef {
                        kind: StepKind::Source {
                            source: "steps/analyze.ax".into(), // reuse
                        },
                        capabilities: vec![],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec!["approve".into()],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                        required_capability_versions: Default::default(),
                    },
                ),
            ]),
            edges: vec![],
        };

        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
            concurrency: 1,
            submit_only: false,
        };

        let result = WorkflowRunner::run(&def, &options).unwrap();
        assert_eq!(result.status, WorkflowStatus::Paused);
        // analyze completed, approve is awaiting, store not reached
        assert_eq!(result.step_results["analyze"].status, StepStatus::Completed);
        assert_eq!(
            result.step_results["approve"].status,
            StepStatus::AwaitingApproval
        );
        assert!(!result.step_results.contains_key("store"));
    }

    #[test]
    fn test_run_empty_workflow_rejected() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "empty".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::new(),
            edges: vec![],
        };
        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: "/tmp".into(),
            live: false,
            concurrency: 1,
            submit_only: false,
        };
        assert!(WorkflowRunner::run(&def, &options).is_err());
    }

    // ── 0.3-S5: retry policy ──

    mod retry {
        use super::*;
        use crate::workflow::runner::{retry_backoff_ms, retry_with_backoff};
        use std::cell::RefCell;

        fn policy(max_attempts: u32, on_transient: bool) -> RetryPolicy {
            RetryPolicy {
                max_attempts,
                on_transient,
                retry_on: vec![],
            }
        }

        #[test]
        fn backoff_curve_doubles_until_capped() {
            // Curve: 100, 200, 400, 800, 1600, 3200, then capped at 5000.
            assert_eq!(retry_backoff_ms(0), 100);
            assert_eq!(retry_backoff_ms(1), 200);
            assert_eq!(retry_backoff_ms(2), 400);
            assert_eq!(retry_backoff_ms(3), 800);
            assert_eq!(retry_backoff_ms(4), 1600);
            assert_eq!(retry_backoff_ms(5), 3200);
            assert_eq!(retry_backoff_ms(6), 5000);
            assert_eq!(retry_backoff_ms(7), 5000);
            // Saturating arithmetic for very large values.
            assert_eq!(retry_backoff_ms(63), 5000);
        }

        #[test]
        fn retry_succeeds_on_first_attempt_no_loop() {
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> =
                retry_with_backoff(Some(&policy(5, true)), "step", |attempt| {
                    *calls.borrow_mut() += 1;
                    assert_eq!(attempt, 1, "first attempt is 1-indexed");
                    Ok(42)
                });
            // 0.3-S11: helper now returns (value, attempt_count).
            let (value, attempts) = result.unwrap();
            assert_eq!(value, 42);
            assert_eq!(attempts, 1, "first-try success → attempt_count = 1");
            assert_eq!(*calls.borrow(), 1, "no retries on success");
        }

        /// 0.4-S8: helper for the legacy retry tests — wraps a
        /// step-failure into the new (WorkflowRunError, &'static str)
        /// shape. Defaults to RUNTIME_ERROR class to preserve the
        /// "transient" semantics these tests originally exercised.
        fn err_runtime(msg: &str) -> (WorkflowRunError, &'static str) {
            (
                WorkflowRunError::StepFailed("step".into(), msg.to_string()),
                error_class::RUNTIME_ERROR,
            )
        }

        #[test]
        fn retry_succeeds_after_two_failures() {
            // Closure fails on attempts 1 and 2, succeeds on 3.
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> =
                retry_with_backoff(Some(&policy(3, true)), "step", |attempt| {
                    *calls.borrow_mut() += 1;
                    if attempt < 3 {
                        Err(err_runtime(&format!(
                            "transient failure on attempt {attempt}"
                        )))
                    } else {
                        Ok(99)
                    }
                });
            let (value, attempts) = result.unwrap();
            assert_eq!(value, 99);
            assert_eq!(attempts, 3, "succeeded on attempt 3");
            assert_eq!(*calls.borrow(), 3);
        }

        #[test]
        fn retry_exhausts_attempts_and_wraps_error() {
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> =
                retry_with_backoff(Some(&policy(3, true)), "step", |_attempt| {
                    *calls.borrow_mut() += 1;
                    Err(err_runtime("permanent failure"))
                });
            let (err, attempts) = result.unwrap_err();
            assert_eq!(attempts, 3, "all 3 attempts exhausted");
            assert_eq!(*calls.borrow(), 3);
            // Wrapped error includes attempt count.
            let msg = err.to_string();
            assert!(
                msg.contains("failed after 3 attempts"),
                "error msg should include attempt count; got: {msg}"
            );
            assert!(
                msg.contains("permanent failure"),
                "error msg should include the underlying error; got: {msg}"
            );
        }

        #[test]
        fn no_retry_when_on_transient_false() {
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> = retry_with_backoff(
                Some(&policy(5, false)), // on_transient = false
                "step",
                |_attempt| {
                    *calls.borrow_mut() += 1;
                    Err(err_runtime("boom"))
                },
            );
            assert!(result.is_err());
            assert_eq!(*calls.borrow(), 1, "on_transient=false disables retry");
            // Single-attempt error preserves exact original shape (no
            // "failed after N attempts" wrapper).
            let (err, attempts) = result.unwrap_err();
            assert_eq!(attempts, 1);
            assert!(!err.to_string().contains("attempts"));
        }

        #[test]
        fn no_retry_when_max_attempts_le_1() {
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> = retry_with_backoff(
                Some(&policy(1, true)), // max_attempts = 1
                "step",
                |_attempt| {
                    *calls.borrow_mut() += 1;
                    Err(err_runtime("boom"))
                },
            );
            assert!(result.is_err());
            assert_eq!(*calls.borrow(), 1);
        }

        #[test]
        fn no_retry_when_no_policy() {
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> =
                retry_with_backoff(None, "step", |_attempt| {
                    *calls.borrow_mut() += 1;
                    Err(err_runtime("boom"))
                });
            assert!(result.is_err());
            assert_eq!(*calls.borrow(), 1);
        }

        // ── 0.4-S8: per-error-class retry allowlist ──

        /// Constructs a policy with an explicit error-class allowlist.
        fn class_policy(max_attempts: u32, classes: &[&str]) -> RetryPolicy {
            RetryPolicy {
                max_attempts,
                on_transient: false, // ignored when retry_on is non-empty
                retry_on: classes.iter().map(|s| s.to_string()).collect(),
            }
        }

        #[test]
        fn retries_when_class_in_allowlist() {
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> = retry_with_backoff(
                Some(&class_policy(3, &[error_class::WALL_TIME_EXCEEDED])),
                "step",
                |attempt| {
                    *calls.borrow_mut() += 1;
                    if attempt < 3 {
                        Err((
                            WorkflowRunError::StepFailed("step".into(), "timeout".into()),
                            error_class::WALL_TIME_EXCEEDED,
                        ))
                    } else {
                        Ok(7)
                    }
                },
            );
            let (value, attempts) = result.unwrap();
            assert_eq!(value, 7);
            assert_eq!(attempts, 3);
            assert_eq!(*calls.borrow(), 3);
        }

        #[test]
        fn does_not_retry_when_class_not_in_allowlist() {
            // Allowlist has WALL_TIME_EXCEEDED but failures are
            // RUNTIME_ERROR — must short-circuit at first attempt.
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> = retry_with_backoff(
                Some(&class_policy(5, &[error_class::WALL_TIME_EXCEEDED])),
                "step",
                |_attempt| {
                    *calls.borrow_mut() += 1;
                    Err(err_runtime("deterministic failure"))
                },
            );
            assert!(result.is_err());
            assert_eq!(*calls.borrow(), 1, "non-allowlisted class must not retry");
            let (err, attempts) = result.unwrap_err();
            assert_eq!(attempts, 1);
            // Single-attempt error preserves original shape (no
            // "failed after N attempts" wrapper).
            assert!(!err.to_string().contains("attempts"));
        }

        #[test]
        fn empty_allowlist_falls_back_to_on_transient() {
            // retry_on=[] + on_transient=true → legacy behavior.
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> =
                retry_with_backoff(Some(&policy(3, true)), "step", |_attempt| {
                    *calls.borrow_mut() += 1;
                    Err(err_runtime("boom"))
                });
            assert!(result.is_err());
            assert_eq!(
                *calls.borrow(),
                3,
                "empty retry_on + on_transient=true must retry as before"
            );
        }

        #[test]
        fn unknown_class_in_allowlist_is_silently_ignored() {
            // Operator typo: "transient_netwrok" instead of
            // "transient_network". Conservative-by-default — never
            // matches a real class, so behaves as if absent. Single-
            // attempt outcome.
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> = retry_with_backoff(
                Some(&class_policy(3, &["transient_netwrok"])),
                "step",
                |_attempt| {
                    *calls.borrow_mut() += 1;
                    Err(err_runtime("boom"))
                },
            );
            assert!(result.is_err());
            assert_eq!(*calls.borrow(), 1);
        }

        #[test]
        fn legacy_retry_policy_json_deserializes_with_default_retry_on() {
            // Backward-compat: a workflow.json from a 0.3.x build has
            // no `retry_on` field. With #[serde(default)] on the new
            // field, deserialization must produce retry_on = vec![]
            // and preserve the legacy on_transient gate.
            let legacy = r#"{
                "max_attempts": 3,
                "on_transient": true
            }"#;
            let p: RetryPolicy = serde_json::from_str(legacy).unwrap();
            assert_eq!(p.max_attempts, 3);
            assert!(p.on_transient);
            assert!(
                p.retry_on.is_empty(),
                "missing retry_on field must default to empty vec"
            );
            // And the empty allowlist correctly falls back to
            // on_transient at the should_retry_class boundary.
            assert!(should_retry_class(Some(&p), error_class::RUNTIME_ERROR));
        }

        #[test]
        fn non_empty_retry_on_takes_precedence_over_on_transient_false() {
            // Documented contract: when retry_on is non-empty, the
            // legacy on_transient flag is ignored. A class in the
            // allowlist retries even if on_transient = false.
            let p = RetryPolicy {
                max_attempts: 3,
                on_transient: false,
                retry_on: vec![error_class::WALL_TIME_EXCEEDED.to_string()],
            };
            assert!(should_retry_class(
                Some(&p),
                error_class::WALL_TIME_EXCEEDED
            ));
            assert!(!should_retry_class(Some(&p), error_class::RUNTIME_ERROR));

            // End-to-end: a wall-time failure retries; a runtime
            // failure short-circuits.
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> =
                retry_with_backoff(Some(&p), "step", |attempt| {
                    *calls.borrow_mut() += 1;
                    if attempt < 3 {
                        Err((
                            WorkflowRunError::StepFailed("step".into(), "timeout".into()),
                            error_class::WALL_TIME_EXCEEDED,
                        ))
                    } else {
                        Ok(1)
                    }
                });
            assert_eq!(*calls.borrow(), 3);
            assert!(result.is_ok());
        }

        #[test]
        fn allowlist_with_multiple_classes_matches_any() {
            let calls = RefCell::new(0);
            let result: Result<(i32, u32), (WorkflowRunError, u32)> = retry_with_backoff(
                Some(&class_policy(
                    4,
                    &[error_class::WALL_TIME_EXCEEDED, error_class::IO_ERROR],
                )),
                "step",
                |attempt| {
                    *calls.borrow_mut() += 1;
                    // Alternate failure classes — both in the
                    // allowlist, so each retries.
                    if attempt == 1 {
                        Err((
                            WorkflowRunError::StepFailed("step".into(), "io".into()),
                            error_class::IO_ERROR,
                        ))
                    } else if attempt == 2 {
                        Err((
                            WorkflowRunError::StepFailed("step".into(), "wall".into()),
                            error_class::WALL_TIME_EXCEEDED,
                        ))
                    } else {
                        Ok(99)
                    }
                },
            );
            let (value, attempts) = result.unwrap();
            assert_eq!(value, 99);
            assert_eq!(attempts, 3);
            assert_eq!(*calls.borrow(), 3);
        }

        // ── classify_vm_error mapping ──

        #[test]
        fn classify_wall_time_error() {
            assert_eq!(
                classify_vm_error(&VmError::WallTimeExceeded(100)),
                error_class::WALL_TIME_EXCEEDED
            );
        }

        #[test]
        fn classify_step_limit_error() {
            assert_eq!(
                classify_vm_error(&VmError::ExecutionLimitExceeded(1_000_000)),
                error_class::STEP_LIMIT_EXCEEDED
            );
        }

        #[test]
        fn classify_capability_denied_error() {
            assert_eq!(
                classify_vm_error(&VmError::CapabilityDenied(
                    boruna_bytecode::Capability::NetFetch
                )),
                error_class::CAPABILITY_DENIED
            );
        }

        #[test]
        fn classify_capability_budget_error() {
            assert_eq!(
                classify_vm_error(&VmError::CapabilityBudgetExceeded(
                    boruna_bytecode::Capability::LlmCall
                )),
                error_class::CAPABILITY_BUDGET_EXCEEDED
            );
        }

        #[test]
        fn classify_runtime_catchall() {
            // Assertion failures, type errors, OOB, division — all
            // surface as runtime_error via the catch-all.
            assert_eq!(
                classify_vm_error(&VmError::AssertionFailed("boom".into())),
                error_class::RUNTIME_ERROR
            );
            assert_eq!(
                classify_vm_error(&VmError::DivisionByZero),
                error_class::RUNTIME_ERROR
            );
            assert_eq!(
                classify_vm_error(&VmError::IndexOutOfBounds {
                    index: 99,
                    length: 3
                }),
                error_class::RUNTIME_ERROR
            );
        }

        // ── should_retry_class semantics ──

        #[test]
        fn should_retry_class_no_policy() {
            assert!(!should_retry_class(None, error_class::WALL_TIME_EXCEEDED));
        }

        #[test]
        fn should_retry_class_max_attempts_le_1() {
            let p = policy(1, true);
            assert!(!should_retry_class(
                Some(&p),
                error_class::WALL_TIME_EXCEEDED
            ));
        }

        #[test]
        fn should_retry_class_allowlist_match() {
            let p = class_policy(3, &[error_class::WALL_TIME_EXCEEDED]);
            assert!(should_retry_class(
                Some(&p),
                error_class::WALL_TIME_EXCEEDED
            ));
            assert!(!should_retry_class(Some(&p), error_class::RUNTIME_ERROR));
        }

        #[test]
        fn should_retry_class_legacy_on_transient_true() {
            let p = policy(3, true);
            // Empty retry_on → falls back to on_transient → matches anything.
            assert!(should_retry_class(Some(&p), error_class::RUNTIME_ERROR));
            assert!(should_retry_class(Some(&p), error_class::COMPILE_ERROR));
        }

        #[test]
        fn should_retry_class_legacy_on_transient_false() {
            let p = policy(3, false);
            assert!(!should_retry_class(
                Some(&p),
                error_class::WALL_TIME_EXCEEDED
            ));
            assert!(!should_retry_class(Some(&p), error_class::RUNTIME_ERROR));
        }

        #[test]
        fn compile_error_step_with_retry_eventually_fails() {
            // Integration-style: a workflow with a syntactically-bad
            // step + RetryPolicy { max_attempts: 3, on_transient: true }
            // exhausts all 3 attempts and surfaces a Failed result
            // with the attempt count in the error message.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("bad.ax"), "fn main( { }").unwrap();
            let mut bad = StepDef {
                kind: StepKind::Source {
                    source: "steps/bad.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry: Some(RetryPolicy {
                    max_attempts: 3,
                    on_transient: true,
                    retry_on: vec![],
                }),
                budget: None,
                required_capability_versions: Default::default(),
            };
            bad.inputs.clear();
            let def = WorkflowDef {
                schema_version: 1,
                name: "retry-fail".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([("bad".into(), bad)]),
                edges: vec![],
            };
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let result = WorkflowRunner::run(&def, &options).unwrap();
            assert_eq!(result.status, WorkflowStatus::Failed);
            let err = result.step_results["bad"].error.as_deref().unwrap();
            assert!(
                err.contains("failed after 3 attempts"),
                "expected attempt-count in error message; got: {err}"
            );
            // 0.3-S13: sequential failure path now persists the
            // accurate attempt count (was: defaulted to 1 in the
            // failure path).
            assert_eq!(
                result.step_results["bad"].attempt_count, 3,
                "sequential failure must surface actual attempt count, not default 1"
            );
        }
    }

    // ── 0.3-S2b: persistent runs + resume ──

    #[cfg(feature = "persist-sqlite")]
    mod persistent {
        use super::*;
        use crate::persistence::{
            RunCheckpointStore, RunStatus as PersistRunStatus, StepStatus as PersistStepStatus,
        };

        fn workflow_dir_with_steps(
            step_sources: &[(&str, &str)],
        ) -> (WorkflowDef, tempfile::TempDir) {
            let (def, dir) = make_workflow_with_steps(step_sources);
            // run_persistent + resume require workflow.json on disk.
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            (def, dir)
        }

        #[test]
        fn run_persistent_writes_run_and_step_rows() {
            let (def, wf_dir) = workflow_dir_with_steps(&[
                ("step1", "fn main() -> Int { 1 }"),
                ("step2", "fn main() -> Int { 2 }"),
            ]);
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };

            let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            assert_eq!(result.status, WorkflowStatus::Completed);

            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let rec = store.get_run_record(&result.run_id).unwrap().unwrap();
            assert_eq!(rec.terminal_status, Some(PersistRunStatus::Completed));
            let cps = store.list_step_checkpoints(&result.run_id).unwrap();
            assert_eq!(cps.len(), 2);
            assert!(cps.iter().all(|c| c.status == PersistStepStatus::Completed));
            assert!(cps.iter().all(|c| c.output_json.is_some()));
        }

        #[test]
        fn run_persistent_run_id_is_deterministic() {
            // Two runs against the same workflow + inputs in the same DB
            // yield distinct run_ids (counter increments). The first one
            // matches derive_run_id(workflow_hash, inputs_hash, 0).
            let (def, wf_dir) = workflow_dir_with_steps(&[("only", "fn main() -> Int { 7 }")]);
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let r1 = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            let r2 = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            assert_ne!(r1.run_id, r2.run_id);
            let workflow_hash = WorkflowRunner::workflow_hash_from_def(&def);
            let inputs_hash = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(b"{}");
                format!("{:x}", h.finalize())
            };
            assert_eq!(r1.run_id, derive_run_id(&workflow_hash, &inputs_hash, 0));
            assert_eq!(r2.run_id, derive_run_id(&workflow_hash, &inputs_hash, 1));
        }

        #[test]
        fn resume_unknown_run_id_returns_typed_error() {
            let data_dir = tempfile::tempdir().unwrap();
            // Initialize the store so the file exists, but don't insert a run.
            let _ = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let err = WorkflowRunner::resume(
                "ffffffffffffffff",
                data_dir.path(),
                &ResumeOptions::default(),
            )
            .expect_err("missing run must error");
            match err {
                WorkflowRunError::RunNotFound(rid) => {
                    assert_eq!(rid, "ffffffffffffffff")
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }

        #[test]
        fn resume_completed_run_returns_terminal_result_without_re_executing() {
            // Run to completion, then resume — must NOT re-execute. Test
            // by writing a sentinel via std::sync::atomic in step output:
            // since steps are pure, easier proof: resume returns the
            // Completed status and same step results from the persisted
            // checkpoints.
            let (def, wf_dir) = workflow_dir_with_steps(&[("only", "fn main() -> Int { 99 }")]);
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            assert_eq!(result.status, WorkflowStatus::Completed);

            let resumed = WorkflowRunner::resume(
                &result.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);
            assert_eq!(resumed.run_id, result.run_id);
            assert_eq!(resumed.step_results.len(), 1);
            assert_eq!(resumed.step_results["only"].status, StepStatus::Completed);
        }

        #[test]
        fn resume_workflow_hash_mismatch_refuses() {
            let (def, wf_dir) = workflow_dir_with_steps(&[("step1", "fn main() -> Int { 1 }")]);
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };

            // Insert a run row with a deliberately-altered workflow_hash
            // so we hit the mismatch branch in resume() against the real
            // on-disk workflow.json.
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let metadata = serde_json::json!({
                "workflow_dir": wf_dir.path().to_string_lossy(),
                "inputs_hash": "deadbeef",
                "boruna_version": "test",
            })
            .to_string();
            let run_id = "00000000deadbeef".to_string();
            store
                .insert_run(&crate::persistence::RunRow {
                    run_id: run_id.clone(),
                    workflow_name: def.name.clone(),
                    workflow_hash: "tampered".into(),
                    status: PersistRunStatus::Paused,
                    started_at_ms: 0,
                    updated_at_ms: 0,
                    policy_json: serde_json::to_string(&options.policy).unwrap(),
                    metadata_json: metadata,
                })
                .unwrap();

            let err = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .expect_err("hash mismatch must refuse");
            match err {
                WorkflowRunError::WorkflowHashMismatch {
                    run_id: rid,
                    expected,
                    actual,
                } => {
                    assert_eq!(rid, run_id);
                    assert_eq!(expected, "tampered");
                    assert_ne!(actual, "tampered");
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }

        #[test]
        fn resume_after_simulated_crash_reuses_completed_steps() {
            // The headline test: simulate a crash after step 1 by manually
            // populating the store with a completed step 1 + a Running run
            // row, then call resume. step 2 + 3 must execute; step 1 must
            // NOT re-execute. We prove "step 1 didn't re-execute" by
            // checking that step 1's output_hash in the resumed result
            // matches the value we manually persisted (i.e. resume
            // restored from the store, didn't recompute it).
            let (def, wf_dir) = workflow_dir_with_steps(&[
                ("step1", "fn main() -> Int { 100 }"),
                ("step2", "fn main() -> Int { 200 }"),
                ("step3", "fn main() -> Int { 300 }"),
            ]);
            let data_dir = tempfile::tempdir().unwrap();

            // Manually compute the expected output for step 1 and write
            // a "stale" sentinel hash in its place. If resume re-runs
            // step 1, the recomputed hash would differ from this sentinel
            // (it'd match the real value 100). If resume restores from
            // the store, the sentinel persists.
            let workflow_hash = WorkflowRunner::workflow_hash_from_def(&def);
            let inputs_hash = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(b"{}");
                format!("{:x}", h.finalize())
            };
            let run_id = derive_run_id(&workflow_hash, &inputs_hash, 0);

            let metadata = serde_json::json!({
                "workflow_dir": wf_dir.path().to_string_lossy(),
                "inputs_hash": inputs_hash,
                "boruna_version": "test",
            })
            .to_string();

            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            store
                .insert_run(&crate::persistence::RunRow {
                    run_id: run_id.clone(),
                    workflow_name: def.name.clone(),
                    workflow_hash: workflow_hash.clone(),
                    status: PersistRunStatus::Running,
                    started_at_ms: 0,
                    updated_at_ms: 0,
                    policy_json: serde_json::to_string(&Some(Policy::allow_all())).unwrap(),
                    metadata_json: metadata,
                })
                .unwrap();

            // Persist step1 as Completed with the actual VM output (Int(100))
            // so the resume's downstream input resolution still has it.
            let step1_value = boruna_bytecode::Value::Int(100);
            let step1_output_json = serde_json::to_string(&step1_value).unwrap();
            let step1_hash = crate::workflow::DataStore::hash_value(&step1_value);
            store
                .upsert_step_checkpoint(&crate::persistence::StepCheckpoint {
                    run_id: run_id.clone(),
                    step_id: "step1".into(),
                    status: PersistStepStatus::Completed,
                    output_json: Some(step1_output_json),
                    output_hash: Some(step1_hash.clone()),
                    started_at_ms: Some(1),
                    ended_at_ms: Some(2),
                    error_msg: None,
                    attempt_count: 1,
                    worker_id: None,
                    lease_expires_at_ms: None,
                    claim_id: 0,
                    output_blob_ref: None,
                })
                .unwrap();

            // Resume should pick up step2 + step3 and skip step1.
            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

            assert_eq!(resumed.status, WorkflowStatus::Completed);
            assert_eq!(resumed.run_id, run_id);
            assert_eq!(resumed.step_results.len(), 3);
            assert_eq!(resumed.step_results["step1"].status, StepStatus::Completed);
            // Hash of step1 in the resumed result equals the hash we
            // persisted in the store — proves step1 was restored, not
            // re-executed.
            assert_eq!(
                resumed.step_results["step1"].output_hash.as_deref(),
                Some(step1_hash.as_str())
            );
            assert_eq!(resumed.step_results["step2"].status, StepStatus::Completed);
            assert_eq!(resumed.step_results["step3"].status, StepStatus::Completed);

            // Final run row is now Completed.
            let rec = store.get_run_record(&run_id).unwrap().unwrap();
            assert_eq!(rec.terminal_status, Some(PersistRunStatus::Completed));
        }

        #[test]
        fn resume_with_running_step_re_executes_it() {
            // Crash-during-step semantics: a step left at status=Running
            // by a crash MUST be re-executed, not skipped. The runner
            // can't trust any partial output — only `Completed` is safe
            // to skip.
            let (def, wf_dir) = workflow_dir_with_steps(&[
                ("step1", "fn main() -> Int { 100 }"),
                ("step2", "fn main() -> Int { 200 }"),
            ]);
            let data_dir = tempfile::tempdir().unwrap();
            let workflow_hash = WorkflowRunner::workflow_hash_from_def(&def);
            let inputs_hash = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(b"{}");
                format!("{:x}", h.finalize())
            };
            let run_id = derive_run_id(&workflow_hash, &inputs_hash, 0);
            let metadata = serde_json::json!({
                "workflow_dir": wf_dir.path().to_string_lossy(),
                "inputs_hash": inputs_hash,
                "boruna_version": "test",
            })
            .to_string();
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            store
                .insert_run(&crate::persistence::RunRow {
                    run_id: run_id.clone(),
                    workflow_name: def.name.clone(),
                    workflow_hash: workflow_hash.clone(),
                    status: PersistRunStatus::Running,
                    started_at_ms: 0,
                    updated_at_ms: 0,
                    policy_json: serde_json::to_string(&Some(Policy::allow_all())).unwrap(),
                    metadata_json: metadata,
                })
                .unwrap();
            // step1 left in status=Running with a STALE output_hash. The
            // runner must ignore it and re-execute.
            store
                .upsert_step_checkpoint(&crate::persistence::StepCheckpoint {
                    run_id: run_id.clone(),
                    step_id: "step1".into(),
                    status: PersistStepStatus::Running,
                    output_json: Some("\"STALE\"".into()),
                    output_hash: Some("stale-hash".into()),
                    started_at_ms: Some(1),
                    ended_at_ms: None,
                    error_msg: None,
                    attempt_count: 1,
                    worker_id: None,
                    lease_expires_at_ms: None,
                    claim_id: 0,
                    output_blob_ref: None,
                })
                .unwrap();

            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

            // step1 was re-executed: its output_hash is the real one for
            // Int(100), NOT "stale-hash".
            let real_hash =
                crate::workflow::DataStore::hash_value(&boruna_bytecode::Value::Int(100));
            assert_eq!(
                resumed.step_results["step1"].output_hash.as_deref(),
                Some(real_hash.as_str())
            );
            assert_ne!(
                resumed.step_results["step1"].output_hash.as_deref(),
                Some("stale-hash")
            );
        }

        #[test]
        fn resume_with_failed_step_in_non_terminal_run_halts_without_re_executing() {
            // H1 regression (review-driven 0.3-S2b): when the original
            // run crashed AFTER persisting a step's failure but BEFORE
            // its trailing terminal-status update, the run row sits in
            // `Running`/`Paused` with a `Failed` step checkpoint.
            // Resuming MUST honor the persisted failure as terminal —
            // not re-execute the failed step or any downstream step.
            // Without the halt sentinel introduced in this fix, the
            // resume loop would silently progress past the failure and
            // potentially flip the run from Failed to Completed.
            let (def, wf_dir) = workflow_dir_with_steps(&[
                ("step1", "fn main() -> Int { 1 }"),
                ("step2", "fn main() -> Int { 2 }"),
            ]);
            let data_dir = tempfile::tempdir().unwrap();
            let workflow_hash = WorkflowRunner::workflow_hash_from_def(&def);
            let inputs_hash = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(b"{}");
                format!("{:x}", h.finalize())
            };
            let run_id = derive_run_id(&workflow_hash, &inputs_hash, 0);
            let metadata = serde_json::json!({
                "workflow_dir": wf_dir.path().to_string_lossy(),
                "inputs_hash": inputs_hash,
                "boruna_version": "test",
            })
            .to_string();
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            store
                .insert_run(&crate::persistence::RunRow {
                    run_id: run_id.clone(),
                    workflow_name: def.name.clone(),
                    workflow_hash,
                    status: PersistRunStatus::Running, // non-terminal
                    started_at_ms: 0,
                    updated_at_ms: 0,
                    policy_json: serde_json::to_string(&Some(Policy::allow_all())).unwrap(),
                    metadata_json: metadata,
                })
                .unwrap();
            // step1 persisted as Failed.
            store
                .upsert_step_checkpoint(&crate::persistence::StepCheckpoint {
                    run_id: run_id.clone(),
                    step_id: "step1".into(),
                    status: PersistStepStatus::Failed,
                    output_json: None,
                    output_hash: None,
                    started_at_ms: Some(1),
                    ended_at_ms: Some(2),
                    error_msg: Some("simulated step1 failure".into()),
                    attempt_count: 1,
                    worker_id: None,
                    lease_expires_at_ms: None,
                    claim_id: 0,
                    output_blob_ref: None,
                })
                .unwrap();

            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

            // Run halts at step1 with the persisted error.
            assert_eq!(resumed.status, WorkflowStatus::Failed);
            assert_eq!(resumed.step_results["step1"].status, StepStatus::Failed);
            assert_eq!(
                resumed.step_results["step1"].error.as_deref(),
                Some("simulated step1 failure"),
                "persisted error must propagate"
            );
            // step2 was NOT re-executed — must be absent from results.
            assert!(
                !resumed.step_results.contains_key("step2"),
                "downstream step must not run after a halted-failed step"
            );
            // Run row is now Failed.
            let rec = store.get_run_record(&run_id).unwrap().unwrap();
            assert_eq!(rec.terminal_status, Some(PersistRunStatus::Failed));
        }

        #[test]
        fn resume_with_policy_none_uses_persisted_policy() {
            // H2 regression (review-driven 0.3-S2b): the default CLI
            // resume invocation passes `--policy` as None. Without this
            // fix, the resume's policy fell through to
            // `Policy::deny_all()` via build_step_policy, breaking any
            // step with a declared capability. The fix: when
            // ResumeOptions::policy is None, deserialize the persisted
            // policy_json from the run row.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            // A step that requires net.fetch capability — only succeeds
            // under allow_all (or an explicit allow rule), fails under
            // deny_all.
            std::fs::write(
                steps_dir.join("fetch.ax"),
                "fn fetch(url: String) -> String !{net.fetch} { url }\nfn main() -> Int { 7 }",
            )
            .unwrap();
            let def = WorkflowDef {
                schema_version: 1,
                name: "policy-resume".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([(
                    "fetch".into(),
                    StepDef {
                        kind: StepKind::Source {
                            source: "steps/fetch.ax".into(),
                        },
                        capabilities: vec!["net.fetch".into()],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec![],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                        required_capability_versions: Default::default(),
                    },
                )]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();

            // Run originally with allow_all so step succeeds.
            let result = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            assert_eq!(result.status, WorkflowStatus::Completed);

            // Resume with policy=None — must use the persisted allow_all
            // policy and return the same Completed result, NOT silently
            // run under deny_all.
            let resumed = WorkflowRunner::resume(
                &result.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: None,
                    workflow_dir_override: Some(dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);
        }

        #[test]
        fn concurrent_run_persistent_inserts_produce_distinct_run_ids() {
            // C2 / H5 regression (review-driven 0.3-S2b): the counter +
            // INSERT race. Before the fix, `unchecked_transaction()`
            // opened a DEFERRED transaction; two writers could both read
            // counter=N and race the INSERT, with the loser producing a
            // raw UNIQUE constraint error rather than a busy retry. The
            // fix uses explicit `BEGIN IMMEDIATE` so the second writer
            // either sees counter=N+1 or hits BUSY (which `with_busy_retry`
            // handles).
            //
            // We exercise this by spawning N threads that share one
            // database file and have each call `insert_run_with_derived_id`
            // with the same workflow_hash. Every thread must succeed and
            // produce a distinct run_id — no UNIQUE collision.
            use std::sync::Arc;
            use std::thread;
            let data_dir = tempfile::tempdir().unwrap();
            let db_path = data_dir.path().join("runs.db");
            // Initialize schema once.
            let _ = RunCheckpointStore::open(&db_path).unwrap();
            let db_path = Arc::new(db_path);
            let mut handles = Vec::new();
            const N: usize = 8;
            for _ in 0..N {
                let path = Arc::clone(&db_path);
                handles.push(thread::spawn(move || {
                    let store = RunCheckpointStore::open(&path).unwrap();
                    store
                        .insert_run_with_derived_id(
                            "wf",
                            "shared-workflow-hash",
                            "shared-inputs-hash",
                            "{}",
                            "{}",
                            1_700_000_000_000,
                        )
                        .expect("must not collide on UNIQUE")
                }));
            }
            let run_ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            // All distinct.
            let mut sorted = run_ids.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                N,
                "all {N} concurrent inserts must produce distinct run_ids; got {run_ids:?}"
            );
            // And every run actually landed in the DB.
            let store = RunCheckpointStore::open(&db_path).unwrap();
            assert_eq!(
                store
                    .count_runs_for_workflow("shared-workflow-hash")
                    .unwrap(),
                N as i64
            );
        }

        #[test]
        fn run_persistent_rejects_root_data_dir() {
            // Defense against accidentally writing to /. The check
            // happens before the run starts, so even an empty workflow
            // dir is fine.
            let (def, wf_dir) = workflow_dir_with_steps(&[("step1", "fn main() -> Int { 1 }")]);
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let err = WorkflowRunner::run_persistent(&def, &options, Path::new("/"))
                .expect_err("must reject /");
            match err {
                WorkflowRunError::Internal(msg) => {
                    assert!(msg.contains("system root"), "msg: {msg}")
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }

        // ── 0.3-S4: concurrent execution ──

        fn fan_out_workflow() -> (WorkflowDef, tempfile::TempDir) {
            // ingest → {classify, extract, summarize} → merge.
            // All produce deterministic Int outputs (steps are pure).
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("ingest.ax"), "fn main() -> Int { 1 }").unwrap();
            std::fs::write(steps_dir.join("classify.ax"), "fn main() -> Int { 10 }").unwrap();
            std::fs::write(steps_dir.join("extract.ax"), "fn main() -> Int { 20 }").unwrap();
            std::fs::write(steps_dir.join("summarize.ax"), "fn main() -> Int { 30 }").unwrap();
            std::fs::write(steps_dir.join("merge.ax"), "fn main() -> Int { 100 }").unwrap();

            let mk = |name: &str, deps: Vec<String>| StepDef {
                kind: StepKind::Source {
                    source: format!("steps/{name}.ax"),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: deps,
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            let def = WorkflowDef {
                schema_version: 1,
                name: "fan-out".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    ("ingest".into(), mk("ingest", vec![])),
                    ("classify".into(), mk("classify", vec!["ingest".into()])),
                    ("extract".into(), mk("extract", vec!["ingest".into()])),
                    ("summarize".into(), mk("summarize", vec!["ingest".into()])),
                    (
                        "merge".into(),
                        mk(
                            "merge",
                            vec!["classify".into(), "extract".into(), "summarize".into()],
                        ),
                    ),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            (def, dir)
        }

        #[test]
        fn concurrency_n_produces_identical_output_hashes_to_concurrency_1() {
            // The headline determinism contract for 0.3-S4: running a
            // workflow at concurrency=4 produces per-step output_hash
            // values bit-identical to a sequential run. Wall-clock
            // fields (started_at_ms, ended_at_ms, total_duration_ms)
            // legitimately vary; the replay-verified subset must not.
            let (def, wf_dir) = fan_out_workflow();
            let make_options = |c| RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: c,
                submit_only: false,
            };
            let dir1 = tempfile::tempdir().unwrap();
            let r1 = WorkflowRunner::run_persistent(&def, &make_options(1), dir1.path()).unwrap();
            let dir4 = tempfile::tempdir().unwrap();
            let r4 = WorkflowRunner::run_persistent(&def, &make_options(4), dir4.path()).unwrap();
            assert_eq!(r1.status, WorkflowStatus::Completed);
            assert_eq!(r4.status, WorkflowStatus::Completed);
            // Same set of steps completed.
            let ids1: BTreeSet<&str> = r1.step_results.keys().map(|k| k.as_str()).collect();
            let ids4: BTreeSet<&str> = r4.step_results.keys().map(|k| k.as_str()).collect();
            assert_eq!(ids1, ids4);
            // Per-step output_hash bit-identical.
            for step_id in ids1 {
                let h1 = r1.step_results[step_id].output_hash.as_deref();
                let h4 = r4.step_results[step_id].output_hash.as_deref();
                assert_eq!(
                    h1, h4,
                    "step '{step_id}' hash differs between concurrency=1 and concurrency=4: {h1:?} vs {h4:?}"
                );
            }
        }

        #[test]
        fn concurrent_run_persists_all_step_checkpoints() {
            let (def, wf_dir) = fan_out_workflow();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 4,
                submit_only: false,
            };
            let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            assert_eq!(result.status, WorkflowStatus::Completed);
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let cps = store.list_step_checkpoints(&result.run_id).unwrap();
            assert_eq!(cps.len(), 5);
            assert!(cps.iter().all(|c| c.status == PersistStepStatus::Completed));
            assert!(cps.iter().all(|c| c.output_json.is_some()));
            assert!(cps.iter().all(|c| c.output_hash.is_some()));
        }

        #[test]
        fn concurrent_run_with_failure_halts_and_other_in_flight_complete() {
            // Build a wave where one step fails (compile error) and
            // others succeed. The wave should fully complete (other
            // workers join), but the run should be Failed afterwards.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("ok1.ax"), "fn main() -> Int { 1 }").unwrap();
            std::fs::write(steps_dir.join("ok2.ax"), "fn main() -> Int { 2 }").unwrap();
            std::fs::write(steps_dir.join("bad.ax"), "fn main( { }").unwrap(); // syntax err

            let mk = |name: &str| StepDef {
                kind: StepKind::Source {
                    source: format!("steps/{name}.ax"),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            let def = WorkflowDef {
                schema_version: 1,
                name: "fail-wave".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    ("ok1".into(), mk("ok1")),
                    ("ok2".into(), mk("ok2")),
                    ("bad".into(), mk("bad")),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();

            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 4,
                submit_only: false,
            };
            let result = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            assert_eq!(result.status, WorkflowStatus::Failed);
            // ok1 + ok2 + bad all started in the same wave; all join.
            assert_eq!(result.step_results["bad"].status, StepStatus::Failed);
            assert_eq!(result.step_results["ok1"].status, StepStatus::Completed);
            assert_eq!(result.step_results["ok2"].status, StepStatus::Completed);
        }

        #[test]
        fn concurrent_resume_honors_already_completed() {
            // A run completed at concurrency=1 has all step
            // checkpoints persisted as Completed. Resuming at
            // concurrency=4 should be a no-op terminal-state path.
            let (def, wf_dir) = fan_out_workflow();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let r1 = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            assert_eq!(r1.status, WorkflowStatus::Completed);
            // Resume with concurrency=4 — terminal Completed is
            // returned via reconstruct_terminal_result, which doesn't
            // re-execute regardless of concurrency.
            let resumed = WorkflowRunner::resume(
                &r1.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    concurrency: 4,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);
            assert_eq!(resumed.step_results.len(), 5);
        }

        #[test]
        fn concurrent_input_failure_does_not_leave_siblings_running() {
            // 0.3-S4 review-driven regression #2: a chunk with one
            // input-resolution failure used to mark earlier siblings
            // Running before discovering the failure, then halt — the
            // siblings stayed Running on disk forever and would be
            // re-executed by the next resume even though sequential
            // semantics never started them.
            //
            // Construct a 2-step layer where one step references a
            // non-existent upstream output. Both steps share the same
            // dependency-free level (level 0). At concurrency=2, the
            // pre-validation pass MUST detect the bad input before
            // marking anything Running. After the run halts, the
            // bad-input step is Failed and the good step has NO
            // checkpoint at all.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("good.ax"), "fn main() -> Int { 1 }").unwrap();
            std::fs::write(steps_dir.join("bad_input.ax"), "fn main() -> Int { 2 }").unwrap();

            // bad_input references an output from a non-existent step.
            let mut bad = StepDef {
                kind: StepKind::Source {
                    source: "steps/bad_input.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            bad.inputs.insert("missing".into(), "ghost.result".into());
            // We need to bypass workflow validation (which would reject
            // the unknown step ref) — but the unknown-input check uses
            // step_ids; if we name the ghost step but don't include
            // it... actually the validator catches unknown_input.
            // Instead, construct a workflow where the bad step
            // references a step that EXISTS but produces no output
            // accessible via the requested name. The data_store's
            // resolve_step_inputs will fail at runtime even though
            // validation passes.
            //
            // Simpler: make bad_input depend on good (so good runs in
            // level 0), and bad's input references good but with a
            // wrong output name. Validation only checks step_ids, not
            // output names, so this slips through to runtime.
            bad.depends_on = vec!["good".into()];
            bad.inputs.clear();
            bad.inputs
                .insert("data".into(), "good.nonexistent_output".into());

            let good = StepDef {
                kind: StepKind::Source {
                    source: "steps/good.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            // Add a third step at level 1, sibling of bad_input, that
            // shares the same input-failure pattern OR depends on
            // good. Use a clean parallel sibling at level 1 whose only
            // job is to be in the same chunk as bad_input.
            std::fs::write(steps_dir.join("sibling.ax"), "fn main() -> Int { 99 }").unwrap();
            let mut sibling = StepDef {
                kind: StepKind::Source {
                    source: "steps/sibling.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec!["good".into()],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            sibling.inputs.clear();

            let def = WorkflowDef {
                schema_version: 1,
                name: "input-fail".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    ("good".into(), good),
                    ("bad_input".into(), bad),
                    ("sibling".into(), sibling),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();

            let result = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 4,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();

            assert_eq!(result.status, WorkflowStatus::Failed);
            // good ran (level 0). bad_input failed input resolution
            // at level 1 (its `good.nonexistent_output` reference
            // doesn't resolve). sibling — also at level 1 — must NOT
            // be left Running on disk. Pre-validation pass means it
            // either ran (if validation passed) or has no checkpoint
            // at all.
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let cps = store.list_step_checkpoints(&result.run_id).unwrap();
            // Every persisted checkpoint must be in a TERMINAL state.
            // Specifically, no `running` left behind.
            for cp in &cps {
                assert_ne!(
                    cp.status,
                    PersistStepStatus::Running,
                    "step '{}' left Running on disk after run halted",
                    cp.step_id
                );
            }
            // bad_input is recorded as Failed.
            let bad_cp = cps
                .iter()
                .find(|c| c.step_id == "bad_input")
                .expect("bad_input checkpoint recorded");
            assert_eq!(bad_cp.status, PersistStepStatus::Failed);
        }

        #[test]
        fn sequential_failure_persists_actual_attempt_count() {
            // 0.3-S13 regression: a step with retry=3 that exhausts
            // all 3 attempts now persists attempt_count=3 in the
            // step_checkpoints row (was: defaulted to 1 in the
            // sequential failure path before 0.3-S13).
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("bad.ax"), "fn main( { }").unwrap();
            let bad = StepDef {
                kind: StepKind::Source {
                    source: "steps/bad.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry: Some(RetryPolicy {
                    max_attempts: 3,
                    on_transient: true,
                    retry_on: vec![],
                }),
                budget: None,
                required_capability_versions: Default::default(),
            };
            let def = WorkflowDef {
                schema_version: 1,
                name: "retry-fail-persist".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([("bad".into(), bad)]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            let result = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            assert_eq!(result.status, WorkflowStatus::Failed);
            assert_eq!(result.step_results["bad"].attempt_count, 3);
            // Verify the count is in the persisted SQL row, not just
            // the in-memory StepResult.
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let cps = store.list_step_checkpoints(&result.run_id).unwrap();
            let bad_cp = cps.iter().find(|c| c.step_id == "bad").unwrap();
            assert_eq!(
                bad_cp.attempt_count, 3,
                "persisted attempt_count must reflect retries"
            );
        }
    }

    // ── 0.3-S2c: approval-gate completion ──

    #[cfg(feature = "persist-sqlite")]
    mod approval_gate {
        use super::*;
        use crate::persistence::{RunCheckpointStore, RunStatus as PersistRunStatus};

        pub(super) fn workflow_with_approval_gate() -> (WorkflowDef, tempfile::TempDir) {
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("analyze.ax"), "fn main() -> Int { 42 }").unwrap();
            std::fs::write(steps_dir.join("publish.ax"), "fn main() -> Int { 7 }").unwrap();
            let def = WorkflowDef {
                schema_version: 1,
                name: "approval-test".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    (
                        "analyze".into(),
                        StepDef {
                            kind: StepKind::Source {
                                source: "steps/analyze.ax".into(),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec![],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "human_review".into(),
                        StepDef {
                            kind: StepKind::ApprovalGate {
                                required_role: "reviewer".into(),
                                condition: None,
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["analyze".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "publish".into(),
                        StepDef {
                            kind: StepKind::Source {
                                source: "steps/publish.ax".into(),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["human_review".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            (def, dir)
        }

        fn paused_run(data_dir: &Path, wf_dir: &Path, def: &WorkflowDef) -> String {
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(def, &options, data_dir).unwrap();
            assert_eq!(r.status, WorkflowStatus::Paused);
            r.run_id
        }

        // ── record_approval_decision validation ──

        #[test]
        fn approve_unknown_run_id_returns_run_not_found() {
            let data_dir = tempfile::tempdir().unwrap();
            let _ = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let err = record_approval_decision(
                data_dir.path(),
                "ffffffffffffffff",
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .expect_err("missing run must error");
            match err {
                WorkflowRunError::RunNotFound(rid) => assert_eq!(rid, "ffffffffffffffff"),
                other => panic!("wrong variant: {other:?}"),
            }
        }

        #[test]
        fn approve_unknown_step_returns_step_not_found() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let err = record_approval_decision(
                data_dir.path(),
                &run_id,
                "no-such-step",
                ApprovalKind::Approved,
                None,
            )
            .expect_err("missing step must error");
            match err {
                WorkflowRunError::StepNotFound {
                    run_id: rid,
                    step_id,
                } => {
                    assert_eq!(rid, run_id);
                    assert_eq!(step_id, "no-such-step");
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }

        #[test]
        fn approve_non_gate_step_returns_not_an_approval_gate_step() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            // 'analyze' is a Source step, not an ApprovalGate.
            let err = record_approval_decision(
                data_dir.path(),
                &run_id,
                "analyze",
                ApprovalKind::Approved,
                None,
            )
            .expect_err("non-gate step must error");
            match err {
                WorkflowRunError::NotAnApprovalGateStep { step_id, .. } => {
                    assert_eq!(step_id, "analyze")
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }

        #[test]
        fn approve_records_decision_with_timestamp() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            record_approval_decision(
                data_dir.path(),
                &run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();
            // Re-open store and inspect metadata.
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let meta_json = store.get_run_metadata(&run_id).unwrap().unwrap();
            let meta: PersistedRunMetadata = serde_json::from_str(&meta_json).unwrap();
            let decision = meta
                .approvals
                .get("human_review")
                .expect("approval recorded");
            assert!(matches!(decision.decision, ApprovalKind::Approved));
            assert!(decision.decided_at_ms > 0);
            assert!(decision.reason.is_none());
        }

        #[test]
        fn reject_carries_reason() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            record_approval_decision(
                data_dir.path(),
                &run_id,
                "human_review",
                ApprovalKind::Rejected,
                Some("compliance check failed".into()),
            )
            .unwrap();
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let meta_json = store.get_run_metadata(&run_id).unwrap().unwrap();
            let meta: PersistedRunMetadata = serde_json::from_str(&meta_json).unwrap();
            let decision = meta.approvals.get("human_review").unwrap();
            assert!(matches!(decision.decision, ApprovalKind::Rejected));
            assert_eq!(decision.reason.as_deref(), Some("compliance check failed"));
        }

        #[test]
        fn approve_already_decided_returns_step_already_decided() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            // First approval succeeds.
            record_approval_decision(
                data_dir.path(),
                &run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();
            // Second one (approve OR reject) refuses.
            let err = record_approval_decision(
                data_dir.path(),
                &run_id,
                "human_review",
                ApprovalKind::Rejected,
                Some("change of mind".into()),
            )
            .expect_err("re-decision must error");
            match err {
                WorkflowRunError::StepAlreadyDecided {
                    step_id,
                    prior_decision,
                    ..
                } => {
                    assert_eq!(step_id, "human_review");
                    assert_eq!(prior_decision, "approved");
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }

        #[test]
        fn approve_terminal_run_returns_run_not_resumable() {
            // Set up a Completed run, then try to approve any step on
            // it. Must refuse with RunNotResumable.
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            // Approve + resume to drive the run to Completed.
            record_approval_decision(
                data_dir.path(),
                &run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();
            let _ = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            // Now try to record another decision (against a different
            // approval — but there isn't one, so create a contrived case
            // via direct metadata clear). Simpler: just hit the terminal
            // check by attempting a fresh approve on the same step.
            let err = record_approval_decision(
                data_dir.path(),
                &run_id,
                "human_review",
                ApprovalKind::Rejected,
                None,
            )
            .expect_err("approve on Completed run must error");
            match err {
                WorkflowRunError::RunNotResumable {
                    terminal_status, ..
                } => {
                    assert_eq!(terminal_status, "completed")
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }

        // ── resume honoring the sentinel ──

        #[test]
        fn resume_with_approved_sentinel_completes_workflow() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            record_approval_decision(
                data_dir.path(),
                &run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();
            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);
            assert_eq!(
                resumed.step_results["human_review"].status,
                StepStatus::Completed
            );
            assert_eq!(
                resumed.step_results["publish"].status,
                StepStatus::Completed,
                "downstream step must execute past the approved gate"
            );
            // Persisted gate checkpoint is now Completed.
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let cps = store.list_step_checkpoints(&run_id).unwrap();
            let gate = cps.iter().find(|c| c.step_id == "human_review").unwrap();
            assert_eq!(gate.status, crate::persistence::StepStatus::Completed);
            assert!(
                gate.output_json.is_some(),
                "approved gate gets synthetic output"
            );
        }

        #[test]
        fn resume_with_rejected_sentinel_halts_failed() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            record_approval_decision(
                data_dir.path(),
                &run_id,
                "human_review",
                ApprovalKind::Rejected,
                Some("policy violation".into()),
            )
            .unwrap();
            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Failed);
            assert_eq!(
                resumed.step_results["human_review"].status,
                StepStatus::Failed
            );
            assert_eq!(
                resumed.step_results["human_review"].error.as_deref(),
                Some("policy violation")
            );
            assert!(
                !resumed.step_results.contains_key("publish"),
                "downstream of rejected gate must not run"
            );
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let rec = store.get_run_record(&run_id).unwrap().unwrap();
            assert_eq!(rec.terminal_status, Some(PersistRunStatus::Failed));
        }

        #[test]
        fn resume_without_sentinel_re_pauses() {
            // Unchanged 0.3-S2b behavior: no sentinel ⇒ re-pause.
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Paused);
            assert_eq!(
                resumed.step_results["human_review"].status,
                StepStatus::AwaitingApproval
            );
            assert!(!resumed.step_results.contains_key("publish"));
        }

        #[test]
        fn list_runs_unfiltered_returns_all() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let _ = paused_run(data_dir.path(), wf_dir.path(), &def);
            let runs = list_runs(data_dir.path(), None).unwrap();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].workflow_name, "approval-test");
            assert_eq!(runs[0].status, PersistRunStatus::Paused);
        }

        #[test]
        fn list_runs_filtered_by_status() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let _ = paused_run(data_dir.path(), wf_dir.path(), &def);
            // Filter for running — should be empty.
            assert!(list_runs(data_dir.path(), Some(PersistRunStatus::Running))
                .unwrap()
                .is_empty());
            // Filter for paused — should match our one run.
            assert_eq!(
                list_runs(data_dir.path(), Some(PersistRunStatus::Paused))
                    .unwrap()
                    .len(),
                1
            );
        }

        #[test]
        fn metadata_back_compat_with_no_approvals_field() {
            // 0.3-S2b databases have no `approvals` key in metadata_json.
            // Verify the runner still parses and resume works.
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            // Manually rewrite metadata_json to a 0.3-S2b shape (no
            // approvals key). serde's default makes it parse as empty.
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let legacy_metadata = serde_json::json!({
                "workflow_dir": wf_dir.path().to_string_lossy(),
                "inputs_hash": "deadbeef",
                "boruna_version": "0.2.0",
            })
            .to_string();
            store
                .update_run_metadata(&run_id, &legacy_metadata, 0)
                .unwrap();
            // Resume should work (re-pause without sentinel).
            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            );
            // The hash check fires here because the legacy metadata's
            // workflow_dir matches the original; only inputs_hash
            // changed (which doesn't feed workflow_hash). Resume should
            // succeed and re-pause.
            let resumed = resumed.unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Paused);
        }

        // ── 0.3-S2c review-driven regression tests ──

        #[test]
        fn concurrent_record_approval_decision_one_succeeds_one_sees_already_decided() {
            // Reviewer #1+H1 regression: prior implementation's
            // read+validate+write spanned 3 separate SQL transactions,
            // letting two operators both pass the in-memory prior-
            // decision check and silently overwrite each other. The CAS
            // loop closes the race: exactly one writer succeeds; the
            // other re-reads, sees the first writer's recorded decision,
            // and surfaces StepAlreadyDecided.
            //
            // This test fans out 4 concurrent threads, each attempting
            // to record a different decision (alternating approve /
            // reject). After all threads return, exactly one succeeds
            // and three see StepAlreadyDecided.
            use std::sync::Arc;
            use std::thread;
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let data_dir_path = Arc::new(data_dir.path().to_path_buf());
            let run_id_arc = Arc::new(run_id);
            let mut handles = Vec::new();
            for i in 0..4 {
                let dp = Arc::clone(&data_dir_path);
                let rid = Arc::clone(&run_id_arc);
                let decision = if i % 2 == 0 {
                    ApprovalKind::Approved
                } else {
                    ApprovalKind::Rejected
                };
                handles.push(thread::spawn(move || {
                    record_approval_decision(
                        &dp,
                        &rid,
                        "human_review",
                        decision,
                        Some(format!("from thread {i}")),
                    )
                }));
            }
            let results: Vec<Result<(), WorkflowRunError>> =
                handles.into_iter().map(|h| h.join().unwrap()).collect();
            let ok_count = results.iter().filter(|r| r.is_ok()).count();
            let already_decided = results
                .iter()
                .filter(|r| matches!(r, Err(WorkflowRunError::StepAlreadyDecided { .. })))
                .count();
            assert_eq!(ok_count, 1, "exactly one writer must win");
            assert_eq!(
                already_decided, 3,
                "all losers must see StepAlreadyDecided (no silent overwrite)"
            );
        }

        #[test]
        fn rejected_sentinel_preserves_earlier_independent_failure_as_halt_cause() {
            // Reviewer #2 regression: prior code used
            // `halt_with_failed_step = Some(...)` unconditionally,
            // overwriting an earlier independent step failure with the
            // approval-gate rejection. Now uses get_or_insert to
            // preserve the FIRST failure as the halt cause.
            //
            // We construct a 3-step workflow: step1 → gate → step3.
            // Plant a Failed checkpoint for step1 (simulating a crashed
            // run that had step1 fail) AND record a rejection sentinel
            // for the gate. On resume, step1's failure must be preserved
            // as the halt cause, not the rejection.
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            // Plant the run row + failed step1 checkpoint manually.
            let workflow_hash = WorkflowRunner::workflow_hash_from_def(&def);
            let inputs_hash = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(b"{}");
                format!("{:x}", h.finalize())
            };
            let run_id = derive_run_id(&workflow_hash, &inputs_hash, 0);
            let metadata = serde_json::json!({
                "workflow_dir": wf_dir.path().to_string_lossy(),
                "inputs_hash": inputs_hash,
                "boruna_version": "test",
                "approvals": {
                    "human_review": {
                        "decision": "rejected",
                        "decided_at_ms": 12345,
                        "reason": "operator says no"
                    }
                }
            })
            .to_string();
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            store
                .insert_run(&crate::persistence::RunRow {
                    run_id: run_id.clone(),
                    workflow_name: def.name.clone(),
                    workflow_hash,
                    status: PersistRunStatus::Running,
                    started_at_ms: 0,
                    updated_at_ms: 0,
                    policy_json: serde_json::to_string(&Some(Policy::allow_all())).unwrap(),
                    metadata_json: metadata,
                })
                .unwrap();
            // analyze step failed independently (the kind of state a
            // crashed run could leave).
            store
                .upsert_step_checkpoint(&crate::persistence::StepCheckpoint {
                    run_id: run_id.clone(),
                    step_id: "analyze".into(),
                    status: PersistStepStatus::Failed,
                    output_json: None,
                    output_hash: None,
                    started_at_ms: Some(1),
                    ended_at_ms: Some(2),
                    error_msg: Some("analyze died".into()),
                    attempt_count: 1,
                    worker_id: None,
                    lease_expires_at_ms: None,
                    claim_id: 0,
                    output_blob_ref: None,
                })
                .unwrap();
            // gate is awaiting approval (sentinel will rejection-halt).
            store
                .upsert_step_checkpoint(&crate::persistence::StepCheckpoint {
                    run_id: run_id.clone(),
                    step_id: "human_review".into(),
                    status: PersistStepStatus::AwaitingApproval,
                    output_json: None,
                    output_hash: None,
                    started_at_ms: Some(3),
                    ended_at_ms: None,
                    error_msg: None,
                    attempt_count: 1,
                    worker_id: None,
                    lease_expires_at_ms: None,
                    claim_id: 0,
                    output_blob_ref: None,
                })
                .unwrap();

            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

            assert_eq!(resumed.status, WorkflowStatus::Failed);
            // analyze is still the persisted failed step with its real error.
            assert_eq!(
                resumed.step_results["analyze"].error.as_deref(),
                Some("analyze died"),
                "earlier failure must remain the surfaced cause"
            );
            // gate's rejection IS recorded (we still process the sentinel),
            // but the halt cause is analyze, not the gate. The step_results
            // include both for operator visibility.
            assert_eq!(
                resumed.step_results["human_review"].status,
                StepStatus::Failed
            );
        }

        #[test]
        fn synthetic_approved_gate_output_hash_is_stable() {
            // Reviewer H2 regression: lock the synthetic empty-record
            // output's hash so a future change to Value::Map's
            // serialization (adding a wrapper, switching encoding)
            // surfaces immediately as a determinism regression rather
            // than silently breaking cross-machine replay of approval-
            // gate steps.
            //
            // Computed externally:
            //   serde_json::to_string(&Value::Map(BTreeMap::new())) → '{"type":"map","value":{}}'
            //   sha256 of that string is the persisted output_hash for
            //   any approved gate.
            let synthetic = boruna_bytecode::Value::Map(BTreeMap::new());
            let actual_hash = DataStore::hash_value(&synthetic);
            // Compute the expected hash inline (from the same Value
            // serialization) so the test self-anchors. If the Value's
            // serialization shape changes, the hash here will change in
            // lockstep — but we ALSO compare to a hard-coded hex string
            // captured at sprint-merge time so a serialization change
            // is impossible to miss in code review.
            let expected_inline = {
                use sha2::{Digest, Sha256};
                let json = serde_json::to_string(&synthetic).unwrap();
                let mut h = Sha256::new();
                h.update(json.as_bytes());
                format!("{:x}", h.finalize())
            };
            assert_eq!(actual_hash, expected_inline, "self-consistency");
            // Hard-coded golden — bumping this requires a deliberate
            // determinism re-baseline + cross-machine verification.
            // Computed externally at 0.3-S2c sprint-merge time:
            //   printf '{"Map":{}}' | shasum -a 256
            // (The default serde enum tag for Value::Map(empty) is
            // `{"Map":{}}` — externally-tagged JSON.)
            assert_eq!(
                actual_hash, "f4242fc8f76818ce8a46162b387ae027d3c25edfd5c265fea2d640e619bad6ed",
                "synthetic approved-gate output hash drifted — \
                 if intentional, update this golden + verify cross-machine determinism"
            );
        }

        #[test]
        fn sentinel_for_pending_step_warns_and_is_ignored() {
            // Reviewer #3 regression: prior code silently no-op'd
            // sentinels for non-AwaitingApproval steps. Now: still
            // no-op (preserving the documented "trust only awaiting"
            // semantics), but with an explicit eprintln warning so the
            // operator sees their approval isn't taking effect.
            //
            // We construct a run with a sentinel for a step whose
            // checkpoint is Pending (operator pre-approved before the
            // workflow reached the gate). Resume should re-pause at
            // the gate (because by definition the gate execution wasn't
            // sentinel-driven), and the sentinel should be ignored.
            //
            // We can't easily assert eprintln output in a unit test
            // without a test logger; the key assertion is that the
            // overall behavior matches the documented contract: pending-
            // checkpoint sentinels do NOT cause execute_steps to skip
            // the step.
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let workflow_hash = WorkflowRunner::workflow_hash_from_def(&def);
            let inputs_hash = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(b"{}");
                format!("{:x}", h.finalize())
            };
            let run_id = derive_run_id(&workflow_hash, &inputs_hash, 0);
            // Sentinel for human_review even though the workflow hasn't
            // reached the gate yet (no checkpoint at all).
            let metadata = serde_json::json!({
                "workflow_dir": wf_dir.path().to_string_lossy(),
                "inputs_hash": inputs_hash,
                "boruna_version": "test",
                "approvals": {
                    "human_review": {
                        "decision": "approved",
                        "decided_at_ms": 1,
                        "reason": null
                    }
                }
            })
            .to_string();
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            store
                .insert_run(&crate::persistence::RunRow {
                    run_id: run_id.clone(),
                    workflow_name: def.name.clone(),
                    workflow_hash,
                    status: PersistRunStatus::Running,
                    started_at_ms: 0,
                    updated_at_ms: 0,
                    policy_json: serde_json::to_string(&Some(Policy::allow_all())).unwrap(),
                    metadata_json: metadata,
                })
                .unwrap();
            // No step checkpoints — workflow hasn't started in this
            // crashed-then-resumed scenario.

            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            // Resume runs analyze, hits the gate (no sentinel applied
            // because no checkpoint pre-existed), pauses there.
            assert_eq!(resumed.status, WorkflowStatus::Paused);
            assert_eq!(
                resumed.step_results["human_review"].status,
                StepStatus::AwaitingApproval,
                "sentinel for a non-existent checkpoint must be ignored, not applied"
            );
        }
    }

    // ── 0.3-S3: workflow show ──

    #[cfg(feature = "persist-sqlite")]
    mod show {
        use super::*;
        use crate::persistence::RunStatus as PersistRunStatus;

        // Reuse the approval-gate workflow helper.
        fn workflow_with_approval_gate() -> (WorkflowDef, tempfile::TempDir) {
            super::approval_gate::workflow_with_approval_gate()
        }

        #[test]
        fn show_unknown_run_returns_not_found() {
            let data_dir = tempfile::tempdir().unwrap();
            // Initialize the store so the file exists.
            let _ = crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db"))
                .unwrap();
            let err =
                show_run(data_dir.path(), "ffffffffffffffff").expect_err("missing run must error");
            match err {
                WorkflowRunError::RunNotFound(rid) => assert_eq!(rid, "ffffffffffffffff"),
                other => panic!("wrong variant: {other:?}"),
            }
        }

        #[test]
        fn show_completed_run_carries_steps_and_no_approvals() {
            let (def, wf_dir) = make_workflow_with_steps(&[
                ("step1", "fn main() -> Int { 1 }"),
                ("step2", "fn main() -> Int { 2 }"),
            ]);
            // Persist to disk so run_persistent works.
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(wf_dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            let result = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            assert_eq!(result.status, WorkflowStatus::Completed);

            let detail = show_run(data_dir.path(), &result.run_id).unwrap();
            assert_eq!(detail.run.run_id, result.run_id);
            assert_eq!(detail.run.status, PersistRunStatus::Completed);
            assert_eq!(detail.checkpoints.len(), 2);
            assert!(detail.approvals.is_empty());
            // Steps are sorted by step_id (deterministic).
            let ids: Vec<&str> = detail
                .checkpoints
                .iter()
                .map(|c| c.step_id.as_str())
                .collect();
            assert_eq!(ids, vec!["step1", "step2"]);
            // Each completed step has an output_hash.
            for cp in &detail.checkpoints {
                assert!(
                    cp.output_hash.is_some(),
                    "step '{}' missing hash",
                    cp.step_id
                );
            }
        }

        #[test]
        fn show_paused_run_after_approve_carries_approval() {
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            // Run pauses at gate.
            let result = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            assert_eq!(result.status, WorkflowStatus::Paused);
            // Approve.
            record_approval_decision(
                data_dir.path(),
                &result.run_id,
                "human_review",
                ApprovalKind::Approved,
                Some("looks good".into()),
            )
            .unwrap();

            let detail = show_run(data_dir.path(), &result.run_id).unwrap();
            // Run is still Paused (resume hasn't happened yet).
            assert_eq!(detail.run.status, PersistRunStatus::Paused);
            // Approval is surfaced.
            assert_eq!(detail.approvals.len(), 1);
            assert_eq!(detail.approvals[0].step_id, "human_review");
            assert!(matches!(
                detail.approvals[0].decision,
                ApprovalKind::Approved
            ));
            assert_eq!(detail.approvals[0].reason.as_deref(), Some("looks good"));
            assert!(detail.approvals[0].decided_at_ms > 0);
        }

        #[test]
        fn show_handles_corrupt_metadata_json_gracefully() {
            // 0.3-S3 contract: corrupt metadata_json doesn't make show
            // fail — operators need to inspect a corrupt run to triage.
            // Run + checkpoints are still authoritative; approvals
            // comes back empty.
            let (def, wf_dir) = workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let result = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            // Corrupt the metadata directly.
            let store =
                crate::persistence::RunCheckpointStore::open(&data_dir.path().join("runs.db"))
                    .unwrap();
            store
                .update_run_metadata(&result.run_id, "{not valid json", 0)
                .unwrap();
            let detail = show_run(data_dir.path(), &result.run_id).unwrap();
            assert_eq!(detail.run.run_id, result.run_id);
            assert!(!detail.checkpoints.is_empty());
            assert!(
                detail.approvals.is_empty(),
                "corrupt metadata must yield empty approvals"
            );
            // Reviewed 0.3-S3 H5: parse failures must surface
            // programmatically, not only on stderr.
            assert!(
                detail.metadata_parse_error.is_some(),
                "corrupt metadata must surface metadata_parse_error"
            );
            // Parse error string should reference a position so
            // operators can triage. Don't lock the exact serde_json
            // wording (varies across versions).
            let err = detail.metadata_parse_error.as_deref().unwrap();
            assert!(
                !err.is_empty() && (err.contains("line") || err.contains("column")),
                "parse error should reference line/column; got: {err}"
            );
        }

        #[test]
        fn show_happy_path_metadata_parse_error_is_none() {
            let (def, wf_dir) = make_workflow_with_steps(&[("step1", "fn main() -> Int { 1 }")]);
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(wf_dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            let result = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            let detail = show_run(data_dir.path(), &result.run_id).unwrap();
            assert!(
                detail.metadata_parse_error.is_none(),
                "non-corrupt metadata must yield None"
            );
        }
    }

    // ── 0.3-S14: step_input builtin (output piping) ──

    mod step_input {
        use super::*;

        fn upstream_downstream_workflow() -> (WorkflowDef, tempfile::TempDir) {
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            // Upstream produces a string. Downstream calls
            // step_input("msg") and returns whatever it received.
            std::fs::write(
                steps_dir.join("upstream.ax"),
                "fn main() -> String { \"hello-from-upstream\" }",
            )
            .unwrap();
            std::fs::write(
                steps_dir.join("downstream.ax"),
                "fn main() -> String {\n    let received: String = step_input(\"msg\")\n    received\n}",
            )
            .unwrap();
            let upstream = StepDef {
                kind: StepKind::Source {
                    source: "steps/upstream.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            let mut downstream = StepDef {
                kind: StepKind::Source {
                    source: "steps/downstream.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec!["upstream".into()],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            downstream
                .inputs
                .insert("msg".into(), "upstream.result".into());
            let def = WorkflowDef {
                schema_version: 1,
                name: "step-input-pipe".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    ("upstream".into(), upstream),
                    ("downstream".into(), downstream),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            (def, dir)
        }

        #[test]
        fn step_input_pipes_upstream_output_to_downstream() {
            // Headline test: a downstream step's `step_input("msg")`
            // call returns the JSON-encoded upstream output. The
            // downstream's final Value carries that JSON string.
            let (def, wf_dir) = upstream_downstream_workflow();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let result = WorkflowRunner::run(&def, &options).unwrap();
            assert_eq!(result.status, WorkflowStatus::Completed);
            assert_eq!(
                result.step_results["upstream"].status,
                StepStatus::Completed
            );
            assert_eq!(
                result.step_results["downstream"].status,
                StepStatus::Completed
            );
            // Downstream's output_hash is bit-stable — it's
            // hash_value(Value::String("{\"String\":\"hello-from-upstream\"}"))
            // (the JSON-encoded upstream value).
            let upstream_hash = result.step_results["upstream"]
                .output_hash
                .as_deref()
                .unwrap();
            let downstream_hash = result.step_results["downstream"]
                .output_hash
                .as_deref()
                .unwrap();
            // The two hashes differ because downstream wraps the
            // upstream JSON in another String. Both must be present
            // and non-empty.
            assert!(!upstream_hash.is_empty());
            assert!(!downstream_hash.is_empty());
            assert_ne!(upstream_hash, downstream_hash);
        }

        #[test]
        fn step_input_unknown_name_returns_typed_error() {
            // 0.3-S14 review-driven regression: a .ax step that
            // calls `step_input("undeclared_name")` (no matching
            // entry in workflow.json::inputs) MUST surface a typed
            // runtime error, not silently return empty data and
            // corrupt the downstream output. The runner's
            // pre-validation only covers DECLARED names; the
            // gateway's StepInputHandler is the catch-all.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(
                steps_dir.join("upstream.ax"),
                "fn main() -> String { \"hi\" }",
            )
            .unwrap();
            // Downstream calls step_input with an UNDECLARED name.
            std::fs::write(
                steps_dir.join("downstream.ax"),
                "fn main() -> String {\n    let x: String = step_input(\"missing\")\n    x\n}",
            )
            .unwrap();
            let upstream = StepDef {
                kind: StepKind::Source {
                    source: "steps/upstream.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            let mut downstream = StepDef {
                kind: StepKind::Source {
                    source: "steps/downstream.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec!["upstream".into()],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            // Declare "msg" but the .ax step asks for "missing" —
            // pre-validation passes, gateway catches the mismatch.
            downstream
                .inputs
                .insert("msg".into(), "upstream.result".into());
            let def = WorkflowDef {
                schema_version: 1,
                name: "step-input-undeclared".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    ("upstream".into(), upstream),
                    ("downstream".into(), downstream),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let result = WorkflowRunner::run(&def, &options).unwrap();
            assert_eq!(result.status, WorkflowStatus::Failed);
            // Downstream failed; error message points at the
            // unknown name with the declared list for triage.
            let err = result.step_results["downstream"].error.as_deref().unwrap();
            assert!(
                err.contains("step.input: unknown name 'missing'"),
                "expected unknown-name error; got: {err}"
            );
            assert!(
                err.contains("declared inputs:"),
                "error should hint at the declared list; got: {err}"
            );
        }

        #[test]
        fn operator_can_deny_step_input_via_policy() {
            // 0.3-S14 review-driven regression: build_step_policy's
            // auto-allow uses .entry().or_insert() so an operator's
            // explicit deny rule is preserved. Verify that a workflow
            // with an explicit step.input deny in its policy fails
            // any step that calls step_input.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(
                steps_dir.join("upstream.ax"),
                "fn main() -> String { \"hi\" }",
            )
            .unwrap();
            std::fs::write(
                steps_dir.join("downstream.ax"),
                "fn main() -> String {\n    let x: String = step_input(\"msg\")\n    x\n}",
            )
            .unwrap();
            let upstream = StepDef {
                kind: StepKind::Source {
                    source: "steps/upstream.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec![],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            let mut downstream = StepDef {
                kind: StepKind::Source {
                    source: "steps/downstream.ax".into(),
                },
                capabilities: vec![],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec!["upstream".into()],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            downstream
                .inputs
                .insert("msg".into(), "upstream.result".into());
            let def = WorkflowDef {
                schema_version: 1,
                name: "step-input-denied".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    ("upstream".into(), upstream),
                    ("downstream".into(), downstream),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();

            // Build a policy that explicitly DENIES step.input.
            // entry().or_insert() in build_step_policy must preserve
            // this — the auto-allow only fires for absent rules.
            let mut policy = Policy::allow_all();
            policy.rules.insert(
                "step.input".to_string(),
                PolicyRule {
                    allow: false,
                    budget: 0,
                },
            );
            let options = RunOptions {
                policy: Some(policy),
                record: false,
                workflow_dir: dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let result = WorkflowRunner::run(&def, &options).unwrap();
            assert_eq!(
                result.status,
                WorkflowStatus::Failed,
                "explicit step.input deny must be preserved"
            );
            assert_eq!(result.step_results["downstream"].status, StepStatus::Failed);
        }

        #[test]
        #[cfg(feature = "persist-sqlite")]
        fn step_input_works_under_concurrent_execution() {
            // Same pipe, but at concurrency=4. Determinism contract:
            // the per-step output_hash is bit-identical to the
            // sequential run.
            let (def, wf_dir) = upstream_downstream_workflow();
            let make_options = |c| RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: c,
                submit_only: false,
            };
            let dir1 = tempfile::tempdir().unwrap();
            let r1 = WorkflowRunner::run_persistent(&def, &make_options(1), dir1.path()).unwrap();
            let dir4 = tempfile::tempdir().unwrap();
            let r4 = WorkflowRunner::run_persistent(&def, &make_options(4), dir4.path()).unwrap();
            assert_eq!(r1.status, WorkflowStatus::Completed);
            assert_eq!(r4.status, WorkflowStatus::Completed);
            for step_id in ["upstream", "downstream"] {
                assert_eq!(
                    r1.step_results[step_id].output_hash, r4.step_results[step_id].output_hash,
                    "step '{step_id}' hash differs across concurrency levels"
                );
            }
        }
    }

    // ── 0.3-S15: external_trigger step (async step execution) ──

    #[cfg(feature = "persist-sqlite")]
    mod external_trigger {
        use super::*;

        pub(super) fn workflow_with_external_trigger() -> (WorkflowDef, tempfile::TempDir) {
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            // Pre-trigger source step + external_trigger gate +
            // post-trigger source step. The post-trigger step reads the
            // payload via step_input("event") and echoes it.
            std::fs::write(steps_dir.join("init.ax"), "fn main() -> Int { 1 }").unwrap();
            std::fs::write(
                steps_dir.join("after.ax"),
                "fn main() -> String {\n    let payload: String = step_input(\"event\")\n    payload\n}",
            )
            .unwrap();
            let mut after = StepDef {
                kind: StepKind::Source {
                    source: "steps/after.ax".into(),
                },
                capabilities: vec!["step.input".into()],
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                depends_on: vec!["webhook".into()],
                timeout_ms: None,
                retry: None,
                budget: None,
                required_capability_versions: Default::default(),
            };
            after.inputs.insert("event".into(), "webhook.result".into());
            let def = WorkflowDef {
                schema_version: 1,
                name: "trigger-test".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    (
                        "init".into(),
                        StepDef {
                            kind: StepKind::Source {
                                source: "steps/init.ax".into(),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec![],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "webhook".into(),
                        StepDef {
                            kind: StepKind::ExternalTrigger {
                                description: Some("test webhook".into()),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["init".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    ("after".into(), after),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            (def, dir)
        }

        fn paused_run(data_dir: &Path, wf_dir: &Path, def: &WorkflowDef) -> String {
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(def, &options, data_dir).unwrap();
            assert_eq!(r.status, WorkflowStatus::Paused);
            assert_eq!(
                r.step_results["webhook"].status,
                StepStatus::AwaitingExternalEvent
            );
            r.run_id
        }

        fn read_token(data_dir: &Path, run_id: &str, step_id: &str) -> String {
            let store = open_store(data_dir).unwrap();
            let metadata_json = store.get_run_metadata(run_id).unwrap().unwrap();
            let metadata: PersistedRunMetadata = serde_json::from_str(&metadata_json).unwrap();
            metadata
                .triggers
                .get(step_id)
                .expect("trigger record must exist after pause")
                .token
                .clone()
        }

        // ── happy path ──

        #[test]
        fn resume_without_trigger_preserves_persisted_token() {
            // Reviewed 0.3-S15 — earlier draft generated a fresh token
            // on every pause entry while persist_trigger_token's
            // "leave existing" branch kept the original. The token
            // printed on resume's stderr would silently disconnect from
            // the validated token; operators copying the just-printed
            // value would get InvalidTriggerToken.
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let token_after_initial_pause = read_token(data_dir.path(), &run_id, "webhook");

            // Resume without triggering — should re-pause and the
            // persisted token must be unchanged.
            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Paused);
            assert_eq!(
                resumed.step_results["webhook"].status,
                StepStatus::AwaitingExternalEvent
            );
            let token_after_resume = read_token(data_dir.path(), &run_id, "webhook");
            assert_eq!(
                token_after_initial_pause, token_after_resume,
                "token must not rotate on resume — operators may have copied the original"
            );

            // The original token should still validate.
            record_external_trigger(
                data_dir.path(),
                &run_id,
                "webhook",
                &token_after_initial_pause,
                "{\"v\":1}",
            )
            .expect("original token must validate after resume");
        }

        // ── 0.3-S16: atomic commit (TOCTOU fix) ──

        #[test]
        fn trigger_transitions_checkpoint_to_completed_atomically() {
            // Reviewed 0.3-S15 → fixed in 0.3-S16. Earlier flow had:
            //  1. record_external_trigger CAS-writes metadata.payload
            //  2. resume's trigger-sentinel pass flips checkpoint to
            //     Completed
            // The two writes were not atomic — a concurrent resume
            // could mark_step_running between them and the next
            // resume's sentinel pass would silently skip the payload.
            //
            // The fix: record_external_trigger commits BOTH writes in
            // a single SQLite transaction under BEGIN IMMEDIATE.
            // Confirm the checkpoint is Completed immediately after
            // the trigger function returns, BEFORE any resume runs.
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let token = read_token(data_dir.path(), &run_id, "webhook");

            record_external_trigger(data_dir.path(), &run_id, "webhook", &token, "{\"v\":1}")
                .unwrap();

            // No resume yet — but the checkpoint should already be
            // Completed because the trigger function's SQL transaction
            // transitions both metadata and checkpoint atomically.
            let store = open_store(data_dir.path()).unwrap();
            let cps = store.list_step_checkpoints(&run_id).unwrap();
            let webhook_cp = cps.iter().find(|c| c.step_id == "webhook").unwrap();
            assert_eq!(
                webhook_cp.status,
                PersistStepStatus::Completed,
                "checkpoint must be Completed immediately after trigger commit"
            );
            // Output must already be persisted; downstream steps read
            // it from output_json on the next resume's checkpoint walk.
            assert!(webhook_cp.output_json.is_some());
            assert!(webhook_cp.output_hash.is_some());
            assert!(webhook_cp.error_msg.is_none());
        }

        #[test]
        fn trigger_after_step_already_completed_returns_state_mismatch() {
            // After the atomic commit, a second call to
            // record_external_trigger sees the checkpoint as Completed.
            // The CheckpointStateMismatch outcome from the persistence
            // layer surfaces as StepNotAtExternalTriggerGate.
            //
            // Note: the metadata-side `StepAlreadyTriggered` guard
            // (line ~2562) catches this earlier in the function — the
            // payload is non-empty after the first trigger. Confirm
            // that's the surfaced error, not the lower-level state-
            // mismatch.
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let token = read_token(data_dir.path(), &run_id, "webhook");

            record_external_trigger(data_dir.path(), &run_id, "webhook", &token, "{\"v\":1}")
                .unwrap();
            let err =
                record_external_trigger(data_dir.path(), &run_id, "webhook", &token, "{\"v\":2}")
                    .expect_err("second trigger must error");
            // The metadata guard fires first — that's the user-facing
            // semantic (webhook replay).
            assert!(matches!(err, WorkflowRunError::StepAlreadyTriggered { .. }));
        }

        #[test]
        fn resume_after_atomic_trigger_uses_persisted_output_not_sentinel_pass() {
            // The atomic commit makes the resume sentinel pass mostly
            // defensive: by the time resume runs, the checkpoint is
            // already Completed and the output is in step_checkpoints.
            // The walk-checkpoints loop restores the output into the
            // in-memory data store; the trigger sentinel pass sees a
            // Completed checkpoint and falls through. Downstream steps
            // read the payload via step_input as expected.
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let token = read_token(data_dir.path(), &run_id, "webhook");
            let payload = r#"{"order":42}"#;

            record_external_trigger(data_dir.path(), &run_id, "webhook", &token, payload).unwrap();

            // Confirm the test setup: checkpoint is Completed BEFORE
            // resume.
            let store = open_store(data_dir.path()).unwrap();
            let cps = store.list_step_checkpoints(&run_id).unwrap();
            assert_eq!(
                cps.iter().find(|c| c.step_id == "webhook").unwrap().status,
                PersistStepStatus::Completed
            );

            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);
            assert_eq!(
                resumed.step_results["webhook"].status,
                StepStatus::Completed
            );
            assert_eq!(resumed.step_results["after"].status, StepStatus::Completed);
        }

        #[test]
        fn legacy_0_3_s15_db_format_upgrades_via_resume_sentinel_pass() {
            // Forward-compat: a 0.3-S15-formatted DB has metadata.triggers
            // with non-empty payload BUT checkpoint still in
            // awaiting_external_event (because S15's record path didn't
            // atomically transition the checkpoint). On 0.3-S16 the
            // resume sentinel pass must still upgrade these runs.
            //
            // Reproduces the S15 shape by injecting metadata + leaving
            // the checkpoint at AwaitingExternalEvent, bypassing the
            // S16 atomic commit.
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);

            // Inject the S15 shape: read metadata, fill in payload,
            // write back via the metadata-only CAS path. Leave the
            // checkpoint untouched (still AwaitingExternalEvent).
            let store = open_store(data_dir.path()).unwrap();
            let metadata_json = store.get_run_metadata(&run_id).unwrap().unwrap();
            let mut metadata: PersistedRunMetadata = serde_json::from_str(&metadata_json).unwrap();
            let token = metadata.triggers.get("webhook").unwrap().token.clone();
            metadata.triggers.insert(
                "webhook".to_string(),
                TriggerRecord {
                    token,
                    payload: "{\"legacy\":true}".to_string(),
                    triggered_at_ms: 1_700_000_000_000,
                },
            );
            let updated_metadata = serde_json::to_string(&metadata).unwrap();
            let swapped = store
                .compare_and_swap_metadata(
                    &run_id,
                    &metadata_json,
                    &updated_metadata,
                    now_unix_ms(),
                )
                .unwrap();
            assert!(swapped);

            // Confirm the legacy shape: payload non-empty, checkpoint
            // still AwaitingExternalEvent.
            let cps = store.list_step_checkpoints(&run_id).unwrap();
            let cp = cps.iter().find(|c| c.step_id == "webhook").unwrap();
            assert_eq!(cp.status, PersistStepStatus::AwaitingExternalEvent);

            // Resume must pick up the legacy shape via the sentinel
            // pass and complete the run.
            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);
            assert_eq!(
                resumed.step_results["webhook"].status,
                StepStatus::Completed
            );
            assert_eq!(resumed.step_results["after"].status, StepStatus::Completed);
        }

        #[test]
        fn empty_payload_returns_validation_error() {
            // The resume sentinel pass uses payload.is_empty() to
            // discriminate placeholder from triggered. An empty
            // payload would be silently treated as "still waiting" and
            // the run would never advance.
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let token = read_token(data_dir.path(), &run_id, "webhook");
            let err = record_external_trigger(data_dir.path(), &run_id, "webhook", &token, "")
                .expect_err("empty payload must error");
            assert!(matches!(err, WorkflowRunError::Validation(_)));
        }

        #[test]
        fn run_pauses_at_external_trigger() {
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            // The token is stashed in metadata.
            let token = read_token(data_dir.path(), &run_id, "webhook");
            assert_eq!(token.len(), 32, "token must be 32 hex chars");
            assert!(
                token.chars().all(|c| c.is_ascii_hexdigit()),
                "token must be hex"
            );
        }

        #[test]
        fn trigger_then_resume_advances_with_payload() {
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let token = read_token(data_dir.path(), &run_id, "webhook");

            let payload = r#"{"order_id":"42","status":"paid"}"#;
            record_external_trigger(data_dir.path(), &run_id, "webhook", &token, payload).unwrap();

            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);
            assert_eq!(
                resumed.step_results["webhook"].status,
                StepStatus::Completed
            );
            assert_eq!(resumed.step_results["after"].status, StepStatus::Completed);
        }

        // ── token validation ──

        #[test]
        fn trigger_with_wrong_token_returns_invalid_token() {
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);

            let err = record_external_trigger(
                data_dir.path(),
                &run_id,
                "webhook",
                "00000000000000000000000000000000",
                "{}",
            )
            .expect_err("wrong token must error");
            match err {
                WorkflowRunError::InvalidTriggerToken { step_id, .. } => {
                    assert_eq!(step_id, "webhook")
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }

        #[test]
        fn trigger_with_short_token_returns_invalid_token() {
            // Defense against any short-circuit equality comparison.
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);

            let err = record_external_trigger(data_dir.path(), &run_id, "webhook", "abc", "{}")
                .expect_err("short token must error");
            assert!(matches!(err, WorkflowRunError::InvalidTriggerToken { .. }));
        }

        // ── replay / idempotency ──

        #[test]
        fn duplicate_trigger_returns_step_already_triggered() {
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let token = read_token(data_dir.path(), &run_id, "webhook");

            record_external_trigger(data_dir.path(), &run_id, "webhook", &token, "{\"v\":1}")
                .unwrap();
            // Second trigger with the same token: webhook replay
            // scenario. Idempotency guard refuses.
            let err =
                record_external_trigger(data_dir.path(), &run_id, "webhook", &token, "{\"v\":2}")
                    .expect_err("duplicate trigger must error");
            match err {
                WorkflowRunError::StepAlreadyTriggered { step_id, .. } => {
                    assert_eq!(step_id, "webhook")
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }

        // ── wrong-state validation ──

        #[test]
        fn trigger_unknown_run_id_returns_run_not_found() {
            let data_dir = tempfile::tempdir().unwrap();
            let _ = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let err = record_external_trigger(
                data_dir.path(),
                "ffffffffffffffff",
                "webhook",
                "deadbeef",
                "{}",
            )
            .expect_err("missing run must error");
            assert!(matches!(err, WorkflowRunError::RunNotFound(_)));
        }

        #[test]
        fn trigger_non_trigger_step_returns_not_an_external_trigger_step() {
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let err = record_external_trigger(
                data_dir.path(),
                &run_id,
                "init",
                "deadbeefdeadbeefdeadbeefdeadbeef",
                "{}",
            )
            .expect_err("non-trigger step must error");
            assert!(matches!(
                err,
                WorkflowRunError::NotAnExternalTriggerStep { .. }
            ));
        }

        #[test]
        fn trigger_unknown_step_returns_step_not_found() {
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let err = record_external_trigger(
                data_dir.path(),
                &run_id,
                "no-such-step",
                "deadbeefdeadbeefdeadbeefdeadbeef",
                "{}",
            )
            .expect_err("missing step must error");
            assert!(matches!(err, WorkflowRunError::StepNotFound { .. }));
        }

        #[test]
        fn ephemeral_run_with_external_trigger_returns_validation_error() {
            // Without persistence there's nowhere to stash the token,
            // so an operator could never advance the run. The runner
            // surfaces a typed Validation error instead of pausing
            // forever.
            let (def, wf_dir) = workflow_with_external_trigger();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let err = WorkflowRunner::run(&def, &options).expect_err("ephemeral path must error");
            assert!(matches!(err, WorkflowRunError::Validation(_)));
        }

        // ── end-to-end: payload becomes step output ──

        #[test]
        fn payload_propagates_to_downstream_step_via_step_input() {
            let (def, wf_dir) = workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let run_id = paused_run(data_dir.path(), wf_dir.path(), &def);
            let token = read_token(data_dir.path(), &run_id, "webhook");

            let payload = r#"{"order_id":"42","status":"paid"}"#;
            record_external_trigger(data_dir.path(), &run_id, "webhook", &token, payload).unwrap();

            let resumed = WorkflowRunner::resume(
                &run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);

            // Confirm the persisted output_json for `webhook` is the
            // payload as a JSON-encoded String value (i.e. the JSON
            // string literal of the payload, not the parsed object).
            let store = open_store(data_dir.path()).unwrap();
            let cps = store.list_step_checkpoints(&run_id).unwrap();
            let webhook_cp = cps.iter().find(|c| c.step_id == "webhook").unwrap();
            let output: boruna_bytecode::Value =
                serde_json::from_str(webhook_cp.output_json.as_ref().unwrap()).unwrap();
            assert_eq!(output, boruna_bytecode::Value::String(payload.to_string()));
        }
    }

    // ── 0.4-S7: multi-pause-per-wave (parallel webhook fan-in) ──

    #[cfg(feature = "persist-sqlite")]
    mod multi_pause_per_wave {
        use super::*;

        /// Build a workflow with TWO ExternalTrigger steps at the same
        /// DAG level (both depend on a single root source step). A
        /// downstream step depends on both. Models a "wait for payment
        /// AND fraud-check" webhook fan-in pattern.
        fn workflow_with_two_parallel_triggers() -> (WorkflowDef, tempfile::TempDir) {
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("init.ax"), "fn main() -> Int { 1 }").unwrap();
            std::fs::write(steps_dir.join("after.ax"), "fn main() -> Int { 99 }").unwrap();
            let def = WorkflowDef {
                schema_version: 1,
                name: "two-trigger-parallel".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    (
                        "init".into(),
                        StepDef {
                            kind: StepKind::Source {
                                source: "steps/init.ax".into(),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec![],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "payment_webhook".into(),
                        StepDef {
                            kind: StepKind::ExternalTrigger {
                                description: Some("Stripe payment.succeeded".into()),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["init".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "fraud_check_webhook".into(),
                        StepDef {
                            kind: StepKind::ExternalTrigger {
                                description: Some("Sift fraud-check verdict".into()),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["init".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "after".into(),
                        StepDef {
                            kind: StepKind::Source {
                                source: "steps/after.ax".into(),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec![
                                "payment_webhook".into(),
                                "fraud_check_webhook".into(),
                            ],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            (def, dir)
        }

        fn read_token_for(data_dir: &Path, run_id: &str, step_id: &str) -> String {
            let store = open_store(data_dir).unwrap();
            let metadata_json = store.get_run_metadata(run_id).unwrap().unwrap();
            let metadata: PersistedRunMetadata = serde_json::from_str(&metadata_json).unwrap();
            metadata
                .triggers
                .get(step_id)
                .expect("trigger record must exist after pause")
                .token
                .clone()
        }

        #[test]
        fn run_pauses_at_both_triggers_in_one_wave() {
            // 0.4-S7 contract: a wave with TWO ExternalTrigger steps
            // pauses BOTH in a single execution pass — earlier
            // behavior processed only the first and left the second
            // for the next resume.
            //
            // Concurrency must be > 1 to engage the wave-loop path
            // (the sequential `execute_steps` serializes pauses by
            // design).
            let (def, wf_dir) = workflow_with_two_parallel_triggers();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 2,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            assert_eq!(r.status, WorkflowStatus::Paused);
            assert_eq!(
                r.step_results["payment_webhook"].status,
                StepStatus::AwaitingExternalEvent
            );
            assert_eq!(
                r.step_results["fraud_check_webhook"].status,
                StepStatus::AwaitingExternalEvent
            );

            // Both checkpoints must be persisted as
            // AwaitingExternalEvent and both must have distinct tokens.
            let store = open_store(data_dir.path()).unwrap();
            let cps = store.list_step_checkpoints(&r.run_id).unwrap();
            let payment_cp = cps.iter().find(|c| c.step_id == "payment_webhook").unwrap();
            let fraud_cp = cps
                .iter()
                .find(|c| c.step_id == "fraud_check_webhook")
                .unwrap();
            assert_eq!(payment_cp.status, PersistStepStatus::AwaitingExternalEvent);
            assert_eq!(fraud_cp.status, PersistStepStatus::AwaitingExternalEvent);

            let payment_token = read_token_for(data_dir.path(), &r.run_id, "payment_webhook");
            let fraud_token = read_token_for(data_dir.path(), &r.run_id, "fraud_check_webhook");
            assert_ne!(
                payment_token, fraud_token,
                "each pause must mint a distinct token"
            );
            assert_eq!(payment_token.len(), 32);
            assert_eq!(fraud_token.len(), 32);
        }

        #[test]
        fn triggering_one_keeps_other_paused() {
            // After triggering the first webhook, the run should
            // remain Paused on the second.
            let (def, wf_dir) = workflow_with_two_parallel_triggers();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 2,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();

            let payment_token = read_token_for(data_dir.path(), &r.run_id, "payment_webhook");
            record_external_trigger(
                data_dir.path(),
                &r.run_id,
                "payment_webhook",
                &payment_token,
                "{\"paid\":true}",
            )
            .unwrap();

            let resumed = WorkflowRunner::resume(
                &r.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    concurrency: 2,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Paused);
            assert_eq!(
                resumed.step_results["payment_webhook"].status,
                StepStatus::Completed
            );
            assert_eq!(
                resumed.step_results["fraud_check_webhook"].status,
                StepStatus::AwaitingExternalEvent
            );
            // The downstream `after` step must NOT have run yet —
            // it depends on both pauses.
            assert!(!resumed.step_results.contains_key("after"));
        }

        #[test]
        fn triggering_both_advances_downstream_step() {
            // Triggering both pauses (across two record_external_trigger
            // calls) and resuming should run the downstream `after`
            // step exactly once.
            let (def, wf_dir) = workflow_with_two_parallel_triggers();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 2,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();

            let payment_token = read_token_for(data_dir.path(), &r.run_id, "payment_webhook");
            let fraud_token = read_token_for(data_dir.path(), &r.run_id, "fraud_check_webhook");

            record_external_trigger(
                data_dir.path(),
                &r.run_id,
                "payment_webhook",
                &payment_token,
                "{\"paid\":true}",
            )
            .unwrap();
            record_external_trigger(
                data_dir.path(),
                &r.run_id,
                "fraud_check_webhook",
                &fraud_token,
                "{\"verdict\":\"clean\"}",
            )
            .unwrap();

            let resumed = WorkflowRunner::resume(
                &r.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    concurrency: 2,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);
            assert_eq!(
                resumed.step_results["payment_webhook"].status,
                StepStatus::Completed
            );
            assert_eq!(
                resumed.step_results["fraud_check_webhook"].status,
                StepStatus::Completed
            );
            assert_eq!(resumed.step_results["after"].status, StepStatus::Completed);
        }

        #[test]
        fn resume_recovers_partial_pause_state() {
            // 0.4-S7 contract: if a wave's pause loop committed pause
            // #1 but failed mid-loop on pause #2 (transient urandom /
            // upsert error), the next resume must re-encounter pause
            // #2 and persist it cleanly without disturbing pause #1.
            //
            // Simulates the partial state by dropping pause #2's
            // checkpoint + metadata entry via direct SQL after the run
            // pauses successfully. Then resume — expect both pauses
            // re-present, pause #1's token unchanged, pause #2 freshly
            // minted.
            use rusqlite::Connection;
            let (def, wf_dir) = workflow_with_two_parallel_triggers();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 2,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            assert_eq!(r.status, WorkflowStatus::Paused);
            let payment_token_before =
                read_token_for(data_dir.path(), &r.run_id, "payment_webhook");

            // Simulate the partial-failure on-disk shape: pause #1
            // committed, pause #2 never touched.
            let db_path = data_dir.path().join("runs.db");
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "DELETE FROM step_checkpoints WHERE run_id = ?1 AND step_id = ?2",
                rusqlite::params![&r.run_id, "fraud_check_webhook"],
            )
            .unwrap();
            let metadata_json: String = conn
                .query_row(
                    "SELECT metadata_json FROM runs WHERE run_id = ?1",
                    rusqlite::params![&r.run_id],
                    |row| row.get(0),
                )
                .unwrap();
            let mut metadata: PersistedRunMetadata = serde_json::from_str(&metadata_json).unwrap();
            metadata.triggers.remove("fraud_check_webhook");
            let updated = serde_json::to_string(&metadata).unwrap();
            conn.execute(
                "UPDATE runs SET metadata_json = ?1 WHERE run_id = ?2",
                rusqlite::params![&updated, &r.run_id],
            )
            .unwrap();
            drop(conn);

            // Resume — wave loop must re-encounter fraud_check_webhook
            // (no checkpoint, no metadata entry) and persist fresh.
            // payment_webhook stays untouched.
            let resumed = WorkflowRunner::resume(
                &r.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    concurrency: 2,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Paused);
            assert_eq!(
                resumed.step_results["payment_webhook"].status,
                StepStatus::AwaitingExternalEvent
            );
            assert_eq!(
                resumed.step_results["fraud_check_webhook"].status,
                StepStatus::AwaitingExternalEvent
            );

            let payment_token_after = read_token_for(data_dir.path(), &r.run_id, "payment_webhook");
            assert_eq!(
                payment_token_before, payment_token_after,
                "pause #1 token must be preserved across recovery resume"
            );
            let fraud_token = read_token_for(data_dir.path(), &r.run_id, "fraud_check_webhook");
            assert_eq!(fraud_token.len(), 32);
            assert_ne!(fraud_token, payment_token_after);
        }

        #[test]
        fn mixed_approval_and_trigger_in_same_wave_both_pause() {
            // Cross-kind parallel pauses: an approval gate and an
            // external trigger at the same DAG level.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("init.ax"), "fn main() -> Int { 1 }").unwrap();
            std::fs::write(steps_dir.join("after.ax"), "fn main() -> Int { 2 }").unwrap();
            let def = WorkflowDef {
                schema_version: 1,
                name: "mixed-pause".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    (
                        "init".into(),
                        StepDef {
                            kind: StepKind::Source {
                                source: "steps/init.ax".into(),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec![],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "approval".into(),
                        StepDef {
                            kind: StepKind::ApprovalGate {
                                required_role: "ops".into(),
                                condition: None,
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["init".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "webhook".into(),
                        StepDef {
                            kind: StepKind::ExternalTrigger { description: None },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["init".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 2,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            assert_eq!(r.status, WorkflowStatus::Paused);
            assert_eq!(
                r.step_results["approval"].status,
                StepStatus::AwaitingApproval
            );
            assert_eq!(
                r.step_results["webhook"].status,
                StepStatus::AwaitingExternalEvent
            );
        }
    }

    // ── 0.4-S9: audit-log integration of approval / trigger decisions ──

    #[cfg(feature = "persist-sqlite")]
    mod audit_decisions {
        use super::*;
        use crate::audit::{AuditEvent, AuditLog};

        pub(super) fn read_audit_log(data_dir: &Path, run_id: &str) -> AuditLog {
            let store = open_store(data_dir).unwrap();
            let metadata_json = store.get_run_metadata(run_id).unwrap().unwrap();
            let metadata: PersistedRunMetadata = serde_json::from_str(&metadata_json).unwrap();
            AuditLog::from_entries(metadata.audit_log)
        }

        // ── approval_gate ──

        #[test]
        fn approval_grant_appends_audit_event_and_chain_verifies() {
            let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            assert_eq!(r.status, WorkflowStatus::Paused);

            // 0.4-S11: chain now starts with WorkflowStarted, plus
            // any step-completion events from steps that ran before
            // the gate (`analyze`). After approval grant, the chain
            // contains the lifecycle events PLUS the new
            // ApprovalGranted entry.
            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();

            let log = read_audit_log(data_dir.path(), &r.run_id);
            let granted_count = log
                .entries()
                .iter()
                .filter(|e| {
                    matches!(
                        &e.event,
                        AuditEvent::ApprovalGranted { step_id, .. } if step_id == "human_review"
                    )
                })
                .count();
            assert_eq!(granted_count, 1, "expect one ApprovalGranted event");
            // First entry is the lifecycle WorkflowStarted (sprint 0.4-S11).
            assert!(matches!(
                log.entries()[0].event,
                AuditEvent::WorkflowStarted { .. }
            ));
            log.verify().expect("hash chain must verify");
        }

        #[test]
        fn approval_reject_appends_audit_event_with_reason() {
            let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();

            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "human_review",
                ApprovalKind::Rejected,
                Some("policy violation".to_string()),
            )
            .unwrap();

            let log = read_audit_log(data_dir.path(), &r.run_id);
            // 0.4-S11: chain contains WorkflowStarted + step events +
            // the new ApprovalDenied. Find the denied entry by variant.
            let denied = log
                .entries()
                .iter()
                .find_map(|e| match &e.event {
                    AuditEvent::ApprovalDenied { step_id, reason } => {
                        Some((step_id.clone(), reason.clone()))
                    }
                    _ => None,
                })
                .expect("ApprovalDenied event must be present");
            assert_eq!(denied.0, "human_review");
            assert_eq!(denied.1, "policy violation");
            log.verify().expect("hash chain must verify");
        }

        // ── external_trigger ──

        #[test]
        fn trigger_appends_audit_event_with_payload_hash() {
            let (def, wf_dir) = external_trigger::workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            let store = open_store(data_dir.path()).unwrap();
            let metadata_json = store.get_run_metadata(&r.run_id).unwrap().unwrap();
            let metadata: PersistedRunMetadata = serde_json::from_str(&metadata_json).unwrap();
            let token = metadata.triggers.get("webhook").unwrap().token.clone();

            let payload = "{\"order_id\":\"42\"}";
            record_external_trigger(data_dir.path(), &r.run_id, "webhook", &token, payload)
                .unwrap();

            let log = read_audit_log(data_dir.path(), &r.run_id);
            // 0.4-S11: chain contains WorkflowStarted + step events
            // for `init` + the new ExternalTriggerReceived. Find the
            // trigger entry by variant.
            let expected_hash =
                DataStore::hash_value(&boruna_bytecode::Value::String(payload.to_string()));
            let trigger = log
                .entries()
                .iter()
                .find_map(|e| match &e.event {
                    AuditEvent::ExternalTriggerReceived {
                        step_id,
                        payload_hash,
                    } => Some((step_id.clone(), payload_hash.clone())),
                    _ => None,
                })
                .expect("ExternalTriggerReceived event must be present");
            assert_eq!(trigger.0, "webhook");
            assert_eq!(trigger.1, expected_hash);
            log.verify().expect("hash chain must verify");
        }

        // ── chain integrity ──

        #[test]
        fn multiple_decisions_chain_correctly() {
            // A workflow with two approval gates. Two record_approval_decision
            // calls produce a 2-entry chain whose hashes link.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("init.ax"), "fn main() -> Int { 1 }").unwrap();
            let def = WorkflowDef {
                schema_version: 1,
                name: "two-gates".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    (
                        "init".into(),
                        StepDef {
                            kind: StepKind::Source {
                                source: "steps/init.ax".into(),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec![],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "gate_a".into(),
                        StepDef {
                            kind: StepKind::ApprovalGate {
                                required_role: "ops".into(),
                                condition: None,
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["init".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "gate_b".into(),
                        StepDef {
                            kind: StepKind::ApprovalGate {
                                required_role: "ops".into(),
                                condition: None,
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["init".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 2,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            assert_eq!(r.status, WorkflowStatus::Paused);

            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "gate_a",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();
            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "gate_b",
                ApprovalKind::Rejected,
                Some("blocked".into()),
            )
            .unwrap();

            let log = read_audit_log(data_dir.path(), &r.run_id);
            // 0.4-S11: chain integrity check — every entry's prev_hash
            // chains to the previous entry's entry_hash, regardless of
            // which event variants are present.
            for window in log.entries().windows(2) {
                assert_eq!(
                    window[1].prev_hash, window[0].entry_hash,
                    "prev_hash linkage must be intact across all entries"
                );
            }
            // The two specific decisions must both appear.
            let decisions: Vec<_> = log
                .entries()
                .iter()
                .filter_map(|e| match &e.event {
                    AuditEvent::ApprovalGranted { step_id, .. } => {
                        Some(("granted", step_id.as_str()))
                    }
                    AuditEvent::ApprovalDenied { step_id, .. } => {
                        Some(("denied", step_id.as_str()))
                    }
                    _ => None,
                })
                .collect();
            assert!(decisions.contains(&("granted", "gate_a")));
            assert!(decisions.contains(&("denied", "gate_b")));
            log.verify().expect("hash chain must verify");
        }

        // ── back-compat ──

        #[test]
        fn legacy_metadata_without_audit_log_field_deserializes_with_empty_chain() {
            // A 0.3.x metadata blob has no `audit_log` key. With
            // #[serde(default)] on the new field, deserialization
            // produces an empty Vec.
            let legacy_json = r#"{
                "workflow_dir": "/tmp/wf",
                "inputs_hash": "deadbeef",
                "boruna_version": "0.3.0",
                "approvals": {},
                "triggers": {}
            }"#;
            let m: PersistedRunMetadata = serde_json::from_str(legacy_json).unwrap();
            assert_eq!(m.workflow_dir, "/tmp/wf");
            assert_eq!(m.inputs_hash, "deadbeef");
            assert!(m.audit_log.is_empty());
        }

        #[test]
        fn first_decision_after_legacy_metadata_starts_chain_at_sequence_zero() {
            // Forward-compat: a 0.3.x DB whose metadata was written
            // before this sprint has no `audit_log` field. The first
            // decision recorded by a 0.4-S9 binary on that run must
            // start a fresh chain — sequence=0, prev_hash="0"*64 —
            // not error or panic. Validates that the
            // `from_entries(Vec::new())` path in record_approval_decision
            // produces a clean genesis entry.
            use rusqlite::Connection;
            let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();

            // Strip the audit_log field from on-disk metadata to
            // simulate a 0.3.x-format row.
            let db_path = data_dir.path().join("runs.db");
            let conn = Connection::open(&db_path).unwrap();
            let metadata_json: String = conn
                .query_row(
                    "SELECT metadata_json FROM runs WHERE run_id = ?1",
                    rusqlite::params![&r.run_id],
                    |row| row.get(0),
                )
                .unwrap();
            let mut meta_value: serde_json::Value = serde_json::from_str(&metadata_json).unwrap();
            meta_value.as_object_mut().unwrap().remove("audit_log");
            let stripped = serde_json::to_string(&meta_value).unwrap();
            conn.execute(
                "UPDATE runs SET metadata_json = ?1 WHERE run_id = ?2",
                rusqlite::params![&stripped, &r.run_id],
            )
            .unwrap();
            drop(conn);

            // Decide — the new entry should be the chain's genesis.
            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();

            let log = read_audit_log(data_dir.path(), &r.run_id);
            assert_eq!(log.entries().len(), 1);
            assert_eq!(log.entries()[0].sequence, 0);
            assert_eq!(log.entries()[0].prev_hash, "0".repeat(64));
            log.verify().expect("genesis chain must verify");
        }

        #[test]
        fn audit_log_grows_across_resume_with_linked_chain() {
            // 0.4-S11: resume now appends step-completion events for
            // steps that ran during the resume + a final
            // WorkflowCompleted. The pre-resume entries must remain
            // unchanged (same entry_hash values), and the new entries
            // chain on top via prev_hash linkage.
            let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let options = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            let r = WorkflowRunner::run_persistent(&def, &options, data_dir.path()).unwrap();
            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();
            let pre_resume_log = read_audit_log(data_dir.path(), &r.run_id);
            let pre_count = pre_resume_log.entries().len();
            let pre_hashes: Vec<String> = pre_resume_log
                .entries()
                .iter()
                .map(|e| e.entry_hash.clone())
                .collect();

            let resumed = WorkflowRunner::resume(
                &r.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);

            let post_resume_log = read_audit_log(data_dir.path(), &r.run_id);
            assert!(
                post_resume_log.entries().len() > pre_count,
                "resume must append lifecycle events to the chain"
            );
            // Pre-resume entries must be byte-identical (no
            // re-hashing or re-ordering).
            for (i, expected_hash) in pre_hashes.iter().enumerate() {
                assert_eq!(
                    &post_resume_log.entries()[i].entry_hash,
                    expected_hash,
                    "pre-resume entry {i} must be unchanged"
                );
            }
            // Final entry is WorkflowCompleted.
            assert!(matches!(
                post_resume_log.entries().last().unwrap().event,
                AuditEvent::WorkflowCompleted { .. }
            ));
            post_resume_log
                .verify()
                .expect("hash chain must verify across resume");
        }
    }

    // ── 0.4-S10: evidence-bundle creation ──

    #[cfg(feature = "persist-sqlite")]
    mod evidence_bundle {
        use super::*;
        use crate::audit::{verify::verify_bundle, AuditEvent, AuditLog};

        #[test]
        fn create_bundle_writes_complete_artifact_for_completed_run() {
            // End-to-end: run a workflow with an approval gate to
            // completion (record decision + resume), then build a
            // bundle. Bundle must contain workflow.json, policy.json,
            // audit_log.json with the recorded chain entries, per-step
            // outputs, env_fingerprint.json, and a manifest.json with
            // a non-empty bundle_hash.
            let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();
            let resumed = WorkflowRunner::resume(
                &r.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(resumed.status, WorkflowStatus::Completed);

            let output_dir = tempfile::tempdir().unwrap();
            let manifest = create_bundle(data_dir.path(), &r.run_id, output_dir.path()).unwrap();

            assert_eq!(manifest.run_id, r.run_id);
            assert_eq!(manifest.workflow_name, "approval-test");
            assert!(!manifest.bundle_hash.is_empty());
            assert!(!manifest.workflow_hash.is_empty());
            assert!(!manifest.policy_hash.is_empty());
            assert!(!manifest.audit_log_hash.is_empty());

            let bundle_path = output_dir.path().join(&r.run_id);
            assert!(bundle_path.join("manifest.json").exists());
            assert!(bundle_path.join("workflow.json").exists());
            assert!(bundle_path.join("policy.json").exists());
            assert!(bundle_path.join("audit_log.json").exists());
            assert!(bundle_path.join("env_fingerprint.json").exists());

            // Per-step outputs: 'analyze' and 'publish' completed.
            // 'human_review' is the gate — synthesized output is empty
            // record, so it does have an output_json. Confirm it
            // ends up in the bundle alongside the source-step outputs.
            assert!(bundle_path.join("outputs/analyze/result.json").exists());
            assert!(bundle_path.join("outputs/publish/result.json").exists());
        }

        #[test]
        fn create_bundle_rejects_tampered_audit_chain() {
            // Defense-in-depth: if metadata.audit_log is mutated
            // out-of-band (sqlite3 surgery), bundle creation must
            // refuse rather than silently producing a bundle whose
            // internal hash matches the tampered chain. The downstream
            // `boruna evidence verify` step would catch the inconsistency
            // when comparing against an out-of-band hash root, but
            // catching at bundle creation prevents the tampered bundle
            // from leaving the operator's machine in the first place.
            use crate::persistence::RunCheckpointStore;
            let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();
            WorkflowRunner::resume(
                &r.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

            // Tamper: read metadata, mutate the first audit entry's
            // `entry_hash` to a known-wrong value, persist back.
            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let raw = store.get_run_metadata(&r.run_id).unwrap().unwrap();
            let mut metadata: serde_json::Value = serde_json::from_str(&raw).unwrap();
            let log = metadata
                .get_mut("audit_log")
                .and_then(|v| v.as_array_mut())
                .expect("metadata.audit_log present");
            assert!(!log.is_empty(), "expected at least one audit entry");
            log[0]["entry_hash"] = serde_json::Value::String("0".repeat(64));
            let tampered = serde_json::to_string(&metadata).unwrap();
            store.update_run_metadata(&r.run_id, &tampered, 0).unwrap();
            drop(store);

            let output_dir = tempfile::tempdir().unwrap();
            let err = create_bundle(data_dir.path(), &r.run_id, output_dir.path())
                .expect_err("tampered chain must abort bundle creation");
            assert!(
                format!("{err}").contains("audit chain integrity check failed"),
                "expected integrity-check error, got: {err}"
            );
            // The bundle directory must NOT contain a manifest — the
            // tampered run produced no bundle artifact.
            assert!(!output_dir
                .path()
                .join(&r.run_id)
                .join("manifest.json")
                .exists());
        }

        #[test]
        fn create_bundle_audit_log_contains_recorded_chain() {
            // The bundle's audit_log.json must contain the chain
            // recorded by record_approval_decision — verified by
            // round-tripping the file through AuditLog::from_json
            // and checking the entries.
            let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "human_review",
                ApprovalKind::Rejected,
                Some("over budget".into()),
            )
            .unwrap();

            let output_dir = tempfile::tempdir().unwrap();
            create_bundle(data_dir.path(), &r.run_id, output_dir.path()).unwrap();

            let bundle_path = output_dir.path().join(&r.run_id);
            let audit_json = std::fs::read_to_string(bundle_path.join("audit_log.json")).unwrap();
            let audit = AuditLog::from_json(&audit_json).unwrap();
            // 0.4-S11: bundle now contains lifecycle events (started,
            // step completions, possibly completed) plus the
            // ApprovalDenied. Find the denial entry by variant.
            let denied = audit
                .entries()
                .iter()
                .find_map(|e| match &e.event {
                    AuditEvent::ApprovalDenied { step_id, reason } => {
                        Some((step_id.clone(), reason.clone()))
                    }
                    _ => None,
                })
                .expect("ApprovalDenied event must be in bundled chain");
            assert_eq!(denied.0, "human_review");
            assert_eq!(denied.1, "over budget");
            audit.verify().expect("bundle's audit chain must verify");
        }

        #[test]
        fn created_bundle_passes_verify_bundle() {
            // The bundle produced by create_bundle must pass
            // `boruna_orchestrator::audit::verify::verify_bundle`
            // — closes the audit-evidence loop end-to-end.
            let (def, wf_dir) = approval_gate::workflow_with_approval_gate();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            record_approval_decision(
                data_dir.path(),
                &r.run_id,
                "human_review",
                ApprovalKind::Approved,
                None,
            )
            .unwrap();
            let _ = WorkflowRunner::resume(
                &r.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

            let output_dir = tempfile::tempdir().unwrap();
            create_bundle(data_dir.path(), &r.run_id, output_dir.path()).unwrap();
            let bundle_path = output_dir.path().join(&r.run_id);
            let result = verify_bundle(&bundle_path);
            assert!(
                result.valid,
                "bundle verification failed: {:?}",
                result.errors
            );
        }

        #[test]
        fn create_bundle_includes_trigger_payload_hash_in_audit() {
            // After a workflow trigger, the bundle's audit log must
            // contain the ExternalTriggerReceived event with
            // payload_hash matching the synthesized output_hash.
            let (def, wf_dir) = external_trigger::workflow_with_external_trigger();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: wf_dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            let store = open_store(data_dir.path()).unwrap();
            let metadata_json = store.get_run_metadata(&r.run_id).unwrap().unwrap();
            let metadata: PersistedRunMetadata = serde_json::from_str(&metadata_json).unwrap();
            let token = metadata.triggers.get("webhook").unwrap().token.clone();
            let payload = "{\"k\":\"v\"}";
            record_external_trigger(data_dir.path(), &r.run_id, "webhook", &token, payload)
                .unwrap();
            let _ = WorkflowRunner::resume(
                &r.run_id,
                data_dir.path(),
                &ResumeOptions {
                    policy: Some(Policy::allow_all()),
                    workflow_dir_override: Some(wf_dir.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

            let output_dir = tempfile::tempdir().unwrap();
            create_bundle(data_dir.path(), &r.run_id, output_dir.path()).unwrap();
            let audit_json =
                std::fs::read_to_string(output_dir.path().join(&r.run_id).join("audit_log.json"))
                    .unwrap();
            let audit = AuditLog::from_json(&audit_json).unwrap();
            let expected_hash =
                DataStore::hash_value(&boruna_bytecode::Value::String(payload.to_string()));
            let trigger_entry = audit
                .entries()
                .iter()
                .find(|e| matches!(e.event, AuditEvent::ExternalTriggerReceived { .. }))
                .expect("trigger event must be present");
            match &trigger_entry.event {
                AuditEvent::ExternalTriggerReceived {
                    step_id,
                    payload_hash,
                } => {
                    assert_eq!(step_id, "webhook");
                    assert_eq!(payload_hash, &expected_hash);
                }
                _ => unreachable!(),
            }
        }

        #[test]
        fn create_bundle_unknown_run_id_returns_run_not_found() {
            let data_dir = tempfile::tempdir().unwrap();
            let _ = open_store(data_dir.path()).unwrap();
            let output_dir = tempfile::tempdir().unwrap();
            let err = create_bundle(data_dir.path(), "ffffffffffffffff", output_dir.path())
                .expect_err("missing run must error");
            assert!(matches!(err, WorkflowRunError::RunNotFound(_)));
        }

        #[test]
        fn lifecycle_events_emitted_in_order_for_multi_step_run() {
            // 0.4-S11: a 2-step linear workflow produces a chain with
            // [WorkflowStarted, StepCompleted(s1), StepCompleted(s2),
            //  WorkflowCompleted] in topological order.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("a.ax"), "fn main() -> Int { 1 }").unwrap();
            std::fs::write(steps_dir.join("b.ax"), "fn main() -> Int { 2 }").unwrap();
            let def = WorkflowDef {
                schema_version: 1,
                name: "linear".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([
                    (
                        "a".into(),
                        StepDef {
                            kind: StepKind::Source {
                                source: "steps/a.ax".into(),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec![],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                    (
                        "b".into(),
                        StepDef {
                            kind: StepKind::Source {
                                source: "steps/b.ax".into(),
                            },
                            capabilities: vec![],
                            inputs: BTreeMap::new(),
                            outputs: BTreeMap::new(),
                            depends_on: vec!["a".into()],
                            timeout_ms: None,
                            retry: None,
                            budget: None,
                            required_capability_versions: Default::default(),
                        },
                    ),
                ]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            assert_eq!(r.status, WorkflowStatus::Completed);

            let log = audit_decisions::read_audit_log(data_dir.path(), &r.run_id);
            // Must have WorkflowStarted + 2 StepCompleted + WorkflowCompleted = 4
            assert_eq!(log.entries().len(), 4);
            // Order: WorkflowStarted first.
            assert!(matches!(
                log.entries()[0].event,
                AuditEvent::WorkflowStarted { .. }
            ));
            // Next two are StepCompleted in topological order (a, b).
            match &log.entries()[1].event {
                AuditEvent::StepCompleted { step_id, .. } => assert_eq!(step_id, "a"),
                other => panic!("entry 1 must be StepCompleted(a), got {other:?}"),
            }
            match &log.entries()[2].event {
                AuditEvent::StepCompleted { step_id, .. } => assert_eq!(step_id, "b"),
                other => panic!("entry 2 must be StepCompleted(b), got {other:?}"),
            }
            // Last is WorkflowCompleted.
            assert!(matches!(
                log.entries()[3].event,
                AuditEvent::WorkflowCompleted { .. }
            ));
            log.verify().expect("lifecycle chain must verify");
        }

        #[test]
        fn step_failed_event_emitted_on_runtime_error() {
            // 0.4-S11: a step that hits a runtime error produces a
            // StepFailed event with the error message captured in the
            // chain.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(
                steps_dir.join("boom.ax"),
                "fn main() -> Int {\n    let xs: List<Int> = [1, 2, 3]\n    list_get(xs, 99)\n}",
            )
            .unwrap();
            let def = WorkflowDef {
                schema_version: 1,
                name: "boom".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([(
                    "boom".into(),
                    StepDef {
                        kind: StepKind::Source {
                            source: "steps/boom.ax".into(),
                        },
                        capabilities: vec![],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec![],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                        required_capability_versions: Default::default(),
                    },
                )]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            assert_eq!(r.status, WorkflowStatus::Failed);

            let log = audit_decisions::read_audit_log(data_dir.path(), &r.run_id);
            let failed = log
                .entries()
                .iter()
                .find_map(|e| match &e.event {
                    AuditEvent::StepFailed { step_id, error } => {
                        Some((step_id.clone(), error.clone()))
                    }
                    _ => None,
                })
                .expect("StepFailed event must be present");
            assert_eq!(failed.0, "boom");
            assert!(
                failed.1.contains("runtime error"),
                "error message should reference the runtime failure: {}",
                failed.1
            );
            log.verify().expect("chain must verify");
        }

        #[test]
        fn create_bundle_lifecycle_chain_for_run_without_decisions() {
            // 0.4-S11: a run with no decisions still produces a chain
            // — WorkflowStarted, one StepCompleted per source step,
            // and WorkflowCompleted on terminal status. The chain
            // verifies; the bundle includes it.
            let dir = tempfile::tempdir().unwrap();
            let steps_dir = dir.path().join("steps");
            std::fs::create_dir_all(&steps_dir).unwrap();
            std::fs::write(steps_dir.join("only.ax"), "fn main() -> Int { 7 }").unwrap();
            let def = WorkflowDef {
                schema_version: 1,
                name: "no-decisions".into(),
                version: "1.0.0".into(),
                description: String::new(),
                steps: BTreeMap::from([(
                    "only".into(),
                    StepDef {
                        kind: StepKind::Source {
                            source: "steps/only.ax".into(),
                        },
                        capabilities: vec![],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec![],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                        required_capability_versions: Default::default(),
                    },
                )]),
                edges: vec![],
            };
            let json = serde_json::to_string_pretty(&def).unwrap();
            std::fs::write(dir.path().join("workflow.json"), &json).unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            let r = WorkflowRunner::run_persistent(
                &def,
                &RunOptions {
                    policy: Some(Policy::allow_all()),
                    record: false,
                    workflow_dir: dir.path().to_string_lossy().to_string(),
                    live: false,
                    concurrency: 1,
                    submit_only: false,
                },
                data_dir.path(),
            )
            .unwrap();
            assert_eq!(r.status, WorkflowStatus::Completed);

            let output_dir = tempfile::tempdir().unwrap();
            let manifest = create_bundle(data_dir.path(), &r.run_id, output_dir.path()).unwrap();
            let audit_json =
                std::fs::read_to_string(output_dir.path().join(&r.run_id).join("audit_log.json"))
                    .unwrap();
            let audit = AuditLog::from_json(&audit_json).unwrap();
            // Lifecycle events: WorkflowStarted + StepCompleted(only)
            // + WorkflowCompleted = 3 entries.
            assert_eq!(audit.entries().len(), 3);
            assert!(matches!(
                audit.entries()[0].event,
                AuditEvent::WorkflowStarted { .. }
            ));
            assert!(matches!(
                audit.entries()[1].event,
                AuditEvent::StepCompleted { .. }
            ));
            assert!(matches!(
                audit.entries()[2].event,
                AuditEvent::WorkflowCompleted { .. }
            ));
            audit.verify().expect("lifecycle chain must verify");
            // Non-empty chain → audit_log_hash is the last entry's hash,
            // not the all-zeros sentinel.
            assert_ne!(manifest.audit_log_hash, "0".repeat(64));
        }
    }

    // ── Sprint 0.5-S7: output blob reference write-side ──────────────────────

    #[cfg(feature = "persist-sqlite")]
    mod output_blob_routing {
        use super::*;
        use crate::persistence::{BlobStore, RunCheckpointStore, StepStatus as PsStatus};

        fn make_single_step_workflow(
            step_id: &str,
            source_code: &str,
        ) -> (WorkflowDef, tempfile::TempDir) {
            make_workflow_with_steps(&[(step_id, source_code)])
        }

        #[test]
        fn output_under_threshold_stays_inline() {
            let (def, dir) = make_single_step_workflow("step1", "fn main() -> Int { 42 }");
            let data_dir = tempfile::tempdir().unwrap();
            let opts = RunOptions {
                policy: Some(Policy::allow_all()),
                record: false,
                workflow_dir: dir.path().to_string_lossy().to_string(),
                live: false,
                concurrency: 1,
                submit_only: false,
            };
            WorkflowRunner::run_persistent(&def, &opts, data_dir.path()).unwrap();

            let store = RunCheckpointStore::open(&data_dir.path().join("runs.db")).unwrap();
            let runs = store.list_runs().unwrap();
            assert!(!runs.is_empty());
            let cps = store.list_step_checkpoints(&runs[0].run_id).unwrap();
            let cp = cps.iter().find(|c| c.step_id == "step1").unwrap();
            assert_eq!(cp.status, PsStatus::Completed);
            // Small output stays inline; no blob reference.
            assert!(cp.output_json.is_some(), "expected inline output_json");
            assert!(
                cp.output_blob_ref.is_none(),
                "expected no blob ref for small output"
            );
        }

        #[test]
        fn output_over_threshold_stores_as_blob() {
            // Step returns a large String via a fn that constructs it.
            // We craft the .ax source so the *checkpoint* output_json
            // (the serde_json of Value::Int(0)) is tiny, but we test
            // the routing logic directly through the helper instead.
            //
            // Direct helper test: call route_output with a string > threshold
            // and a real BlobStore, verify the blob is written and ref returned.
            let tmp = tempfile::tempdir().unwrap();
            let bs = BlobStore::open(tmp.path().to_path_buf()).unwrap();
            // Build a payload just over the threshold.
            let payload = "a".repeat(crate::persistence::BLOB_THRESHOLD + 1);
            let (out_json, blob_ref) = route_output(payload.clone(), Some(&bs));
            assert!(
                out_json.is_none(),
                "large output should be offloaded, not inline"
            );
            let hash = blob_ref.expect("expected a blob ref");
            assert_eq!(hash.len(), 64, "blob ref must be 64-char hex");
            // The blob must be readable and match the original payload.
            let stored = bs.read_string(&hash).unwrap();
            assert_eq!(stored, payload);
        }

        #[test]
        fn output_over_threshold_falls_back_to_inline_without_blob_store() {
            let payload = "b".repeat(crate::persistence::BLOB_THRESHOLD + 1);
            let (out_json, blob_ref) = route_output(payload.clone(), None);
            assert_eq!(
                out_json.as_deref(),
                Some(payload.as_str()),
                "no blob_store → must stay inline"
            );
            assert!(blob_ref.is_none());
        }

        #[test]
        fn blob_roundtrip_resume() {
            // Write a checkpoint with output_blob_ref set, then read it back
            // via read_step_output and verify the value round-trips.
            let blobs_dir = tempfile::tempdir().unwrap();
            let store =
                RunCheckpointStore::open_in_memory_with_blob_store(blobs_dir.path().to_path_buf())
                    .unwrap();

            use crate::persistence::{RunRow, RunStatus, StepCheckpoint, StepStatus};
            // Insert a minimal run row so FK constraints are satisfied.
            store
                .insert_run(&RunRow {
                    run_id: "R1".into(),
                    workflow_name: "test-wf".into(),
                    workflow_hash: "abc".into(),
                    status: RunStatus::Running,
                    started_at_ms: 0,
                    updated_at_ms: 0,
                    policy_json: "{}".into(),
                    metadata_json: "{}".into(),
                })
                .unwrap();

            // Write the blob manually and record its hash.
            let payload = "c".repeat(crate::persistence::BLOB_THRESHOLD + 1);
            let hash = sha256_hex(&payload);
            store
                .blob_store()
                .unwrap()
                .write(&hash, payload.as_bytes())
                .unwrap();

            // Insert a checkpoint referencing the blob (no inline output_json).
            store
                .upsert_step_checkpoint(&StepCheckpoint {
                    run_id: "R1".into(),
                    step_id: "s1".into(),
                    status: StepStatus::Completed,
                    output_json: None,
                    output_hash: Some(hash.clone()),
                    started_at_ms: None,
                    ended_at_ms: Some(1),
                    error_msg: None,
                    attempt_count: 1,
                    worker_id: None,
                    lease_expires_at_ms: None,
                    claim_id: 0,
                    output_blob_ref: Some(hash.clone()),
                })
                .unwrap();

            // read_step_output must resolve the blob and return the payload.
            let got = store.read_step_output("R1", "s1").unwrap();
            assert_eq!(got.as_deref(), Some(payload.as_str()));
        }
    }
}
