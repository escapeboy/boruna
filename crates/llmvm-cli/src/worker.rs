//! Distributed-execution worker (sprint `0.5-S2b`). Polls a
//! coordinator over HTTP for claimable steps, compiles + runs
//! the step's `.ax` source, reports the result back.
//!
//! See `docs/design-coordinator-worker-http.md` and
//! `docs/architecture-coordinator-worker-http.md`.

use std::path::PathBuf;
use std::time::Duration;

use boruna_bytecode::{compute_capability_set_hash, Value};
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::vm::Vm;
use sha2::{Digest, Sha256};

use crate::coordinator::{
    CompleteRequest, ErrorBody, FailRequest, HeartbeatRequest, RegisterRequest, RegisterResponse,
    WorkItem,
};

const HEARTBEAT_INTERVAL_MS: u64 = 10_000;

/// File-path bundle for the worker's mTLS client config (sprint
/// `W6-A`). All three paths required together — partial sets are
/// a startup error so half-configured TLS doesn't silently fall
/// back to plaintext.
#[derive(Debug, Clone)]
pub struct ClientTlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub server_ca: PathBuf,
}

impl ClientTlsPaths {
    pub fn from_optional(
        cert: Option<PathBuf>,
        key: Option<PathBuf>,
        server_ca: Option<PathBuf>,
    ) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        match (cert, key, server_ca) {
            (None, None, None) => Ok(None),
            (Some(cert), Some(key), Some(server_ca)) => Ok(Some(Self {
                cert,
                key,
                server_ca,
            })),
            _ => Err("--tls-cert, --tls-key, --tls-server-ca must all be provided together".into()),
        }
    }
}

/// Read a cert PEM and key PEM and concatenate them in the format
/// `reqwest::Identity::from_pem` expects (cert blocks then key
/// block, separated by newlines). Reqwest's parser is fine with
/// multi-cert chains followed by a single key.
fn read_pem_pair(
    cert: &std::path::Path,
    key: &std::path::Path,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut cert_bytes =
        std::fs::read(cert).map_err(|e| format!("read --tls-cert {}: {e}", cert.display()))?;
    let key_bytes =
        std::fs::read(key).map_err(|e| format!("read --tls-key {}: {e}", key.display()))?;
    if !cert_bytes.ends_with(b"\n") {
        cert_bytes.push(b'\n');
    }
    cert_bytes.extend_from_slice(&key_bytes);
    Ok(cert_bytes)
}

/// Conditionally attach the `Authorization: Bearer <secret>` header
/// to a reqwest request when a shared-secret is configured. When
/// `secret` is `None`, the request is returned unchanged — the
/// pre-0.5-S3 no-auth behavior. Sprint `0.5-S3`.
fn add_bearer(req: reqwest::RequestBuilder, secret: &Option<String>) -> reqwest::RequestBuilder {
    match secret {
        Some(s) => req.bearer_auth(s),
        None => req,
    }
}

/// Sprint `W3-A` — parse a comma-separated `--advertise-caps`
/// value into the wire-shape `Vec<String>`. Empty or whitespace-only
/// input maps to `None` (full-fleet behavior, matches the absent
/// flag). Trims whitespace around each element and drops empty
/// fragments produced by trailing commas.
pub fn parse_advertise_caps(raw: Option<&str>) -> Option<Vec<String>> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    let names: Vec<String> = s
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

#[derive(Clone)]
struct WorkerHandle {
    coord_url: String,
    worker_id: String,
    session_token: String,
    client: reqwest::Client,
    lease_ttl_ms: u64,
    /// Reserved for 0.5-S2c: a future tighter coupling between
    /// the worker's claim long-poll and the coordinator's
    /// poll_timeout_ms cap. Today the worker's reqwest client
    /// timeout is poll_timeout_ms + 30 s buffer.
    #[allow(dead_code)]
    poll_timeout_ms: u64,
    /// Shared-secret bearer token (sprint `0.5-S3`). When
    /// `Some`, every HTTP request to the coordinator carries
    /// `Authorization: Bearer <secret>`. When `None`, no
    /// auth header is sent — only works when the coordinator
    /// has no secret configured (legacy/loopback deployments).
    shared_secret: Option<String>,
}

/// Parse a `--coordinator` value that may carry one or more
/// comma-separated coord URLs into a normalized `Vec<String>`.
/// Sprint W2: multi-URL workers fail over at registration time.
///
/// Whitespace around commas is trimmed; empty entries are
/// dropped; trailing slashes are removed for stable comparison
/// in error messages. Returns `Err` if the value parses to zero
/// usable URLs (e.g. `--coordinator " , "` or empty string).
pub fn parse_coordinator_urls(raw: &str) -> Result<Vec<String>, String> {
    let urls: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if urls.is_empty() {
        return Err(format!(
            "--coordinator must be one or more comma-separated URLs, got '{raw}'"
        ));
    }
    Ok(urls)
}

#[tokio::main]
pub async fn run_worker(
    coordinator: String,
    worker_id: Option<String>,
    lease_ttl_ms: u64,
    poll_timeout_ms: u64,
    shared_secret: Option<String>,
    advertised_capabilities: Option<Vec<String>>,
    tls_paths: Option<ClientTlsPaths>,
) -> Result<(), Box<dyn std::error::Error>> {
    let coord_urls = parse_coordinator_urls(&coordinator)?;
    let mut builder = reqwest::Client::builder()
        // Long-poll buffer: client timeout MUST be greater than
        // server-side poll_timeout_ms so a 30s long-poll doesn't
        // trip a 10s default client timeout.
        .timeout(Duration::from_millis(poll_timeout_ms + 30_000));
    if let Some(tls) = &tls_paths {
        let identity_pem = read_pem_pair(&tls.cert, &tls.key)?;
        let identity = reqwest::Identity::from_pem(&identity_pem)
            .map_err(|e| format!("client cert+key parse: {e}"))?;
        let ca_pem = std::fs::read(&tls.server_ca)
            .map_err(|e| format!("read --tls-server-ca {}: {e}", tls.server_ca.display()))?;
        let ca =
            reqwest::Certificate::from_pem(&ca_pem).map_err(|e| format!("server CA parse: {e}"))?;
        builder = builder
            .use_rustls_tls()
            .identity(identity)
            .add_root_certificate(ca);
        eprintln!(
            "worker mTLS: cert={} key={} server-ca={}",
            tls.cert.display(),
            tls.key.display(),
            tls.server_ca.display()
        );
    }
    let client = builder.build()?;

    let capability_set_hash = compute_capability_set_hash(
        boruna_bytecode::Capability::ALL
            .iter()
            .map(|c| (c.name().to_string(), c.version().to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str())),
    );

    // Sprint W2: register against the first reachable URL in the
    // operator-supplied list. After registration succeeds, we
    // "stick" to that coord for the lifetime of this worker
    // process — `session_token` is per-coord, so mid-session
    // failover would require re-register against a different
    // coord. Operators recover from a sticky-coord crash by
    // restarting the worker; on next start it picks the next
    // healthy URL. This is the standard k8s liveness-probe model.
    //
    // Sprint W3-A: every register attempt carries the same
    // `advertised_capabilities` payload so any winning coord
    // applies the same placement filter.
    let mut last_err: Option<String> = None;
    let (winning_url, reg) = {
        let mut found: Option<(String, RegisterResponse)> = None;
        for candidate in &coord_urls {
            let register_url = format!("{}/api/workers/register", candidate);
            let req = client.post(&register_url).json(&RegisterRequest {
                worker_id: worker_id.clone(),
                capability_set_hash: capability_set_hash.clone(),
                advertised_capabilities: advertised_capabilities.clone(),
            });
            match add_bearer(req, &shared_secret).send().await {
                Ok(reg_resp) => {
                    let status = reg_resp.status();
                    if !status.is_success() {
                        let body: ErrorBody = reg_resp.json().await.unwrap_or(ErrorBody {
                            protocol_version: 1,
                            error_kind: "coord.invalid_request".into(),
                            message: "registration failed; could not parse error body".into(),
                            current_claim_id: None,
                            current_status: None,
                            expected_hash: None,
                            max_bytes: None,
                        });
                        last_err = Some(format!(
                            "register against {candidate} returned {status}: {} ({})",
                            body.error_kind, body.message
                        ));
                        if coord_urls.len() > 1 {
                            eprintln!(
                                "worker register: {candidate} returned {status}; trying next"
                            );
                        }
                        continue;
                    }
                    let reg: RegisterResponse = reg_resp.json().await?;
                    found = Some((candidate.clone(), reg));
                    break;
                }
                Err(e) => {
                    last_err = Some(format!("connect to {candidate} failed: {e}"));
                    if coord_urls.len() > 1 {
                        eprintln!(
                            "worker register: connect to {candidate} failed ({e}); trying next"
                        );
                    }
                    continue;
                }
            }
        }
        match found {
            Some(t) => t,
            None => {
                let detail = last_err.unwrap_or_else(|| "no candidates tried".into());
                return Err(format!(
                    "worker could not register against any of {} coord URL(s): {detail}",
                    coord_urls.len()
                )
                .into());
            }
        }
    };
    eprintln!(
        "worker {} registered with coordinator {} (selected from {} candidate(s))",
        reg.worker_id,
        winning_url,
        coord_urls.len()
    );

    let handle = WorkerHandle {
        coord_url: winning_url,
        worker_id: reg.worker_id,
        session_token: reg.session_token,
        client,
        lease_ttl_ms,
        poll_timeout_ms,
        shared_secret,
    };

    // Spawn heartbeat task.
    let hb = handle.clone();
    let hb_task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
        // First tick fires immediately; skip it.
        tick.tick().await;
        loop {
            tick.tick().await;
            let req = hb
                .client
                .post(format!("{}/api/workers/heartbeat", hb.coord_url))
                .json(&HeartbeatRequest {
                    worker_id: hb.worker_id.clone(),
                    session_token: hb.session_token.clone(),
                });
            let _ = add_bearer(req, &hb.shared_secret).send().await;
        }
    });

    let result = main_loop(handle).await;
    hb_task.abort();
    result
}

async fn main_loop(handle: WorkerHandle) -> Result<(), Box<dyn std::error::Error>> {
    // Floor on the empty-claim retry interval. The coordinator
    // does its own server-side long-poll; if a misconfigured or
    // proxied coordinator returns 204 instantly, this floor
    // prevents the worker from CPU-spinning at full rate.
    // Adversarial review caught the busy-spin (F2) — without
    // this sleep, an instant 204 produces ~thousands of HTTP
    // requests per second indefinitely.
    let empty_backoff = Duration::from_millis(100);
    loop {
        match claim_one(&handle).await {
            Ok(None) => {
                tokio::time::sleep(empty_backoff).await;
            }
            Ok(Some(work)) => {
                let result = execute_step(&work);
                match result {
                    Ok((output_json, output_hash)) => {
                        report_complete(&handle, &work, output_json, output_hash).await?;
                    }
                    Err(error_msg) => {
                        report_fail(&handle, &work, error_msg).await?;
                    }
                }
            }
            Err(e) => {
                eprintln!("worker {} claim error: {e}", handle.worker_id);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn claim_one(handle: &WorkerHandle) -> Result<Option<WorkItem>, Box<dyn std::error::Error>> {
    let url = format!(
        "{}/api/work/claim?worker_id={}&session_token={}&lease_ttl_ms={}",
        handle.coord_url,
        urlencoding_simple(&handle.worker_id),
        urlencoding_simple(&handle.session_token),
        handle.lease_ttl_ms
    );
    let resp = add_bearer(handle.client.get(&url), &handle.shared_secret)
        .send()
        .await?;
    let status = resp.status();
    if status.as_u16() == 204 {
        return Ok(None);
    }
    if !status.is_success() {
        let body: ErrorBody = resp.json().await.unwrap_or(ErrorBody {
            protocol_version: 1,
            error_kind: "coord.invalid_request".into(),
            message: format!("claim failed with {status}"),
            current_claim_id: None,
            current_status: None,
            expected_hash: None,
            max_bytes: None,
        });
        return Err(format!("claim {status}: {} ({})", body.error_kind, body.message).into());
    }
    let item: WorkItem = resp.json().await?;
    Ok(Some(item))
}

/// Compile + run the step's `.ax` source under the work item's
/// policy. Returns `(output_json, output_hash)` on success or an
/// error message on failure.
///
/// Policy parsing goes through the strict validator from sprint
/// `0.4-S15` (`boruna_vm::policy_validate::parse`) so workers
/// reject the same shapes the CLI rejects, with the same stable
/// `error_kind` strings. This closes the validate-vs-execute
/// drift surface at the worker boundary.
fn execute_step(work: &WorkItem) -> Result<(String, String), String> {
    let policy: Policy = boruna_vm::policy_validate::parse(&work.policy_json)
        .map_err(|e| format!("policy parse: {e}"))?;
    let module = boruna_compiler::compile(&work.step_id, &work.source)
        .map_err(|e| format!("compile: {e}"))?;
    let gateway = CapabilityGateway::new(policy);
    let mut vm = Vm::new(module, gateway);
    let value = vm.run().map_err(|e| format!("runtime: {e}"))?;
    let output_json = value_to_json(&value);
    let mut hasher = Sha256::new();
    hasher.update(output_json.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(7 + 64);
    hex.push_str("sha256:");
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok((output_json, hex))
}

fn value_to_json(v: &Value) -> String {
    // For the MVP: serialize via the existing `format_value`
    // shape used by the MCP `boruna_run` tool.
    match v {
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap(),
        Value::Bool(b) => b.to_string(),
        Value::Unit => "null".into(),
        Value::None => r#"{"option":"None"}"#.into(),
        _ => serde_json::to_string(&format!("{v:?}")).unwrap(),
    }
}

async fn report_complete(
    handle: &WorkerHandle,
    work: &WorkItem,
    output_json: String,
    output_hash: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/api/work/complete", handle.coord_url);
    let body = CompleteRequest {
        worker_id: handle.worker_id.clone(),
        session_token: handle.session_token.clone(),
        run_id: work.run_id.clone(),
        step_id: work.step_id.clone(),
        claim_id: work.claim_id,
        output_json,
        output_hash,
        attempt_count: 1,
    };
    let resp = add_bearer(handle.client.post(&url).json(&body), &handle.shared_secret)
        .send()
        .await?;
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    let err: ErrorBody = resp.json().await.unwrap_or(ErrorBody {
        protocol_version: 1,
        error_kind: "coord.invalid_request".into(),
        message: format!("complete failed with {status}"),
        current_claim_id: None,
        current_status: None,
        expected_hash: None,
        max_bytes: None,
    });
    eprintln!(
        "worker {} complete returned {}: {} ({})",
        handle.worker_id, status, err.error_kind, err.message
    );
    // Adversarial-review F1: do NOT silently swallow non-success
    // responses. Distinguish three cases:
    //
    // 1. `coord.lease_expired` (409) — per ADR 002 the
    //    coordinator has already re-dispatched the step to
    //    another worker. Discard our work; do NOT report_fail
    //    (that would race with the new claim). Just log + move
    //    on.
    //
    // 2. `coord.output_too_large` (413) and other
    //    output-validation errors — the work is genuinely done
    //    but the coordinator can't accept the output. Re-running
    //    the same source produces the same output, so retry is
    //    pointless. Report as a step failure so the run can
    //    progress (or terminal-fail per retry policy).
    //
    // 3. Anything else (5xx, network error after retry, unknown
    //    error_kind) — best-effort: report_fail so the step
    //    doesn't strand. Caller's retry policy decides whether
    //    to re-attempt.
    match err.error_kind.as_str() {
        "coord.lease_expired" => Ok(()),
        _ => {
            // Map the failure into a step-fail report so the row
            // doesn't sit in Running until lease expiry.
            let fail_msg = format!(
                "report_complete rejected by coordinator: {} ({})",
                err.error_kind, err.message
            );
            report_fail(handle, work, fail_msg).await
        }
    }
}

async fn report_fail(
    handle: &WorkerHandle,
    work: &WorkItem,
    error_msg: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/api/work/fail", handle.coord_url);
    let body = FailRequest {
        worker_id: handle.worker_id.clone(),
        session_token: handle.session_token.clone(),
        run_id: work.run_id.clone(),
        step_id: work.step_id.clone(),
        claim_id: work.claim_id,
        error_msg,
        attempt_count: 1,
    };
    let resp = add_bearer(handle.client.post(&url).json(&body), &handle.shared_secret)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        eprintln!("worker {} fail returned {}", handle.worker_id, status);
    }
    Ok(())
}

/// Minimal URL-encoding for the small alphabet our worker_ids
/// and session_tokens use (alphanumerics, hyphens, underscores).
/// Avoids pulling a urlencode dep.
fn urlencoding_simple(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_pure_function_returns_int_output() {
        let work = WorkItem {
            protocol_version: 1,
            run_id: "r".into(),
            step_id: "s".into(),
            claim_id: 1,
            lease_expires_at_ms: 0,
            source: "fn main() -> Int { 42 }\n".into(),
            policy_json: r#"{"default_allow":true}"#.into(),
            inputs_json: None,
        };
        let (output_json, output_hash) = execute_step(&work).unwrap();
        assert_eq!(output_json, "42");
        assert!(output_hash.starts_with("sha256:"));
    }

    #[test]
    fn execute_returns_deterministic_hash_for_same_input() {
        let work = WorkItem {
            protocol_version: 1,
            run_id: "r".into(),
            step_id: "s".into(),
            claim_id: 1,
            lease_expires_at_ms: 0,
            source: "fn main() -> Int { 1 + 2 }\n".into(),
            policy_json: r#"{"default_allow":true}"#.into(),
            inputs_json: None,
        };
        let (out1, hash1) = execute_step(&work).unwrap();
        let (out2, hash2) = execute_step(&work).unwrap();
        assert_eq!(out1, out2);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn execute_compile_error_returns_err() {
        let work = WorkItem {
            protocol_version: 1,
            run_id: "r".into(),
            step_id: "s".into(),
            claim_id: 1,
            lease_expires_at_ms: 0,
            source: "@@@ not valid".into(),
            policy_json: r#"{"default_allow":true}"#.into(),
            inputs_json: None,
        };
        let err = execute_step(&work).unwrap_err();
        assert!(err.contains("compile"));
    }

    #[test]
    fn urlencoding_passes_through_safe_chars() {
        assert_eq!(urlencoding_simple("wkr-abc123"), "wkr-abc123");
        assert_eq!(urlencoding_simple("hello world"), "hello%20world");
    }

    // Sprint W2 — coordinator URL parsing for HA failover.

    #[test]
    fn parse_single_coord_url_keeps_one_entry() {
        let urls = parse_coordinator_urls("http://coord:8090").unwrap();
        assert_eq!(urls, vec!["http://coord:8090".to_string()]);
    }

    #[test]
    fn parse_strips_trailing_slash_for_stable_logging() {
        let urls = parse_coordinator_urls("http://coord:8090/").unwrap();
        assert_eq!(urls, vec!["http://coord:8090".to_string()]);
    }

    #[test]
    fn parse_multiple_coord_urls_preserves_order() {
        let urls =
            parse_coordinator_urls("http://coord-1:8090,http://coord-2:8090,http://coord-3:8090")
                .unwrap();
        assert_eq!(
            urls,
            vec![
                "http://coord-1:8090".to_string(),
                "http://coord-2:8090".to_string(),
                "http://coord-3:8090".to_string(),
            ]
        );
    }

    #[test]
    fn parse_tolerates_whitespace_around_commas() {
        let urls = parse_coordinator_urls("http://a:1 ,  http://b:2  , http://c:3").unwrap();
        assert_eq!(
            urls,
            vec![
                "http://a:1".to_string(),
                "http://b:2".to_string(),
                "http://c:3".to_string(),
            ]
        );
    }

    #[test]
    fn parse_drops_empty_entries() {
        let urls = parse_coordinator_urls("http://a:1,,http://b:2").unwrap();
        assert_eq!(
            urls,
            vec!["http://a:1".to_string(), "http://b:2".to_string()]
        );
    }

    #[test]
    fn parse_empty_string_is_an_error() {
        assert!(parse_coordinator_urls("").is_err());
        assert!(parse_coordinator_urls(" , ").is_err());
        assert!(parse_coordinator_urls(",,,").is_err());
    }

    // Sprint W3-A — advertised capabilities parsing.

    #[test]
    fn parse_advertise_caps_absent_or_empty_returns_none() {
        assert_eq!(parse_advertise_caps(None), None);
        assert_eq!(parse_advertise_caps(Some("")), None);
        assert_eq!(parse_advertise_caps(Some("   ")), None);
        assert_eq!(parse_advertise_caps(Some(",,, ,")), None);
    }

    #[test]
    fn parse_advertise_caps_splits_and_trims() {
        assert_eq!(
            parse_advertise_caps(Some("net.fetch, db.query ,fs.read")),
            Some(vec![
                "net.fetch".into(),
                "db.query".into(),
                "fs.read".into()
            ])
        );
    }
}
