use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::audit::fingerprint::EnvFingerprint;
use crate::audit::log::AuditLog;

fn default_schema_version() -> u32 {
    1
}

/// Manifest describing an evidence bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub run_id: String,
    pub workflow_name: String,
    pub workflow_hash: String,
    pub policy_hash: String,
    pub audit_log_hash: String,
    pub file_checksums: BTreeMap<String, String>,
    pub env_fingerprint: EnvFingerprint,
    pub started_at: String,
    pub completed_at: String,
    pub bundle_hash: String,
}

/// Builder for creating evidence bundles on disk.
pub struct EvidenceBundleBuilder {
    bundle_dir: PathBuf,
    run_id: String,
    workflow_name: String,
    workflow_hash: String,
    policy_hash: String,
    started_at: String,
    file_checksums: BTreeMap<String, String>,
}

impl EvidenceBundleBuilder {
    /// Start building a new evidence bundle.
    pub fn new(base_dir: &Path, run_id: &str, workflow_name: &str) -> std::io::Result<Self> {
        let bundle_dir = base_dir.join(run_id);
        std::fs::create_dir_all(&bundle_dir)?;

        Ok(EvidenceBundleBuilder {
            bundle_dir,
            run_id: run_id.to_string(),
            workflow_name: workflow_name.to_string(),
            workflow_hash: String::new(),
            policy_hash: String::new(),
            started_at: chrono::Utc::now().to_rfc3339(),
            file_checksums: BTreeMap::new(),
        })
    }

    /// Store the workflow definition in the bundle.
    pub fn add_workflow_def(&mut self, json: &str) -> std::io::Result<()> {
        self.workflow_hash = sha256_str(json);
        self.write_file("workflow.json", json)
    }

    /// Store the policy snapshot.
    pub fn add_policy(&mut self, json: &str) -> std::io::Result<()> {
        self.policy_hash = sha256_str(json);
        self.write_file("policy.json", json)
    }

    /// Store a step's output.
    pub fn add_step_output(
        &mut self,
        step_id: &str,
        name: &str,
        json: &str,
    ) -> std::io::Result<()> {
        let subdir = self.bundle_dir.join("outputs").join(step_id);
        std::fs::create_dir_all(&subdir)?;
        let filename = format!("outputs/{step_id}/{name}.json");
        let path = self.bundle_dir.join(&filename);
        std::fs::write(&path, json)?;
        self.file_checksums.insert(filename, sha256_str(json));
        Ok(())
    }

    /// Store a raw file in the bundle.
    pub fn add_file(&mut self, name: &str, content: &str) -> std::io::Result<()> {
        self.write_file(name, content)
    }

    /// Finalize the bundle: write audit log, env fingerprint, and manifest.
    pub fn finalize(mut self, audit_log: &AuditLog) -> std::io::Result<BundleManifest> {
        let completed_at = chrono::Utc::now().to_rfc3339();

        // Write audit log
        let audit_json = audit_log.to_json().map_err(std::io::Error::other)?;
        self.write_file("audit_log.json", &audit_json)?;

        let env_fingerprint = EnvFingerprint::capture();
        let env_json =
            serde_json::to_string_pretty(&env_fingerprint).map_err(std::io::Error::other)?;
        self.write_file("env_fingerprint.json", &env_json)?;

        // Build manifest
        let manifest = BundleManifest {
            schema_version: 1,
            run_id: self.run_id.clone(),
            workflow_name: self.workflow_name.clone(),
            workflow_hash: self.workflow_hash.clone(),
            policy_hash: self.policy_hash.clone(),
            audit_log_hash: audit_log.hash(),
            file_checksums: self.file_checksums.clone(),
            env_fingerprint,
            started_at: self.started_at.clone(),
            completed_at: completed_at.clone(),
            bundle_hash: String::new(), // filled below
        };

        // Compute bundle hash from manifest (excluding bundle_hash itself)
        let manifest_json =
            serde_json::to_string_pretty(&manifest).map_err(std::io::Error::other)?;
        let bundle_hash = sha256_str(&manifest_json);

        let final_manifest = BundleManifest {
            bundle_hash: bundle_hash.clone(),
            ..manifest
        };

        let final_json =
            serde_json::to_string_pretty(&final_manifest).map_err(std::io::Error::other)?;
        std::fs::write(self.bundle_dir.join("manifest.json"), &final_json)?;

        Ok(final_manifest)
    }

    /// Get the bundle directory path.
    pub fn bundle_dir(&self) -> &Path {
        &self.bundle_dir
    }

    fn write_file(&mut self, name: &str, content: &str) -> std::io::Result<()> {
        let path = self.bundle_dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        self.file_checksums
            .insert(name.to_string(), sha256_str(content));
        Ok(())
    }
}

fn sha256_str(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::log::{AuditEvent, AuditLog};

    #[test]
    fn test_bundle_creation() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder =
            EvidenceBundleBuilder::new(dir.path(), "run-test-001", "test-workflow").unwrap();

        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        builder
            .add_step_output("step1", "result", r#"{"value":42}"#)
            .unwrap();

        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        audit.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 100,
        });

        let manifest = builder.finalize(&audit).unwrap();
        assert!(!manifest.bundle_hash.is_empty());
        assert!(!manifest.workflow_hash.is_empty());
        assert!(!manifest.policy_hash.is_empty());
        assert!(!manifest.audit_log_hash.is_empty());

        // Verify files exist on disk
        let bundle_path = dir.path().join("run-test-001");
        assert!(bundle_path.join("manifest.json").exists());
        assert!(bundle_path.join("workflow.json").exists());
        assert!(bundle_path.join("policy.json").exists());
        assert!(bundle_path.join("audit_log.json").exists());
        assert!(bundle_path.join("env_fingerprint.json").exists());
        assert!(bundle_path.join("outputs/step1/result.json").exists());
    }

    #[test]
    fn test_bundle_checksums() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = EvidenceBundleBuilder::new(dir.path(), "run-chk-001", "test").unwrap();

        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder.add_policy(r#"{}"#).unwrap();

        let audit = AuditLog::new();
        let manifest = builder.finalize(&audit).unwrap();

        // Check that all files have checksums
        assert!(manifest.file_checksums.contains_key("workflow.json"));
        assert!(manifest.file_checksums.contains_key("policy.json"));
        assert!(manifest.file_checksums.contains_key("audit_log.json"));
        assert!(manifest.file_checksums.contains_key("env_fingerprint.json"));
    }

    #[test]
    fn test_bundle_determinism() {
        // Two bundles with same content should have same checksums (except timestamps)
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        let workflow = r#"{"name":"det-test","version":"1.0"}"#;
        let policy = r#"{"default_allow":true}"#;

        let mut b1 = EvidenceBundleBuilder::new(dir1.path(), "run-1", "det-test").unwrap();
        b1.add_workflow_def(workflow).unwrap();
        b1.add_policy(policy).unwrap();

        let mut b2 = EvidenceBundleBuilder::new(dir2.path(), "run-1", "det-test").unwrap();
        b2.add_workflow_def(workflow).unwrap();
        b2.add_policy(policy).unwrap();

        // Same content → same file checksums
        assert_eq!(
            b1.file_checksums.get("workflow.json"),
            b2.file_checksums.get("workflow.json")
        );
        assert_eq!(
            b1.file_checksums.get("policy.json"),
            b2.file_checksums.get("policy.json")
        );
    }
}
