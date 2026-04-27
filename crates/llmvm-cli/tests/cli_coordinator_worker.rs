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
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
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

    // Wait long enough for at least one sweep tick.
    std::thread::sleep(Duration::from_millis(600));

    // Verify the row is back to Pending without any external
    // sweep call.
    let store = RunCheckpointStore::open(&dir.path().join("runs.db")).unwrap();
    let cp = store
        .list_step_checkpoints("run-bg")
        .unwrap()
        .pop()
        .unwrap();
    drop(store);
    kill_child(coord_child);

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
