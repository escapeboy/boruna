use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchBundle {
    pub version: u32,
    pub metadata: PatchMetadata,
    pub patches: Vec<FilePatch>,
    pub expected_checks: ExpectedChecks,
    pub reviewer_checklist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchMetadata {
    pub id: String,
    pub intent: String,
    pub author: String,
    pub timestamp: String,
    pub touched_modules: Vec<String>,
    pub risk_level: RiskLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePatch {
    pub file: String,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    pub start_line: usize,
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedChecks {
    pub compile: bool,
    pub test: bool,
    pub replay: bool,
    pub diagnostics_count: Option<usize>,
}

impl PatchBundle {
    /// Load a patch bundle from a JSON file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let data = fs::read_to_string(path).map_err(|e| format!("failed to read bundle: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("invalid bundle JSON: {e}"))
    }

    /// Save the bundle to a JSON file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize bundle: {e}"))?;
        fs::write(path, json).map_err(|e| format!("failed to write bundle: {e}"))
    }

    /// Validate the bundle format.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.version != 1 {
            errors.push(format!("unsupported version: {}", self.version));
        }
        if self.metadata.id.is_empty() {
            errors.push("metadata.id is empty".into());
        }
        if self.metadata.intent.is_empty() {
            errors.push("metadata.intent is empty".into());
        }
        if self.metadata.author.is_empty() {
            errors.push("metadata.author is empty".into());
        }
        if self.patches.is_empty() {
            errors.push("no patches in bundle".into());
        }
        for (i, patch) in self.patches.iter().enumerate() {
            if patch.file.is_empty() {
                errors.push(format!("patches[{i}].file is empty"));
            }
            // Reject path traversal attempts
            let patch_path = Path::new(&patch.file);
            if patch_path
                .components()
                .any(|c| c == std::path::Component::ParentDir)
            {
                errors.push(format!("patches[{i}].file contains '..': {}", patch.file));
            }
            if patch_path.is_absolute() {
                errors.push(format!(
                    "patches[{i}].file is an absolute path: {}",
                    patch.file
                ));
            }
            if patch.hunks.is_empty() {
                errors.push(format!("patches[{i}].hunks is empty"));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Compute a stable SHA-256 hash of the bundle content (patches only, not metadata).
    pub fn content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        for patch in &self.patches {
            hasher.update(patch.file.as_bytes());
            for hunk in &patch.hunks {
                hasher.update(hunk.start_line.to_le_bytes());
                hasher.update(hunk.old_text.as_bytes());
                hasher.update(hunk.new_text.as_bytes());
            }
        }
        format!("{:x}", hasher.finalize())
    }

    /// Apply the bundle to the filesystem rooted at `base_dir`.
    /// Returns a rollback bundle that can undo the changes.
    pub fn apply(&self, base_dir: &Path) -> Result<PatchBundle, String> {
        let mut rollback_patches = Vec::new();

        for patch in &self.patches {
            let file_path = base_dir.join(&patch.file);
            // Defense-in-depth: verify resolved path stays within base_dir
            let canonical = file_path
                .canonicalize()
                .map_err(|e| format!("cannot resolve {}: {e}", patch.file))?;
            let canonical_base = base_dir
                .canonicalize()
                .map_err(|e| format!("cannot resolve base dir: {e}"))?;
            if !canonical.starts_with(&canonical_base) {
                return Err(format!(
                    "path traversal rejected: {} resolves outside workspace",
                    patch.file
                ));
            }
            let content = fs::read_to_string(&file_path)
                .map_err(|e| format!("cannot read {}: {e}", patch.file))?;
            let lines: Vec<&str> = content.lines().collect();

            let mut new_lines = lines.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            let mut rollback_hunks = Vec::new();

            // Apply hunks in reverse order to preserve line numbers
            let mut sorted_hunks = patch.hunks.clone();
            sorted_hunks.sort_by(|a, b| b.start_line.cmp(&a.start_line));

            for hunk in &sorted_hunks {
                let start = hunk.start_line.saturating_sub(1); // 1-indexed to 0-indexed
                let old_lines: Vec<&str> = hunk.old_text.lines().collect();
                let old_count = old_lines.len();

                // Verify old text matches
                if start + old_count > new_lines.len() {
                    return Err(format!(
                        "hunk at line {} in {} extends past end of file",
                        hunk.start_line, patch.file
                    ));
                }

                let actual: Vec<&str> = new_lines[start..start + old_count]
                    .iter()
                    .map(|s| s.as_str())
                    .collect();
                if actual != old_lines {
                    return Err(format!(
                        "hunk at line {} in {} does not match: expected {:?}, got {:?}",
                        hunk.start_line, patch.file, old_lines, actual
                    ));
                }

                // Build rollback hunk
                rollback_hunks.push(Hunk {
                    start_line: hunk.start_line,
                    old_text: hunk.new_text.clone(),
                    new_text: hunk.old_text.clone(),
                });

                // Replace lines
                let new_hunk_lines: Vec<String> =
                    hunk.new_text.lines().map(|s| s.to_string()).collect();
                new_lines.splice(start..start + old_count, new_hunk_lines);
            }

            // Write modified file
            let new_content = new_lines.join("\n") + "\n";
            fs::write(&file_path, new_content)
                .map_err(|e| format!("cannot write {}: {e}", patch.file))?;

            rollback_patches.push(FilePatch {
                file: patch.file.clone(),
                hunks: rollback_hunks,
            });
        }

        Ok(PatchBundle {
            version: 1,
            metadata: PatchMetadata {
                id: format!("{}-rollback", self.metadata.id),
                intent: format!("Rollback: {}", self.metadata.intent),
                author: "orchestrator".into(),
                timestamp: self.metadata.timestamp.clone(),
                touched_modules: self.metadata.touched_modules.clone(),
                risk_level: self.metadata.risk_level.clone(),
            },
            patches: rollback_patches,
            expected_checks: self.expected_checks.clone(),
            reviewer_checklist: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_bundle() -> PatchBundle {
        PatchBundle {
            version: 1,
            metadata: PatchMetadata {
                id: "PB-001".into(),
                intent: "test patch".into(),
                author: "test".into(),
                timestamp: "2026-02-20T00:00:00Z".into(),
                touched_modules: vec!["mod_a".into()],
                risk_level: RiskLevel::Low,
            },
            patches: vec![FilePatch {
                file: "test.txt".into(),
                hunks: vec![Hunk {
                    start_line: 1,
                    old_text: "hello".into(),
                    new_text: "world".into(),
                }],
            }],
            expected_checks: ExpectedChecks {
                compile: true,
                test: true,
                replay: false,
                diagnostics_count: None,
            },
            reviewer_checklist: vec!["looks good".into()],
        }
    }

    #[test]
    fn test_validate_valid_bundle() {
        let bundle = sample_bundle();
        assert!(bundle.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_id() {
        let mut bundle = sample_bundle();
        bundle.metadata.id = "".into();
        let err = bundle.validate().unwrap_err();
        assert!(err.iter().any(|e| e.contains("id is empty")));
    }

    #[test]
    fn test_validate_no_patches() {
        let mut bundle = sample_bundle();
        bundle.patches = vec![];
        let err = bundle.validate().unwrap_err();
        assert!(err.iter().any(|e| e.contains("no patches")));
    }

    #[test]
    fn test_content_hash_stable() {
        let bundle = sample_bundle();
        let h1 = bundle.content_hash();
        let h2 = bundle.content_hash();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn test_content_hash_changes_with_content() {
        let b1 = sample_bundle();
        let mut b2 = sample_bundle();
        b2.patches[0].hunks[0].new_text = "different".into();
        assert_ne!(b1.content_hash(), b2.content_hash());
    }

    #[test]
    fn test_apply_and_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut f = fs::File::create(&file_path).unwrap();
        writeln!(f, "hello").unwrap();

        let bundle = sample_bundle();
        let rollback = bundle.apply(dir.path()).unwrap();

        // Check file was modified
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content.trim(), "world");

        // Apply rollback
        rollback.apply(dir.path()).unwrap();
        let restored = fs::read_to_string(&file_path).unwrap();
        assert_eq!(restored.trim(), "hello");
    }

    #[test]
    fn test_apply_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "different content\n").unwrap();

        let bundle = sample_bundle();
        let result = bundle.apply(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not match"));
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let bundle_path = dir.path().join("test.patchbundle.json");

        let bundle = sample_bundle();
        bundle.save(&bundle_path).unwrap();

        let loaded = PatchBundle::load(&bundle_path).unwrap();
        assert_eq!(loaded.metadata.id, "PB-001");
        assert_eq!(loaded.patches.len(), 1);
    }

    #[test]
    fn test_validate_rejects_path_traversal() {
        let mut bundle = sample_bundle();
        bundle.patches[0].file = "../../etc/passwd".into();
        let err = bundle.validate().unwrap_err();
        assert!(err
            .iter()
            .any(|e| e.contains("'..'") || e.contains("path traversal")));
    }

    #[test]
    fn test_validate_rejects_absolute_path() {
        let mut bundle = sample_bundle();
        bundle.patches[0].file = "/etc/passwd".into();
        let err = bundle.validate().unwrap_err();
        assert!(err.iter().any(|e| e.contains("absolute")));
    }

    #[test]
    fn test_apply_rejects_path_traversal_at_runtime() {
        let dir = tempfile::tempdir().unwrap();
        // Create a file inside the dir
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello\n").unwrap();

        // Also create the traversal target to ensure canonicalize works
        let parent = dir.path().parent().unwrap();
        let target = parent.join("traversal_target.txt");
        fs::write(&target, "secret\n").unwrap();

        let mut bundle = sample_bundle();
        bundle.patches[0].file = format!("../{}", "traversal_target.txt");

        let result = bundle.apply(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("path traversal rejected") || err.contains("resolves outside"),
            "expected path traversal error, got: {err}"
        );

        // Clean up
        let _ = fs::remove_file(&target);
    }
}
