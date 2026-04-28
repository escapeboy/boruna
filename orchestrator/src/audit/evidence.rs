use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::audit::encryption::{EncryptionError, EncryptionInfo, Envelope, KEY_LEN};
use crate::audit::fingerprint::EnvFingerprint;
use crate::audit::log::AuditLog;
use crate::audit::BUNDLE_FORMAT_VERSION;

fn default_schema_version() -> u32 {
    1
}

/// Top-level bundle manifest written as `bundle.json` at the bundle root.
///
/// This is the version-gated entry point for readers (see
/// [`crate::audit::verify::verify_bundle`]). It is the LAST file written
/// during finalize, after every other component has been flushed and
/// parent-dir fsynced — so its presence implies a complete bundle.
///
/// `format_version` follows semver-like compat: `1.x` is forward-compat,
/// `2.x` is breaking. See `docs/spec/evidence-bundle-1.0.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleJson {
    pub format_version: String,
    pub boruna_version: String,
    pub created_at: String,
    pub run_id: String,
    pub workflow_hash: String,
    pub components: Vec<String>,
}

/// Manifest describing an evidence bundle.
///
/// Sprint W6-B added the optional `encryption` field. Plaintext
/// bundles serialize without it (skip-if-none); existing W1-C readers
/// that don't know about encryption see an unchanged shape.
/// `encryption.algorithm`, `kek_id`, `wrapped_dek`, and
/// `wrapped_dek_nonce` are REPLAY-VERIFIED (they participate in
/// `bundle_hash`); `encryption.files` is OPERATIONAL.
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
    /// Sprint W6-B: present iff the bundle is envelope-encrypted with
    /// AES-256-GCM. Absent → plaintext bundle (W1-C behavior).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption: Option<EncryptionInfo>,
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
    /// Sprint W6-B: when present, every file the builder writes is
    /// encrypted with AES-256-GCM under the envelope's DEK before
    /// hitting disk. `file_checksums` are still SHA-256(plaintext) so
    /// the existing verify path matches once the verifier decrypts.
    encryption: Option<Envelope>,
    /// Tracks filenames written through the encrypted path; used to
    /// populate `EncryptionInfo.files` at finalize time.
    encrypted_files: Vec<String>,
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
            encryption: None,
            encrypted_files: Vec::new(),
        })
    }

    /// Sprint W6-B: enable AES-256-GCM envelope encryption for all
    /// subsequent file writes. Generates a fresh DEK and wraps it
    /// with the supplied KEK. The DEK lives in-memory only and is
    /// dropped when the builder is consumed by `finalize`.
    pub fn with_encryption(
        mut self,
        kek: &[u8; KEY_LEN],
        kek_id: &str,
    ) -> Result<Self, EncryptionError> {
        self.encryption = Some(Envelope::new(kek, kek_id)?);
        Ok(self)
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
        let bytes = self.encrypt_if_needed(&filename, json.as_bytes());
        std::fs::write(&path, &bytes)?;
        // Checksum is over the plaintext: verify decrypts then hashes.
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

        // Build encryption metadata if the builder is encrypted. The
        // `files` list is populated from the running tracker; sort
        // for deterministic JSON output.
        let encryption_info = self.encryption.as_ref().map(|env| {
            let mut info = env.info.clone();
            let mut files = self.encrypted_files.clone();
            files.sort();
            files.dedup();
            info.files = files;
            info
        });

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
            encryption: encryption_info,
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

        // Write top-level versioned bundle.json LAST, after every other
        // component is on disk. Readers gate on this file: if it's
        // missing, the bundle is treated as legacy/incomplete.
        // Components are listed by what was actually written.
        let mut components: Vec<String> = vec![
            "manifest.json".to_string(),
            "audit_log.json".to_string(),
            "env_fingerprint.json".to_string(),
        ];
        if self.bundle_dir.join("workflow.json").exists() {
            components.push("workflow.json".to_string());
        }
        if self.bundle_dir.join("policy.json").exists() {
            components.push("policy.json".to_string());
        }
        if self.bundle_dir.join("outputs").exists() {
            components.push("outputs/".to_string());
        }
        components.sort();

        let bundle_json = BundleJson {
            format_version: BUNDLE_FORMAT_VERSION.to_string(),
            boruna_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: completed_at.clone(),
            run_id: self.run_id.clone(),
            workflow_hash: self.workflow_hash.clone(),
            components,
        };
        let bundle_json_str =
            serde_json::to_string_pretty(&bundle_json).map_err(std::io::Error::other)?;

        // Atomic write + parent-dir fsync, mirroring the audit-log /
        // step-output write pattern (see workflow/data_flow.rs).
        atomic_write_with_dir_fsync(&self.bundle_dir, "bundle.json", bundle_json_str.as_bytes())?;

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
        let bytes = self.encrypt_if_needed(name, content.as_bytes());
        std::fs::write(&path, &bytes)?;
        self.file_checksums
            .insert(name.to_string(), sha256_str(content));
        Ok(())
    }

    /// Encrypt `bytes` for `name` if the builder has an envelope; also
    /// record the filename so finalize can populate
    /// `EncryptionInfo.files`. Returns the bytes to write to disk.
    fn encrypt_if_needed(&mut self, name: &str, bytes: &[u8]) -> Vec<u8> {
        match &self.encryption {
            Some(env) => {
                let ct = env.encrypt_file(name, bytes);
                self.encrypted_files.push(name.to_string());
                ct
            }
            None => bytes.to_vec(),
        }
    }
}

fn sha256_str(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Write `bytes` to `dir/name` atomically and fsync the parent directory
/// so the rename's directory entry is journaled to stable media. Mirrors
/// the pattern in `workflow::data_flow::DataStore::store_output`.
///
/// Used for `bundle.json` so that readers seeing the file are guaranteed
/// to see a complete bundle (every other component was written first).
fn atomic_write_with_dir_fsync(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, bytes)?;
    fullsync_file(tmp.as_file())?;
    tmp.persist(dir.join(name)).map_err(|e| e.error)?;
    #[cfg(unix)]
    {
        let dir_handle = std::fs::File::open(dir)?;
        fullsync_file(&dir_handle)?;
    }
    Ok(())
}

fn fullsync_file(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `file` is a valid open file descriptor; F_FULLFSYNC
        // takes no further argument. Returns 0 on success, -1 on error.
        let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) };
        if rc == 0 {
            Ok(())
        } else {
            file.sync_all()
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        file.sync_all()
    }
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
    fn bundle_writes_format_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder =
            EvidenceBundleBuilder::new(dir.path(), "run-fmt-001", "fmt-test").unwrap();
        builder.add_workflow_def(r#"{"name":"fmt"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        builder
            .add_step_output("s1", "result", r#"{"v":1}"#)
            .unwrap();

        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        audit.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 1,
        });

        builder.finalize(&audit).unwrap();

        let bundle_dir = dir.path().join("run-fmt-001");
        let bundle_json_path = bundle_dir.join("bundle.json");
        assert!(
            bundle_json_path.exists(),
            "bundle.json must be present at bundle root"
        );
        let raw = std::fs::read_to_string(&bundle_json_path).unwrap();
        let parsed: BundleJson = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.format_version, "1.0");
        assert_eq!(parsed.run_id, "run-fmt-001");
        assert!(!parsed.boruna_version.is_empty());
        assert!(parsed.components.iter().any(|c| c == "manifest.json"));
        assert!(parsed.components.iter().any(|c| c == "audit_log.json"));
        assert!(parsed.components.iter().any(|c| c == "outputs/"));
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
