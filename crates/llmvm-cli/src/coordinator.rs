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

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use boruna_bytecode::compute_capability_set_hash;
use boruna_orchestrator::persistence::{
    BlobStoreError, ClaimOutcome, ExtendOutcome, RunCheckpointStore, RunStatus, StepStatus,
    TerminalOutcome,
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
    /// Process start time, captured once at `run_serve` entry. Used
    /// by the `/api/health` endpoint to report uptime so operators
    /// can detect coord restarts during a multi-coord rollout
    /// (sprint W2).
    start_time_ms: i64,
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
    /// Shared-secret bearer token for HTTP authentication
    /// (sprint `0.5-S3`). When `Some`, every coord HTTP route
    /// requires `Authorization: Bearer <secret>` header; mismatched
    /// or missing headers return `401 + coord.unauthorized`. When
    /// `None`, no auth is enforced (the pre-0.5-S3 behavior is
    /// preserved for backwards-compatibility on loopback-only
    /// deployments).
    ///
    /// Operators generate a secret via `openssl rand -hex 32` and
    /// pass it via `--shared-secret <hex>` flag or
    /// `BORUNA_COORD_SECRET` env var. The same value MUST be set
    /// on every worker via the analogous flag or env var.
    pub shared_secret: Option<String>,
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
    shared_secret: Option<String>,
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

    // On startup, eagerly sweep expired leases. The persistence
    // layer's `expire_leases_and_requeue(threshold)` only voids
    // leases whose `lease_expires_at < threshold` (CAS update on
    // a strict comparison) — so this is HA-safe under concurrent
    // coords: peer coords with healthy in-flight leases (those
    // with `lease_expires_at >= now_ms`) are unaffected.
    //
    // Note (sprint W2 audit): an earlier comment claimed this
    // sweep voids "any row in Running status." That was misleading
    // — the SQL filter on `lease_expires_at < ?1` always preserved
    // healthy leases. The ADR 002 phrase "coordinator restart =
    // all leases void" was the design intent before the lease
    // mechanism stabilized; the actual implementation is the
    // safer threshold-based variant we keep here.
    let now_ms = now_unix_ms();
    let n = store
        .expire_leases_and_requeue(now_ms + 1)
        .map_err(|e| format!("startup lease sweep failed: {e}"))?;
    if n > 0 {
        eprintln!("coordinator startup: requeued {n} expired-lease step(s)");
    }
    let start_time_ms = now_ms;

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

    let auth_state = match shared_secret.as_deref() {
        Some(_) => "enabled (shared-secret bearer)",
        None if bind.is_loopback() => "disabled (loopback bind only)",
        None => "DISABLED (non-loopback bind without --shared-secret)",
    };
    eprintln!("    auth: {auth_state}");
    if shared_secret.is_none() && !bind.is_loopback() {
        eprintln!(
            "[WARNING] coordinator is bound to a non-loopback address with NO --shared-secret. \
             Anyone with network access can SUBMIT and CONTROL distributed work. \
             Pass --shared-secret <hex> (or BORUNA_COORD_SECRET env) to enable auth."
        );
    }

    let state = CoordinatorState {
        store: Arc::new(Mutex::new(store)),
        workers: Arc::new(Mutex::new(HashMap::new())),
        workflow_dirs: Arc::new(Mutex::new(HashMap::new())),
        capability_set_hash,
        config: CoordinatorConfig {
            max_lease_ttl_ms,
            poll_timeout_ms,
            bind_warning,
            shared_secret,
        },
        start_time_ms,
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

/// Constant-time byte-slice equality. Avoids the early-exit pattern of `==`
/// that would leak per-byte timing information about a bearer token's
/// content to a network-adjacent attacker.
///
/// **Length-leakage:** the early-return on length-mismatch leaks the
/// expected secret length. Acceptable for our use case — operators
/// generate secrets via `openssl rand -hex 32` (a known length) and the
/// length is not what an attacker is trying to brute-force.
fn constant_time_bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Build a JSON 401 response with the stable `coord.unauthorized`
/// `error_kind`. Matches the `ErrorBody` shape used elsewhere.
fn unauthorized_response() -> Response {
    let body = ErrorBody::new(
        "coord.unauthorized",
        "missing or invalid Authorization: Bearer header",
    );
    (StatusCode::UNAUTHORIZED, Json(body)).into_response()
}

/// Axum middleware that validates the `Authorization: Bearer <secret>`
/// header against the coordinator's configured shared-secret. When no
/// shared-secret is configured, the middleware is a pass-through (the
/// pre-0.5-S3 no-auth behavior).
async fn auth_middleware(
    State(state): State<CoordinatorState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    // Sprint W2: liveness/readiness probes bypass auth so external
    // load balancers (and concerned operators with `curl`) can
    // verify a coord is up without holding the shared secret.
    // Health responses are non-sensitive — uptime, capability hash,
    // and version — so the bypass does not leak secret state.
    if request.uri().path() == "/api/health" {
        return next.run(request).await;
    }
    let Some(expected) = state.config.shared_secret.as_deref() else {
        return next.run(request).await;
    };
    let Some(header_val) = headers.get(axum::http::header::AUTHORIZATION) else {
        return unauthorized_response();
    };
    let Ok(header_str) = header_val.to_str() else {
        return unauthorized_response();
    };
    let Some(provided) = header_str.strip_prefix("Bearer ") else {
        return unauthorized_response();
    };
    if !constant_time_bytes_eq(provided.as_bytes(), expected.as_bytes()) {
        return unauthorized_response();
    }
    next.run(request).await
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
    // Sprint 0.5-S3: auth middleware applies to BOTH coord routes
    // (mutations + claims) AND the dashboard's read-only routes
    // (since they expose run state including step_sources). Operators
    // who specifically want a public read-only dashboard with auth-
    // gated mutations should run a separate `boruna dashboard serve`
    // process without the shared-secret. The merged listener is
    // strictly all-or-nothing for auth.
    let coord_router = Router::new()
        .route("/api/workers/register", post(handle_register))
        .route("/api/workers/heartbeat", post(handle_heartbeat))
        .route("/api/work/claim", get(handle_claim))
        .route("/api/work/complete", post(handle_complete))
        .route("/api/work/fail", post(handle_fail))
        .route("/api/work/extend-lease", post(handle_extend_lease))
        // Sprint 0.5-S4: operator-facing routes for CI runners that
        // do not share a data-dir with the cluster. Same auth
        // middleware as worker routes.
        .route("/api/runs/submit", post(handle_submit_run))
        .route("/api/runs/{run_id}/status", get(handle_run_status))
        // Sprint 0.5-S6: operator-facing routes for human-in-the-loop
        // and webhook-driven gates. Same bearer-token auth as the
        // submit / status routes.
        .route("/api/runs/{run_id}/approve", post(handle_approve_run))
        .route("/api/runs/{run_id}/trigger", post(handle_trigger_run))
        // Sprint 0.5-S7: fetch a large step output stored in the
        // coordinator's blob store. Run-scoped: the route only
        // returns bytes if the requested hash is referenced by a
        // checkpoint under this run, preventing the route from
        // serving as a generic blob server. Same auth as the rest.
        .route("/api/runs/{run_id}/blobs/{hash}", get(handle_get_blob))
        // Sprint W2: liveness/readiness probe for HA deployments.
        // Returns 200 + a small JSON document when the coord is
        // healthy; 503 when the SQLite store is unreachable. The
        // health check probes the store via a lightweight query
        // (PRAGMA quick_check would be too expensive; we just take
        // and release the mutex guard).
        .route("/api/health", get(handle_health))
        // The 8 MiB DefaultBodyLimit applies to coord routes
        // ONLY (not dashboard routes) because Axum's per-
        // router layer scoping means layers attached pre-merge
        // stay bound to their own routes. Dashboard is
        // GET-only today, so no body-limit need. If a future
        // sprint adds a mutating dashboard route (e.g. "cancel
        // run"), it must opt into a body limit explicitly OR
        // be added to coord_router instead.
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state.clone());
    let merged = coord_router.merge(dashboard_router);
    merged.layer(middleware::from_fn_with_state(state, auth_middleware))
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

/// Sprint `0.5-S4` — operator-side `POST /api/runs/submit` payload.
/// Inlines the full workflow definition + every Source-kind step's
/// `.ax` body so the coordinator's data-dir is the single source of
/// truth (CI runner does not need shared filesystem access).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SubmitRunRequest {
    pub workflow: boruna_orchestrator::workflow::definition::WorkflowDef,
    #[serde(default)]
    pub step_sources: BTreeMap<String, String>,
    #[serde(default)]
    pub policy: Option<boruna_vm::Policy>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SubmitRunResponse {
    pub protocol_version: u32,
    pub run_id: String,
    pub workflow_hash: String,
}

/// Sprint `0.5-S6` — `POST /api/runs/{run_id}/approve` body. Decision
/// is the canonical lowercase string (`"approved"` | `"rejected"`)
/// so the wire format matches the local CLI's argument shape.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApproveRequest {
    pub step_id: String,
    pub decision: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Sprint `0.5-S6` — `POST /api/runs/{run_id}/trigger` body. The
/// `token` field is the per-step trigger token stashed at gate-pause
/// time (NOT the bearer token for the auth middleware — that goes
/// in the `Authorization` header). Two separate secrets matches the
/// 0.3-S15 trigger model unchanged.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TriggerRequest {
    pub step_id: String,
    pub token: String,
    pub payload: String,
}

/// Sprint `0.5-S4` — `GET /api/runs/{run_id}/status` response.
/// Per-step status map mirrors the format `coordinator wait` uses
/// for stdout transition lines so a future HTTP-mode `wait` can
/// reuse the same wire shape.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RunStatusResponse {
    pub protocol_version: u32,
    pub run_id: String,
    pub status: String,
    pub step_statuses: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
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

// ── operator-facing submit + status (sprint 0.5-S4) ──

/// `POST /api/runs/submit` — register a workflow run from a CI
/// runner that does NOT share a `data-dir` with the cluster. Body
/// inlines the workflow def + every Source-kind step's `.ax` body;
/// the cluster persists everything into `metadata.audit_log`'s
/// surrounding metadata blob (same surface that `submit-only` mode
/// populates from disk). Auth: bearer-gated by the existing
/// `auth_middleware`. Failures map to the same `error_kind`
/// taxonomy used by other coordinator routes, plus three new kinds
/// scoped to this surface (`coord.submit.*`).
async fn handle_submit_run(
    State(state): State<CoordinatorState>,
    Json(req): Json<SubmitRunRequest>,
) -> Response {
    use boruna_orchestrator::workflow::WorkflowRunner;
    let policy = req.policy.unwrap_or_default();
    let store = state.store.clone();
    let store_guard = match store.lock() {
        Ok(g) => g,
        Err(e) => return internal_error(&format!("store mutex poisoned: {e}")),
    };
    let result = WorkflowRunner::submit_with_inline_sources(
        &req.workflow,
        req.step_sources,
        &policy,
        &store_guard,
    );
    drop(store_guard);
    match result {
        Ok(run_id) => {
            let workflow_hash = WorkflowRunner::workflow_hash_from_def(&req.workflow);
            (
                StatusCode::OK,
                Json(SubmitRunResponse {
                    protocol_version: PROTOCOL_VERSION,
                    run_id,
                    workflow_hash,
                }),
            )
                .into_response()
        }
        Err(e) => match e {
            boruna_orchestrator::workflow::WorkflowRunError::Validation(msg) => respond_err(
                StatusCode::BAD_REQUEST,
                ErrorBody::new("coord.submit.invalid_workflow", msg),
            ),
            boruna_orchestrator::workflow::WorkflowRunError::Internal(msg) => respond_err(
                StatusCode::BAD_REQUEST,
                ErrorBody::new("coord.submit.bad_payload", msg),
            ),
            other => internal_error(&format!("submit failed: {other}")),
        },
    }
}

/// `GET /api/runs/{run_id}/status` — return a compact status
/// snapshot for the named run. The shape (`status` string +
/// per-step status map + optional `error_msg`) matches what
/// `coordinator wait`'s stdout reflects, so a future HTTP-mode
/// `wait` can reuse the same wire format. 404 with stable
/// `coord.runs.not_found` when the run_id isn't in the store.
///
/// The handler also advances the run one tick before reading
/// state. In the local-data-dir model `coordinator wait` was the
/// thing that drove `advance_run_one_tick`; in the remote-submit
/// model the operator is polling over HTTP and there is no
/// separate wait driver. Folding `advance` into the status read
/// makes the operator's poll the wait driver. Concurrent pollers
/// race-safely converge — the same property locked by the
/// `cli_coordinator_wait_two_concurrent_waits_converge` regression
/// test from the 0.5-S2f cleanup.
async fn handle_run_status(
    State(state): State<CoordinatorState>,
    Path(run_id): Path<String>,
) -> Response {
    use boruna_orchestrator::workflow::{AdvanceRunStatus, WorkflowRunner};

    let store = state.store.clone();
    let store_guard = match store.lock() {
        Ok(g) => g,
        Err(e) => return internal_error(&format!("store mutex poisoned: {e}")),
    };

    // Confirm the run exists before advancing — advance_run_one_tick
    // returns an Internal error for unknown run ids; we want to
    // produce the stable `coord.runs.not_found` taxonomy entry, so
    // dispatch on existence first.
    match store_guard.get_run(&run_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return respond_err(
                StatusCode::NOT_FOUND,
                ErrorBody::new("coord.runs.not_found", format!("no run with id '{run_id}'")),
            );
        }
        Err(e) => {
            return internal_error(&format!("read run: {e}"));
        }
    }

    // Drive the run forward. In the local-data-dir model
    // `coordinator wait` was the thing that did this; in the
    // remote-submit model the operator is polling over HTTP and
    // there is no separate driver. Folding `advance` into the
    // status read makes the operator's poll the wait driver.
    // Concurrent pollers race-safely converge — same property
    // locked by the `cli_coordinator_wait_two_concurrent_waits_converge`
    // regression test from the 0.5-S2f cleanup.
    let advance = match WorkflowRunner::advance_run_one_tick(&store_guard, &run_id) {
        Ok(r) => r,
        Err(e) => {
            return internal_error(&format!("advance run: {e}"));
        }
    };
    let cps = match store_guard.list_step_checkpoints(&run_id) {
        Ok(cs) => cs,
        Err(e) => {
            return internal_error(&format!("read checkpoints: {e}"));
        }
    };

    let mut step_statuses = BTreeMap::new();
    let mut error_msg: Option<String> = None;
    for cp in &cps {
        step_statuses.insert(
            cp.step_id.clone(),
            persist_status_str(cp.status).to_string(),
        );
        if cp.status == StepStatus::Failed && error_msg.is_none() {
            error_msg.clone_from(&cp.error_msg);
        }
    }

    // Surface the *computed* run status from advance_run_one_tick
    // — the same value `coordinator wait` consults to exit. The
    // run row's `status` column is operationally maintained by
    // the in-process runner only; in distributed mode it doesn't
    // transition. Mirroring the wait driver here keeps wire and
    // local semantics aligned.
    let status_str = match advance.run_status {
        AdvanceRunStatus::Running => "running",
        AdvanceRunStatus::Completed => "completed",
        AdvanceRunStatus::Failed => "failed",
    }
    .to_string();

    // Terminal: append closing WorkflowCompleted audit event
    // idempotently — same posture as `coordinator wait`'s terminal
    // exit paths. Without this, runs driven entirely through the
    // remote API would have no closing audit chain entry.
    if matches!(
        advance.run_status,
        AdvanceRunStatus::Completed | AdvanceRunStatus::Failed
    ) {
        if let Err(e) = WorkflowRunner::append_wait_terminal_audit_event(&store_guard, &run_id) {
            eprintln!("warning: failed to append terminal audit event for '{run_id}': {e}");
        }
    }
    drop(store_guard);

    Json(RunStatusResponse {
        protocol_version: PROTOCOL_VERSION,
        run_id,
        status: status_str,
        step_statuses,
        error_msg,
    })
    .into_response()
}

// ── operator-facing approve + trigger (sprint 0.5-S6) ──

/// `POST /api/runs/{run_id}/approve` — record an approval-gate
/// decision (approved or rejected) for a paused step. Delegates to
/// `record_approval_decision_in_store`. Decision string is
/// lowercase `"approved"` / `"rejected"`. Auth: bearer-gated by
/// `auth_middleware` like all other operator routes.
async fn handle_approve_run(
    State(state): State<CoordinatorState>,
    Path(run_id): Path<String>,
    Json(req): Json<ApproveRequest>,
) -> Response {
    use boruna_orchestrator::workflow::ApprovalKind;

    let kind = match req.decision.as_str() {
        "approved" => ApprovalKind::Approved,
        "rejected" => ApprovalKind::Rejected,
        other => {
            return respond_err(
                StatusCode::BAD_REQUEST,
                ErrorBody::new(
                    "coord.approve.bad_payload",
                    format!("decision must be \"approved\" or \"rejected\", got {other:?}"),
                ),
            );
        }
    };
    let store = state.store.clone();
    let store_guard = match store.lock() {
        Ok(g) => g,
        Err(e) => return internal_error(&format!("store mutex poisoned: {e}")),
    };
    let result = boruna_orchestrator::workflow::record_approval_decision_in_store(
        &store_guard,
        &run_id,
        &req.step_id,
        kind,
        req.reason.clone(),
    );
    drop(store_guard);
    match result {
        Ok(()) => Json(OkResponse {
            protocol_version: PROTOCOL_VERSION,
            ok: true,
        })
        .into_response(),
        Err(e) => approve_error_response(e),
    }
}

/// `POST /api/runs/{run_id}/trigger` — record an external-trigger
/// payload for a paused step. Delegates to
/// `record_external_trigger_in_store`. Bearer-gated.
async fn handle_trigger_run(
    State(state): State<CoordinatorState>,
    Path(run_id): Path<String>,
    Json(req): Json<TriggerRequest>,
) -> Response {
    let store = state.store.clone();
    let store_guard = match store.lock() {
        Ok(g) => g,
        Err(e) => return internal_error(&format!("store mutex poisoned: {e}")),
    };
    let result = boruna_orchestrator::workflow::record_external_trigger_in_store(
        &store_guard,
        &run_id,
        &req.step_id,
        &req.token,
        &req.payload,
    );
    drop(store_guard);
    match result {
        Ok(()) => Json(OkResponse {
            protocol_version: PROTOCOL_VERSION,
            ok: true,
        })
        .into_response(),
        Err(e) => trigger_error_response(e),
    }
}

/// `GET /api/runs/{run_id}/blobs/{hash}` — return the bytes of a
/// large step output stored in the coordinator's blob store. Sprint
/// 0.5-S7.
///
/// **Run-scoped:** the route only returns bytes if `hash` is referenced
/// by a step checkpoint under `run_id`. Even though hashes are
/// content-addressed and globally unique by collision resistance, this
/// scope makes the route's authorization story trivial — every run is
/// already gated by the bearer-token middleware, and access to
/// `run_id` implies access to its outputs. A future cross-run dedup
/// route would be a NEW endpoint with its own access-control story.
///
/// Error_kind taxonomy (sprint 0.5-S7, locked):
/// - `coord.blobs.bad_hash` — 400 — `hash` is not 64 lowercase hex
///   characters.
/// - `coord.blobs.not_found` — 404 — no checkpoint under `run_id`
///   references this hash, OR the checkpoint references it but the
///   blob file is missing on disk.
/// - `coord.unauthorized` — 401 (handled upstream by `auth_middleware`).
async fn handle_get_blob(
    State(state): State<CoordinatorState>,
    Path((run_id, hash)): Path<(String, String)>,
) -> Response {
    // Validate hash format BEFORE any other check. A malformed hash is
    // never a valid query — even on an unknown run — so 400 is the
    // accurate signal for clients passing a bad URL.
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return respond_err(
            StatusCode::BAD_REQUEST,
            ErrorBody::new(
                "coord.blobs.bad_hash",
                "hash must be 64 lowercase hex characters",
            ),
        );
    }

    // Acquire store inside the async fn synchronously (the orchestrator's
    // single-threaded SQLite connection lives inside Arc<Mutex<...>>).
    let store_guard = match state.store.lock() {
        Ok(g) => g,
        Err(_) => return internal_error("store lock poisoned"),
    };

    // Run-scope check before any filesystem access.
    let owned = match store_guard.run_owns_blob_ref(&run_id, &hash) {
        Ok(b) => b,
        Err(e) => return internal_error(&format!("run_owns_blob_ref: {e}")),
    };
    if !owned {
        // 404 covers both "no run", "no checkpoint", and "checkpoint
        // does not reference this hash". Doesn't disambiguate to avoid
        // exposing run existence to unauthorized-but-otherwise-valid-
        // bearer callers.
        return respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new(
                "coord.blobs.not_found",
                format!("no blob '{hash}' referenced by run '{run_id}'"),
            ),
        );
    }

    // Run owns the ref; resolve via blob store.
    let blob_store = match store_guard.blob_store() {
        Some(bs) => bs.clone(),
        None => {
            return internal_error("coordinator opened without a blob store (in-memory mode?)");
        }
    };
    drop(store_guard);

    match blob_store.read_bytes(&hash) {
        Ok(bytes) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(BlobStoreError::BadHash) => respond_err(
            StatusCode::BAD_REQUEST,
            ErrorBody::new(
                "coord.blobs.bad_hash",
                "hash must be 64 lowercase hex characters",
            ),
        ),
        Err(BlobStoreError::NotFound) => respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new(
                "coord.blobs.not_found",
                format!(
                    "blob '{hash}' is referenced by a checkpoint under '{run_id}' \
                     but is missing from the blob store on disk"
                ),
            ),
        ),
        Err(e) => internal_error(&format!("blob read: {e}")),
    }
}

/// Health/readiness probe response. Sprint W2.
///
/// Non-sensitive content only — load balancers and external probes
/// receive this without bearer-auth (see `auth_middleware` bypass).
/// `boruna_version` is read from the workspace package version at
/// build time. `capability_set_hash` lets workers verify they're
/// addressing a coord with a compatible capability table before
/// committing to a registration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HealthResponse {
    pub protocol_version: u32,
    pub status: String,
    pub boruna_version: &'static str,
    pub capability_set_hash: String,
    pub uptime_ms: i64,
}

async fn handle_health(State(state): State<CoordinatorState>) -> Response {
    // Probe the store mutex to detect a poisoned lock — that's the
    // only failure mode that wouldn't already surface as a TCP
    // connect error. We don't run any SQL against the store: a
    // crashed-process replacement coord still has a fresh
    // connection, and forcing every probe through SQL would amplify
    // load-balancer-driven query traffic.
    if state.store.lock().is_err() {
        return respond_err(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorBody::new(
                "coord.unavailable",
                "store mutex poisoned; coord is not ready",
            ),
        );
    }
    let now_ms = now_unix_ms();
    let uptime_ms = now_ms.saturating_sub(state.start_time_ms);
    let body = HealthResponse {
        protocol_version: PROTOCOL_VERSION,
        status: "ready".to_string(),
        boruna_version: env!("CARGO_PKG_VERSION"),
        capability_set_hash: state.capability_set_hash.clone(),
        uptime_ms,
    };
    (StatusCode::OK, Json(body)).into_response()
}

/// Map a `WorkflowRunError` from the approve path to an HTTP
/// response with the locked `coord.approve.*` error_kind taxonomy.
/// Sprint 0.5-S6.
fn approve_error_response(e: boruna_orchestrator::workflow::WorkflowRunError) -> Response {
    use boruna_orchestrator::workflow::WorkflowRunError;
    match e {
        WorkflowRunError::RunNotFound(id) => respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new("coord.runs.not_found", format!("no run with id '{id}'")),
        ),
        WorkflowRunError::RunNotResumable { run_id, terminal_status } => respond_err(
            StatusCode::CONFLICT,
            ErrorBody::new(
                "coord.approve.invalid_state",
                format!("run '{run_id}' is in terminal status '{terminal_status}'"),
            ),
        ),
        WorkflowRunError::StepNotFound { run_id, step_id } => respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new(
                "coord.approve.invalid_state",
                format!("step '{step_id}' not found in run '{run_id}'"),
            ),
        ),
        WorkflowRunError::NotAnApprovalGateStep { run_id, step_id } => respond_err(
            StatusCode::CONFLICT,
            ErrorBody::new(
                "coord.approve.invalid_state",
                format!("step '{step_id}' in run '{run_id}' is not an approval-gate step"),
            ),
        ),
        WorkflowRunError::StepNotAtApprovalGate { run_id, step_id, current_status } => respond_err(
            StatusCode::CONFLICT,
            ErrorBody::new(
                "coord.approve.invalid_state",
                format!(
                    "step '{step_id}' in run '{run_id}' is in '{current_status}', not 'awaiting_approval'"
                ),
            ),
        ),
        WorkflowRunError::StepAlreadyDecided { run_id, step_id, prior_decision } => respond_err(
            StatusCode::CONFLICT,
            ErrorBody::new(
                "coord.approve.invalid_state",
                format!(
                    "step '{step_id}' in run '{run_id}' was already decided ({prior_decision})"
                ),
            ),
        ),
        other => internal_error(&format!("approve failed: {other}")),
    }
}

/// Map a `WorkflowRunError` from the trigger path to an HTTP
/// response with the locked `coord.trigger.*` error_kind taxonomy.
fn trigger_error_response(e: boruna_orchestrator::workflow::WorkflowRunError) -> Response {
    use boruna_orchestrator::workflow::WorkflowRunError;
    match e {
        WorkflowRunError::RunNotFound(id) => respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new("coord.runs.not_found", format!("no run with id '{id}'")),
        ),
        WorkflowRunError::RunNotResumable { run_id, terminal_status } => respond_err(
            StatusCode::CONFLICT,
            ErrorBody::new(
                "coord.trigger.invalid_state",
                format!("run '{run_id}' is in terminal status '{terminal_status}'"),
            ),
        ),
        WorkflowRunError::StepNotFound { run_id, step_id } => respond_err(
            StatusCode::NOT_FOUND,
            ErrorBody::new(
                "coord.trigger.invalid_state",
                format!("step '{step_id}' not found in run '{run_id}'"),
            ),
        ),
        WorkflowRunError::NotAnExternalTriggerStep { run_id, step_id } => respond_err(
            StatusCode::CONFLICT,
            ErrorBody::new(
                "coord.trigger.invalid_state",
                format!(
                    "step '{step_id}' in run '{run_id}' is not an external-trigger step"
                ),
            ),
        ),
        WorkflowRunError::StepNotAtExternalTriggerGate { run_id, step_id, current_status } => respond_err(
            StatusCode::CONFLICT,
            ErrorBody::new(
                "coord.trigger.invalid_state",
                format!(
                    "step '{step_id}' in run '{run_id}' is in '{current_status}', not 'awaiting_external_event'"
                ),
            ),
        ),
        WorkflowRunError::InvalidTriggerToken { run_id, step_id } => respond_err(
            StatusCode::UNAUTHORIZED,
            ErrorBody::new(
                "coord.trigger.bad_token",
                format!(
                    "trigger token mismatch for step '{step_id}' in run '{run_id}'"
                ),
            ),
        ),
        WorkflowRunError::StepAlreadyTriggered { run_id, step_id, prior_triggered_at_ms } => respond_err(
            StatusCode::CONFLICT,
            ErrorBody::new(
                "coord.trigger.invalid_state",
                format!(
                    "step '{step_id}' in run '{run_id}' was already triggered at {prior_triggered_at_ms}"
                ),
            ),
        ),
        WorkflowRunError::Validation(msg) => respond_err(
            StatusCode::BAD_REQUEST,
            ErrorBody::new("coord.trigger.bad_payload", msg),
        ),
        other => internal_error(&format!("trigger failed: {other}")),
    }
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

// ── workflow run --coordinator client (sprint 0.5-S4) ──

/// Drive the operator-side flow for `boruna workflow run --coordinator`:
/// 1. Read `workflow_dir/workflow.json` + each Source-step's `.ax`.
/// 2. POST `/api/runs/submit` with the inlined payload.
/// 3. Poll `/api/runs/{run_id}/status` until terminal.
/// 4. Print step transitions to stdout (matching `coordinator wait`'s
///    line-per-transition format).
/// 5. Return an exit code: `0` Completed, `1` Failed, `2` Timeout.
///
/// `coord_url` may end with or without a trailing slash; we normalize.
/// `coord_token` is sent as `Authorization: Bearer <token>` when
/// `Some`, omitted otherwise — operators running an unauthenticated
/// loopback coordinator can pass `None` (or omit the env var).
pub fn run_remote(
    def: &boruna_orchestrator::workflow::definition::WorkflowDef,
    workflow_dir: &std::path::Path,
    policy: &boruna_vm::Policy,
    coord_url: &str,
    coord_token: Option<&str>,
    poll_interval_ms: u64,
    max_wait_secs: u64,
) -> Result<i32, Box<dyn std::error::Error>> {
    use boruna_orchestrator::workflow::definition::StepKind;

    // 1. Collect step sources from disk.
    let mut step_sources: BTreeMap<String, String> = BTreeMap::new();
    for (step_id, step_def) in &def.steps {
        if let StepKind::Source { source } = &step_def.kind {
            let path = workflow_dir.join(source);
            let body = std::fs::read_to_string(&path).map_err(|e| {
                format!(
                    "step '{step_id}' source '{}' read failed: {e}",
                    path.display()
                )
            })?;
            step_sources.insert(step_id.clone(), body);
        }
    }

    // 2. Build URLs. The submit URL is fixed; the status URL needs a
    //    placeholder for the run_id we don't yet know.
    let base = coord_url.trim_end_matches('/').to_string();
    let submit_url = format!("{base}/api/runs/submit");

    // 3. Synchronous Tokio runtime — `reqwest` here is the async
    //    client (the same one the worker uses) and we want a simple
    //    synchronous CLI surface. A short-lived current-thread
    //    runtime keeps memory + thread overhead minimal.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to build tokio runtime: {e}"))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let submit = SubmitRunRequest {
        workflow: def.clone(),
        step_sources,
        policy: Some(policy.clone()),
    };

    eprintln!("workflow run --coordinator");
    eprintln!("    coordinator: {base}");
    eprintln!("    workflow:    {}", def.name);
    eprintln!("    poll-ms:     {poll_interval_ms}");
    if max_wait_secs > 0 {
        eprintln!("    max-wait-s:  {max_wait_secs}");
    }

    // 4. Submit + poll under the runtime.
    rt.block_on(async move {
        let mut req = client.post(&submit_url).json(&submit);
        if let Some(tok) = coord_token {
            req = req.bearer_auth(tok);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("submit failed: HTTP error: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err::<i32, Box<dyn std::error::Error>>(
                format!("submit failed: {status}: {body}").into(),
            );
        }
        let submit_resp: SubmitRunResponse = resp
            .json()
            .await
            .map_err(|e| format!("submit response not parseable as JSON: {e}"))?;
        let run_id = submit_resp.run_id;
        eprintln!("    run_id:      {run_id}");
        eprintln!("    workflow_hash: {}", submit_resp.workflow_hash);

        let status_url = format!("{base}/api/runs/{run_id}/status");
        let effective_poll_ms = poll_interval_ms.max(MIN_WAIT_POLL_INTERVAL_MS);
        if poll_interval_ms < MIN_WAIT_POLL_INTERVAL_MS {
            eprintln!(
                "[WARNING] --coord-poll-interval-ms {poll_interval_ms} below minimum \
                 {MIN_WAIT_POLL_INTERVAL_MS}; using {effective_poll_ms} ms"
            );
        }

        let started = std::time::Instant::now();
        let mut prev: BTreeMap<String, String> = BTreeMap::new();
        loop {
            let mut req = client.get(&status_url);
            if let Some(tok) = coord_token {
                req = req.bearer_auth(tok);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| format!("status poll failed: {e}"))?;
            if !resp.status().is_success() {
                let s = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err::<i32, Box<dyn std::error::Error>>(
                    format!("status poll: {s}: {body}").into(),
                );
            }
            let snapshot: RunStatusResponse = resp
                .json()
                .await
                .map_err(|e| format!("status response not parseable as JSON: {e}"))?;

            for (sid, sstatus) in &snapshot.step_statuses {
                match prev.get(sid) {
                    Some(p) if p == sstatus => {}
                    _ => {
                        println!("step {sid}: {sstatus}");
                        prev.insert(sid.clone(), sstatus.clone());
                    }
                }
            }

            match snapshot.status.as_str() {
                "completed" => {
                    println!("run {run_id}: completed");
                    return Ok::<i32, Box<dyn std::error::Error>>(0);
                }
                "failed" => {
                    if let Some(msg) = &snapshot.error_msg {
                        println!("run {run_id}: failed — {msg}");
                    } else {
                        println!("run {run_id}: failed");
                    }
                    return Ok(1);
                }
                _ => {}
            }

            if max_wait_secs > 0 && started.elapsed().as_secs() >= max_wait_secs {
                eprintln!(
                    "run {run_id}: exceeded --coord-max-wait-secs={max_wait_secs}; \
                     remote run continues; CLI exiting with 2"
                );
                return Ok(2);
            }

            tokio::time::sleep(Duration::from_millis(effective_poll_ms)).await;
        }
    })
}

// ── workflow approve / reject / trigger client (sprint 0.5-S6) ──

/// POST `/api/runs/{run_id}/approve` against a remote coordinator.
/// Used by `boruna workflow approve --coordinator <url>` and
/// `boruna workflow reject --coordinator <url>`. Returns `Ok(())` on
/// success, an error with the coordinator's `error_kind` and
/// message verbatim on a non-2xx response.
pub fn send_approve_remote(
    coord_url: &str,
    coord_token: Option<&str>,
    run_id: &str,
    step_id: &str,
    decision: &str,
    reason: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let base = coord_url.trim_end_matches('/');
    let url = format!("{base}/api/runs/{run_id}/approve");
    let body = ApproveRequest {
        step_id: step_id.to_string(),
        decision: decision.to_string(),
        reason: reason.map(|s| s.to_string()),
    };
    post_operator_command(&url, coord_token, &body)
}

/// POST `/api/runs/{run_id}/trigger` against a remote coordinator.
/// Mirrors `send_approve_remote`'s shape; separate function only
/// because the body type differs.
pub fn send_trigger_remote(
    coord_url: &str,
    coord_token: Option<&str>,
    run_id: &str,
    step_id: &str,
    trigger_token: &str,
    payload: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let base = coord_url.trim_end_matches('/');
    let url = format!("{base}/api/runs/{run_id}/trigger");
    let body = TriggerRequest {
        step_id: step_id.to_string(),
        token: trigger_token.to_string(),
        payload: payload.to_string(),
    };
    post_operator_command(&url, coord_token, &body)
}

/// Shared POST helper for the operator-side mutation routes.
/// Builds a tokio runtime, sends the request, surfaces non-2xx
/// responses with the coordinator's full error body so operators
/// get a clear `coord.*` error_kind without us re-parsing here.
fn post_operator_command<T: serde::Serialize + ?Sized>(
    url: &str,
    coord_token: Option<&str>,
    body: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to build tokio runtime: {e}"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    rt.block_on(async move {
        let mut req = client.post(url).json(body);
        if let Some(tok) = coord_token {
            req = req.bearer_auth(tok);
        }
        let resp = req.send().await.map_err(|e| format!("HTTP error: {e}"))?;
        let status = resp.status();
        if status.is_success() {
            return Ok::<(), Box<dyn std::error::Error>>(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err::<(), Box<dyn std::error::Error>>(format!("{status}: {body}").into())
    })
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
        // Print explicit "requeued" lines (sprint 0.5-S5) BEFORE the
        // generic transition loop so operators see "step s1: requeued
        // (retry)" instead of just "step s1: pending" when a Failed
        // step transitions back to Pending via the retry policy.
        for sid in &result.newly_requeued {
            println!("step {sid}: requeued (retry)");
            prev.insert(sid.clone(), "pending".into());
        }
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
                // Sprint follow-up to 0.5-S2f: emit a terminating
                // WorkflowCompleted audit event so the chain has
                // a closing entry to match its WorkflowStarted
                // genesis. Idempotent on re-invocation.
                if let Err(e) = WorkflowRunner::append_wait_terminal_audit_event(&store, &run_id) {
                    eprintln!(
                        "warning: failed to append WorkflowCompleted audit event for run \
                         '{run_id}': {e}"
                    );
                }
                println!("run {run_id}: completed");
                return Ok(0);
            }
            AdvanceRunStatus::Failed => {
                if let Err(e) = WorkflowRunner::append_wait_terminal_audit_event(&store, &run_id) {
                    eprintln!(
                        "warning: failed to append WorkflowCompleted audit event for run \
                         '{run_id}': {e}"
                    );
                }
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
                shared_secret: None,
            },
            start_time_ms: 0,
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
                output_blob_ref: None,
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
        // Manually claim and then simulate expiry+reclaim. The lock
        // is scoped to a block so the MutexGuard drops before any
        // .await — clippy::await_holding_lock would flag a guard
        // bound at the function level even with an explicit drop().
        {
            let store = state.store.lock().unwrap();
            store
                .claim_step("run-c", "s1", "A", 5_000_000_000_000, 0)
                .unwrap();
            store.expire_leases_and_requeue(5_000_000_000_001).unwrap();
            store
                .claim_step("run-c", "s1", "B", 5_000_000_000_002, 0)
                .unwrap();
        }
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

    // ── Sprint 0.5-S4 — submit + status handler tests ──

    fn make_submit_workflow() -> serde_json::Value {
        // Minimal one-step Source workflow. Inline source compiles
        // to `42`. The handler doesn't run the workflow, only
        // registers it; compilability of the source is the
        // worker's concern.
        serde_json::json!({
            "name": "wf-s4",
            "version": "1.0.0",
            "steps": {
                "s1": {
                    "kind": "source",
                    "source": "s1.ax"
                }
            },
            "edges": []
        })
    }

    #[tokio::test]
    async fn submit_run_inserts_run_and_returns_run_id() {
        let state = fresh_state();
        let app = build_router(state.clone());
        let body = serde_json::json!({
            "workflow": make_submit_workflow(),
            "step_sources": { "s1": "fn main() -> Int { 42 }" }
        });
        let (status, v) = post_json(&app, "/api/runs/submit", &body).await;
        assert_eq!(status, StatusCode::OK, "body: {v}");
        let run_id = v["run_id"].as_str().expect("run_id present").to_string();
        assert_eq!(run_id.len(), 16, "run_id is 16-hex deterministic");
        assert_eq!(v["protocol_version"], 1);
        assert!(!v["workflow_hash"].as_str().unwrap().is_empty());

        // Run row is now in the store with the initial Pending checkpoint.
        let store = state.store.lock().unwrap();
        let row = store.get_run(&run_id).unwrap().expect("run inserted");
        assert_eq!(row.workflow_name, "wf-s4");
        let cps = store.list_step_checkpoints(&run_id).unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].step_id, "s1");
        assert_eq!(cps[0].status, StepStatus::Pending);
    }

    #[tokio::test]
    async fn submit_run_rejects_workflow_with_missing_step_source() {
        let state = fresh_state();
        let app = build_router(state.clone());
        let body = serde_json::json!({
            "workflow": make_submit_workflow(),
            "step_sources": {}  // s1 source missing
        });
        let (status, v) = post_json(&app, "/api/runs/submit", &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error_kind"], "coord.submit.invalid_workflow");
        assert!(
            v["message"]
                .as_str()
                .unwrap()
                .contains("missing inline source"),
            "expected missing-source error, got: {v}"
        );
    }

    #[tokio::test]
    async fn submit_run_rejects_oversized_step_source() {
        let state = fresh_state();
        let app = build_router(state.clone());
        // 257 KiB — over the 256 KiB per-step cap.
        let big = "x".repeat(257 * 1024);
        let body = serde_json::json!({
            "workflow": make_submit_workflow(),
            "step_sources": { "s1": big }
        });
        let (status, v) = post_json(&app, "/api/runs/submit", &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error_kind"], "coord.submit.invalid_workflow");
        assert!(
            v["message"]
                .as_str()
                .unwrap()
                .contains("exceeds per-step cap"),
            "expected size-cap error, got: {v}"
        );
    }

    #[tokio::test]
    async fn run_status_returns_404_for_unknown_run_id() {
        let state = fresh_state();
        let app = build_router(state.clone());
        let req = Request::builder()
            .method("GET")
            .uri("/api/runs/nope1234nope1234/status")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error_kind"], "coord.runs.not_found");
    }

    #[tokio::test]
    async fn run_status_reflects_submitted_run() {
        // Submit a run, then GET its status; the response must
        // mirror the row + initial Pending checkpoint.
        let state = fresh_state();
        let app = build_router(state.clone());
        let body = serde_json::json!({
            "workflow": make_submit_workflow(),
            "step_sources": { "s1": "fn main() -> Int { 42 }" }
        });
        let (_, v) = post_json(&app, "/api/runs/submit", &body).await;
        let run_id = v["run_id"].as_str().unwrap().to_string();

        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/runs/{run_id}/status"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let snap: RunStatusResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(snap.run_id, run_id);
        assert_eq!(snap.status, "running");
        assert_eq!(snap.step_statuses.get("s1").unwrap(), "pending");
        assert!(snap.error_msg.is_none());
    }

    // ── Sprint 0.5-S6 — approve + trigger handler tests ──

    fn make_approval_workflow() -> serde_json::Value {
        // Two-step workflow: a Source step "analyze" feeds an
        // ApprovalGate "human_review". Submit drives `analyze` to
        // Pending; we manually mark it Completed in tests, then a
        // tick opens the gate, then approve/reject closes it.
        serde_json::json!({
            "name": "wf-s6-approve",
            "version": "1.0.0",
            "steps": {
                "analyze": {
                    "kind": "source",
                    "source": "analyze.ax"
                },
                "human_review": {
                    "kind": "approval_gate",
                    "required_role": "reviewer",
                    "depends_on": ["analyze"]
                }
            },
            "edges": [["analyze", "human_review"]]
        })
    }

    async fn submit_with_open_gate(state: &CoordinatorState) -> String {
        // Helper for the approve handler tests: submit, drive analyze
        // Completed, tick to open the gate, return run_id.
        let app = build_router(state.clone());
        let body = serde_json::json!({
            "workflow": make_approval_workflow(),
            "step_sources": { "analyze": "fn main() -> Int { 1 }" }
        });
        let (status, v) = post_json(&app, "/api/runs/submit", &body).await;
        assert_eq!(status, StatusCode::OK, "submit failed: {v}");
        let run_id = v["run_id"].as_str().unwrap().to_string();

        {
            let store = state.store.lock().unwrap();
            let claim = store
                .claim_step(&run_id, "analyze", "w", 1_000_000_000, 0)
                .unwrap();
            let claim_id = match claim {
                boruna_orchestrator::persistence::ClaimOutcome::Claimed { claim_id } => claim_id,
                other => panic!("{other:?}"),
            };
            store
                .complete_step_cas(&run_id, "analyze", claim_id, "{}", "0", 1, 1)
                .unwrap();
        }

        // Tick via the status endpoint (which calls advance) so the
        // gate opens.
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/runs/{run_id}/status"))
            .body(Body::empty())
            .unwrap();
        app.clone().oneshot(req).await.unwrap();
        run_id
    }

    #[tokio::test]
    async fn approve_run_advances_gate_to_completed() {
        let state = fresh_state();
        let run_id = submit_with_open_gate(&state).await;
        let app = build_router(state.clone());

        let body = serde_json::json!({
            "step_id": "human_review",
            "decision": "approved"
        });
        let (status, v) = post_json(&app, &format!("/api/runs/{run_id}/approve"), &body).await;
        assert_eq!(status, StatusCode::OK, "approve failed: {v}");
        assert_eq!(v["ok"], true);

        // Next status read advances the run; the gate is now Completed.
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/runs/{run_id}/status"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let snap: RunStatusResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            snap.step_statuses.get("human_review").map(|s| s.as_str()),
            Some("completed"),
            "gate should be Completed after approve, got: {snap:?}"
        );
        // The run is itself Completed because all steps reached terminal.
        assert_eq!(snap.status, "completed");
    }

    #[tokio::test]
    async fn approve_run_rejects_invalid_decision_string() {
        let state = fresh_state();
        let run_id = submit_with_open_gate(&state).await;
        let app = build_router(state.clone());

        let body = serde_json::json!({
            "step_id": "human_review",
            "decision": "maybe"
        });
        let (status, v) = post_json(&app, &format!("/api/runs/{run_id}/approve"), &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error_kind"], "coord.approve.bad_payload");
    }

    #[tokio::test]
    async fn approve_run_returns_404_for_unknown_run_id() {
        let state = fresh_state();
        let app = build_router(state);
        let body = serde_json::json!({
            "step_id": "x",
            "decision": "approved"
        });
        let (status, v) = post_json(&app, "/api/runs/deadbeef0badcafe/approve", &body).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(v["error_kind"], "coord.runs.not_found");
    }

    #[tokio::test]
    async fn approve_run_rejects_double_decision() {
        let state = fresh_state();
        let run_id = submit_with_open_gate(&state).await;
        let app = build_router(state.clone());
        let body = serde_json::json!({
            "step_id": "human_review",
            "decision": "approved"
        });
        let (s1, _) = post_json(&app, &format!("/api/runs/{run_id}/approve"), &body).await;
        assert_eq!(s1, StatusCode::OK);
        // Second approval on the same step must be rejected.
        let (s2, v2) = post_json(&app, &format!("/api/runs/{run_id}/approve"), &body).await;
        assert_eq!(s2, StatusCode::CONFLICT);
        assert_eq!(v2["error_kind"], "coord.approve.invalid_state");
    }

    #[tokio::test]
    async fn approve_run_rejects_unauthenticated_when_secret_configured() {
        // Symmetric to the submit-run auth check: the approve route
        // must inherit the bearer-gating from auth_middleware.
        let mut state = fresh_state();
        state.config.shared_secret = Some("super-secret".into());
        let app = build_router(state);
        let body = serde_json::json!({
            "step_id": "x",
            "decision": "approved"
        });
        let (status, v) = post_json(&app, "/api/runs/abcd0123abcd0123/approve", &body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(v["error_kind"], "coord.unauthorized");
    }

    #[tokio::test]
    async fn submit_run_rejects_unauthenticated_when_secret_configured() {
        // Auth middleware (sprint 0.5-S3) must guard the new
        // submit endpoint just like the worker endpoints.
        let mut state = fresh_state();
        state.config.shared_secret = Some("super-secret".into());
        let app = build_router(state.clone());
        let body = serde_json::json!({
            "workflow": make_submit_workflow(),
            "step_sources": { "s1": "fn main() -> Int { 42 }" }
        });
        let (status, v) = post_json(&app, "/api/runs/submit", &body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(v["error_kind"], "coord.unauthorized");
    }

    // ── Sprint 0.5-S7: blob-fetch route ──

    /// In-memory store wired to a real on-disk blob store at a per-test
    /// tempdir. Used by the blob-route handler tests.
    fn fresh_state_with_blob_store() -> (CoordinatorState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let blobs_root = dir.path().join("blobs");
        let store = RunCheckpointStore::open_in_memory_with_blob_store(blobs_root).unwrap();
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
                max_lease_ttl_ms: 600_000,
                poll_timeout_ms: 200,
                bind_warning: None,
                shared_secret: None,
            },
            start_time_ms: 0,
        };
        (state, dir)
    }

    fn complete_running_step_in_state(
        state: &CoordinatorState,
        run_id: &str,
        step_id: &str,
        output_json: &str,
    ) -> String {
        use sha2::Digest;
        pending_step(state, run_id, step_id, "fn main() -> Int { 0 }");
        let store = state.store.lock().unwrap();
        let claim_id = match store
            .claim_step(run_id, step_id, "wkr-test", 9_999_999_999, 1_000)
            .unwrap()
        {
            ClaimOutcome::Claimed { claim_id } => claim_id,
            other => panic!("expected Claimed, got {other:?}"),
        };
        let mut h = sha2::Sha256::new();
        h.update(output_json.as_bytes());
        let hash = format!("{:x}", h.finalize());
        store
            .complete_step_cas(run_id, step_id, claim_id, output_json, &hash, 1, 2_000)
            .unwrap();
        hash
    }

    async fn get_request(app: &Router, uri: &str) -> (StatusCode, Vec<u8>) {
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, bytes)
    }

    #[tokio::test]
    async fn get_blob_returns_bytes_for_referenced_hash() {
        let (state, _dir) = fresh_state_with_blob_store();
        let payload = "\"".to_string()
            + &"a".repeat(boruna_orchestrator::persistence::BLOB_THRESHOLD + 1)
            + "\"";
        let hash = complete_running_step_in_state(&state, "RUN-1", "s1", &payload);
        let app = build_router(state);
        let (status, bytes) = get_request(&app, &format!("/api/runs/RUN-1/blobs/{hash}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, payload.as_bytes());
    }

    #[tokio::test]
    async fn get_blob_bad_hash_short() {
        let (state, _dir) = fresh_state_with_blob_store();
        let app = build_router(state);
        let (status, bytes) = get_request(&app, "/api/runs/RUN-1/blobs/abc").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error_kind"], "coord.blobs.bad_hash");
    }

    #[tokio::test]
    async fn get_blob_bad_hash_uppercase() {
        let (state, _dir) = fresh_state_with_blob_store();
        let app = build_router(state);
        // 64 chars but uppercase → format check fails before scope check.
        let bad = "A".repeat(64);
        let (status, bytes) = get_request(&app, &format!("/api/runs/RUN-1/blobs/{bad}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error_kind"], "coord.blobs.bad_hash");
    }

    #[tokio::test]
    async fn get_blob_not_found_unknown_hash() {
        let (state, _dir) = fresh_state_with_blob_store();
        let app = build_router(state);
        let bogus = "0".repeat(64);
        let (status, bytes) = get_request(&app, &format!("/api/runs/RUN-1/blobs/{bogus}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error_kind"], "coord.blobs.not_found");
    }

    #[tokio::test]
    async fn get_blob_run_scope_enforced() {
        // Run-A produces a blob; GET on run-B for the same hash → 404.
        let (state, _dir) = fresh_state_with_blob_store();
        let payload = "\"".to_string()
            + &"q".repeat(boruna_orchestrator::persistence::BLOB_THRESHOLD + 1)
            + "\"";
        let hash = complete_running_step_in_state(&state, "RUN-A", "s1", &payload);
        let app = build_router(state);
        let (status, bytes) = get_request(&app, &format!("/api/runs/RUN-B/blobs/{hash}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error_kind"], "coord.blobs.not_found");
    }

    #[tokio::test]
    async fn get_blob_unauthorized_no_bearer() {
        let (mut state, _dir) = fresh_state_with_blob_store();
        state.config.shared_secret = Some("super-secret".into());
        let app = build_router(state);
        let bogus = "0".repeat(64);
        let (status, bytes) = get_request(&app, &format!("/api/runs/RUN-1/blobs/{bogus}")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error_kind"], "coord.unauthorized");
    }

    // Sprint W2 — health endpoint + auth bypass.

    #[tokio::test]
    async fn health_returns_ready_status_and_metadata() {
        let state = fresh_state();
        let app = build_router(state.clone());
        let (status, bytes) = get_request(&app, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["protocol_version"], 1);
        assert_eq!(v["status"], "ready");
        assert_eq!(v["boruna_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["capability_set_hash"], state.capability_set_hash);
        assert!(
            v["uptime_ms"].as_i64().unwrap() >= 0,
            "uptime must be non-negative"
        );
    }

    #[tokio::test]
    async fn health_bypasses_auth_when_secret_configured() {
        // Per W2 design: load balancers and external probes don't
        // hold the bearer secret. /api/health must answer 200 even
        // with the secret enabled.
        let mut state = fresh_state();
        state.config.shared_secret = Some("super-secret-token-not-leaked".into());
        let app = build_router(state);
        let (status, bytes) = get_request(&app, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["status"], "ready");
        // The secret itself MUST NOT leak into health output.
        let body_str = String::from_utf8_lossy(&bytes);
        assert!(
            !body_str.contains("super-secret-token-not-leaked"),
            "shared secret leaked into /api/health response"
        );
    }

    #[tokio::test]
    async fn other_routes_still_require_auth_when_secret_configured() {
        // Sanity: the W2 health bypass MUST NOT relax auth on
        // any other route. Verify a non-health GET still 401s.
        let mut state = fresh_state();
        state.config.shared_secret = Some("super-secret".into());
        let app = build_router(state);
        let (status, _bytes) = get_request(&app, "/api/runs").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
