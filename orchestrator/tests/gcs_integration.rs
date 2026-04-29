//! GCS BundleStorage integration tests (post1-T-3.2).
//!
//! Spins up `fsouza/fake-gcs-server` via testcontainers and
//! exercises the full `put → list → get` round-trip of the
//! [`boruna_orchestrator::audit::storage_gcs::GcsBucket`] adapter
//! against it.
//!
//! testcontainers-modules has no GCS module (it covers only the
//! Bigtable/Datastore/Firestore/PubSub/Spanner emulators in the
//! `google-cloud-sdk` image). We define a tiny custom [`Image`]
//! implementation for the well-known `fsouza/fake-gcs-server`
//! container right here.
//!
//! ## Running locally
//!
//! ```text
//! # Requires Docker daemon running on the host.
//! cargo test -p boruna-orchestrator --features gcs-it --test gcs_integration -- --nocapture
//! ```
//!
//! ## CI behavior
//!
//! The `gcs-it` feature gate keeps these tests OFF by default.
//! Each test self-skips with `eprintln!` + early-return when
//! testcontainers cannot reach the Docker daemon, so a developer
//! who enables `gcs-it` without Docker installed sees a clear skip
//! message instead of an opaque failure. Matches the s3-it pattern.

#![cfg(feature = "gcs-it")]

use std::borrow::Cow;
use std::path::Path;
use std::sync::OnceLock;

use boruna_orchestrator::audit::storage::{BundleStorage, StorageRef};
use boruna_orchestrator::audit::storage_gcs::GcsBucketBuilder;
use testcontainers::core::{ContainerPort, IntoContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{Container, Image};

/// fake-gcs-server's HTTP API port (configurable via the `-port`
/// flag on the binary; we let it use the default and map it).
const FAKE_GCS_PORT: u16 = 4443;

/// Custom [`Image`] for `fsouza/fake-gcs-server`. Configured to
/// serve over plain HTTP on FAKE_GCS_PORT. The `-public-host` /
/// `-external-url` flags tell the server to advertise the
/// host:port object_store will hit, which is set per-test once the
/// host port is known.
#[derive(Debug, Clone, Default)]
struct FakeGcsServer {
    /// External URL the server should advertise, e.g.
    /// `http://localhost:32787`. Only relevant when the `Location`
    /// headers in upload responses need to round-trip back to the
    /// client; object_store doesn't depend on these for the
    /// operations we exercise (put/list/get with explicit paths).
    external_url: Option<String>,
}

impl Image for FakeGcsServer {
    fn name(&self) -> &str {
        "fsouza/fake-gcs-server"
    }

    fn tag(&self) -> &str {
        // Pinned for reproducibility. Bump consciously; the API is
        // stable but new flags are added over time.
        "1.49.2"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        // The server logs `server started at <addr>` once it is
        // listening. Wait for that line on stderr (fake-gcs-server
        // logs to stderr by default).
        vec![WaitFor::message_on_stderr("server started at")]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        std::iter::empty::<(String, String)>()
    }

    fn cmd(&self) -> impl IntoIterator<Item = impl Into<Cow<'_, str>>> {
        let mut args: Vec<String> = vec![
            "-scheme".to_string(),
            "http".to_string(),
            "-host".to_string(),
            "0.0.0.0".to_string(),
            "-port".to_string(),
            FAKE_GCS_PORT.to_string(),
        ];
        if let Some(ref u) = self.external_url {
            args.push("-public-host".to_string());
            args.push(u.clone());
        }
        args.into_iter()
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(FAKE_GCS_PORT)]
    }
}

/// Spin up a fake-gcs-server container. Returns None when Docker
/// is unreachable so the test can skip with a clear message.
fn try_start() -> Option<Container<FakeGcsServer>> {
    match FakeGcsServer::default().start() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!(
                "skipping gcs integration test: could not start fake-gcs-server container ({e}); \
                 ensure Docker is running on the host"
            );
            None
        }
    }
}

/// Create a bucket via fake-gcs-server's POST /storage/v1/b API.
/// fake-gcs-server doesn't need a project, but the GCS API does;
/// we pass a placeholder.
fn create_bucket(host: &str, port: u16, bucket: &str) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let url = format!("http://{host}:{port}/storage/v1/b?project=test-project");
        let client = reqwest::Client::new();
        let body = serde_json::json!({ "name": bucket });
        let resp = client.post(&url).json(&body).send().await?;
        let status = resp.status();
        // 200 (created) or 409 (already exists) both fine.
        if !status.is_success() && status.as_u16() != 409 {
            let text = resp.text().await.unwrap_or_default();
            return Err::<(), Box<dyn std::error::Error>>(
                format!("create bucket failed: {status} {text}").into(),
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

/// Serialize the tests in this file. testcontainers + the in-test
/// reqwest client + the shared lock keep them from racing on the
/// shared runtime.
static SERIAL: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Build the `with_endpoint` URL the GCS adapter passes to
/// object_store. fake-gcs-server's storage API is rooted at
/// `/storage/v1/`; object_store's `with_url` expects the bucket
/// root, but for the gcp builder we want the API root.
fn endpoint_for(host: &str, port: u16) -> String {
    format!("http://{host}:{port}")
}

#[test]
fn gcs_put_then_get_roundtrips() {
    let _g = serial_lock();
    let Some(container) = try_start() else {
        return;
    };
    let host = container.get_host().expect("get_host").to_string();
    let port = container
        .get_host_port_ipv4(FAKE_GCS_PORT.tcp())
        .expect("get_host_port");

    create_bucket(&host, port, "boruna-test").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let bundle_dir = tempfile::tempdir().unwrap();
    write_bundle(bundle_dir.path());

    let store = GcsBucketBuilder::new("gs://boruna-test/audit/prod")
        .with_endpoint(endpoint_for(&host, port))
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build GcsBucket");

    let r = store.put("run-roundtrip", bundle_dir.path()).expect("put");
    assert_eq!(r.0, "gs://boruna-test/audit/prod/run-roundtrip");

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
fn gcs_get_returns_not_found_for_missing_run() {
    let _g = serial_lock();
    let Some(container) = try_start() else {
        return;
    };
    let host = container.get_host().expect("get_host").to_string();
    let port = container
        .get_host_port_ipv4(FAKE_GCS_PORT.tcp())
        .expect("get_host_port");

    create_bucket(&host, port, "boruna-test-missing").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let store = GcsBucketBuilder::new("gs://boruna-test-missing")
        .with_endpoint(endpoint_for(&host, port))
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build");

    let err = store
        .get(&StorageRef(
            "gs://boruna-test-missing/never-uploaded".into(),
        ))
        .unwrap_err();
    use boruna_orchestrator::audit::storage::StorageError;
    assert!(matches!(err, StorageError::NotFound(_)), "got {err:?}");
}

#[test]
fn gcs_list_returns_uploaded_run_ids_in_sorted_order() {
    let _g = serial_lock();
    let Some(container) = try_start() else {
        return;
    };
    let host = container.get_host().expect("get_host").to_string();
    let port = container
        .get_host_port_ipv4(FAKE_GCS_PORT.tcp())
        .expect("get_host_port");

    create_bucket(&host, port, "boruna-test-list").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let store = GcsBucketBuilder::new("gs://boruna-test-list/audit")
        .with_endpoint(endpoint_for(&host, port))
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
            "gs://boruna-test-list/audit/run-a".to_string(),
            "gs://boruna-test-list/audit/run-b".to_string(),
            "gs://boruna-test-list/audit/run-c".to_string(),
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
        vec!["gs://boruna-test-list/audit/run-a".to_string()]
    );
}
