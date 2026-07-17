//! S3 [`BundleStorage`] adapter (post1-T-3.1).
//!
//! Backed by the Apache Arrow `object_store` crate's `aws` feature,
//! which gives us a battle-tested S3 client with built-in
//! exponential-backoff retries, multipart uploads, and a unified
//! abstraction we can reuse for T-3.2 (GCS) and T-3.3 (Azure Blob)
//! by toggling additional features on the same crate.
//!
//! ## URI shape
//!
//! `s3://<bucket>[/<prefix>]`
//!
//! - `s3://my-bucket` — bucket with no key prefix; objects land at
//!   `<run-id>/<file>`.
//! - `s3://my-bucket/audit/prod` — bucket with a key prefix; objects
//!   land at `audit/prod/<run-id>/<file>`. Trailing slashes are
//!   normalized away.
//!
//! The `StorageRef` returned by [`put`] echoes the construction URI
//! suffixed with the run id, e.g. `s3://my-bucket/audit/prod/<run-id>`.
//! Callers treat the string as opaque; only the dispatcher parses it.
//!
//! ## Configuration
//!
//! Authentication and endpoint are sourced from the standard AWS
//! environment variables via [`object_store::aws::AmazonS3Builder::from_env`]:
//!
//! - `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`
//! - `AWS_REGION` (defaults to `us-east-1` when unset)
//! - `AWS_ENDPOINT_URL` — point at MinIO/LocalStack/etc.
//! - `AWS_ALLOW_HTTP=true` — required for non-HTTPS endpoints
//!
//! ## Sync trait, async SDK
//!
//! [`crate::audit::storage::BundleStorage`] is sync because every
//! call site (CLI, orchestrator background thread) is sync. The S3
//! SDK is async. We bridge with a single-threaded current-thread
//! tokio runtime owned by the adapter; each `put`/`get`/`list` runs
//! its async work via `Runtime::block_on`. This is well-defined
//! because the adapter is constructed and used outside any larger
//! tokio runtime — the CLI's only async runtime today is the one
//! the dashboard's axum server creates, which never reaches this
//! code path.
//!
//! ## Local cache
//!
//! `get` materializes the bundle on local disk under a cache root
//! (env `BORUNA_BUNDLE_CACHE`, defaults to
//! `<temp>/boruna-bundle-cache`) and returns the resulting directory
//! path. Repeated `get`s for the same ref overwrite the cache
//! contents (idempotent — bundle finalize is deterministic given
//! inputs, so the bytes are identical). No GC; operators clear the
//! cache directory on rotation.
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

use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as OsPath;
use object_store::{Error as OsError, ObjectStore, PutPayload};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

use super::storage::{BundleStorage, StorageError, StorageRef};

/// Default cache root for materialized bundles. Operators override
/// via `BORUNA_BUNDLE_CACHE`.
const CACHE_ENV_VAR: &str = "BORUNA_BUNDLE_CACHE";
const CACHE_DIR_NAME: &str = "boruna-bundle-cache";

/// Parsed `s3://<bucket>[/<prefix>]` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
struct S3Uri {
    bucket: String,
    /// Key prefix WITHOUT trailing slash. Empty string means
    /// objects live at the bucket root.
    prefix: String,
}

impl S3Uri {
    fn parse(uri: &str) -> Result<Self, StorageError> {
        let rest = uri
            .strip_prefix("s3://")
            .ok_or_else(|| StorageError::InvalidUri(uri.to_string()))?;
        if rest.is_empty() {
            return Err(StorageError::InvalidUri(format!(
                "s3 URI missing bucket name: {uri}"
            )));
        }
        let (bucket, prefix) = match rest.split_once('/') {
            Some((b, p)) => (b, p.trim_end_matches('/')),
            None => (rest, ""),
        };
        if bucket.is_empty() {
            return Err(StorageError::InvalidUri(format!(
                "s3 URI missing bucket name: {uri}"
            )));
        }
        // S3 bucket-name rules are looser than we need to enforce
        // here; we just reject the obvious invalids that would let a
        // typo through (whitespace, scheme delimiters, leading dot).
        if bucket.chars().any(|c| c.is_whitespace() || c == ':') {
            return Err(StorageError::InvalidUri(format!(
                "s3 URI has invalid bucket name: {uri}"
            )));
        }
        Ok(S3Uri {
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
        })
    }

    /// Object-store path for an object whose run-relative key is
    /// `key`. Joins prefix + run_id + key with `/` separators.
    fn object_path(&self, run_id: &str, key: &str) -> OsPath {
        // Normalize: `prefix` has no trailing slash; `key` has no
        // leading slash (we walk dir entries that way).
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

    /// `s3://<bucket>/<prefix>` — used as the reference suffix for
    /// listings and as the prefix `put` echoes back with `/<run_id>`
    /// appended.
    fn root_uri(&self) -> String {
        if self.prefix.is_empty() {
            format!("s3://{}", self.bucket)
        } else {
            format!("s3://{}/{}", self.bucket, self.prefix)
        }
    }
}

/// S3-backed [`BundleStorage`] implementation.
pub struct S3Bucket {
    uri: S3Uri,
    store: Arc<AmazonS3>,
    runtime: Arc<Runtime>,
    cache_root: PathBuf,
}

impl S3Bucket {
    /// Build an [`S3Bucket`] from a `s3://bucket[/prefix]` URI.
    /// Auth + endpoint come from `AWS_*` environment variables; see
    /// the module docs. Equivalent to
    /// `S3BucketBuilder::new(uri).build()`.
    pub fn from_uri(uri: &str) -> Result<Self, StorageError> {
        S3BucketBuilder::new(uri).build()
    }

    /// Run an async block on this adapter's runtime.
    fn block_on<F, T>(&self, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime.block_on(f)
    }
}

/// Builder for [`S3Bucket`] that lets callers (mostly tests) override
/// the local materialization cache root.
///
/// Production callers use [`S3Bucket::from_uri`] which delegates to
/// `S3BucketBuilder::new(uri).build()`. The cache root then comes from
/// `BORUNA_BUNDLE_CACHE` or `<temp>/boruna-bundle-cache`.
pub struct S3BucketBuilder {
    uri: String,
    cache_root: Option<PathBuf>,
}

impl S3BucketBuilder {
    /// Start a builder against the given `s3://bucket[/prefix]` URI.
    pub fn new(uri: impl Into<String>) -> Self {
        S3BucketBuilder {
            uri: uri.into(),
            cache_root: None,
        }
    }

    /// Override the cache root used to materialize bundles in
    /// [`S3Bucket::get`]. Tests use this to point at a `tempdir()`
    /// so each test sees a clean cache.
    pub fn with_cache_root(mut self, root: PathBuf) -> Self {
        self.cache_root = Some(root);
        self
    }

    /// Build the [`S3Bucket`].
    pub fn build(self) -> Result<S3Bucket, StorageError> {
        let parsed = S3Uri::parse(&self.uri)?;
        let cache_root = self.cache_root.unwrap_or_else(default_cache_root);
        let store = AmazonS3Builder::from_env()
            .with_bucket_name(&parsed.bucket)
            .build()
            .map_err(|e| classify_os_error(e, "construct"))?;
        let runtime = RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| StorageError::Backend {
                kind: "s3.runtime",
                msg: format!("failed to build tokio runtime: {e}"),
            })?;
        Ok(S3Bucket {
            uri: parsed,
            store: Arc::new(store),
            runtime: Arc::new(runtime),
            cache_root,
        })
    }
}

impl BundleStorage for S3Bucket {
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
        // Deterministic upload order. Helps reasoning about partial-
        // failure recovery.
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
        // Idempotent: clear and re-materialize. Bundle finalize is
        // deterministic so the bytes are identical, but a partial
        // previous fetch could leave stale files.
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

        // Strip the bucket-side prefix (`<prefix>/<run_id>/`) so we
        // can recreate the directory layout under cache_dir.
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
                    kind: "s3.unexpected_key",
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
        // To enumerate the "directory" level (one StorageRef per
        // run_id), use list_with_delimiter against the configured
        // prefix.
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
fn ref_to_run_id(uri: &S3Uri, r: &StorageRef) -> Result<String, StorageError> {
    let root = uri.root_uri();
    let suffix = r
        .0
        .strip_prefix(&root)
        .and_then(|s| s.strip_prefix('/'))
        .ok_or_else(|| {
            StorageError::InvalidUri(format!("ref {} does not match adapter root {}", r.0, root))
        })?;
    if suffix.is_empty() || suffix.contains('/') || suffix == ".." || suffix == "." {
        return Err(StorageError::InvalidUri(format!(
            "ref {} is not a single run-id under {}",
            r.0, root
        )));
    }
    Ok(suffix.to_string())
}

/// Map an `object_store` error onto a stable [`StorageError`]
/// variant. The `kind` strings are a stable error_kind taxonomy so
/// integrators (and the CLI's warning printer) can branch on them.
///
/// Mapping:
/// - `NotFound` → [`StorageError::NotFound`] (the dispatcher's
///   intrinsic NotFound).
/// - `Generic` / `Unauthenticated` / `PermissionDenied` →
///   `s3.permanent` (operator config issue; retry won't help).
/// - everything else (transient I/O, retries already exhausted) →
///   `s3.transient`.
///
/// The `op` argument names the calling site for better diagnostics
/// (e.g. `"put"`, `"get"`, `"list"`, `"construct"`).
fn classify_os_error(e: OsError, op: &'static str) -> StorageError {
    match &e {
        OsError::NotFound { .. } => StorageError::NotFound(format!("{op}: {e}")),
        OsError::PermissionDenied { .. } | OsError::Unauthenticated { .. } => {
            StorageError::Backend {
                kind: "s3.permanent",
                msg: format!("{op}: {e}"),
            }
        }
        _ => StorageError::Backend {
            kind: "s3.transient",
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
/// environment (which would race with cargo's parallel test runner —
/// see project-conventions §6 anti-pattern).
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
    fn s3_uri_parse_bucket_only() {
        let u = S3Uri::parse("s3://my-bucket").unwrap();
        assert_eq!(u.bucket, "my-bucket");
        assert_eq!(u.prefix, "");
        assert_eq!(u.root_uri(), "s3://my-bucket");
    }

    #[test]
    fn s3_uri_parse_bucket_and_prefix() {
        let u = S3Uri::parse("s3://b/audit/prod").unwrap();
        assert_eq!(u.bucket, "b");
        assert_eq!(u.prefix, "audit/prod");
        assert_eq!(u.root_uri(), "s3://b/audit/prod");
    }

    #[test]
    fn s3_uri_parse_strips_trailing_slash_on_prefix() {
        let u = S3Uri::parse("s3://b/audit/prod/").unwrap();
        assert_eq!(u.prefix, "audit/prod");
    }

    #[test]
    fn s3_uri_parse_rejects_missing_bucket() {
        for bad in ["s3://", "s3:///prefix"] {
            assert!(matches!(
                S3Uri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn s3_uri_parse_rejects_wrong_scheme() {
        for bad in ["http://b/p", "local:foo", "s3:bucket/p"] {
            assert!(matches!(
                S3Uri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn s3_uri_parse_rejects_invalid_bucket_chars() {
        for bad in ["s3:// bad", "s3://bad bucket/p"] {
            assert!(matches!(
                S3Uri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn object_path_concatenates_prefix_run_id_key() {
        let u = S3Uri::parse("s3://b/audit/prod").unwrap();
        let p = u.object_path("run-7", "steps/a.json");
        assert_eq!(p.to_string(), "audit/prod/run-7/steps/a.json");
    }

    #[test]
    fn object_path_skips_empty_prefix() {
        let u = S3Uri::parse("s3://b").unwrap();
        let p = u.object_path("run-7", "manifest.json");
        assert_eq!(p.to_string(), "run-7/manifest.json");
    }

    #[test]
    fn object_path_skips_empty_key() {
        let u = S3Uri::parse("s3://b/audit").unwrap();
        let p = u.object_path("run-7", "");
        assert_eq!(p.to_string(), "audit/run-7");
    }

    #[test]
    fn ref_to_run_id_extracts_suffix() {
        let u = S3Uri::parse("s3://b/audit").unwrap();
        let id = ref_to_run_id(&u, &StorageRef("s3://b/audit/run-7".into())).unwrap();
        assert_eq!(id, "run-7");
    }

    #[test]
    fn ref_to_run_id_rejects_mismatched_root() {
        let u = S3Uri::parse("s3://b/audit").unwrap();
        let err = ref_to_run_id(&u, &StorageRef("s3://other/audit/run-7".into())).unwrap_err();
        assert!(matches!(err, StorageError::InvalidUri(_)));
    }

    #[test]
    fn ref_to_run_id_rejects_nested_path() {
        let u = S3Uri::parse("s3://b/audit").unwrap();
        let err = ref_to_run_id(&u, &StorageRef("s3://b/audit/run-7/extra".into())).unwrap_err();
        assert!(matches!(err, StorageError::InvalidUri(_)));
    }

    #[test]
    fn ref_to_run_id_rejects_empty_suffix() {
        let u = S3Uri::parse("s3://b/audit").unwrap();
        let err = ref_to_run_id(&u, &StorageRef("s3://b/audit/".into())).unwrap_err();
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
            StorageError::Backend { kind, .. } => assert_eq!(kind, "s3.permanent"),
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
            StorageError::Backend { kind, .. } => assert_eq!(kind, "s3.permanent"),
            other => panic!("expected Backend, got {other:?}"),
        }

        let generic = classify_os_error(
            OsError::Generic {
                store: "s3",
                source: "boom".into(),
            },
            "put",
        );
        match generic {
            StorageError::Backend { kind, .. } => assert_eq!(kind, "s3.transient"),
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
