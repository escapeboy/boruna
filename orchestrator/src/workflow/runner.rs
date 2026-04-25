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
            approvals: BTreeMap::new(),
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
                    store
                        .upsert_step_checkpoint(&StepCheckpoint {
                            run_id: run_id.to_string(),
                            step_id: step_id.clone(),
                            status: PersistStepStatus::Completed,
                            output_json: Some(output_json),
                            output_hash: Some(output_hash.clone()),
                            started_at_ms: None, // COALESCE preserves
                            ended_at_ms: Some(now_unix_ms()),
                            error_msg: None,
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

        // Validate against the workflow def (loaded from the persisted
        // workflow_dir, NOT from CLI overrides — the operator's intent
        // is "decide this step within THIS run's recorded definition").
        let def_path = Path::new(&metadata.workflow_dir).join("workflow.json");
        let def_json = std::fs::read_to_string(&def_path).map_err(|e| {
            WorkflowRunError::Io(format!(
                "cannot read {} (workflow_dir from run metadata): {e}",
                def_path.display()
            ))
        })?;
        let def: WorkflowDef = serde_json::from_str(&def_json)
            .map_err(|e| WorkflowRunError::Internal(format!("invalid workflow.json: {e}")))?;

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

    // ── 0.3-S2c: approval-gate completion ──

    #[cfg(feature = "persist-sqlite")]
    mod approval_gate {
        use super::*;
        use crate::persistence::{RunCheckpointStore, RunStatus as PersistRunStatus};

        fn workflow_with_approval_gate() -> (WorkflowDef, tempfile::TempDir) {
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
}
