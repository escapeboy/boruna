//! Sprint W7 (M-6 from W6 security review): CLI integration test
//! asserting that `boruna evidence inspect <dir>` does NOT print
//! decrypted payload bytes for an encrypted bundle when `--decrypt`
//! is absent.
//!
//! Today's `inspect` only reads top-level `manifest.json` +
//! `bundle.json` (both plaintext metadata), so the gate is
//! future-proofing: a future change that adds payload preview to
//! `inspect` MUST keep `--decrypt` as the explicit opt-in for
//! ciphertext decryption.

#![cfg(feature = "persist-sqlite")]

use std::process::Command;

use boruna_orchestrator::audit::evidence::EvidenceBundleBuilder;
use boruna_orchestrator::audit::log::{AuditEvent, AuditLog};
use tempfile::tempdir;

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

fn fixed_kek() -> [u8; 32] {
    // Deterministic test KEK — never used in production. The literal
    // PLAINTEXT_CANARY appears below as a step output; if `inspect`
    // ever leaks it without `--decrypt`, this test fails.
    [
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
        0x42, 0x42,
    ]
}

const PLAINTEXT_CANARY: &str = "BORUNA-W7-PLAINTEXT-LEAK-CANARY-XYZ";

fn build_encrypted_bundle(base: &std::path::Path, run_id: &str) {
    let kek = fixed_kek();
    let mut builder = EvidenceBundleBuilder::new(base, run_id, "inspect-decrypt-test")
        .unwrap()
        .with_encryption(&kek, "test-kek")
        .unwrap();
    builder.add_workflow_def(r#"{"name":"enc-test"}"#).unwrap();
    builder.add_policy(r#"{"default_allow":true}"#).unwrap();
    // Step output carries the canary string. Encrypted on disk.
    builder
        .add_step_output(
            "s1",
            "result",
            &format!(r#"{{"secret":"{PLAINTEXT_CANARY}"}}"#),
        )
        .unwrap();
    let mut audit = AuditLog::new();
    audit.append(AuditEvent::WorkflowStarted {
        workflow_hash: "wf".into(),
        policy_hash: "pol".into(),
    });
    audit.append(AuditEvent::WorkflowCompleted {
        result_hash: "res".into(),
        total_duration_ms: 1,
    });
    builder.finalize(&audit).unwrap();
}

#[test]
fn inspect_does_not_leak_plaintext_without_decrypt_flag() {
    let dir = tempdir().unwrap();
    let run_id = "R-inspect-canary";
    build_encrypted_bundle(dir.path(), run_id);
    let bundle_dir = dir.path().join(run_id);

    // Sanity: on-disk audit_log is ciphertext — verify it does NOT
    // contain the canary string. If this fails, the bundle wasn't
    // actually encrypted and the rest of the test is meaningless.
    let on_disk = std::fs::read(bundle_dir.join("audit_log.json")).unwrap();
    assert!(
        !std::str::from_utf8(&on_disk)
            .map(|s| s.contains(PLAINTEXT_CANARY))
            .unwrap_or(false),
        "audit_log on disk leaked the canary — bundle was not encrypted"
    );

    // Run `boruna evidence inspect <dir>` WITHOUT --decrypt.
    let output = Command::new(boruna_bin())
        .args(["evidence", "inspect", bundle_dir.to_str().unwrap()])
        .output()
        .expect("inspect failed to spawn");

    // The command itself should succeed (manifest.json is plaintext
    // metadata; printing it is fine).
    assert!(
        output.status.success(),
        "inspect should succeed on encrypted bundle without --decrypt; \
         got status {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // CRITICAL: the canary MUST NOT appear in either stream. If it
    // does, a future change leaked decrypted bytes through inspect
    // without the operator opting in via --decrypt.
    assert!(
        !stdout.contains(PLAINTEXT_CANARY),
        "stdout contained the plaintext canary without --decrypt — \
         inspect leaked decrypted payload. stdout was: {stdout}"
    );
    assert!(
        !stderr.contains(PLAINTEXT_CANARY),
        "stderr contained the plaintext canary without --decrypt — \
         inspect leaked decrypted payload. stderr was: {stderr}"
    );

    // The encrypted-bundle hint MUST be surfaced so operators
    // discover the --decrypt flag exists. The hint goes to stderr
    // (per the W6-B implementation in main.rs). Match on the stable
    // substring "is encrypted" rather than the full message.
    assert!(
        stderr.contains("is encrypted"),
        "stderr should hint that the bundle is encrypted; got: {stderr}"
    );
}

#[test]
fn inspect_decrypt_flag_is_inert_when_payload_display_unimplemented() {
    // Regression guard for the current behavior: today `inspect`
    // does NOT print decrypted payloads even with --decrypt, because
    // payload preview is not implemented. The flag is wired through
    // the CLI for future-proofing. This test pins that behavior so a
    // future change adding payload display is forced to update both
    // the gate AND this test.
    let dir = tempdir().unwrap();
    let run_id = "R-decrypt-noop";
    build_encrypted_bundle(dir.path(), run_id);
    let bundle_dir = dir.path().join(run_id);

    let output = Command::new(boruna_bin())
        .args([
            "evidence",
            "inspect",
            bundle_dir.to_str().unwrap(),
            "--decrypt",
            "--bundle-encryption-key",
            "4242424242424242424242424242424242424242424242424242424242424242",
        ])
        .output()
        .expect("inspect --decrypt failed to spawn");

    assert!(
        output.status.success(),
        "inspect --decrypt should succeed; status {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Today: even WITH --decrypt, the canary should not appear
    // because payload preview is unimplemented. If a future sprint
    // adds payload display under --decrypt, change this assertion to
    // require the canary IS present, and add a complementary
    // assertion that without --decrypt it stays hidden.
    assert!(
        !combined.contains(PLAINTEXT_CANARY),
        "current `inspect` is metadata-only; this test guards the \
         contract. If a future change adds payload preview under \
         --decrypt, update this assertion (and the no-flag test). \
         combined output was: {combined}"
    );
}
