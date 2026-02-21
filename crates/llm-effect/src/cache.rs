use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json;

use boruna_bytecode::Value;

/// A cached LLM response entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub cache_key: String,
    pub prompt_id: String,
    pub model: String,
    pub schema_id: String,
    pub prompt_hash: String,
    pub result: Value,
}

/// Deterministic LLM cache backed by the filesystem.
pub struct LlmCache {
    cache_dir: PathBuf,
}

impl LlmCache {
    /// Open (or create) a cache at `cache_dir`.
    pub fn open(cache_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(cache_dir).map_err(|e| format!("cannot create cache dir: {e}"))?;
        Ok(LlmCache {
            cache_dir: cache_dir.to_path_buf(),
        })
    }

    /// Derive the filename for a cache key.
    fn entry_path(&self, cache_key: &str) -> PathBuf {
        // Strip "sha256:" prefix for filename
        let hex = cache_key.strip_prefix("sha256:").unwrap_or(cache_key);
        self.cache_dir.join(format!("{hex}.json"))
    }

    /// Read a cached entry by cache key.
    pub fn read(&self, cache_key: &str) -> Option<CacheEntry> {
        let path = self.entry_path(cache_key);
        let data = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Write a cache entry. Uses canonical JSON for determinism.
    pub fn write(&self, entry: &CacheEntry) -> Result<(), String> {
        let path = self.entry_path(&entry.cache_key);
        // Use sorted keys via BTreeMap-based serialization (serde_json sorts by default with BTreeMap)
        let json = canonical_serialize(entry)?;
        fs::write(&path, json).map_err(|e| format!("cache write error: {e}"))
    }

    /// Check if a cache entry exists.
    pub fn exists(&self, cache_key: &str) -> bool {
        self.entry_path(cache_key).exists()
    }

    /// Delete a cache entry.
    pub fn delete(&self, cache_key: &str) -> Result<(), String> {
        let path = self.entry_path(cache_key);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("cache delete error: {e}"))?;
        }
        Ok(())
    }

    /// List all cache keys.
    pub fn list_keys(&self) -> Result<Vec<String>, String> {
        let mut keys = Vec::new();
        let entries = fs::read_dir(&self.cache_dir).map_err(|e| format!("read dir error: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("entry error: {e}"))?;
            if let Some(name) = entry.file_name().to_str() {
                if let Some(hex) = name.strip_suffix(".json") {
                    keys.push(format!("sha256:{hex}"));
                }
            }
        }
        keys.sort();
        Ok(keys)
    }

    /// Clear the entire cache.
    pub fn clear(&self) -> Result<usize, String> {
        let mut count = 0;
        let entries = fs::read_dir(&self.cache_dir).map_err(|e| format!("read dir error: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("entry error: {e}"))?;
            if entry.path().extension().and_then(|e| e.to_str()) == Some("json") {
                fs::remove_file(entry.path()).map_err(|e| format!("delete error: {e}"))?;
                count += 1;
            }
        }
        Ok(count)
    }
}

/// Serialize a CacheEntry to canonical JSON (sorted keys).
fn canonical_serialize(entry: &CacheEntry) -> Result<String, String> {
    // Convert to a BTreeMap for sorted keys
    let mut map = BTreeMap::new();
    map.insert(
        "cache_key".to_string(),
        serde_json::to_value(&entry.cache_key).unwrap(),
    );
    map.insert(
        "model".to_string(),
        serde_json::to_value(&entry.model).unwrap(),
    );
    map.insert(
        "prompt_hash".to_string(),
        serde_json::to_value(&entry.prompt_hash).unwrap(),
    );
    map.insert(
        "prompt_id".to_string(),
        serde_json::to_value(&entry.prompt_id).unwrap(),
    );
    map.insert(
        "result".to_string(),
        serde_json::to_value(&entry.result).unwrap(),
    );
    map.insert(
        "schema_id".to_string(),
        serde_json::to_value(&entry.schema_id).unwrap(),
    );

    serde_json::to_string_pretty(&map).map_err(|e| format!("serialize error: {e}"))
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn make_entry(key: &str) -> CacheEntry {
        CacheEntry {
            cache_key: format!("sha256:{key}"),
            prompt_id: "test.prompt".into(),
            model: "default".into(),
            schema_id: "json_object".into(),
            prompt_hash: "sha256:abc".into(),
            result: Value::Map({
                let mut m = BTreeMap::new();
                m.insert("answer".into(), Value::String("42".into()));
                m
            }),
        }
    }

    #[test]
    fn test_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LlmCache::open(dir.path()).unwrap();

        let entry = make_entry("abc123");
        cache.write(&entry).unwrap();

        let loaded = cache.read("sha256:abc123").unwrap();
        assert_eq!(loaded.cache_key, "sha256:abc123");
        assert_eq!(loaded.prompt_id, "test.prompt");
        assert_eq!(loaded.result, entry.result);
    }

    #[test]
    fn test_read_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LlmCache::open(dir.path()).unwrap();
        assert!(cache.read("sha256:nonexistent").is_none());
    }

    #[test]
    fn test_exists() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LlmCache::open(dir.path()).unwrap();

        assert!(!cache.exists("sha256:abc"));
        cache.write(&make_entry("abc")).unwrap();
        assert!(cache.exists("sha256:abc"));
    }

    #[test]
    fn test_delete() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LlmCache::open(dir.path()).unwrap();

        cache.write(&make_entry("abc")).unwrap();
        assert!(cache.exists("sha256:abc"));

        cache.delete("sha256:abc").unwrap();
        assert!(!cache.exists("sha256:abc"));
    }

    #[test]
    fn test_list_keys() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LlmCache::open(dir.path()).unwrap();

        cache.write(&make_entry("aaa")).unwrap();
        cache.write(&make_entry("bbb")).unwrap();

        let keys = cache.list_keys().unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"sha256:aaa".to_string()));
        assert!(keys.contains(&"sha256:bbb".to_string()));
    }

    #[test]
    fn test_clear() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LlmCache::open(dir.path()).unwrap();

        cache.write(&make_entry("aaa")).unwrap();
        cache.write(&make_entry("bbb")).unwrap();

        let count = cache.clear().unwrap();
        assert_eq!(count, 2);
        assert_eq!(cache.list_keys().unwrap().len(), 0);
    }

    #[test]
    fn test_canonical_serialization_deterministic() {
        let entry = make_entry("det_test");
        let json1 = canonical_serialize(&entry).unwrap();
        let json2 = canonical_serialize(&entry).unwrap();
        assert_eq!(json1, json2);
        // Verify keys are sorted
        let first_key_pos = json1.find("\"cache_key\"").unwrap();
        let second_key_pos = json1.find("\"model\"").unwrap();
        assert!(first_key_pos < second_key_pos);
    }

    #[test]
    fn test_same_request_same_cache_key() {
        // The cache key is passed in, so same key -> same file
        let dir = tempfile::tempdir().unwrap();
        let cache = LlmCache::open(dir.path()).unwrap();

        let entry = make_entry("same_key");
        cache.write(&entry).unwrap();

        let loaded = cache.read("sha256:same_key").unwrap();
        assert_eq!(loaded.result, entry.result);
    }
}
