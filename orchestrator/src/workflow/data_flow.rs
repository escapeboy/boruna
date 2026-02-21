use boruna_bytecode::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    pub fn store_output(
        &mut self,
        step_id: &str,
        output_name: &str,
        value: &Value,
    ) -> std::io::Result<()> {
        let dir = self.base_dir.join("outputs").join(step_id);
        std::fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
        std::fs::write(dir.join(format!("{output_name}.json")), &json)?;

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
