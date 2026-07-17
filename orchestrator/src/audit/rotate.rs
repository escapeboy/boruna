//! KEK rotation tooling for evidence bundles (post1-T-2.4).
//!
//! Rotation is a manifest-only operation: the DEK is unwrapped with
//! the old KEK, re-wrapped under the new KEK, and the manifest is
//! atomically rewritten with the new wrapping. The bundle's
//! ciphertext files are NOT touched — per-file AES-GCM tags depend
//! on the DEK, which is unchanged.
//!
//! The single-bundle path is [`rotate_bundle`]; the directory-of-
//! bundles parallel path is [`rotate_dir`].
//!
//! ## Atomicity
//!
//! Per-bundle: read manifest → unwrap → rewrap → write to
//! `manifest.new.json` (sibling) → rename to `manifest.json`. A
//! crash between rename'd manifests on different bundles leaves the
//! already-rotated bundles fully rotated and the remaining bundles
//! unchanged — no half-rotated states.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::audit::encryption::{EncryptionError, Envelope, KEY_LEN};
use crate::audit::evidence::BundleManifest;

/// Outcome of a single-bundle rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotationOutcome {
    /// Manifest rewritten with the new wrapping. Carries the new
    /// `kek_id` for operator-facing reporting.
    Rotated { new_kek_id: String },
    /// `--dry-run`: nothing was written.
    PlannedDryRun { new_kek_id: String },
    /// Bundle is plaintext (no `encryption` block) — nothing to do.
    /// Reported rather than skipped silently so the operator can
    /// see the count in batch mode.
    NotEncrypted,
}

#[derive(Debug)]
pub enum RotationError {
    Io(std::io::Error),
    InvalidManifest(String),
    Encryption(EncryptionError),
    /// The bundle's `kek_id` doesn't match `--kek-id-from`. When the
    /// operator supplies a `kek_id_from` filter (e.g. they rotated
    /// in a prior pass) we MUST refuse rather than silently skip.
    KekIdMismatch {
        expected: String,
        found: String,
    },
}

impl std::fmt::Display for RotationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RotationError::Io(e) => write!(f, "io: {e}"),
            RotationError::InvalidManifest(s) => write!(f, "invalid manifest: {s}"),
            RotationError::Encryption(e) => write!(f, "encryption: {e}"),
            RotationError::KekIdMismatch { expected, found } => write!(
                f,
                "bundle kek_id is '{found}'; --kek-id-from is '{expected}'"
            ),
        }
    }
}

impl std::error::Error for RotationError {}

impl From<std::io::Error> for RotationError {
    fn from(e: std::io::Error) -> Self {
        RotationError::Io(e)
    }
}

impl From<EncryptionError> for RotationError {
    fn from(e: EncryptionError) -> Self {
        RotationError::Encryption(e)
    }
}

/// Options for a rotation operation.
#[derive(Debug, Clone)]
pub struct RotateOptions {
    pub old_kek: [u8; KEY_LEN],
    pub new_kek: [u8; KEY_LEN],
    /// When `Some`, only rotate bundles whose current `kek_id`
    /// equals this value. Defends against accidental double-rotation.
    pub kek_id_from: Option<String>,
    /// New `kek_id` to write into the manifest.
    pub kek_id_to: String,
    /// When `true`, validate the rotation but do not write anything
    /// to disk.
    pub dry_run: bool,
}

/// Rotate the manifest of a single bundle directory.
///
/// `bundle_dir` is the path that contains `manifest.json`. The
/// manifest is read, the DEK is unwrapped with `opts.old_kek`,
/// re-wrapped under `opts.new_kek`/`opts.kek_id_to`, and the
/// manifest is atomically rewritten unless `dry_run`.
pub fn rotate_bundle(
    bundle_dir: &Path,
    opts: &RotateOptions,
) -> Result<RotationOutcome, RotationError> {
    let manifest_path = bundle_dir.join("manifest.json");
    let raw = std::fs::read(&manifest_path)?;
    let mut manifest: BundleManifest = serde_json::from_slice(&raw)
        .map_err(|e| RotationError::InvalidManifest(format!("parse: {e}")))?;

    let Some(info) = manifest.encryption.clone() else {
        return Ok(RotationOutcome::NotEncrypted);
    };

    if let Some(expected) = &opts.kek_id_from {
        if &info.kek_id != expected {
            return Err(RotationError::KekIdMismatch {
                expected: expected.clone(),
                found: info.kek_id.clone(),
            });
        }
    }

    // Unwrap with old KEK.
    let envelope = Envelope::unwrap(&info, &opts.old_kek)?;
    // Re-wrap under new KEK.
    let rewrapped = envelope.rewrap(&opts.new_kek, &opts.kek_id_to)?;
    // `Envelope` now implements `Drop` (F10 DEK zeroize), so `info`
    // can't be moved out; clone the (non-secret) metadata.
    let new_info = rewrapped.info.clone();

    if opts.dry_run {
        return Ok(RotationOutcome::PlannedDryRun {
            new_kek_id: new_info.kek_id,
        });
    }

    // Update manifest with the new encryption block + recomputed
    // bundle_hash. The bundle_hash is sha256 of the
    // pretty-printed manifest with bundle_hash itself zeroed,
    // matching the build-time computation in
    // `EvidenceBundleBuilder::finalize`.
    manifest.encryption = Some(new_info.clone());
    let new_bundle_hash = compute_bundle_hash(&manifest)?;
    manifest.bundle_hash = new_bundle_hash;

    let final_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| RotationError::InvalidManifest(format!("re-serialize: {e}")))?;

    // Atomic write: sibling tmp + rename. Use a per-process tmp
    // suffix (project §38) so concurrent rotations on the same
    // bundle (shouldn't happen, but defense in depth) don't collide.
    let pid = std::process::id();
    let tmp = bundle_dir.join(format!("manifest.tmp.{pid}.json"));
    std::fs::write(&tmp, final_json.as_bytes())?;
    std::fs::rename(&tmp, &manifest_path)?;

    Ok(RotationOutcome::Rotated {
        new_kek_id: new_info.kek_id,
    })
}

/// Rotate every bundle directory under `parent`. Each immediate
/// subdirectory containing a `manifest.json` is treated as a bundle.
/// Rotation runs in parallel via rayon, bounded by `parallelism`.
///
/// On success returns one `(bundle_dir, outcome)` per bundle, in
/// path order. On per-bundle failure, the failing entry returns
/// `Err`; already-rotated bundles stay rotated. The outer `Result`
/// only fails if listing `parent` fails.
pub fn rotate_dir(
    parent: &Path,
    opts: &RotateOptions,
    parallelism: usize,
) -> std::io::Result<Vec<(PathBuf, Result<RotationOutcome, RotationError>)>> {
    let mut bundles: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if path.join("manifest.json").is_file() {
            bundles.push(path);
        }
    }
    bundles.sort();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(parallelism.max(1))
        .build()
        .map_err(std::io::Error::other)?;

    let results: Vec<(PathBuf, Result<RotationOutcome, RotationError>)> = pool.install(|| {
        bundles
            .par_iter()
            .map(|bundle_dir| {
                let outcome = rotate_bundle(bundle_dir, opts);
                (bundle_dir.clone(), outcome)
            })
            .collect()
    });

    Ok(results)
}

/// Compute the bundle hash exactly the way `EvidenceBundleBuilder::finalize`
/// does at build time: pretty-print the manifest with its
/// `bundle_hash` field reset to empty, then sha256 the result.
fn compute_bundle_hash(manifest: &BundleManifest) -> Result<String, RotationError> {
    let mut clone = manifest.clone();
    clone.bundle_hash = String::new();
    // The signature is excluded from bundle_hash by construction
    // (it is added AFTER the hash in finalize), so clear it here to
    // match that convention. Rotating a signed bundle invalidates its
    // signature — verify surfaces the mismatch.
    clone.signature = None;
    let json = serde_json::to_string_pretty(&clone)
        .map_err(|e| RotationError::InvalidManifest(format!("hash-prep: {e}")))?;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::evidence::EvidenceBundleBuilder;
    use crate::audit::log::{AuditEvent, AuditLog};
    use crate::audit::verify::verify_bundle_with_kek;

    fn make_kek(seed: u8) -> [u8; KEY_LEN] {
        let mut k = [0u8; KEY_LEN];
        for (i, b) in k.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8);
        }
        k
    }

    fn build_encrypted_bundle(
        dir: &Path,
        run_id: &str,
        kek: &[u8; KEY_LEN],
        kek_id: &str,
    ) -> PathBuf {
        let mut builder = EvidenceBundleBuilder::new(dir, run_id, "rotate-test").unwrap();
        builder = builder.with_encryption(kek, kek_id).unwrap();
        builder.add_workflow_def("{\"name\":\"x\"}").unwrap();
        builder.add_policy("{}").unwrap();
        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "wf".into(),
            policy_hash: "po".into(),
        });
        builder.finalize(&audit).unwrap();
        dir.join(run_id)
    }

    #[test]
    fn rotate_then_verify_with_new_kek() {
        let dir = tempfile::tempdir().unwrap();
        let old = make_kek(1);
        let new = make_kek(2);
        let bundle = build_encrypted_bundle(dir.path(), "r-1", &old, "key-old");

        let outcome = rotate_bundle(
            &bundle,
            &RotateOptions {
                old_kek: old,
                new_kek: new,
                kek_id_from: None,
                kek_id_to: "key-new".into(),
                dry_run: false,
            },
        )
        .unwrap();
        assert_eq!(
            outcome,
            RotationOutcome::Rotated {
                new_kek_id: "key-new".into()
            }
        );

        // Verify with NEW kek succeeds.
        let report = verify_bundle_with_kek(&bundle, Some(&new));
        assert!(report.valid, "report: {report:?}");

        // Verify with OLD kek fails (key mismatch).
        let report = verify_bundle_with_kek(&bundle, Some(&old));
        assert!(!report.valid, "old kek should not verify: {report:?}");
    }

    #[test]
    fn dry_run_does_not_modify() {
        let dir = tempfile::tempdir().unwrap();
        let old = make_kek(1);
        let new = make_kek(2);
        let bundle = build_encrypted_bundle(dir.path(), "r-2", &old, "key-old");

        let manifest_before = std::fs::read(bundle.join("manifest.json")).unwrap();

        let outcome = rotate_bundle(
            &bundle,
            &RotateOptions {
                old_kek: old,
                new_kek: new,
                kek_id_from: None,
                kek_id_to: "key-new".into(),
                dry_run: true,
            },
        )
        .unwrap();
        assert_eq!(
            outcome,
            RotationOutcome::PlannedDryRun {
                new_kek_id: "key-new".into()
            }
        );

        let manifest_after = std::fs::read(bundle.join("manifest.json")).unwrap();
        assert_eq!(
            manifest_before, manifest_after,
            "dry-run must not modify the manifest"
        );

        // Old KEK still works (rotation didn't happen).
        let report = verify_bundle_with_kek(&bundle, Some(&old));
        assert!(report.valid);
    }

    #[test]
    fn kek_id_from_mismatch_rejects() {
        let dir = tempfile::tempdir().unwrap();
        let old = make_kek(1);
        let new = make_kek(2);
        let bundle = build_encrypted_bundle(dir.path(), "r-3", &old, "key-old");

        let err = rotate_bundle(
            &bundle,
            &RotateOptions {
                old_kek: old,
                new_kek: new,
                kek_id_from: Some("key-different".into()),
                kek_id_to: "key-new".into(),
                dry_run: false,
            },
        )
        .unwrap_err();
        match err {
            RotationError::KekIdMismatch { expected, found } => {
                assert_eq!(expected, "key-different");
                assert_eq!(found, "key-old");
            }
            other => panic!("expected KekIdMismatch, got {other:?}"),
        }
    }

    #[test]
    fn rotate_dir_processes_only_bundle_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let old = make_kek(1);
        let new = make_kek(2);
        build_encrypted_bundle(dir.path(), "r-a", &old, "key-old");
        build_encrypted_bundle(dir.path(), "r-b", &old, "key-old");
        // A non-bundle subdirectory is ignored (no manifest.json).
        std::fs::create_dir_all(dir.path().join("not-a-bundle")).unwrap();
        std::fs::write(dir.path().join("not-a-bundle").join("readme.txt"), b"x").unwrap();

        let results = rotate_dir(
            dir.path(),
            &RotateOptions {
                old_kek: old,
                new_kek: new,
                kek_id_from: None,
                kek_id_to: "key-new".into(),
                dry_run: false,
            },
            2,
        )
        .unwrap();

        assert_eq!(results.len(), 2, "should pick up r-a and r-b only");
        for (_, outcome) in &results {
            assert!(matches!(outcome, Ok(RotationOutcome::Rotated { .. })));
        }
    }

    #[test]
    fn rotate_dir_per_bundle_failure_does_not_abort_others() {
        let dir = tempfile::tempdir().unwrap();
        let old = make_kek(1);
        let new = make_kek(2);
        build_encrypted_bundle(dir.path(), "good", &old, "key-old");
        // A bundle whose manifest is corrupt — rotation should fail
        // for it, but the good bundle should still rotate.
        let bad = dir.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("manifest.json"), b"{not-json").unwrap();

        let results = rotate_dir(
            dir.path(),
            &RotateOptions {
                old_kek: old,
                new_kek: new,
                kek_id_from: None,
                kek_id_to: "key-new".into(),
                dry_run: false,
            },
            2,
        )
        .unwrap();

        let by_name: std::collections::BTreeMap<_, _> = results
            .into_iter()
            .map(|(p, r)| (p.file_name().unwrap().to_string_lossy().to_string(), r))
            .collect();
        assert!(matches!(
            by_name["good"],
            Ok(RotationOutcome::Rotated { .. })
        ));
        assert!(by_name["bad"].is_err());
    }

    #[test]
    fn plaintext_bundle_is_reported_not_rotated() {
        let dir = tempfile::tempdir().unwrap();
        let old = make_kek(1);
        let new = make_kek(2);
        // Plaintext bundle (no `with_encryption`).
        let mut builder = EvidenceBundleBuilder::new(dir.path(), "plain", "rotate-test").unwrap();
        builder.add_workflow_def("{}").unwrap();
        builder.add_policy("{}").unwrap();
        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "wf".into(),
            policy_hash: "po".into(),
        });
        builder.finalize(&audit).unwrap();

        let outcome = rotate_bundle(
            &dir.path().join("plain"),
            &RotateOptions {
                old_kek: old,
                new_kek: new,
                kek_id_from: None,
                kek_id_to: "key-new".into(),
                dry_run: false,
            },
        )
        .unwrap();
        assert_eq!(outcome, RotationOutcome::NotEncrypted);
    }
}
