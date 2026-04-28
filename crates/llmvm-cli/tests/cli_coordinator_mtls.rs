//! End-to-end mTLS surface integration tests for the
//! coordinator (sprint `W6-A`). Generates a self-signed CA +
//! server cert + client cert in a tempdir using `rcgen`, spins
//! up `boruna coordinator serve` with mTLS enabled, and asserts
//! the four required adversarial properties:
//!
//! 1. A connection without a client cert is rejected at the TLS
//!    handshake.
//! 2. A connection with a client cert from a DIFFERENT CA is
//!    rejected at the TLS handshake.
//! 3. A valid client cert succeeds; the cert subject CN drives
//!    the recorded `worker_id` on registration.
//! 4. A valid cert with a body `worker_id` that does NOT match
//!    the cert CN returns 401 `coord.identity_mismatch`.
//!
//! Only compiled when `--features serve` is enabled.

#![cfg(feature = "serve")]

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
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
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            std::thread::sleep(Duration::from_millis(150));
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server on port {port} never came up within 15s");
}

fn populate_pending_step(data_dir: &Path) {
    std::fs::create_dir_all(data_dir).unwrap();
    let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();
    let metadata_json = serde_json::json!({
        "step_sources": { "noop": "fn main() -> Int { 0 }\n" }
    })
    .to_string();
    store
        .insert_run(&RunRow {
            run_id: "run-init".into(),
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
            run_id: "run-init".into(),
            step_id: "noop".into(),
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

fn kill_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// A complete on-disk certificate bundle for the test.
struct CertBundle {
    /// CA cert PEM path (used both as server's client-CA and as
    /// the worker's server-CA — the same self-signed root signs
    /// both ends in this test).
    ca_pem: PathBuf,
    server_cert_pem: PathBuf,
    server_key_pem: PathBuf,
    /// Per-test client cert + key. Subject CN = `client_cn`.
    client_cert_pem: PathBuf,
    client_key_pem: PathBuf,
    client_cn: String,
}

fn generate_certs(dir: &Path, client_cn: &str) -> CertBundle {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
        KeyUsagePurpose,
    };

    // 1. CA.
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "boruna-mtls-test-CA");
    ca_params.distinguished_name = ca_dn;
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    // 2. Server cert (CN=localhost, SAN includes IP 127.0.0.1).
    let mut server_params =
        CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let mut server_dn = DistinguishedName::new();
    server_dn.push(DnType::CommonName, "localhost");
    server_params.distinguished_name = server_dn;
    let server_key = KeyPair::generate().unwrap();
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .unwrap();

    // 3. Client cert with the requested CN.
    let mut client_params = CertificateParams::new(vec![client_cn.to_string()]).unwrap();
    let mut client_dn = DistinguishedName::new();
    client_dn.push(DnType::CommonName, client_cn);
    client_params.distinguished_name = client_dn;
    let client_key = KeyPair::generate().unwrap();
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .unwrap();

    let ca_pem = dir.join("ca.pem");
    let server_cert_pem = dir.join("server-cert.pem");
    let server_key_pem = dir.join("server-key.pem");
    let client_cert_pem = dir.join("client-cert.pem");
    let client_key_pem = dir.join("client-key.pem");
    write_pem(&ca_pem, &ca_cert.pem());
    write_pem(&server_cert_pem, &server_cert.pem());
    write_pem(&server_key_pem, &server_key.serialize_pem());
    write_pem(&client_cert_pem, &client_cert.pem());
    write_pem(&client_key_pem, &client_key.serialize_pem());

    CertBundle {
        ca_pem,
        server_cert_pem,
        server_key_pem,
        client_cert_pem,
        client_key_pem,
        client_cn: client_cn.to_string(),
    }
}

fn write_pem(path: &Path, content: &str) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
}

fn spawn_mtls_coordinator(data_dir: &Path, certs: &CertBundle) -> (Child, u16) {
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
            "--tls-cert",
            certs.server_cert_pem.to_str().unwrap(),
            "--tls-key",
            certs.server_key_pem.to_str().unwrap(),
            "--tls-client-ca",
            certs.ca_pem.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mTLS coordinator");
    wait_for_server(port);
    (child, port)
}

/// Build a blocking reqwest client with a specific client cert
/// and trust root.
fn build_https_client(
    cert_pem: &Path,
    key_pem: &Path,
    server_ca_pem: &Path,
) -> reqwest::blocking::Client {
    let mut id_pem = std::fs::read(cert_pem).unwrap();
    if !id_pem.ends_with(b"\n") {
        id_pem.push(b'\n');
    }
    id_pem.extend_from_slice(&std::fs::read(key_pem).unwrap());
    let identity = reqwest::Identity::from_pem(&id_pem).expect("identity");
    let ca_pem = std::fs::read(server_ca_pem).unwrap();
    let ca = reqwest::Certificate::from_pem(&ca_pem).expect("server CA");
    reqwest::blocking::Client::builder()
        .use_rustls_tls()
        .identity(identity)
        .add_root_certificate(ca)
        .timeout(Duration::from_secs(10))
        .build()
        .expect("https client")
}

/// Connect with no client cert at all and confirm the TLS
/// handshake does not produce an HTTP response. Uses a low-level
/// rustls ClientConfig with no client identity. The server-side
/// `WebPkiClientVerifier` requires a client cert; the handshake
/// should fail with a "certificate required" alert.
fn assert_handshake_rejected_no_cert(port: u16, server_ca_pem: &Path) {
    use std::io::Read;

    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, RootCertStore};
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let ca_pem = std::fs::read(server_ca_pem).unwrap();
    let mut reader = std::io::Cursor::new(ca_pem);
    let mut roots = RootCertStore::empty();
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<_, _>>()
        .unwrap();
    for c in certs {
        roots.add(c).unwrap();
    }
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = ServerName::try_from("localhost").unwrap();
    let mut client = rustls::ClientConnection::new(Arc::new(config), server_name).unwrap();
    let mut sock = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    sock.set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut tls = rustls::Stream::new(&mut client, &mut sock);
    let req = format!(
        "POST /api/workers/register HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
    );
    // Try to drive the handshake to completion. Rustls buffers
    // application data until handshake is done; the failure
    // surfaces on the next read. Either write OR read failing
    // proves the handshake was rejected.
    let write_res = tls.write_all(req.as_bytes());
    let mut buf = [0u8; 64];
    let read_res = tls.read(&mut buf);
    let read_zero = matches!(read_res, Ok(0));
    assert!(
        write_res.is_err() || read_res.is_err() || read_zero,
        "expected handshake rejection without client cert; \
         write={write_res:?} read={read_res:?}"
    );
}

fn cap_hash() -> String {
    boruna_bytecode::compute_capability_set_hash(
        boruna_bytecode::Capability::ALL
            .iter()
            .map(|c| (c.name().to_string(), c.version().to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str())),
    )
}

#[test]
fn mtls_no_client_cert_handshake_rejected() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path());
    let cert_dir = dir.path().join("certs");
    std::fs::create_dir_all(&cert_dir).unwrap();
    let certs = generate_certs(&cert_dir, "worker-A");
    let (child, port) = spawn_mtls_coordinator(dir.path(), &certs);
    let result = std::panic::catch_unwind(|| {
        assert_handshake_rejected_no_cert(port, &certs.ca_pem);
    });
    kill_child(child);
    result.expect("handshake-rejection assertion");
}

#[test]
fn mtls_client_cert_from_different_ca_handshake_rejected() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path());
    let cert_dir = dir.path().join("certs");
    let foreign_dir = dir.path().join("foreign");
    std::fs::create_dir_all(&cert_dir).unwrap();
    std::fs::create_dir_all(&foreign_dir).unwrap();
    let certs = generate_certs(&cert_dir, "worker-A");
    // Generate a SECOND, untrusted CA + client cert.
    let foreign = generate_certs(&foreign_dir, "worker-A");
    let (child, port) = spawn_mtls_coordinator(dir.path(), &certs);

    // Use the foreign client cert against the server's trusted
    // CA. The handshake should fail because the server's
    // WebPkiClientVerifier doesn't trust the foreign CA.
    let client = build_https_client(
        &foreign.client_cert_pem,
        &foreign.client_key_pem,
        &certs.ca_pem,
    );
    let body = serde_json::json!({
        "worker_id": "worker-A",
        "capability_set_hash": cap_hash(),
    })
    .to_string();
    let url = format!("https://localhost:{port}/api/workers/register");
    let res = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send();
    kill_child(child);
    // We expect a transport-level error (handshake failure), not
    // an HTTP response.
    assert!(
        res.is_err(),
        "expected handshake rejection for foreign-CA cert; got {res:?}"
    );
}

#[test]
fn mtls_valid_client_cert_succeeds_and_cn_drives_worker_id() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path());
    let cert_dir = dir.path().join("certs");
    std::fs::create_dir_all(&cert_dir).unwrap();
    let certs = generate_certs(&cert_dir, "worker-good-7");
    let (child, port) = spawn_mtls_coordinator(dir.path(), &certs);
    let client = build_https_client(&certs.client_cert_pem, &certs.client_key_pem, &certs.ca_pem);
    // No worker_id in body — exercise the CN-drives-identity
    // branch.
    let body = serde_json::json!({
        "capability_set_hash": cap_hash(),
    })
    .to_string();
    let url = format!("https://localhost:{port}/api/workers/register");
    let res = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send();
    let status = res.as_ref().map(|r| r.status().as_u16()).unwrap_or(0);
    let text = res
        .map(|r| r.text().unwrap_or_default())
        .unwrap_or_default();
    kill_child(child);
    assert_eq!(status, 200, "resp: {text}");
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(v["worker_id"], certs.client_cn);
}

#[test]
fn mtls_cn_mismatch_returns_identity_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path());
    let cert_dir = dir.path().join("certs");
    std::fs::create_dir_all(&cert_dir).unwrap();
    let certs = generate_certs(&cert_dir, "worker-A");
    let (child, port) = spawn_mtls_coordinator(dir.path(), &certs);
    let client = build_https_client(&certs.client_cert_pem, &certs.client_key_pem, &certs.ca_pem);
    // Body says we are worker-B but our cert CN is worker-A.
    let body = serde_json::json!({
        "worker_id": "worker-B",
        "capability_set_hash": cap_hash(),
    })
    .to_string();
    let url = format!("https://localhost:{port}/api/workers/register");
    let res = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send();
    let status = res.as_ref().map(|r| r.status().as_u16()).unwrap_or(0);
    let text = res
        .map(|r| r.text().unwrap_or_default())
        .unwrap_or_default();
    kill_child(child);
    assert_eq!(status, 401, "resp: {text}");
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(v["error_kind"], "coord.identity_mismatch");
}

#[test]
fn mtls_partial_flags_are_rejected_at_startup() {
    // Pass --tls-cert without --tls-key/--tls-client-ca and
    // confirm the process exits with the typed error message
    // (project §1: half-configured TLS rejected at parse).
    let dir = tempfile::tempdir().unwrap();
    populate_pending_step(dir.path());
    let cert_dir = dir.path().join("certs");
    std::fs::create_dir_all(&cert_dir).unwrap();
    let certs = generate_certs(&cert_dir, "worker-A");
    let port = pick_free_port();
    let out = Command::new(boruna_bin())
        .args([
            "coordinator",
            "serve",
            "--data-dir",
            dir.path().to_str().unwrap(),
            "--port",
            &port.to_string(),
            "--tls-cert",
            certs.server_cert_pem.to_str().unwrap(),
        ])
        .output()
        .expect("invoke");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("must all be provided together"),
        "stderr: {stderr}"
    );
}
