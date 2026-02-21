use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::spec::{Lockfile, PackageManifest, ResolvedPackage};
use crate::storage::Registry;

/// Resolution result: the set of all packages needed.
#[derive(Debug)]
pub struct ResolutionResult {
    /// Package ID → manifest
    pub packages: BTreeMap<String, PackageManifest>,
    /// Topological order (leaves first)
    pub install_order: Vec<String>,
}

/// Resolve all dependencies starting from a root manifest.
/// Uses exact version matching only. No ranges.
pub fn resolve(
    root: &PackageManifest,
    registry: &Registry,
) -> Result<ResolutionResult, String> {
    let mut resolved: BTreeMap<String, PackageManifest> = BTreeMap::new();
    let mut queue: VecDeque<(String, String)> = VecDeque::new();

    // Seed with root's dependencies
    for (name, version) in &root.dependencies {
        queue.push_back((name.clone(), version.clone()));
    }

    while let Some((name, version)) = queue.pop_front() {
        let id = format!("{name}@{version}");

        // Already resolved
        if resolved.contains_key(&id) {
            continue;
        }

        // Load from registry
        let manifest = registry.load_manifest(&name, &version)
            .map_err(|e| format!("failed to load {id}: {e}"))?;

        // Validate
        manifest.validate()
            .map_err(|errs| format!("invalid package {id}: {}", errs.join("; ")))?;

        // Check for version conflicts: same name, different version
        for existing_id in resolved.keys() {
            if existing_id.starts_with(&format!("{name}@")) && !existing_id.ends_with(&format!("@{version}")) {
                return Err(format!(
                    "version conflict: both '{existing_id}' and '{id}' required"
                ));
            }
        }

        // Queue transitive dependencies
        for (dep_name, dep_ver) in &manifest.dependencies {
            let dep_id = format!("{dep_name}@{dep_ver}");
            if !resolved.contains_key(&dep_id) {
                queue.push_back((dep_name.clone(), dep_ver.clone()));
            }
        }

        resolved.insert(id, manifest);
    }

    // Check for circular dependencies via topological sort
    let install_order = topological_sort(&resolved)?;

    Ok(ResolutionResult {
        packages: resolved,
        install_order,
    })
}

/// Generate a lockfile from resolution result.
pub fn generate_lockfile(
    result: &ResolutionResult,
    registry: &Registry,
) -> Result<Lockfile, String> {
    let mut lockfile = Lockfile::new();

    // Compute hashes bottom-up (install order is leaves first)
    let mut hash_cache: BTreeMap<String, String> = BTreeMap::new();

    for pkg_id in &result.install_order {
        let manifest = &result.packages[pkg_id];
        let pkg_dir = registry.package_dir(&manifest.name, &manifest.version);

        // Collect dependency hashes
        let mut dep_hashes = BTreeMap::new();
        for (dep_name, dep_ver) in &manifest.dependencies {
            let dep_id = format!("{dep_name}@{dep_ver}");
            if let Some(hash) = hash_cache.get(&dep_id) {
                dep_hashes.insert(dep_name.clone(), hash.clone());
            }
        }

        let hash = crate::spec::compute_content_hash(&pkg_dir, &dep_hashes)?;
        hash_cache.insert(pkg_id.clone(), hash.clone());

        lockfile.resolved.insert(pkg_id.clone(), ResolvedPackage {
            integrity: hash,
            dependencies: manifest.dependencies.clone(),
        });
    }

    Ok(lockfile)
}

/// Aggregate all required capabilities from resolved packages.
pub fn aggregate_capabilities(result: &ResolutionResult) -> Vec<String> {
    let mut caps: BTreeSet<String> = BTreeSet::new();
    for manifest in result.packages.values() {
        for cap in &manifest.required_capabilities {
            caps.insert(cap.clone());
        }
    }
    caps.into_iter().collect()
}

/// Topological sort of resolved packages. Returns leaves first.
fn topological_sort(packages: &BTreeMap<String, PackageManifest>) -> Result<Vec<String>, String> {
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    // Initialize
    for (id, manifest) in packages {
        in_degree.entry(id.clone()).or_insert(0);
        for (dep_name, dep_ver) in &manifest.dependencies {
            let dep_id = format!("{dep_name}@{dep_ver}");
            *in_degree.entry(id.clone()).or_insert(0) += 1;
            dependents.entry(dep_id).or_default().push(id.clone());
        }
    }

    let mut queue: VecDeque<String> = VecDeque::new();
    for (id, &deg) in &in_degree {
        if deg == 0 {
            queue.push_back(id.clone());
        }
    }

    // Sort the initial queue for determinism
    let mut sorted_queue: Vec<_> = queue.drain(..).collect();
    sorted_queue.sort();
    queue.extend(sorted_queue);

    let mut order = Vec::new();
    while let Some(id) = queue.pop_front() {
        order.push(id.clone());
        if let Some(deps) = dependents.get(&id) {
            let mut next = Vec::new();
            for dep in deps {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg -= 1;
                    if *deg == 0 {
                        next.push(dep.clone());
                    }
                }
            }
            next.sort();
            queue.extend(next);
        }
    }

    if order.len() != packages.len() {
        return Err("circular dependency detected".into());
    }

    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::PackageManifest;
    use crate::storage::Registry;

    fn setup_registry() -> (tempfile::TempDir, Registry) {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path()).unwrap();
        (dir, reg)
    }

    fn make_manifest(name: &str, version: &str, deps: &[(&str, &str)], caps: &[&str]) -> PackageManifest {
        let mut dependencies = BTreeMap::new();
        for (n, v) in deps {
            dependencies.insert(n.to_string(), v.to_string());
        }
        PackageManifest {
            name: name.into(),
            version: version.into(),
            description: format!("Package {name}"),
            dependencies,
            required_capabilities: caps.iter().map(|s| s.to_string()).collect(),
            exposed_modules: vec!["core".into()],
            integrity: None,
        }
    }

    fn publish_package(reg: &Registry, manifest: &PackageManifest) {
        let pkg_dir = reg.package_dir(&manifest.name, &manifest.version);
        std::fs::create_dir_all(pkg_dir.join("src")).unwrap();
        std::fs::write(
            pkg_dir.join("src/core.ax"),
            format!("// {} v{}", manifest.name, manifest.version),
        ).unwrap();
        manifest.save(&pkg_dir.join("package.ax.json")).unwrap();
        let hash = crate::spec::compute_content_hash(&pkg_dir, &BTreeMap::new()).unwrap();
        std::fs::write(pkg_dir.join("HASH"), &hash).unwrap();
    }

    #[test]
    fn test_resolve_no_deps() {
        let (_dir, reg) = setup_registry();
        let root = make_manifest("app", "1.0.0", &[], &[]);
        let result = resolve(&root, &reg).unwrap();
        assert!(result.packages.is_empty());
        assert!(result.install_order.is_empty());
    }

    #[test]
    fn test_resolve_single_dep() {
        let (_dir, reg) = setup_registry();
        let dep = make_manifest("lib.utils", "0.1.0", &[], &["net.fetch"]);
        publish_package(&reg, &dep);

        let root = make_manifest("app", "1.0.0", &[("lib.utils", "0.1.0")], &[]);
        let result = resolve(&root, &reg).unwrap();
        assert_eq!(result.packages.len(), 1);
        assert!(result.packages.contains_key("lib.utils@0.1.0"));
    }

    #[test]
    fn test_resolve_transitive() {
        let (_dir, reg) = setup_registry();
        let c = make_manifest("pkg.c", "0.1.0", &[], &[]);
        publish_package(&reg, &c);
        let b = make_manifest("pkg.b", "0.2.0", &[("pkg.c", "0.1.0")], &[]);
        publish_package(&reg, &b);

        let root = make_manifest("app", "1.0.0", &[("pkg.b", "0.2.0")], &[]);
        let result = resolve(&root, &reg).unwrap();
        assert_eq!(result.packages.len(), 2);
        // C should come before B in install order
        let pos_c = result.install_order.iter().position(|x| x == "pkg.c@0.1.0").unwrap();
        let pos_b = result.install_order.iter().position(|x| x == "pkg.b@0.2.0").unwrap();
        assert!(pos_c < pos_b);
    }

    #[test]
    fn test_resolve_version_conflict() {
        let (_dir, reg) = setup_registry();
        let c1 = make_manifest("pkg.c", "0.1.0", &[], &[]);
        publish_package(&reg, &c1);
        let c2 = make_manifest("pkg.c", "0.2.0", &[], &[]);
        publish_package(&reg, &c2);
        let b = make_manifest("pkg.b", "0.1.0", &[("pkg.c", "0.2.0")], &[]);
        publish_package(&reg, &b);

        // Root wants c@0.1.0 but b wants c@0.2.0
        let root = make_manifest("app", "1.0.0", &[("pkg.c", "0.1.0"), ("pkg.b", "0.1.0")], &[]);
        let err = resolve(&root, &reg).unwrap_err();
        assert!(err.contains("version conflict"));
    }

    #[test]
    fn test_resolve_missing_package() {
        let (_dir, reg) = setup_registry();
        let root = make_manifest("app", "1.0.0", &[("nonexistent", "0.1.0")], &[]);
        assert!(resolve(&root, &reg).is_err());
    }

    #[test]
    fn test_resolve_circular() {
        let (_dir, reg) = setup_registry();
        // a depends on b, b depends on a
        let a = make_manifest("pkg.a", "0.1.0", &[("pkg.b", "0.1.0")], &[]);
        publish_package(&reg, &a);
        let b = make_manifest("pkg.b", "0.1.0", &[("pkg.a", "0.1.0")], &[]);
        publish_package(&reg, &b);

        let root = make_manifest("app", "1.0.0", &[("pkg.a", "0.1.0")], &[]);
        let err = resolve(&root, &reg).unwrap_err();
        assert!(err.contains("circular"));
    }

    #[test]
    fn test_generate_lockfile() {
        let (_dir, reg) = setup_registry();
        let dep = make_manifest("lib.utils", "0.1.0", &[], &[]);
        publish_package(&reg, &dep);

        let root = make_manifest("app", "1.0.0", &[("lib.utils", "0.1.0")], &[]);
        let result = resolve(&root, &reg).unwrap();
        let lockfile = generate_lockfile(&result, &reg).unwrap();

        assert_eq!(lockfile.resolved.len(), 1);
        let entry = &lockfile.resolved["lib.utils@0.1.0"];
        assert!(entry.integrity.starts_with("sha256:"));
    }

    #[test]
    fn test_lockfile_deterministic() {
        let (_dir, reg) = setup_registry();
        let dep = make_manifest("lib.utils", "0.1.0", &[], &[]);
        publish_package(&reg, &dep);

        let root = make_manifest("app", "1.0.0", &[("lib.utils", "0.1.0")], &[]);
        let r1 = resolve(&root, &reg).unwrap();
        let l1 = generate_lockfile(&r1, &reg).unwrap();

        let r2 = resolve(&root, &reg).unwrap();
        let l2 = generate_lockfile(&r2, &reg).unwrap();

        let j1 = serde_json::to_string(&l1).unwrap();
        let j2 = serde_json::to_string(&l2).unwrap();
        assert_eq!(j1, j2);
    }

    #[test]
    fn test_aggregate_capabilities() {
        let (_dir, reg) = setup_registry();
        let a = make_manifest("pkg.a", "0.1.0", &[], &["net.fetch"]);
        publish_package(&reg, &a);
        let b = make_manifest("pkg.b", "0.1.0", &[], &["db.query", "net.fetch"]);
        publish_package(&reg, &b);

        let root = make_manifest("app", "1.0.0", &[("pkg.a", "0.1.0"), ("pkg.b", "0.1.0")], &[]);
        let result = resolve(&root, &reg).unwrap();
        let caps = aggregate_capabilities(&result);
        assert_eq!(caps, vec!["db.query", "net.fetch"]);
    }
}
