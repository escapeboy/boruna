use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Content-addressed blob store for LLM context.
pub struct ContextStore {
    blobs_dir: PathBuf,
}

impl ContextStore {
    /// Open (or create) a context store at `base_dir`.
    pub fn open(base_dir: &Path) -> Result<Self, String> {
        let blobs_dir = base_dir.join("blobs");
        fs::create_dir_all(&blobs_dir).map_err(|e| format!("cannot create blobs dir: {e}"))?;
        Ok(ContextStore { blobs_dir })
    }

    /// Store content and return its hash.
    pub fn put(&self, content: &str) -> Result<String, String> {
        let hash = sha256_hex(content);
        let path = self.blobs_dir.join(&hash);
        fs::write(&path, content).map_err(|e| format!("write error: {e}"))?;
        Ok(hash)
    }

    /// Retrieve content by hash.
    pub fn get(&self, hash: &str) -> Result<String, String> {
        let path = self.blobs_dir.join(hash);
        fs::read_to_string(&path).map_err(|e| format!("blob not found ({hash}): {e}"))
    }

    /// Check if a blob exists.
    pub fn exists(&self, hash: &str) -> bool {
        self.blobs_dir.join(hash).exists()
    }

    /// Pack multiple blobs into a bounded context, respecting `max_bytes`.
    /// Returns included blobs in the order provided (stable).
    /// If `max_bytes` is 0, no limit is applied.
    pub fn pack(&self, hashes: &[String], max_bytes: u64) -> Result<Vec<(String, String)>, String> {
        let mut result = Vec::new();
        let mut total_bytes = 0u64;

        for hash in hashes {
            let content = self.get(hash)?;
            let content_len = content.len() as u64;

            if max_bytes > 0 && total_bytes + content_len > max_bytes {
                break;
            }

            total_bytes += content_len;
            result.push((hash.clone(), content));
        }

        Ok(result)
    }

    /// List all blob hashes in the store.
    pub fn list(&self) -> Result<Vec<String>, String> {
        let mut hashes = Vec::new();
        let entries = fs::read_dir(&self.blobs_dir).map_err(|e| format!("read dir error: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("entry error: {e}"))?;
            if let Some(name) = entry.file_name().to_str() {
                hashes.push(name.to_string());
            }
        }
        hashes.sort();
        Ok(hashes)
    }

    /// Total size of all blobs in bytes.
    pub fn total_size(&self) -> Result<u64, String> {
        let mut total = 0u64;
        let entries = fs::read_dir(&self.blobs_dir).map_err(|e| format!("read dir error: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("entry error: {e}"))?;
            let meta = entry
                .metadata()
                .map_err(|e| format!("metadata error: {e}"))?;
            total += meta.len();
        }
        Ok(total)
    }
}

fn sha256_hex(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn test_put_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextStore::open(dir.path()).unwrap();

        let hash = store.put("hello world").unwrap();
        assert!(!hash.is_empty());

        let content = store.get(&hash).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_put_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextStore::open(dir.path()).unwrap();

        let h1 = store.put("same content").unwrap();
        let h2 = store.put("same content").unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_exists() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextStore::open(dir.path()).unwrap();

        assert!(!store.exists("nonexistent"));
        let hash = store.put("data").unwrap();
        assert!(store.exists(&hash));
    }

    #[test]
    fn test_get_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextStore::open(dir.path()).unwrap();

        let result = store.get("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_pack_no_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextStore::open(dir.path()).unwrap();

        let h1 = store.put("aaa").unwrap();
        let h2 = store.put("bbb").unwrap();

        let packed = store.pack(&[h1.clone(), h2.clone()], 0).unwrap();
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].1, "aaa");
        assert_eq!(packed[1].1, "bbb");
    }

    #[test]
    fn test_pack_with_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextStore::open(dir.path()).unwrap();

        let h1 = store.put("aaa").unwrap(); // 3 bytes
        let h2 = store.put("bbb").unwrap(); // 3 bytes

        let packed = store.pack(&[h1, h2], 5).unwrap();
        assert_eq!(packed.len(), 1); // only first fits in 5 bytes
        assert_eq!(packed[0].1, "aaa");
    }

    #[test]
    fn test_list_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextStore::open(dir.path()).unwrap();

        store.put("one").unwrap();
        store.put("two").unwrap();

        let hashes = store.list().unwrap();
        assert_eq!(hashes.len(), 2);
    }

    #[test]
    fn test_total_size() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextStore::open(dir.path()).unwrap();

        store.put("123").unwrap(); // 3 bytes
        store.put("4567").unwrap(); // 4 bytes

        let size = store.total_size().unwrap();
        assert_eq!(size, 7);
    }
}
