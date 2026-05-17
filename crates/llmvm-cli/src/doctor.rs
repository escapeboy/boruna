//! `boruna doctor` — environment and toolchain health checks.
//!
//! Read-only. Reports the binary version, which optional features were
//! compiled in, whether a Rust toolchain is reachable, the persistent data
//! directory's writability, and whether the current directory looks like a
//! Boruna project root. `--json` emits the report for agent consumption.

use std::path::Path;
use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub ok: bool,
    pub boruna_version: String,
    pub checks: Vec<Check>,
}

fn check(name: &str, status: Status, detail: impl Into<String>) -> Check {
    Check {
        name: name.to_string(),
        status,
        detail: detail.into(),
    }
}

/// A space-separated list of optional features compiled into this binary.
fn compiled_features() -> String {
    let mut features = Vec::new();
    if cfg!(feature = "persist-sqlite") {
        features.push("persist-sqlite");
    }
    if cfg!(feature = "serve") {
        features.push("serve");
    }
    if cfg!(feature = "http") {
        features.push("http");
    }
    if cfg!(feature = "telemetry") {
        features.push("telemetry");
    }
    if features.is_empty() {
        "none".to_string()
    } else {
        features.join(" ")
    }
}

fn check_rust_toolchain() -> Check {
    match Command::new("rustc").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            check("rust_toolchain", Status::Ok, version)
        }
        _ => check(
            "rust_toolchain",
            Status::Warn,
            "rustc not found — only needed to build Boruna from source",
        ),
    }
}

fn check_data_dir(data_dir: &Path) -> Check {
    if !data_dir.exists() {
        return check(
            "data_dir",
            Status::Ok,
            format!(
                "{} does not exist yet — created on first persistent run",
                data_dir.display()
            ),
        );
    }
    if !data_dir.is_dir() {
        return check(
            "data_dir",
            Status::Error,
            format!("{} exists but is not a directory", data_dir.display()),
        );
    }
    let probe = data_dir.join(".boruna-doctor-probe");
    match std::fs::write(&probe, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            check(
                "data_dir",
                Status::Ok,
                format!("{} exists and is writable", data_dir.display()),
            )
        }
        Err(e) => check(
            "data_dir",
            Status::Error,
            format!("{} is not writable: {e}", data_dir.display()),
        ),
    }
}

fn check_project_layout() -> Check {
    let expected = ["templates", "libs", "examples"];
    let missing: Vec<&str> = expected
        .iter()
        .filter(|d| !Path::new(d).is_dir())
        .copied()
        .collect();
    if missing.is_empty() {
        check(
            "project_layout",
            Status::Ok,
            "templates/, libs/, examples/ present — looks like a Boruna repo root",
        )
    } else {
        check(
            "project_layout",
            Status::Warn,
            format!(
                "missing {} — current directory may not be a Boruna repo root",
                missing.join(", ")
            ),
        )
    }
}

/// Run the doctor checks. Returns `true` if no check reported `Error`.
pub fn run(data_dir: &Path, json: bool) -> bool {
    let version = env!("CARGO_PKG_VERSION").to_string();
    let checks = vec![
        check("boruna_version", Status::Ok, version.clone()),
        check("compiled_features", Status::Ok, compiled_features()),
        check_rust_toolchain(),
        check_data_dir(data_dir),
        check_project_layout(),
    ];
    let ok = !checks.iter().any(|c| c.status == Status::Error);
    let report = Report {
        ok,
        boruna_version: version,
        checks,
    };

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to serialize doctor report: {e}"),
        }
    } else {
        println!("boruna doctor — version {}", report.boruna_version);
        for c in &report.checks {
            let mark = match c.status {
                Status::Ok => "ok",
                Status::Warn => "warn",
                Status::Error => "ERROR",
            };
            println!("  [{mark}] {}: {}", c.name, c.detail);
        }
        println!(
            "{}",
            if report.ok {
                "status: healthy"
            } else {
                "status: problems found"
            }
        );
    }
    ok
}
