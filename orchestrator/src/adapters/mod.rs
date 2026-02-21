use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

/// Result of a single gate check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    pub gate: String,
    pub status: GateStatus,
    pub duration_ms: u64,
    pub output: String,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pass,
    Fail,
    Skip,
}

/// Context passed to gate adapters.
pub struct GateContext<'a> {
    pub workspace_root: &'a Path,
    pub example_files: Vec<String>,
}

/// Trait for gate adapters that wrap existing tooling.
pub trait GateAdapter {
    fn name(&self) -> &str;
    fn run(&self, ctx: &GateContext) -> GateResult;
}

/// Adapter: `cargo build --workspace`
pub struct CompileAdapter;

impl GateAdapter for CompileAdapter {
    fn name(&self) -> &str {
        "compile"
    }

    fn run(&self, ctx: &GateContext) -> GateResult {
        let start = Instant::now();
        let output = Command::new("cargo")
            .args(["build", "--workspace"])
            .current_dir(ctx.workspace_root)
            .output();

        let duration_ms = start.elapsed().as_millis() as u64;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let combined = format!("{stdout}{stderr}");
                let status = if out.status.success() {
                    GateStatus::Pass
                } else {
                    GateStatus::Fail
                };
                GateResult {
                    gate: "compile".into(),
                    status,
                    duration_ms,
                    output: combined,
                    details: serde_json::json!({"exit_code": out.status.code()}),
                }
            }
            Err(e) => GateResult {
                gate: "compile".into(),
                status: GateStatus::Fail,
                duration_ms,
                output: format!("failed to run cargo build: {e}"),
                details: serde_json::json!({}),
            },
        }
    }
}

/// Adapter: `cargo test --workspace`
pub struct TestAdapter;

impl GateAdapter for TestAdapter {
    fn name(&self) -> &str {
        "test"
    }

    fn run(&self, ctx: &GateContext) -> GateResult {
        let start = Instant::now();
        let output = Command::new("cargo")
            .args(["test", "--workspace"])
            .current_dir(ctx.workspace_root)
            .output();

        let duration_ms = start.elapsed().as_millis() as u64;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let combined = format!("{stdout}{stderr}");

                let status = if out.status.success() {
                    GateStatus::Pass
                } else {
                    GateStatus::Fail
                };

                // Parse test counts from output
                let (total, passed, failed) = parse_test_counts(&combined);

                GateResult {
                    gate: "test".into(),
                    status,
                    duration_ms,
                    output: combined,
                    details: serde_json::json!({
                        "total": total,
                        "passed": passed,
                        "failed": failed,
                        "exit_code": out.status.code(),
                    }),
                }
            }
            Err(e) => GateResult {
                gate: "test".into(),
                status: GateStatus::Fail,
                duration_ms,
                output: format!("failed to run cargo test: {e}"),
                details: serde_json::json!({}),
            },
        }
    }
}

/// Adapter: `cargo run -p llmvm-cli -- framework trace-hash <file>`
pub struct ReplayAdapter {
    pub expected_hashes: Vec<(String, String)>, // (file, expected_hash)
}

impl GateAdapter for ReplayAdapter {
    fn name(&self) -> &str {
        "replay"
    }

    fn run(&self, ctx: &GateContext) -> GateResult {
        if self.expected_hashes.is_empty() {
            return GateResult {
                gate: "replay".into(),
                status: GateStatus::Skip,
                duration_ms: 0,
                output: "no expected hashes configured".into(),
                details: serde_json::json!({}),
            };
        }

        let start = Instant::now();
        let mut all_pass = true;
        let mut results = Vec::new();

        for (file, expected) in &self.expected_hashes {
            let output = Command::new("cargo")
                .args([
                    "run",
                    "-p",
                    "llmvm-cli",
                    "--",
                    "framework",
                    "trace-hash",
                    file,
                ])
                .current_dir(ctx.workspace_root)
                .output();

            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    let actual_hash = stdout.lines().next().unwrap_or("").trim().to_string();
                    let matches = actual_hash == *expected;
                    if !matches {
                        all_pass = false;
                    }
                    results.push(serde_json::json!({
                        "file": file,
                        "expected": expected,
                        "actual": actual_hash,
                        "match": matches,
                    }));
                }
                Err(e) => {
                    all_pass = false;
                    results.push(serde_json::json!({
                        "file": file,
                        "error": format!("{e}"),
                    }));
                }
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        GateResult {
            gate: "replay".into(),
            status: if all_pass {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            duration_ms,
            output: format!("{} trace hash checks", self.expected_hashes.len()),
            details: serde_json::json!({"checks": results}),
        }
    }
}

/// Adapter: `cargo run -p llmvm-cli -- framework diag <file>`
pub struct DiagAdapter {
    pub files: Vec<String>,
}

impl GateAdapter for DiagAdapter {
    fn name(&self) -> &str {
        "diag"
    }

    fn run(&self, ctx: &GateContext) -> GateResult {
        let start = Instant::now();
        let mut outputs = Vec::new();

        for file in &self.files {
            let output = Command::new("cargo")
                .args(["run", "-p", "llmvm-cli", "--", "framework", "diag", file])
                .current_dir(ctx.workspace_root)
                .output();

            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    outputs.push(serde_json::json!({
                        "file": file,
                        "output": stdout,
                    }));
                }
                Err(e) => {
                    outputs.push(serde_json::json!({
                        "file": file,
                        "error": format!("{e}"),
                    }));
                }
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        GateResult {
            gate: "diag".into(),
            status: GateStatus::Pass, // diag is informational
            duration_ms,
            output: format!("{} diagnostic runs", self.files.len()),
            details: serde_json::json!({"diagnostics": outputs}),
        }
    }
}

/// Adapter: `boruna-pkg verify` — verify package integrity after dependency changes.
pub struct PackageVerifyAdapter;

impl GateAdapter for PackageVerifyAdapter {
    fn name(&self) -> &str {
        "package_verify"
    }

    fn run(&self, ctx: &GateContext) -> GateResult {
        let start = Instant::now();
        let output = Command::new("cargo")
            .args(["run", "-p", "boruna-pkg", "--", "verify"])
            .current_dir(ctx.workspace_root)
            .output();

        let duration_ms = start.elapsed().as_millis() as u64;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let combined = format!("{stdout}{stderr}");
                let status = if out.status.success() {
                    GateStatus::Pass
                } else {
                    GateStatus::Fail
                };
                GateResult {
                    gate: "package_verify".into(),
                    status,
                    duration_ms,
                    output: combined,
                    details: serde_json::json!({"exit_code": out.status.code()}),
                }
            }
            Err(e) => GateResult {
                gate: "package_verify".into(),
                status: GateStatus::Fail,
                duration_ms,
                output: format!("failed to run boruna-pkg verify: {e}"),
                details: serde_json::json!({}),
            },
        }
    }
}

/// Adapter: `boruna-pkg resolve` — verify lockfile is up-to-date after dependency changes.
pub struct PackageResolveAdapter;

impl GateAdapter for PackageResolveAdapter {
    fn name(&self) -> &str {
        "package_resolve"
    }

    fn run(&self, ctx: &GateContext) -> GateResult {
        let start = Instant::now();
        let output = Command::new("cargo")
            .args(["run", "-p", "boruna-pkg", "--", "resolve"])
            .current_dir(ctx.workspace_root)
            .output();

        let duration_ms = start.elapsed().as_millis() as u64;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let combined = format!("{stdout}{stderr}");
                let status = if out.status.success() {
                    GateStatus::Pass
                } else {
                    GateStatus::Fail
                };
                GateResult {
                    gate: "package_resolve".into(),
                    status,
                    duration_ms,
                    output: combined,
                    details: serde_json::json!({"exit_code": out.status.code()}),
                }
            }
            Err(e) => GateResult {
                gate: "package_resolve".into(),
                status: GateStatus::Fail,
                duration_ms,
                output: format!("failed to run boruna-pkg resolve: {e}"),
                details: serde_json::json!({}),
            },
        }
    }
}

/// Adapter: Verify no external LLM calls happen in deterministic pipelines.
/// Reports estimated token consumption.
pub struct LlmMockGateAdapter;

impl GateAdapter for LlmMockGateAdapter {
    fn name(&self) -> &str {
        "llm_mock_verify"
    }

    fn run(&self, ctx: &GateContext) -> GateResult {
        let start = Instant::now();

        // Check that LLM_BACKEND is not set to "external"
        let backend = std::env::var("LLM_BACKEND").unwrap_or_else(|_| "mock".to_string());
        if backend == "external" {
            return GateResult {
                gate: "llm_mock_verify".into(),
                status: GateStatus::Fail,
                duration_ms: start.elapsed().as_millis() as u64,
                output: "LLM_BACKEND=external is not allowed in deterministic pipelines".into(),
                details: serde_json::json!({"backend": backend}),
            };
        }

        // Run boruna-effect tests to verify mock mode works
        let output = Command::new("cargo")
            .args(["test", "-p", "boruna-effect"])
            .current_dir(ctx.workspace_root)
            .output();

        let duration_ms = start.elapsed().as_millis() as u64;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let combined = format!("{stdout}{stderr}");
                let status = if out.status.success() {
                    GateStatus::Pass
                } else {
                    GateStatus::Fail
                };
                GateResult {
                    gate: "llm_mock_verify".into(),
                    status,
                    duration_ms,
                    output: combined,
                    details: serde_json::json!({
                        "backend": backend,
                        "exit_code": out.status.code(),
                    }),
                }
            }
            Err(e) => GateResult {
                gate: "llm_mock_verify".into(),
                status: GateStatus::Fail,
                duration_ms,
                output: format!("failed to run boruna-effect tests: {e}"),
                details: serde_json::json!({}),
            },
        }
    }
}

/// Run all gates in order. Stops on first failure.
pub fn run_gates(adapters: &[Box<dyn GateAdapter>], ctx: &GateContext) -> Vec<GateResult> {
    let mut results = Vec::new();
    for adapter in adapters {
        let result = adapter.run(ctx);
        let failed = result.status == GateStatus::Fail;
        results.push(result);
        if failed {
            break;
        }
    }
    results
}

/// Parse test counts from cargo test output.
/// Looks for lines like "test result: ok. 51 passed; 0 failed;"
fn parse_test_counts(output: &str) -> (usize, usize, usize) {
    let mut total = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;

    for line in output.lines() {
        if line.contains("test result:") {
            // Parse "N passed" and "N failed"
            for part in line.split(';') {
                let part = part.trim();
                if part.contains("passed") {
                    if let Some(n) = part
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse::<usize>().ok())
                    {
                        // Handle "test result: ok. N passed"
                        passed += n;
                        total += n;
                    } else {
                        // Try "ok. N passed" format
                        for word in part.split_whitespace() {
                            if let Ok(n) = word.parse::<usize>() {
                                passed += n;
                                total += n;
                                break;
                            }
                        }
                    }
                }
                if part.contains("failed") && !part.contains("test result") {
                    for word in part.split_whitespace() {
                        if let Ok(n) = word.parse::<usize>() {
                            failed += n;
                            total += n;
                            break;
                        }
                    }
                }
            }
        }
    }

    (total, passed, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_test_counts() {
        let output = r#"
running 51 tests
...
test result: ok. 51 passed; 0 failed; 0 ignored

running 76 tests
...
test result: ok. 76 passed; 0 failed; 0 ignored
"#;
        let (total, passed, failed) = parse_test_counts(output);
        assert_eq!(passed, 127);
        assert_eq!(failed, 0);
        assert_eq!(total, 127);
    }

    #[test]
    fn test_parse_test_counts_with_failures() {
        let output = "test result: FAILED. 10 passed; 2 failed; 0 ignored\n";
        let (total, passed, failed) = parse_test_counts(output);
        assert_eq!(passed, 10);
        assert_eq!(failed, 2);
        assert_eq!(total, 12);
    }
}
