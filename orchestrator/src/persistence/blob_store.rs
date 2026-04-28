//! Content-addressed blob store for large step outputs.
//!
//! Sprint 0.5-S7. Stores `output_json` bytes whose serialized length
//! exceeds [`crate::persistence::BLOB_THRESHOLD`] under a sharded
//! directory keyed by SHA-256 hash. The hash is precomputed by the
//! caller (it's the same hash already required for the audit chain
//! via [`crate::persistence::StepCheckpoint::output_hash`]).
//!
//! ## Layout
//!
//! ```text
//! <root>/
//! ├── 00/
//! │   └── 00aa11bb22cc...  (64-hex-char SHA-256, no extension)
//! ├── 01/
//! │   └── 0123456789ab...
//! ├── ...
//! └── ff/
//! ```
//!
//! The first two hex characters of the hash form the shard directory.
//! 256 buckets keep `ls` reasonable when blob counts grow into the
//! millions.
//!
//! ## Atomicity
//!
//! Writes use the temp-file + rename pattern. If the process crashes
//! between `write_all` and the rename, only a `.tmp.<rand>` file is
//! left behind; no partial blob is ever visible at its final path.
//! This means a successful return from [`BlobStore::write`] implies
//! the blob is fully on disk and readable.
//!
//! ## Path-traversal hardening
//!
//! Per the project precedent in `LlmCache` and `ContextStore`, the
//! hash parameter is validated as 64 lowercase hex characters BEFORE
//! any filesystem access. Anything else returns
//! [`BlobStoreError::BadHash`] without touching the disk.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Errors from blob store operations.
#[derive(Debug)]
pub enum BlobStoreError {
    /// Hash parameter failed validation. Always rejected before any
    /// filesystem access.
    BadHash,
    /// No blob exists at the requested hash.
    NotFound,
    /// `read_string` was called on a blob whose bytes are not valid
    /// UTF-8. (`output_json` blobs are always UTF-8 by construction;
    /// this would indicate corruption.)
    NotUtf8,
    /// Underlying I/O failure.
    Io(io::Error),
}

impl std::fmt::Display for BlobStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobStoreError::BadHash => f.write_str("blob hash is not 64 lowercase hex chars"),
            BlobStoreError::NotFound => f.write_str("blob not found"),
            BlobStoreError::NotUtf8 => f.write_str("blob bytes are not valid UTF-8"),
            BlobStoreError::Io(e) => write!(f, "blob I/O: {e}"),
        }
    }
}

impl std::error::Error for BlobStoreError {}

impl From<io::Error> for BlobStoreError {
    fn from(e: io::Error) -> Self {
        BlobStoreError::Io(e)
    }
}

/// Sharded content-addressed blob store rooted at a directory.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Open or create a blob store at `root`. Creates the directory
    /// if absent. Sub-shard directories are created on demand by
    /// [`BlobStore::write`].
    pub fn open(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(BlobStore { root })
    }

    /// Path to the blob's final location (does not check existence).
    fn blob_path(&self, hash: &str) -> PathBuf {
        // Caller must validate hash first via `validate_hash`.
        let shard = &hash[..2];
        self.root.join(shard).join(hash)
    }

    /// Write `bytes` to `<root>/<aa>/<hash>` atomically via tempfile +
    /// rename. The hash is taken as-is and is NOT verified to match
    /// the bytes — the caller has it from the audit chain (it's the
    /// same `output_hash` already computed).
    ///
    /// Idempotent: writing the same hash with the same content twice
    /// is a successful no-op modulo I/O.
    ///
    /// **Concurrency:** the tmp filename is unique per writer
    /// (process pid + atomic counter) so two concurrent writers
    /// targeting the same final hash never share a tmp inode.
    /// `rename` is atomic on POSIX same-FS; whichever writer renames
    /// first wins, the other re-renames over an identical content
    /// atomically (the kernel-level rename is benign because the
    /// destination contents are byte-identical — guaranteed by
    /// SHA-256 collision resistance).
    pub fn write(&self, hash: &str, bytes: &[u8]) -> Result<(), BlobStoreError> {
        validate_hash(hash)?;
        let final_path = self.blob_path(hash);
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Idempotent shortcut: if the file already exists at the
        // target hash, our content matches by SHA-256 collision
        // resistance. Skip the rewrite.
        if final_path.exists() {
            return Ok(());
        }
        // Per-writer unique tmp suffix: pid + per-process atomic
        // counter. Avoids the cross-writer same-tmp-inode race that
        // a fixed `<hash>.tmp` would create. The counter is
        // process-local; pid disambiguates across processes.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_path = final_path.with_extension(format!("tmp.{pid}.{seq}"));
        // Open with `write_only + create + truncate`; closing the file
        // (drop of `f`) flushes to OS buffer.
        {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        match fs::rename(&tmp_path, &final_path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                Err(BlobStoreError::Io(e))
            }
        }
    }

    /// Read the blob at `hash` as a UTF-8 string. Returns
    /// [`BlobStoreError::NotFound`] if absent and
    /// [`BlobStoreError::NotUtf8`] if the bytes are not valid UTF-8.
    pub fn read_string(&self, hash: &str) -> Result<String, BlobStoreError> {
        let bytes = self.read_bytes(hash)?;
        String::from_utf8(bytes).map_err(|_| BlobStoreError::NotUtf8)
    }

    /// Read the blob's raw bytes. Used by the coord HTTP route
    /// (octet-stream response) which doesn't need UTF-8 validation.
    pub fn read_bytes(&self, hash: &str) -> Result<Vec<u8>, BlobStoreError> {
        validate_hash(hash)?;
        let path = self.blob_path(hash);
        match File::open(&path) {
            Ok(mut f) => {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                Ok(buf)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(BlobStoreError::NotFound),
            Err(e) => Err(BlobStoreError::Io(e)),
        }
    }

    /// Existence check without read. Used by the dashboard's
    /// "view blob" affordance to render a placeholder when the blob
    /// is missing (cleaned out of band).
    pub fn exists(&self, hash: &str) -> bool {
        if validate_hash(hash).is_err() {
            return false;
        }
        self.blob_path(hash).is_file()
    }

    /// Filesystem root of this store. Useful for diagnostic logging.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Delete the blob at `hash`. Idempotent — `NotFound` is reported
    /// as `Ok(())` rather than an error so a GC sweep that races with
    /// another deleter (or with a manual `rm`) does not abort.
    ///
    /// Sprint `W3-B`. Used by [`crate::audit`] / `boruna evidence
    /// gc-blobs` to reclaim orphan blob files. Also tries to prune the
    /// shard directory `<root>/<aa>/` if it ends up empty after the
    /// delete; failure to prune the shard is logged-and-ignored
    /// (another concurrent writer may have just placed a sibling
    /// blob in it). The blob root itself is never removed.
    ///
    /// Like every other method, validates the hash before touching the
    /// filesystem.
    pub fn delete(&self, hash: &str) -> Result<(), BlobStoreError> {
        validate_hash(hash)?;
        let path = self.blob_path(hash);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(BlobStoreError::Io(e)),
        }
        // Best-effort shard prune. `remove_dir` only succeeds on an
        // empty directory so this is naturally race-safe — if a
        // concurrent writer just placed a sibling, `remove_dir` errors
        // with `ENOTEMPTY` and we simply ignore it.
        if let Some(shard) = path.parent() {
            let _ = fs::remove_dir(shard);
        }
        Ok(())
    }

    /// Walk the blobs/ tree and return every on-disk hash that is NOT
    /// in `referenced`. Used by `boruna evidence gc-blobs` to find
    /// orphan blobs left behind when their referencing checkpoint
    /// row was deleted (e.g., a future `runs prune` command, or a
    /// pre-W3-B run where the checkpoint upsert overwrote
    /// `output_blob_ref` with NULL on a re-attempt).
    ///
    /// Sprint `W3-B`. The walk is rooted at `<root>/<aa>/<bb>` (where
    /// `<aa>` is the 2-char shard prefix and `<bb>` is the full 64-char
    /// hash filename). Anything else encountered in the tree (a
    /// stray file, a `.tmp.<pid>.<seq>` from a crashed writer, a
    /// non-shard-shaped directory) is **skipped, not an error** — the
    /// goal is to report orphan content-addressed blobs, not to act
    /// as a tree-shape linter.
    ///
    /// The returned vector is in deterministic ascending order
    /// (BTreeSet iteration order). Caller is responsible for the
    /// TOCTOU window between this read and any subsequent
    /// [`Self::delete`] — see `boruna evidence gc-blobs` for the
    /// exclusive-lock strategy.
    pub fn find_orphans(
        &self,
        referenced: &BTreeSet<String>,
    ) -> Result<Vec<String>, BlobStoreError> {
        let mut on_disk: BTreeSet<String> = BTreeSet::new();
        // The blobs/ root may not exist yet on a brand-new data-dir
        // that has never written a >64KiB output. Treat that as
        // "no orphans" rather than an error.
        let shard_iter = match fs::read_dir(&self.root) {
            Ok(it) => it,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(BlobStoreError::Io(e)),
        };
        for shard_entry in shard_iter {
            let shard_entry = shard_entry?;
            let shard_path = shard_entry.path();
            // Only descend into 2-char hex shard directories. Skip
            // stray files at root, non-hex names, and anything with
            // an unexpected length.
            if !shard_path.is_dir() {
                continue;
            }
            let shard_name = match shard_entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };
            if shard_name.len() != 2 || !is_hex_lower(&shard_name) {
                continue;
            }
            for blob_entry in fs::read_dir(&shard_path)? {
                let blob_entry = blob_entry?;
                if !blob_entry.file_type()?.is_file() {
                    continue;
                }
                let name = match blob_entry.file_name().into_string() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                // Only count files whose name is itself a valid
                // 64-char lowercase hex string AND lives under the
                // matching shard. A file at <root>/aa/bbbb…bbbb
                // (shard "aa", hash starts "bb") is structural noise,
                // not a referenced blob.
                if validate_hash(&name).is_err() {
                    continue;
                }
                if name[..2] != shard_name {
                    continue;
                }
                on_disk.insert(name);
            }
        }
        Ok(on_disk.difference(referenced).cloned().collect())
    }
}

/// Helper used by [`BlobStore::find_orphans`] to filter shard
/// directories. Mirrors the constraint inside [`validate_hash`] but
/// for arbitrary-length strings.
fn is_hex_lower(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Validate a hash string: exactly 64 lowercase hex characters.
/// Returns [`BlobStoreError::BadHash`] otherwise.
fn validate_hash(hash: &str) -> Result<(), BlobStoreError> {
    if hash.len() != 64 {
        return Err(BlobStoreError::BadHash);
    }
    if !hash
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(BlobStoreError::BadHash);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_root() -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("boruna-blob-test-{pid}-{n}"))
    }

    fn cleanup(p: &Path) {
        let _ = fs::remove_dir_all(p);
    }

    const A64: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B64: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn write_then_read_string_roundtrip() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        let payload = "hello world".to_string();
        store.write(A64, payload.as_bytes()).unwrap();
        let got = store.read_string(A64).unwrap();
        assert_eq!(got, payload);
        cleanup(&root);
    }

    #[test]
    fn write_then_read_bytes_roundtrip() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        let bytes: Vec<u8> = (0..=255u8).collect();
        store.write(A64, &bytes).unwrap();
        let got = store.read_bytes(A64).unwrap();
        assert_eq!(got, bytes);
        cleanup(&root);
    }

    #[test]
    fn bad_hash_rejected_on_write() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        let err = store.write("not-hex-at-all", b"x").unwrap_err();
        assert!(matches!(err, BlobStoreError::BadHash));
        cleanup(&root);
    }

    #[test]
    fn bad_hash_rejected_on_read() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        let err = store.read_string("../etc/passwd").unwrap_err();
        assert!(matches!(err, BlobStoreError::BadHash));
        cleanup(&root);
    }

    #[test]
    fn bad_hash_traversal_dots() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        // Exactly 64 chars but contains slashes (and dots) → rejected.
        // Padding the dotted-path with `a`s to hit exactly 64.
        let bad = "../etc/passwd/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(bad.len(), 64);
        let err = store.write(bad, b"x").unwrap_err();
        assert!(matches!(err, BlobStoreError::BadHash));
        cleanup(&root);
    }

    #[test]
    fn bad_hash_uppercase() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        let bad = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let err = store.write(bad, b"x").unwrap_err();
        assert!(matches!(err, BlobStoreError::BadHash));
        cleanup(&root);
    }

    #[test]
    fn bad_hash_short() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        let bad = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; // 63
        let err = store.write(bad, b"x").unwrap_err();
        assert!(matches!(err, BlobStoreError::BadHash));
        cleanup(&root);
    }

    #[test]
    fn bad_hash_long() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        let bad = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; // 65
        let err = store.write(bad, b"x").unwrap_err();
        assert!(matches!(err, BlobStoreError::BadHash));
        cleanup(&root);
    }

    #[test]
    fn not_found_returns_typed_error() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        let err = store.read_string(A64).unwrap_err();
        assert!(matches!(err, BlobStoreError::NotFound));
        cleanup(&root);
    }

    #[test]
    fn idempotent_rewrite_same_hash() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        store.write(A64, b"hello").unwrap();
        store.write(A64, b"hello").unwrap();
        let got = store.read_string(A64).unwrap();
        assert_eq!(got, "hello");
        cleanup(&root);
    }

    #[test]
    fn exists_returns_true_after_write() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        store.write(A64, b"x").unwrap();
        assert!(store.exists(A64));
        cleanup(&root);
    }

    #[test]
    fn exists_returns_false_for_missing() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        assert!(!store.exists(B64));
        cleanup(&root);
    }

    #[test]
    fn exists_returns_false_for_bad_hash() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        assert!(!store.exists("not-a-hash"));
        cleanup(&root);
    }

    #[test]
    fn not_utf8_returns_typed_error() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        // Invalid UTF-8 byte sequence
        let bytes = vec![0xff, 0xfe, 0xfd];
        store.write(A64, &bytes).unwrap();
        let err = store.read_string(A64).unwrap_err();
        assert!(matches!(err, BlobStoreError::NotUtf8));
        cleanup(&root);
    }

    #[test]
    fn shard_directory_created_on_demand() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        store.write(A64, b"x").unwrap();
        // Shard "aa" should exist
        assert!(root.join("aa").is_dir());
        // The blob should be at <root>/aa/<full hash>
        assert!(root.join("aa").join(A64).is_file());
        cleanup(&root);
    }

    // ────────────────────────────────────────────────────────────
    // Sprint W3-B: blob GC primitives.
    // ────────────────────────────────────────────────────────────

    const C64: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    #[test]
    fn delete_removes_blob_and_returns_ok() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        store.write(A64, b"x").unwrap();
        assert!(store.exists(A64));
        store.delete(A64).unwrap();
        assert!(!store.exists(A64));
        cleanup(&root);
    }

    #[test]
    fn delete_is_idempotent_for_missing_hash() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        // Never wrote anything. Delete must succeed (not NotFound).
        store.delete(A64).unwrap();
        // Second call also Ok.
        store.delete(A64).unwrap();
        cleanup(&root);
    }

    #[test]
    fn delete_rejects_bad_hash() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        let err = store.delete("not-a-hash").unwrap_err();
        assert!(matches!(err, BlobStoreError::BadHash));
        cleanup(&root);
    }

    #[test]
    fn delete_prunes_empty_shard_directory() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        store.write(A64, b"x").unwrap();
        assert!(root.join("aa").is_dir());
        store.delete(A64).unwrap();
        // Shard "aa" should now be gone.
        assert!(!root.join("aa").exists());
        cleanup(&root);
    }

    #[test]
    fn delete_keeps_shard_when_sibling_remains() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        // Two blobs sharing shard "aa": A64 and another aa-prefixed hash.
        let a_sibling = "aa00000000000000000000000000000000000000000000000000000000000000";
        store.write(A64, b"x").unwrap();
        store.write(a_sibling, b"y").unwrap();
        store.delete(A64).unwrap();
        assert!(root.join("aa").is_dir());
        assert!(root.join("aa").join(a_sibling).is_file());
        cleanup(&root);
    }

    #[test]
    fn find_orphans_returns_empty_when_all_blobs_referenced() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        store.write(A64, b"a").unwrap();
        store.write(B64, b"b").unwrap();
        let mut referenced = BTreeSet::new();
        referenced.insert(A64.to_string());
        referenced.insert(B64.to_string());
        let orphans = store.find_orphans(&referenced).unwrap();
        assert!(orphans.is_empty(), "expected no orphans, got {orphans:?}");
        cleanup(&root);
    }

    #[test]
    fn find_orphans_returns_empty_for_fresh_root_without_blobs_dir() {
        // A data-dir whose blobs/ subdir was never created should be
        // treated as "no orphans", not an I/O error.
        let root = unique_root();
        // Don't open via BlobStore::open (which create_dir_alls) —
        // construct one against a non-existent path. We have to use
        // open() to keep field privacy, but immediately delete the
        // dir to simulate the never-written case.
        let store = BlobStore::open(root.clone()).unwrap();
        cleanup(&root);
        let referenced = BTreeSet::new();
        let orphans = store.find_orphans(&referenced).unwrap();
        assert!(orphans.is_empty());
    }

    #[test]
    fn find_orphans_returns_unreferenced_hashes() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        store.write(A64, b"a").unwrap();
        store.write(B64, b"b").unwrap();
        store.write(C64, b"c").unwrap();
        let mut referenced = BTreeSet::new();
        referenced.insert(A64.to_string()); // only A is referenced
        let mut orphans = store.find_orphans(&referenced).unwrap();
        orphans.sort();
        assert_eq!(orphans, vec![B64.to_string(), C64.to_string()]);
        cleanup(&root);
    }

    #[test]
    fn find_orphans_skips_unrelated_files_and_dirs() {
        let root = unique_root();
        let store = BlobStore::open(root.clone()).unwrap();
        store.write(A64, b"a").unwrap();
        // Stray junk at root and inside a valid shard. None should
        // be reported as orphans.
        std::fs::write(root.join("README"), "not a blob").unwrap();
        std::fs::create_dir_all(root.join("not-hex")).unwrap();
        std::fs::write(root.join("aa").join("not-a-hash"), "junk").unwrap();
        // A leftover .tmp file from a crashed writer.
        std::fs::write(root.join("aa").join(format!("{A64}.tmp.123.0")), "crash").unwrap();
        // A blob in the wrong shard (shard "bb" but hash starts "aa").
        std::fs::create_dir_all(root.join("bb")).unwrap();
        std::fs::write(root.join("bb").join(A64), "wrong shard").unwrap();

        let mut referenced = BTreeSet::new();
        referenced.insert(A64.to_string());
        let orphans = store.find_orphans(&referenced).unwrap();
        assert!(orphans.is_empty(), "unexpected orphans: {orphans:?}");
        cleanup(&root);
    }
}
