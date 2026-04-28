use sha2::{Digest, Sha256};
use std::path::Path;

use crate::audit::evidence::{BundleJson, BundleManifest};
use crate::audit::log::AuditLog;
use crate::audit::BUNDLE_FORMAT_VERSION;

/// Errors raised by the bundle reader gate that callers may want to
/// match on programmatically (vs. the free-form strings in
/// `VerifyResult::errors`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceError {
    /// `bundle.json` is missing or has no `format_version` field, OR the
    /// version's major component does not match the reader's supported
    /// major (semver-like compat: `1.x` is forward-compat for a `1.0`
    /// reader; `2.x` is rejected).
    UnsupportedFormat { found: String, expected: String },
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvidenceError::UnsupportedFormat { found, expected } => {
                write!(
                    f,
                    "unsupported evidence bundle format_version: found `{found}`, expected major `{expected}`"
                )
            }
        }
    }
}

impl std::error::Error for EvidenceError {}

/// Verification result.
#[derive(Debug)]
pub struct VerifyResult {
    pub valid: bool,
    pub errors: Vec<String>,
}

/// Read and validate the top-level `bundle.json` format gate.
///
/// Implements §1 ("reject at parse, don't silently override"):
/// - Legacy bundles (pre-1.0, no `bundle.json`) → reject with a clear
///   migration hint pointing at `boruna migrate evidence-bundle` (W5-C).
/// - Future-major bundles (`2.x`) → reject — the reader can't safely
///   interpret an unknown major.
/// - Same-major bundles (`1.x`) → accept (forward-compat: unknown
///   fields are ignored).
pub fn check_bundle_format(bundle_dir: &Path) -> Result<BundleJson, EvidenceError> {
    let bundle_path = bundle_dir.join("bundle.json");
    let raw = match std::fs::read_to_string(&bundle_path) {
        Ok(s) => s,
        Err(_) => {
            return Err(EvidenceError::UnsupportedFormat {
                found: "missing bundle.json (legacy bundle from pre-1.0 release; use `boruna migrate evidence-bundle` to upgrade)".to_string(),
                expected: BUNDLE_FORMAT_VERSION.to_string(),
            });
        }
    };
    let parsed: BundleJson = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            return Err(EvidenceError::UnsupportedFormat {
                found: format!("invalid bundle.json: {e}"),
                expected: BUNDLE_FORMAT_VERSION.to_string(),
            });
        }
    };
    let major = parsed.format_version.split('.').next().unwrap_or("");
    let expected_major = BUNDLE_FORMAT_VERSION.split('.').next().unwrap_or("1");
    if major.is_empty() || major != expected_major {
        return Err(EvidenceError::UnsupportedFormat {
            found: parsed.format_version.clone(),
            expected: format!("{expected_major}.x"),
        });
    }
    Ok(parsed)
}

/// Verify an evidence bundle directory for integrity.
pub fn verify_bundle(bundle_dir: &Path) -> VerifyResult {
    let mut errors = Vec::new();

    // 0. Format-version gate: reject pre-1.0 legacy bundles and any
    //    bundle from a future incompatible major. This runs FIRST so
    //    we don't try to read content we can't safely interpret.
    if let Err(e) = check_bundle_format(bundle_dir) {
        return VerifyResult {
            valid: false,
            errors: vec![e.to_string()],
        };
    }

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

    // 2. Verify file checksums
    for (filename, expected_hash) in &manifest.file_checksums {
        let file_path = bundle_dir.join(filename);
        match std::fs::read_to_string(&file_path) {
            Ok(content) => {
                let actual_hash = sha256_str(&content);
                if actual_hash != *expected_hash {
                    errors.push(format!(
                        "checksum mismatch for {filename}: expected {expected_hash}, got {actual_hash}"
                    ));
                }
            }
            Err(e) => {
                errors.push(format!("cannot read {filename}: {e}"));
            }
        }
    }

    // 3. Verify audit log chain integrity
    let audit_path = bundle_dir.join("audit_log.json");
    if let Ok(audit_json) = std::fs::read_to_string(&audit_path) {
        match AuditLog::from_json(&audit_json) {
            Ok(audit_log) => {
                if let Err(bad_seq) = audit_log.verify() {
                    errors.push(format!("audit log chain broken at entry {bad_seq}"));
                }
                // Verify audit log hash matches manifest
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
        }
    } else {
        errors.push("missing audit_log.json".into());
    }

    // 4. Verify required files exist
    for required in &[
        "bundle.json",
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

fn sha256_str(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
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

    #[test]
    fn verify_rejects_missing_format_version() {
        // A legacy bundle has no bundle.json. The reader gate must
        // reject it before any further checks run.
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        std::fs::remove_file(bundle_dir.join("bundle.json")).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        let joined = result.errors.join(" ");
        assert!(
            joined.contains("legacy bundle") || joined.contains("missing bundle.json"),
            "expected legacy-bundle hint, got: {:?}",
            result.errors
        );
        assert!(
            joined.contains("boruna migrate evidence-bundle"),
            "expected migration hint, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn verify_rejects_future_major_version() {
        // A 2.x bundle MUST be rejected — major-version bumps are
        // breaking by spec.
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        let bundle_path = bundle_dir.join("bundle.json");
        let raw = std::fs::read_to_string(&bundle_path).unwrap();
        let mut parsed: BundleJson = serde_json::from_str(&raw).unwrap();
        parsed.format_version = "2.0".to_string();
        std::fs::write(&bundle_path, serde_json::to_string_pretty(&parsed).unwrap()).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        let joined = result.errors.join(" ");
        assert!(
            joined.contains("unsupported evidence bundle format_version") && joined.contains("2.0"),
            "expected unsupported-format error, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn verify_accepts_minor_bump() {
        // A 1.5 bundle MUST be accepted by a 1.0 reader — same-major
        // means forward-compat (unknown fields ignored).
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        let bundle_path = bundle_dir.join("bundle.json");
        let raw = std::fs::read_to_string(&bundle_path).unwrap();
        let mut parsed: BundleJson = serde_json::from_str(&raw).unwrap();
        parsed.format_version = "1.5".to_string();
        std::fs::write(&bundle_path, serde_json::to_string_pretty(&parsed).unwrap()).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(
            result.valid,
            "1.5 bundle should pass 1.0 reader; got errors: {:?}",
            result.errors
        );
    }
}
