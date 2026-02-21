use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A prompt template stored in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub id: String,
    pub version: String,
    pub template: String,
    pub parameters: Vec<String>,
    pub default_model: String,
    pub default_max_tokens: u64,
    pub default_temperature: u64,
    pub default_schema_id: String,
}

/// Registry entry metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptEntry {
    pub file: String,
    pub content_hash: String,
    pub version: String,
}

/// Schema entry metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaEntry {
    pub file: String,
    pub content_hash: String,
}

/// The prompt registry manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryManifest {
    pub version: u32,
    pub prompts: BTreeMap<String, PromptEntry>,
    pub schemas: BTreeMap<String, SchemaEntry>,
}

impl Default for RegistryManifest {
    fn default() -> Self {
        RegistryManifest {
            version: 1,
            prompts: BTreeMap::new(),
            schemas: BTreeMap::new(),
        }
    }
}

/// The prompt registry — loads and manages prompts and schemas.
pub struct PromptRegistry {
    base_dir: PathBuf,
    manifest: RegistryManifest,
}

impl PromptRegistry {
    /// Open (or create) a prompt registry at `base_dir`.
    pub fn open(base_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(base_dir).map_err(|e| format!("cannot create prompt dir: {e}"))?;
        fs::create_dir_all(base_dir.join("schemas"))
            .map_err(|e| format!("cannot create schemas dir: {e}"))?;

        let manifest_path = base_dir.join("registry.json");
        let manifest = if manifest_path.exists() {
            let data = fs::read_to_string(&manifest_path)
                .map_err(|e| format!("cannot read registry.json: {e}"))?;
            serde_json::from_str(&data).map_err(|e| format!("invalid registry.json: {e}"))?
        } else {
            RegistryManifest::default()
        };

        Ok(PromptRegistry {
            base_dir: base_dir.to_path_buf(),
            manifest,
        })
    }

    /// Save the registry manifest.
    pub fn save(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.manifest)
            .map_err(|e| format!("serialize error: {e}"))?;
        fs::write(self.base_dir.join("registry.json"), json)
            .map_err(|e| format!("write error: {e}"))
    }

    /// Register a prompt template.
    pub fn register_prompt(&mut self, template: &PromptTemplate) -> Result<String, String> {
        let json =
            serde_json::to_string_pretty(template).map_err(|e| format!("serialize error: {e}"))?;
        let hash = content_hash(&json);

        let filename = format!("{}.prompt.json", template.id);
        fs::write(self.base_dir.join(&filename), &json).map_err(|e| format!("write error: {e}"))?;

        self.manifest.prompts.insert(
            template.id.clone(),
            PromptEntry {
                file: filename,
                content_hash: hash.clone(),
                version: template.version.clone(),
            },
        );

        self.save()?;
        Ok(hash)
    }

    /// Register an output schema.
    pub fn register_schema(
        &mut self,
        schema_id: &str,
        schema_json: &str,
    ) -> Result<String, String> {
        let hash = content_hash(schema_json);
        let filename = format!("schemas/{schema_id}.json");
        fs::write(self.base_dir.join(&filename), schema_json)
            .map_err(|e| format!("write error: {e}"))?;

        self.manifest.schemas.insert(
            schema_id.to_string(),
            SchemaEntry {
                file: filename,
                content_hash: hash.clone(),
            },
        );

        self.save()?;
        Ok(hash)
    }

    /// Load a prompt template by ID.
    pub fn load_prompt(&self, prompt_id: &str) -> Result<PromptTemplate, String> {
        let entry = self
            .manifest
            .prompts
            .get(prompt_id)
            .ok_or_else(|| format!("prompt not found: {prompt_id}"))?;
        let data = fs::read_to_string(self.base_dir.join(&entry.file))
            .map_err(|e| format!("read error: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("invalid prompt JSON: {e}"))
    }

    /// Load an output schema by ID. Returns raw JSON string.
    pub fn load_schema(&self, schema_id: &str) -> Result<String, String> {
        let entry = self
            .manifest
            .schemas
            .get(schema_id)
            .ok_or_else(|| format!("schema not found: {schema_id}"))?;
        fs::read_to_string(self.base_dir.join(&entry.file)).map_err(|e| format!("read error: {e}"))
    }

    /// Get the content hash for a prompt.
    pub fn prompt_hash(&self, prompt_id: &str) -> Result<String, String> {
        self.manifest
            .prompts
            .get(prompt_id)
            .map(|e| e.content_hash.clone())
            .ok_or_else(|| format!("prompt not found: {prompt_id}"))
    }

    /// Get the content hash for a schema.
    pub fn schema_hash(&self, schema_id: &str) -> Result<String, String> {
        self.manifest
            .schemas
            .get(schema_id)
            .map(|e| e.content_hash.clone())
            .ok_or_else(|| format!("schema not found: {schema_id}"))
    }

    /// Verify that all registered prompts and schemas match their content hashes.
    pub fn verify(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        for (id, entry) in &self.manifest.prompts {
            let path = self.base_dir.join(&entry.file);
            match fs::read_to_string(&path) {
                Ok(data) => {
                    let actual = content_hash(&data);
                    if actual != entry.content_hash {
                        errors.push(format!(
                            "prompt {id}: hash mismatch (expected {}, got {actual})",
                            entry.content_hash
                        ));
                    }
                }
                Err(e) => errors.push(format!("prompt {id}: {e}")),
            }
        }

        for (id, entry) in &self.manifest.schemas {
            let path = self.base_dir.join(&entry.file);
            match fs::read_to_string(&path) {
                Ok(data) => {
                    let actual = content_hash(&data);
                    if actual != entry.content_hash {
                        errors.push(format!(
                            "schema {id}: hash mismatch (expected {}, got {actual})",
                            entry.content_hash
                        ));
                    }
                }
                Err(e) => errors.push(format!("schema {id}: {e}")),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Compile a prompt template with arguments.
    /// Replaces `{{param}}` with provided values.
    pub fn compile_prompt(
        &self,
        prompt_id: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<String, String> {
        let template = self.load_prompt(prompt_id)?;

        // Validate all required parameters are provided
        for param in &template.parameters {
            if !args.contains_key(param) {
                return Err(format!("missing parameter: {param}"));
            }
        }

        let mut result = template.template.clone();
        for (key, value) in args {
            result = result.replace(&format!("{{{{{key}}}}}"), value);
        }

        Ok(result)
    }

    /// List all registered prompt IDs.
    pub fn list_prompts(&self) -> Vec<&str> {
        self.manifest.prompts.keys().map(|s| s.as_str()).collect()
    }

    /// List all registered schema IDs.
    pub fn list_schemas(&self) -> Vec<&str> {
        self.manifest.schemas.keys().map(|s| s.as_str()).collect()
    }
}

/// Compute SHA-256 hash of content.
pub fn content_hash(content: &str) -> String {
    let hash = Sha256::digest(content.as_bytes());
    format!("sha256:{:x}", hash)
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    fn make_template(id: &str) -> PromptTemplate {
        PromptTemplate {
            id: id.into(),
            version: "1.0.0".into(),
            template: "Hello {{name}}, your task is {{task}}.".into(),
            parameters: vec!["name".into(), "task".into()],
            default_model: "default".into(),
            default_max_tokens: 512,
            default_temperature: 0,
            default_schema_id: "json_object".into(),
        }
    }

    #[test]
    fn test_register_and_load_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = PromptRegistry::open(dir.path()).unwrap();
        let tmpl = make_template("test.greet");
        let hash = reg.register_prompt(&tmpl).unwrap();
        assert!(hash.starts_with("sha256:"));

        let loaded = reg.load_prompt("test.greet").unwrap();
        assert_eq!(loaded.id, "test.greet");
        assert_eq!(loaded.template, tmpl.template);
    }

    #[test]
    fn test_register_and_load_schema() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = PromptRegistry::open(dir.path()).unwrap();
        let schema = r#"{"type": "object", "properties": {"result": {"type": "string"}}}"#;
        let hash = reg.register_schema("test_schema", schema).unwrap();
        assert!(hash.starts_with("sha256:"));

        let loaded = reg.load_schema("test_schema").unwrap();
        assert_eq!(loaded, schema);
    }

    #[test]
    fn test_compile_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = PromptRegistry::open(dir.path()).unwrap();
        reg.register_prompt(&make_template("test.greet")).unwrap();

        let mut args = BTreeMap::new();
        args.insert("name".into(), "Alice".into());
        args.insert("task".into(), "refactor".into());

        let result = reg.compile_prompt("test.greet", &args).unwrap();
        assert_eq!(result, "Hello Alice, your task is refactor.");
    }

    #[test]
    fn test_compile_prompt_missing_param() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = PromptRegistry::open(dir.path()).unwrap();
        reg.register_prompt(&make_template("test.greet")).unwrap();

        let mut args = BTreeMap::new();
        args.insert("name".into(), "Alice".into());
        // missing "task"

        let result = reg.compile_prompt("test.greet", &args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing parameter: task"));
    }

    #[test]
    fn test_verify_intact() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = PromptRegistry::open(dir.path()).unwrap();
        reg.register_prompt(&make_template("test.greet")).unwrap();
        reg.register_schema("s1", "{}").unwrap();

        assert!(reg.verify().is_ok());
    }

    #[test]
    fn test_verify_tampered() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = PromptRegistry::open(dir.path()).unwrap();
        reg.register_prompt(&make_template("test.greet")).unwrap();

        // Tamper with the file
        let path = dir.path().join("test.greet.prompt.json");
        fs::write(&path, "TAMPERED").unwrap();

        let result = reg.verify();
        assert!(result.is_err());
    }

    #[test]
    fn test_prompt_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let reg = PromptRegistry::open(dir.path()).unwrap();
        let result = reg.load_prompt("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_content_hash_deterministic() {
        let h1 = content_hash("hello world");
        let h2 = content_hash("hello world");
        assert_eq!(h1, h2);
        assert!(h1.starts_with("sha256:"));
    }
}
