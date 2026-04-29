//! Azure Blob Storage [`BundleStorage`] adapter (post1-T-3.3).
//!
//! Backed by the Apache Arrow `object_store` crate's `azure`
//! feature. Mirrors the [`crate::audit::storage_s3`] (T-3.1) and
//! [`crate::audit::storage_gcs`] (T-3.2) adapters — same trait,
//! same builder pattern, same error_kind taxonomy structure
//! (`azure.*` instead of `s3.*` / `gcs.*`).
//!
//! With T-3.3 landing, all three remote schemes (`s3://`, `gs://`,
//! `azblob://`) ship and the [`BundleStorage`] trait was promoted
//! out of `#[doc(hidden)]` to public 1.x API. The three adapter
//! files remain separate intentionally; YAGNI says wait until a
//! fourth provider (or an actual abstraction need) before
//! extracting a generic `storage_object_store.rs` module.
//!
//! ## URI shape
//!
//! `azblob://<account>/<container>[/<prefix>]`
//!
//! Unlike S3 (single-namespace) and GCS (single-namespace), Azure
//! has a two-level namespace: `account` (the storage account
//! resource) and `container` (a "bucket" inside that account). We
//! encode both in the URI so an operator can grep their config
//! and see exactly which account a bundle landed in, without
//! depending on out-of-band env vars to disambiguate.
//!
//! - `azblob://myacct/audit-bundles` — account `myacct`, container
//!   `audit-bundles`, no key prefix.
//! - `azblob://myacct/audit-bundles/prod` — same as above with the
//!   `prod/` key prefix.
//!
//! Trailing slashes on the prefix are normalized away.
//!
//! The `StorageRef` returned by [`put`] echoes the construction URI
//! suffixed with the run id, e.g. `azblob://myacct/audit-bundles/prod/<run-id>`.
//!
//! ## Configuration
//!
//! Authentication is sourced from
//! [`object_store::azure::MicrosoftAzureBuilder::from_env`], which
//! recognizes:
//!
//! - `AZURE_STORAGE_ACCOUNT_NAME` — storage account name (we
//!   override this from the URI; env-var override would mismatch
//!   the URI account and produce confusing errors).
//! - `AZURE_STORAGE_ACCOUNT_KEY` / `AZURE_STORAGE_ACCESS_KEY` —
//!   master key.
//! - `AZURE_STORAGE_CLIENT_ID` / `AZURE_STORAGE_CLIENT_SECRET` /
//!   `AZURE_STORAGE_TENANT_ID` — service-principal OAuth.
//! - `AZURE_STORAGE_SAS_KEY` — pre-shared SAS token.
//!
//! For Workload Identity / Managed Identity, set
//! `AZURE_STORAGE_USE_AZURE_CLI=true` or use the equivalent
//! object_store key.
//!
//! ### Pointing at Azurite / a custom endpoint
//!
//! Use [`AzureBlobBucketBuilder::with_use_emulator`] for the
//! standard Azurite well-known credentials + endpoint. For other
//! private-network deployments, [`AzureBlobBucketBuilder::with_endpoint`]
//! takes a full base URL.
//!
//! ## Sync trait, async SDK
//!
//! Same approach as S3/GCS: a per-instance current-thread tokio
//! runtime owned by the adapter bridges sync calls onto the async
//! SDK. See the storage_s3.rs module docs for the rationale.
//!
//! ## Local cache
//!
//! Same as S3/GCS: `get` materializes under
//! [`BORUNA_BUNDLE_CACHE`] (default `<temp>/boruna-bundle-cache`).
//!
//! ## Determinism contract
//!
//! - Object listings are sorted before iteration so [`list`] returns
//!   `Vec<StorageRef>` in stable order across runs.
//! - Upload is best-effort idempotent.
//! - `put` failures are logged at the call site; the local bundle
//!   remains the authoritative record (see `storage.rs` doc).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use object_store::azure::{MicrosoftAzure, MicrosoftAzureBuilder};
use object_store::path::Path as OsPath;
use object_store::{Error as OsError, ObjectStore, PutPayload};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

use super::storage::{BundleStorage, StorageError, StorageRef};

/// Default cache root for materialized bundles. Operators override
/// via `BORUNA_BUNDLE_CACHE`. Shared with the S3 + GCS adapters.
const CACHE_ENV_VAR: &str = "BORUNA_BUNDLE_CACHE";
const CACHE_DIR_NAME: &str = "boruna-bundle-cache";

/// Parsed `azblob://<account>/<container>[/<prefix>]` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AzUri {
    account: String,
    container: String,
    /// Key prefix WITHOUT trailing slash. Empty string means
    /// objects live at the container root.
    prefix: String,
}

impl AzUri {
    fn parse(uri: &str) -> Result<Self, StorageError> {
        let rest = uri
            .strip_prefix("azblob://")
            .ok_or_else(|| StorageError::InvalidUri(uri.to_string()))?;
        if rest.is_empty() {
            return Err(StorageError::InvalidUri(format!(
                "azblob URI missing account/container: {uri}"
            )));
        }
        // Split at first '/': account before, container[/prefix] after.
        let (account, after_account) = rest.split_once('/').ok_or_else(|| {
            StorageError::InvalidUri(format!("azblob URI missing container: {uri}"))
        })?;
        if account.is_empty() {
            return Err(StorageError::InvalidUri(format!(
                "azblob URI missing account name: {uri}"
            )));
        }
        if after_account.is_empty() {
            return Err(StorageError::InvalidUri(format!(
                "azblob URI missing container name: {uri}"
            )));
        }
        // Split at next '/': container before, prefix after.
        let (container, prefix) = match after_account.split_once('/') {
            Some((c, p)) => (c, p.trim_end_matches('/')),
            None => (after_account, ""),
        };
        if container.is_empty() {
            return Err(StorageError::InvalidUri(format!(
                "azblob URI missing container name: {uri}"
            )));
        }
        // Reject obvious typos in the path components.
        for (label, val) in [("account", account), ("container", container)] {
            if val.chars().any(|c| c.is_whitespace() || c == ':') {
                return Err(StorageError::InvalidUri(format!(
                    "azblob URI has invalid {label} name: {uri}"
                )));
            }
        }
        Ok(AzUri {
            account: account.to_string(),
            container: container.to_string(),
            prefix: prefix.to_string(),
        })
    }

    /// Object-store path for an object whose run-relative key is
    /// `key`. Joins prefix + run_id + key with `/` separators.
    /// Note: the `container` is set on the object_store builder
    /// (via `with_container_name`), so it does NOT appear in the
    /// object path here.
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

    /// `azblob://<account>/<container>/<prefix>` — used as the
    /// reference suffix for listings and as the prefix `put`
    /// echoes back with `/<run_id>` appended.
    fn root_uri(&self) -> String {
        if self.prefix.is_empty() {
            format!("azblob://{}/{}", self.account, self.container)
        } else {
            format!(
                "azblob://{}/{}/{}",
                self.account, self.container, self.prefix
            )
        }
    }
}

/// Azure Blob-backed [`BundleStorage`] implementation.
pub struct AzureBlobBucket {
    uri: AzUri,
    store: Arc<MicrosoftAzure>,
    runtime: Arc<Runtime>,
    cache_root: PathBuf,
}

impl AzureBlobBucket {
    /// Build an [`AzureBlobBucket`] from a
    /// `azblob://account/container[/prefix]` URI. Auth comes from
    /// `AZURE_STORAGE_*` environment variables; see the module
    /// docs. Equivalent to
    /// `AzureBlobBucketBuilder::new(uri).build()`.
    pub fn from_uri(uri: &str) -> Result<Self, StorageError> {
        AzureBlobBucketBuilder::new(uri).build()
    }

    /// Run an async block on this adapter's runtime.
    fn block_on<F, T>(&self, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime.block_on(f)
    }
}

/// Builder for [`AzureBlobBucket`] that lets callers (mostly tests)
/// override the local materialization cache root, the Azure
/// endpoint, and switch the SDK into Azurite-emulator mode.
///
/// Production callers use [`AzureBlobBucket::from_uri`] which
/// delegates to `AzureBlobBucketBuilder::new(uri).build()`. The
/// cache root then comes from `BORUNA_BUNDLE_CACHE` or
/// `<temp>/boruna-bundle-cache`.
pub struct AzureBlobBucketBuilder {
    uri: String,
    cache_root: Option<PathBuf>,
    endpoint: Option<String>,
    use_emulator: bool,
}

impl AzureBlobBucketBuilder {
    /// Start a builder against the given
    /// `azblob://account/container[/prefix]` URI.
    pub fn new(uri: impl Into<String>) -> Self {
        AzureBlobBucketBuilder {
            uri: uri.into(),
            cache_root: None,
            endpoint: None,
            use_emulator: false,
        }
    }

    /// Override the cache root used to materialize bundles in
    /// [`AzureBlobBucket::get`]. Tests use this to point at a
    /// `tempdir()` so each test sees a clean cache.
    pub fn with_cache_root(mut self, root: PathBuf) -> Self {
        self.cache_root = Some(root);
        self
    }

    /// Override the Azure endpoint (e.g.
    /// `http://localhost:10000/devstoreaccount1` for Azurite).
    /// Production callers leave this unset to talk to Azure proper.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Configure the adapter to use the Azurite emulator's
    /// well-known account + key + endpoint shape. Equivalent to
    /// setting `AZURE_USE_EMULATOR=true` in the env, but doesn't
    /// pollute the global env.
    pub fn with_use_emulator(mut self, on: bool) -> Self {
        self.use_emulator = on;
        self
    }

    /// Build the [`AzureBlobBucket`].
    pub fn build(self) -> Result<AzureBlobBucket, StorageError> {
        let parsed = AzUri::parse(&self.uri)?;
        let cache_root = self.cache_root.unwrap_or_else(default_cache_root);
        let mut sb = MicrosoftAzureBuilder::from_env()
            .with_account(&parsed.account)
            .with_container_name(&parsed.container);
        if self.use_emulator {
            sb = sb.with_use_emulator(true);
        }
        if let Some(endpoint) = self.endpoint {
            sb = sb.with_endpoint(endpoint).with_allow_http(true);
        }
        let store = sb.build().map_err(|e| classify_os_error(e, "construct"))?;
        let runtime = RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| StorageError::Backend {
                kind: "azure.runtime",
                msg: format!("failed to build tokio runtime: {e}"),
            })?;
        Ok(AzureBlobBucket {
            uri: parsed,
            store: Arc::new(store),
            runtime: Arc::new(runtime),
            cache_root,
        })
    }
}

impl BundleStorage for AzureBlobBucket {
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
                    kind: "azure.unexpected_key",
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
/// previously emitted. Validates that the account+container+prefix
/// match the adapter's configured root.
fn ref_to_run_id(uri: &AzUri, r: &StorageRef) -> Result<String, StorageError> {
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
/// variant. Same shape as the S3/GCS adapters but emits `azure.*`
/// kinds so integrators can distinguish.
fn classify_os_error(e: OsError, op: &'static str) -> StorageError {
    match &e {
        OsError::NotFound { .. } => StorageError::NotFound(format!("{op}: {e}")),
        OsError::PermissionDenied { .. } | OsError::Unauthenticated { .. } => {
            StorageError::Backend {
                kind: "azure.permanent",
                msg: format!("{op}: {e}"),
            }
        }
        _ => StorageError::Backend {
            kind: "azure.transient",
            msg: format!("{op}: {e}"),
        },
    }
}

fn default_cache_root() -> PathBuf {
    resolve_cache_root(std::env::var(CACHE_ENV_VAR).ok(), std::env::temp_dir())
}

/// Pure helper: pick the cache root given a raw env-var value and a
/// fallback temp-dir. Extracted for the same testability reason as
/// in the S3 / GCS adapters.
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
    fn az_uri_parse_account_and_container() {
        let u = AzUri::parse("azblob://acct/cont").unwrap();
        assert_eq!(u.account, "acct");
        assert_eq!(u.container, "cont");
        assert_eq!(u.prefix, "");
        assert_eq!(u.root_uri(), "azblob://acct/cont");
    }

    #[test]
    fn az_uri_parse_with_prefix() {
        let u = AzUri::parse("azblob://acct/cont/audit/prod").unwrap();
        assert_eq!(u.account, "acct");
        assert_eq!(u.container, "cont");
        assert_eq!(u.prefix, "audit/prod");
        assert_eq!(u.root_uri(), "azblob://acct/cont/audit/prod");
    }

    #[test]
    fn az_uri_parse_strips_trailing_slash_on_prefix() {
        let u = AzUri::parse("azblob://acct/cont/audit/prod/").unwrap();
        assert_eq!(u.prefix, "audit/prod");
    }

    #[test]
    fn az_uri_parse_rejects_missing_container() {
        for bad in ["azblob://acct", "azblob://acct/"] {
            assert!(matches!(
                AzUri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn az_uri_parse_rejects_missing_account() {
        for bad in ["azblob://", "azblob:///cont"] {
            assert!(matches!(
                AzUri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn az_uri_parse_rejects_wrong_scheme() {
        for bad in ["http://b/p", "local:foo", "s3://b/p", "gs://b/p"] {
            assert!(matches!(
                AzUri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn az_uri_parse_rejects_invalid_account_or_container() {
        for bad in ["azblob:// /cont", "azblob://acct/ ", "azblob://a:c/cont"] {
            assert!(matches!(
                AzUri::parse(bad),
                Err(StorageError::InvalidUri(_))
            ));
        }
    }

    #[test]
    fn object_path_concatenates_prefix_run_id_key() {
        let u = AzUri::parse("azblob://acct/cont/audit").unwrap();
        let p = u.object_path("run-7", "steps/a.json");
        assert_eq!(p.to_string(), "audit/run-7/steps/a.json");
    }

    #[test]
    fn object_path_skips_empty_prefix() {
        let u = AzUri::parse("azblob://acct/cont").unwrap();
        let p = u.object_path("run-7", "manifest.json");
        assert_eq!(p.to_string(), "run-7/manifest.json");
    }

    #[test]
    fn object_path_skips_empty_key() {
        let u = AzUri::parse("azblob://acct/cont/audit").unwrap();
        let p = u.object_path("run-7", "");
        assert_eq!(p.to_string(), "audit/run-7");
    }

    #[test]
    fn ref_to_run_id_extracts_suffix() {
        let u = AzUri::parse("azblob://acct/cont/audit").unwrap();
        let id = ref_to_run_id(&u, &StorageRef("azblob://acct/cont/audit/run-7".into())).unwrap();
        assert_eq!(id, "run-7");
    }

    #[test]
    fn ref_to_run_id_rejects_mismatched_root() {
        let u = AzUri::parse("azblob://acct/cont/audit").unwrap();
        let err =
            ref_to_run_id(&u, &StorageRef("azblob://other/cont/audit/run-7".into())).unwrap_err();
        assert!(matches!(err, StorageError::InvalidUri(_)));
    }

    #[test]
    fn ref_to_run_id_rejects_nested_path() {
        let u = AzUri::parse("azblob://acct/cont/audit").unwrap();
        let err = ref_to_run_id(
            &u,
            &StorageRef("azblob://acct/cont/audit/run-7/extra".into()),
        )
        .unwrap_err();
        assert!(matches!(err, StorageError::InvalidUri(_)));
    }

    #[test]
    fn ref_to_run_id_rejects_empty_suffix() {
        let u = AzUri::parse("azblob://acct/cont/audit").unwrap();
        let err = ref_to_run_id(&u, &StorageRef("azblob://acct/cont/audit/".into())).unwrap_err();
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
            StorageError::Backend { kind, .. } => assert_eq!(kind, "azure.permanent"),
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
            StorageError::Backend { kind, .. } => assert_eq!(kind, "azure.permanent"),
            other => panic!("expected Backend, got {other:?}"),
        }

        let generic = classify_os_error(
            OsError::Generic {
                store: "azure",
                source: "boom".into(),
            },
            "put",
        );
        match generic {
            StorageError::Backend { kind, .. } => assert_eq!(kind, "azure.transient"),
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
