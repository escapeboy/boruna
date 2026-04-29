//! Pluggable bundle-storage backends.
//!
//! [`BundleStorage`] is the narrow trait that lets evidence bundles
//! be written to anything that implements `put`/`get`/`list`. The
//! trait shipped in post1-T-2.3 (LocalFs only, hidden); the three
//! remote adapters landed in T-3.1 (S3), T-3.2 (GCS), and T-3.3
//! (Azure Blob). With three independent implementations, the trait
//! shape has stabilized and is now part of the public 1.x API
//! surface.
//!
//! ## Status
//!
//! **Stable** as of post1 promotion (Apr 2026). The trait,
//! [`StorageRef`], [`StorageError`], [`LocalFs`], and the
//! `from_uri` dispatcher are public 1.x API. New schemes are
//! additive (existing code keeps working). New error_kind strings
//! on `Backend { kind, .. }` are also additive — integrators
//! switching on `kind` should treat unknown values as
//! `transient`.
//!
//! See `docs/concepts/bundle-storage.md` for the operator-facing
//! overview that links to the per-provider guides
//! (`docs/guides/bundle-storage-{s3,gcs,azure}.md`).
//!
//! ## Semantics
//!
//! - Storage is called **after** the local `EvidenceBundleBuilder`
//!   finalize has succeeded. The local bundle on disk is the
//!   primary record; storage is a copy. A storage-side failure is
//!   **logged**, not bubbled up — the workflow already succeeded.
//! - `put` is idempotent on `run_id`: re-uploading is allowed and
//!   should succeed, since bundle finalize is itself deterministic
//!   given inputs.
//! - `get` MAY hit a local cache directory (the remote adapters use
//!   `<temp>/boruna-bundle-cache` overridable via
//!   `BORUNA_BUNDLE_CACHE`) so repeated `boruna evidence verify
//!   <ref>` calls don't round-trip the network. The `LocalFs`
//!   adapter trivially returns the same path it was given.

use std::fmt;
use std::path::{Path, PathBuf};

/// Opaque pointer to a stored bundle.
///
/// The string carries a scheme prefix the dispatcher uses
/// (`local:<run-id>`, `s3://bucket/prefix/<run-id>`,
/// `gs://...`, `azblob://...`). Callers are expected to treat the
/// inner string as opaque; only the dispatcher parses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageRef(pub String);

impl fmt::Display for StorageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Errors produced by the bundle-storage layer.
///
/// Stable for the 1.x line. New variants will be additive (existing
/// match arms keep compiling because the enum is `#[non_exhaustive]`
/// — see below). New `Backend { kind, .. }` strings are also
/// additive; integrators switching on `kind` should treat unknown
/// values as `transient` (retryable).
#[derive(Debug)]
#[non_exhaustive]
pub enum StorageError {
    /// Local I/O failure (file not found, permission denied, etc).
    /// Wraps the underlying `std::io::Error`.
    Io(std::io::Error),
    /// The supplied URI does not match a recognized scheme, or is
    /// malformed within a recognized scheme. Includes the offending
    /// URI in the message for operator triage.
    InvalidUri(String),
    /// `get` was called against a `StorageRef` whose backing
    /// objects no longer exist (or never existed).
    NotFound(String),
    /// Adapter-specific failure with a stable identifying string.
    /// `LocalFs` does not produce these; remote adapters use them
    /// for transient/permanent backend errors. The `kind` strings
    /// are stable (`s3.transient`, `s3.permanent`, `gcs.transient`,
    /// etc.); see each adapter's docs for the per-provider taxonomy.
    Backend {
        /// Stable identifier; see the per-adapter docs.
        kind: &'static str,
        /// Human-readable message for logs.
        msg: String,
    },
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::Io(e) => write!(f, "io: {e}"),
            StorageError::InvalidUri(s) => write!(f, "invalid storage URI: {s}"),
            StorageError::NotFound(s) => write!(f, "not found: {s}"),
            StorageError::Backend { kind, msg } => write!(f, "{kind}: {msg}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        StorageError::Io(e)
    }
}

/// Pluggable bundle storage. `Send + Sync` so adapters can live in
/// shared state and be invoked from any thread.
///
/// `put` returns the storage ref the caller should record alongside
/// the run (typically in the orchestrator's metadata for
/// `boruna evidence verify`). `get` materializes the bundle on
/// local disk and returns the directory path; the caller is
/// expected to read it the same way it would read a freshly
/// produced local bundle. `list` enumerates run-id-keyed entries
/// under the configured root, optionally filtered by a `prefix`.
///
/// Implementations ship in the orchestrator: `LocalFs` (always),
/// `S3Bucket` (`s3` feature), `GcsBucket` (`gcs` feature),
/// `AzureBlobBucket` (`azure` feature). Construct via
/// [`from_uri`] or call the per-adapter `from_uri` constructors
/// directly.
pub trait BundleStorage: Send + Sync {
    /// Upload (or local-copy, for `LocalFs`) the contents of
    /// `bundle_dir` under the storage namespace, keyed by `run_id`.
    /// Returns the [`StorageRef`] to record in metadata.
    fn put(&self, run_id: &str, bundle_dir: &Path) -> Result<StorageRef, StorageError>;
    /// Materialize the bundle behind the given ref onto local disk
    /// and return the resulting directory path. Remote adapters use
    /// a local cache directory; the returned path is the cached
    /// copy.
    fn get(&self, r: &StorageRef) -> Result<PathBuf, StorageError>;
    /// Enumerate refs at this storage's root. Pass `Some(prefix)`
    /// to filter run ids by a literal prefix. Returns refs in
    /// lexicographic order.
    fn list(&self, prefix: Option<&str>) -> Result<Vec<StorageRef>, StorageError>;
}

/// Local-filesystem adapter. The bundle directory IS the storage
/// location — `put` is a no-op (the builder already wrote there)
/// and `get` returns the directory back to the caller.
///
/// Construct via [`LocalFs::new`] over the operator's bundle root
/// (e.g. `<data-dir>/evidence/`). The root is the directory under
/// which bundle subdirectories live, one per `run_id`.
pub struct LocalFs {
    root: PathBuf,
}

impl LocalFs {
    /// Construct a [`LocalFs`] rooted at `root`. The root directory
    /// does not need to exist yet; it will be created on the first
    /// `put`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        LocalFs { root: root.into() }
    }
}

impl BundleStorage for LocalFs {
    fn put(&self, run_id: &str, bundle_dir: &Path) -> Result<StorageRef, StorageError> {
        // Sanity: the bundle dir SHOULD already live under `root` on
        // success, but we don't enforce — operators may finalize
        // into a temp dir and then put. If it isn't under root,
        // copy it. If it is, no-op.
        let target = self.root.join(run_id);
        if bundle_dir != target {
            // Best-effort recursive copy. Failures bubble up as
            // StorageError::Io. Real-world callers should arrange
            // for bundle_dir to already be under root so this
            // branch never runs.
            copy_dir_recursive(bundle_dir, &target)?;
        }
        Ok(StorageRef(format!("local:{run_id}")))
    }

    fn get(&self, r: &StorageRef) -> Result<PathBuf, StorageError> {
        let id =
            r.0.strip_prefix("local:")
                .ok_or_else(|| StorageError::InvalidUri(r.0.clone()))?;
        let path = self.root.join(id);
        if !path.exists() {
            return Err(StorageError::NotFound(r.0.clone()));
        }
        Ok(path)
    }

    fn list(&self, prefix: Option<&str>) -> Result<Vec<StorageRef>, StorageError> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(p) = prefix {
                if !name.starts_with(p) {
                    continue;
                }
            }
            out.push(StorageRef(format!("local:{name}")));
        }
        // Deterministic order.
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir_recursive(&path, &dst_path)?;
        } else if ft.is_file() {
            std::fs::copy(&path, &dst_path)?;
        }
        // Skip symlinks etc — bundles are pure files+dirs.
    }
    Ok(())
}

/// Parse a `--bundle-storage <uri>` value into a constructed
/// [`BundleStorage`] adapter.
///
/// Recognized schemes:
/// - `local:<root>` — always available; constructs a [`LocalFs`].
/// - `s3://<bucket>[/<prefix>]` — requires the `s3` feature
///   (post1-T-3.1). Constructs an
///   [`crate::audit::storage_s3::S3Bucket`] backed by `object_store`.
/// - `gs://<bucket>[/<prefix>]` — requires the `gcs` feature
///   (post1-T-3.2). Constructs a
///   [`crate::audit::storage_gcs::GcsBucket`] backed by `object_store`.
/// - `azblob://<account>/<container>[/<prefix>]` — requires the
///   `azure` feature (post1-T-3.3). Constructs an
///   [`crate::audit::storage_azure::AzureBlobBucket`] backed by
///   `object_store`.
///
/// When a remote scheme's feature is OFF the URI rejects with an
/// actionable message that points the operator at the feature
/// flag — never silently ignored, which would create an audit gap.
///
/// Empty / `None` URI returns `None` so callers can fall back to
/// their existing local-only path.
pub fn from_uri(uri: Option<&str>) -> Result<Option<Box<dyn BundleStorage>>, StorageError> {
    let Some(uri) = uri else {
        return Ok(None);
    };
    let uri = uri.trim();
    if uri.is_empty() {
        return Ok(None);
    }
    if let Some(rest) = uri.strip_prefix("local:") {
        if rest.is_empty() {
            return Err(StorageError::InvalidUri(uri.to_string()));
        }
        return Ok(Some(Box::new(LocalFs::new(rest))));
    }
    #[cfg(feature = "s3")]
    if uri.starts_with("s3://") {
        let bucket = crate::audit::storage_s3::S3Bucket::from_uri(uri)?;
        return Ok(Some(Box::new(bucket)));
    }
    #[cfg(not(feature = "s3"))]
    if uri.starts_with("s3://") {
        return Err(StorageError::InvalidUri(format!(
            "{uri} requires the `s3` feature; rebuild with \
             `--features boruna-orchestrator/s3` (or \
             `--features boruna-cli/s3` for the CLI binary)"
        )));
    }
    #[cfg(feature = "gcs")]
    if uri.starts_with("gs://") {
        let bucket = crate::audit::storage_gcs::GcsBucket::from_uri(uri)?;
        return Ok(Some(Box::new(bucket)));
    }
    #[cfg(not(feature = "gcs"))]
    if uri.starts_with("gs://") {
        return Err(StorageError::InvalidUri(format!(
            "{uri} requires the `gcs` feature; rebuild with \
             `--features boruna-orchestrator/gcs` (or \
             `--features boruna-cli/gcs` for the CLI binary)"
        )));
    }
    #[cfg(feature = "azure")]
    if uri.starts_with("azblob://") {
        let bucket = crate::audit::storage_azure::AzureBlobBucket::from_uri(uri)?;
        return Ok(Some(Box::new(bucket)));
    }
    #[cfg(not(feature = "azure"))]
    if uri.starts_with("azblob://") {
        return Err(StorageError::InvalidUri(format!(
            "{uri} requires the `azure` feature; rebuild with \
             `--features boruna-orchestrator/azure` (or \
             `--features boruna-cli/azure` for the CLI binary)"
        )));
    }
    Err(StorageError::InvalidUri(uri.to_string()))
}

/// Build a human-readable list of the schemes this binary supports.
/// Reserved for use in any future scheme-specific reservation
/// messages. Currently unused (all remote schemes ship with their
/// own actionable OFF-feature messages) but kept available because
/// the next reserved scheme will need it.
#[allow(dead_code)]
fn available_schemes_help() -> String {
    let mut schemes = vec!["local:<path>"];
    if cfg!(feature = "s3") {
        schemes.push("s3://");
    }
    if cfg!(feature = "gcs") {
        schemes.push("gs://");
    }
    if cfg!(feature = "azure") {
        schemes.push("azblob://");
    }
    schemes.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_bundle(root: &Path, run_id: &str) -> PathBuf {
        let bundle = root.join(run_id);
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("manifest.json"), b"{\"v\":1}").unwrap();
        std::fs::create_dir_all(bundle.join("steps")).unwrap();
        std::fs::write(bundle.join("steps").join("a.json"), b"a").unwrap();
        bundle
    }

    #[test]
    fn localfs_put_then_get_roundtrips() {
        let dir = tempdir().unwrap();
        let bundle = write_bundle(dir.path(), "run-1");
        let fs = LocalFs::new(dir.path());
        let r = fs.put("run-1", &bundle).unwrap();
        assert_eq!(r.0, "local:run-1");
        let resolved = fs.get(&r).unwrap();
        assert!(resolved.exists());
        assert!(resolved.join("manifest.json").exists());
        assert!(resolved.join("steps").join("a.json").exists());
    }

    #[test]
    fn localfs_get_returns_not_found_for_missing() {
        let dir = tempdir().unwrap();
        let fs = LocalFs::new(dir.path());
        let err = fs.get(&StorageRef("local:missing".into())).unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
    }

    #[test]
    fn localfs_get_rejects_wrong_scheme() {
        let dir = tempdir().unwrap();
        let fs = LocalFs::new(dir.path());
        let err = fs.get(&StorageRef("s3://bucket/key".into())).unwrap_err();
        assert!(matches!(err, StorageError::InvalidUri(_)));
    }

    #[test]
    fn localfs_put_copies_when_bundle_outside_root() {
        let root = tempdir().unwrap();
        let other = tempdir().unwrap();
        let bundle = write_bundle(other.path(), "run-2");
        let fs = LocalFs::new(root.path());
        let r = fs.put("run-2", &bundle).unwrap();
        let resolved = fs.get(&r).unwrap();
        // Resolved path is rooted at fs's root, not the original
        // tempdir.
        assert!(resolved.starts_with(root.path()));
        assert!(resolved.join("manifest.json").exists());
    }

    #[test]
    fn localfs_list_returns_sorted_refs() {
        let dir = tempdir().unwrap();
        write_bundle(dir.path(), "run-c");
        write_bundle(dir.path(), "run-a");
        write_bundle(dir.path(), "run-b");
        let fs = LocalFs::new(dir.path());
        let refs = fs.list(None).unwrap();
        assert_eq!(
            refs.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            vec!["local:run-a", "local:run-b", "local:run-c"]
        );
    }

    #[test]
    fn localfs_list_filters_by_prefix() {
        let dir = tempdir().unwrap();
        write_bundle(dir.path(), "alpha-1");
        write_bundle(dir.path(), "alpha-2");
        write_bundle(dir.path(), "beta-1");
        let fs = LocalFs::new(dir.path());
        let refs = fs.list(Some("alpha-")).unwrap();
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().all(|r| r.0.starts_with("local:alpha-")));
    }

    #[test]
    fn from_uri_none_and_empty_yield_none() {
        assert!(from_uri(None).unwrap().is_none());
        assert!(from_uri(Some("")).unwrap().is_none());
        assert!(from_uri(Some("   ")).unwrap().is_none());
    }

    #[test]
    fn from_uri_local_constructs_adapter() {
        let dir = tempdir().unwrap();
        let uri = format!("local:{}", dir.path().display());
        let storage = from_uri(Some(&uri)).unwrap().unwrap();
        // Smoke: list on an empty root returns no refs.
        assert!(storage.list(None).unwrap().is_empty());
    }

    fn assert_invalid_uri<T>(r: Result<T, StorageError>) {
        match r {
            Err(StorageError::InvalidUri(_)) => {}
            Err(other) => panic!("expected InvalidUri, got {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    /// `azblob://` is feature-gated as of T-3.3. With the `azure`
    /// feature OFF, the URI must reject with an actionable message
    /// (matches the s3/gcs OFF-feature pattern).
    #[cfg(not(feature = "azure"))]
    #[test]
    fn from_uri_azure_without_feature_rejects_with_actionable_message() {
        match from_uri(Some("azblob://acct/cont")) {
            Err(StorageError::InvalidUri(msg)) => {
                assert!(
                    msg.contains("requires the `azure` feature"),
                    "expected actionable message, got: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidUri, got {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    /// When the `s3` feature is OFF, `s3://` URIs must reject with
    /// `InvalidUri` whose message tells the operator how to enable
    /// the feature. This is a critical UX guarantee — silently
    /// shipping a binary that ignores `--bundle-storage s3://...`
    /// would produce an audit gap.
    #[cfg(not(feature = "s3"))]
    #[test]
    fn from_uri_s3_without_feature_rejects_with_actionable_message() {
        match from_uri(Some("s3://bucket/prefix")) {
            Err(StorageError::InvalidUri(msg)) => {
                assert!(
                    msg.contains("requires the `s3` feature"),
                    "expected actionable message, got: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidUri, got {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    /// Same UX guarantee for `gs://` when the `gcs` feature is OFF
    /// (post1-T-3.2). Mirrors the s3 test above.
    #[cfg(not(feature = "gcs"))]
    #[test]
    fn from_uri_gcs_without_feature_rejects_with_actionable_message() {
        match from_uri(Some("gs://bucket/prefix")) {
            Err(StorageError::InvalidUri(msg)) => {
                assert!(
                    msg.contains("requires the `gcs` feature"),
                    "expected actionable message, got: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidUri, got {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn from_uri_unknown_scheme_rejects() {
        assert_invalid_uri(from_uri(Some("ftp://oldschool")));
    }
}
