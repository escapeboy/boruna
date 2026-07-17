/// `boruna evidence serve` — web-based evidence bundle inspector (post1-T-4.4).
///
/// Starts a local axum HTTP server that renders the bundle in a browser.
/// Bundle data is loaded once at startup; requests are stateless reads.
use std::collections::BTreeMap;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use boruna_orchestrator::audit::{
    evidence::{BundleJson, BundleManifest},
    log::{AuditEntry, AuditEvent},
    verify::verify_bundle,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Bundle data loaded at startup
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct BundleData {
    pub manifest: BundleManifest,
    pub bundle_meta: Option<BundleJson>,
    pub audit_entries: Vec<AuditEntry>,
    /// step_name → pretty-printed JSON
    pub outputs: BTreeMap<String, String>,
    pub verify_valid: bool,
    pub verify_errors: Vec<String>,
}

pub(crate) fn load_bundle(dir: &Path) -> Result<BundleData, Box<dyn std::error::Error>> {
    // manifest.json — required
    let manifest: BundleManifest = serde_json::from_str(
        &fs::read_to_string(dir.join("manifest.json"))
            .map_err(|e| format!("cannot read manifest.json: {e}"))?,
    )
    .map_err(|e| format!("invalid manifest.json: {e}"))?;

    // bundle.json — optional
    let bundle_meta: Option<BundleJson> = fs::read_to_string(dir.join("bundle.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    // audit.log — optional (may not exist for partial bundles)
    let audit_entries: Vec<AuditEntry> = fs::read_to_string(dir.join("audit.log"))
        .map(|content| {
            content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default();

    // outputs/<step>/result.json
    let mut outputs = BTreeMap::new();
    let outputs_dir = dir.join("outputs");
    if outputs_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&outputs_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let step_name = entry.file_name().to_string_lossy().into_owned();
                    let result_path = entry.path().join("result.json");
                    if let Ok(raw) = fs::read_to_string(&result_path) {
                        let pretty = serde_json::from_str::<Value>(&raw)
                            .ok()
                            .and_then(|v| serde_json::to_string_pretty(&v).ok())
                            .unwrap_or(raw);
                        outputs.insert(step_name, pretty);
                    }
                }
            }
        }
    }

    // verify
    let result = verify_bundle(dir);

    Ok(BundleData {
        manifest,
        bundle_meta,
        audit_entries,
        outputs,
        verify_valid: result.valid,
        verify_errors: result.errors,
    })
}

// ---------------------------------------------------------------------------
// HTML helpers
// ---------------------------------------------------------------------------

fn page(title: &str, active: &str, body: &str) -> String {
    // "you are here" cue: mark the matching nav link with aria-current.
    let cur = |key: &str| {
        if key == active {
            r#" aria-current="page""#
        } else {
            ""
        }
    };
    let nav = format!(
        r#"<a href="/bundle"{b}>Overview</a>
    <a href="/audit"{a}>Audit Log</a>
    <a href="/outputs"{o}>Outputs</a>
    <a href="/api/bundle"{j}>JSON API</a>"#,
        b = cur("bundle"),
        a = cur("audit"),
        o = cur("outputs"),
        j = cur("api"),
    );
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title} — Boruna Evidence</title>
<style>
:root{{color-scheme:light dark}}
*{{box-sizing:border-box;margin:0;padding:0}}
body{{font-family:system-ui,-apple-system,sans-serif;background:#f5f5f5;color:#1a1a2e;line-height:1.5}}
a{{color:#2563eb}}
header{{background:#1a1a2e;color:#fff;padding:12px 24px;display:flex;flex-wrap:wrap;gap:6px 24px;align-items:baseline}}
header h1{{font-size:1.05rem;font-weight:600}}
nav{{display:flex;flex-wrap:wrap;gap:4px 14px;font-size:.9rem}}
nav a{{color:#a0c4ff;text-decoration:none;padding:2px 6px;border-radius:3px}}
nav a:hover{{text-decoration:underline}}
nav a[aria-current="page"]{{color:#fff;background:rgba(255,255,255,.15);font-weight:600}}
main{{max-width:1100px;margin:24px auto;padding:0 16px}}
h2{{font-size:1.05rem;font-weight:600;margin:26px 0 12px;color:#1a1a2e}}
p.lead{{color:#444;font-size:.9rem;margin-bottom:16px;max-width:74ch}}
.hint{{color:#666;font-size:.8rem;margin:-2px 0 12px;max-width:82ch}}
.card{{background:#fff;border:1px solid #ddd;border-radius:6px;padding:20px;margin-bottom:20px}}
.kv{{display:grid;grid-template-columns:190px 1fr;gap:8px 16px}}
@media(max-width:560px){{.kv{{grid-template-columns:1fr}}.kv .k{{margin-top:8px}}}}
.kv .k{{font-weight:600;color:#555;font-size:.85rem}}
.kv .v{{font-size:.85rem;word-break:break-all}}
.badge{{display:inline-block;padding:2px 10px;border-radius:12px;font-size:.8rem;font-weight:600}}
.pass{{background:#d1fae5;color:#065f46}}
.fail{{background:#fee2e2;color:#991b1b}}
.table-wrap{{overflow-x:auto;-webkit-overflow-scrolling:touch}}
table{{width:100%;border-collapse:collapse;font-size:.85rem}}
caption{{text-align:left;color:#666;font-size:.8rem;padding:0 0 10px}}
th{{background:#eee;text-align:left;padding:8px 10px;border-bottom:2px solid #ddd;white-space:nowrap}}
td{{padding:7px 10px;border-bottom:1px solid #eee;vertical-align:top}}
tr:hover td{{background:#f9f9f9}}
.mono{{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.8rem;overflow-wrap:anywhere}}
details{{margin-bottom:12px}}
summary{{cursor:pointer;padding:10px;background:#eef;border:1px solid #ccd;border-radius:4px;font-weight:600;font-size:.9rem}}
pre{{background:#1e1e1e;color:#d4d4d4;padding:14px;border-radius:4px;overflow:auto;font-size:.8rem;margin-top:8px}}
.warn{{background:#fffbeb;border:1px solid #f59e0b;padding:12px 16px;border-radius:4px;color:#78350f;font-size:.85rem;margin-bottom:16px}}
.empty{{color:#666;font-size:.9rem;background:#fafafa;border:1px dashed #ccc;border-radius:6px;padding:16px;max-width:82ch}}
footer{{max-width:1100px;margin:8px auto 40px;padding:16px;color:#888;font-size:.75rem;border-top:1px solid #e5e5e5}}
@media (prefers-color-scheme: dark){{
 body{{background:#14141f;color:#e5e7eb}}
 a{{color:#93c5fd}}
 h2{{color:#c7d2fe}}
 p.lead{{color:#c9cbd1}}
 .hint{{color:#9aa0ad}}
 .card{{background:#1e1e2e;border-color:#33334a}}
 .kv .k{{color:#a9adbb}}
 th{{background:#26263a;border-color:#33334a}}
 td{{border-color:#2a2a3d}}
 tr:hover td{{background:#242438}}
 summary{{background:#26263a;border-color:#3a3a55;color:#e5e7eb}}
 caption{{color:#9aa0ad}}
 .empty{{background:#1b1b28;border-color:#3a3a55;color:#a9adbb}}
 footer{{color:#7b7f8c;border-color:#2a2a3d}}
}}
</style>
</head>
<body>
<header>
  <h1>Boruna Evidence Inspector</h1>
  <nav aria-label="Evidence bundle sections">
    {nav}
  </nav>
</header>
<main>
{body}
</main>
<footer>
  Read-only inspector for one evidence bundle. Runs on <span class="mono">127.0.0.1</span> with no
  authentication — do not expose it to a network. Verification and export are also available via
  <span class="mono">boruna evidence verify</span> / <span class="mono">inspect</span>.
</footer>
</body>
</html>"#
    )
}

/// Minimal HTML escaper for untrusted bundle-derived text. Bundle content (step
/// outputs, filenames, workflow names, error strings) can carry attacker-influenced
/// text, so every interpolation of it into these pages MUST be escaped — otherwise
/// inspecting a hostile bundle executes stored XSS against the `127.0.0.1` origin.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

pub(crate) fn render_bundle_page(data: &BundleData) -> String {
    let verify_badge = if data.verify_valid {
        r#"<span class="badge pass">✓ VALID</span>"#
    } else {
        r#"<span class="badge fail">✗ INVALID</span>"#
    };

    let errors_html = if data.verify_errors.is_empty() {
        String::new()
    } else {
        let items: String = data
            .verify_errors
            .iter()
            .map(|e| format!("<li>{}</li>", esc(e)))
            .collect();
        format!(r#"<div class="warn"><strong>Verification errors:</strong><ul>{items}</ul></div>"#)
    };

    let enc_row = match &data.manifest.encryption {
        Some(info) => format!(
            r#"<div class="k">encrypted</div><div class="v">yes (algorithm={}, kek_id={})</div>"#,
            esc(&info.algorithm),
            esc(&info.kek_id)
        ),
        None => String::from(r#"<div class="k">encrypted</div><div class="v">no</div>"#),
    };

    let format_version = esc(data
        .bundle_meta
        .as_ref()
        .map(|b| b.format_version.as_str())
        .unwrap_or("(missing bundle.json)"));

    // Explain what VALID / INVALID actually proves — the whole point of the page.
    let verify_hint = if data.verify_valid {
        "Every file in this bundle matches the SHA-256 checksum recorded in the manifest, and the \
         audit log's hash chain is intact. Nothing has been altered since the run was sealed."
    } else {
        "One or more checks failed — a file, hash, or audit-chain link does not match what was \
         sealed at run time. Treat this bundle as untrustworthy until the errors below are resolved."
    };

    let body = format!(
        r#"<h2>Bundle Overview</h2>
<p class="lead">An <strong>evidence bundle</strong> is a tamper-evident, self-contained record of one
workflow run: its inputs' hashes, a hash-chained audit log of every step, the recorded outputs, and a
manifest of SHA-256 checksums that ties it all together. This page verifies that record and shows what
it contains.</p>
{errors_html}
<div class="card">
  <div class="kv">
    <div class="k">verification</div><div class="v">{verify_badge}</div>
  </div>
  <p class="hint">{verify_hint}</p>
  <div class="kv" style="margin-top:12px">
    <div class="k">run_id</div><div class="v mono" title="{run_id}">{run_id}</div>
    <div class="k">workflow</div><div class="v">{workflow}</div>
    <div class="k">started_at</div><div class="v">{started}</div>
    <div class="k">completed_at</div><div class="v">{completed}</div>
    <div class="k">format_version</div><div class="v">{format_version}</div>
    {enc_row}
  </div>
  <p class="hint">Look up this same <span class="mono">run_id</span> in the Boruna Workflow
  Dashboard to see the live run and its per-step status.</p>
</div>
<h2>Hashes</h2>
<p class="hint">Fingerprints that make the bundle tamper-evident. <span class="mono">bundle_hash</span>
covers the whole bundle; <span class="mono">workflow_hash</span> / <span class="mono">policy_hash</span>
identify exactly which workflow and policy ran; <span class="mono">audit_log_hash</span> seals the audit
chain. Re-running the same inputs reproduces the same hashes.</p>
<div class="card">
  <div class="kv">
    <div class="k">bundle_hash</div><div class="v mono" title="{bundle_hash}">{bundle_hash}</div>
    <div class="k">workflow_hash</div><div class="v mono" title="{workflow_hash}">{workflow_hash}</div>
    <div class="k">policy_hash</div><div class="v mono" title="{policy_hash}">{policy_hash}</div>
    <div class="k">audit_log_hash</div><div class="v mono" title="{audit_hash}">{audit_hash}</div>
  </div>
</div>
<h2>File Checksums ({file_count} files)</h2>
<div class="card">
  <div class="table-wrap">
  <table>
    <caption>Each row is a file in the bundle and the SHA-256 it must match. Verification recomputes
    these; any mismatch flips the bundle to INVALID. Hover a value to read it in full.</caption>
    <thead><tr><th scope="col">File</th><th scope="col">SHA-256</th></tr></thead>
    <tbody>{file_rows}</tbody>
  </table>
  </div>
</div>"#,
        run_id = esc(&data.manifest.run_id),
        workflow = esc(&data.manifest.workflow_name),
        started = esc(&data.manifest.started_at),
        completed = esc(&data.manifest.completed_at),
        bundle_hash = esc(&data.manifest.bundle_hash),
        workflow_hash = esc(&data.manifest.workflow_hash),
        policy_hash = esc(&data.manifest.policy_hash),
        audit_hash = esc(&data.manifest.audit_log_hash),
        file_count = data.manifest.file_checksums.len(),
        file_rows = data
            .manifest
            .file_checksums
            .iter()
            .map(|(f, h)| {
                format!(
                    r#"<tr><td class="mono">{}</td><td class="mono" title="{h}">{h}</td></tr>"#,
                    esc(f),
                    h = esc(h)
                )
            })
            .collect::<String>(),
    );
    page("Bundle", "bundle", &body)
}

pub(crate) fn render_audit_page(data: &BundleData) -> String {
    let rows: String = data
        .audit_entries
        .iter()
        .map(|entry| {
            let (step, event_type, detail) = describe_event(&entry.event);
            format!(
                r#"<tr>
  <td class="mono">{}</td>
  <td>{step}</td>
  <td>{event_type}</td>
  <td class="mono" style="font-size:.75rem">{detail}</td>
</tr>"#,
                entry.sequence,
                step = esc(&step),
                detail = esc(&detail)
            )
        })
        .collect();

    let inner = if data.audit_entries.is_empty() {
        String::from(
            r#"<div class="card"><p class="empty">This bundle recorded no audit entries. That usually
means a partial or demo-mode bundle — a fully recorded run logs one entry per lifecycle event
(workflow started, each step started/completed, policy checks, and workflow completed).</p></div>"#,
        )
    } else {
        format!(
            r#"<div class="card" style="padding:0">
  <div class="table-wrap">
  <table>
    <caption style="padding:12px 14px 0">Chronological, append-only record of the run. Each entry is
    hash-linked to the one before it (entry #N embeds the hash of #N-1), so removing or editing any
    line breaks the chain and fails verification.</caption>
    <thead><tr><th scope="col">#</th><th scope="col">Step</th><th scope="col">Event</th><th scope="col">Detail</th></tr></thead>
    <tbody>{rows}</tbody>
  </table>
  </div>
</div>"#
        )
    };

    let body = format!(
        r#"<h2>Audit Log ({count} entries)</h2>
{inner}"#,
        count = data.audit_entries.len()
    );
    page("Audit Log", "audit", &body)
}

fn describe_event(event: &AuditEvent) -> (String, &'static str, String) {
    match event {
        AuditEvent::WorkflowStarted {
            workflow_hash,
            policy_hash,
        } => (
            String::new(),
            "WorkflowStarted",
            format!("workflow={workflow_hash} policy={policy_hash}"),
        ),
        AuditEvent::StepStarted {
            step_id,
            input_hash,
        } => (
            step_id.clone(),
            "StepStarted",
            format!("input={input_hash}"),
        ),
        AuditEvent::StepCompleted {
            step_id,
            output_hash,
            duration_ms,
        } => (
            step_id.clone(),
            "StepCompleted",
            format!("output={output_hash} dur={duration_ms}ms"),
        ),
        AuditEvent::StepFailed { step_id, error } => (step_id.clone(), "StepFailed", error.clone()),
        AuditEvent::CapabilityInvoked {
            step_id,
            capability,
            allowed,
        } => (
            step_id.clone(),
            "CapabilityInvoked",
            format!("{capability} allowed={allowed}"),
        ),
        AuditEvent::PolicyEvaluated {
            step_id,
            rule,
            decision,
        } => (
            step_id.clone(),
            "PolicyEvaluated",
            format!("{rule} → {decision}"),
        ),
        AuditEvent::BudgetConsumed {
            step_id,
            tokens,
            remaining,
        } => (
            step_id.clone(),
            "BudgetConsumed",
            format!("tokens={tokens} remaining={remaining}"),
        ),
        AuditEvent::ApprovalRequested { step_id, role } => {
            (step_id.clone(), "ApprovalRequested", role.clone())
        }
        AuditEvent::ApprovalGranted { step_id, approver } => {
            (step_id.clone(), "ApprovalGranted", approver.clone())
        }
        AuditEvent::ApprovalDenied { step_id, reason } => {
            (step_id.clone(), "ApprovalDenied", reason.clone())
        }
        AuditEvent::ExternalTriggerReceived {
            step_id,
            payload_hash,
        } => (
            step_id.clone(),
            "ExternalTriggerReceived",
            format!("payload={payload_hash}"),
        ),
        AuditEvent::WorkflowCompleted {
            result_hash,
            total_duration_ms,
        } => (
            String::new(),
            "WorkflowCompleted",
            format!("result={result_hash} dur={total_duration_ms}ms"),
        ),
    }
}

pub(crate) fn render_outputs_page(data: &BundleData) -> String {
    let items: String = if data.outputs.is_empty() {
        String::from(
            r#"<p class="empty">No outputs found in this bundle. Demo-mode runs and workflows that
don't record step results show nothing here — the run may still be valid. Click a step below (when
present) to expand its recorded JSON result.</p>"#,
        )
    } else {
        data.outputs
            .iter()
            .map(|(step, json)| {
                format!(
                    r#"<details>
  <summary>{}</summary>
  <pre>{}</pre>
</details>"#,
                    esc(step),
                    esc(json)
                )
            })
            .collect()
    };

    let body = format!(
        r#"<h2>Step Outputs ({count} steps)</h2>
<p class="lead">The exact JSON result each step produced, as it was sealed into the bundle. These are
the recorded outputs verified by the checksums on the Overview page.</p>
<div class="card">{items}</div>"#,
        count = data.outputs.len()
    );
    page("Outputs", "outputs", &body)
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

type AppState = Arc<BundleData>;

async fn handle_root() -> Redirect {
    Redirect::permanent("/bundle")
}

async fn handle_bundle(State(data): State<AppState>) -> Html<String> {
    Html(render_bundle_page(&data))
}

async fn handle_audit(State(data): State<AppState>) -> Html<String> {
    Html(render_audit_page(&data))
}

async fn handle_outputs(State(data): State<AppState>) -> Html<String> {
    Html(render_outputs_page(&data))
}

async fn handle_api_bundle(State(data): State<AppState>) -> Response {
    let merged = serde_json::json!({
        "manifest": data.manifest,
        "bundle_meta": data.bundle_meta,
        "audit_entries": data.audit_entries,
        "outputs": data.outputs,
        "verify": {
            "valid": data.verify_valid,
            "errors": data.verify_errors,
        },
    });
    let json = serde_json::to_string_pretty(&merged).unwrap_or_default();
    ([("content-type", "application/json")], json).into_response()
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn serve(bundle_dir: &Path, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let data = load_bundle(bundle_dir).map_err(|e| format!("failed to load bundle: {e}"))?;

    let url = format!("http://localhost:{port}/bundle");
    if data.verify_valid {
        println!("evidence bundle: VALID");
    } else {
        eprintln!("warning: bundle verification failed:");
        for err in &data.verify_errors {
            eprintln!("  {err}");
        }
    }
    println!("serving evidence bundle at {url}");
    println!("press Ctrl-C to stop");

    // Try to open the browser (best-effort; ignore failure)
    let _ = open_browser(&url);

    let state: AppState = Arc::new(data);
    let app = Router::new()
        .route("/", get(handle_root))
        .route("/bundle", get(handle_bundle))
        .route("/audit", get(handle_audit))
        .route("/outputs", get(handle_outputs))
        .route("/api/bundle", get(handle_api_bundle))
        .with_state(state);

    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", url])
            .spawn()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use boruna_orchestrator::audit::{
        evidence::BundleManifest, fingerprint::EnvFingerprint, log::AuditEntry,
    };
    use std::collections::BTreeMap;

    fn make_data(verify_valid: bool) -> BundleData {
        BundleData {
            manifest: BundleManifest {
                schema_version: 1,
                run_id: String::from("abc123"),
                workflow_name: String::from("test_workflow"),
                workflow_hash: String::from("wh"),
                policy_hash: String::from("ph"),
                audit_log_hash: String::from("alh"),
                file_checksums: BTreeMap::new(),
                env_fingerprint: EnvFingerprint::capture(),
                started_at: String::from("2026-01-01T00:00:00Z"),
                completed_at: String::from("2026-01-01T00:01:00Z"),
                bundle_hash: String::from("bh"),
                encryption: None,
                signature: None,
            },
            bundle_meta: None,
            audit_entries: Vec::new(),
            outputs: BTreeMap::new(),
            verify_valid,
            verify_errors: if verify_valid {
                vec![]
            } else {
                vec![String::from("bad hash")]
            },
        }
    }

    #[test]
    fn render_bundle_page_valid_contains_pass() {
        let data = make_data(true);
        let html = render_bundle_page(&data);
        assert!(html.contains("VALID"));
        assert!(html.contains("abc123"));
        assert!(html.contains("test_workflow"));
    }

    #[test]
    fn render_bundle_page_invalid_contains_fail() {
        let data = make_data(false);
        let html = render_bundle_page(&data);
        assert!(html.contains("INVALID"));
        assert!(html.contains("bad hash"));
    }

    #[test]
    fn render_audit_page_empty() {
        let data = make_data(true);
        let html = render_audit_page(&data);
        assert!(html.contains("0 entries"));
    }

    #[test]
    fn render_audit_page_with_entry() {
        let mut data = make_data(true);
        data.audit_entries.push(AuditEntry {
            sequence: 1,
            prev_hash: String::from("0000"),
            event: AuditEvent::StepStarted {
                step_id: String::from("step_a"),
                input_hash: String::from("ihash"),
            },
            entry_hash: String::from("ehash"),
        });
        let html = render_audit_page(&data);
        assert!(html.contains("1 entries"));
        assert!(html.contains("StepStarted"));
        assert!(html.contains("step_a"));
    }

    #[test]
    fn render_outputs_page_with_output() {
        let mut data = make_data(true);
        data.outputs
            .insert(String::from("step_a"), String::from(r#"{"ok":true}"#));
        let html = render_outputs_page(&data);
        assert!(html.contains("1 steps"));
        assert!(html.contains("step_a"));
        // Output is HTML-escaped (XSS defense): the raw quotes become &quot;.
        assert!(html.contains(r#"{&quot;ok&quot;:true}"#));
        assert!(!html.contains(r#"{"ok":true}"#));
    }

    #[test]
    fn render_outputs_page_escapes_script() {
        // A hostile bundle output must not inject live markup.
        let mut data = make_data(true);
        data.outputs.insert(
            String::from("evil"),
            String::from("<script>alert(1)</script>"),
        );
        let html = render_outputs_page(&data);
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn render_outputs_page_empty() {
        let data = make_data(true);
        let html = render_outputs_page(&data);
        assert!(html.contains("0 steps"));
        assert!(html.contains("No outputs found"));
    }

    #[test]
    fn describe_event_workflow_started() {
        let event = AuditEvent::WorkflowStarted {
            workflow_hash: String::from("wh"),
            policy_hash: String::from("ph"),
        };
        let (step, etype, detail) = describe_event(&event);
        assert_eq!(step, "");
        assert_eq!(etype, "WorkflowStarted");
        assert!(detail.contains("wh"));
    }
}
