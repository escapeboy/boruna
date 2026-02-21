use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

// ── Package Manifest ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    pub exposed_modules: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
}

impl PackageManifest {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let data = std::fs::read_to_string(path).map_err(|e| format!("read manifest: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("parse manifest: {e}"))
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let json =
            serde_json::to_string_pretty(self).map_err(|e| format!("serialize manifest: {e}"))?;
        std::fs::write(path, json).map_err(|e| format!("write manifest: {e}"))
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Name: dotted lowercase identifiers
        let name_re = regex_lite(r"^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)*$");
        if !name_re.is_match(&self.name) {
            errors.push(format!(
                "invalid package name '{}': must be dotted lowercase identifiers",
                self.name
            ));
        }

        // Version: semver
        if !is_valid_semver(&self.version) {
            errors.push(format!(
                "invalid version '{}': must be MAJOR.MINOR.PATCH",
                self.version
            ));
        }

        if self.description.is_empty() {
            errors.push("description must not be empty".into());
        }

        // Dependencies: exact versions
        for (dep, ver) in &self.dependencies {
            if !name_re.is_match(dep) {
                errors.push(format!("invalid dependency name '{dep}'"));
            }
            if !is_valid_semver(ver) {
                errors.push(format!(
                    "dependency '{dep}' version '{ver}' is not valid semver"
                ));
            }
        }

        // Capabilities: must be known
        for cap in &self.required_capabilities {
            if boruna_bytecode::Capability::from_name(cap).is_none() {
                errors.push(format!("unknown capability '{cap}'"));
            }
        }

        if self.exposed_modules.is_empty() {
            errors.push("exposed_modules must contain at least one entry".into());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Package identifier: name@version
    pub fn id(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

// ── Lockfile ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    pub lockfile_version: u32,
    pub resolved: BTreeMap<String, ResolvedPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPackage {
    pub integrity: String,
    pub dependencies: BTreeMap<String, String>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

impl Lockfile {
    pub fn new() -> Self {
        Lockfile {
            lockfile_version: 1,
            resolved: BTreeMap::new(),
        }
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let data = std::fs::read_to_string(path).map_err(|e| format!("read lockfile: {e}"))?;
        let lf: Self = serde_json::from_str(&data).map_err(|e| format!("parse lockfile: {e}"))?;
        if lf.lockfile_version != 1 {
            return Err(format!(
                "unsupported lockfile version: {}",
                lf.lockfile_version
            ));
        }
        Ok(lf)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let json =
            serde_json::to_string_pretty(self).map_err(|e| format!("serialize lockfile: {e}"))?;
        std::fs::write(path, json).map_err(|e| format!("write lockfile: {e}"))
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        for (id, resolved) in &self.resolved {
            if resolved.integrity.is_empty() {
                errors.push(format!("package '{id}' has no integrity hash"));
            }
            // Check that all dependencies are also in the lockfile
            for (dep_name, dep_ver) in &resolved.dependencies {
                let dep_id = format!("{dep_name}@{dep_ver}");
                if !self.resolved.contains_key(&dep_id) {
                    errors.push(format!(
                        "package '{id}' depends on '{dep_id}' which is not in lockfile"
                    ));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// ── Capability Policy ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_capabilities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied_capabilities: Option<Vec<String>>,
}

impl CapabilityPolicy {
    pub fn allow_all() -> Self {
        CapabilityPolicy {
            allowed_capabilities: None,
            denied_capabilities: None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.allowed_capabilities.is_some() && self.denied_capabilities.is_some() {
            return Err("cannot specify both allowed_capabilities and denied_capabilities".into());
        }
        Ok(())
    }

    pub fn is_allowed(&self, cap: &str) -> bool {
        if let Some(allowed) = &self.allowed_capabilities {
            return allowed.iter().any(|a| a == cap);
        }
        if let Some(denied) = &self.denied_capabilities {
            return !denied.iter().any(|d| d == cap);
        }
        true
    }

    pub fn check_capabilities(&self, caps: &[String]) -> Result<(), Vec<String>> {
        let violations: Vec<String> = caps
            .iter()
            .filter(|c| !self.is_allowed(c))
            .cloned()
            .collect();
        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

// ── Content Hashing ──

/// Compute content hash for a package directory.
/// Hashes: sorted source files + manifest (without integrity) + dependency hashes.
pub fn compute_content_hash(
    pkg_dir: &std::path::Path,
    dep_hashes: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut hasher = Sha256::new();

    // 1. Source files sorted by path
    let src_dir = pkg_dir.join("src");
    if src_dir.exists() {
        let mut files: Vec<_> = walkdir_sorted(&src_dir)?;
        files.sort();
        for file_path in &files {
            let rel = file_path
                .strip_prefix(pkg_dir)
                .map_err(|e| format!("strip prefix: {e}"))?;
            let content =
                std::fs::read(file_path).map_err(|e| format!("read {}: {e}", rel.display()))?;
            hasher.update(rel.to_string_lossy().as_bytes());
            hasher.update(&content);
        }
    }

    // 2. Manifest without integrity field
    let manifest_path = pkg_dir.join("package.ax.json");
    if manifest_path.exists() {
        let mut manifest = PackageManifest::load(&manifest_path)?;
        manifest.integrity = None;
        let canonical = serde_json::to_string(&manifest)
            .map_err(|e| format!("serialize manifest for hash: {e}"))?;
        hasher.update(b"MANIFEST:");
        hasher.update(canonical.as_bytes());
    }

    // 3. Dependency hashes sorted by name
    for (name, hash) in dep_hashes {
        hasher.update(b"DEP:");
        hasher.update(name.as_bytes());
        hasher.update(b"=");
        hasher.update(hash.as_bytes());
    }

    let result = hasher.finalize();
    Ok(format!("sha256:{:x}", result))
}

/// Verify a package's hash matches its HASH file.
pub fn verify_hash(
    pkg_dir: &std::path::Path,
    dep_hashes: &BTreeMap<String, String>,
) -> Result<bool, String> {
    let hash_file = pkg_dir.join("HASH");
    if !hash_file.exists() {
        return Err("HASH file not found".into());
    }
    let expected = std::fs::read_to_string(&hash_file).map_err(|e| format!("read HASH: {e}"))?;
    let actual = compute_content_hash(pkg_dir, dep_hashes)?;
    Ok(expected.trim() == actual)
}

// ── Helpers ──

fn is_valid_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| p.parse::<u32>().is_ok())
}

/// Simple regex matching without pulling in regex crate.
struct SimpleRegex {
    pattern: String,
}

fn regex_lite(pattern: &str) -> SimpleRegex {
    SimpleRegex {
        pattern: pattern.to_string(),
    }
}

impl SimpleRegex {
    fn is_match(&self, s: &str) -> bool {
        // We only need one pattern: ^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)*$
        if self.pattern == r"^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)*$" {
            return is_valid_package_name(s);
        }
        false
    }
}

fn is_valid_package_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    for (i, part) in s.split('.').enumerate() {
        if part.is_empty() {
            return false;
        }
        let mut chars = part.chars();
        match chars.next() {
            Some(c) if c.is_ascii_lowercase() => {}
            _ => return false,
        }
        for c in chars {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() {
                return false;
            }
        }
        // Ensure the original string has dots in the right places
        if i > 0 {
            // verified by split
        }
    }
    true
}

fn walkdir_sorted(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut files = Vec::new();
    collect_files(dir, &mut files)?;
    Ok(files)
}

fn collect_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read dir {}: {e}", dir.display()))?;
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_package_name() {
        assert!(is_valid_package_name("std"));
        assert!(is_valid_package_name("std.collections"));
        assert!(is_valid_package_name("my.pkg2"));
        assert!(!is_valid_package_name(""));
        assert!(!is_valid_package_name("Std"));
        assert!(!is_valid_package_name("std."));
        assert!(!is_valid_package_name(".std"));
        assert!(!is_valid_package_name("std..collections"));
        assert!(!is_valid_package_name("2std"));
    }

    #[test]
    fn test_valid_semver() {
        assert!(is_valid_semver("0.1.0"));
        assert!(is_valid_semver("1.0.0"));
        assert!(is_valid_semver("10.20.30"));
        assert!(!is_valid_semver("1.0"));
        assert!(!is_valid_semver("1.0.0.0"));
        assert!(!is_valid_semver("a.b.c"));
        assert!(!is_valid_semver(""));
    }

    #[test]
    fn test_manifest_validate_valid() {
        let m = sample_manifest();
        assert!(m.validate().is_ok());
    }

    #[test]
    fn test_manifest_validate_bad_name() {
        let mut m = sample_manifest();
        m.name = "BAD-name".into();
        let errs = m.validate().unwrap_err();
        assert!(errs[0].contains("invalid package name"));
    }

    #[test]
    fn test_manifest_validate_bad_version() {
        let mut m = sample_manifest();
        m.version = "1.0".into();
        let errs = m.validate().unwrap_err();
        assert!(errs[0].contains("invalid version"));
    }

    #[test]
    fn test_manifest_validate_unknown_capability() {
        let mut m = sample_manifest();
        m.required_capabilities = vec!["quantum.compute".into()];
        let errs = m.validate().unwrap_err();
        assert!(errs[0].contains("unknown capability"));
    }

    #[test]
    fn test_manifest_validate_empty_modules() {
        let mut m = sample_manifest();
        m.exposed_modules = vec![];
        let errs = m.validate().unwrap_err();
        assert!(errs[0].contains("exposed_modules"));
    }

    #[test]
    fn test_lockfile_validate() {
        let mut lf = Lockfile::new();
        lf.resolved.insert(
            "a@0.1.0".into(),
            ResolvedPackage {
                integrity: "sha256:abc".into(),
                dependencies: BTreeMap::new(),
            },
        );
        assert!(lf.validate().is_ok());
    }

    #[test]
    fn test_lockfile_validate_missing_dep() {
        let mut lf = Lockfile::new();
        let mut deps = BTreeMap::new();
        deps.insert("b".into(), "0.2.0".into());
        lf.resolved.insert(
            "a@0.1.0".into(),
            ResolvedPackage {
                integrity: "sha256:abc".into(),
                dependencies: deps,
            },
        );
        let errs = lf.validate().unwrap_err();
        assert!(errs[0].contains("b@0.2.0"));
    }

    #[test]
    fn test_policy_allow_all() {
        let p = CapabilityPolicy::allow_all();
        assert!(p.is_allowed("net.fetch"));
        assert!(p.is_allowed("anything"));
    }

    #[test]
    fn test_policy_allowed_list() {
        let p = CapabilityPolicy {
            allowed_capabilities: Some(vec!["net.fetch".into()]),
            denied_capabilities: None,
        };
        assert!(p.is_allowed("net.fetch"));
        assert!(!p.is_allowed("db.query"));
    }

    #[test]
    fn test_policy_denied_list() {
        let p = CapabilityPolicy {
            allowed_capabilities: None,
            denied_capabilities: Some(vec!["fs.write".into()]),
        };
        assert!(p.is_allowed("net.fetch"));
        assert!(!p.is_allowed("fs.write"));
    }

    #[test]
    fn test_policy_validate_both() {
        let p = CapabilityPolicy {
            allowed_capabilities: Some(vec![]),
            denied_capabilities: Some(vec![]),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn test_content_hash_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("core.ax"), "fn main() -> Int { 42 }").unwrap();

        let manifest = sample_manifest();
        manifest.save(&dir.path().join("package.ax.json")).unwrap();

        let deps = BTreeMap::new();
        let h1 = compute_content_hash(dir.path(), &deps).unwrap();
        let h2 = compute_content_hash(dir.path(), &deps).unwrap();
        assert_eq!(h1, h2);
        assert!(h1.starts_with("sha256:"));
    }

    #[test]
    fn test_content_hash_changes_with_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("core.ax"), "fn main() -> Int { 42 }").unwrap();

        let manifest = sample_manifest();
        manifest.save(&dir.path().join("package.ax.json")).unwrap();

        let deps = BTreeMap::new();
        let h1 = compute_content_hash(dir.path(), &deps).unwrap();

        std::fs::write(src.join("core.ax"), "fn main() -> Int { 99 }").unwrap();
        let h2 = compute_content_hash(dir.path(), &deps).unwrap();
        assert_ne!(h1, h2);
    }

    fn sample_manifest() -> PackageManifest {
        PackageManifest {
            name: "test.pkg".into(),
            version: "0.1.0".into(),
            description: "A test package".into(),
            dependencies: BTreeMap::new(),
            required_capabilities: vec!["net.fetch".into()],
            exposed_modules: vec!["core".into()],
            integrity: None,
        }
    }
}
