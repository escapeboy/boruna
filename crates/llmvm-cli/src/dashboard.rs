//! Workflow dashboard — read-only HTTP view over `runs.db`
//! (sprint `0.4-S16`).
//!
//! See `docs/design-workflow-dashboard.md` and
//! `docs/architecture-workflow-dashboard.md` for the design rationale,
//! security posture, and route contract.
//!
//! ## Security posture
//!
//! - Loopback (127.0.0.1) by default. Operators who pass
//!   `--bind 0.0.0.0` get a loud warning at startup AND a banner in
//!   the rendered HTML.
//! - **No authentication.** This sprint ships dev-grade access only.
//!   Operators exposing the dashboard to a network MUST front it with
//!   an auth-enforcing reverse proxy.
//! - **Read-only.** Zero mutation routes. Confirmed by a regression
//!   test that asserts non-GET methods on every route return 405.
//!
//! ## Stability
//!
//! Per `docs/stability.md`: **experimental**. Route paths and flag
//! names are stable; rendered HTML is not. JSON shapes inherit the
//! stability of `RunRow`, `RunRecord`, `RunOperational`, and
//! `StepCheckpoint`.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::get;
use axum::{Json, Router};

use boruna_orchestrator::persistence::{
    RunCheckpointStore, RunOperational, RunRecord, RunRow, RunStatus, StepCheckpoint,
};
use serde::Serialize;

/// Shared state for handlers.
///
/// `bind_warning` is `Some(addr)` only when the dashboard was bound
/// to a non-loopback address; the index handler renders a banner in
/// that case.
#[derive(Clone)]
pub struct DashboardState {
    store: Arc<Mutex<RunCheckpointStore>>,
    bind_warning: Option<String>,
}

#[tokio::main]
pub async fn run_serve(
    data_dir: PathBuf,
    port: u16,
    bind: IpAddr,
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

    let bind_warning = if bind.is_loopback() {
        None
    } else {
        let msg = format!("{bind}:{port}");
        eprintln!(
            "[WARNING] dashboard bound to non-loopback {msg}; \
             anyone with network access to this port can READ all run data; \
             the dashboard ships no auth — front it with an auth-enforcing reverse proxy"
        );
        Some(msg)
    };

    let store_handle = Arc::new(Mutex::new(store));
    let app = dashboard_routes(store_handle, bind_warning);

    let addr = std::net::SocketAddr::new(bind, port);
    eprintln!("dashboard serving on http://{addr}");
    eprintln!("    data-dir: {}", data_dir.display());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the dashboard's read-only route surface. Public so the
/// coordinator (sprint `0.5-S2d`) can `.merge(...)` these onto
/// its own router and serve fleet visibility + distributed
/// dispatch from a single listener.
///
/// Takes primitive args (the shared store handle and an
/// optional `bind_warning` string for the banner) instead of
/// the internal `DashboardState` so the coordinator doesn't
/// have to reach into the dashboard's internals.
pub fn dashboard_routes(
    store: Arc<Mutex<RunCheckpointStore>>,
    bind_warning: Option<String>,
) -> Router {
    let state = DashboardState {
        store,
        bind_warning,
    };
    Router::new()
        .route("/", get(handle_index))
        .route("/runs/{id}", get(handle_run_detail))
        .route("/api/runs", get(handle_api_runs))
        .route("/api/runs/{id}", get(handle_api_run_detail))
        .with_state(state)
}

// ── Response shapes ──

/// Slim list-view of a run — no `policy_json` or `metadata_json`.
///
/// Sprint `0.4-S16` adversarial review found that returning full
/// `RunRow` in the list endpoint multiplied the disclosure surface:
/// every operator hitting `/api/runs` would see ALL runs' policies
/// and metadata. Operators sometimes embed secrets, hostnames, or
/// customer identifiers in `metadata_json`; serving them by default
/// in a no-auth dashboard is the wrong default.
///
/// The detail endpoint (`/api/runs/:id`) still returns the full
/// record — operator drilling in is a deliberate action.
#[derive(Serialize, Debug)]
struct RunSummary {
    run_id: String,
    workflow_name: String,
    workflow_hash: String,
    status: RunStatus,
    started_at_ms: i64,
    updated_at_ms: i64,
}

impl From<&RunRow> for RunSummary {
    fn from(row: &RunRow) -> Self {
        Self {
            run_id: row.run_id.clone(),
            workflow_name: row.workflow_name.clone(),
            workflow_hash: row.workflow_hash.clone(),
            status: row.status,
            started_at_ms: row.started_at_ms,
            updated_at_ms: row.updated_at_ms,
        }
    }
}

#[derive(Serialize, Debug)]
struct RunsListResponse {
    runs: Vec<RunSummary>,
}

#[derive(Serialize, Debug)]
struct RunDetailResponse {
    run: RunRecord,
    operational: Option<RunOperational>,
    steps: Vec<StepCheckpoint>,
}

// ── Handlers ──

async fn handle_index(State(state): State<DashboardState>) -> Result<Html<String>, StatusCode> {
    let runs = {
        let store = state
            .store
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        store
            .list_runs()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Html(render_index(&runs, state.bind_warning.as_deref())))
}

async fn handle_run_detail(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
) -> Result<Html<String>, StatusCode> {
    let (run, operational, steps) = load_run_detail(&state, &id)?;
    // Sprint 0.5-S7b: resolve per-step output for the HTML render.
    // For inline outputs we read the bytes; for blob-stored outputs
    // we hand the renderer the ref so it can emit a link to the
    // S7 blob route without fetching the bytes (avoids slurping
    // multi-MB blobs into a dashboard render).
    let outputs = resolve_step_outputs_for_render(&state, &id, &steps)?;
    Ok(Html(render_detail(
        &run,
        operational.as_ref(),
        &steps,
        &outputs,
        state.bind_warning.as_deref(),
    )))
}

async fn handle_api_runs(
    State(state): State<DashboardState>,
) -> Result<Json<RunsListResponse>, StatusCode> {
    let runs = {
        let store = state
            .store
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        store
            .list_runs()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let summaries: Vec<RunSummary> = runs.iter().map(RunSummary::from).collect();
    Ok(Json(RunsListResponse { runs: summaries }))
}

async fn handle_api_run_detail(
    State(state): State<DashboardState>,
    Path(id): Path<String>,
) -> Result<Json<RunDetailResponse>, StatusCode> {
    let (run, operational, steps) = load_run_detail(&state, &id)?;
    Ok(Json(RunDetailResponse {
        run,
        operational,
        steps,
    }))
}

fn load_run_detail(
    state: &DashboardState,
    id: &str,
) -> Result<(RunRecord, Option<RunOperational>, Vec<StepCheckpoint>), StatusCode> {
    let store = state
        .store
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let run = store
        .get_run_record(id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let operational = store
        .get_run_operational(id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let steps = store
        .list_step_checkpoints(id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((run, operational, steps))
}

/// Per-step output as it should render in the HTML detail page.
/// Sprint 0.5-S7b — distinguishes inline-rendered text from a
/// blob-link placeholder so `render_detail` can emit different
/// markup without re-querying the persistence layer.
#[derive(Debug, Clone)]
enum StepOutputDisplay {
    /// No output yet (Pending / Running / paused / Failed-without-output).
    None,
    /// Output stored inline; the value is the JSON-encoded text.
    /// Renderer applies truncation + html_escape.
    Inline(String),
    /// Output stored in the blob store. The hash is the
    /// `output_blob_ref` column value. Renderer emits a link to
    /// the coord/dashboard's blob route — does NOT fetch bytes
    /// (large blobs would bloat the dashboard render).
    Blob(String),
}

/// Resolve the per-step output rendering for the HTML detail page.
/// Reads through `read_step_output` for inline outputs; for
/// blob-stored steps it short-circuits to the ref (which lives on
/// the StepCheckpoint already) so we don't pull large bytes into
/// memory just to render a link.
fn resolve_step_outputs_for_render(
    state: &DashboardState,
    run_id: &str,
    steps: &[StepCheckpoint],
) -> Result<Vec<StepOutputDisplay>, StatusCode> {
    let store = state
        .store
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut out = Vec::with_capacity(steps.len());
    for cp in steps {
        // Blob-stored: skip the byte read; we want a link, not the bytes.
        if let Some(hash) = &cp.output_blob_ref {
            out.push(StepOutputDisplay::Blob(hash.clone()));
            continue;
        }
        // Inline (or no output yet) — read_step_output handles both,
        // returning Some(json) for completed-inline and None for
        // pending/running/failed-without-output.
        match store.read_step_output(run_id, &cp.step_id) {
            Ok(Some(json)) => out.push(StepOutputDisplay::Inline(json)),
            Ok(None) => out.push(StepOutputDisplay::None),
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
    Ok(out)
}

// ── Rendering ──

/// Escape HTML special characters. Every value rendered into the
/// HTML output must go through this helper. Operator-controlled
/// run_ids and workflow_names are operational state but could in
/// principle contain XSS payloads (especially when run_ids come
/// from external triggers).
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

const PAGE_STYLE: &str = r#"
<style>
body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
       max-width: 1200px; margin: 2rem auto; padding: 0 1rem; color: #222; }
h1, h2 { font-weight: 600; }
table { border-collapse: collapse; width: 100%; margin: 1rem 0; }
th, td { text-align: left; padding: 0.5rem 0.75rem; border-bottom: 1px solid #eee; }
th { background: #f7f7f7; font-weight: 600; }
tr:hover { background: #fafafa; }
a { color: #2563eb; text-decoration: none; }
a:hover { text-decoration: underline; }
.status-running { color: #2563eb; }
.status-paused { color: #d97706; }
.status-completed { color: #16a34a; }
.status-failed { color: #dc2626; }
.banner { padding: 0.75rem 1rem; background: #fee; color: #991b1b;
          border: 1px solid #fca5a5; border-radius: 4px; margin-bottom: 1rem; }
code { background: #f3f4f6; padding: 0.1rem 0.3rem; border-radius: 3px;
       font-family: ui-monospace, SFMono-Regular, monospace; }
.muted { color: #6b7280; font-size: 0.9rem; }
</style>
"#;

fn render_banner(bind_warning: Option<&str>) -> String {
    match bind_warning {
        Some(addr) => format!(
            r#"<div class="banner"><strong>WARNING:</strong> dashboard is bound to <code>{}</code> — \
               anyone with network access can READ all run data; this dashboard ships no auth.</div>"#,
            html_escape(addr)
        ),
        None => String::new(),
    }
}

fn render_index(runs: &[RunRow], bind_warning: Option<&str>) -> String {
    let banner = render_banner(bind_warning);
    let mut body = String::new();
    body.push_str("<h1>Boruna runs</h1>");
    body.push_str(&format!(
        r#"<p class="muted">{} run{} total</p>"#,
        runs.len(),
        if runs.len() == 1 { "" } else { "s" }
    ));

    if runs.is_empty() {
        body.push_str(r#"<p class="muted">No runs yet. Start one with <code>boruna workflow run ...</code>.</p>"#);
    } else {
        body.push_str("<table><thead><tr>");
        body.push_str(
            "<th>Run ID</th><th>Workflow</th><th>Status</th><th>Started</th><th>Updated</th>",
        );
        body.push_str("</tr></thead><tbody>");
        for run in runs {
            let status_class = format!("status-{}", run.status.as_str());
            body.push_str(&format!(
                r#"<tr><td><a href="/runs/{}">{}</a></td><td>{}</td><td class="{}">{}</td><td>{}</td><td>{}</td></tr>"#,
                html_escape(&run.run_id),
                html_escape(&run.run_id),
                html_escape(&run.workflow_name),
                status_class,
                run.status.as_str(),
                format_ms(run.started_at_ms),
                format_ms(run.updated_at_ms),
            ));
        }
        body.push_str("</tbody></table>");
    }

    body.push_str(r#"<p class="muted">JSON: <code>GET /api/runs</code></p>"#);

    wrap_page("Boruna runs", &banner, &body)
}

/// Maximum chars of inline output rendered in the HTML detail
/// page. Sprint 0.5-S7b. Truncated outputs append `…` to signal
/// elision. The full bytes are still in the persistence layer
/// (or the blob store); operators can fetch via `boruna evidence
/// inspect` or the JSON API for the full content.
const HTML_INLINE_OUTPUT_MAX: usize = 256;

/// Visible chars of a blob hash in the HTML detail page link
/// label. Sprint 0.5-S7b. The full 64-char hash is preserved in
/// the link target; the visible label is shortened so the table
/// stays readable.
const HTML_BLOB_HASH_LABEL_LEN: usize = 16;

fn render_detail(
    run: &RunRecord,
    operational: Option<&RunOperational>,
    steps: &[StepCheckpoint],
    outputs: &[StepOutputDisplay],
    bind_warning: Option<&str>,
) -> String {
    let banner = render_banner(bind_warning);
    let mut body = String::new();
    body.push_str(r#"<p class="muted"><a href="/">&larr; All runs</a></p>"#);
    body.push_str(&format!("<h1>Run {}</h1>", html_escape(&run.run_id)));

    body.push_str("<table>");
    body.push_str(&format!(
        "<tr><th>Workflow</th><td>{}</td></tr>",
        html_escape(&run.workflow_name)
    ));
    body.push_str(&format!(
        "<tr><th>Workflow hash</th><td><code>{}</code></td></tr>",
        html_escape(&run.workflow_hash)
    ));
    if let Some(op) = operational {
        let status_class = format!("status-{}", op.transient_status.as_str());
        body.push_str(&format!(
            r#"<tr><th>Status</th><td class="{}">{}</td></tr>"#,
            status_class,
            op.transient_status.as_str()
        ));
        body.push_str(&format!(
            "<tr><th>Started</th><td>{}</td></tr>",
            format_ms(op.started_at_ms)
        ));
        body.push_str(&format!(
            "<tr><th>Updated</th><td>{}</td></tr>",
            format_ms(op.updated_at_ms)
        ));
    }
    if let Some(t) = &run.terminal_status {
        body.push_str(&format!(
            "<tr><th>Terminal</th><td>{}</td></tr>",
            t.as_str()
        ));
    }
    body.push_str("</table>");

    body.push_str(&format!("<h2>Steps ({})</h2>", steps.len()));
    if steps.is_empty() {
        body.push_str(r#"<p class="muted">No step checkpoints recorded.</p>"#);
    } else {
        body.push_str("<table><thead><tr>");
        body.push_str(
            "<th>Step</th><th>Status</th><th>Attempts</th><th>Started</th><th>Ended</th><th>Output</th><th>Error</th>",
        );
        body.push_str("</tr></thead><tbody>");
        for (step, output) in steps.iter().zip(outputs.iter()) {
            let status_class = format!("status-{}", step.status.as_str());
            let started = step.started_at_ms.map(format_ms).unwrap_or_default();
            let ended = step.ended_at_ms.map(format_ms).unwrap_or_default();
            let error = step
                .error_msg
                .as_deref()
                .map(html_escape)
                .unwrap_or_default();
            let output_cell = render_output_cell(&run.run_id, output);
            body.push_str(&format!(
                r#"<tr><td><code>{}</code></td><td class="{}">{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>"#,
                html_escape(&step.step_id),
                status_class,
                step.status.as_str(),
                step.attempt_count,
                started,
                ended,
                output_cell,
                error,
            ));
        }
        body.push_str("</tbody></table>");
    }

    body.push_str(&format!(
        r#"<p class="muted">JSON: <code>GET /api/runs/{}</code></p>"#,
        html_escape(&run.run_id)
    ));

    wrap_page(&format!("Run {}", &run.run_id), &banner, &body)
}

/// Render a single step's Output cell. Sprint 0.5-S7b.
///
/// - `StepOutputDisplay::None` → render `—` (em dash).
/// - `StepOutputDisplay::Inline(json)` → render in a `<code>` block,
///   truncated to [`HTML_INLINE_OUTPUT_MAX`] chars (UTF-8 char
///   boundary safe). Contents pass through `html_escape`.
/// - `StepOutputDisplay::Blob(hash)` → render `[blob: <hash[..N]>…]`
///   linked to `/api/runs/{run_id}/blobs/{hash}`. The full hash is
///   in the URL; the visible label is shortened so the table stays
///   readable.
fn render_output_cell(run_id: &str, output: &StepOutputDisplay) -> String {
    match output {
        StepOutputDisplay::None => "—".to_string(),
        StepOutputDisplay::Inline(json) => {
            let truncated = truncate_chars(json, HTML_INLINE_OUTPUT_MAX);
            let suffix = if truncated.len() < json.len() {
                "…"
            } else {
                ""
            };
            format!("<code>{}{}</code>", html_escape(truncated), suffix)
        }
        StepOutputDisplay::Blob(hash) => {
            // Defensive: only build the link if hash matches the
            // expected 64-lowercase-hex shape. Otherwise the
            // blob-route will reject; render plain text.
            let valid = hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            if !valid {
                return format!("<code>[blob: {}]</code>", html_escape(hash));
            }
            let label_len = hash.len().min(HTML_BLOB_HASH_LABEL_LEN);
            let short = &hash[..label_len];
            format!(
                r#"<a href="/api/runs/{}/blobs/{}"><code>[blob: {}…]</code></a>"#,
                html_escape(run_id),
                html_escape(hash),
                html_escape(short),
            )
        }
    }
}

/// Truncate `s` to at most `max_chars` Unicode scalar values
/// (NOT bytes), staying on a char boundary so `html_escape`
/// receives a valid `&str`. Returns a borrowed prefix.
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    let mut end = s.len();
    for (count, (i, _)) in s.char_indices().enumerate() {
        if count == max_chars {
            end = i;
            break;
        }
    }
    &s[..end]
}

fn wrap_page(title: &str, banner: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>{}</title>{}</head>
<body>{}{}</body>
</html>"#,
        html_escape(title),
        PAGE_STYLE,
        banner,
        body
    )
}

/// Format a Unix epoch ms timestamp as ISO-8601 UTC with a `Z`
/// suffix. We don't pull `chrono` for this since `boruna-cli`
/// doesn't already depend on it; manual computation is fine for
/// a 19-character string.
fn format_ms(ms: i64) -> String {
    // Negative ms (clock skew or bad data) would produce malformed
    // output via the year-walking loop below. The 0 sentinel
    // already returns empty for missing timestamps; treat negatives
    // the same way. Caught by adversarial review for sprint
    // 0.4-S16.
    if ms <= 0 {
        return String::new();
    }
    // Convert to days since 1970-01-01 + intra-day seconds.
    let total_secs = ms.div_euclid(1000);
    let day_secs = total_secs.rem_euclid(86_400);
    let mut days = total_secs.div_euclid(86_400);
    let hours = (day_secs / 3600) as u32;
    let minutes = ((day_secs % 3600) / 60) as u32;
    let seconds = (day_secs % 60) as u32;

    // Convert days-since-epoch to YYYY-MM-DD via the standard
    // proleptic-Gregorian algorithm.
    let mut year: i64 = 1970;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let mut month: u32 = 1;
    let days_in_month = |m: u32, y: i64| -> i64 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap(y) => 29,
            2 => 28,
            _ => 0,
        }
    };
    while days >= days_in_month(month, year) {
        days -= days_in_month(month, year);
        month += 1;
    }
    let day = (days + 1) as u32;
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use boruna_orchestrator::persistence::{ClaimOutcome, RunStatus, StepStatus};

    fn fresh_store() -> RunCheckpointStore {
        RunCheckpointStore::open_in_memory().expect("open in-memory store")
    }

    fn sample_run(id: &str, name: &str, status: RunStatus) -> RunRow {
        RunRow {
            run_id: id.into(),
            workflow_name: name.into(),
            workflow_hash: "x".into(),
            status,
            started_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_500,
            policy_json: "{}".into(),
            metadata_json: "{}".into(),
        }
    }

    fn sample_step(run_id: &str, step_id: &str, status: StepStatus) -> StepCheckpoint {
        StepCheckpoint {
            run_id: run_id.into(),
            step_id: step_id.into(),
            status,
            output_json: None,
            output_hash: None,
            started_at_ms: Some(1_700_000_001_000),
            ended_at_ms: Some(1_700_000_002_000),
            error_msg: None,
            attempt_count: 1,
            worker_id: None,
            lease_expires_at_ms: None,
            claim_id: 0,
            output_blob_ref: None,
        }
    }

    fn state_with(store: RunCheckpointStore, bind_warning: Option<&str>) -> DashboardState {
        DashboardState {
            store: Arc::new(Mutex::new(store)),
            bind_warning: bind_warning.map(String::from),
        }
    }

    // ── Index handler ──

    #[tokio::test]
    async fn handle_index_empty_store_renders_empty_table() {
        let state = state_with(fresh_store(), None);
        let html = handle_index(State(state)).await.unwrap().0;
        assert!(html.contains("0 runs total"));
        assert!(html.contains("No runs yet"));
    }

    #[tokio::test]
    async fn handle_index_renders_runs_grouped_by_status() {
        let store = fresh_store();
        store
            .insert_run(&sample_run("r1", "wf-a", RunStatus::Running))
            .unwrap();
        store
            .insert_run(&sample_run("r2", "wf-b", RunStatus::Completed))
            .unwrap();
        store
            .insert_run(&sample_run("r3", "wf-a", RunStatus::Paused))
            .unwrap();
        let state = state_with(store, None);
        let html = handle_index(State(state)).await.unwrap().0;
        assert!(html.contains("3 runs total"));
        assert!(html.contains("r1") && html.contains("r2") && html.contains("r3"));
        assert!(html.contains("running") && html.contains("paused") && html.contains("completed"));
    }

    #[tokio::test]
    async fn handle_index_html_escapes_run_ids() {
        let store = fresh_store();
        store
            .insert_run(&sample_run(
                "<script>alert(1)</script>",
                "wf",
                RunStatus::Running,
            ))
            .unwrap();
        let state = state_with(store, None);
        let html = handle_index(State(state)).await.unwrap().0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert"));
    }

    #[tokio::test]
    async fn handle_index_warns_when_bound_non_loopback() {
        let state = state_with(fresh_store(), Some("0.0.0.0:8080"));
        let html = handle_index(State(state)).await.unwrap().0;
        assert!(html.contains("WARNING"));
        assert!(html.contains("0.0.0.0:8080"));
    }

    #[tokio::test]
    async fn handle_index_no_warning_when_loopback() {
        let state = state_with(fresh_store(), None);
        let html = handle_index(State(state)).await.unwrap().0;
        assert!(!html.contains("WARNING"));
        assert!(!html.contains("class=\"banner\""));
    }

    // ── Run detail handler ──

    #[tokio::test]
    async fn handle_run_detail_renders_run_and_steps() {
        let store = fresh_store();
        store
            .insert_run(&sample_run("r1", "wf", RunStatus::Running))
            .unwrap();
        store
            .upsert_step_checkpoint(&sample_step("r1", "extract", StepStatus::Completed))
            .unwrap();
        store
            .upsert_step_checkpoint(&sample_step("r1", "load", StepStatus::Failed))
            .unwrap();
        let state = state_with(store, None);
        let html = handle_run_detail(State(state), Path("r1".into()))
            .await
            .unwrap()
            .0;
        assert!(html.contains("Run r1"));
        assert!(html.contains("extract"));
        assert!(html.contains("load"));
        assert!(html.contains("Steps (2)"));
    }

    #[tokio::test]
    async fn handle_run_detail_404_for_unknown_id() {
        let state = state_with(fresh_store(), None);
        let err = handle_run_detail(State(state), Path("no-such-id".into()))
            .await
            .unwrap_err();
        assert_eq!(err, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn handle_run_detail_html_escapes_step_error_msg() {
        let store = fresh_store();
        store
            .insert_run(&sample_run("r1", "wf", RunStatus::Failed))
            .unwrap();
        let mut step = sample_step("r1", "extract", StepStatus::Failed);
        step.error_msg = Some("<x>boom</x>".into());
        store.upsert_step_checkpoint(&step).unwrap();
        let state = state_with(store, None);
        let html = handle_run_detail(State(state), Path("r1".into()))
            .await
            .unwrap()
            .0;
        assert!(html.contains("&lt;x&gt;boom&lt;/x&gt;"));
        assert!(!html.contains("<x>boom"));
    }

    // ── Sprint 0.5-S7b: Output column rendering ──

    /// Helper: build a fresh store with an on-disk blob store at a
    /// per-test tempdir so we can exercise the offload path. Returns
    /// a guard that holds the tempdir alive for the test.
    fn fresh_store_with_blobs() -> (RunCheckpointStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let blobs_root = dir.path().join("blobs");
        let store = RunCheckpointStore::open_in_memory_with_blob_store(blobs_root).unwrap();
        (store, dir)
    }

    /// Drive a step from Pending → Completed via the public claim/CAS
    /// path, exercising the threshold gate inside `complete_step_cas`.
    fn complete_step_in_store(
        store: &RunCheckpointStore,
        run_id: &str,
        step_id: &str,
        output_json: &str,
    ) -> String {
        use sha2::Digest;
        store
            .upsert_step_checkpoint(&sample_step(run_id, step_id, StepStatus::Pending))
            .unwrap();
        let claim = match store
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
            .complete_step_cas(run_id, step_id, claim, output_json, &hash, 1, 2_000)
            .unwrap();
        hash
    }

    #[tokio::test]
    async fn handle_run_detail_renders_inline_output_truncated() {
        let (store, _dir) = fresh_store_with_blobs();
        store
            .insert_run(&sample_run("r1", "wf", RunStatus::Running))
            .unwrap();
        // Small inline output → renders verbatim in the table cell.
        complete_step_in_store(&store, "r1", "small", "\"hello world\"");
        let state = state_with(store, None);
        let html = handle_run_detail(State(state), Path("r1".into()))
            .await
            .unwrap()
            .0;
        // Output column header present.
        assert!(html.contains("<th>Output</th>"));
        // Inline value rendered (with HTML-escaped quotes).
        assert!(
            html.contains("&quot;hello world&quot;"),
            "expected escaped JSON in render; got:\n{html}"
        );
    }

    #[tokio::test]
    async fn handle_run_detail_renders_blob_link_for_offloaded_output() {
        let (store, _dir) = fresh_store_with_blobs();
        store
            .insert_run(&sample_run("r1", "wf", RunStatus::Running))
            .unwrap();
        // 100 KiB output > BLOB_THRESHOLD → offloaded to blob store.
        let big = "\"".to_string()
            + &"q".repeat(boruna_orchestrator::persistence::BLOB_THRESHOLD + 1)
            + "\"";
        let hash = complete_step_in_store(&store, "r1", "big", &big);
        let state = state_with(store, None);
        let html = handle_run_detail(State(state), Path("r1".into()))
            .await
            .unwrap()
            .0;
        // Render must NOT include the literal 100 KiB output.
        assert!(
            !html.contains("qqqqqqqqqq"),
            "blob output bytes must not appear inline in the dashboard render"
        );
        // Render MUST include a blob link with the full hash in the
        // URL and a short label.
        let expected_href = format!(r#"href="/api/runs/r1/blobs/{hash}""#);
        assert!(
            html.contains(&expected_href),
            "expected blob link href to include full hash; got:\n{html}"
        );
        let expected_label = format!("[blob: {}", &hash[..16]);
        assert!(
            html.contains(&expected_label),
            "expected blob short-label '{expected_label}' in render; got:\n{html}"
        );
    }

    #[test]
    fn truncate_chars_handles_boundary_cases() {
        // Empty / max=0
        assert_eq!(truncate_chars("hello", 0), "");
        assert_eq!(truncate_chars("", 5), "");
        // Input shorter than max → returned whole
        assert_eq!(truncate_chars("abc", 10), "abc");
        // Exact boundary
        assert_eq!(truncate_chars("abcdef", 6), "abcdef");
        // Multi-byte char at boundary: "héllo" — é is 2 bytes
        // 4 chars would be "héll", which has length 5 (h=1, é=2, l=1, l=1).
        let s = "héllo";
        assert_eq!(s.chars().count(), 5);
        let t = truncate_chars(s, 4);
        assert_eq!(t.chars().count(), 4);
        assert_eq!(t, "héll");
    }

    #[test]
    fn render_output_cell_falls_back_for_malformed_blob_ref() {
        // Defensive branch: a row with an output_blob_ref that fails
        // hex validation (corruption / future schema drift) must still
        // render — as plain text, not a broken link.
        let bad = StepOutputDisplay::Blob("not-a-valid-hash".to_string());
        let html = render_output_cell("r1", &bad);
        assert!(
            !html.contains(r#"href=""#),
            "must NOT emit href for malformed hash; got: {html}"
        );
        assert!(html.contains("[blob: not-a-valid-hash]"));
    }

    #[tokio::test]
    async fn handle_run_detail_renders_em_dash_for_pending_output() {
        let store = fresh_store();
        store
            .insert_run(&sample_run("r1", "wf", RunStatus::Running))
            .unwrap();
        // Step exists in Pending state — no output yet.
        store
            .upsert_step_checkpoint(&sample_step("r1", "p", StepStatus::Pending))
            .unwrap();
        let state = state_with(store, None);
        let html = handle_run_detail(State(state), Path("r1".into()))
            .await
            .unwrap()
            .0;
        // The em dash placeholder must appear in a cell. We can't
        // do an exact match because the page can contain other em
        // dashes (e.g. headings); assert it appears in the steps
        // table by checking it follows the step_id row markup.
        assert!(
            html.contains("<td><code>p</code>"),
            "step row must be present"
        );
        assert!(html.contains("—"));
    }

    #[tokio::test]
    async fn handle_run_detail_warns_when_bound_non_loopback() {
        // Adversarial-review finding (HIGH): the detail page must
        // also render the bind-warning banner. Pre-fix it called
        // `wrap_page(..., "", ...)` (hard-coded empty banner),
        // contradicting the doc'd promise that the banner appears
        // on every page.
        let store = fresh_store();
        store
            .insert_run(&sample_run("r1", "wf", RunStatus::Running))
            .unwrap();
        let state = state_with(store, Some("0.0.0.0:8080"));
        let html = handle_run_detail(State(state), Path("r1".into()))
            .await
            .unwrap()
            .0;
        assert!(html.contains("WARNING"));
        assert!(html.contains("0.0.0.0:8080"));
    }

    // ── API handlers ──

    #[tokio::test]
    async fn handle_api_runs_returns_run_list_json() {
        let store = fresh_store();
        store
            .insert_run(&sample_run("r1", "wf", RunStatus::Running))
            .unwrap();
        store
            .insert_run(&sample_run("r2", "wf", RunStatus::Completed))
            .unwrap();
        let state = state_with(store, None);
        let resp = handle_api_runs(State(state)).await.unwrap().0;
        assert_eq!(resp.runs.len(), 2);
    }

    #[tokio::test]
    async fn handle_api_runs_empty_store_returns_empty_array() {
        let state = state_with(fresh_store(), None);
        let resp = handle_api_runs(State(state)).await.unwrap().0;
        assert!(resp.runs.is_empty());
    }

    #[tokio::test]
    async fn handle_api_runs_summary_does_not_leak_policy_or_metadata() {
        // Adversarial-review finding (MEDIUM): /api/runs returned
        // full RunRow including policy_json and metadata_json for
        // ALL runs in one shot. The slim RunSummary type is the
        // safer default. This test enforces that contract — adding
        // a sensitive field back to RunSummary later would have
        // to consciously update this assertion.
        let store = fresh_store();
        let mut row = sample_run("r1", "wf", RunStatus::Running);
        row.policy_json = r#"{"secret":"S3CR3T-API-KEY"}"#.into();
        row.metadata_json = r#"{"customer":"acme-corp"}"#.into();
        store.insert_run(&row).unwrap();
        let state = state_with(store, None);
        let resp = handle_api_runs(State(state)).await.unwrap().0;
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("S3CR3T-API-KEY"), "json: {json}");
        assert!(!json.contains("acme-corp"), "json: {json}");
        assert!(!json.contains("policy_json"), "json: {json}");
        assert!(!json.contains("metadata_json"), "json: {json}");
        // But the operator-relevant identifiers ARE present.
        assert!(json.contains("r1"));
        assert!(json.contains("wf"));
    }

    #[tokio::test]
    async fn handle_api_run_detail_returns_full_record() {
        let store = fresh_store();
        store
            .insert_run(&sample_run("r1", "wf", RunStatus::Running))
            .unwrap();
        store
            .upsert_step_checkpoint(&sample_step("r1", "extract", StepStatus::Completed))
            .unwrap();
        let state = state_with(store, None);
        let resp = handle_api_run_detail(State(state), Path("r1".into()))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.run.run_id, "r1");
        assert!(resp.operational.is_some());
        assert_eq!(resp.steps.len(), 1);
    }

    #[tokio::test]
    async fn handle_api_run_detail_404_for_unknown_id() {
        let state = state_with(fresh_store(), None);
        let err = handle_api_run_detail(State(state), Path("no-such-id".into()))
            .await
            .unwrap_err();
        assert_eq!(err, StatusCode::NOT_FOUND);
    }

    // ── Format helpers ──

    #[test]
    fn format_ms_zero_is_empty() {
        assert_eq!(format_ms(0), "");
    }

    #[test]
    fn format_ms_negative_is_empty() {
        // Adversarial-review finding (LOW): negative ms (clock
        // skew, bad data) used to walk the year-loop with a
        // negative `days` value and overflow into a u32 cast,
        // producing malformed output like "1970-01-4294967295T".
        // Now treated as missing.
        assert_eq!(format_ms(-1), "");
        assert_eq!(format_ms(-1_000_000_000_000), "");
    }

    #[test]
    fn format_ms_unix_epoch() {
        // 1700000000000 ms = 2023-11-14T22:13:20Z
        assert_eq!(format_ms(1_700_000_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn format_ms_leap_year_feb_29() {
        // 2024-02-29T00:00:00Z = 1709164800000
        assert_eq!(format_ms(1_709_164_800_000), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn html_escape_all_special_chars() {
        assert_eq!(html_escape("<>&\"'"), "&lt;&gt;&amp;&quot;&#x27;");
    }

    #[test]
    fn html_escape_passes_safe_chars() {
        assert_eq!(html_escape("hello world 123"), "hello world 123");
    }

    #[test]
    fn is_leap_year_known_values() {
        assert!(is_leap(2000));
        assert!(is_leap(2024));
        assert!(!is_leap(1900));
        assert!(!is_leap(2023));
    }
}
