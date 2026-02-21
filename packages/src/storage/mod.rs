use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::spec::{compute_content_hash, PackageManifest};

/// Local package registry backed by filesystem.
pub struct Registry {
    base_dir: PathBuf,
}

impl Registry {
    pub fn new(base_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(base_dir).map_err(|e| format!("create registry dir: {e}"))?;
        Ok(Registry {
            base_dir: base_dir.to_path_buf(),
        })
    }

    /// Path to a specific package version directory.
    pub fn package_dir(&self, name: &str, version: &str) -> PathBuf {
        self.base_dir.join(name).join(version)
    }

    /// Check if a package version exists in the registry.
    pub fn exists(&self, name: &str, version: &str) -> bool {
        let dir = self.package_dir(name, version);
        dir.join("package.ax.json").exists()
    }

    /// Load a package manifest from the registry.
    pub fn load_manifest(&self, name: &str, version: &str) -> Result<PackageManifest, String> {
        let dir = self.package_dir(name, version);
        let path = dir.join("package.ax.json");
        if !path.exists() {
            return Err(format!("package {name}@{version} not found in registry"));
        }
        PackageManifest::load(&path)
    }

    /// List all versions of a package.
    pub fn list_versions(&self, name: &str) -> Result<Vec<String>, String> {
        let pkg_dir = self.base_dir.join(name);
        if !pkg_dir.exists() {
            return Ok(vec![]);
        }
        let mut versions = Vec::new();
        let entries =
            std::fs::read_dir(&pkg_dir).map_err(|e| format!("list versions for {name}: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read entry: {e}"))?;
            if entry.path().is_dir() {
                versions.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        versions.sort();
        Ok(versions)
    }

    /// List all packages in the registry.
    pub fn list_packages(&self) -> Result<Vec<String>, String> {
        let mut packages = Vec::new();
        collect_packages(&self.base_dir, "", &mut packages)?;
        packages.sort();
        Ok(packages)
    }

    /// Publish a package from a source directory to the registry.
    /// Source directory must contain package.ax.json and src/.
    pub fn publish(&self, source_dir: &Path) -> Result<String, String> {
        let manifest_path = source_dir.join("package.ax.json");
        let mut manifest = PackageManifest::load(&manifest_path)?;
        manifest.validate().map_err(|errs| errs.join("; "))?;

        let target_dir = self.package_dir(&manifest.name, &manifest.version);
        if target_dir.exists() {
            return Err(format!(
                "package {}@{} already exists in registry",
                manifest.name, manifest.version
            ));
        }

        // Create target structure
        std::fs::create_dir_all(target_dir.join("src"))
            .map_err(|e| format!("create package dir: {e}"))?;
        std::fs::create_dir_all(target_dir.join("bytecode"))
            .map_err(|e| format!("create bytecode dir: {e}"))?;

        // Copy source files
        let src_dir = source_dir.join("src");
        if src_dir.exists() {
            copy_dir_recursive(&src_dir, &target_dir.join("src"))?;
        }

        // Save manifest without integrity first (for hashing)
        manifest.save(&target_dir.join("package.ax.json"))?;

        // Compile exposed modules
        compile_modules(&manifest, &target_dir)?;

        // Compute content hash
        let dep_hashes = collect_dep_hashes(&manifest, self)?;
        let hash = compute_content_hash(&target_dir, &dep_hashes)?;

        // Write hash
        std::fs::write(target_dir.join("HASH"), &hash).map_err(|e| format!("write HASH: {e}"))?;

        // Update manifest with integrity and save
        manifest.integrity = Some(hash.clone());
        manifest.save(&target_dir.join("package.ax.json"))?;

        Ok(hash)
    }

    /// Verify all packages in the registry match their hashes.
    pub fn verify_all(&self) -> Result<Vec<String>, Vec<String>> {
        let mut verified = Vec::new();
        let mut failures = Vec::new();

        let packages = self.list_packages().map_err(|e| vec![e])?;
        for pkg_name in &packages {
            let versions = self.list_versions(pkg_name).map_err(|e| vec![e])?;
            for version in &versions {
                let pkg_dir = self.package_dir(pkg_name, version);
                let id = format!("{pkg_name}@{version}");

                let manifest = match self.load_manifest(pkg_name, version) {
                    Ok(m) => m,
                    Err(e) => {
                        failures.push(format!("{id}: {e}"));
                        continue;
                    }
                };

                let dep_hashes = match collect_dep_hashes(&manifest, self) {
                    Ok(h) => h,
                    Err(e) => {
                        failures.push(format!("{id}: {e}"));
                        continue;
                    }
                };

                match crate::spec::verify_hash(&pkg_dir, &dep_hashes) {
                    Ok(true) => verified.push(id),
                    Ok(false) => failures.push(format!("{id}: hash mismatch")),
                    Err(e) => failures.push(format!("{id}: {e}")),
                }
            }
        }

        if failures.is_empty() {
            Ok(verified)
        } else {
            Err(failures)
        }
    }
}

/// Collect dependency hashes from the registry.
fn collect_dep_hashes(
    manifest: &PackageManifest,
    registry: &Registry,
) -> Result<BTreeMap<String, String>, String> {
    let mut dep_hashes = BTreeMap::new();
    for (dep_name, dep_ver) in &manifest.dependencies {
        let dep_dir = registry.package_dir(dep_name, dep_ver);
        let hash_file = dep_dir.join("HASH");
        if hash_file.exists() {
            let hash =
                std::fs::read_to_string(&hash_file).map_err(|e| format!("read dep hash: {e}"))?;
            dep_hashes.insert(dep_name.clone(), hash.trim().to_string());
        }
    }
    Ok(dep_hashes)
}

/// Compile exposed modules using boruna_compiler.
fn compile_modules(manifest: &PackageManifest, pkg_dir: &Path) -> Result<(), String> {
    for module_name in &manifest.exposed_modules {
        let src_path = pkg_dir.join("src").join(format!("{module_name}.ax"));
        if !src_path.exists() {
            return Err(format!("source file not found: src/{module_name}.ax"));
        }

        let source = std::fs::read_to_string(&src_path)
            .map_err(|e| format!("read source {module_name}.ax: {e}"))?;

        let module = boruna_compiler::compile(module_name, &source)
            .map_err(|e| format!("compile {module_name}: {e}"))?;

        let bc = module
            .to_json()
            .map_err(|e| format!("serialize {module_name}: {e}"))?;

        let bc_path = pkg_dir.join("bytecode").join(format!("{module_name}.axbc"));
        std::fs::write(&bc_path, bc).map_err(|e| format!("write bytecode {module_name}: {e}"))?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("create dir {}: {e}", dst.display()))?;

    let entries = std::fs::read_dir(src).map_err(|e| format!("read dir {}: {e}", src.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {}: {e}", src_path.display()))?;
        }
    }
    Ok(())
}

/// Collect dotted package names by scanning directory structure.
fn collect_packages(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read dir: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();

        // If this dir contains a version subdir with package.ax.json, it's a package
        let full_name = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };

        // Check if any subdirectory contains package.ax.json (version dir)
        let mut has_versions = false;
        if let Ok(sub_entries) = std::fs::read_dir(&path) {
            for sub in sub_entries.flatten() {
                if sub.path().is_dir() && sub.path().join("package.ax.json").exists() {
                    has_versions = true;
                    break;
                }
            }
        }

        if has_versions {
            out.push(full_name);
        } else {
            // Could be a namespace prefix, recurse
            collect_packages(&path, &full_name, out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::PackageManifest;
    use std::collections::BTreeMap;

    fn make_manifest(name: &str, version: &str) -> PackageManifest {
        PackageManifest {
            name: name.into(),
            version: version.into(),
            description: format!("Package {name}"),
            dependencies: BTreeMap::new(),
            required_capabilities: vec![],
            exposed_modules: vec!["core".into()],
            integrity: None,
        }
    }

    #[test]
    fn test_registry_publish_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path()).unwrap();

        // Create source package
        let src_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src_dir.path().join("src")).unwrap();
        std::fs::write(
            src_dir.path().join("src/core.ax"),
            "fn main() -> Int { 42 }\n",
        )
        .unwrap();
        let manifest = make_manifest("test.pkg", "0.1.0");
        manifest
            .save(&src_dir.path().join("package.ax.json"))
            .unwrap();

        let hash = reg.publish(src_dir.path()).unwrap();
        assert!(hash.starts_with("sha256:"));

        // Load back
        let loaded = reg.load_manifest("test.pkg", "0.1.0").unwrap();
        assert_eq!(loaded.name, "test.pkg");
        assert_eq!(loaded.integrity, Some(hash));
    }

    #[test]
    fn test_registry_exists() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path()).unwrap();
        assert!(!reg.exists("test.pkg", "0.1.0"));
    }

    #[test]
    fn test_registry_list_versions() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path()).unwrap();

        // Publish two versions
        for ver in &["0.1.0", "0.2.0"] {
            let src_dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(src_dir.path().join("src")).unwrap();
            std::fs::write(
                src_dir.path().join("src/core.ax"),
                "fn main() -> Int { 42 }\n",
            )
            .unwrap();
            let manifest = PackageManifest {
                name: "test.pkg".into(),
                version: ver.to_string(),
                description: "test".into(),
                dependencies: BTreeMap::new(),
                required_capabilities: vec![],
                exposed_modules: vec!["core".into()],
                integrity: None,
            };
            manifest
                .save(&src_dir.path().join("package.ax.json"))
                .unwrap();
            reg.publish(src_dir.path()).unwrap();
        }

        let versions = reg.list_versions("test.pkg").unwrap();
        assert_eq!(versions, vec!["0.1.0", "0.2.0"]);
    }

    #[test]
    fn test_registry_duplicate_publish_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path()).unwrap();

        let src_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src_dir.path().join("src")).unwrap();
        std::fs::write(
            src_dir.path().join("src/core.ax"),
            "fn main() -> Int { 42 }\n",
        )
        .unwrap();
        let manifest = make_manifest("test.pkg", "0.1.0");
        manifest
            .save(&src_dir.path().join("package.ax.json"))
            .unwrap();

        reg.publish(src_dir.path()).unwrap();
        let err = reg.publish(src_dir.path()).unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn test_registry_verify() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path()).unwrap();

        let src_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src_dir.path().join("src")).unwrap();
        std::fs::write(
            src_dir.path().join("src/core.ax"),
            "fn main() -> Int { 42 }\n",
        )
        .unwrap();
        let manifest = make_manifest("test.pkg", "0.1.0");
        manifest
            .save(&src_dir.path().join("package.ax.json"))
            .unwrap();

        reg.publish(src_dir.path()).unwrap();
        let result = reg.verify_all();
        assert!(result.is_ok());
    }
}
