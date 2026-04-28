//! Integration test for `boruna run --watch` (post1-T-1.4).
//!
//! Spawns the CLI under watch mode, modifies the watched `.ax` file,
//! and asserts a second run is observed within a short window. This
//! is timing-dependent — the deadline is generous so a busy CI
//! runner does not flake.

use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::tempdir;

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

const PROGRAM_V1: &str = "fn main() -> Int {\n    1\n}\n";
const PROGRAM_V2: &str = "fn main() -> Int {\n    2\n}\n";

/// Watch mode reruns the file when it changes.
///
/// Layout of the test:
///   1. Write `1.ax` containing PROGRAM_V1.
///   2. Spawn `boruna run --watch 1.ax`. Capture stdout line-by-line.
///   3. Wait for the first reload banner (initial run).
///   4. Overwrite the file with PROGRAM_V2.
///   5. Assert a second reload banner appears within 5 seconds.
///   6. Kill the child.
#[test]
fn watch_reruns_on_file_change() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("watched.ax");
    fs::write(&path, PROGRAM_V1).expect("write initial");

    let mut child = Command::new(boruna_bin())
        .args(["run", "--watch"])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn boruna");

    let stdout = child.stdout.take().expect("piped stdout");
    let (line_tx, line_rx) = mpsc::channel::<String>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let reader = thread::spawn(move || {
        let mut br = BufReader::new(stdout);
        let mut buf = String::new();
        while !stop_thread.load(Ordering::Relaxed) {
            buf.clear();
            match br.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    let _ = line_tx.send(buf.trim_end().to_string());
                }
                Err(_) => break,
            }
        }
    });

    let banner_count = |deadline: Instant| -> usize {
        let mut count = 0;
        while Instant::now() < deadline {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match line_rx.recv_timeout(timeout) {
                Ok(line) => {
                    if line.contains("reloading") {
                        count += 1;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        count
    };

    // Wait up to 5s for the initial banner (first run on launch).
    let initial = banner_count(Instant::now() + Duration::from_secs(5));
    assert!(
        initial >= 1,
        "expected initial reload banner, observed {initial}"
    );

    // Modify the file. Some platforms collapse adjacent fsevents, so
    // we sleep briefly first and use a real overwrite (not append) so
    // the inode's mtime advances.
    thread::sleep(Duration::from_millis(300));
    fs::write(&path, PROGRAM_V2).expect("write update");

    // Wait up to 5s for a second banner.
    let after = banner_count(Instant::now() + Duration::from_secs(5));
    let total_after_modify = initial + after;
    assert!(
        total_after_modify >= 2,
        "expected >=2 reload banners (initial + post-edit), observed {total_after_modify}"
    );

    // Tear down.
    stop.store(true, Ordering::Relaxed);
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
}
