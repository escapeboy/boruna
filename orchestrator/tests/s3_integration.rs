//! S3 BundleStorage integration tests (post1-T-3.1).
//!
//! These tests spin up a real MinIO container via testcontainers
//! and exercise the full `put → list → get` round-trip of the
//! [`boruna_orchestrator::audit::storage_s3::S3Bucket`] adapter
//! against it.
//!
//! ## Running locally
//!
//! ```text
//! # Requires Docker daemon running on the host.
//! cargo test -p boruna-orchestrator --features s3-it --test s3_integration -- --nocapture
//! ```
//!
//! ## CI behavior
//!
//! The `s3-it` feature gate keeps these tests OFF by default. They
//! are not in the standard `cargo test --workspace` rotation. The
//! GitHub Actions self-hosted runner (Friday) has Docker available
//! and can opt in via a follow-up workflow if we ever decide to
//! gate releases on a real S3 round-trip; for now the unit tests
//! against `S3Uri::parse`, `object_path`, `ref_to_run_id`, and
//! `classify_os_error` cover the determinism-relevant logic.
//!
//! ## Docker absence
//!
//! Each test self-skips with `eprintln!` + early-return when
//! `testcontainers` cannot reach the Docker daemon, so a developer
//! who enables `s3-it` without Docker installed sees a clear skip
//! message instead of an opaque failure. This deliberately does
//! NOT fail the test — Docker availability is an environment
//! concern, not a regression signal.

#![cfg(feature = "s3-it")]

use std::path::Path;
use std::sync::OnceLock;

use boruna_orchestrator::audit::storage::{BundleStorage, StorageRef};
use boruna_orchestrator::audit::storage_s3::S3BucketBuilder;
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::SyncRunner;
use testcontainers::Container;
use testcontainers_modules::minio::MinIO;

/// MinIO's API port. Mapped to a random host port by Docker.
const MINIO_PORT: u16 = 9000;

/// Default MinIO root credentials when no MINIO_ROOT_USER /
/// MINIO_ROOT_PASSWORD env vars are passed to the container.
const MINIO_USER: &str = "minioadmin";
const MINIO_PASSWORD: &str = "minioadmin";

/// Spin up a MinIO container. Returns None when Docker is
/// unreachable so the test can skip with a clear message.
fn try_start_minio() -> Option<Container<MinIO>> {
    match MinIO::default().start() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!(
                "skipping s3 integration test: could not start MinIO container ({e}); \
                 ensure Docker is running on the host"
            );
            None
        }
    }
}

/// Wire up the AWS_* env vars object_store reads via
/// `AmazonS3Builder::from_env`. Tests run sequentially within this
/// file (separate processes from other test binaries) so the global
/// env mutation is acceptable in this narrow scope. We restore
/// nothing after the test — the process exits.
fn set_aws_env(host: &str, port: u16) {
    let endpoint = format!("http://{host}:{port}");
    // SAFETY: cargo runs each integration-test binary in its own
    // process and these tests are serialized via `INIT`. No other
    // code in this binary reads AWS_* during construction.
    unsafe {
        std::env::set_var("AWS_ENDPOINT_URL", &endpoint);
        std::env::set_var("AWS_ACCESS_KEY_ID", MINIO_USER);
        std::env::set_var("AWS_SECRET_ACCESS_KEY", MINIO_PASSWORD);
        std::env::set_var("AWS_REGION", "us-east-1");
        std::env::set_var("AWS_ALLOW_HTTP", "true");
    }
}

/// Create the test bucket in MinIO using a one-off object_store
/// client. We can't rely on auto-bucket-create in object_store; the
/// bucket must exist before put.
fn create_bucket(host: &str, port: u16, bucket: &str) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let url = format!("http://{host}:{port}/{bucket}");
        let client = reqwest::Client::new();
        let resp = client
            .put(&url)
            .basic_auth(MINIO_USER, Some(MINIO_PASSWORD))
            .send()
            .await?;
        // 200 (created) or 409 (already exists) both fine.
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 409 {
            let body = resp.text().await.unwrap_or_default();
            return Err::<(), Box<dyn std::error::Error>>(
                format!("create bucket failed: {status} {body}").into(),
            );
        }
        Ok(())
    })
}

fn write_bundle(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("manifest.json"), b"{\"v\":1}").unwrap();
    std::fs::create_dir_all(root.join("steps")).unwrap();
    std::fs::write(root.join("steps").join("a.json"), b"step-a").unwrap();
    std::fs::write(root.join("steps").join("b.json"), b"step-b").unwrap();
    std::fs::create_dir_all(root.join("attachments").join("nested")).unwrap();
    std::fs::write(
        root.join("attachments").join("nested").join("c.bin"),
        b"binary-content",
    )
    .unwrap();
}

/// Serialize the tests in this file. testcontainers + the AWS_* env
/// vars + the shared `INIT` lock keep them from racing.
static SERIAL: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[test]
fn s3_put_then_get_roundtrips() {
    let _g = serial_lock();
    let Some(container) = try_start_minio() else {
        return;
    };
    let host = container.get_host().expect("get_host").to_string();
    let port = container
        .get_host_port_ipv4(MINIO_PORT.tcp())
        .expect("get_host_port");

    set_aws_env(&host, port);
    create_bucket(&host, port, "boruna-test").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let bundle_dir = tempfile::tempdir().unwrap();
    write_bundle(bundle_dir.path());

    let store = S3BucketBuilder::new("s3://boruna-test/audit/prod")
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build S3Bucket");

    let r = store.put("run-roundtrip", bundle_dir.path()).expect("put");
    assert_eq!(r.0, "s3://boruna-test/audit/prod/run-roundtrip");

    let resolved = store.get(&r).expect("get");
    assert!(resolved.exists());
    assert!(resolved.join("manifest.json").exists());
    assert_eq!(
        std::fs::read(resolved.join("manifest.json")).unwrap(),
        b"{\"v\":1}"
    );
    assert_eq!(
        std::fs::read(resolved.join("steps").join("a.json")).unwrap(),
        b"step-a"
    );
    assert_eq!(
        std::fs::read(resolved.join("attachments").join("nested").join("c.bin")).unwrap(),
        b"binary-content"
    );
}

#[test]
fn s3_get_returns_not_found_for_missing_run() {
    let _g = serial_lock();
    let Some(container) = try_start_minio() else {
        return;
    };
    let host = container.get_host().expect("get_host").to_string();
    let port = container
        .get_host_port_ipv4(MINIO_PORT.tcp())
        .expect("get_host_port");

    set_aws_env(&host, port);
    create_bucket(&host, port, "boruna-test-missing").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let store = S3BucketBuilder::new("s3://boruna-test-missing")
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build");

    let err = store
        .get(&StorageRef(
            "s3://boruna-test-missing/never-uploaded".into(),
        ))
        .unwrap_err();
    use boruna_orchestrator::audit::storage::StorageError;
    assert!(matches!(err, StorageError::NotFound(_)), "got {err:?}");
}

#[test]
fn s3_list_returns_uploaded_run_ids_in_sorted_order() {
    let _g = serial_lock();
    let Some(container) = try_start_minio() else {
        return;
    };
    let host = container.get_host().expect("get_host").to_string();
    let port = container
        .get_host_port_ipv4(MINIO_PORT.tcp())
        .expect("get_host_port");

    set_aws_env(&host, port);
    create_bucket(&host, port, "boruna-test-list").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let store = S3BucketBuilder::new("s3://boruna-test-list/audit")
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build");

    let bundle = tempfile::tempdir().unwrap();
    write_bundle(bundle.path());

    for run_id in ["run-c", "run-a", "run-b"] {
        store.put(run_id, bundle.path()).expect("put");
    }

    let refs: Vec<String> = store
        .list(None)
        .expect("list")
        .into_iter()
        .map(|r| r.0)
        .collect();
    assert_eq!(
        refs,
        vec![
            "s3://boruna-test-list/audit/run-a".to_string(),
            "s3://boruna-test-list/audit/run-b".to_string(),
            "s3://boruna-test-list/audit/run-c".to_string(),
        ]
    );

    let filtered: Vec<String> = store
        .list(Some("run-a"))
        .expect("list filtered")
        .into_iter()
        .map(|r| r.0)
        .collect();
    assert_eq!(
        filtered,
        vec!["s3://boruna-test-list/audit/run-a".to_string()]
    );
}
