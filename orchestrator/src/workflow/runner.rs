use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use boruna_vm::capability_gateway::{CapabilityGateway, Policy, PolicyRule};
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
}

/// Options for resuming a previously-paused or crashed workflow run.
///
/// Carries the runtime knobs that may legitimately vary between the
/// original run and the resume — `policy` (operator may have widened or
/// narrowed it), `live` (may have toggled record/replay mode), `record`.
/// `workflow_dir` is read from persisted metadata by default but can be
/// overridden via `workflow_dir_override` for relocated checkouts.
#[cfg(feature = "persist-sqlite")]
#[derive(Debug, Clone, Default)]
pub struct ResumeOptions {
    pub policy: Option<Policy>,
    pub record: bool,
    pub live: bool,
    pub workflow_dir_override: Option<String>,
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedRunMetadata {
    workflow_dir: String,
    inputs_hash: String,
    boruna_version: String,
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
        WorkflowValidator::validate(def).map_err(|errors| {
            WorkflowRunError::Validation(
                errors
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        let order =
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

        // Persistent runs use a stable per-run subdir, not a tempdir, so
        // the data store survives a crash. Caller controls the parent;
        // each run gets its own folder keyed by run_id.
        let run_data_dir = data_dir.join("runs").join(&run_id);
        let mut data_store =
            DataStore::new(&run_data_dir).map_err(|e| WorkflowRunError::Io(e.to_string()))?;

        let result = Self::execute_steps(
            def,
            &order,
            options,
            &run_id,
            &mut data_store,
            BTreeSet::new(),
            &BTreeMap::new(),
            Some(&store),
        );

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
                    if let Some(output_json) = &cp.output_json {
                        let value: boruna_bytecode::Value = serde_json::from_str(output_json)
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
                        },
                    );
                    halt_with_failed_step = Some(cp.step_id.clone());
                }
                // Pending / Running / AwaitingApproval: re-execute. The
                // running-on-resume case is the crash-mid-step scenario:
                // we do NOT trust any partial output. Don't seed
                // already_completed; let execute_steps run them fresh.
                _ => {}
            }
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
        };

        // Reset run status to Running for the resume window.
        store
            .update_run_status(run_id, PersistRunStatus::Running, now_unix_ms())
            .map_err(WorkflowRunError::from)?;

        let result = Self::execute_steps(
            &def,
            &order,
            &synthesized_options,
            run_id,
            &mut data_store,
            already_completed,
            &prior_results,
            Some(&store),
        );

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
                        })
                        .map_err(WorkflowRunError::from)?;
                    }
                    break;
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

                    let outcome: Result<StepResult, WorkflowRunError> = match result {
                        Ok(sr) => Ok(StepResult { duration_ms, ..sr }),
                        Err(e) => {
                            let should_retry = step_def
                                .retry
                                .as_ref()
                                .is_some_and(|r| r.max_attempts > 1 && r.on_transient);
                            if should_retry {
                                Self::execute_source_step(
                                    step_id,
                                    source,
                                    step_def,
                                    &options.workflow_dir,
                                    &options.policy,
                                    data_store,
                                    options.live,
                                )
                                .map(|sr| StepResult { duration_ms, ..sr })
                                .map_err(|retry_err| {
                                    WorkflowRunError::StepFailed(
                                        step_id.clone(),
                                        retry_err.to_string(),
                                    )
                                })
                            } else {
                                Err(WorkflowRunError::StepFailed(step_id.clone(), e.to_string()))
                            }
                        }
                    };

                    match outcome {
                        Ok(sr) => {
                            #[cfg(feature = "persist-sqlite")]
                            if let Some(s) = store {
                                let output_json =
                                    Self::lookup_output_json(data_store, step_id, "result")?;
                                s.upsert_step_checkpoint(&StepCheckpoint {
                                    run_id: run_id.to_string(),
                                    step_id: step_id.clone(),
                                    status: PersistStepStatus::Completed,
                                    output_json,
                                    output_hash: sr.output_hash.clone(),
                                    started_at_ms: None, // COALESCE preserves
                                    ended_at_ms: Some(now_unix_ms()),
                                    error_msg: None,
                                })
                                .map_err(WorkflowRunError::from)?;
                            }
                            step_results.insert(step_id.clone(), sr);
                        }
                        Err(e) => {
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
                                    error_msg: Some(err_msg),
                                })
                                .map_err(WorkflowRunError::from)?;
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
    ) -> Result<StepResult, WorkflowRunError> {
        // Read source file
        let source_path = Path::new(workflow_dir).join(source);
        let source_code = std::fs::read_to_string(&source_path).map_err(|e| {
            WorkflowRunError::StepFailed(
                step_id.to_string(),
                format!("cannot read {}: {e}", source_path.display()),
            )
        })?;

        // Resolve inputs (validated, available for future step parameter injection)
        let _resolved_inputs = data_store
            .resolve_step_inputs(&step_def.inputs)
            .map_err(|e| WorkflowRunError::StepFailed(step_id.to_string(), e))?;

        // Compile
        let module = boruna_compiler::compile(step_id, &source_code).map_err(|e| {
            WorkflowRunError::StepFailed(step_id.to_string(), format!("compile error: {e}"))
        })?;

        // Build policy for this step
        let step_policy = Self::build_step_policy(policy, step_def);

        // Create VM and run — use HttpHandler when live mode is enabled
        let gateway = if live {
            #[cfg(feature = "http")]
            {
                let net_policy = step_policy.net_policy.clone().unwrap_or_default();
                CapabilityGateway::with_handler(
                    step_policy,
                    Box::new(boruna_vm::http_handler::HttpHandler::new(net_policy)),
                )
            }
            #[cfg(not(feature = "http"))]
            {
                eprintln!(
                    "warning: --live requires the `http` feature; falling back to mock handler"
                );
                CapabilityGateway::new(step_policy)
            }
        } else {
            CapabilityGateway::new(step_policy)
        };
        let mut vm = Vm::new(module, gateway);

        let result = vm.run().map_err(|e| {
            WorkflowRunError::StepFailed(step_id.to_string(), format!("runtime error: {e}"))
        })?;

        // Store output
        let output_hash = DataStore::hash_value(&result);
        data_store
            .store_output(step_id, "result", &result)
            .map_err(|e| WorkflowRunError::StepFailed(step_id.to_string(), e.to_string()))?;

        // Collect capabilities used from event log
        let caps_used: Vec<String> = step_def.capabilities.clone();

        Ok(StepResult {
            step_id: step_id.to_string(),
            status: StepStatus::Completed,
            output_hash: Some(output_hash),
            duration_ms: 0, // filled in by caller
            capabilities_used: caps_used,
            error: None,
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
                policy
            }
            None => Policy::deny_all(),
        }
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
        };

        let result = WorkflowRunner::run(&def, &options).unwrap();
        assert_eq!(result.status, WorkflowStatus::Completed);
        assert_eq!(result.step_results.len(), 3);
        for (_, sr) in &result.step_results {
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
                },
            )]),
            edges: vec![],
        };

        let options = RunOptions {
            policy: Some(Policy::allow_all()),
            record: false,
            workflow_dir: dir.path().to_string_lossy().to_string(),
            live: false,
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
        };
        assert!(WorkflowRunner::run(&def, &options).is_err());
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
    }
}
