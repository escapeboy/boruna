//! End-to-end CLI integration tests for `boruna dashboard serve`
//! (sprint 0.4-S16). These tests spawn the binary, hit the running
//! HTTP server, and assert end-to-end behavior.
//!
//! Only compiled when `--features serve` is enabled.

#![cfg(feature = "serve")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

/// Find a free TCP port on the loopback interface.
fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Wait for the server to become reachable on `127.0.0.1:port`.
/// Returns the running child handle on success.
fn wait_for_server(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server on port {port} never came up within 5s");
}

/// Hand-rolled minimal HTTP/1.1 GET to keep test deps out of the
/// CLI Cargo.toml. Returns `(status_code, body)`.
fn http_get(port: u16, path: &str) -> (u16, String) {
    http_request(port, "GET", path)
}

fn http_request(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to server");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let req =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).expect("write request");
    let mut reader = BufReader::new(&stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).expect("read status");
    // Parse "HTTP/1.1 200 OK\r\n"
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    let code: u16 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    // Skip headers until blank line
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap_or(0);
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut body = String::new();
    let _ = reader.read_to_string(&mut body);
    let _ = stream.shutdown(Shutdown::Both);
    (code, body)
}

/// Spawn the dashboard, return the running child + port.
/// Caller MUST call `kill_child` when done.
fn spawn_dashboard(data_dir: &Path) -> (Child, u16) {
    let port = pick_free_port();
    let child = Command::new(boruna_bin())
        .args([
            "dashboard",
            "serve",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--port",
            &port.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn boruna dashboard");
    wait_for_server(port);
    (child, port)
}

fn kill_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Populate a runs.db with a single run + step using the
/// orchestrator's persistence APIs directly. Faster than running
/// an actual workflow.
fn populate_db(data_dir: &Path) {
    use boruna_orchestrator::persistence::{
        RunCheckpointStore, RunRow, RunStatus, StepCheckpoint, StepStatus,
    };
    std::fs::create_dir_all(data_dir).unwrap();
    let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();
    store
        .insert_run(&RunRow {
            run_id: "r-test".into(),
            workflow_name: "etl".into(),
            workflow_hash: "deadbeef".into(),
            status: RunStatus::Running,
            started_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_500,
            policy_json: "{}".into(),
            metadata_json: "{}".into(),
        })
        .unwrap();
    store
        .upsert_step_checkpoint(&StepCheckpoint {
            run_id: "r-test".into(),
            step_id: "extract".into(),
            status: StepStatus::Completed,
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
        })
        .unwrap();
}

#[test]
fn cli_dashboard_serve_responds_to_index() {
    let dir = tempfile::tempdir().unwrap();
    populate_db(dir.path());
    let (child, port) = spawn_dashboard(dir.path());
    let (code, body) = http_get(port, "/");
    kill_child(child);
    assert_eq!(code, 200);
    assert!(body.contains("<html"), "body: {body}");
    assert!(body.contains("Boruna runs"), "body: {body}");
    assert!(body.contains("r-test"), "body: {body}");
}

#[test]
fn cli_dashboard_serve_responds_to_api_runs() {
    let dir = tempfile::tempdir().unwrap();
    populate_db(dir.path());
    let (child, port) = spawn_dashboard(dir.path());
    let (code, body) = http_get(port, "/api/runs");
    kill_child(child);
    assert_eq!(code, 200);
    let v: serde_json::Value = serde_json::from_str(&body).expect("body is JSON");
    assert!(v["runs"].is_array());
    assert_eq!(v["runs"][0]["run_id"], "r-test");
}

#[test]
fn cli_dashboard_serve_run_detail_html() {
    let dir = tempfile::tempdir().unwrap();
    populate_db(dir.path());
    let (child, port) = spawn_dashboard(dir.path());
    let (code, body) = http_get(port, "/runs/r-test");
    kill_child(child);
    assert_eq!(code, 200);
    assert!(body.contains("Run r-test"), "body: {body}");
    assert!(body.contains("extract"), "body: {body}");
}

#[test]
fn cli_dashboard_serve_api_run_detail_json() {
    let dir = tempfile::tempdir().unwrap();
    populate_db(dir.path());
    let (child, port) = spawn_dashboard(dir.path());
    let (code, body) = http_get(port, "/api/runs/r-test");
    kill_child(child);
    assert_eq!(code, 200);
    let v: serde_json::Value = serde_json::from_str(&body).expect("body is JSON");
    assert_eq!(v["run"]["run_id"], "r-test");
    assert!(v["operational"].is_object());
    assert_eq!(v["steps"][0]["step_id"], "extract");
}

#[test]
fn cli_dashboard_serve_404_for_unknown_run() {
    let dir = tempfile::tempdir().unwrap();
    populate_db(dir.path());
    let (child, port) = spawn_dashboard(dir.path());
    let (code, _) = http_get(port, "/runs/no-such-id");
    let (api_code, _) = http_get(port, "/api/runs/no-such-id");
    kill_child(child);
    assert_eq!(code, 404);
    assert_eq!(api_code, 404);
}

#[test]
fn cli_dashboard_serve_post_returns_405() {
    // Regression: read-only contract. POST/PUT/DELETE on any route
    // must NOT be wired. Axum's default for an unmatched method on
    // a registered path is 405 Method Not Allowed.
    let dir = tempfile::tempdir().unwrap();
    populate_db(dir.path());
    let (child, port) = spawn_dashboard(dir.path());
    let (code_root, _) = http_request(port, "POST", "/");
    let (code_api, _) = http_request(port, "POST", "/api/runs");
    let (code_delete, _) = http_request(port, "DELETE", "/api/runs/r-test");
    kill_child(child);
    assert_eq!(code_root, 405, "POST / should be 405");
    assert_eq!(code_api, 405, "POST /api/runs should be 405");
    assert_eq!(code_delete, 405, "DELETE /api/runs/{{id}} should be 405");
}

#[test]
fn cli_dashboard_serve_missing_data_dir_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let bogus = dir.path().join("does-not-exist");
    let port = pick_free_port();
    let out = Command::new(boruna_bin())
        .args([
            "dashboard",
            "serve",
            "--data-dir",
            bogus.to_str().unwrap(),
            "--port",
            &port.to_string(),
        ])
        .output()
        .expect("invoke boruna");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("runs.db"), "stderr: {stderr}");
}

#[test]
fn cli_dashboard_serve_invalid_bind_address_fails() {
    let dir = tempfile::tempdir().unwrap();
    populate_db(dir.path());
    let port = pick_free_port();
    let out = Command::new(boruna_bin())
        .args([
            "dashboard",
            "serve",
            "--data-dir",
            dir.path().to_str().unwrap(),
            "--port",
            &port.to_string(),
            "--bind",
            "not-an-ip",
        ])
        .output()
        .expect("invoke boruna");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid --bind"), "stderr: {stderr}");
}
