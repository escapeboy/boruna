//! Workflow-JSON migrator (sprint `W5-C`).
//!
//! Sprint `W4` introduced the `schema_version` field on
//! `workflow.json`. This migrator adds `"schema_version": 1` to files
//! produced before W4 landed. Files already at `schema_version: 1` are
//! a no-op; files claiming a higher major schema version are rejected
//! with [`MigrationError::UnsupportedFutureVersion`] (downgrade is out
//! of scope for the beta).

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

use super::{MigrationError, MigrationReport};

/// Schema version the migrator targets ("current").
pub const TARGET_SCHEMA_VERSION: u64 = 1;

/// Run the workflow-json migrator against a single file.
///
/// `path` must point to an existing JSON file. When `dry_run` is true,
/// no write happens. When `in_place` is true, the input is overwritten;
/// otherwise the migrated content is written to `<path>.migrated`.
pub fn migrate_workflow_json(
    path: &Path,
    dry_run: bool,
    in_place: bool,
) -> Result<MigrationReport, MigrationError> {
    if !path.is_file() {
        return Err(MigrationError::Malformed(format!(
            "{} is not a file",
            path.display()
        )));
    }

    let raw = std::fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|e| MigrationError::Malformed(format!("invalid JSON: {e}")))?;

    let obj = match value {
        Value::Object(m) => m,
        _ => {
            return Err(MigrationError::Malformed(
                "workflow.json root must be a JSON object".into(),
            ));
        }
    };

    let action = classify(&obj)?;

    match action {
        SchemaAction::AlreadyTarget => Ok(MigrationReport {
            kind: "workflow-json".into(),
            no_op: true,
            dry_run,
            written_path: None,
            changes: vec![],
        }),
        SchemaAction::AddMissing => {
            let migrated_obj = insert_schema_version(obj, TARGET_SCHEMA_VERSION);
            let migrated_value = Value::Object(migrated_obj);
            let json = serde_json::to_string_pretty(&migrated_value)
                .map_err(|e| MigrationError::Malformed(format!("serialize: {e}")))?;

            let target: PathBuf = if in_place {
                path.to_path_buf()
            } else {
                let mut t = path.as_os_str().to_owned();
                t.push(".migrated");
                PathBuf::from(t)
            };

            if !dry_run {
                std::fs::write(&target, &json)?;
            }

            Ok(MigrationReport {
                kind: "workflow-json".into(),
                no_op: false,
                dry_run,
                written_path: Some(target),
                changes: vec![format!(
                    "add schema_version: {} (was missing)",
                    TARGET_SCHEMA_VERSION
                )],
            })
        }
    }
}

enum SchemaAction {
    AlreadyTarget,
    AddMissing,
}

fn classify(obj: &Map<String, Value>) -> Result<SchemaAction, MigrationError> {
    match obj.get("schema_version") {
        None => Ok(SchemaAction::AddMissing),
        Some(Value::Number(n)) => {
            let v = n.as_u64().ok_or_else(|| {
                MigrationError::Malformed(format!(
                    "schema_version must be a non-negative integer, got {n}"
                ))
            })?;
            if v == TARGET_SCHEMA_VERSION {
                Ok(SchemaAction::AlreadyTarget)
            } else if v > TARGET_SCHEMA_VERSION {
                Err(MigrationError::UnsupportedFutureVersion(format!(
                    "workflow.json declares schema_version: {v}, this migrator targets {TARGET_SCHEMA_VERSION}"
                )))
            } else {
                // v < TARGET would be a downgrade scenario we don't
                // know how to perform; treat as malformed for the beta
                // (no v0 schema ever existed in the wild — schema_version
                // was added in W4 with value 1).
                Err(MigrationError::Malformed(format!(
                    "schema_version: {v} below target {TARGET_SCHEMA_VERSION}; \
                     no upgrade path defined"
                )))
            }
        }
        Some(other) => Err(MigrationError::Malformed(format!(
            "schema_version must be a number, got {}",
            other
        ))),
    }
}

/// Insert `schema_version: target` as the FIRST key of the resulting
/// object. `serde_json::Map` (with the `preserve_order` feature off,
/// the default in this workspace) is a `BTreeMap`, so insertion order
/// is not preserved on serialization — fields land in lexicographic
/// order regardless. We document this in `docs/guides/migration.md`.
fn insert_schema_version(mut obj: Map<String, Value>, target: u64) -> Map<String, Value> {
    obj.insert(
        "schema_version".to_string(),
        Value::Number(serde_json::Number::from(target)),
    );
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_json(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn workflow_json_migrator_adds_missing_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_json(
            tmp.path(),
            "workflow.json",
            r#"{"name":"legacy","version":"1.0.0","steps":{},"edges":[]}"#,
        );

        let report = migrate_workflow_json(&p, false, true).unwrap();
        assert!(!report.no_op);
        assert_eq!(report.written_path.as_ref().unwrap(), &p);

        let raw = std::fs::read_to_string(&p).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            value.get("schema_version").and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(value.get("name").and_then(|v| v.as_str()), Some("legacy"));
    }

    #[test]
    fn workflow_json_migrator_rejects_future_major() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_json(
            tmp.path(),
            "workflow.json",
            r#"{"schema_version":2,"name":"future"}"#,
        );

        let err = migrate_workflow_json(&p, false, true).unwrap_err();
        match err {
            MigrationError::UnsupportedFutureVersion(_) => {}
            other => panic!("expected UnsupportedFutureVersion, got {other}"),
        }
    }

    #[test]
    fn workflow_json_migrator_no_op_on_already_versioned() {
        let tmp = tempfile::tempdir().unwrap();
        let original = r#"{"name":"current","schema_version":1,"steps":{}}"#;
        let p = write_json(tmp.path(), "workflow.json", original);

        let report = migrate_workflow_json(&p, false, true).unwrap();
        assert!(report.no_op);
        assert!(report.written_path.is_none());

        // File must be byte-identical (no-op should not rewrite).
        let after = std::fs::read_to_string(&p).unwrap();
        assert_eq!(after, original);
    }

    #[test]
    fn workflow_json_migrator_dry_run_does_not_write() {
        let tmp = tempfile::tempdir().unwrap();
        let original = r#"{"name":"legacy","steps":{}}"#;
        let p = write_json(tmp.path(), "workflow.json", original);

        let report = migrate_workflow_json(&p, true, true).unwrap();
        assert!(!report.no_op);
        assert!(report.dry_run);

        let after = std::fs::read_to_string(&p).unwrap();
        assert_eq!(after, original, "dry-run must not modify the file");
    }

    #[test]
    fn workflow_json_migrator_writes_sibling_when_not_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let original = r#"{"name":"legacy","steps":{}}"#;
        let p = write_json(tmp.path(), "workflow.json", original);

        let report = migrate_workflow_json(&p, false, false).unwrap();
        assert!(!report.no_op);
        let target = report.written_path.as_ref().unwrap();
        assert_eq!(
            target.file_name().unwrap().to_string_lossy(),
            "workflow.json.migrated"
        );
        assert!(target.exists());
        // Original untouched.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), original);
    }

    #[test]
    fn workflow_json_migrator_rejects_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_json(tmp.path(), "workflow.json", "{not valid json");
        let err = migrate_workflow_json(&p, false, true).unwrap_err();
        match err {
            MigrationError::Malformed(_) => {}
            other => panic!("expected Malformed, got {other}"),
        }
    }

    #[test]
    fn workflow_json_migrator_rejects_non_object_root() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_json(tmp.path(), "workflow.json", "[1,2,3]");
        let err = migrate_workflow_json(&p, false, true).unwrap_err();
        match err {
            MigrationError::Malformed(_) => {}
            other => panic!("expected Malformed, got {other}"),
        }
    }
}
