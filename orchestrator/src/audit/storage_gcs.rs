//! GCS [`BundleStorage`] adapter (post1-T-3.2).
//!
//! Backed by the Apache Arrow `object_store` crate's `gcp` feature.
//! Mirrors the [`crate::audit::storage_s3`] adapter shipped in
//! T-3.1 — same trait, same builder pattern, same error_kind
//! taxonomy shape (`gcs.*` instead of `s3.*`). The two files are
//! intentionally NOT abstracted into a generic
//! `storage_object_store.rs` module yet: each adapter has its own
//! auth quirks, endpoint conventions, and error mapping, and the
//! YAGNI bar is "wait for T-3.3 (Azure) before deciding whether
//! the abstraction is worth the indirection."
//!
//! ## URI shape
//!
//! `gs://<bucket>[/<prefix>]`
//!
//! - `gs://my-bucket` — bucket with no key prefix; objects land at
//!   `<run-id>/<file>`.
//! - `gs://my-bucket/audit/prod` — objects land at
//!   `audit/prod/<run-id>/<file>`. Trailing slashes are normalized
//!   away.
//!
//! The `StorageRef` returned by [`put`] echoes the construction URI
//! suffixed with the run id, e.g. `gs://my-bucket/audit/prod/<run-id>`.
//!
//! ## Configuration
//!
//! Authentication is sourced from
//! [`object_store::gcp::GoogleCloudStorageBuilder::from_env`], which
//! recognizes:
//!
//! - `GOOGLE_SERVICE_ACCOUNT` / `GOOGLE_SERVICE_ACCOUNT_PATH` — path
//!   to a JSON service-account key file.
//! - `GOOGLE_SERVICE_ACCOUNT_KEY` — the JSON service-account key
//!   inline (handy for K8s secrets).
//! - `GOOGLE_BUCKET` / `GOOGLE_BUCKET_NAME` — explicit bucket
//!   override (we set this from the URI; env-var override would
//!   mismatch the parsed prefix and produce confusing errors).
//!
//! For Application Default Credentials (Workload Identity, gcloud
//! login), point `GOOGLE_APPLICATION_CREDENTIALS` at the JSON file
//! the SDK installs.
//!
//! ### Pointing at fake-gcs-server / a custom endpoint
//!
//! Set `GOOGLE_STORAGE_EMULATOR_ENDPOINT=http://host:port` in the
//! process environment OR pass the endpoint to
//! [`GcsBucketBuilder::with_endpoint`]. The integration tests use
//! the builder hook so they don't pollute the global env.
//!
//! ## Sync trait, async SDK
//!
//! Same approach as S3: a per-instance current-thread tokio runtime
//! owned by the adapter bridges sync calls onto the async SDK. See
//! the storage_s3.rs module docs for the rationale.
//!
//! ## Local cache
//!
//! Same as S3: `get` materializes under [`BORUNA_BUNDLE_CACHE`]
//! (default `<temp>/boruna-bundle-cache`). Idempotent (clear +
//! re-fetch on each `get`).
//!
//! ## Determinism contract
//!
//! - Object listings are sorted before iteration so [`list`] returns
//!   `Vec<StorageRef>` in stable order across runs.
//! - Upload is best-effort idempotent: re-`put`-ing the same `run_id`
//!   overwrites the existing objects (which should be byte-identical
//!   given the determinism contract on bundle finalize).
//! - `put` failures are logged at the call site; the local bundle
//!   remains the authoritative record (see `storage.rs` doc).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use object_store::gcp::{GoogleCloudStorage, GoogleCloudStorageBuilder};
use object_store::path::Path as OsPath;
use object_store::{Error as OsError, ObjectStore, PutPayload};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

use super::storage::{BundleStorage, StorageError, StorageRef};

/// Default cache root for materialized bundles. Operators override
/// via `BORUNA_BUNDLE_CACHE`. Shared with the S3 adapter — both
/// adapters write into the same root keyed by `run_id`. Operators
/// who multi-cloud and care about cross-bucket collisions should
/// set per-bucket cache dirs themselves.
const CACHE_ENV_VAR: &str = "BORUNA_BUNDLE_CACHE";
const CACHE_DIR_NAME: &str = "boruna-bundle-cache";

/// Parsed `gs://<bucket>[/<prefix>]` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GsUri {
    bucket: String,
    /// Key prefix WITHOUT trailing slash. Empty string means
    /// objects live at the bucket root.
    prefix: String,
}

impl GsUri {
    fn parse(uri: &str) -> Result<Self, StorageError> {
        let rest = uri
            .strip_prefix("gs://")
            .ok_or_else(|| StorageError::InvalidUri(uri.to_string()))?;
        if rest.is_empty() {
            return Err(StorageError::InvalidUri(format!(
                "gs URI missing bucket name: {uri}"
            )));
        }
        let (bucket, prefix) = match rest.split_once('/') {
            Some((b, p)) => (b, p.trim_end_matches('/')),
            None => (rest, ""),
        };
        if bucket.is_empty() {
            return Err(StorageError::InvalidUri(format!(
                "gs URI missing bucket name: {uri}"
            )));
        }
        // GCS bucket-name rules are looser than we need to enforce
        // here; we just reject the obvious invalids that would let a
        // typo through (whitespace, scheme delimiters).
        if bucket.chars().any(|c| c.is_whitespace() || c == ':') {
            return Err(StorageError::InvalidUri(format!(
                "gs URI has invalid bucket name: {uri}"
            )));
        }
        Ok(GsUri {
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
        })
    }

    /// Object-store path for an object whose run-relative key is
    /// `key`. Joins prefix + run_id + key with `/` separators.
    fn object_path(&self, run_id: &str, key: &str) -> OsPath {
        let mut parts = Vec::with_capacity(3);
        if !self.prefix.is_empty() {
            parts.push(self.prefix.as_str());
        }
        parts.push(run_id);
        if !key.is_empty() {
            parts.push(key);
        }
        OsPath::from(parts.join("/"))
    }

    /// `gs://<bucket>/<prefix>` — used as the reference suffix for
    /// listings and as the prefix `put` echoes back with `/<run_id>`
    /// appended.
    fn root_uri(&self) -> String {
        if self.prefix.is_empty() {
            format!("gs://{}", self.bucket)
        } else {
            format!("gs://{}/{}", self.bucket, self.prefix)
        }
    }
}

/// GCS-backed [`BundleStorage`] implementation.
pub struct GcsBucket {
    uri: GsUri,
    store: Arc<GoogleCloudStorage>,
    runtime: Arc<Runtime>,
    cache_root: PathBuf,
}

impl GcsBucket {
    /// Build a [`GcsBucket`] from a `gs://bucket[/prefix]` URI.
    /// Auth comes from `GOOGLE_*` environment variables; see the
    /// module docs. Equivalent to `GcsBucketBuilder::new(uri).build()`.
    pub fn from_uri(uri: &str) -> Result<Self, StorageError> {
        GcsBucketBuilder::new(uri).build()
    }

    /// Run an async block on this adapter's runtime.
    fn block_on<F, T>(&self, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime.block_on(f)
    }
}

/// Builder for [`GcsBucket`] that lets callers (mostly tests)
/// override the local materialization cache root and the GCS
/// endpoint (for fake-gcs-server / private-network deployments).
///
/// Production callers use [`GcsBucket::from_uri`] which delegates to
/// `GcsBucketBuilder::new(uri).build()`. The cache root then comes from
/// `BORUNA_BUNDLE_CACHE` or `<temp>/boruna-bundle-cache`.
pub struct GcsBucketBuilder {
    uri: String,
    cache_root: Option<PathBuf>,
    endpoint: Option<String>,
}

impl GcsBucketBuilder {
    /// Start a builder against the given `gs://bucket[/prefix]` URI.
    pub fn new(uri: impl Into<String>) -> Self {
        GcsBucketBuilder {
            uri: uri.into(),
            cache_root: None,
            endpoint: None,
        }
    }

    /// Override the cache root used to materialize bundles in
    /// [`GcsBucket::get`]. Tests use this to point at a `tempdir()`
    /// so each test sees a clean cache.
    pub fn with_cache_root(mut self, root: PathBuf) -> Self {
        self.cache_root = Some(root);
        self
    }

    /// Override the GCS endpoint (e.g.
    /// `http://localhost:4443/storage/v1/` for fake-gcs-server).
    /// Production callers leave this unset to talk to GCS proper.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Build the [`GcsBucket`].
    pub fn build(self) -> Result<GcsBucket, StorageError> {
        let parsed = GsUri::parse(&self.uri)?;
        let cache_root = self.cache_root.unwrap_or_else(default_cache_root);
        let sb = if let Some(endpoint) = self.endpoint {
            // Emulator mode (e.g. fake-gcs-server): inject a service-account
            // JSON that sets `gcs_base_url` to the emulator HTTP URL and
            // disables OAuth so no real GCP credentials are required.
            // `with_url` rejects non-gs:// schemes; the service-account
            // JSON path is the object_store-documented way to override the
            // base URL for local emulators (see builder.rs docs).
            let sa_json = serde_json::json!({
                "gcs_base_url": endpoint,
                "disable_oauth": true,
                "client_email": "",
                "private_key": "",
                "private_key_id": ""
            })
            .to_string();
            GoogleCloudStorageBuilder::new()
                .with_bucket_name(&parsed.bucket)
                .with_service_account_key(sa_json)
        } else {
            GoogleCloudStorageBuilder::from_env().with_bucket_name(&parsed.bucket)
        };
        let store = sb.build().map_err(|e| classify_os_error(e, "construct"))?;
        let runtime = RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| StorageError::Backend {
                kind: "gcs.runtime",
                msg: format!("failed to build tokio runtime: {e}"),
            })?;
        Ok(GcsBucket {
            uri: parsed,
            store: Arc::new(store),
            runtime: Arc::new(runtime),
            cache_root,
        })
    }
}

impl BundleStorage for GcsBucket {
    fn put(&self, run_id: &str, bundle_dir: &Path) -> Result<StorageRef, StorageError> {
        if run_id.is_empty() {
            return Err(StorageError::InvalidUri("empty run_id".into()));
        }
        if !bundle_dir.is_dir() {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("bundle dir not found: {}", bundle_dir.display()),
            )));
        }
        let files = walk_files(bundle_dir)?;
        let mut sorted = files;
        sorted.sort();
        for relative in &sorted {
            let abs = bundle_dir.join(relative);
            let bytes = std::fs::read(&abs)?;
            let key = relative.to_string_lossy().replace('\\', "/");
            let target = self.uri.object_path(run_id, &key);
            self.block_on(async {
                self.store
                    .put(&target, PutPayload::from(bytes))
                    .await
                    .map(|_| ())
                    .map_err(|e| classify_os_error(e, "put"))
            })?;
        }
        Ok(StorageRef(format!("{}/{}", self.uri.root_uri(), run_id)))
    }

    fn get(&self, r: &StorageRef) -> Result<PathBuf, StorageError> {
        let run_id = ref_to_run_id(&self.uri, r)?;
        let cache_dir = self.cache_root.join(&run_id);
        if cache_dir.exists() {
            std::fs::remove_dir_all(&cache_dir)?;
        }
        std::fs::create_dir_all(&cache_dir)?;

        let prefix = self.uri.object_path(&run_id, "");
        let entries = self.block_on(async {
            use futures::StreamExt;
            let mut stream = self.store.list(Some(&prefix));
            let mut out = Vec::new();
            while let Some(meta) = stream.next().await {
                let meta = meta.map_err(|e| classify_os_error(e, "list"))?;
                out.push(meta.location);
            }
            Ok::<_, StorageError>(out)
        })?;

        if entries.is_empty() {
            return Err(StorageError::NotFound(r.0.clone()));
        }

        let strip = {
            let mut s = prefix.to_string();
            if !s.ends_with('/') {
                s.push('/');
            }
            s
        };

        for location in entries {
            let s = location.to_string();
            let rel = s
                .strip_prefix(&strip)
                .ok_or_else(|| StorageError::Backend {
                    kind: "gcs.unexpected_key",
                    msg: format!("object outside expected prefix: {s}"),
                })?;
            let dest = cache_dir.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes = self.block_on(async {
                self.store
                    .get(&location)
                    .await
                    .map_err(|e| classify_os_error(e, "get"))?
                    .bytes()
                    .await
                    .map_err(|e| classify_os_error(e, "get"))
            })?;
            std::fs::write(&dest, &bytes)?;
        }

        Ok(cache_dir)
    }

    fn list(&self, prefix_filter: Option<&str>) -> Result<Vec<StorageRef>, StorageError> {
        let scan_prefix = if self.uri.prefix.is_empty() {
            None
        } else {
            Some(OsPath::from(self.uri.prefix.as_str()))
        };
        let result = self.block_on(async {
            self.store
                .list_with_delimiter(scan_prefix.as_ref())
                .await
                .map_err(|e| classify_os_error(e, "list"))
        })?;

        let strip = if self.uri.prefix.is_empty() {
            String::new()
        } else {
            let mut s = self.uri.prefix.clone();
            if !s.ends_with('/') {
                s.push('/');
            }
            s
        };

        let mut refs = Vec::new();
        for common in result.common_prefixes {
            let s = common.to_string();
            let id = if strip.is_empty() {
                s.trim_end_matches('/').to_string()
            } else {
                match s.strip_prefix(&strip) {
                    Some(r) => r.trim_end_matches('/').to_string(),
                    None => continue,
                }
            };
            if id.is_empty() {
                continue;
            }
            if let Some(p) = prefix_filter {
                if !id.starts_with(p) {
                    continue;
                }
            }
            refs.push(StorageRef(format!("{}/{}", self.uri.root_uri(), id)));
        }
        refs.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(refs)
    }
}

/// Recursively collect file paths under `root`, returning paths
/// relative to `root`. Skips symlinks (bundles are pure files+dirs,
/// matching `LocalFs` behavior).
fn walk_files(root: &Path) -> Result<Vec<PathBuf>, StorageError> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), StorageError> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk(root, &path, out)?;
            } else if ft.is_file() {
                let rel = path.strip_prefix(root).map_err(|_| {
                    StorageError::Io(std::io::Error::other(format!(
                        "walk: path {} not under root",
                        path.display()
                    )))
                })?;
                out.push(rel.to_path_buf());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    Ok(out)
}

/// Extract the run id from a [`StorageRef`] that this adapter
/// previously emitted. Validates that the bucket+prefix match the
/// adapter's configured root so an operator-provided ref aimed at
/// the wrong bucket/prefix surfaces clearly instead of silently
/// returning NotFound.
fn ref_to_run_id(uri: &GsUri, r: &StorageRef) -> Result<String, StorageError> {
    let root = uri.root_uri();
    let suffix = r
        .0
        .strip_prefix(&root)
        .and_then(|s| s.strip_prefix('/'))
        .ok_or_else(|| {
            StorageError::InvalidUri(format!("ref {} does not match adapter root {}", r.0, root))
        })?;
    if suffix.is_empty() || suffix.contains('/') {
        return Err(StorageError::InvalidUri(format!(
            "ref {} is not a single run-id under {}",
            r.0, root
        )));
    }
    Ok(suffix.to_string())
}

/// Map an `object_store` error onto a stable [`StorageError`]
/// variant. Same shape as the S3 adapter's `classify_os_error`,
/// but emits `gcs.*` kinds so integrators can distinguish.
fn classify_os_error(e: OsError, op: &'static str) -> StorageError {
    match &e {
        OsError::NotFound { .. } => StorageError::NotFound(format!("{op}: {e}")),
        OsError::PermissionDenied { .. } | OsError::Unauthenticated { .. } => {
            StorageError::Backend {
                kind: "gcs.permanent",
                msg: format!("{op}: {e}"),
            }
        }
        _ => StorageError::Backend {
            kind: "gcs.transient",
            msg: format!("{op}: {e}"),
        },
    }
}

fn default_cache_root() -> PathBuf {
    resolve_cache_root(std::env::var(CACHE_ENV_VAR).ok(), std::env::temp_dir())
}

/// Pure helper: pick the cache root given a raw env-var value and a
/// fallback temp-dir. Extracted from [`default_cache_root`] so the
/// resolution logic can be tested without touching process-wide
/// environment (which would race with cargo's parallel test runner).
fn resolve_cache_root(env_value: Option<String>, temp_dir: PathBuf) -> PathBuf {
    match env_value {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => temp_dir.join(CACHE_DIR_NAME),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gs_uri_parse_bucket_only() {
        let u = GsUri::parse("gs://my-bucket").unwrap();
        assert_eq!(u.bucket, "my-bucket");
        assert_eq!(u.prefix, "");
        assert_eq!(u.root_uri(), "gs://my-bucket");
    }

    #[test]
    fn gs_uri_parse_bucket_and_prefix() {
        let u = GsUri::parse("gs://b/audit/prod").unwrap();
        assert_eq!(u.bucket, "b");
        assert_eq!(u.prefix, "audit/prod");
        assert_eq!(u.root_uri(), "gs://b/audit/prod");
    }

    #[test]
    fn gs_uri_parse_strips_trailing_slash_on_prefix() {
        let u = GsUri::parse("gs://b/audit/prod/").unwrap();
        assert_eq!(u.prefix, "audit/prod");
    }

    #[test]
    fn gs_uri_parse_rejects_missing_bucket() {
        for bad in ["gs://", "gs:///prefix"] {
            assert!(matches!(
                GsUri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn gs_uri_parse_rejects_wrong_scheme() {
        for bad in ["http://b/p", "local:foo", "s3://b/p", "gs:bucket/p"] {
            assert!(matches!(
                GsUri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn gs_uri_parse_rejects_invalid_bucket_chars() {
        for bad in ["gs:// bad", "gs://bad bucket/p"] {
            assert!(matches!(
                GsUri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn object_path_concatenates_prefix_run_id_key() {
        let u = GsUri::parse("gs://b/audit/prod").unwrap();
        let p = u.object_path("run-7", "steps/a.json");
        assert_eq!(p.to_string(), "audit/prod/run-7/steps/a.json");
    }

    #[test]
    fn object_path_skips_empty_prefix() {
        let u = GsUri::parse("gs://b").unwrap();
        let p = u.object_path("run-7", "manifest.json");
        assert_eq!(p.to_string(), "run-7/manifest.json");
    }

    #[test]
    fn object_path_skips_empty_key() {
        let u = GsUri::parse("gs://b/audit").unwrap();
        let p = u.object_path("run-7", "");
        assert_eq!(p.to_string(), "audit/run-7");
    }

    #[test]
    fn ref_to_run_id_extracts_suffix() {
        let u = GsUri::parse("gs://b/audit").unwrap();
        let id = ref_to_run_id(&u, &StorageRef("gs://b/audit/run-7".into())).unwrap();
        assert_eq!(id, "run-7");
    }

    #[test]
    fn ref_to_run_id_rejects_mismatched_root() {
        let u = GsUri::parse("gs://b/audit").unwrap();
        let err = ref_to_run_id(&u, &StorageRef("gs://other/audit/run-7".into())).unwrap_err();
        assert!(matches!(err, StorageError::InvalidUri(_)));
    }

    #[test]
    fn ref_to_run_id_rejects_nested_path() {
        let u = GsUri::parse("gs://b/audit").unwrap();
        let err = ref_to_run_id(&u, &StorageRef("gs://b/audit/run-7/extra".into())).unwrap_err();
        assert!(matches!(err, StorageError::InvalidUri(_)));
    }

    #[test]
    fn ref_to_run_id_rejects_empty_suffix() {
        let u = GsUri::parse("gs://b/audit").unwrap();
        let err = ref_to_run_id(&u, &StorageRef("gs://b/audit/".into())).unwrap_err();
        assert!(matches!(err, StorageError::InvalidUri(_)));
    }

    #[test]
    fn walk_files_collects_recursively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("b.txt"), b"b").unwrap();
        let mut files = walk_files(dir.path()).unwrap();
        files.sort();
        assert_eq!(
            files,
            vec![PathBuf::from("a.txt"), PathBuf::from("sub").join("b.txt")]
        );
    }

    #[test]
    fn classify_os_error_maps_known_kinds() {
        let nf = classify_os_error(
            OsError::NotFound {
                path: "x".into(),
                source: "missing".into(),
            },
            "get",
        );
        assert!(matches!(nf, StorageError::NotFound(_)));

        let perm = classify_os_error(
            OsError::PermissionDenied {
                path: "x".into(),
                source: "denied".into(),
            },
            "put",
        );
        match perm {
            StorageError::Backend { kind, .. } => assert_eq!(kind, "gcs.permanent"),
            other => panic!("expected Backend, got {other:?}"),
        }

        let unauth = classify_os_error(
            OsError::Unauthenticated {
                path: "x".into(),
                source: "no creds".into(),
            },
            "list",
        );
        match unauth {
            StorageError::Backend { kind, .. } => assert_eq!(kind, "gcs.permanent"),
            other => panic!("expected Backend, got {other:?}"),
        }

        let generic = classify_os_error(
            OsError::Generic {
                store: "gcs",
                source: "boom".into(),
            },
            "put",
        );
        match generic {
            StorageError::Backend { kind, .. } => assert_eq!(kind, "gcs.transient"),
            other => panic!("expected Backend, got {other:?}"),
        }
    }

    #[test]
    fn resolve_cache_root_uses_env_when_set() {
        let p = resolve_cache_root(Some("/tmp/custom-cache".into()), PathBuf::from("/ignored"));
        assert_eq!(p, PathBuf::from("/tmp/custom-cache"));
    }

    #[test]
    fn resolve_cache_root_falls_back_when_env_unset_or_empty() {
        let temp = PathBuf::from("/tmpdir");
        assert_eq!(
            resolve_cache_root(None, temp.clone()),
            temp.join(CACHE_DIR_NAME)
        );
        assert_eq!(
            resolve_cache_root(Some(String::new()), temp.clone()),
            temp.join(CACHE_DIR_NAME)
        );
    }
}
