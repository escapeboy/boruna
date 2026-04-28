//! Sprint W6-B: integration tests for evidence bundle envelope
//! encryption.

use boruna_orchestrator::audit::evidence::{BundleManifest, EvidenceBundleBuilder};
use boruna_orchestrator::audit::log::{AuditEvent, AuditLog};
use boruna_orchestrator::audit::verify::{verify_bundle, verify_bundle_with_kek};

/// Build a small audit log so the bundle has well-formed contents.
fn audit_for(run_id: &str) -> AuditLog {
    let mut audit = AuditLog::new();
    audit.append(AuditEvent::WorkflowStarted {
        workflow_hash: format!("wf-{run_id}"),
        policy_hash: format!("pol-{run_id}"),
    });
    audit.append(AuditEvent::WorkflowCompleted {
        result_hash: format!("res-{run_id}"),
        total_duration_ms: 10,
    });
    audit
}

fn build_encrypted_bundle(
    base: &std::path::Path,
    run_id: &str,
    kek: &[u8; 32],
    kek_id: &str,
) -> BundleManifest {
    let builder = EvidenceBundleBuilder::new(base, run_id, "enc-test")
        .unwrap()
        .with_encryption(kek, kek_id)
        .unwrap();
    let mut builder = builder;
    builder.add_workflow_def(r#"{"name":"enc"}"#).unwrap();
    builder.add_policy(r#"{"default_allow":true}"#).unwrap();
    builder
        .add_step_output("s1", "result", r#"{"value":"secret"}"#)
        .unwrap();
    builder.finalize(&audit_for(run_id)).unwrap()
}

fn build_plaintext_bundle(base: &std::path::Path, run_id: &str) -> BundleManifest {
    let mut builder = EvidenceBundleBuilder::new(base, run_id, "plain-test").unwrap();
    builder.add_workflow_def(r#"{"name":"plain"}"#).unwrap();
    builder.add_policy(r#"{"default_allow":true}"#).unwrap();
    builder
        .add_step_output("s1", "result", r#"{"value":42}"#)
        .unwrap();
    builder.finalize(&audit_for(run_id)).unwrap()
}

fn fixed_kek(seed: u8) -> [u8; 32] {
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = seed.wrapping_add(i as u8);
    }
    k
}

#[test]
fn bundle_encrypts_and_verifies_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let kek = fixed_kek(1);
    let manifest = build_encrypted_bundle(dir.path(), "run-enc-001", &kek, "k-prod");
    assert!(manifest.encryption.is_some());

    let bundle_dir = dir.path().join("run-enc-001");

    // On-disk audit_log.json is ciphertext (NOT the JSON the audit
    // log produces).
    let on_disk = std::fs::read(bundle_dir.join("audit_log.json")).unwrap();
    assert!(
        std::str::from_utf8(&on_disk)
            .ok()
            .map(|s| !s.contains("WorkflowStarted"))
            .unwrap_or(true),
        "audit_log.json on disk should be ciphertext"
    );

    // Verify with correct KEK passes.
    let result = verify_bundle_with_kek(&bundle_dir, Some(&kek));
    assert!(result.valid, "errors: {:?}", result.errors);

    // Bundle hash matches a freshly-deserialized read.
    let manifest_json = std::fs::read_to_string(bundle_dir.join("manifest.json")).unwrap();
    let reread: BundleManifest = serde_json::from_str(&manifest_json).unwrap();
    assert_eq!(reread.bundle_hash, manifest.bundle_hash);
    assert_eq!(reread.audit_log_hash, manifest.audit_log_hash);
}

#[test]
fn bundle_verify_fails_with_wrong_kek() {
    let dir = tempfile::tempdir().unwrap();
    let kek = fixed_kek(2);
    let bad_kek = fixed_kek(99);
    build_encrypted_bundle(dir.path(), "run-enc-wrong", &kek, "k-a");
    let bundle_dir = dir.path().join("run-enc-wrong");

    let result = verify_bundle_with_kek(&bundle_dir, Some(&bad_kek));
    assert!(!result.valid);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("evidence.encryption_key_mismatch")),
        "expected encryption_key_mismatch, got {:?}",
        result.errors
    );
}

#[test]
fn bundle_verify_fails_with_no_kek() {
    let dir = tempfile::tempdir().unwrap();
    let kek = fixed_kek(3);
    build_encrypted_bundle(dir.path(), "run-enc-nokey", &kek, "k-prod");
    let bundle_dir = dir.path().join("run-enc-nokey");

    // Make sure env var is not set in this test process.
    // SAFETY: tests in the same binary share env. The other tests
    // in this file pass an explicit KEK and do not consult env.
    unsafe {
        std::env::remove_var("BORUNA_BUNDLE_KEK");
    }

    let result = verify_bundle(&bundle_dir);
    assert!(!result.valid);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("evidence.encryption_key_required")),
        "expected encryption_key_required, got {:?}",
        result.errors
    );
}

#[test]
fn bundle_tampered_ciphertext_fails_at_verify() {
    let dir = tempfile::tempdir().unwrap();
    let kek = fixed_kek(4);
    build_encrypted_bundle(dir.path(), "run-enc-tamper", &kek, "k-x");
    let bundle_dir = dir.path().join("run-enc-tamper");

    // Flip a byte inside the encrypted workflow.json (skip last 16
    // bytes if we want the body, but flipping anywhere causes the
    // GCM tag to fail).
    let path = bundle_dir.join("workflow.json");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[0] ^= 0x42;
    std::fs::write(&path, &bytes).unwrap();

    let result = verify_bundle_with_kek(&bundle_dir, Some(&kek));
    assert!(!result.valid);
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("evidence.cipher_tag_invalid")),
        "expected cipher_tag_invalid, got {:?}",
        result.errors
    );
}

#[test]
fn bundle_without_encryption_field_still_works() {
    // W1-C backwards-compat: bundles produced WITHOUT --encrypt-bundle
    // continue to verify with no KEK supplied.
    let dir = tempfile::tempdir().unwrap();
    let manifest = build_plaintext_bundle(dir.path(), "run-plain-001");
    assert!(manifest.encryption.is_none());

    let bundle_dir = dir.path().join("run-plain-001");
    let result = verify_bundle(&bundle_dir);
    assert!(result.valid, "errors: {:?}", result.errors);

    // And the manifest serialized without an `encryption` key at
    // all (skip_serializing_if), so older readers see an unchanged
    // shape.
    let raw = std::fs::read_to_string(bundle_dir.join("manifest.json")).unwrap();
    assert!(!raw.contains("\"encryption\""));
}

#[test]
fn bundle_inspect_refuses_to_print_decrypted_without_flag() {
    // The CLI inspect path is gated in main.rs. Here we assert the
    // contract at the manifest level: an encrypted bundle's
    // encryption metadata is the only payload-bearing field, and a
    // reader can detect encryption without touching ciphertext.
    let dir = tempfile::tempdir().unwrap();
    let kek = fixed_kek(5);
    build_encrypted_bundle(dir.path(), "run-enc-insp", &kek, "k-insp");
    let bundle_dir = dir.path().join("run-enc-insp");

    let manifest_json = std::fs::read_to_string(bundle_dir.join("manifest.json")).unwrap();
    let manifest: BundleManifest = serde_json::from_str(&manifest_json).unwrap();
    assert!(manifest.encryption.is_some());

    // Manifest-level read does not expose plaintext bodies — the
    // file_checksums are SHA-256 hex (one-way) and `encryption.files`
    // lists names only.
    let info = manifest.encryption.as_ref().unwrap();
    assert!(info.files.contains(&"audit_log.json".to_string()));
    assert!(info.files.contains(&"workflow.json".to_string()));
    // Reading the on-disk file yields ciphertext, not JSON.
    let raw = std::fs::read(bundle_dir.join("workflow.json")).unwrap();
    assert!(!raw.starts_with(b"{\"name\""));
}

#[test]
fn encrypted_bundle_format_version_is_1_0() {
    // Encryption is additive: schema_version stays at 1.
    let dir = tempfile::tempdir().unwrap();
    let kek = fixed_kek(6);
    let manifest = build_encrypted_bundle(dir.path(), "run-enc-fmt", &kek, "k-fmt");
    assert_eq!(manifest.schema_version, 1);

    // Plaintext bundle: same.
    let plain = build_plaintext_bundle(dir.path(), "run-plain-fmt");
    assert_eq!(plain.schema_version, 1);
}

#[test]
fn bundle_verify_rejects_unknown_algorithm() {
    // Sprint W7 (NEW-1 from W7 follow-up security review): the spec
    // commits the 1.x reader to ONLY `aes-256-gcm`. A manifest
    // declaring a different algorithm — whether tampered or from a
    // hypothetical future major version — must be rejected at parse
    // before any KEK-related work happens.
    let dir = tempfile::tempdir().unwrap();
    let kek = fixed_kek(7);
    let manifest = build_encrypted_bundle(dir.path(), "run-enc-alg", &kek, "k-alg");
    assert!(manifest.encryption.is_some());

    let bundle_dir = dir.path().join("run-enc-alg");
    let manifest_path = bundle_dir.join("manifest.json");

    // Tamper with the on-disk manifest: rewrite algorithm to a value
    // the reader does not support. Everything else stays valid.
    let raw = std::fs::read_to_string(&manifest_path).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    value["encryption"]["algorithm"] = serde_json::Value::String("chacha20-poly1305".into());
    std::fs::write(&manifest_path, value.to_string()).unwrap();

    let result = verify_bundle_with_kek(&bundle_dir, Some(&kek));
    assert!(
        !result.valid,
        "expected verify to fail on tampered algorithm"
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("evidence.unsupported_algorithm")),
        "expected evidence.unsupported_algorithm, got {:?}",
        result.errors
    );
    // The KEK is correct — the algorithm gate must fire BEFORE the
    // KEK check, so we should NOT see encryption_key_mismatch.
    assert!(
        !result
            .errors
            .iter()
            .any(|e| e.contains("evidence.encryption_key_mismatch")),
        "algorithm gate must fire before KEK check; got {:?}",
        result.errors
    );
}
