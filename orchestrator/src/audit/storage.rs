//! Pluggable bundle-storage backends (post1-T-2.3).
//!
//! [`BundleStorage`] is the narrow trait that lets evidence bundles
//! be written to anything that implements `put`/`get`/`list` —
//! initially only the [`LocalFs`] adapter, with S3 / GCS / Azure
//! Blob landing in T-3.1 / T-3.2 / T-3.3 respectively.
//!
//! ## Status
//!
//! The trait is `#[doc(hidden)]` until at least one remote adapter
//! ships (T-3.1, S3). Until then we may still adjust shape based on
//! what the first remote impl actually needs. Once the S3 adapter
//! lands the trait gets the `pub` treatment in the docs.
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
//! - `get` MAY hit a local cache directory at `<data-dir>/cache/`
//!   so repeated `boruna evidence verify <ref>` calls don't
//!   round-trip the network. The `LocalFs` adapter trivially
//!   returns the same path it was given.

#![allow(dead_code)] // Wave 3 adapters will exercise the full surface.

use std::fmt;
use std::path::{Path, PathBuf};

/// Opaque pointer to a stored bundle.
///
/// The string carries a scheme prefix the dispatcher uses
/// (`local:<run-id>`, `s3://bucket/prefix/<run-id>`,
/// `gs://...`, `azblob://...`). Callers are expected to treat the
/// inner string as opaque; only the dispatcher parses it.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageRef(pub String);

impl fmt::Display for StorageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub enum StorageError {
    Io(std::io::Error),
    InvalidUri(String),
    NotFound(String),
    /// Adapter-specific failure with a stable identifying string.
    /// `LocalFs` does not produce these; remote adapters use them
    /// for transient/permanent backend errors.
    Backend {
        kind: &'static str,
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

/// Pluggable bundle storage. Send + Sync so adapters can live in
/// shared state and be invoked from any thread.
///
/// `put` returns the storage ref the caller should record alongside
/// the run (typically in the orchestrator's metadata for
/// `boruna evidence verify`). `get` materializes the bundle on
/// local disk and returns the directory path; the caller is
/// expected to read it the same way it would read a freshly
/// produced local bundle.
#[doc(hidden)]
pub trait BundleStorage: Send + Sync {
    fn put(&self, run_id: &str, bundle_dir: &Path) -> Result<StorageRef, StorageError>;
    fn get(&self, r: &StorageRef) -> Result<PathBuf, StorageError>;
    fn list(&self, prefix: Option<&str>) -> Result<Vec<StorageRef>, StorageError>;
}

/// Local-filesystem adapter. The bundle directory IS the storage
/// location — `put` is a no-op (the builder already wrote there)
/// and `get` returns the directory back to the caller.
///
/// Construct via [`LocalFs::new`] over the operator's bundle root
/// (e.g. `<data-dir>/evidence/`). The root is the directory under
/// which bundle subdirectories live, one per `run_id`.
#[doc(hidden)]
pub struct LocalFs {
    root: PathBuf,
}

impl LocalFs {
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
/// [`BundleStorage`] adapter. Only the `local:` scheme is
/// supported in this PR; remote schemes return
/// `StorageError::InvalidUri` until the relevant Wave 3 adapter
/// lands.
///
/// `local:<root>` constructs a `LocalFs` rooted at `<root>`.
/// Empty / `None` URI returns `None` so callers can fall back to
/// their existing local-only path.
#[doc(hidden)]
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
    // Reserve the remote schemes so the error message is helpful
    // when an operator points a 1.x cluster at a not-yet-shipped
    // adapter.
    if uri.starts_with("s3://") || uri.starts_with("gs://") || uri.starts_with("azblob://") {
        return Err(StorageError::InvalidUri(format!(
            "{uri} is reserved for a future remote-storage adapter; \
             this Boruna build only supports local:<path>"
        )));
    }
    Err(StorageError::InvalidUri(uri.to_string()))
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

    #[test]
    fn from_uri_remote_schemes_reserve_clear_error() {
        for uri in ["s3://b/p", "gs://b/p", "azblob://c/p"] {
            assert_invalid_uri(from_uri(Some(uri)));
        }
    }

    #[test]
    fn from_uri_unknown_scheme_rejects() {
        assert_invalid_uri(from_uri(Some("ftp://oldschool")));
    }
}
