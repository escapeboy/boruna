use boruna_pkg::resolver::*;
use boruna_pkg::spec::*;
use boruna_pkg::storage::*;
use std::collections::BTreeMap;

// === Helpers ===

fn make_manifest(
    name: &str,
    version: &str,
    deps: &[(&str, &str)],
    caps: &[&str],
) -> PackageManifest {
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

fn setup_registry() -> (tempfile::TempDir, Registry) {
    let dir = tempfile::tempdir().unwrap();
    let reg = Registry::new(dir.path()).unwrap();
    (dir, reg)
}

fn publish_to_registry(
    reg: &Registry,
    name: &str,
    version: &str,
    deps: &[(&str, &str)],
    caps: &[&str],
) {
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("src")).unwrap();
    std::fs::write(
        src.path().join("src/core.ax"),
        &format!("// {name} v{version}\nfn main() -> Int {{ 42 }}\n"),
    )
    .unwrap();
    let manifest = make_manifest(name, version, deps, caps);
    manifest.save(&src.path().join("package.ax.json")).unwrap();
    reg.publish(src.path()).unwrap();
}

// === Resolution Tests ===

#[test]
fn test_resolve_empty_deps() {
    let (_dir, reg) = setup_registry();
    let root = make_manifest("app", "1.0.0", &[], &[]);
    let result = resolve(&root, &reg).unwrap();
    assert!(result.packages.is_empty());
}

#[test]
fn test_resolve_single_dependency() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "lib.utils", "0.1.0", &[], &["net.fetch"]);

    let root = make_manifest("app", "1.0.0", &[("lib.utils", "0.1.0")], &[]);
    let result = resolve(&root, &reg).unwrap();
    assert_eq!(result.packages.len(), 1);
    assert_eq!(result.install_order, vec!["lib.utils@0.1.0"]);
}

#[test]
fn test_resolve_transitive_deps() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "pkg.c", "0.1.0", &[], &[]);
    publish_to_registry(&reg, "pkg.b", "0.2.0", &[("pkg.c", "0.1.0")], &[]);

    let root = make_manifest("app", "1.0.0", &[("pkg.b", "0.2.0")], &[]);
    let result = resolve(&root, &reg).unwrap();
    assert_eq!(result.packages.len(), 2);

    // C must be installed before B
    let pos_c = result
        .install_order
        .iter()
        .position(|x| x == "pkg.c@0.1.0")
        .unwrap();
    let pos_b = result
        .install_order
        .iter()
        .position(|x| x == "pkg.b@0.2.0")
        .unwrap();
    assert!(pos_c < pos_b);
}

#[test]
fn test_resolve_diamond() {
    let (_dir, reg) = setup_registry();
    // D depends on B and C, both depend on A
    publish_to_registry(&reg, "pkg.a", "0.1.0", &[], &[]);
    publish_to_registry(&reg, "pkg.b", "0.1.0", &[("pkg.a", "0.1.0")], &[]);
    publish_to_registry(&reg, "pkg.c", "0.1.0", &[("pkg.a", "0.1.0")], &[]);

    let root = make_manifest(
        "app",
        "1.0.0",
        &[("pkg.b", "0.1.0"), ("pkg.c", "0.1.0")],
        &[],
    );
    let result = resolve(&root, &reg).unwrap();
    assert_eq!(result.packages.len(), 3); // a, b, c (a is shared)
}

#[test]
fn test_resolve_version_conflict_fails() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "pkg.shared", "0.1.0", &[], &[]);
    publish_to_registry(&reg, "pkg.shared", "0.2.0", &[], &[]);
    publish_to_registry(&reg, "pkg.b", "0.1.0", &[("pkg.shared", "0.2.0")], &[]);

    // Root wants shared@0.1.0, but b wants shared@0.2.0
    let root = make_manifest(
        "app",
        "1.0.0",
        &[("pkg.shared", "0.1.0"), ("pkg.b", "0.1.0")],
        &[],
    );
    let err = resolve(&root, &reg).unwrap_err();
    assert!(err.contains("version conflict"));
}

#[test]
fn test_resolve_missing_package_fails() {
    let (_dir, reg) = setup_registry();
    let root = make_manifest("app", "1.0.0", &[("nonexistent", "0.1.0")], &[]);
    assert!(resolve(&root, &reg).is_err());
}

// === Lockfile Determinism ===

#[test]
fn test_lockfile_deterministic() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "pkg.a", "0.1.0", &[], &[]);
    publish_to_registry(&reg, "pkg.b", "0.1.0", &[], &[]);

    let root = make_manifest(
        "app",
        "1.0.0",
        &[("pkg.a", "0.1.0"), ("pkg.b", "0.1.0")],
        &[],
    );

    let r1 = resolve(&root, &reg).unwrap();
    let l1 = generate_lockfile(&r1, &reg).unwrap();

    let r2 = resolve(&root, &reg).unwrap();
    let l2 = generate_lockfile(&r2, &reg).unwrap();

    let j1 = serde_json::to_string_pretty(&l1).unwrap();
    let j2 = serde_json::to_string_pretty(&l2).unwrap();
    assert_eq!(j1, j2);
}

#[test]
fn test_lockfile_save_load_roundtrip() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "pkg.a", "0.1.0", &[], &[]);

    let root = make_manifest("app", "1.0.0", &[("pkg.a", "0.1.0")], &[]);
    let result = resolve(&root, &reg).unwrap();
    let lockfile = generate_lockfile(&result, &reg).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("llm.lock.json");
    lockfile.save(&path).unwrap();

    let loaded = Lockfile::load(&path).unwrap();
    assert_eq!(loaded.resolved.len(), lockfile.resolved.len());
    for (id, entry) in &lockfile.resolved {
        assert_eq!(loaded.resolved[id].integrity, entry.integrity);
    }
}

// === Hash Integrity ===

#[test]
fn test_hash_integrity_after_publish() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "test.pkg", "0.1.0", &[], &[]);

    // Verify passes
    assert!(reg.verify_all().is_ok());
}

#[test]
fn test_hash_tamper_detected() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "test.pkg", "0.1.0", &[], &[]);

    // Tamper with source
    let pkg_dir = reg.package_dir("test.pkg", "0.1.0");
    std::fs::write(pkg_dir.join("src/core.ax"), "TAMPERED").unwrap();

    // Verify should fail
    assert!(reg.verify_all().is_err());
}

// === Capability Aggregation ===

#[test]
fn test_capability_aggregation() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "pkg.a", "0.1.0", &[], &["net.fetch"]);
    publish_to_registry(&reg, "pkg.b", "0.1.0", &[], &["db.query", "time.now"]);

    let root = make_manifest(
        "app",
        "1.0.0",
        &[("pkg.a", "0.1.0"), ("pkg.b", "0.1.0")],
        &[],
    );
    let result = resolve(&root, &reg).unwrap();
    let caps = aggregate_capabilities(&result);
    assert_eq!(caps, vec!["db.query", "net.fetch", "time.now"]);
}

#[test]
fn test_capability_policy_enforcement() {
    let policy = CapabilityPolicy {
        allowed_capabilities: Some(vec!["net.fetch".into()]),
        denied_capabilities: None,
    };
    let caps = vec!["net.fetch".into(), "db.query".into()];
    let violations = policy.check_capabilities(&caps).unwrap_err();
    assert_eq!(violations, vec!["db.query"]);
}

#[test]
fn test_capability_policy_all_allowed() {
    let policy = CapabilityPolicy::allow_all();
    let caps = vec!["net.fetch".into(), "db.query".into(), "fs.write".into()];
    assert!(policy.check_capabilities(&caps).is_ok());
}

// === Manifest Validation ===

#[test]
fn test_manifest_validation_multiple_errors() {
    let m = PackageManifest {
        name: "BAD".into(),
        version: "nope".into(),
        description: "".into(),
        dependencies: BTreeMap::new(),
        required_capabilities: vec!["unknown.cap".into()],
        exposed_modules: vec![],
        integrity: None,
    };
    let errs = m.validate().unwrap_err();
    assert!(errs.len() >= 4); // name, version, description, capability, modules
}

// === Publish & Verify Flow ===

#[test]
fn test_publish_with_dependencies() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "dep.a", "0.1.0", &[], &[]);
    publish_to_registry(&reg, "app.main", "1.0.0", &[("dep.a", "0.1.0")], &[]);

    assert!(reg.exists("dep.a", "0.1.0"));
    assert!(reg.exists("app.main", "1.0.0"));

    // Verify both
    assert!(reg.verify_all().is_ok());
}

#[test]
fn test_duplicate_publish_rejected() {
    let (_dir, reg) = setup_registry();
    publish_to_registry(&reg, "test.pkg", "0.1.0", &[], &[]);

    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("src")).unwrap();
    std::fs::write(src.path().join("src/core.ax"), "fn main() -> Int { 99 }\n").unwrap();
    let manifest = make_manifest("test.pkg", "0.1.0", &[], &[]);
    manifest.save(&src.path().join("package.ax.json")).unwrap();

    let err = reg.publish(src.path()).unwrap_err();
    assert!(err.contains("already exists"));
}

// === CLI Init Flow ===

#[test]
fn test_init_creates_manifest() {
    let dir = tempfile::tempdir().unwrap();
    boruna_pkg::cli::cmd_init(dir.path()).unwrap();

    let manifest = PackageManifest::load(&dir.path().join("package.ax.json")).unwrap();
    assert_eq!(manifest.version, "0.1.0");
    assert!(manifest.dependencies.is_empty());
    assert!(dir.path().join("src").exists());
}

#[test]
fn test_add_remove_dependency() {
    let dir = tempfile::tempdir().unwrap();
    boruna_pkg::cli::cmd_init(dir.path()).unwrap();

    boruna_pkg::cli::cmd_add(dir.path(), "some.lib", "0.2.0").unwrap();
    let m = PackageManifest::load(&dir.path().join("package.ax.json")).unwrap();
    assert_eq!(m.dependencies["some.lib"], "0.2.0");

    boruna_pkg::cli::cmd_remove(dir.path(), "some.lib").unwrap();
    let m = PackageManifest::load(&dir.path().join("package.ax.json")).unwrap();
    assert!(!m.dependencies.contains_key("some.lib"));
}

// === End-to-End Workflow ===

#[test]
fn test_full_workflow() {
    let reg_dir = tempfile::tempdir().unwrap();
    let reg = Registry::new(reg_dir.path()).unwrap();

    // 1. Publish a library
    publish_to_registry(&reg, "std.math", "0.1.0", &[], &["time.now"]);

    // 2. Create an app that depends on it
    let app_dir = tempfile::tempdir().unwrap();
    let manifest = make_manifest("my.app", "1.0.0", &[("std.math", "0.1.0")], &[]);
    manifest
        .save(&app_dir.path().join("package.ax.json"))
        .unwrap();

    // 3. Resolve
    boruna_pkg::cli::cmd_resolve(app_dir.path(), reg_dir.path()).unwrap();

    // 4. Verify lockfile exists and is valid
    let lockfile = Lockfile::load(&app_dir.path().join("llm.lock.json")).unwrap();
    assert!(lockfile.validate().is_ok());
    assert!(lockfile.resolved.contains_key("std.math@0.1.0"));

    // 5. Verify registry integrity
    assert!(reg.verify_all().is_ok());
}

#[test]
fn test_full_workflow_with_policy_violation() {
    let reg_dir = tempfile::tempdir().unwrap();
    let reg = Registry::new(reg_dir.path()).unwrap();

    // Publish library that needs fs.write
    publish_to_registry(&reg, "lib.io", "0.1.0", &[], &["fs.write"]);

    // Create app with restricted policy
    let app_dir = tempfile::tempdir().unwrap();
    let manifest = make_manifest("my.app", "1.0.0", &[("lib.io", "0.1.0")], &[]);
    manifest
        .save(&app_dir.path().join("package.ax.json"))
        .unwrap();

    // Write policy that forbids fs.write
    let policy = CapabilityPolicy {
        allowed_capabilities: Some(vec!["net.fetch".into()]),
        denied_capabilities: None,
    };
    let policy_json = serde_json::to_string_pretty(&policy).unwrap();
    std::fs::write(app_dir.path().join("policy.ax.json"), policy_json).unwrap();

    // Install should fail due to policy violation
    let err = boruna_pkg::cli::cmd_install(app_dir.path(), reg_dir.path()).unwrap_err();
    assert!(err.contains("capability policy violation"));
}
