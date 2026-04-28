//! CLI integration tests for `boruna evidence gc-blobs` (sprint W3-B).
//!
//! Spins up a temp data-dir, populates the blob store with one
//! referenced + one orphan blob via the persistence APIs, then
//! invokes the CLI in dry-run and actual modes and asserts the
//! reported counts and on-disk effects.

#![cfg(feature = "persist-sqlite")]

use std::fs;
use std::path::Path;
use std::process::Command;

use boruna_orchestrator::persistence::{
    BlobStore, RunCheckpointStore, RunRow, RunStatus, StepCheckpoint, StepStatus,
};
use tempfile::tempdir;

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

/// Writes one large output (offloaded to blob store as the
/// "referenced" blob) and one stray orphan blob into the data-dir's
/// blobs/ tree. Returns `(referenced_hash, orphan_hash)`.
fn seed_data_dir(data_dir: &Path) -> (String, String) {
    fs::create_dir_all(data_dir).unwrap();
    let store = RunCheckpointStore::open(&data_dir.join("runs.db")).unwrap();

    // Seed a run + a Completed checkpoint with output_blob_ref
    // pointing at a deliberately-known hash. We don't go through
    // complete_step_cas because that would require a 64KiB+ output;
    // a manual INSERT is sufficient to populate the
    // referenced-set without touching the threshold logic.
    let referenced_hash = "1".repeat(64);
    let orphan_hash = "2".repeat(64);

    store
        .insert_run(&RunRow {
            run_id: "R-gc".into(),
            workflow_name: "wf".into(),
            workflow_hash: "h".into(),
            status: RunStatus::Completed,
            started_at_ms: 0,
            updated_at_ms: 0,
            policy_json: "{}".into(),
            metadata_json: "{}".into(),
        })
        .unwrap();

    let cp_completed = StepCheckpoint {
        run_id: "R-gc".into(),
        step_id: "s1".into(),
        status: StepStatus::Completed,
        output_json: None,
        output_hash: Some(referenced_hash.clone()),
        started_at_ms: Some(0),
        ended_at_ms: Some(0),
        error_msg: None,
        attempt_count: 1,
        worker_id: None,
        lease_expires_at_ms: None,
        claim_id: 1,
        output_blob_ref: Some(referenced_hash.clone()),
    };
    store.upsert_step_checkpoint(&cp_completed).unwrap();

    // Drop both blob files into the on-disk store directly. The
    // BlobStore::write contract takes the hash as-is; we don't need
    // it to be a real SHA-256 of the content for this test.
    let blobs_root = data_dir.join("blobs");
    let bs = BlobStore::open(blobs_root).unwrap();
    bs.write(&referenced_hash, b"referenced bytes").unwrap();
    bs.write(&orphan_hash, b"orphan-bytes-larger-payload-yyyyyyyyyy")
        .unwrap();

    (referenced_hash, orphan_hash)
}

fn blob_path(data_dir: &Path, hash: &str) -> std::path::PathBuf {
    data_dir.join("blobs").join(&hash[..2]).join(hash)
}

#[test]
fn gc_blobs_dry_run_reports_orphan_without_deleting() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let (referenced, orphan) = seed_data_dir(&data_dir);

    let out = Command::new(boruna_bin())
        .args(["evidence", "gc-blobs", "--data-dir"])
        .arg(&data_dir)
        .args(["--dry-run", "--json"])
        .output()
        .expect("invoke boruna");
    assert!(
        out.status.success(),
        "dry-run gc-blobs failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("report must be JSON");
    assert_eq!(report["dry_run"], serde_json::Value::Bool(true));
    assert_eq!(report["deleted"], serde_json::Value::from(0u64));
    assert_eq!(report["orphans_found"], serde_json::Value::from(1u64));
    assert_eq!(report["referenced_count"], serde_json::Value::from(1u64));

    // Both files still on disk.
    assert!(blob_path(&data_dir, &referenced).is_file());
    assert!(blob_path(&data_dir, &orphan).is_file());
}

#[test]
fn gc_blobs_actually_deletes_orphan() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let (referenced, orphan) = seed_data_dir(&data_dir);

    let out = Command::new(boruna_bin())
        .args(["evidence", "gc-blobs", "--data-dir"])
        .arg(&data_dir)
        .args(["--json"])
        .output()
        .expect("invoke boruna");
    assert!(
        out.status.success(),
        "gc-blobs failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("report must be JSON");
    assert_eq!(report["dry_run"], serde_json::Value::Bool(false));
    assert_eq!(report["deleted"], serde_json::Value::from(1u64));
    assert_eq!(report["orphans_found"], serde_json::Value::from(1u64));

    // Referenced still on disk.
    assert!(blob_path(&data_dir, &referenced).is_file());
    // Orphan gone.
    assert!(!blob_path(&data_dir, &orphan).exists());
}

#[test]
fn gc_blobs_errors_when_runs_db_missing() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().join("data-empty");
    fs::create_dir_all(&data_dir).unwrap();

    let out = Command::new(boruna_bin())
        .args(["evidence", "gc-blobs", "--data-dir"])
        .arg(&data_dir)
        .output()
        .expect("invoke boruna");
    assert!(
        !out.status.success(),
        "expected gc-blobs to fail when runs.db is missing"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no runs.db"), "unexpected stderr: {stderr}");
}
