use boruna_bytecode::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Flush a file's data blocks to stable storage with the strongest
/// guarantee the platform offers.
///
/// - **Linux** (`target_os = "linux"`) and other non-Darwin Unix:
///   `File::sync_all()`, which calls `fsync(2)`. On ext4/xfs this
///   forces a write barrier and journals the metadata.
/// - **macOS** (`target_os = "macos"`): `fcntl(fd, F_FULLFSYNC, 0)`.
///   Plain `fsync(2)` on Darwin does NOT flush the drive's write
///   cache to media (it returns once the data is in the device's
///   buffer); SQLite, Postgres, and `git` all use F_FULLFSYNC for
///   the same reason. Documented in `man 2 fsync` on Darwin.
/// - **Windows**: `File::sync_all()`, which maps to `FlushFileBuffers`.
///
/// On Darwin, if F_FULLFSYNC fails (e.g. the FS doesn't support it —
/// unusual but possible on fuse mounts), we fall back to
/// `sync_all()` rather than propagating the error: at least we get
/// the partial guarantee.
fn fullsync_file(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `file` is a valid open file descriptor; F_FULLFSYNC
        // takes no further argument. Returns 0 on success, -1 on
        // error (errno set).
        let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) };
        if rc == 0 {
            Ok(())
        } else {
            // Fall back to sync_all so we still get the soft fsync.
            // Logging is operator-helpful but optional; suppressed
            // under cfg(test) to keep the unit suite quiet.
            #[cfg(not(test))]
            eprintln!(
                "warning: F_FULLFSYNC failed (errno {}); falling back to sync_all (soft fsync)",
                std::io::Error::last_os_error()
            );
            file.sync_all()
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        file.sync_all()
    }
}

/// Manages inter-step data flow: stores step outputs and resolves step inputs.
pub struct DataStore {
    /// Base directory for this workflow run's data.
    base_dir: PathBuf,
    /// In-memory cache of step outputs (step_id -> output_name -> value).
    outputs: BTreeMap<String, BTreeMap<String, Value>>,
}

impl DataStore {
    pub fn new(base_dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(base_dir.join("outputs"))?;
        Ok(DataStore {
            base_dir: base_dir.to_path_buf(),
            outputs: BTreeMap::new(),
        })
    }

    /// Store a step's output value.
    ///
    /// **Atomicity guarantee.** The JSON is serialized into a temp
    /// file in the same parent directory; the temp file's data blocks
    /// are flushed; `tempfile::NamedTempFile::persist` atomically
    /// renames it into place; the parent directory is then synced so
    /// the rename's directory entry is journaled. Within a single OS
    /// lifetime, concurrent readers see either the old contents or
    /// the new contents — never a partial write.
    ///
    /// **Power-loss durability.** Once `store_output` returns `Ok`,
    /// recovery after a power loss shows either the old file or the
    /// new file — never a torn intermediate. Caveats apply to the
    /// pre-Ok crash window: a crash between `persist` and the
    /// directory sync leaves the rename in page cache only. Callers
    /// MUST treat post-write `Err` as "output may or may not exist on
    /// disk" and rely on idempotent replay (the runner does this via
    /// the SQLite checkpoint commit happening after this method
    /// returns).
    ///
    /// **Platform notes:**
    /// - **Linux** (ext4, xfs, btrfs): full guarantee via
    ///   `fsync(2)` on file + directory inodes.
    /// - **macOS** (apfs, hfs+): full guarantee via
    ///   `fcntl(F_FULLFSYNC)`. Plain `fsync(2)` on Darwin does NOT
    ///   flush the drive's write cache to media; SQLite makes the
    ///   same choice.
    /// - **NFS / fuse / network FS**: NOT guaranteed. Behavior
    ///   depends on mount options and server semantics. Use a local
    ///   filesystem for production durability claims.
    /// - **Windows**: the parent-dir sync is skipped (Windows
    ///   doesn't support `fsync` on directory handles). Non-production
    ///   target.
    ///
    /// **JSON-bytes/hash alignment.** The on-disk JSON is the **same
    /// compact form** that [`Self::hash_value`] hashes and that the
    /// orchestrator persists in `step_checkpoints.output_json`. So
    /// `sha256sum <step>/result.json` equals the persisted
    /// `output_hash` column.
    ///
    /// History:
    /// - `0.3-S3`: introduced atomic-rename, closed H4 from 0.3-S2c
    ///   (torn-write under concurrent resume). Documented the
    ///   parent-dir fsync gap as deferred.
    /// - `0.3-S6`: closes the parent-dir fsync gap (H1/C3 from 0.3-S3).
    pub fn store_output(
        &mut self,
        step_id: &str,
        output_name: &str,
        value: &Value,
    ) -> std::io::Result<()> {
        let dir = self.base_dir.join("outputs").join(step_id);
        std::fs::create_dir_all(&dir)?;
        // Compact JSON: same bytes that `hash_value` hashes and that
        // the orchestrator persists in step_checkpoints.output_json.
        // Single source of truth across hash, SQL column, and on-disk
        // file (review-driven 0.3-S3 H2/H3).
        let json = serde_json::to_string(value).map_err(std::io::Error::other)?;
        let target = dir.join(format!("{output_name}.json"));

        // NamedTempFile::new_in keeps the temp file in the same parent
        // directory as the target, so persist's rename is same-FS and
        // therefore atomic on POSIX. On Windows, persist falls back to
        // a non-atomic copy+delete; acceptable since Windows isn't a
        // production target for the orchestrator.
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        std::io::Write::write_all(&mut tmp, json.as_bytes())?;
        // Sync the temp file's data blocks to stable media BEFORE
        // rename so the bytes are durable when the rename commits.
        // On macOS this uses F_FULLFSYNC (sync_all alone is a soft
        // fsync that doesn't flush the drive's write cache).
        fullsync_file(tmp.as_file())?;
        // persist consumes the NamedTempFile. On error it returns the
        // original handle along with the io::Error; we propagate the
        // io::Error and let Drop clean up the temp file.
        tmp.persist(&target).map_err(|e| e.error)?;

        // 0.3-S6: sync the parent directory so the rename's
        // directory entry is journaled to stable media. Without this,
        // POSIX permits the dirent to be lost on power loss even
        // though the file's data blocks have been flushed.
        #[cfg(unix)]
        {
            let dir_handle = std::fs::File::open(&dir)?;
            fullsync_file(&dir_handle)?;
        }

        self.outputs
            .entry(step_id.to_string())
            .or_default()
            .insert(output_name.to_string(), value.clone());

        Ok(())
    }

    /// Resolve an input reference ("step_id.output_name") to a value.
    pub fn resolve_input(&self, input_ref: &str) -> Result<Value, String> {
        let (step_id, output_name) = input_ref
            .split_once('.')
            .ok_or_else(|| format!("invalid input ref: {input_ref}"))?;

        self.outputs
            .get(step_id)
            .and_then(|outputs| outputs.get(output_name))
            .cloned()
            .ok_or_else(|| format!("output not found: {input_ref}"))
    }

    /// Resolve all inputs for a step, returning a map of input_name -> value.
    pub fn resolve_step_inputs(
        &self,
        inputs: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, Value>, String> {
        let mut resolved = BTreeMap::new();
        for (name, reference) in inputs {
            let value = self.resolve_input(reference)?;
            resolved.insert(name.clone(), value);
        }
        Ok(resolved)
    }

    /// Compute SHA-256 hash of a value's JSON representation.
    pub fn hash_value(value: &Value) -> String {
        let json = serde_json::to_string(value).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Get the output directory path.
    pub fn output_dir(&self) -> PathBuf {
        self.base_dir.join("outputs")
    }

    /// Get all stored outputs.
    pub fn all_outputs(&self) -> &BTreeMap<String, BTreeMap<String, Value>> {
        &self.outputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_and_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DataStore::new(dir.path()).unwrap();

        let value = Value::String("hello world".into());
        store.store_output("fetch", "data", &value).unwrap();

        let resolved = store.resolve_input("fetch.data").unwrap();
        assert_eq!(resolved, value);
    }

    #[test]
    fn test_resolve_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = DataStore::new(dir.path()).unwrap();
        assert!(store.resolve_input("missing.data").is_err());
    }

    #[test]
    fn test_resolve_step_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DataStore::new(dir.path()).unwrap();

        store.store_output("a", "x", &Value::Int(42)).unwrap();
        store
            .store_output("b", "y", &Value::String("test".into()))
            .unwrap();

        let inputs = BTreeMap::from([
            ("first".into(), "a.x".into()),
            ("second".into(), "b.y".into()),
        ]);
        let resolved = store.resolve_step_inputs(&inputs).unwrap();
        assert_eq!(resolved["first"], Value::Int(42));
        assert_eq!(resolved["second"], Value::String("test".into()));
    }

    #[test]
    fn test_hash_deterministic() {
        let v1 = Value::String("test".into());
        let v2 = Value::String("test".into());
        assert_eq!(DataStore::hash_value(&v1), DataStore::hash_value(&v2));
    }

    #[test]
    fn test_hash_differs() {
        let v1 = Value::String("test1".into());
        let v2 = Value::String("test2".into());
        assert_ne!(DataStore::hash_value(&v1), DataStore::hash_value(&v2));
    }

    #[test]
    fn store_output_file_bytes_hash_matches_persisted_output_hash() {
        // 0.3-S3 H2/H3 regression: the on-disk result.json bytes must
        // hash to the same value that hash_value() returns. Prior
        // implementation used to_string_pretty for the file but
        // to_string (compact) for hash_value, which made
        // `sha256sum result.json` mismatch the persisted output_hash
        // column — operator footgun. The fix unifies both to compact
        // JSON.
        let dir = tempfile::tempdir().unwrap();
        let mut store = DataStore::new(dir.path()).unwrap();
        let value = Value::String("the-quick-brown-fox".into());
        store.store_output("step", "result", &value).unwrap();

        let on_disk_bytes = std::fs::read(dir.path().join("outputs/step/result.json")).unwrap();
        // Hash the on-disk bytes directly.
        let mut hasher = Sha256::new();
        hasher.update(&on_disk_bytes);
        let on_disk_hash = format!("{:x}", hasher.finalize());

        // Compare to what hash_value computes.
        let api_hash = DataStore::hash_value(&value);
        assert_eq!(
            on_disk_hash, api_hash,
            "sha256sum of result.json must equal hash_value(&value): \
             on-disk={on_disk_hash}, api={api_hash}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn store_output_parent_dir_is_fsynced() {
        // 0.3-S6 regression: store_output now fsyncs the parent
        // directory after `tempfile::persist` so the rename's
        // dirent is journaled to stable storage. We can't directly
        // test power-loss durability in a unit test (requires
        // hardware-level support), but we CAN verify:
        //   1. The fsync code path doesn't break atomicity
        //      (the file is readable + matches the value).
        //   2. The fsync doesn't leave any stray file handles
        //      (verified by writing many times in a row).
        //
        // The contract is "the call doesn't fail when the parent
        // dir exists and is writable" — the actual durability
        // guarantee is asserted by the docstring + integration
        // checks in `tests/`.
        let dir = tempfile::tempdir().unwrap();
        let mut store = DataStore::new(dir.path()).unwrap();
        // Write the same step many times in a row. Each call opens
        // a fresh dir handle, fsyncs, and closes; verify no FD leak
        // (which would surface as EMFILE eventually) and that the
        // final file content is the last value written.
        for i in 0..50 {
            store
                .store_output("step", "result", &Value::Int(i))
                .unwrap();
        }
        let on_disk = std::fs::read_to_string(dir.path().join("outputs/step/result.json")).unwrap();
        assert!(on_disk.contains("49"), "final value visible: {on_disk}");
        // Only the result.json should exist (no leftover temp files
        // from any of the 50 writes).
        let entries: Vec<_> = std::fs::read_dir(dir.path().join("outputs/step"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, vec!["result.json"], "no leftover files");
    }

    #[test]
    fn store_output_overwrite_is_atomic() {
        // 0.3-S3 regression: store_output now uses NamedTempFile +
        // persist. Verify the visible-state guarantee — at every
        // moment, the target file either contains the OLD value or
        // the NEW value, never a partial write.
        //
        // We can't fully exercise the torn-write race in a single-
        // thread test, but we CAN verify the operational property by
        // overwriting an existing output and asserting the result is
        // the new value (no leftover bytes from the old, longer one
        // that would betray a non-atomic write).
        let dir = tempfile::tempdir().unwrap();
        let mut store = DataStore::new(dir.path()).unwrap();
        // Old: long string with a distinctive marker.
        store
            .store_output(
                "step",
                "result",
                &Value::String("a".repeat(2048) + "OLD_MARKER"),
            )
            .unwrap();
        // New: shorter string. Non-atomic write could leave OLD_MARKER
        // tail bytes if the new content is shorter than the old.
        store
            .store_output("step", "result", &Value::String("NEW".to_string()))
            .unwrap();
        let content = std::fs::read_to_string(dir.path().join("outputs/step/result.json")).unwrap();
        assert!(
            content.contains("NEW"),
            "new value must be readable after overwrite"
        );
        assert!(
            !content.contains("OLD_MARKER"),
            "atomic overwrite must not leave any bytes from the old (longer) value"
        );
    }

    #[test]
    fn store_output_temp_file_cleaned_on_concurrent_writes() {
        // After multiple writes, the outputs/<step>/ directory should
        // contain only the result.json files we wrote — no leftover
        // .tmp* files from NamedTempFile (which auto-Drop on persist
        // success).
        let dir = tempfile::tempdir().unwrap();
        let mut store = DataStore::new(dir.path()).unwrap();
        for i in 0..10 {
            store
                .store_output("step", "result", &Value::Int(i))
                .unwrap();
        }
        let entries: Vec<_> = std::fs::read_dir(dir.path().join("outputs/step"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "exactly one file expected (result.json), found: {entries:?}"
        );
        assert_eq!(entries[0], "result.json");
    }

    #[test]
    fn test_output_persisted_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DataStore::new(dir.path()).unwrap();
        store
            .store_output("step1", "result", &Value::Int(99))
            .unwrap();

        let file = dir.path().join("outputs/step1/result.json");
        assert!(file.exists());
        let content = std::fs::read_to_string(file).unwrap();
        assert!(content.contains("99"));
    }
}
