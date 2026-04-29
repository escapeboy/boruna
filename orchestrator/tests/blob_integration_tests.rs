// Tests require the Docker services from docker-compose.blob-tests.yml.
//
// Start:   docker compose -f docker-compose.blob-tests.yml up -d
// Run:     cargo test -p boruna-orchestrator --features blob-integration -- --ignored blob_
// Stop:    docker compose -f docker-compose.blob-tests.yml down
//
// Each test reads its endpoint from an environment variable and returns
// early (skips) when the variable is not set. This keeps them safe to
// compile and run in environments without Docker.
//
// Feature flags required:
//   blob-integration = ["s3", "gcs", "azure", "dep:reqwest", "dep:tokio"]
//
// Environment variables:
//   S3  (MinIO):   MINIO_ENDPOINT=http://localhost:9000
//                  MINIO_ACCESS_KEY=minioadmin
//                  MINIO_SECRET_KEY=minioadmin
//   GCS (fake-gcs-server):
//                  FAKE_GCS_ENDPOINT=http://localhost:4443
//   Azure (Azurite):
//                  AZURITE_ENDPOINT=http://localhost:10000/devstoreaccount1

#![cfg(feature = "blob-integration")]

use std::path::Path;

use boruna_orchestrator::audit::storage::BundleStorage;

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

// ────────────────────────────────────────────────────────────
// S3 (MinIO)
// ────────────────────────────────────────────────────────────

/// Create the named bucket in the MinIO instance at `endpoint`.
fn minio_create_bucket(
    endpoint: &str,
    user: &str,
    password: &str,
    bucket: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let url = format!("{endpoint}/{bucket}");
        let resp = reqwest::Client::new()
            .put(&url)
            .basic_auth(user, Some(password))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 409 {
            let body = resp.text().await.unwrap_or_default();
            return Err::<(), Box<dyn std::error::Error>>(
                format!("MinIO create bucket failed: {status} {body}").into(),
            );
        }
        Ok(())
    })
}

#[test]
#[ignore]
fn blob_s3_put_get_roundtrip() {
    use boruna_orchestrator::audit::storage_s3::S3BucketBuilder;

    let endpoint = match std::env::var("MINIO_ENDPOINT") {
        Ok(v) => v,
        Err(_) => return,
    };
    let user = std::env::var("MINIO_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".into());
    let password = std::env::var("MINIO_SECRET_KEY").unwrap_or_else(|_| "minioadmin".into());

    unsafe {
        std::env::set_var("AWS_ENDPOINT_URL", &endpoint);
        std::env::set_var("AWS_ACCESS_KEY_ID", &user);
        std::env::set_var("AWS_SECRET_ACCESS_KEY", &password);
        std::env::set_var("AWS_REGION", "us-east-1");
        std::env::set_var("AWS_ALLOW_HTTP", "true");
    }
    minio_create_bucket(&endpoint, &user, &password, "blob-it-s3").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    write_bundle(bundle.path());

    let store = S3BucketBuilder::new("s3://blob-it-s3/test")
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build S3Bucket");

    let r = store.put("run-1", bundle.path()).expect("put");
    assert_eq!(r.0, "s3://blob-it-s3/test/run-1");

    let resolved = store.get(&r).expect("get");
    assert!(resolved.join("manifest.json").exists());
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
#[ignore]
fn blob_s3_list_sorted() {
    use boruna_orchestrator::audit::storage_s3::S3BucketBuilder;

    let endpoint = match std::env::var("MINIO_ENDPOINT") {
        Ok(v) => v,
        Err(_) => return,
    };
    let user = std::env::var("MINIO_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".into());
    let password = std::env::var("MINIO_SECRET_KEY").unwrap_or_else(|_| "minioadmin".into());

    unsafe {
        std::env::set_var("AWS_ENDPOINT_URL", &endpoint);
        std::env::set_var("AWS_ACCESS_KEY_ID", &user);
        std::env::set_var("AWS_SECRET_ACCESS_KEY", &password);
        std::env::set_var("AWS_REGION", "us-east-1");
        std::env::set_var("AWS_ALLOW_HTTP", "true");
    }
    minio_create_bucket(&endpoint, &user, &password, "blob-it-s3-list").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    write_bundle(bundle.path());

    let store = S3BucketBuilder::new("s3://blob-it-s3-list/audit")
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build");

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
            "s3://blob-it-s3-list/audit/run-a",
            "s3://blob-it-s3-list/audit/run-b",
            "s3://blob-it-s3-list/audit/run-c",
        ]
    );
}

// ────────────────────────────────────────────────────────────
// GCS (fake-gcs-server)
// ────────────────────────────────────────────────────────────

fn fake_gcs_create_bucket(
    endpoint: &str,
    bucket: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let url = format!("{endpoint}/storage/v1/b?project=test-project");
        let body = serde_json::json!({ "name": bucket });
        let resp = reqwest::Client::new().post(&url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 409 {
            let text = resp.text().await.unwrap_or_default();
            return Err::<(), Box<dyn std::error::Error>>(
                format!("fake-gcs create bucket failed: {status} {text}").into(),
            );
        }
        Ok(())
    })
}

#[test]
#[ignore]
fn blob_gcs_put_get_roundtrip() {
    use boruna_orchestrator::audit::storage_gcs::GcsBucketBuilder;

    let endpoint = match std::env::var("FAKE_GCS_ENDPOINT") {
        Ok(v) => v,
        Err(_) => return,
    };

    fake_gcs_create_bucket(&endpoint, "blob-it-gcs").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    write_bundle(bundle.path());

    let store = GcsBucketBuilder::new("gs://blob-it-gcs/test")
        .with_endpoint(endpoint)
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build GcsBucket");

    let r = store.put("run-1", bundle.path()).expect("put");
    assert_eq!(r.0, "gs://blob-it-gcs/test/run-1");

    let resolved = store.get(&r).expect("get");
    assert!(resolved.join("manifest.json").exists());
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
#[ignore]
fn blob_gcs_list_sorted() {
    use boruna_orchestrator::audit::storage_gcs::GcsBucketBuilder;

    let endpoint = match std::env::var("FAKE_GCS_ENDPOINT") {
        Ok(v) => v,
        Err(_) => return,
    };

    fake_gcs_create_bucket(&endpoint, "blob-it-gcs-list").expect("create bucket");

    let cache = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    write_bundle(bundle.path());

    let store = GcsBucketBuilder::new("gs://blob-it-gcs-list/audit")
        .with_endpoint(endpoint)
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build");

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
            "gs://blob-it-gcs-list/audit/run-a",
            "gs://blob-it-gcs-list/audit/run-b",
            "gs://blob-it-gcs-list/audit/run-c",
        ]
    );
}

// ────────────────────────────────────────────────────────────
// Azure (Azurite)
// ────────────────────────────────────────────────────────────
//
// Azurite's well-known devstoreaccount1 credentials are used.
// Container creation uses the Azurite REST API with the well-known
// SharedKey credentials (account: devstoreaccount1).
// AzureBlobBucketBuilder::with_use_emulator(true) wires the
// standard Azurite endpoint + key automatically.

/// Create a blob container in Azurite.
///
/// Azurite is started with `--loose` in docker-compose.blob-tests.yml
/// which disables request-signature validation. This lets us create
/// containers with a minimal unsigned PUT, avoiding the need to
/// implement full SharedKey HMAC signing in the test.
fn azurite_create_container(
    endpoint: &str,
    container: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let url = format!("{endpoint}/devstoreaccount1/{container}?restype=container");
        let resp = reqwest::Client::new()
            .put(&url)
            .header("x-ms-version", "2020-12-06")
            .header("x-ms-date", "Thu, 01 Jan 2099 00:00:00 GMT")
            .header("Authorization", "SharedKeyLite devstoreaccount1:placeholder")
            .send()
            .await?;
        let status = resp.status();
        // 201 Created or 409 ContainerAlreadyExists are both fine.
        if !status.is_success() && status.as_u16() != 409 {
            let body = resp.text().await.unwrap_or_default();
            return Err::<(), Box<dyn std::error::Error>>(
                format!("Azurite create container failed: {status} {body}").into(),
            );
        }
        Ok(())
    })
}

#[test]
#[ignore]
fn blob_azure_put_get_roundtrip() {
    use boruna_orchestrator::audit::storage_azure::AzureBlobBucketBuilder;

    let endpoint = match std::env::var("AZURITE_ENDPOINT") {
        Ok(v) => v,
        Err(_) => return,
    };

    azurite_create_container(&endpoint, "blob-it-az").expect("create container");

    let cache = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    write_bundle(bundle.path());

    let store = AzureBlobBucketBuilder::new("azblob://devstoreaccount1/blob-it-az/test")
        .with_use_emulator(true)
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build AzureBlobBucket");

    let r = store.put("run-1", bundle.path()).expect("put");
    assert_eq!(
        r.0,
        "azblob://devstoreaccount1/blob-it-az/test/run-1"
    );

    let resolved = store.get(&r).expect("get");
    assert!(resolved.join("manifest.json").exists());
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
#[ignore]
fn blob_azure_list_sorted() {
    use boruna_orchestrator::audit::storage_azure::AzureBlobBucketBuilder;

    let endpoint = match std::env::var("AZURITE_ENDPOINT") {
        Ok(v) => v,
        Err(_) => return,
    };

    azurite_create_container(&endpoint, "blob-it-az-list").expect("create container");

    let cache = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    write_bundle(bundle.path());

    let store = AzureBlobBucketBuilder::new("azblob://devstoreaccount1/blob-it-az-list/audit")
        .with_use_emulator(true)
        .with_cache_root(cache.path().to_path_buf())
        .build()
        .expect("build");

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
            "azblob://devstoreaccount1/blob-it-az-list/audit/run-a",
            "azblob://devstoreaccount1/blob-it-az-list/audit/run-b",
            "azblob://devstoreaccount1/blob-it-az-list/audit/run-c",
        ]
    );
}
