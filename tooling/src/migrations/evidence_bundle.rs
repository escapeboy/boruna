//! Evidence-bundle migrator (sprint `W5-C`).
//!
//! Sprint `W1-C` introduced a `bundle.json` summary file that every
//! evidence bundle is expected to carry. Bundles produced by Boruna
//! v0.5.0 and earlier predate that sprint and lack the file. This
//! migrator synthesizes a `bundle.json` from artifacts that DO exist
//! (the audit log, output files, the workflow definition) so legacy
//! bundles become discoverable and inspectable by 0.6.0+ tooling.
//!
//! What the migrator does NOT do: it does not synthesize the canonical
//! `manifest.json` (with per-file SHA-256 checksums) that
//! [`boruna_orchestrator::audit::verify_bundle`] needs for full
//! integrity verification. A legacy bundle that never had a manifest
//! can never produce one with the same hashes after the fact —
//! attempting to do so would manufacture a false integrity guarantee.
//! When `manifest.json` is present we re-run `verify_bundle` after
//! synthesis as a cross-check; when it is absent we report that fact
//! plainly in [`MigrationReport::changes`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use boruna_orchestrator::audit::log::{AuditEvent, AuditLog};
use boruna_orchestrator::audit::verify::verify_bundle;

use super::{MigrationError, MigrationReport};

/// Default `boruna_version` written into a synthesized bundle.json
/// when no metadata recoverable from the source bundle pins it down.
pub const DEFAULT_SYNTHESIZED_BORUNA_VERSION: &str = "0.6.0-pre";

/// Format version this migrator emits.
pub const SYNTHESIZED_FORMAT_VERSION: &str = "1.0";

/// On-disk shape of `bundle.json`.
///
/// This is the lightweight summary file synthesized by this migrator;
/// it is intentionally separate from the rich `manifest.json`
/// (`BundleManifest` in `boruna-orchestrator`) which carries every
/// per-file checksum. `bundle.json` answers "what's in this directory
/// and roughly when was it produced" without claiming integrity it
/// cannot prove.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SynthesizedBundleJson {
    pub format_version: String,
    pub boruna_version: String,
    pub created_at: String,
    pub run_id: String,
    pub workflow_hash: String,
    /// Filenames present in the bundle directory at synthesis time
    /// (sorted, relative to the bundle root).
    pub components: Vec<String>,
    /// Set true when this file was synthesized after-the-fact by
    /// `boruna migrate`, false when written natively by the runner.
    pub synthesized: bool,
}

/// Run the evidence-bundle migrator.
///
/// `bundle_dir` must be an existing directory. When `dry_run` is true
/// no file is written; the planned `bundle.json` content is computed
/// and discarded. When `in_place` is true the migrator writes
/// `bundle.json` directly into `bundle_dir`; otherwise it writes
/// `bundle_dir/bundle.json.migrated` so operators can diff before
/// swapping.
pub fn migrate_bundle_dir(
    bundle_dir: &Path,
    dry_run: bool,
    in_place: bool,
) -> Result<MigrationReport, MigrationError> {
    if !bundle_dir.is_dir() {
        return Err(MigrationError::Malformed(format!(
            "{} is not a directory",
            bundle_dir.display()
        )));
    }

    let bundle_json_path = bundle_dir.join("bundle.json");
    if bundle_json_path.exists() {
        return Ok(MigrationReport {
            kind: "evidence-bundle".into(),
            no_op: true,
            dry_run,
            written_path: None,
            changes: vec![],
        });
    }

    let synthesized = synthesize(bundle_dir)?;

    let json = serde_json::to_string_pretty(&synthesized)
        .map_err(|e| MigrationError::Malformed(format!("serialize bundle.json: {e}")))?;

    let target = if in_place {
        bundle_json_path
    } else {
        bundle_dir.join("bundle.json.migrated")
    };

    let mut changes = vec![format!(
        "synthesize bundle.json (format_version={}, run_id={}, components={})",
        synthesized.format_version,
        synthesized.run_id,
        synthesized.components.len()
    )];

    if !dry_run {
        std::fs::write(&target, &json)?;
    }

    // Cross-check via the orchestrator's verify_bundle when manifest.json
    // is present. We don't synthesize manifest.json (see module docs),
    // so a legacy bundle without one cannot reach `valid: true` here —
    // surface that honestly rather than silently skipping.
    let manifest_present = bundle_dir.join("manifest.json").exists();
    if manifest_present {
        // Use the actual on-disk path for verification: in dry-run we
        // didn't write bundle.json, but verify_bundle doesn't read it
        // (it reads manifest.json), so the result is well-defined
        // either way.
        let result = verify_bundle(bundle_dir);
        if result.valid {
            changes.push("verify_bundle (manifest.json present): PASS".into());
        } else {
            changes.push(format!(
                "verify_bundle (manifest.json present): FAIL ({} error(s))",
                result.errors.len()
            ));
        }
    } else {
        changes.push(
            "manifest.json absent — full integrity verification not possible \
             (synthesized bundle.json is a summary, not a checksum manifest)"
                .into(),
        );
    }

    Ok(MigrationReport {
        kind: "evidence-bundle".into(),
        no_op: false,
        dry_run,
        written_path: Some(target),
        changes,
    })
}

fn synthesize(bundle_dir: &Path) -> Result<SynthesizedBundleJson, MigrationError> {
    // Collect components (top-level filenames + relative subpaths).
    let components = collect_components(bundle_dir)?;

    // Recover what we can from audit_log.json (or .jsonl as a fallback
    // tolerated for forward-compatibility).
    let (run_id, workflow_hash, created_at) = recover_metadata(bundle_dir)?;

    Ok(SynthesizedBundleJson {
        format_version: SYNTHESIZED_FORMAT_VERSION.into(),
        boruna_version: DEFAULT_SYNTHESIZED_BORUNA_VERSION.into(),
        created_at,
        run_id,
        workflow_hash,
        components,
        synthesized: true,
    })
}

fn collect_components(bundle_dir: &Path) -> Result<Vec<String>, MigrationError> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    walk(bundle_dir, bundle_dir, &mut out)?;
    Ok(out.into_iter().collect())
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) -> Result<(), MigrationError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk(root, &path, out)?;
        } else {
            // Relative path with forward slashes for cross-platform
            // determinism.
            let rel = path
                .strip_prefix(root)
                .map_err(|e| MigrationError::Malformed(format!("path strip: {e}")))?
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel);
        }
    }
    Ok(())
}

/// Best-effort metadata recovery. Returns (run_id, workflow_hash, created_at).
///
/// `run_id`: derived from the bundle directory name (the runner names
/// the directory after the run id).
///
/// `workflow_hash`: pulled from the first `WorkflowStarted` event in
/// `audit_log.json` if present; empty string otherwise (we do not
/// fabricate one — an empty hash is recognizable as "unknown").
///
/// `created_at`: mtime of `audit_log.json` (or `.jsonl`) in RFC3339 if
/// readable; falls back to current time. Stable across operating
/// systems via `chrono`.
fn recover_metadata(bundle_dir: &Path) -> Result<(String, String, String), MigrationError> {
    let run_id = bundle_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown-run".into());

    let audit_path = find_audit_log(bundle_dir);

    let workflow_hash = match &audit_path {
        Some(p) => extract_workflow_hash(p).unwrap_or_default(),
        None => String::new(),
    };

    let created_at = audit_path
        .as_deref()
        .and_then(mtime_rfc3339)
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

    Ok((run_id, workflow_hash, created_at))
}

fn find_audit_log(bundle_dir: &Path) -> Option<PathBuf> {
    for name in ["audit_log.json", "audit_log.jsonl"] {
        let p = bundle_dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn extract_workflow_hash(audit_path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(audit_path).ok()?;
    if let Ok(log) = AuditLog::from_json(&raw) {
        for entry in log.entries() {
            if let AuditEvent::WorkflowStarted { workflow_hash, .. } = &entry.event {
                return Some(workflow_hash.clone());
            }
        }
    }
    // Tolerate jsonl: parse line-by-line if the bulk decode failed.
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(hash) = value
                .pointer("/event/WorkflowStarted/workflow_hash")
                .and_then(|v| v.as_str())
            {
                return Some(hash.to_string());
            }
        }
    }
    None
}

fn mtime_rfc3339(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let datetime: chrono::DateTime<chrono::Utc> = modified.into();
    Some(datetime.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use boruna_orchestrator::audit::log::{AuditEvent, AuditLog};

    fn fake_legacy_bundle(dir: &Path, run_id: &str, workflow_hash: &str) {
        let bundle_dir = dir.join(run_id);
        std::fs::create_dir_all(&bundle_dir).unwrap();
        std::fs::create_dir_all(bundle_dir.join("outputs/step1")).unwrap();
        std::fs::write(
            bundle_dir.join("outputs/step1/result.json"),
            r#"{"value":42}"#,
        )
        .unwrap();
        std::fs::write(bundle_dir.join("workflow.json"), r#"{"name":"legacy"}"#).unwrap();
        std::fs::write(bundle_dir.join("policy.json"), r#"{"default_allow":true}"#).unwrap();

        let mut log = AuditLog::new();
        log.append(AuditEvent::WorkflowStarted {
            workflow_hash: workflow_hash.into(),
            policy_hash: "polhash".into(),
        });
        log.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 1,
        });
        std::fs::write(bundle_dir.join("audit_log.json"), log.to_json().unwrap()).unwrap();
    }

    #[test]
    fn evidence_bundle_migrator_synthesizes_missing_bundle_json() {
        let tmp = tempfile::tempdir().unwrap();
        fake_legacy_bundle(tmp.path(), "run-001", "wfhash-abc");
        let bundle = tmp.path().join("run-001");

        let report = migrate_bundle_dir(&bundle, false, true).unwrap();
        assert!(!report.no_op, "should not be a no-op for legacy bundle");
        assert!(!report.dry_run);

        let written = bundle.join("bundle.json");
        assert!(written.exists(), "bundle.json should have been written");

        let raw = std::fs::read_to_string(&written).unwrap();
        let parsed: SynthesizedBundleJson = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.format_version, SYNTHESIZED_FORMAT_VERSION);
        assert_eq!(parsed.boruna_version, DEFAULT_SYNTHESIZED_BORUNA_VERSION);
        assert_eq!(parsed.run_id, "run-001");
        assert_eq!(parsed.workflow_hash, "wfhash-abc");
        assert!(parsed.synthesized);
        assert!(parsed.components.iter().any(|c| c == "audit_log.json"));
        assert!(parsed.components.iter().any(|c| c == "workflow.json"));
        assert!(parsed
            .components
            .iter()
            .any(|c| c == "outputs/step1/result.json"));
    }

    #[test]
    fn evidence_bundle_migrator_no_op_on_already_versioned() {
        let tmp = tempfile::tempdir().unwrap();
        fake_legacy_bundle(tmp.path(), "run-002", "h");
        let bundle = tmp.path().join("run-002");
        std::fs::write(
            bundle.join("bundle.json"),
            serde_json::to_string_pretty(&SynthesizedBundleJson {
                format_version: SYNTHESIZED_FORMAT_VERSION.into(),
                boruna_version: "0.6.0".into(),
                created_at: "2026-04-28T00:00:00Z".into(),
                run_id: "run-002".into(),
                workflow_hash: "h".into(),
                components: vec!["audit_log.json".into()],
                synthesized: false,
            })
            .unwrap(),
        )
        .unwrap();

        let report = migrate_bundle_dir(&bundle, false, true).unwrap();
        assert!(report.no_op);
        assert!(report.written_path.is_none());
    }

    #[test]
    fn evidence_bundle_migrator_dry_run_does_not_write() {
        let tmp = tempfile::tempdir().unwrap();
        fake_legacy_bundle(tmp.path(), "run-003", "h");
        let bundle = tmp.path().join("run-003");

        // Snapshot directory contents BEFORE.
        let before: BTreeSet<String> = std::fs::read_dir(&bundle)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();

        let report = migrate_bundle_dir(&bundle, true, false).unwrap();
        assert!(!report.no_op);
        assert!(report.dry_run);
        // Path is reported but file must NOT exist.
        let target = report.written_path.as_ref().unwrap();
        assert!(
            !target.exists(),
            "dry-run must not create {}",
            target.display()
        );

        let after: BTreeSet<String> = std::fs::read_dir(&bundle)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(before, after, "dry-run must not alter directory listing");
    }

    #[test]
    fn evidence_bundle_migrator_rejects_non_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-dir");
        std::fs::write(&file, "x").unwrap();
        let err = migrate_bundle_dir(&file, false, true).unwrap_err();
        match err {
            MigrationError::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn evidence_bundle_migrator_writes_sibling_when_not_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        fake_legacy_bundle(tmp.path(), "run-004", "h");
        let bundle = tmp.path().join("run-004");

        let report = migrate_bundle_dir(&bundle, false, false).unwrap();
        assert!(!report.no_op);
        let target = report.written_path.as_ref().unwrap();
        assert_eq!(target.file_name().unwrap(), "bundle.json.migrated");
        assert!(target.exists());
        assert!(!bundle.join("bundle.json").exists());
    }
}
