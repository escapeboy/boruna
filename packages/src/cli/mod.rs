use std::collections::BTreeMap;
use std::path::Path;

use crate::resolver;
use crate::spec::{CapabilityPolicy, PackageManifest};
use crate::storage::Registry;

/// Initialize a new package manifest in the given directory.
pub fn cmd_init(dir: &Path) -> Result<(), String> {
    let manifest_path = dir.join("package.ax.json");
    if manifest_path.exists() {
        return Err("package.ax.json already exists".into());
    }

    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "my.package".into())
        .replace(['-', '_'], ".")
        .to_lowercase();

    let manifest = PackageManifest {
        name,
        version: "0.1.0".into(),
        description: "A new package".into(),
        dependencies: BTreeMap::new(),
        required_capabilities: vec![],
        exposed_modules: vec!["core".into()],
        integrity: None,
    };

    manifest.save(&manifest_path)?;

    // Create src directory
    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir).map_err(|e| format!("create src dir: {e}"))?;

    println!("initialized package: {}", manifest.name);
    Ok(())
}

/// Add a dependency to the manifest.
pub fn cmd_add(dir: &Path, name: &str, version: &str) -> Result<(), String> {
    let manifest_path = dir.join("package.ax.json");
    let mut manifest = PackageManifest::load(&manifest_path)?;
    manifest.dependencies.insert(name.into(), version.into());
    manifest.save(&manifest_path)?;
    println!("added dependency: {name}@{version}");
    Ok(())
}

/// Remove a dependency from the manifest.
pub fn cmd_remove(dir: &Path, name: &str) -> Result<(), String> {
    let manifest_path = dir.join("package.ax.json");
    let mut manifest = PackageManifest::load(&manifest_path)?;
    if manifest.dependencies.remove(name).is_none() {
        return Err(format!("dependency '{name}' not found"));
    }
    manifest.save(&manifest_path)?;
    println!("removed dependency: {name}");
    Ok(())
}

/// Resolve dependencies and generate lockfile.
pub fn cmd_resolve(dir: &Path, registry_path: &Path) -> Result<(), String> {
    let manifest = PackageManifest::load(&dir.join("package.ax.json"))?;
    manifest.validate().map_err(|errs| errs.join("; "))?;

    let registry = Registry::new(registry_path)?;
    let result = resolver::resolve(&manifest, &registry)?;
    let lockfile = resolver::generate_lockfile(&result, &registry)?;

    lockfile.save(&dir.join("llm.lock.json"))?;

    println!("resolved {} packages:", result.packages.len());
    for id in &result.install_order {
        println!("  {id}");
    }
    Ok(())
}

/// Resolve and verify all packages exist.
pub fn cmd_install(dir: &Path, registry_path: &Path) -> Result<(), String> {
    let manifest = PackageManifest::load(&dir.join("package.ax.json"))?;
    manifest.validate().map_err(|errs| errs.join("; "))?;

    let registry = Registry::new(registry_path)?;
    let result = resolver::resolve(&manifest, &registry)?;

    // Verify all packages
    for (id, pkg) in &result.packages {
        let pkg_dir = registry.package_dir(&pkg.name, &pkg.version);
        if !pkg_dir.exists() {
            return Err(format!("package {id} not found in registry"));
        }
    }

    // Generate lockfile
    let lockfile = resolver::generate_lockfile(&result, &registry)?;
    lockfile.save(&dir.join("llm.lock.json"))?;

    // Check capability policy
    let policy_path = dir.join("policy.ax.json");
    if policy_path.exists() {
        let data =
            std::fs::read_to_string(&policy_path).map_err(|e| format!("read policy: {e}"))?;
        let policy: CapabilityPolicy =
            serde_json::from_str(&data).map_err(|e| format!("parse policy: {e}"))?;
        policy.validate()?;

        let caps = resolver::aggregate_capabilities(&result);
        if let Err(violations) = policy.check_capabilities(&caps) {
            return Err(format!(
                "capability policy violation: dependencies require forbidden capabilities: {}",
                violations.join(", ")
            ));
        }
    }

    println!("installed {} packages", result.packages.len());
    Ok(())
}

/// Publish package to local registry.
pub fn cmd_publish(dir: &Path, registry_path: &Path) -> Result<(), String> {
    let registry = Registry::new(registry_path)?;
    let hash = registry.publish(dir)?;

    let manifest = PackageManifest::load(&dir.join("package.ax.json"))?;
    println!("published {}@{}", manifest.name, manifest.version);
    println!("  integrity: {hash}");
    Ok(())
}

/// Verify all packages in registry match their hashes.
pub fn cmd_verify(registry_path: &Path) -> Result<(), String> {
    let registry = Registry::new(registry_path)?;
    match registry.verify_all() {
        Ok(verified) => {
            println!("verified {} packages:", verified.len());
            for id in &verified {
                println!("  {id} OK");
            }
            Ok(())
        }
        Err(failures) => {
            for f in &failures {
                eprintln!("  FAIL: {f}");
            }
            Err(format!("{} packages failed verification", failures.len()))
        }
    }
}

/// Print dependency tree.
pub fn cmd_tree(dir: &Path, registry_path: &Path) -> Result<(), String> {
    let manifest = PackageManifest::load(&dir.join("package.ax.json"))?;
    let registry = Registry::new(registry_path)?;

    println!("{}@{}", manifest.name, manifest.version);
    print_tree_deps(&manifest, &registry, "", true)?;
    Ok(())
}

fn print_tree_deps(
    manifest: &PackageManifest,
    registry: &Registry,
    prefix: &str,
    _is_root: bool,
) -> Result<(), String> {
    let deps: Vec<_> = manifest.dependencies.iter().collect();
    for (i, (name, version)) in deps.iter().enumerate() {
        let is_last = i == deps.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let continuation = if is_last { "    " } else { "│   " };

        println!("{prefix}{connector}{name}@{version}");

        if let Ok(dep_manifest) = registry.load_manifest(name, version) {
            let new_prefix = format!("{prefix}{continuation}");
            print_tree_deps(&dep_manifest, registry, &new_prefix, false)?;
        }
    }
    Ok(())
}
