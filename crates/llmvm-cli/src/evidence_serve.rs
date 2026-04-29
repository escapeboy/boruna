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

fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title} — Boruna Evidence</title>
<style>
*{{box-sizing:border-box;margin:0;padding:0}}
body{{font-family:system-ui,sans-serif;background:#f5f5f5;color:#222;line-height:1.5}}
header{{background:#1a1a2e;color:#fff;padding:12px 24px;display:flex;gap:24px;align-items:center}}
header h1{{font-size:1.1rem;font-weight:600}}
nav a{{color:#a0c4ff;text-decoration:none;font-size:.9rem}}
nav a:hover{{text-decoration:underline}}
main{{max-width:1100px;margin:24px auto;padding:0 16px}}
h2{{font-size:1.1rem;font-weight:600;margin-bottom:12px;color:#1a1a2e}}
.card{{background:#fff;border:1px solid #ddd;border-radius:6px;padding:20px;margin-bottom:20px}}
.kv{{display:grid;grid-template-columns:200px 1fr;gap:6px 16px}}
.kv .k{{font-weight:600;color:#555;font-size:.85rem}}
.kv .v{{font-size:.85rem;word-break:break-all}}
.badge{{display:inline-block;padding:2px 10px;border-radius:12px;font-size:.8rem;font-weight:600}}
.pass{{background:#d1fae5;color:#065f46}}
.fail{{background:#fee2e2;color:#991b1b}}
table{{width:100%;border-collapse:collapse;font-size:.85rem}}
th{{background:#eee;text-align:left;padding:8px 10px;border-bottom:2px solid #ddd}}
td{{padding:7px 10px;border-bottom:1px solid #eee;vertical-align:top}}
tr:hover td{{background:#f9f9f9}}
.mono{{font-family:monospace;font-size:.8rem}}
details{{margin-bottom:12px}}
summary{{cursor:pointer;padding:10px;background:#eef;border:1px solid #ccd;border-radius:4px;font-weight:600;font-size:.9rem}}
pre{{background:#1e1e1e;color:#d4d4d4;padding:14px;border-radius:4px;overflow:auto;font-size:.8rem;margin-top:8px}}
.warn{{background:#fffbeb;border:1px solid #f59e0b;padding:12px 16px;border-radius:4px;color:#78350f;font-size:.85rem;margin-bottom:16px}}
</style>
</head>
<body>
<header>
  <h1>Boruna Evidence Inspector</h1>
  <nav>
    <a href="/bundle">Bundle</a> &nbsp;|&nbsp;
    <a href="/audit">Audit Log</a> &nbsp;|&nbsp;
    <a href="/outputs">Outputs</a> &nbsp;|&nbsp;
    <a href="/api/bundle">JSON API</a>
  </nav>
</header>
<main>
{body}
</main>
</body>
</html>"#
    )
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
            .map(|e| format!("<li>{e}</li>"))
            .collect();
        format!(r#"<div class="warn"><strong>Verification errors:</strong><ul>{items}</ul></div>"#)
    };

    let enc_row = match &data.manifest.encryption {
        Some(info) => format!(
            r#"<div class="k">encrypted</div><div class="v">yes (algorithm={}, kek_id={})</div>"#,
            info.algorithm, info.kek_id
        ),
        None => String::from(r#"<div class="k">encrypted</div><div class="v">no</div>"#),
    };

    let format_version = data
        .bundle_meta
        .as_ref()
        .map(|b| b.format_version.as_str())
        .unwrap_or("(missing bundle.json)");

    let body = format!(
        r#"<h2>Bundle Overview</h2>
{errors_html}
<div class="card">
  <div class="kv">
    <div class="k">verification</div><div class="v">{verify_badge}</div>
    <div class="k">run_id</div><div class="v mono">{run_id}</div>
    <div class="k">workflow</div><div class="v">{workflow}</div>
    <div class="k">started_at</div><div class="v">{started}</div>
    <div class="k">completed_at</div><div class="v">{completed}</div>
    <div class="k">format_version</div><div class="v">{format_version}</div>
    {enc_row}
  </div>
</div>
<h2>Hashes</h2>
<div class="card">
  <div class="kv">
    <div class="k">bundle_hash</div><div class="v mono">{bundle_hash}</div>
    <div class="k">workflow_hash</div><div class="v mono">{workflow_hash}</div>
    <div class="k">policy_hash</div><div class="v mono">{policy_hash}</div>
    <div class="k">audit_log_hash</div><div class="v mono">{audit_hash}</div>
  </div>
</div>
<h2>File Checksums ({file_count} files)</h2>
<div class="card">
  <table>
    <tr><th>File</th><th>SHA-256</th></tr>
    {file_rows}
  </table>
</div>"#,
        run_id = data.manifest.run_id,
        workflow = data.manifest.workflow_name,
        started = data.manifest.started_at,
        completed = data.manifest.completed_at,
        bundle_hash = data.manifest.bundle_hash,
        workflow_hash = data.manifest.workflow_hash,
        policy_hash = data.manifest.policy_hash,
        audit_hash = data.manifest.audit_log_hash,
        file_count = data.manifest.file_checksums.len(),
        file_rows = data
            .manifest
            .file_checksums
            .iter()
            .map(|(f, h)| format!(
                r#"<tr><td class="mono">{f}</td><td class="mono">{h}</td></tr>"#
            ))
            .collect::<String>(),
    );
    page("Bundle", &body)
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
                entry.sequence
            )
        })
        .collect();

    let body = format!(
        r#"<h2>Audit Log ({count} entries)</h2>
<div class="card" style="padding:0;overflow:auto">
  <table>
    <tr><th>#</th><th>Step</th><th>Event</th><th>Detail</th></tr>
    {rows}
  </table>
</div>"#,
        count = data.audit_entries.len()
    );
    page("Audit Log", &body)
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
        AuditEvent::StepStarted { step_id, input_hash } => (
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
        AuditEvent::StepFailed { step_id, error } => {
            (step_id.clone(), "StepFailed", error.clone())
        }
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
        String::from("<p style='color:#888'>No outputs found.</p>")
    } else {
        data.outputs
            .iter()
            .map(|(step, json)| {
                format!(
                    r#"<details>
  <summary>{step}</summary>
  <pre>{json}</pre>
</details>"#
                )
            })
            .collect()
    };

    let body = format!(
        r#"<h2>Step Outputs ({count} steps)</h2>
<div class="card">{items}</div>"#,
        count = data.outputs.len()
    );
    page("Outputs", &body)
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
        assert!(html.contains(r#"{"ok":true}"#));
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
