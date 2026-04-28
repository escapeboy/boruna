//! End-to-end CLI integration tests for `boruna coordinator
//! serve` + `boruna worker run` (sprint 0.5-S2b). Spawns the
//! binary in both modes and asserts the protocol works
//! end-to-end.
//!
//! Only compiled when `--features serve` is enabled.

#![cfg(feature = "serve")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use boruna_orchestrator::persistence::{
    RunCheckpointStore, RunRow, RunStatus, StepCheckpoint, StepStatus,
};

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

/// Connect to a freshly-spawned server with a small retry budget.
/// Sprint W10 — pre-existing flaky surface flagged by W9-D's local
/// runs: even after `wait_for_server` returns, a busy CI runner can
/// race the listener-backlog state and surface ConnectionRefused on
/// the first real request. Retrying fixes the race without changing
/// what the test verifies (HTTP responses, not connect timing).
/// Mirrors the W8 fix in `cli_dashboard.rs::http_request`.
fn connect_with_retries(port: u16) -> TcpStream {
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..5 {
        match TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(500),
        ) {
            Ok(s) => return s,
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(50 * (attempt + 1)));
            }
        }
    }
    panic!(
        "connect to 127.0.0.1:{port} failed after 5 retries; last err: {}",
        last_err
            .as_ref()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
}

fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn wait_for_server(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            // Brief wait for the server's HTTP layer to be ready
            // after TCP accept.
            std::thread::sleep(Duration::from_millis(100));
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server on port {port} never came up within 10s");
}

fn http_request(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = connect_with_retries(port);
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write");
    let mut reader = BufReader::new(&stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).expect("read status");
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    let code: u16 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap_or(0);
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut body = String::new();
    let _ = reader.read_to_string(&mut body);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    (code, body)
}

fn populate_pending_step(data_dir: &Path, run_id: &str, step_id: &str, source: &str) {
    std::fs::create_dir_all(data_dir).unwrap();
    let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();
    let metadata_json = serde_json::json!({
        "step_sources": { step_id: source }
    })
    .to_string();
    store
        .insert_run(&RunRow {
            run_id: run_id.into(),
            workflow_name: "wf".into(),
            workflow_hash: "h".into(),
            status: RunStatus::Running,
            started_at_ms: 0,
            updated_at_ms: 0,
            policy_json: r#"{"default_allow":true}"#.into(),
            metadata_json,
        })
        .unwrap();
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

fn spawn_coordinator(data_dir: &Path, max_lease_ttl_ms: u64, poll_timeout_ms: u64) -> (Child, u16) {
    spawn_coordinator_with_sweep(data_dir, max_lease_ttl_ms, poll_timeout_ms, 30_000)
}

fn spawn_coordinator_with_sweep(
    data_dir: &Path,
    max_lease_ttl_ms: u64,
    poll_timeout_ms: u64,
    sweep_interval_ms: u64,
) -> (Child, u16) {
    let port = pick_free_port();
    let child = Command::new(boruna_bin())
        .args([
            "coordinator",
            "serve",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--port",
            &port.to_string(),
            "--max-lease-ttl-ms",
            &max_lease_ttl_ms.to_string(),
            "--poll-timeout-ms",
            &poll_timeout_ms.to_string(),
            "--sweep-interval-ms",
            &sweep_interval_ms.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn coordinator");
    wait_for_server(port);
    (child, port)
}

fn spawn_worker(coord_url: &str, worker_id: &str, lease_ttl_ms: u64) -> Child {
    Command::new(boruna_bin())
        .args([
            "worker",
            "run",
            "--coordinator",
            coord_url,
            "--worker-id",
            worker_id,
            "--lease-ttl-ms",
            &lease_ttl_ms.to_string(),
            "--poll-timeout-ms",
            "1000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn worker")
}

fn kill_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Spawn a coordinator with an auth shared-secret. Sprint 0.5-S3.
fn spawn_coordinator_with_secret(data_dir: &Path, secret: &str) -> (Child, u16) {
    let port = pick_free_port();
    let child = Command::new(boruna_bin())
        .args([
            "coordinator",
            "serve",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--port",
            &port.to_string(),
            "--max-lease-ttl-ms",
            "60000",
            "--poll-timeout-ms",
            "200",
            "--shared-secret",
            secret,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn coordinator with secret");
    wait_for_server(port);
    (child, port)
}

fn http_request_with_auth(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    bearer: Option<&str>,
) -> (u16, String) {
    let mut stream = connect_with_retries(port);
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = body.unwrap_or("");
    let auth_header = match bearer {
        Some(b) => format!("Authorization: Bearer {b}\r\n"),
        None => String::new(),
    };
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write");
    let mut reader = BufReader::new(&stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).expect("read status");
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    let code: u16 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap_or(0);
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut body = String::new();
    let _ = reader.read_to_string(&mut body);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    (code, body)
}

#[test]
fn coord_register_returns_worker_id_and_session_token() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-init", "noop", "fn main() -> Int { 0 }\n");
    let (child, port) = spawn_coordinator(dir.path(), 60_000, 200);
    let cap_hash = boruna_bytecode::compute_capability_set_hash(
        boruna_bytecode::Capability::ALL
            .iter()
            .map(|c| (c.name().to_string(), c.version().to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str())),
    );
    let body = serde_json::json!({
        "capability_set_hash": cap_hash,
    })
    .to_string();
    let (code, resp) = http_request(port, "POST", "/api/workers/register", Some(&body));
    kill_child(child);
    assert_eq!(code, 200, "resp: {resp}");
    let v: serde_json::Value = serde_json::from_str(&resp).expect("json");
    assert_eq!(v["protocol_version"], 1);
    assert!(v["worker_id"].as_str().unwrap().starts_with("wkr-"));
    assert!(v["session_token"].as_str().unwrap().starts_with("sess-"));
}

#[test]
fn coord_register_rejects_binary_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-init", "noop", "fn main() -> Int { 0 }\n");
    let (child, port) = spawn_coordinator(dir.path(), 60_000, 200);
    let body = serde_json::json!({
        "capability_set_hash": "sha256:bogus",
    })
    .to_string();
    let (code, resp) = http_request(port, "POST", "/api/workers/register", Some(&body));
    kill_child(child);
    assert_eq!(code, 409);
    let v: serde_json::Value = serde_json::from_str(&resp).expect("json");
    assert_eq!(v["error_kind"], "coord.binary_mismatch");
    assert!(v["expected_hash"].is_string());
}

#[test]
fn coord_missing_data_dir_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let bogus = dir.path().join("does-not-exist");
    let port = pick_free_port();
    let out = Command::new(boruna_bin())
        .args([
            "coordinator",
            "serve",
            "--data-dir",
            bogus.to_str().unwrap(),
            "--port",
            &port.to_string(),
        ])
        .output()
        .expect("invoke");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("runs.db"), "stderr: {stderr}");
}

#[test]
fn coord_oversize_body_returns_413() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-init", "noop", "fn main() -> Int { 0 }\n");
    let (child, port) = spawn_coordinator(dir.path(), 60_000, 200);
    // Build a body just over 8 MiB.
    let oversize = "x".repeat(8 * 1024 * 1024 + 100);
    let body = serde_json::json!({
        "worker_id": "ghost",
        "session_token": "x",
        "run_id": "r",
        "step_id": "s",
        "claim_id": 1,
        "output_json": oversize,
        "output_hash": "h",
        "attempt_count": 1,
    })
    .to_string();
    let (code, _resp) = http_request(port, "POST", "/api/work/complete", Some(&body));
    kill_child(child);
    // Axum's DefaultBodyLimit returns 413 for oversized bodies.
    assert_eq!(code, 413, "expected 413, got {code}");
}

#[test]
fn worker_subprocess_completes_step_end_to_end() {
    // The MVP smoke test: spawn coordinator + worker, pre-populate
    // a single Pending step, wait for the worker to claim+execute+
    // complete, assert the row's final state.
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(
        dir.path(),
        "run-smoke",
        "compute",
        "fn main() -> Int { 1 + 2 + 3 + 4 }\n",
    );
    let (coord_child, port) = spawn_coordinator(dir.path(), 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker_child = spawn_worker(&coord_url, "smoke-worker", 30_000);

    // Poll runs.db until the step transitions to Completed (or
    // timeout).
    let db_path = dir.path().join("runs.db");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen_completed = false;
    let mut last_status = String::new();
    while Instant::now() < deadline {
        let store = RunCheckpointStore::open(&db_path).unwrap();
        let cps = store.list_step_checkpoints("run-smoke").unwrap();
        if let Some(cp) = cps.first() {
            last_status = cp.status.as_str().into();
            if cp.status == StepStatus::Completed {
                seen_completed = true;
                assert_eq!(cp.output_json.as_deref(), Some("10"));
                assert!(cp.output_hash.as_deref().unwrap().starts_with("sha256:"));
                assert!(cp.worker_id.is_none());
                assert!(cp.lease_expires_at_ms.is_none());
                assert_eq!(cp.claim_id, 1);
                break;
            }
        }
        drop(store);
        std::thread::sleep(Duration::from_millis(100));
    }
    kill_child(worker_child);
    kill_child(coord_child);
    assert!(
        seen_completed,
        "step never reached Completed; last status: {last_status}"
    );
}

#[test]
fn worker_kill_mid_step_lease_expires_then_reclaim() {
    // Flagship regression: prove that a step claimed by some
    // worker that never completes (lease expires) is reclaimed
    // by a fresh worker via the HTTP path. Asserts the final
    // row's `claim_id == 2`, proving the CAS state machine
    // worked end-to-end through the coordinator's claim
    // endpoint.
    //
    // Adversarial-review fix (F5): the prior version spawned a
    // real worker A subprocess, which was racy under fast CI
    // (worker A could complete before the lease expired,
    // turning the test into a no-op). The deterministic
    // version simulates worker A's claim by inserting a
    // Running checkpoint with an already-expired
    // `lease_expires_at` directly into runs.db. The
    // coordinator's startup sweep + the explicit
    // `expire_leases_and_requeue` call moves the row back to
    // Pending; only then does worker B come up via the HTTP
    // path and reclaim.
    use boruna_orchestrator::persistence::StepCheckpoint;
    let dir = tempfile::tempdir().unwrap();
    let metadata_json = serde_json::json!({
        "step_sources": { "step1": "fn main() -> Int { 99 }\n" }
    })
    .to_string();
    std::fs::create_dir_all(dir.path()).unwrap();
    let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
    store
        .insert_run(&RunRow {
            run_id: "run-race".into(),
            workflow_name: "wf".into(),
            workflow_hash: "h".into(),
            status: RunStatus::Running,
            started_at_ms: 0,
            updated_at_ms: 0,
            policy_json: r#"{"default_allow":true}"#.into(),
            metadata_json,
        })
        .unwrap();
    // First insert as Pending so claim_step can transition it.
    store
        .upsert_step_checkpoint(&StepCheckpoint {
            run_id: "run-race".into(),
            step_id: "step1".into(),
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
    // Simulate "worker A claimed and was killed": call the
    // public claim_step API with a `lease_expires_at` that's
    // already in the past relative to wall-clock-now. The
    // coordinator's startup sweep will then expire this lease.
    store
        .claim_step("run-race", "step1", "worker-A", 1, 0)
        .unwrap();
    drop(store);

    // Spawn the coordinator — its startup sweep should expire
    // the stale lease and re-enqueue the step as Pending.
    let (coord_child, port) = spawn_coordinator(dir.path(), 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let db_path = dir.path().join("runs.db");

    // Verify the coordinator's startup sweep ran.
    let store = RunCheckpointStore::open(&db_path).unwrap();
    let cp = store
        .list_step_checkpoints("run-race")
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        cp.status,
        StepStatus::Pending,
        "coordinator startup sweep should have requeued the stale-lease row"
    );
    assert_eq!(cp.worker_id, None);
    assert_eq!(cp.claim_id, 1, "claim_id preserved across requeue");
    drop(store);

    // Spawn worker B; it should claim (claim_id=2) and complete.
    let worker_b = spawn_worker(&coord_url, "worker-B", 60_000);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut completed = false;
    while Instant::now() < deadline {
        let store = RunCheckpointStore::open(&db_path).unwrap();
        let cps = store.list_step_checkpoints("run-race").unwrap();
        if let Some(cp) = cps.first() {
            if cp.status == StepStatus::Completed {
                completed = true;
                assert_eq!(cp.claim_id, 2, "expected claim_id=2 after reclaim");
                assert_eq!(cp.output_json.as_deref(), Some("99"));
                break;
            }
        }
        drop(store);
        std::thread::sleep(Duration::from_millis(100));
    }
    kill_child(worker_b);
    kill_child(coord_child);
    assert!(completed, "worker B never reclaimed and completed");
}

#[test]
fn cli_workflow_run_submit_only_then_worker_completes() {
    // Sprint 0.5-S2e: full end-to-end via the marquee CLI
    // path. Spawn coordinator + worker. Use
    // `boruna workflow run --submit-only` against a 1-step
    // workflow on disk (real workflow.json + .ax file).
    // Assert:
    //   1. `workflow run --submit-only` exits 0.
    //   2. The step transitions Pending → Running →
    //      Completed via the worker.
    //   3. The output_json matches the expected value (proof
    //      that the `.ax` source flowed through metadata,
    //      coordinator, worker, and back).
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let wf_dir = dir.path().join("wf");
    std::fs::create_dir_all(&wf_dir).unwrap();
    std::fs::write(wf_dir.join("step1.ax"), "fn main() -> Int { 7 }\n").unwrap();
    std::fs::write(
        wf_dir.join("workflow.json"),
        r#"{
            "schema_version": 1,
            "name": "submit-test",
            "version": "1.0.0",
            "steps": {
                "step1": {
                    "kind": "source",
                    "source": "step1.ax",
                    "capabilities": [],
                    "outputs": {"result": "Int"}
                }
            },
            "edges": []
        }"#,
    )
    .unwrap();

    // Pre-create the data dir so the coordinator can open
    // runs.db. We bootstrap by running submit-only first
    // (which creates runs.db), then start the coordinator.
    std::fs::create_dir_all(&data_dir).unwrap();

    // Submit the workflow (creates runs.db + Pending step1).
    let submit_out = Command::new(boruna_bin())
        .args([
            "workflow",
            "run",
            wf_dir.to_str().unwrap(),
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--submit-only",
        ])
        .output()
        .expect("invoke boruna workflow run --submit-only");
    assert!(
        submit_out.status.success(),
        "submit failed: stderr={}",
        String::from_utf8_lossy(&submit_out.stderr)
    );

    // Find the run_id by scanning runs.db.
    let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();
    let runs = store.list_runs().unwrap();
    assert_eq!(runs.len(), 1, "expected exactly one run after submit");
    let run_id = runs[0].run_id.clone();
    drop(store);

    // Spawn coordinator + worker and wait for the step to complete.
    let (coord_child, port) = spawn_coordinator(&data_dir, 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "submit-worker", 30_000);

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut completed = false;
    let mut last_state = String::new();
    while Instant::now() < deadline {
        let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();
        let cps = store.list_step_checkpoints(&run_id).unwrap();
        last_state = format!(
            "{:?}",
            cps.iter()
                .map(|c| (c.step_id.as_str(), c.status))
                .collect::<Vec<_>>()
        );
        if let Some(cp) = cps.first() {
            if cp.status == StepStatus::Completed {
                completed = true;
                assert_eq!(cp.output_json.as_deref(), Some("7"));
                assert_eq!(cp.claim_id, 1);
                break;
            }
        }
        drop(store);
        std::thread::sleep(Duration::from_millis(100));
    }
    kill_child(worker);
    kill_child(coord_child);
    assert!(
        completed,
        "step never reached Completed; last state: {last_state}"
    );
}

// ── coordinator wait (sprint 0.5-S2f) ──

/// Build a 3-step fan-in workflow on disk for the wait-driver tests.
/// `s1` and `s2` are wave-1 source steps; `s3` depends on both.
/// `s3.ax` body controls success/failure for the failed-run test.
fn make_fan_in_workflow_on_disk(wf_dir: &Path, s3_body: &str) {
    std::fs::create_dir_all(wf_dir).unwrap();
    std::fs::write(wf_dir.join("s1.ax"), "fn main() -> Int { 1 }\n").unwrap();
    std::fs::write(wf_dir.join("s2.ax"), "fn main() -> Int { 2 }\n").unwrap();
    std::fs::write(wf_dir.join("s3.ax"), s3_body).unwrap();
    std::fs::write(
        wf_dir.join("workflow.json"),
        r#"{
            "schema_version": 1,
            "name": "wait-test",
            "version": "1.0.0",
            "steps": {
                "s1": {"kind": "source", "source": "s1.ax", "capabilities": [], "outputs": {"result": "Int"}},
                "s2": {"kind": "source", "source": "s2.ax", "capabilities": [], "outputs": {"result": "Int"}},
                "s3": {"kind": "source", "source": "s3.ax", "capabilities": [], "outputs": {"result": "Int"}}
            },
            "edges": [["s1", "s3"], ["s2", "s3"]]
        }"#,
    )
    .unwrap();
}

fn submit_only(data_dir: &Path, wf_dir: &Path) -> String {
    let out = Command::new(boruna_bin())
        .args([
            "workflow",
            "run",
            wf_dir.to_str().unwrap(),
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--submit-only",
        ])
        .output()
        .expect("invoke boruna workflow run --submit-only");
    assert!(
        out.status.success(),
        "submit failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();
    let runs = store.list_runs().unwrap();
    assert_eq!(runs.len(), 1, "expected exactly one run after submit");
    runs[0].run_id.clone()
}

fn spawn_wait(data_dir: &Path, run_id: &str) -> Child {
    Command::new(boruna_bin())
        .args([
            "coordinator",
            "wait",
            run_id,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--poll-interval-ms",
            "100",
            "--max-wait-secs",
            "30",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn coordinator wait")
}

#[test]
fn cli_coordinator_wait_drives_multi_wave_to_completion() {
    // Sprint 0.5-S2f marquee test. Submit a 3-step fan-in workflow
    // (s1, s2 → s3) and prove the `coordinator wait` driver advances
    // wave-2 (s3) to Pending after wave-1 completes, the worker picks
    // it up, and the run reaches Completed with all 3 steps Completed.
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let wf_dir = dir.path().join("wf");
    std::fs::create_dir_all(&data_dir).unwrap();
    make_fan_in_workflow_on_disk(&wf_dir, "fn main() -> Int { 3 }\n");

    let run_id = submit_only(&data_dir, &wf_dir);

    let (coord_child, port) = spawn_coordinator(&data_dir, 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "wait-worker", 30_000);

    // Run wait synchronously and wait for it to exit.
    let wait_out = Command::new(boruna_bin())
        .args([
            "coordinator",
            "wait",
            &run_id,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--poll-interval-ms",
            "100",
            "--max-wait-secs",
            "30",
        ])
        .output()
        .expect("invoke coordinator wait");

    kill_child(worker);
    kill_child(coord_child);

    assert!(
        wait_out.status.success(),
        "wait exited non-zero; status={:?}\nstdout={}\nstderr={}",
        wait_out.status.code(),
        String::from_utf8_lossy(&wait_out.stdout),
        String::from_utf8_lossy(&wait_out.stderr),
    );

    // All 3 steps Completed; correct outputs.
    let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();
    let cps = store.list_step_checkpoints(&run_id).unwrap();
    assert_eq!(cps.len(), 3, "got {} checkpoints", cps.len());
    for cp in &cps {
        assert_eq!(
            cp.status,
            StepStatus::Completed,
            "step {} not Completed: {:?}",
            cp.step_id,
            cp.status
        );
    }
    let outputs: std::collections::BTreeMap<&str, &str> = cps
        .iter()
        .map(|c| (c.step_id.as_str(), c.output_json.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(outputs["s1"], "1");
    assert_eq!(outputs["s2"], "2");
    assert_eq!(outputs["s3"], "3");

    // Wait stdout should contain transitions per step.
    let stdout = String::from_utf8_lossy(&wait_out.stdout);
    assert!(stdout.contains("step s1"), "stdout missing s1: {stdout}");
    assert!(stdout.contains("step s3"), "stdout missing s3: {stdout}");
    assert!(
        stdout.contains("completed"),
        "stdout missing completed: {stdout}"
    );
}

#[test]
fn cli_coordinator_wait_resumes_after_kill() {
    // Kill the wait process between waves; re-invoke; the run still
    // completes. Proves the wait driver is stateless on the client
    // side — all state lives in runs.db.
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let wf_dir = dir.path().join("wf");
    std::fs::create_dir_all(&data_dir).unwrap();
    make_fan_in_workflow_on_disk(&wf_dir, "fn main() -> Int { 3 }\n");

    let run_id = submit_only(&data_dir, &wf_dir);

    let (coord_child, port) = spawn_coordinator(&data_dir, 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "resume-worker", 30_000);

    // First wait: kill it after a short delay (likely between
    // waves but state survives regardless).
    let wait1 = spawn_wait(&data_dir, &run_id);
    std::thread::sleep(Duration::from_millis(800));
    kill_child(wait1);

    // Second wait: drive to terminal.
    let wait_out = Command::new(boruna_bin())
        .args([
            "coordinator",
            "wait",
            &run_id,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--poll-interval-ms",
            "100",
            "--max-wait-secs",
            "30",
        ])
        .output()
        .expect("invoke wait #2");

    kill_child(worker);
    kill_child(coord_child);

    assert!(
        wait_out.status.success(),
        "wait #2 exited non-zero; status={:?}\nstdout={}\nstderr={}",
        wait_out.status.code(),
        String::from_utf8_lossy(&wait_out.stdout),
        String::from_utf8_lossy(&wait_out.stderr),
    );
    let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();
    let cps = store.list_step_checkpoints(&run_id).unwrap();
    assert_eq!(cps.len(), 3);
    for cp in &cps {
        assert_eq!(cp.status, StepStatus::Completed);
    }
}

#[test]
fn cli_coordinator_wait_two_concurrent_waits_converge() {
    // CORR-6 from 0.5-S2f: two `coordinator wait` processes against
    // the same run_id must each converge to exit 0 (Completed). The
    // race-safe persistence primitive (`insert_pending_step_if_absent`,
    // ON CONFLICT DO NOTHING) guarantees only one wait wins each
    // Pending insert; both observe the same terminal state.
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let wf_dir = dir.path().join("wf");
    std::fs::create_dir_all(&data_dir).unwrap();
    make_fan_in_workflow_on_disk(&wf_dir, "fn main() -> Int { 3 }\n");

    let run_id = submit_only(&data_dir, &wf_dir);
    let (coord_child, port) = spawn_coordinator(&data_dir, 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "concurrent-waits-worker", 30_000);

    // Spawn two `coordinator wait` children racing on the same run_id.
    let wait1 = spawn_wait(&data_dir, &run_id);
    let wait2 = spawn_wait(&data_dir, &run_id);

    // Each wait runs in its own process; collect their exit codes
    // by spawning a third invocation that we wait on synchronously
    // (it will see the same terminal state). Then ensure both
    // background waits also converge.
    let synchronous_wait = Command::new(boruna_bin())
        .args([
            "coordinator",
            "wait",
            &run_id,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--poll-interval-ms",
            "100",
            "--max-wait-secs",
            "30",
        ])
        .output()
        .expect("invoke synchronous wait");

    // Background waits should also exit 0 — they race against the
    // synchronous one but all see the same terminal state. Use
    // try_wait with a short retry to confirm without leaking.
    let _ = wait1.id();
    let _ = wait2.id();
    let mut wait1_status = None;
    let mut wait2_status = None;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut wait1 = wait1;
    let mut wait2 = wait2;
    while Instant::now() < deadline {
        if wait1_status.is_none() {
            if let Ok(Some(s)) = wait1.try_wait() {
                wait1_status = Some(s);
            }
        }
        if wait2_status.is_none() {
            if let Ok(Some(s)) = wait2.try_wait() {
                wait2_status = Some(s);
            }
        }
        if wait1_status.is_some() && wait2_status.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Kill any wait still running (may have lost the race to detect
    // terminal). Then the synchronous wait above is the one we
    // assert against — its exit code is the contract.
    if wait1_status.is_none() {
        kill_child(wait1);
    }
    if wait2_status.is_none() {
        kill_child(wait2);
    }
    kill_child(worker);
    kill_child(coord_child);

    assert!(
        synchronous_wait.status.success(),
        "synchronous wait exited non-zero: status={:?}\nstdout={}\nstderr={}",
        synchronous_wait.status.code(),
        String::from_utf8_lossy(&synchronous_wait.stdout),
        String::from_utf8_lossy(&synchronous_wait.stderr),
    );

    // Verify the run reached Completed; both background waits, if
    // they exited, exited 0.
    let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();
    let cps = store.list_step_checkpoints(&run_id).unwrap();
    assert_eq!(cps.len(), 3);
    for cp in &cps {
        assert_eq!(cp.status, StepStatus::Completed);
    }
    if let Some(s) = wait1_status {
        assert_eq!(s.code(), Some(0), "wait1 exited non-zero");
    }
    if let Some(s) = wait2_status {
        assert_eq!(s.code(), Some(0), "wait2 exited non-zero");
    }
}

#[test]
fn cli_coordinator_wait_exits_zero_immediately_for_already_completed_run() {
    // Drive a run to Completed via the marquee path, then re-invoke
    // wait against the same run_id. The second wait should exit 0
    // immediately on the first tick (no polling loop).
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let wf_dir = dir.path().join("wf");
    std::fs::create_dir_all(&data_dir).unwrap();
    make_fan_in_workflow_on_disk(&wf_dir, "fn main() -> Int { 3 }\n");

    let run_id = submit_only(&data_dir, &wf_dir);
    let (coord_child, port) = spawn_coordinator(&data_dir, 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "completed-worker", 30_000);

    // First wait: drive to Completed.
    let _ = Command::new(boruna_bin())
        .args([
            "coordinator",
            "wait",
            &run_id,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--poll-interval-ms",
            "100",
            "--max-wait-secs",
            "30",
        ])
        .output()
        .expect("invoke wait #1");

    // Second wait: should exit 0 immediately. Use a short max-wait
    // budget — if the loop spins without detecting Completed on the
    // first tick, this would time out (exit 3) instead.
    let started = Instant::now();
    let wait_out = Command::new(boruna_bin())
        .args([
            "coordinator",
            "wait",
            &run_id,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--poll-interval-ms",
            "5000", // long poll; if we hit it, the test fails
            "--max-wait-secs",
            "10",
        ])
        .output()
        .expect("invoke wait #2");
    let elapsed = started.elapsed();

    kill_child(worker);
    kill_child(coord_child);

    assert_eq!(
        wait_out.status.code(),
        Some(0),
        "expected exit 0; got {:?}\nstdout={}\nstderr={}",
        wait_out.status.code(),
        String::from_utf8_lossy(&wait_out.stdout),
        String::from_utf8_lossy(&wait_out.stderr),
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "wait took {elapsed:?}; expected immediate exit on first tick"
    );
}

#[test]
fn cli_coordinator_wait_exits_nonzero_on_failed_run() {
    // s3 has a deliberately broken .ax (missing main); worker fails it.
    // Wait should exit non-zero (1 = run Failed).
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let wf_dir = dir.path().join("wf");
    std::fs::create_dir_all(&data_dir).unwrap();
    // Compile-error body — the worker should fail this step.
    make_fan_in_workflow_on_disk(&wf_dir, "this is not valid ax syntax\n");

    let run_id = submit_only(&data_dir, &wf_dir);

    let (coord_child, port) = spawn_coordinator(&data_dir, 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "fail-worker", 30_000);

    let wait_out = Command::new(boruna_bin())
        .args([
            "coordinator",
            "wait",
            &run_id,
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--poll-interval-ms",
            "100",
            "--max-wait-secs",
            "30",
        ])
        .output()
        .expect("invoke wait");

    kill_child(worker);
    kill_child(coord_child);

    assert_eq!(
        wait_out.status.code(),
        Some(1),
        "expected exit code 1 (run Failed); got {:?}\nstdout={}\nstderr={}",
        wait_out.status.code(),
        String::from_utf8_lossy(&wait_out.stdout),
        String::from_utf8_lossy(&wait_out.stderr),
    );
}

#[test]
fn coord_bg_sweep_requeues_expired_lease() {
    // Sprint 0.5-S2c: prove the background sweep fires
    // periodically and requeues stale leases without
    // requiring a coordinator restart.
    use boruna_orchestrator::persistence::StepCheckpoint;
    let dir = tempfile::tempdir().unwrap();
    let metadata_json = serde_json::json!({
        "step_sources": { "step1": "fn main() -> Int { 1 }\n" }
    })
    .to_string();
    std::fs::create_dir_all(dir.path()).unwrap();
    let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
    store
        .insert_run(&RunRow {
            run_id: "run-bg".into(),
            workflow_name: "wf".into(),
            workflow_hash: "h".into(),
            status: RunStatus::Running,
            started_at_ms: 0,
            updated_at_ms: 0,
            policy_json: r#"{"default_allow":true}"#.into(),
            metadata_json,
        })
        .unwrap();
    store
        .upsert_step_checkpoint(&StepCheckpoint {
            run_id: "run-bg".into(),
            step_id: "step1".into(),
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
    drop(store);

    // Spawn coordinator with a fast sweep interval (200 ms).
    let (coord_child, _port) = spawn_coordinator_with_sweep(dir.path(), 60_000, 1_000, 200);

    // Give the coordinator a moment past startup, then create
    // a stale claim using the persistence API directly. The
    // claim's lease expires in the past (`lease_expires_at=1`).
    std::thread::sleep(Duration::from_millis(300));
    let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
    let outcome = store
        .claim_step("run-bg", "step1", "ghost-worker", 1, 0)
        .unwrap();
    assert!(matches!(
        outcome,
        boruna_orchestrator::persistence::ClaimOutcome::Claimed { .. }
    ));
    drop(store);

    // Poll for status flip (deadline 5s — plenty of margin
    // even under parallel-test CPU contention; sweep interval
    // is 200 ms).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut cp = None;
    while Instant::now() < deadline {
        let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
        let candidate = store
            .list_step_checkpoints("run-bg")
            .unwrap()
            .pop()
            .unwrap();
        drop(store);
        if candidate.status == StepStatus::Pending {
            cp = Some(candidate);
            break;
        }
        cp = Some(candidate);
        std::thread::sleep(Duration::from_millis(100));
    }
    kill_child(coord_child);
    let cp = cp.expect("step row not found");

    assert_eq!(
        cp.status,
        StepStatus::Pending,
        "background sweep should have requeued the stale-lease row"
    );
    assert_eq!(cp.worker_id, None);
    assert_eq!(cp.lease_expires_at_ms, None);
    // claim_id is preserved across requeue (per 0.5-S2a contract).
    assert_eq!(cp.claim_id, 1);
}

#[test]
fn coord_serve_responds_to_dashboard_index() {
    // Sprint 0.5-S2d: the coordinator now serves the
    // dashboard's read routes on the same listener.
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(
        dir.path(),
        "run-merged",
        "step1",
        "fn main() -> Int { 1 }\n",
    );
    let (child, port) = spawn_coordinator(dir.path(), 60_000, 200);
    let (code, body) = http_request(port, "GET", "/", None);
    kill_child(child);
    assert_eq!(code, 200, "body: {body}");
    assert!(body.contains("<html"), "body: {body}");
    assert!(body.contains("Boruna runs"), "body: {body}");
    assert!(body.contains("run-merged"), "body: {body}");
}

#[test]
fn coord_serve_responds_to_dashboard_api_runs() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-api", "step1", "fn main() -> Int { 1 }\n");
    let (child, port) = spawn_coordinator(dir.path(), 60_000, 200);
    let (code, body) = http_request(port, "GET", "/api/runs", None);
    kill_child(child);
    assert_eq!(code, 200);
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(v["runs"].is_array());
    assert_eq!(v["runs"][0]["run_id"], "run-api");
    // Slim RunSummary contract from 0.4-S16: no policy/metadata
    // leakage even on the merged listener.
    let json = body.clone();
    assert!(!json.contains("policy_json"));
    assert!(!json.contains("metadata_json"));
}

#[test]
fn coord_serve_handles_both_coord_and_dashboard_routes_on_same_listener() {
    // Adversarial-review gap: existing tests exercise coord
    // routes OR dashboard routes against a coord process,
    // never both against the same running instance. This
    // test proves the merge actually works at runtime — both
    // route trees coexist on one port.
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-merge", "step1", "fn main() -> Int { 1 }\n");
    let (child, port) = spawn_coordinator(dir.path(), 60_000, 200);

    // 1. Hit a coord route — register a worker.
    let cap_hash = boruna_bytecode::compute_capability_set_hash(
        boruna_bytecode::Capability::ALL
            .iter()
            .map(|c| (c.name().to_string(), c.version().to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str())),
    );
    let body = serde_json::json!({"capability_set_hash": cap_hash}).to_string();
    let (coord_code, coord_resp) = http_request(port, "POST", "/api/workers/register", Some(&body));
    assert_eq!(coord_code, 200, "coord route failed: {coord_resp}");

    // 2. Hit a dashboard route — list runs — on the SAME
    //    listener. Note: same port; new connection (our HTTP
    //    helper closes after each request).
    let (dash_code, dash_resp) = http_request(port, "GET", "/api/runs", None);
    assert_eq!(dash_code, 200, "dashboard route failed: {dash_resp}");
    let v: serde_json::Value = serde_json::from_str(&dash_resp).expect("json");
    assert_eq!(v["runs"][0]["run_id"], "run-merge");

    // 3. Hit both again to ensure neither broke the other's
    //    state.
    let (h_code, _) = http_request(
        port,
        "POST",
        "/api/workers/heartbeat",
        Some(&serde_json::json!({
            "worker_id": serde_json::from_str::<serde_json::Value>(&coord_resp).unwrap()["worker_id"],
            "session_token": serde_json::from_str::<serde_json::Value>(&coord_resp).unwrap()["session_token"],
        }).to_string()),
    );
    assert_eq!(h_code, 200, "heartbeat failed after dashboard call");
    let (idx_code, _) = http_request(port, "GET", "/", None);
    assert_eq!(idx_code, 200, "index failed after coord call");

    kill_child(child);
}

#[test]
fn coord_serve_dashboard_404_for_unknown_run() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-x", "step1", "fn main() -> Int { 1 }\n");
    let (child, port) = spawn_coordinator(dir.path(), 60_000, 200);
    let (code, _) = http_request(port, "GET", "/runs/no-such-id", None);
    let (api_code, _) = http_request(port, "GET", "/api/runs/no-such-id", None);
    kill_child(child);
    assert_eq!(code, 404);
    assert_eq!(api_code, 404);
}

#[test]
fn worker_completes_two_step_linear_dag() {
    // Sprint 0.5-S2c: prove the protocol scales beyond a
    // single step. Pre-populate two Pending steps; the
    // worker claims+completes both. Note: the coordinator
    // does NOT yet do DAG advancement (that's 0.5-S2d), so
    // this test pre-populates BOTH steps as Pending up
    // front. In practice the operator's wave loop would do
    // this via separate calls to upsert_step_checkpoint as
    // each step's dependency is satisfied.
    use boruna_orchestrator::persistence::StepCheckpoint;
    let dir = tempfile::tempdir().unwrap();
    let metadata_json = serde_json::json!({
        "step_sources": {
            "step1": "fn main() -> Int { 10 }\n",
            "step2": "fn main() -> Int { 20 }\n",
        }
    })
    .to_string();
    std::fs::create_dir_all(dir.path()).unwrap();
    let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
    store
        .insert_run(&RunRow {
            run_id: "run-2step".into(),
            workflow_name: "wf".into(),
            workflow_hash: "h".into(),
            status: RunStatus::Running,
            started_at_ms: 0,
            updated_at_ms: 0,
            policy_json: r#"{"default_allow":true}"#.into(),
            metadata_json,
        })
        .unwrap();
    for step_id in ["step1", "step2"] {
        store
            .upsert_step_checkpoint(&StepCheckpoint {
                run_id: "run-2step".into(),
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
    drop(store);

    let (coord_child, port) = spawn_coordinator(dir.path(), 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "two-step-worker", 30_000);

    let db_path = dir.path().join("runs.db");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut both_completed = false;
    let mut last_state = String::new();
    while Instant::now() < deadline {
        let store = RunCheckpointStore::open(&db_path).unwrap();
        let cps = store.list_step_checkpoints("run-2step").unwrap();
        last_state = format!(
            "{:?}",
            cps.iter()
                .map(|c| (c.step_id.as_str(), c.status))
                .collect::<Vec<_>>()
        );
        if cps.len() == 2 && cps.iter().all(|c| c.status == StepStatus::Completed) {
            both_completed = true;
            // Verify the per-step outputs.
            for cp in &cps {
                let expected_output = match cp.step_id.as_str() {
                    "step1" => "10",
                    "step2" => "20",
                    _ => panic!("unexpected step_id {}", cp.step_id),
                };
                assert_eq!(cp.output_json.as_deref(), Some(expected_output));
                assert!(cp.output_hash.as_deref().unwrap().starts_with("sha256:"));
                assert_eq!(cp.claim_id, 1);
            }
            break;
        }
        drop(store);
        std::thread::sleep(Duration::from_millis(100));
    }
    kill_child(worker);
    kill_child(coord_child);
    assert!(
        both_completed,
        "expected both steps Completed within 15s; last state: {last_state}"
    );
}

// ── shared-secret auth (sprint 0.5-S3) ──

#[test]
fn coord_with_secret_rejects_request_without_bearer() {
    // Coord configured with --shared-secret. A naked request to
    // /api/workers/register without Authorization header → 401.
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-init", "noop", "fn main() -> Int { 0 }\n");
    let secret = "test-secret-32-hex-chars-aaaa";
    let (child, port) = spawn_coordinator_with_secret(dir.path(), secret);
    let cap_hash = boruna_bytecode::compute_capability_set_hash(
        boruna_bytecode::Capability::ALL
            .iter()
            .map(|c| (c.name().to_string(), c.version().to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str())),
    );
    let body = serde_json::json!({ "capability_set_hash": cap_hash }).to_string();
    let (code, resp) =
        http_request_with_auth(port, "POST", "/api/workers/register", Some(&body), None);
    kill_child(child);
    assert_eq!(code, 401, "resp: {resp}");
    let v: serde_json::Value = serde_json::from_str(&resp).expect("json");
    assert_eq!(v["error_kind"], "coord.unauthorized");
    assert_eq!(v["protocol_version"], 1);
}

#[test]
fn coord_with_secret_rejects_request_with_wrong_bearer() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-init", "noop", "fn main() -> Int { 0 }\n");
    let secret = "the-real-secret-aaaaaaaaaaaa";
    let wrong = "the-wrong-secret-bbbbbbbbbbbb";
    let (child, port) = spawn_coordinator_with_secret(dir.path(), secret);
    let cap_hash = boruna_bytecode::compute_capability_set_hash(
        boruna_bytecode::Capability::ALL
            .iter()
            .map(|c| (c.name().to_string(), c.version().to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str())),
    );
    let body = serde_json::json!({ "capability_set_hash": cap_hash }).to_string();
    let (code, resp) = http_request_with_auth(
        port,
        "POST",
        "/api/workers/register",
        Some(&body),
        Some(wrong),
    );
    kill_child(child);
    assert_eq!(code, 401, "resp: {resp}");
    let v: serde_json::Value = serde_json::from_str(&resp).expect("json");
    assert_eq!(v["error_kind"], "coord.unauthorized");
}

#[test]
fn coord_with_secret_accepts_request_with_correct_bearer() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-init", "noop", "fn main() -> Int { 0 }\n");
    let secret = "matching-secret-xxxxxxxxxxx";
    let (child, port) = spawn_coordinator_with_secret(dir.path(), secret);
    let cap_hash = boruna_bytecode::compute_capability_set_hash(
        boruna_bytecode::Capability::ALL
            .iter()
            .map(|c| (c.name().to_string(), c.version().to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str())),
    );
    let body = serde_json::json!({ "capability_set_hash": cap_hash }).to_string();
    let (code, resp) = http_request_with_auth(
        port,
        "POST",
        "/api/workers/register",
        Some(&body),
        Some(secret),
    );
    kill_child(child);
    assert_eq!(code, 200, "resp: {resp}");
    let v: serde_json::Value = serde_json::from_str(&resp).expect("json");
    assert_eq!(v["protocol_version"], 1);
    assert!(v["worker_id"].as_str().unwrap().starts_with("wkr-"));
}

#[test]
fn coord_without_secret_accepts_unauth_request_no_regression() {
    // Existing test surface: when no --shared-secret, no auth required.
    // This duplicates `coord_register_returns_worker_id_and_session_token`
    // explicitly via the auth-aware HTTP helper to lock the no-regression
    // contract. Sprint 0.5-S3 must not break loopback-only deployments.
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path(), "run-init", "noop", "fn main() -> Int { 0 }\n");
    let (child, port) = spawn_coordinator(dir.path(), 60_000, 200);
    let cap_hash = boruna_bytecode::compute_capability_set_hash(
        boruna_bytecode::Capability::ALL
            .iter()
            .map(|c| (c.name().to_string(), c.version().to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str())),
    );
    let body = serde_json::json!({ "capability_set_hash": cap_hash }).to_string();
    let (code, _resp) =
        http_request_with_auth(port, "POST", "/api/workers/register", Some(&body), None);
    kill_child(child);
    assert_eq!(code, 200);
}

// ── Sprint 0.5-S4 — `workflow run --coordinator` end-to-end ──

fn write_single_step_workflow(wf_dir: &Path, body: &str) {
    std::fs::create_dir_all(wf_dir).unwrap();
    std::fs::write(wf_dir.join("s1.ax"), body).unwrap();
    std::fs::write(
        wf_dir.join("workflow.json"),
        r#"{
            "schema_version": 1,
            "name": "remote-run-test",
            "version": "1.0.0",
            "steps": {
                "s1": {"kind": "source", "source": "s1.ax", "capabilities": [], "outputs": {"result": "Int"}}
            },
            "edges": []
        }"#,
    )
    .unwrap();
}

#[test]
fn cli_workflow_run_coordinator_drives_remote_run_to_completion() {
    // End-to-end: operator's CLI submits a workflow over HTTP to a
    // remote coordinator (different data-dir), the coordinator
    // dispatches it to a connected worker, the worker runs the
    // step, and the CLI's polling loop sees Completed and exits 0.
    let tmp = tempfile::tempdir().unwrap();
    let coord_data = tmp.path().join("coord-data");
    std::fs::create_dir_all(&coord_data).unwrap();
    // Coordinator's `serve` requires runs.db to already exist (the
    // pre-0.5-S4 model assumed an operator had submitted at least
    // one workflow locally first). Touch the schema by opening the
    // store and dropping it.
    drop(RunCheckpointStore::open(&coord_data.join("runs.db")).unwrap());
    let wf_dir = tmp.path().join("wf");
    write_single_step_workflow(&wf_dir, "fn main() -> Int { 7 }\n");

    let (coord_child, port) = spawn_coordinator(&coord_data, 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "remote-run-worker", 30_000);

    let out = Command::new(boruna_bin())
        .args([
            "workflow",
            "run",
            wf_dir.to_str().unwrap(),
            "--coordinator",
            &coord_url,
            "--coord-poll-interval-ms",
            "200",
            "--coord-max-wait-secs",
            "30",
        ])
        .output()
        .expect("invoke boruna workflow run --coordinator");

    kill_child(worker);
    kill_child(coord_child);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "workflow run --coordinator failed:\nstdout={stdout}\nstderr={stderr}"
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.contains("step s1: completed") && stdout.contains(": completed"),
        "expected completion lines in stdout, got: {stdout}"
    );
}

#[test]
fn cli_workflow_run_coordinator_exits_1_on_step_failure() {
    // Mirror of the success case but with a step source that fails
    // at runtime. Exit code must be 1 (Failed), not 2 (timeout).
    let tmp = tempfile::tempdir().unwrap();
    let coord_data = tmp.path().join("coord-data");
    std::fs::create_dir_all(&coord_data).unwrap();
    // Coordinator's `serve` requires runs.db to already exist (the
    // pre-0.5-S4 model assumed an operator had submitted at least
    // one workflow locally first). Touch the schema by opening the
    // store and dropping it.
    drop(RunCheckpointStore::open(&coord_data.join("runs.db")).unwrap());
    let wf_dir = tmp.path().join("wf");
    // Use match exhaustion to force a runtime failure.
    write_single_step_workflow(
        &wf_dir,
        r#"fn main() -> Int { match 99 { 0 => 0, 1 => 1 } }
"#,
    );

    let (coord_child, port) = spawn_coordinator(&coord_data, 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "remote-run-fail-worker", 30_000);

    let out = Command::new(boruna_bin())
        .args([
            "workflow",
            "run",
            wf_dir.to_str().unwrap(),
            "--coordinator",
            &coord_url,
            "--coord-poll-interval-ms",
            "200",
            "--coord-max-wait-secs",
            "30",
        ])
        .output()
        .expect("invoke boruna workflow run --coordinator");

    kill_child(worker);
    kill_child(coord_child);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 (Failed), got {:?}\nstdout={stdout}",
        out.status.code()
    );
    assert!(
        stdout.contains("failed"),
        "expected 'failed' line in stdout, got: {stdout}"
    );
}

#[test]
fn cli_workflow_approve_via_coordinator_advances_remote_run() {
    // Sprint 0.5-S6: end-to-end approval gate over HTTP.
    // 1. Submit a workflow with an approval gate via --coordinator.
    //    The CLI submits + polls; while it polls in the foreground,
    //    a worker drives `analyze` to Completed; the gate then
    //    opens (AwaitingApproval) and the foreground CLI keeps
    //    polling.
    // 2. From a separate process we run `workflow approve --coordinator`
    //    against the open gate.
    // 3. The foreground CLI sees Completed and exits 0.
    let tmp = tempfile::tempdir().unwrap();
    let coord_data = tmp.path().join("coord-data");
    std::fs::create_dir_all(&coord_data).unwrap();
    drop(RunCheckpointStore::open(&coord_data.join("runs.db")).unwrap());
    let wf_dir = tmp.path().join("wf");
    std::fs::create_dir_all(&wf_dir).unwrap();
    std::fs::write(wf_dir.join("analyze.ax"), "fn main() -> Int { 1 }\n").unwrap();
    std::fs::write(
        wf_dir.join("workflow.json"),
        r#"{
            "schema_version": 1,
            "name": "approve-test",
            "version": "1.0.0",
            "steps": {
                "analyze": {"kind": "source", "source": "analyze.ax", "capabilities": [], "outputs": {"result": "Int"}},
                "human_review": {"kind": "approval_gate", "required_role": "reviewer", "depends_on": ["analyze"], "capabilities": [], "outputs": {}}
            },
            "edges": [["analyze", "human_review"]]
        }"#,
    )
    .unwrap();

    let (coord_child, port) = spawn_coordinator(&coord_data, 60_000, 1_000);
    let coord_url = format!("http://127.0.0.1:{port}");
    let worker = spawn_worker(&coord_url, "approve-worker", 30_000);

    // Spawn the foreground `workflow run --coordinator` in the
    // background so the approval can race-in while it polls.
    let mut run_child = Command::new(boruna_bin())
        .args([
            "workflow",
            "run",
            wf_dir.to_str().unwrap(),
            "--coordinator",
            &coord_url,
            "--coord-poll-interval-ms",
            "200",
            "--coord-max-wait-secs",
            "30",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn workflow run");

    // Wait for analyze to finish + gate to open (poll the dashboard
    // store directly to find the run_id).
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut run_id = String::new();
    while Instant::now() < deadline {
        let store = RunCheckpointStore::open(&coord_data.join("runs.db")).unwrap();
        let runs = store.list_runs().unwrap();
        if let Some(r) = runs.first() {
            let cps = store.list_step_checkpoints(&r.run_id).unwrap();
            if cps
                .iter()
                .any(|c| c.step_id == "human_review" && c.status == StepStatus::AwaitingApproval)
            {
                run_id = r.run_id.clone();
                break;
            }
        }
        drop(store);
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!run_id.is_empty(), "gate never opened within 20s");

    // Approve via remote CLI.
    let approve = Command::new(boruna_bin())
        .args([
            "workflow",
            "approve",
            &run_id,
            "human_review",
            "--coordinator",
            &coord_url,
        ])
        .output()
        .expect("invoke workflow approve --coordinator");
    assert!(
        approve.status.success(),
        "approve failed: stderr={}",
        String::from_utf8_lossy(&approve.stderr)
    );

    // Foreground CLI should now exit 0.
    let exit = run_child.wait().expect("wait foreground run");
    kill_child(worker);
    kill_child(coord_child);
    assert_eq!(
        exit.code(),
        Some(0),
        "foreground CLI did not exit 0 (got {:?})",
        exit.code()
    );
}

#[test]
fn cli_workflow_run_coordinator_rejects_data_dir_combo() {
    // --coordinator and --data-dir are mutually exclusive at the
    // clap level. clap should refuse before any side effect.
    let tmp = tempfile::tempdir().unwrap();
    let wf_dir = tmp.path().join("wf");
    write_single_step_workflow(&wf_dir, "fn main() -> Int { 1 }\n");
    let out = Command::new(boruna_bin())
        .args([
            "workflow",
            "run",
            wf_dir.to_str().unwrap(),
            "--coordinator",
            "http://127.0.0.1:1",
            "--data-dir",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .expect("invoke boruna workflow run");
    assert!(
        !out.status.success(),
        "expected clap to reject --coordinator + --data-dir"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("conflicts"),
        "expected conflict error, got: {stderr}"
    );
}
