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

/// Optional ed25519 signature over a manifest's `bundle_hash`
/// (verify-A "sign-side").
///
/// When present, the operator (or their signing service) has signed
/// the bundle's `bundle_hash` — the SHA-256 that already covers
/// `file_checksums` + `audit_log_hash` — with an ed25519 key. A
/// verifier that trusts the embedded `public_key` can then prove the
/// bundle's integrity WITHOUT carrying an out-of-band `bundle_hash`
/// anchor: the signature roots trust in the operator's key instead.
///
/// The signature covers `bundle_hash` and is NOT itself part of the
/// hash (it is added AFTER `bundle_hash` is computed). Verifiers
/// recompute `bundle_hash` with this field excluded — see
/// [`crate::audit::verify::verify_bundle_with_opts`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSignature {
    /// Signature algorithm — currently always `"ed25519"`.
    pub algorithm: String,
    /// Hex-encoded 32-byte ed25519 public key.
    pub public_key: String,
    /// Hex-encoded 64-byte ed25519 signature over `bundle_hash`
    /// (signed message is `bundle_hash.as_bytes()`).
    pub signature: String,
}

/// Manifest describing an evidence bundle.
///
/// Sprint W6-B added the optional `encryption` field. Plaintext
/// bundles serialize without it (skip-if-none); existing W1-C readers
/// that don't know about encryption see an unchanged shape.
/// `encryption.algorithm`, `kek_id`, `wrapped_dek`, and
/// `wrapped_dek_nonce` are REPLAY-VERIFIED (they participate in
/// `bundle_hash`); `encryption.files` is OPERATIONAL.
///
/// verify-A added the optional `signature` field. It is written
/// AFTER `bundle_hash` is computed and is skipped when absent, so
/// unsigned manifests (and their `bundle_hash`) are byte-for-byte
/// unchanged; existing readers ignore the unknown field.
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
    /// verify-A: present iff the manifest's `bundle_hash` was signed
    /// with an ed25519 key at build time. Absent → unsigned bundle
    /// (unchanged manifest bytes). Not part of `bundle_hash`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<ManifestSignature>,
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
    /// verify-A: when present, `finalize` signs the manifest's
    /// `bundle_hash` with this ed25519 key and records the signature
    /// (+ public key) in `manifest.signature`. Absent → unsigned
    /// bundle (unchanged behavior).
    signing_key: Option<ed25519_dalek::SigningKey>,
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
            signing_key: None,
        })
    }

    /// verify-A: sign the finalized manifest's `bundle_hash` with the
    /// ed25519 key derived from `seed` (a 32-byte secret seed). When
    /// set, `finalize` populates `manifest.signature`; when unset the
    /// bundle is unsigned and byte-identical to today's output.
    pub fn with_signing_key(mut self, seed: &[u8; 32]) -> Self {
        self.signing_key = Some(ed25519_dalek::SigningKey::from_bytes(seed));
        self
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

    /// Store per-step declared intents (`intent "..."` clauses) as
    /// `intents.json`, mapping step_id → declared purpose. Only steps
    /// that declared an intent appear. Written via `write_file`, so the
    /// component is checksummed, covered by `bundle_hash`, and checked by
    /// `evidence verify` like every other component. Intent is
    /// replay-verified evidence: it records what each step was *authorized*
    /// to do, alongside the outputs recording what it actually did.
    /// No-op when the map is empty (no `intents.json` is written).
    pub fn add_intents(
        &mut self,
        intents: &std::collections::BTreeMap<String, String>,
    ) -> std::io::Result<()> {
        if intents.is_empty() {
            return Ok(());
        }
        // BTreeMap serializes with sorted keys → deterministic bytes.
        let json = serde_json::to_string_pretty(intents).map_err(std::io::Error::other)?;
        self.write_file("intents.json", &json)
    }

    /// Store the sorted list of step ids that transitively invoke an LLM
    /// (an `llm.*` capability reachable through the step's call graph) as
    /// `model_invoking_steps.json`. Lets an auditor see which steps touched
    /// a model — the effect having propagated up the call graph — without
    /// re-analyzing sources. Checksummed + hash-covered like every other
    /// component. No-op when the list is empty. Callers pass an
    /// already-sorted, de-duplicated list for deterministic bytes.
    pub fn add_model_invocations(&mut self, step_ids: &[String]) -> std::io::Result<()> {
        if step_ids.is_empty() {
            return Ok(());
        }
        let json = serde_json::to_string_pretty(step_ids).map_err(std::io::Error::other)?;
        self.write_file("model_invoking_steps.json", &json)
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
            signature: None, // filled below iff a signing key was supplied
        };

        // Compute bundle hash from manifest (excluding bundle_hash and
        // the not-yet-present signature — both are `skip_if_none`/empty
        // here, so the serialized bytes match the verifier's recompute).
        let manifest_json =
            serde_json::to_string_pretty(&manifest).map_err(std::io::Error::other)?;
        let bundle_hash = sha256_str(&manifest_json);

        // verify-A: sign the bundle_hash bytes iff a signing key was
        // supplied. The signature is added AFTER bundle_hash is
        // computed, so it never feeds the hash.
        let signature = self.signing_key.as_ref().map(|sk| {
            use ed25519_dalek::Signer;
            let sig = sk.sign(bundle_hash.as_bytes());
            ManifestSignature {
                algorithm: "ed25519".to_string(),
                public_key: to_hex(sk.verifying_key().as_bytes()),
                signature: to_hex(&sig.to_bytes()),
            }
        });

        let final_manifest = BundleManifest {
            bundle_hash: bundle_hash.clone(),
            signature,
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
        if self.bundle_dir.join("intents.json").exists() {
            components.push("intents.json".to_string());
        }
        if self.bundle_dir.join("model_invoking_steps.json").exists() {
            components.push("model_invoking_steps.json".to_string());
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

/// Lowercase-hex encode bytes (for ed25519 public key / signature in
/// `ManifestSignature`).
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
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

/// Outcome of a successful [`redact_bundle`] call.
#[derive(Debug, Clone)]
pub struct RedactOutcome {
    /// Sequence number of the redacted audit-log entry.
    pub redacted_sequence: u64,
    /// The preserved content commitment for the removed event.
    pub content_sha256: String,
    /// True iff a now-stale manifest signature was dropped (the operator
    /// must re-sign or re-anchor after redaction — see below).
    pub signature_stripped: bool,
    /// The recomputed manifest `bundle_hash` after redaction.
    pub new_bundle_hash: String,
    /// The `audit_log_hash`, which is INVARIANT under redaction. An
    /// operator holding this out-of-band anchor can distinguish a
    /// redaction (anchor unchanged) from a content tamper (anchor
    /// changes).
    pub audit_log_hash: String,
}

/// Errors from [`redact_bundle`].
#[derive(Debug)]
pub enum BundleRedactError {
    Io(std::io::Error),
    InvalidManifest(String),
    InvalidAuditLog(String),
    /// The bundle's audit chain does not verify — refuse to redact a
    /// bundle that is already broken (nothing to preserve).
    ChainInvalid(u64),
    /// Redaction of encrypted bundles is not supported here; rotate/
    /// decrypt first. See `orchestrator/docs/verifiable-redaction.md`.
    EncryptedUnsupported,
    Redact(crate::audit::log::RedactError),
}

impl std::fmt::Display for BundleRedactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BundleRedactError::Io(e) => write!(f, "io: {e}"),
            BundleRedactError::InvalidManifest(s) => write!(f, "invalid manifest: {s}"),
            BundleRedactError::InvalidAuditLog(s) => write!(f, "invalid audit_log.json: {s}"),
            BundleRedactError::ChainInvalid(seq) => {
                write!(
                    f,
                    "audit chain is broken at entry {seq}; refusing to redact"
                )
            }
            BundleRedactError::EncryptedUnsupported => write!(
                f,
                "redaction of encrypted bundles is not supported (decrypt/rotate first)"
            ),
            BundleRedactError::Redact(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BundleRedactError {}

impl From<std::io::Error> for BundleRedactError {
    fn from(e: std::io::Error) -> Self {
        BundleRedactError::Io(e)
    }
}

/// Verifiably redact one audit-log entry inside an evidence bundle on
/// disk, keeping the bundle verifiable.
///
/// The commitment chain (format 1.1) makes this possible: the redacted
/// entry's `content_sha256` is preserved, so its `entry_hash`, the
/// prev-hash links, and the log's overall `audit_log_hash` are all
/// UNCHANGED. What legitimately changes is:
///
/// - `audit_log.json` bytes (the event content is blanked + a redaction
///   marker added), hence
/// - `manifest.file_checksums["audit_log.json"]`, hence
/// - `manifest.bundle_hash` (recomputed here so the bundle stays
///   self-consistent and `evidence verify` passes).
///
/// A prior ed25519 `signature` (which signed the OLD `bundle_hash`) is
/// dropped, because redaction is a post-seal authorized transformation
/// the original signer did not endorse; the operator re-signs afterward.
///
/// **Distinguishing redaction from tampering.** `audit_log_hash` is the
/// invariant: a redaction leaves it unchanged, while any tamper that
/// alters event *content* must change a `content_sha256` and therefore
/// the chain and `audit_log_hash`. An operator who anchored the original
/// `audit_log_hash` out-of-band sees it survive redaction but not a
/// tamper. Within the bundle, verification also reports which entries
/// are redacted, and the chain still verifies for a valid redaction but
/// fails for a content tamper.
///
/// Only plaintext bundles are supported; encrypted bundles return
/// [`BundleRedactError::EncryptedUnsupported`].
pub fn redact_bundle(
    bundle_dir: &Path,
    index: usize,
    field: Option<&str>,
    reason: Option<String>,
) -> Result<RedactOutcome, BundleRedactError> {
    // 1. Load manifest.
    let manifest_path = bundle_dir.join("manifest.json");
    let manifest_raw = std::fs::read_to_string(&manifest_path)?;
    let mut manifest: BundleManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| BundleRedactError::InvalidManifest(e.to_string()))?;

    if manifest.encryption.is_some() {
        return Err(BundleRedactError::EncryptedUnsupported);
    }

    // 2. Load + verify the audit log BEFORE mutating anything.
    let audit_path = bundle_dir.join("audit_log.json");
    let audit_raw = std::fs::read_to_string(&audit_path)?;
    let mut log = AuditLog::from_json(&audit_raw)
        .map_err(|e| BundleRedactError::InvalidAuditLog(e.to_string()))?;
    if let Err(seq) = log.verify() {
        return Err(BundleRedactError::ChainInvalid(seq));
    }
    let audit_log_hash = log.hash();

    // 3. Redact the entry; the chain (and audit_log_hash) is preserved.
    let content_sha256 = log
        .redact_entry(index, field, reason)
        .map_err(BundleRedactError::Redact)?;
    debug_assert_eq!(
        log.hash(),
        audit_log_hash,
        "redaction must preserve audit_log_hash"
    );
    let redacted_sequence = log.entries()[index].sequence;

    // 4. Rewrite audit_log.json and update its checksum.
    let new_audit_json = log
        .to_json()
        .map_err(|e| BundleRedactError::InvalidAuditLog(e.to_string()))?;
    atomic_write_with_dir_fsync(bundle_dir, "audit_log.json", new_audit_json.as_bytes())?;
    manifest
        .file_checksums
        .insert("audit_log.json".to_string(), sha256_str(&new_audit_json));

    // 5. audit_log_hash is unchanged; drop the now-stale signature.
    let signature_stripped = manifest.signature.take().is_some();

    // 6. Recompute bundle_hash exactly as finalize does (clear
    //    bundle_hash + signature, pretty-print, sha256).
    let new_bundle_hash = recompute_bundle_hash(&manifest)
        .map_err(|e| BundleRedactError::InvalidManifest(e.to_string()))?;
    manifest.bundle_hash = new_bundle_hash.clone();

    // 7. Atomically rewrite the manifest.
    let final_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| BundleRedactError::InvalidManifest(e.to_string()))?;
    atomic_write_with_dir_fsync(bundle_dir, "manifest.json", final_json.as_bytes())?;

    Ok(RedactOutcome {
        redacted_sequence,
        content_sha256,
        signature_stripped,
        new_bundle_hash,
        audit_log_hash,
    })
}

/// Recompute a manifest's `bundle_hash` the way
/// [`EvidenceBundleBuilder::finalize`] does: clone, clear `bundle_hash`
/// and `signature`, pretty-print, sha256. Mirrors
/// `verify::recompute_bundle_hash` / `rotate::compute_bundle_hash`.
fn recompute_bundle_hash(manifest: &BundleManifest) -> Result<String, serde_json::Error> {
    let mut clone = manifest.clone();
    clone.bundle_hash = String::new();
    clone.signature = None;
    let json = serde_json::to_string_pretty(&clone)?;
    Ok(sha256_str(&json))
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
    fn test_bundle_captures_intents_and_is_tamper_evident() {
        use crate::audit::verify::verify_bundle;

        let dir = tempfile::tempdir().unwrap();
        let mut builder =
            EvidenceBundleBuilder::new(dir.path(), "run-intent-001", "intent-wf").unwrap();
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();

        let mut intents = std::collections::BTreeMap::new();
        intents.insert(
            "step1".to_string(),
            "Move funds between accounts".to_string(),
        );
        builder.add_intents(&intents).unwrap();

        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        audit.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 10,
        });
        let manifest = builder.finalize(&audit).unwrap();

        let bundle_path = dir.path().join("run-intent-001");
        // intents.json written and covered by the manifest checksums.
        assert!(bundle_path.join("intents.json").exists());
        assert!(manifest.file_checksums.contains_key("intents.json"));

        // A pristine bundle verifies.
        let ok = verify_bundle(&bundle_path);
        assert!(ok.valid, "expected valid bundle, errors: {:?}", ok.errors);

        // Tampering the captured intent breaks verification — intent is
        // inside the hash chain, not decorative.
        std::fs::write(
            bundle_path.join("intents.json"),
            r#"{"step1":"tampered purpose"}"#,
        )
        .unwrap();
        let bad = verify_bundle(&bundle_path);
        assert!(!bad.valid, "tampered intents.json must fail verification");
    }

    #[test]
    fn test_bundle_captures_model_invocations_and_is_covered() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = EvidenceBundleBuilder::new(dir.path(), "run-model-001", "wf").unwrap();
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder
            .add_model_invocations(&["stepA".to_string(), "stepC".to_string()])
            .unwrap();
        let audit = AuditLog::new();
        let manifest = builder.finalize(&audit).unwrap();
        let bundle_path = dir.path().join("run-model-001");
        assert!(bundle_path.join("model_invoking_steps.json").exists());
        assert!(manifest
            .file_checksums
            .contains_key("model_invoking_steps.json"));
    }

    #[test]
    fn test_bundle_no_model_invocations_writes_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = EvidenceBundleBuilder::new(dir.path(), "run-model-002", "wf").unwrap();
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder.add_model_invocations(&[]).unwrap();
        builder.finalize(&AuditLog::new()).unwrap();
        assert!(!dir
            .path()
            .join("run-model-002")
            .join("model_invoking_steps.json")
            .exists());
    }

    #[test]
    fn test_bundle_no_intents_writes_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder =
            EvidenceBundleBuilder::new(dir.path(), "run-intent-002", "intent-wf").unwrap();
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        // Empty intent map → no intents.json component.
        builder
            .add_intents(&std::collections::BTreeMap::new())
            .unwrap();
        let audit = AuditLog::new();
        builder.finalize(&audit).unwrap();
        assert!(!dir
            .path()
            .join("run-intent-002")
            .join("intents.json")
            .exists());
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
        // Locks the current emitted format version byte-exactly. Bumped
        // to 1.1 with the commitment-chain audit log (verifiable
        // redaction). A 1.0 reader still accepts this bundle (same major).
        assert_eq!(parsed.format_version, "1.1");
        assert_eq!(parsed.run_id, "run-fmt-001");
        assert!(!parsed.boruna_version.is_empty());
        assert!(parsed.components.iter().any(|c| c == "manifest.json"));
        assert!(parsed.components.iter().any(|c| c == "audit_log.json"));
        assert!(parsed.components.iter().any(|c| c == "outputs/"));
    }

    #[test]
    fn test_bundle_carries_contract_check_events() {
        // End-to-end: compile+run an .ax program whose `requires`
        // precondition passes, capture the VM event log, seal it into an
        // evidence bundle, verify the bundle, then read the log back and
        // confirm the ContractCheck event survived into verified evidence.
        use crate::audit::verify::verify_bundle;
        use boruna_vm::replay::{Event, EventLog};
        use boruna_vm::{CapabilityGateway, Policy, Vm};

        let module =
            boruna_compiler::compile("contract_prog", "fn main() -> Int requires true { 42 }")
                .expect("program compiles");
        let mut vm = Vm::new(module, CapabilityGateway::new(Policy::allow_all()));
        assert_eq!(vm.run().unwrap(), boruna_bytecode::Value::Int(42));
        let event_log_json = vm.event_log().to_json().unwrap();

        // Sanity: the running VM did record a ContractCheck.
        assert!(vm
            .event_log()
            .events()
            .iter()
            .any(|e| matches!(e, Event::ContractCheck { .. })));

        let dir = tempfile::tempdir().unwrap();
        let mut builder =
            EvidenceBundleBuilder::new(dir.path(), "run-contract-001", "contract-wf").unwrap();
        builder.add_workflow_def(r#"{"name":"contract"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        builder.add_file("event_log.json", &event_log_json).unwrap();

        let audit = AuditLog::new();
        let manifest = builder.finalize(&audit).unwrap();

        let bundle_path = dir.path().join("run-contract-001");
        // The event log is a checksummed, hash-covered component.
        assert!(manifest.file_checksums.contains_key("event_log.json"));

        // The pristine bundle verifies.
        let ok = verify_bundle(&bundle_path);
        assert!(ok.valid, "expected valid bundle, errors: {:?}", ok.errors);

        // Read the sealed log back and confirm the ContractCheck survived.
        let sealed = std::fs::read_to_string(bundle_path.join("event_log.json")).unwrap();
        let restored = EventLog::from_json(&sealed).unwrap();
        let contract = restored
            .events()
            .iter()
            .find(|e| matches!(e, Event::ContractCheck { .. }))
            .expect("ContractCheck event survived into the bundle");
        match contract {
            Event::ContractCheck {
                function,
                kind,
                passed,
                ..
            } => {
                assert_eq!(function, "main");
                assert_eq!(kind, "requires");
                assert!(*passed);
            }
            other => panic!("expected ContractCheck, got {other:?}"),
        }

        // Tampering the sealed contract event breaks verification — the
        // contract trail is inside the hash chain, not decorative.
        std::fs::write(
            bundle_path.join("event_log.json"),
            r#"{"version":2,"events":[]}"#,
        )
        .unwrap();
        let bad = verify_bundle(&bundle_path);
        assert!(!bad.valid, "tampered event_log.json must fail verification");
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
