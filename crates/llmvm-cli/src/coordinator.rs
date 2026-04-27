//! Distributed-execution coordinator HTTP server (sprint
//! `0.5-S2b`). Wraps the persistence-layer claim/lease state
//! machine from `0.5-S2a` in an HTTP protocol so remote workers
//! can claim work over the wire.
//!
//! See `docs/design-coordinator-worker-http.md` and
//! `docs/architecture-coordinator-worker-http.md` for the
//! design rationale and wire format.
//!
//! ## Security posture
//!
//! - Loopback (`127.0.0.1`) by default. `--bind 0.0.0.0` emits a
//!   loud stderr warning and includes `bind_warning` in any
//!   future dashboard banner.
//! - **No authentication.** Operators exposing the coordinator
//!   to a network MUST front it with an auth-enforcing reverse
//!   proxy. Mutations are possible — this is a stronger
//!   warning than the dashboard's read-only one.
//! - Output payload size is capped at 8 MiB per ADR 002.
//! - Workers must match the coordinator's `capability_set_hash`
//!   (atomic-upgrade rule from ADR 002).
//!
//! ## Protocol
//!
//! Every response carries `protocol_version: 1`. Failure
//! responses also carry `error_kind: "<stable_string>"` from
//! the locked `coord.*` taxonomy.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use boruna_bytecode::compute_capability_set_hash;
use boruna_orchestrator::persistence::{
    ClaimOutcome, ExtendOutcome, RunCheckpointStore, RunStatus, StepStatus, TerminalOutcome,
};
use serde::{Deserialize, Serialize};

const PROTOCOL_VERSION: u32 = 1;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct CoordinatorState {
    store: Arc<Mutex<RunCheckpointStore>>,
    workers: Arc<Mutex<HashMap<String, WorkerSession>>>,
    /// Reserved for 0.5-S2c: maps run_id → workflow_dir on disk
    /// so the coordinator can resolve `.ax` source paths from the
    /// run metadata. Today the MVP gets sources inline via
    /// `metadata_json.step_sources`.
    #[allow(dead_code)]
    workflow_dirs: Arc<Mutex<HashMap<String, PathBuf>>>,
    capability_set_hash: String,
    config: CoordinatorConfig,
}

#[derive(Clone)]
struct WorkerSession {
    session_token: String,
    last_heartbeat_ms: i64,
    /// Captured at registration; reserved for future
    /// rolling-upgrade detection (per ADR 002 open question 5).
    #[allow(dead_code)]
    capability_set_hash: String,
}

#[derive(Clone)]
pub struct CoordinatorConfig {
    pub max_lease_ttl_ms: u64,
    pub poll_timeout_ms: u64,
    /// Forwarded to the merged dashboard's HTML banner so the
    /// red WARNING block appears on coordinator-served pages
    /// when bound to a non-loopback address (sprint 0.5-S2d).
    pub bind_warning: Option<String>,
}

/// Minimum sweep interval. Lower values would cause the
/// background task to busy-loop. 100 ms is fast enough for
/// integration tests; production operators pick something
/// larger via `--sweep-interval-ms`.
const MIN_SWEEP_INTERVAL_MS: u64 = 100;

#[tokio::main]
#[allow(clippy::too_many_arguments)]
pub async fn run_serve(
    data_dir: PathBuf,
    port: u16,
    bind: IpAddr,
    max_lease_ttl_ms: u64,
    poll_timeout_ms: u64,
    sweep_interval_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = data_dir.join("runs.db");
    if !db_path.exists() {
        return Err(format!(
            "no runs.db at {} — run a workflow first or pass a different --data-dir",
            db_path.display()
        )
        .into());
    }

    let store = RunCheckpointStore::open(&db_path)
        .map_err(|e| format!("failed to open {}: {e}", db_path.display()))?;

    // On startup, sweep stale leases. Per ADR 002 ("coordinator
    // restart = all leases void"), any row in `Running` status is
    // a leftover from a prior coordinator process; void its lease
    // and re-enqueue.
    let now_ms = now_unix_ms();
    let n = store
        .expire_leases_and_requeue(now_ms + 1)
        .map_err(|e| format!("startup lease sweep failed: {e}"))?;
    if n > 0 {
        eprintln!("coordinator startup: requeued {n} stale-lease step(s)");
    }

    let bind_warning = if bind.is_loopback() {
        None
    } else {
        let msg = format!("{bind}:{port}");
        eprintln!(
            "[WARNING] coordinator bound to non-loopback {msg}; \
             anyone with network access can SUBMIT and CONTROL distributed work; \
             the coordinator ships no auth — front it with an auth-enforcing reverse proxy"
        );
        Some(msg)
    };

    let capability_set_hash = compute_capability_set_hash(
        boruna_bytecode::Capability::ALL
            .iter()
            .map(|c| (c.name().to_string(), c.version().to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str())),
    );

    let state = CoordinatorState {
        store: Arc::new(Mutex::new(store)),
        workers: Arc::new(Mutex::new(HashMap::new())),
        workflow_dirs: Arc::new(Mutex::new(HashMap::new())),
        capability_set_hash,
        config: CoordinatorConfig {
            max_lease_ttl_ms,
            poll_timeout_ms,
            bind_warning,
        },
    };

    // Background lease-expiry sweep (sprint 0.5-S2c). Wakes
    // up every `sweep_interval_ms`, calls
    // `expire_leases_and_requeue`. Logs only when a non-zero
    // number of leases were requeued.
    //
    // Without this loop, the coordinator's startup sweep is
    // the ONLY recovery from a worker crash — operators
    // would have to restart the coordinator process to
    // unstick a stranded step.
    let effective_sweep_ms = sweep_interval_ms.max(MIN_SWEEP_INTERVAL_MS);
    if sweep_interval_ms < MIN_SWEEP_INTERVAL_MS {
        eprintln!(
            "[WARNING] --sweep-interval-ms {sweep_interval_ms} below minimum \
             {MIN_SWEEP_INTERVAL_MS}; using {effective_sweep_ms} ms"
        );
    }
    let sweep_state = state.clone();
    let sweep_task = tokio::spawn(background_sweep_loop(sweep_state, effective_sweep_ms));

    let app = build_router(state);

    let addr = std::net::SocketAddr::new(bind, port);
    eprintln!("coordinator serving on http://{addr}");
    eprintln!("    data-dir: {}", data_dir.display());
    eprintln!("    max_lease_ttl_ms: {max_lease_ttl_ms}");
    eprintln!("    poll_timeout_ms: {poll_timeout_ms}");
    eprintln!("    sweep_interval_ms: {effective_sweep_ms}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let result = axum::serve(listener, app).await;
    sweep_task.abort();
    result?;
    Ok(())
}

/// Background lease-expiry sweep task. Runs for the lifetime
/// of the coordinator process; aborted when `axum::serve`
/// exits.
///
/// Failure semantics: best-effort. Errors log + continue to
/// the next tick. The HTTP server keeps running even if the
/// sweep panics — operators monitor stderr to notice
/// unrecovered failures.
async fn background_sweep_loop(state: CoordinatorState, interval_ms: u64) {
    let mut tick = tokio::time::interval(Duration::from_millis(interval_ms));
    // First tick fires immediately; skip it (the startup
    // sweep already ran).
    tick.tick().await;
    // Track poison-mutex state so we log once instead of
    // silently skipping forever (adversarial-review F2).
    let mut poison_logged = false;
    loop {
        tick.tick().await;
        // `now_ms + 1` matches the startup sweep's threshold
        // (line ~109) so the boundary `lease_expires_at == now_ms`
        // is treated as expired by both code paths
        // (adversarial-review F1).
        let now_ms = now_unix_ms();
        let result = {
            let store = match state.store.lock() {
                Ok(g) => g,
                Err(_) => {
                    if !poison_logged {
                        eprintln!(
                            "coordinator sweep: store mutex poisoned; \
                             background sweep is now silently skipping ticks. \
                             A handler panicked while holding the lock — \
                             investigate stderr for the original panic."
                        );
                        poison_logged = true;
                    }
                    continue;
                }
            };
            store.expire_leases_and_requeue(now_ms + 1)
        };
        match result {
            Ok(0) => {} // no-op tick; quiet
            Ok(n) => {
                eprintln!("coordinator sweep: requeued {n} expired-lease step(s)")
            }
            Err(e) => {
                eprintln!("coordinator sweep: error {e} — retrying next tick")
            }
        }
    }
}

pub fn build_router(state: CoordinatorState) -> Router {
    // Sprint 0.5-S2d: merge the dashboard's read-only routes
    // (/, /runs/:id, /api/runs, /api/runs/:id) onto the
    // coordinator's listener so operators get fleet visibility
    // + distributed dispatch from a single port.
    //
    // Route paths don't overlap by design (per ADR 002): the
    // coordinator owns /api/work/* and /api/workers/*; the
    // dashboard owns /api/runs and /api/runs/:id. The HTML
    // routes (/ and /runs/:id) are dashboard-only.
    //
    // The coordinator's bind_warning flows into the dashboard
    // builder so the red HTML banner appears on coordinator-
    // served pages too.
    let dashboard_router =
        crate::dashboard::dashboard_routes(state.store.clone(), state.config.bind_warning.clone());
    let coord_router = Router::new()
        .route("/api/workers/register", post(handle_register))
        .route("/api/workers/heartbeat", post(handle_heartbeat))
        .route("/api/work/claim", get(handle_claim))
        .route("/api/work/complete", post(handle_complete))
        .route("/api/work/fail", post(handle_fail))
        .route("/api/work/extend-lease", post(handle_extend_lease))
        // The 8 MiB DefaultBodyLimit applies to coord routes
        // ONLY (not dashboard routes) because Axum's per-
        // router layer scoping means layers attached pre-merge
        // stay bound to their own routes. Dashboard is
        // GET-only today, so no body-limit need. If a future
        // sprint adds a mutating dashboard route (e.g. "cancel
        // run"), it must opt into a body limit explicitly OR
        // be added to coord_router instead.
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state);
    coord_router.merge(dashboard_router)
}

// ── Wire shapes ──

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RegisterRequest {
    #[serde(default)]
    pub worker_id: Option<String>,
    pub capability_set_hash: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RegisterResponse {
    pub protocol_version: u32,
    pub worker_id: String,
    pub session_token: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HeartbeatRequest {
    pub worker_id: String,
    pub session_token: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OkResponse {
    pub protocol_version: u32,
    pub ok: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ClaimQuery {
    pub worker_id: String,
    pub session_token: String,
    pub lease_ttl_ms: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorkItem {
    pub protocol_version: u32,
    pub run_id: String,
    pub step_id: String,
    pub claim_id: u64,
    pub lease_expires_at_ms: i64,
    pub source: String,
    pub policy_json: String,
    #[serde(default)]
    pub inputs_json: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CompleteRequest {
    pub worker_id: String,
    pub session_token: String,
    pub run_id: String,
    pub step_id: String,
    pub claim_id: u64,
    pub output_json: String,
    pub output_hash: String,
    pub attempt_count: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FailRequest {
    pub worker_id: String,
    pub session_token: String,
    pub run_id: String,
    pub step_id: String,
    pub claim_id: u64,
    pub error_msg: String,
    pub attempt_count: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExtendLeaseRequest {
    pub worker_id: String,
    pub session_token: String,
    pub run_id: String,
    pub step_id: String,
    pub claim_id: u64,
    pub extend_by_ms: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExtendLeaseResponse {
    pub protocol_version: u32,
    pub new_lease_expires_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ErrorBody {
    pub protocol_version: u32,
    pub error_kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_claim_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
}

impl ErrorBody {
    fn new(error_kind: &str, message: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            error_kind: error_kind.into(),
            message: message.into(),
            current_claim_id: None,
            current_status: None,
            expected_hash: None,
            max_bytes: None,
        }
    }
}

fn respond_err(status: StatusCode, body: ErrorBody) -> Response {
    (status, Json(body)).into_response()
}

// ── Handlers ──

async fn handle_register(
    State(state): State<CoordinatorState>,
    Json(req): Json<RegisterRequest>,
) -> Response {
    if req.capability_set_hash != state.capability_set_hash {
        let mut body = ErrorBody::new(
            "coord.binary_mismatch",
            format!(
                "worker hash {:?} does not match coordinator's {:?}",
                req.capability_set_hash, state.capability_set_hash
            ),
        );
        body.expected_hash = Some(state.capability_set_hash.clone());
        return respond_err(StatusCode::CONFLICT, body);
    }

    let worker_id = req
        .worker_id
        .unwrap_or_else(|| format!("wkr-{}", uuid::Uuid::new_v4().simple()));
    let session_token = format!("sess-{}", uuid::Uuid::new_v4().simple());

    {
        let mut workers = match state.workers.lock() {
            Ok(g) => g,
            Err(_) => return internal_error("workers lock poisoned"),
        };
        workers.insert(
            worker_id.clone(),
            WorkerSession {
                session_token: session_token.clone(),
                last_heartbeat_ms: now_unix_ms(),
                capability_set_hash: req.capability_set_hash,
            },
        );
    }

    Json(RegisterResponse {
        protocol_version: PROTOCOL_VERSION,
        worker_id,
        session_token,
    })
    .into_response()
}

async fn handle_heartbeat(
    State(state): State<CoordinatorState>,
    Json(req): Json<HeartbeatRequest>,
) -> Response {
    let mut workers = match state.workers.lock() {
        Ok(g) => g,
        Err(_) => return internal_error("workers lock poisoned"),
    };
    match workers.get_mut(&req.worker_id) {
        Some(sess) if sess.session_token == req.session_token => {
            sess.last_heartbeat_ms = now_unix_ms();
            Json(OkResponse {
                protocol_version: PROTOCOL_VERSION,
                ok: true,
            })
            .into_response()
        }
        _ => respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new(
                "coord.unknown_worker",
                format!("worker {} not registered; re-register", req.worker_id),
            ),
        ),
    }
}

async fn handle_claim(
    State(state): State<CoordinatorState>,
    Query(q): Query<ClaimQuery>,
) -> Response {
    // Validate worker session.
    {
        let workers = match state.workers.lock() {
            Ok(g) => g,
            Err(_) => return internal_error("workers lock poisoned"),
        };
        match workers.get(&q.worker_id) {
            Some(sess) if sess.session_token == q.session_token => {}
            _ => {
                return respond_err(
                    StatusCode::NOT_FOUND,
                    ErrorBody::new(
                        "coord.unknown_worker",
                        format!("worker {} not registered; re-register", q.worker_id),
                    ),
                );
            }
        }
    }

    // Cap lease TTL at coordinator config.
    let lease_ttl_ms = q.lease_ttl_ms.min(state.config.max_lease_ttl_ms);
    let poll_timeout = Duration::from_millis(state.config.poll_timeout_ms);
    let poll_interval = Duration::from_millis(250);
    let deadline = std::time::Instant::now() + poll_timeout;

    loop {
        match try_claim_one(&state, &q.worker_id, lease_ttl_ms) {
            Ok(Some(item)) => return Json(item).into_response(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    return StatusCode::NO_CONTENT.into_response();
                }
                tokio::time::sleep(poll_interval).await;
            }
            Err(resp) => return resp,
        }
    }
}

/// Find one claimable Pending step in any run, atomically claim
/// it, and return the work item. Returns Ok(None) if nothing
/// claimable.
// `Response` is a fairly large enum; clippy's `result_large_err`
// fires here. Acceptable: this Err is the slow path and we
// don't want to box a heap allocation for every successful claim.
#[allow(clippy::result_large_err)]
fn try_claim_one(
    state: &CoordinatorState,
    worker_id: &str,
    lease_ttl_ms: u64,
) -> Result<Option<WorkItem>, Response> {
    let now_ms = now_unix_ms();
    let lease_expires_at_ms = now_ms + lease_ttl_ms as i64;

    // Look up a Pending step. We do this in two phases to keep
    // the lock-hold short: first find the (run_id, step_id),
    // then call claim_step which has its own atomic CAS.
    let candidate = {
        let store = state
            .store
            .lock()
            .map_err(|_| internal_error("store lock poisoned"))?;
        find_one_pending_step(&store).map_err(|e| internal_error(&format!("scan pending: {e}")))?
    };

    let Some((run_id, step_id, source, policy_json)) = candidate else {
        return Ok(None);
    };

    let claim_id = {
        let store = state
            .store
            .lock()
            .map_err(|_| internal_error("store lock poisoned"))?;
        match store
            .claim_step(&run_id, &step_id, worker_id, lease_expires_at_ms, now_ms)
            .map_err(|e| internal_error(&format!("claim_step: {e}")))?
        {
            ClaimOutcome::Claimed { claim_id } => claim_id,
            ClaimOutcome::NotClaimable { .. } | ClaimOutcome::StepNotFound => {
                // Race: someone else claimed between our SELECT and
                // claim_step. The caller's loop will retry.
                return Ok(None);
            }
        }
    };

    Ok(Some(WorkItem {
        protocol_version: PROTOCOL_VERSION,
        run_id,
        step_id,
        claim_id,
        lease_expires_at_ms,
        source,
        policy_json,
        inputs_json: None,
    }))
}

/// Scan runs.db for one Pending step. Returns
/// `(run_id, step_id, source, policy_json)`. The `source` is
/// resolved from the workflow_dir map populated at startup OR
/// inline in the run's metadata_json. For this MVP we use a
/// simple convention: metadata_json optionally carries
/// `step_sources: { "<step_id>": "<.ax source>" }`. This keeps
/// the test surface simple while leaving room for the future
/// `boruna workflow run --coordinator` to populate it from
/// workflow_dir.
/// Tuple of `(run_id, step_id, source, policy_json)` returned
/// by [`find_one_pending_step`] when a claimable step exists.
type PendingStepDescriptor = (String, String, String, String);

fn find_one_pending_step(
    store: &RunCheckpointStore,
) -> Result<Option<PendingStepDescriptor>, Box<dyn std::error::Error>> {
    let runs = store.list_runs_by_status(RunStatus::Running)?;
    for run in runs {
        let steps = store.list_step_checkpoints(&run.run_id)?;
        for step in steps {
            if step.status == StepStatus::Pending {
                let source = extract_step_source(&run.metadata_json, &step.step_id)
                    .ok_or_else(|| format!(
                        "step {} in run {} has no inline source; metadata_json.step_sources missing",
                        step.step_id, run.run_id
                    ))?;
                return Ok(Some((run.run_id, step.step_id, source, run.policy_json)));
            }
        }
    }
    Ok(None)
}

/// Pull the step's `.ax` source from the run's metadata JSON.
/// Convention for the MVP: `metadata_json` looks like
/// `{ "step_sources": { "extract": "fn main()..." } }`.
fn extract_step_source(metadata_json: &str, step_id: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(metadata_json).ok()?;
    v.get("step_sources")?
        .get(step_id)?
        .as_str()
        .map(String::from)
}

async fn handle_complete(
    State(state): State<CoordinatorState>,
    Json(req): Json<CompleteRequest>,
) -> Response {
    if let Err(resp) = validate_session(&state, &req.worker_id, &req.session_token) {
        return resp;
    }
    let store = match state.store.lock() {
        Ok(g) => g,
        Err(_) => return internal_error("store lock poisoned"),
    };
    let now_ms = now_unix_ms();
    let outcome = match store.complete_step_cas(
        &req.run_id,
        &req.step_id,
        req.claim_id,
        &req.output_json,
        &req.output_hash,
        req.attempt_count,
        now_ms,
    ) {
        Ok(o) => o,
        Err(e) => return internal_error(&format!("complete_step_cas: {e}")),
    };
    drop(store);
    terminal_outcome_to_response(outcome)
}

async fn handle_fail(
    State(state): State<CoordinatorState>,
    Json(req): Json<FailRequest>,
) -> Response {
    if let Err(resp) = validate_session(&state, &req.worker_id, &req.session_token) {
        return resp;
    }
    let store = match state.store.lock() {
        Ok(g) => g,
        Err(_) => return internal_error("store lock poisoned"),
    };
    let now_ms = now_unix_ms();
    let outcome = match store.fail_step_cas(
        &req.run_id,
        &req.step_id,
        req.claim_id,
        &req.error_msg,
        req.attempt_count,
        now_ms,
    ) {
        Ok(o) => o,
        Err(e) => return internal_error(&format!("fail_step_cas: {e}")),
    };
    drop(store);
    terminal_outcome_to_response(outcome)
}

async fn handle_extend_lease(
    State(state): State<CoordinatorState>,
    Json(req): Json<ExtendLeaseRequest>,
) -> Response {
    if let Err(resp) = validate_session(&state, &req.worker_id, &req.session_token) {
        return resp;
    }
    // Adversarial-review F4: enforce a floor so a worker that
    // mistakenly passes `extend_by_ms: 0` doesn't get a
    // 200-OK with a lease deadline that's already in the past.
    // 1s is the minimum reasonable extension; below that the
    // round-trip latency itself dominates.
    const MIN_EXTEND_MS: u64 = 1_000;
    if req.extend_by_ms < MIN_EXTEND_MS {
        return respond_err(
            StatusCode::BAD_REQUEST,
            ErrorBody::new(
                "coord.invalid_request",
                format!(
                    "extend_by_ms {} below minimum {} ms",
                    req.extend_by_ms, MIN_EXTEND_MS
                ),
            ),
        );
    }
    let extend_by_ms = req.extend_by_ms.min(state.config.max_lease_ttl_ms);
    let new_lease_expires_at_ms = now_unix_ms() + extend_by_ms as i64;
    let store = match state.store.lock() {
        Ok(g) => g,
        Err(_) => return internal_error("store lock poisoned"),
    };
    let outcome = match store.extend_lease_cas(
        &req.run_id,
        &req.step_id,
        req.claim_id,
        new_lease_expires_at_ms,
    ) {
        Ok(o) => o,
        Err(e) => return internal_error(&format!("extend_lease_cas: {e}")),
    };
    drop(store);
    match outcome {
        ExtendOutcome::Extended {
            new_lease_expires_at_ms,
        } => Json(ExtendLeaseResponse {
            protocol_version: PROTOCOL_VERSION,
            new_lease_expires_at_ms,
        })
        .into_response(),
        ExtendOutcome::LeaseExpired {
            current_claim_id,
            current_status,
        } => {
            let mut body = ErrorBody::new(
                "coord.lease_expired",
                format!(
                    "lease for step has claim_id={current_claim_id} status={}",
                    current_status.as_str()
                ),
            );
            body.current_claim_id = Some(current_claim_id);
            body.current_status = Some(current_status.as_str().to_string());
            respond_err(StatusCode::CONFLICT, body)
        }
        ExtendOutcome::StepNotFound => respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new("coord.step_not_found", "step not found"),
        ),
    }
}

fn terminal_outcome_to_response(outcome: TerminalOutcome) -> Response {
    match outcome {
        TerminalOutcome::Committed => Json(OkResponse {
            protocol_version: PROTOCOL_VERSION,
            ok: true,
        })
        .into_response(),
        TerminalOutcome::LeaseExpired {
            current_claim_id,
            current_status,
        } => {
            let mut body = ErrorBody::new(
                "coord.lease_expired",
                format!(
                    "step has claim_id={current_claim_id} status={}",
                    current_status.as_str()
                ),
            );
            body.current_claim_id = Some(current_claim_id);
            body.current_status = Some(current_status.as_str().to_string());
            respond_err(StatusCode::CONFLICT, body)
        }
        TerminalOutcome::StepNotFound => respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new("coord.step_not_found", "step not found"),
        ),
    }
}

// Same `result_large_err` justification as `try_claim_one`.
#[allow(clippy::result_large_err)]
fn validate_session(
    state: &CoordinatorState,
    worker_id: &str,
    session_token: &str,
) -> Result<(), Response> {
    let workers = state
        .workers
        .lock()
        .map_err(|_| internal_error("workers lock poisoned"))?;
    match workers.get(worker_id) {
        Some(sess) if sess.session_token == session_token => Ok(()),
        _ => Err(respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new(
                "coord.unknown_worker",
                format!("worker {worker_id} not registered; re-register"),
            ),
        )),
    }
}

fn internal_error(msg: &str) -> Response {
    let body = ErrorBody::new("coord.invalid_request", msg);
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// Helper accessors for tests / future dashboard merge.
#[allow(dead_code)]
impl CoordinatorState {
    pub fn store_handle(&self) -> Arc<Mutex<RunCheckpointStore>> {
        self.store.clone()
    }
    pub fn bind_warning(&self) -> Option<&str> {
        self.config.bind_warning.as_deref()
    }
}

// ── coordinator wait (sprint 0.5-S2f) ──

/// Minimum poll interval for the wait loop. Mirrors
/// `MIN_SWEEP_INTERVAL_MS` — sub-100 ms polling is allowed only as
/// a test/operator override, with a clamping warning.
const MIN_WAIT_POLL_INTERVAL_MS: u64 = 100;

/// Drive a submit-only run to terminal status by computing
/// downstream-ready successors via
/// [`boruna_orchestrator::workflow::WorkflowRunner::advance_run_one_tick`]
/// every `poll_interval_ms`. Sprint `0.5-S2f`.
///
/// Returns the intended process exit code:
/// - `0` — run reached `Completed`.
/// - `1` — run reached `Failed`.
/// - `2` — `--max-wait-secs` exceeded.
///
/// This function is synchronous (no tokio runtime); the wait loop is
/// driven by `std::thread::sleep` between ticks. The advance call
/// itself is short-lived (a SQLite read + a few small writes).
pub fn run_wait(
    data_dir: PathBuf,
    run_id: String,
    poll_interval_ms: u64,
    max_wait_secs: u64,
) -> Result<i32, Box<dyn std::error::Error>> {
    use boruna_orchestrator::workflow::{AdvanceRunStatus, WorkflowRunner};

    let db_path = data_dir.join("runs.db");
    if !db_path.exists() {
        return Err(format!(
            "no runs.db at {} — pass --data-dir matching the coordinator process",
            db_path.display()
        )
        .into());
    }
    let store = RunCheckpointStore::open(&db_path)
        .map_err(|e| format!("failed to open {}: {e}", db_path.display()))?;

    let effective_poll_ms = poll_interval_ms.max(MIN_WAIT_POLL_INTERVAL_MS);
    if poll_interval_ms < MIN_WAIT_POLL_INTERVAL_MS {
        eprintln!(
            "[WARNING] --poll-interval-ms {poll_interval_ms} below minimum \
             {MIN_WAIT_POLL_INTERVAL_MS}; using {effective_poll_ms} ms"
        );
    }

    eprintln!("coordinator wait run_id={run_id}");
    eprintln!("    data-dir: {}", data_dir.display());
    eprintln!("    poll-interval-ms: {effective_poll_ms}");
    if max_wait_secs > 0 {
        eprintln!("    max-wait-secs: {max_wait_secs}");
    }

    // Track previous step statuses so we only print transitions, not
    // the entire status map every tick.
    let mut prev: BTreeMap<String, String> = BTreeMap::new();
    let started = std::time::Instant::now();

    loop {
        let result = match WorkflowRunner::advance_run_one_tick(&store, &run_id) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                return Ok(2);
            }
        };
        // Print step transitions in sorted order (BTreeMap iteration).
        // newly_pending is always reflected in all_step_statuses (see
        // advance_run_one_tick: status_map is updated alongside the
        // newly_pending push), so this single loop covers all
        // transitions.
        for (sid, status) in &result.all_step_statuses {
            let status_str = persist_status_str(*status).to_string();
            match prev.get(sid) {
                Some(p) if p == &status_str => {}
                _ => {
                    println!("step {sid}: {status_str}");
                    prev.insert(sid.clone(), status_str);
                }
            }
        }
        match result.run_status {
            AdvanceRunStatus::Completed => {
                println!("run {run_id}: completed");
                return Ok(0);
            }
            AdvanceRunStatus::Failed => {
                println!("run {run_id}: failed");
                return Ok(1);
            }
            AdvanceRunStatus::Running => {}
        }
        if max_wait_secs > 0 && started.elapsed().as_secs() >= max_wait_secs {
            eprintln!("error: --max-wait-secs {max_wait_secs} exceeded");
            return Ok(3);
        }
        std::thread::sleep(std::time::Duration::from_millis(effective_poll_ms));
    }
}

fn persist_status_str(s: boruna_orchestrator::persistence::StepStatus) -> &'static str {
    use boruna_orchestrator::persistence::StepStatus;
    match s {
        StepStatus::Pending => "pending",
        StepStatus::Running => "running",
        StepStatus::Completed => "completed",
        StepStatus::Failed => "failed",
        StepStatus::AwaitingApproval => "awaiting_approval",
        StepStatus::AwaitingExternalEvent => "awaiting_external_event",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use boruna_orchestrator::persistence::{RunRow, StepCheckpoint};
    use tower::ServiceExt;

    fn fresh_state() -> CoordinatorState {
        let store = RunCheckpointStore::open_in_memory().unwrap();
        let capability_set_hash = compute_capability_set_hash(
            boruna_bytecode::Capability::ALL
                .iter()
                .map(|c| (c.name().to_string(), c.version().to_string()))
                .collect::<Vec<_>>()
                .iter()
                .map(|(n, v)| (n.as_str(), v.as_str())),
        );
        CoordinatorState {
            store: Arc::new(Mutex::new(store)),
            workers: Arc::new(Mutex::new(HashMap::new())),
            workflow_dirs: Arc::new(Mutex::new(HashMap::new())),
            capability_set_hash,
            config: CoordinatorConfig {
                max_lease_ttl_ms: 600_000,
                poll_timeout_ms: 200,
                bind_warning: None,
            },
        }
    }

    fn pending_step(state: &CoordinatorState, run_id: &str, step_id: &str, source: &str) {
        let metadata_json = serde_json::json!({
            "step_sources": { step_id: source }
        })
        .to_string();
        let store = state.store.lock().unwrap();
        let _ = store.insert_run(&RunRow {
            run_id: run_id.into(),
            workflow_name: "wf".into(),
            workflow_hash: "h".into(),
            status: RunStatus::Running,
            started_at_ms: 0,
            updated_at_ms: 0,
            policy_json: r#"{"default_allow":true}"#.into(),
            metadata_json,
        });
        store
            .upsert_step_checkpoint(&StepCheckpoint {
                run_id: run_id.into(),
                step_id: step_id.into(),
                status: StepStatus::Pending,
                output_json: None,
                output_hash: None,
                started_at_ms: None,
                ended_at_ms: None,
                error_msg: None,
                attempt_count: 1,
                worker_id: None,
                lease_expires_at_ms: None,
                claim_id: 0,
            })
            .unwrap();
    }

    async fn post_json<T: Serialize>(
        app: &Router,
        path: &str,
        body: &T,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    fn register_payload(hash: String) -> RegisterRequest {
        RegisterRequest {
            worker_id: None,
            capability_set_hash: hash,
        }
    }

    #[tokio::test]
    async fn register_allocates_worker_id() {
        let state = fresh_state();
        let app = build_router(state.clone());
        let (status, v) = post_json(
            &app,
            "/api/workers/register",
            &register_payload(state.capability_set_hash.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(v["worker_id"].as_str().unwrap().starts_with("wkr-"));
        assert!(v["session_token"].as_str().unwrap().starts_with("sess-"));
        assert_eq!(v["protocol_version"], 1);
    }

    #[tokio::test]
    async fn register_accepts_caller_supplied_worker_id() {
        let state = fresh_state();
        let app = build_router(state.clone());
        let req = RegisterRequest {
            worker_id: Some("custom-host-7".into()),
            capability_set_hash: state.capability_set_hash.clone(),
        };
        let (status, v) = post_json(&app, "/api/workers/register", &req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["worker_id"], "custom-host-7");
    }

    #[tokio::test]
    async fn register_rejects_binary_mismatch() {
        let state = fresh_state();
        let app = build_router(state);
        let (status, v) = post_json(
            &app,
            "/api/workers/register",
            &register_payload("sha256:bogus".into()),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(v["error_kind"], "coord.binary_mismatch");
        assert!(v["expected_hash"].is_string());
        assert_eq!(v["protocol_version"], 1);
    }

    #[tokio::test]
    async fn heartbeat_unknown_worker_returns_404() {
        let state = fresh_state();
        let app = build_router(state);
        let (status, v) = post_json(
            &app,
            "/api/workers/heartbeat",
            &HeartbeatRequest {
                worker_id: "ghost".into(),
                session_token: "x".into(),
            },
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(v["error_kind"], "coord.unknown_worker");
    }

    #[tokio::test]
    async fn claim_returns_204_when_no_pending_work() {
        let state = fresh_state();
        let app = build_router(state.clone());
        // Register first.
        let (_, reg) = post_json(
            &app,
            "/api/workers/register",
            &register_payload(state.capability_set_hash.clone()),
        )
        .await;
        let worker_id = reg["worker_id"].as_str().unwrap();
        let token = reg["session_token"].as_str().unwrap();
        let req = Request::builder()
            .method("GET")
            .uri(format!(
                "/api/work/claim?worker_id={worker_id}&session_token={token}&lease_ttl_ms=10000"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn claim_returns_work_item_when_pending_step_exists() {
        let state = fresh_state();
        pending_step(&state, "run-1", "extract", "fn main() -> Int { 42 }\n");
        let app = build_router(state.clone());
        let (_, reg) = post_json(
            &app,
            "/api/workers/register",
            &register_payload(state.capability_set_hash.clone()),
        )
        .await;
        let worker_id = reg["worker_id"].as_str().unwrap();
        let token = reg["session_token"].as_str().unwrap();
        let req = Request::builder()
            .method("GET")
            .uri(format!(
                "/api/work/claim?worker_id={worker_id}&session_token={token}&lease_ttl_ms=10000"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let item: WorkItem = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(item.run_id, "run-1");
        assert_eq!(item.step_id, "extract");
        assert_eq!(item.claim_id, 1);
        assert!(item.source.contains("42"));
    }

    #[tokio::test]
    async fn complete_with_stale_claim_returns_409() {
        let state = fresh_state();
        pending_step(&state, "run-c", "s1", "fn main() -> Int { 1 }\n");
        // Manually claim and then simulate expiry+reclaim.
        let store = state.store.lock().unwrap();
        store
            .claim_step("run-c", "s1", "A", 5_000_000_000_000, 0)
            .unwrap();
        store.expire_leases_and_requeue(5_000_000_000_001).unwrap();
        store
            .claim_step("run-c", "s1", "B", 5_000_000_000_002, 0)
            .unwrap();
        drop(store);
        let app = build_router(state.clone());
        // Register a worker so we have a session token.
        let (_, reg) = post_json(
            &app,
            "/api/workers/register",
            &register_payload(state.capability_set_hash.clone()),
        )
        .await;
        // Late completion with stale claim_id=1.
        let (status, v) = post_json(
            &app,
            "/api/work/complete",
            &CompleteRequest {
                worker_id: reg["worker_id"].as_str().unwrap().into(),
                session_token: reg["session_token"].as_str().unwrap().into(),
                run_id: "run-c".into(),
                step_id: "s1".into(),
                claim_id: 1, // stale; current is 2
                output_json: r#""nope""#.into(),
                output_hash: "h".into(),
                attempt_count: 1,
            },
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(v["error_kind"], "coord.lease_expired");
        assert_eq!(v["current_claim_id"], 2);
    }

    #[tokio::test]
    async fn complete_step_not_found_returns_404() {
        let state = fresh_state();
        let app = build_router(state.clone());
        let (_, reg) = post_json(
            &app,
            "/api/workers/register",
            &register_payload(state.capability_set_hash.clone()),
        )
        .await;
        let (status, v) = post_json(
            &app,
            "/api/work/complete",
            &CompleteRequest {
                worker_id: reg["worker_id"].as_str().unwrap().into(),
                session_token: reg["session_token"].as_str().unwrap().into(),
                run_id: "ghost".into(),
                step_id: "ghost".into(),
                claim_id: 1,
                output_json: "0".into(),
                output_hash: "h".into(),
                attempt_count: 1,
            },
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(v["error_kind"], "coord.step_not_found");
    }

    #[tokio::test]
    async fn extend_lease_rejects_below_minimum() {
        // Adversarial-review F4 regression: extend_by_ms below
        // the 1s floor returns 400 + coord.invalid_request.
        let state = fresh_state();
        pending_step(&state, "run-e0", "s1", "fn main() -> Int { 1 }\n");
        let app = build_router(state.clone());
        let (_, reg) = post_json(
            &app,
            "/api/workers/register",
            &register_payload(state.capability_set_hash.clone()),
        )
        .await;
        let worker_id = reg["worker_id"].as_str().unwrap();
        let token = reg["session_token"].as_str().unwrap();
        // Claim first to set up a valid lease.
        let req = Request::builder()
            .method("GET")
            .uri(format!(
                "/api/work/claim?worker_id={worker_id}&session_token={token}&lease_ttl_ms=10000"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let item: WorkItem = serde_json::from_slice(&bytes).unwrap();
        // Try to extend by 0 ms — should be rejected.
        let (status, v) = post_json(
            &app,
            "/api/work/extend-lease",
            &ExtendLeaseRequest {
                worker_id: worker_id.into(),
                session_token: token.into(),
                run_id: item.run_id.clone(),
                step_id: item.step_id.clone(),
                claim_id: item.claim_id,
                extend_by_ms: 0,
            },
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error_kind"], "coord.invalid_request");
        assert!(v["message"].as_str().unwrap().contains("minimum"));
    }

    #[tokio::test]
    async fn extend_lease_caps_at_max_lease_ttl_ms() {
        let state = fresh_state();
        pending_step(&state, "run-e", "s1", "fn main() -> Int { 1 }\n");
        let app = build_router(state.clone());
        let (_, reg) = post_json(
            &app,
            "/api/workers/register",
            &register_payload(state.capability_set_hash.clone()),
        )
        .await;
        // Claim via the http route to set up the lease.
        let worker_id = reg["worker_id"].as_str().unwrap();
        let token = reg["session_token"].as_str().unwrap();
        let req = Request::builder()
            .method("GET")
            .uri(format!(
                "/api/work/claim?worker_id={worker_id}&session_token={token}&lease_ttl_ms=10000"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let item: WorkItem = serde_json::from_slice(&bytes).unwrap();

        // Ask for more than max_lease_ttl_ms (600_000 in fresh_state).
        let now_before = now_unix_ms();
        let (status, v) = post_json(
            &app,
            "/api/work/extend-lease",
            &ExtendLeaseRequest {
                worker_id: worker_id.into(),
                session_token: token.into(),
                run_id: item.run_id.clone(),
                step_id: item.step_id.clone(),
                claim_id: item.claim_id,
                extend_by_ms: 9_999_999_999,
            },
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let new_deadline = v["new_lease_expires_at_ms"].as_i64().unwrap();
        // Capped at 600s: deadline should be within ~601s of now.
        assert!(new_deadline <= now_before + 600_001 + 5);
    }
}
