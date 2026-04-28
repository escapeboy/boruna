use sha2::{Digest, Sha256};
use std::path::Path;

use crate::audit::encryption::{EncryptionError, Envelope, KEY_LEN};
use crate::audit::evidence::BundleManifest;
use crate::audit::log::AuditLog;

/// Verification result.
#[derive(Debug)]
pub struct VerifyResult {
    pub valid: bool,
    pub errors: Vec<String>,
}

/// Verify an evidence bundle directory for integrity.
///
/// For unencrypted bundles this is a pure on-disk check. For
/// encrypted bundles (sprint W6-B), the KEK is resolved from
/// `BORUNA_BUNDLE_KEK` env var; if absent the result includes an
/// `evidence.encryption_key_required` error and verification stops.
pub fn verify_bundle(bundle_dir: &Path) -> VerifyResult {
    verify_bundle_with_kek(bundle_dir, None)
}

/// Verify an evidence bundle, optionally with an explicit KEK
/// supplied by the caller (CLI flag path). Falls back to env var
/// when `kek` is `None`.
pub fn verify_bundle_with_kek(bundle_dir: &Path, kek: Option<&[u8; KEY_LEN]>) -> VerifyResult {
    let mut errors = Vec::new();

    // 1. Load and parse manifest
    let manifest_path = bundle_dir.join("manifest.json");
    let manifest_json = match std::fs::read_to_string(&manifest_path) {
        Ok(j) => j,
        Err(e) => {
            return VerifyResult {
                valid: false,
                errors: vec![format!("cannot read manifest.json: {e}")],
            };
        }
    };

    let manifest: BundleManifest = match serde_json::from_str(&manifest_json) {
        Ok(m) => m,
        Err(e) => {
            return VerifyResult {
                valid: false,
                errors: vec![format!("invalid manifest.json: {e}")],
            };
        }
    };

    // 2. Resolve envelope when the bundle is encrypted.
    let envelope = match &manifest.encryption {
        Some(info) => {
            let resolved_kek = match kek {
                Some(k) => Some(*k),
                None => match crate::audit::encryption::resolve_kek(None) {
                    Ok(k) => k,
                    Err(e) => {
                        return VerifyResult {
                            valid: false,
                            errors: vec![format!("invalid KEK: {e}")],
                        };
                    }
                },
            };
            let key = match resolved_kek {
                Some(k) => k,
                None => {
                    return VerifyResult {
                        valid: false,
                        errors: vec![format!(
                            "evidence.encryption_key_required: bundle is encrypted (kek_id={}); \
                             supply --bundle-encryption-key <hex> or set BORUNA_BUNDLE_KEK",
                            info.kek_id
                        )],
                    };
                }
            };
            match Envelope::unwrap(info, &key) {
                Ok(env) => Some(env),
                Err(EncryptionError::EncryptionKeyMismatch) => {
                    return VerifyResult {
                        valid: false,
                        errors: vec![format!(
                            "evidence.encryption_key_mismatch: supplied KEK does not unwrap the \
                             bundle's wrapped_dek (kek_id={})",
                            info.kek_id
                        )],
                    };
                }
                Err(e) => {
                    return VerifyResult {
                        valid: false,
                        errors: vec![format!("invalid encryption metadata: {e}")],
                    };
                }
            }
        }
        None => None,
    };

    // 3. Verify file checksums (decrypt-in-memory if needed).
    for (filename, expected_hash) in &manifest.file_checksums {
        let file_path = bundle_dir.join(filename);
        let raw = match std::fs::read(&file_path) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("cannot read {filename}: {e}"));
                continue;
            }
        };
        let plaintext_bytes = match &envelope {
            Some(env) => match env.decrypt_file(filename, &raw) {
                Ok(pt) => pt,
                Err(EncryptionError::CipherTagInvalid { file }) => {
                    errors.push(format!(
                        "evidence.cipher_tag_invalid: {file} failed AES-GCM authentication \
                         (bundle tampered or wrong key)"
                    ));
                    continue;
                }
                Err(e) => {
                    errors.push(format!("decrypt {filename}: {e}"));
                    continue;
                }
            },
            None => raw,
        };
        let actual_hash = sha256_bytes(&plaintext_bytes);
        if actual_hash != *expected_hash {
            errors.push(format!(
                "checksum mismatch for {filename}: expected {expected_hash}, got {actual_hash}"
            ));
        }
    }

    // 4. Verify audit log chain integrity (decrypt-then-parse).
    let audit_path = bundle_dir.join("audit_log.json");
    match std::fs::read(&audit_path) {
        Ok(raw) => {
            let audit_pt = match &envelope {
                // On decrypt failure here we return Vec::new() so the
                // chain check is skipped — the same tamper is already
                // reported via the file_checksums loop, no
                // double-error.
                Some(env) => env.decrypt_file("audit_log.json", &raw).unwrap_or_default(),
                None => raw,
            };
            if !audit_pt.is_empty() {
                match std::str::from_utf8(&audit_pt) {
                    Ok(audit_json) => match AuditLog::from_json(audit_json) {
                        Ok(audit_log) => {
                            if let Err(bad_seq) = audit_log.verify() {
                                errors.push(format!("audit log chain broken at entry {bad_seq}"));
                            }
                            if audit_log.hash() != manifest.audit_log_hash {
                                errors.push(format!(
                                    "audit log hash mismatch: manifest says {}, actual is {}",
                                    manifest.audit_log_hash,
                                    audit_log.hash()
                                ));
                            }
                        }
                        Err(e) => {
                            errors.push(format!("invalid audit_log.json: {e}"));
                        }
                    },
                    Err(_) => {
                        errors.push("audit_log.json is not valid UTF-8".into());
                    }
                }
            }
        }
        Err(_) => errors.push("missing audit_log.json".into()),
    }

    // 5. Verify required files exist
    for required in &[
        "manifest.json",
        "workflow.json",
        "policy.json",
        "audit_log.json",
        "env_fingerprint.json",
    ] {
        if !bundle_dir.join(required).exists() {
            errors.push(format!("missing required file: {required}"));
        }
    }

    VerifyResult {
        valid: errors.is_empty(),
        errors,
    }
}

fn sha256_bytes(b: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::evidence::EvidenceBundleBuilder;
    use crate::audit::log::{AuditEvent, AuditLog};

    fn build_valid_bundle(dir: &Path) -> BundleManifest {
        let mut builder = EvidenceBundleBuilder::new(dir, "run-verify-001", "verify-test").unwrap();
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        builder
            .add_step_output("s1", "result", r#"{"value":1}"#)
            .unwrap();

        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        audit.append(AuditEvent::StepStarted {
            step_id: "s1".into(),
            input_hash: "inp".into(),
        });
        audit.append(AuditEvent::StepCompleted {
            step_id: "s1".into(),
            output_hash: "out".into(),
            duration_ms: 50,
        });
        audit.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 60,
        });

        builder.finalize(&audit).unwrap()
    }

    #[test]
    fn test_verify_valid_bundle() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        let result = verify_bundle(&bundle_dir);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_verify_detects_tampered_file() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        // Tamper with workflow.json
        std::fs::write(bundle_dir.join("workflow.json"), r#"{"name":"TAMPERED"}"#).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("checksum mismatch")));
    }

    #[test]
    fn test_verify_detects_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        // Remove a required file
        std::fs::remove_file(bundle_dir.join("policy.json")).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("cannot read policy.json")));
    }

    #[test]
    fn test_verify_detects_tampered_audit_log() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        // Read and tamper with the audit log
        let audit_path = bundle_dir.join("audit_log.json");
        let mut audit_json = std::fs::read_to_string(&audit_path).unwrap();
        audit_json = audit_json.replace("abc", "TAMPERED");
        std::fs::write(&audit_path, &audit_json).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        // Should detect either chain break or checksum mismatch
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_verify_nonexistent_dir() {
        let result = verify_bundle(Path::new("/nonexistent/path"));
        assert!(!result.valid);
    }
}
