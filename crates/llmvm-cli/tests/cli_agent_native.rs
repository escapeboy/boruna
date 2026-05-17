//! CLI integration tests for the agent-native surfaces:
//! `lang codes`, `doctor`, `workflow graph`, `size`, `skills`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use tempfile::tempdir;

fn boruna_bin() -> &'static str {
    env!("CARGO_BIN_EXE_boruna")
}

/// Repo root — `CARGO_MANIFEST_DIR` is `crates/llmvm-cli/`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(boruna_bin())
        .args(args)
        .output()
        .expect("invoke boruna")
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ---- lang codes ----------------------------------------------------------

#[test]
fn lang_codes_human_lists_all_codes() {
    let out = run(&["lang", "codes"]);
    assert!(out.status.success());
    let s = stdout(&out);
    for code in [
        "E001", "E002", "E003", "E004", "E005", "E006", "E007", "E008", "E009",
    ] {
        assert!(s.contains(code), "missing {code} in:\n{s}");
    }
}

#[test]
fn lang_codes_json_has_nine_entries() {
    let out = run(&["lang", "codes", "--json"]);
    assert!(out.status.success());
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let codes = v["codes"].as_array().expect("codes array");
    assert_eq!(codes.len(), 9);
    for c in codes {
        assert!(c["code"].is_string());
        assert!(c["name"].is_string());
        assert!(c["summary"].is_string());
        assert!(c["category"].is_string());
    }
}

// ---- doctor --------------------------------------------------------------

#[test]
fn doctor_json_is_well_formed() {
    let out = run(&["doctor", "--json"]);
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(v["boruna_version"], env!("CARGO_PKG_VERSION"));
    let checks = v["checks"].as_array().expect("checks array");
    assert!(!checks.is_empty());
    let mut any_error = false;
    for c in checks {
        let status = c["status"].as_str().expect("status string");
        assert!(
            matches!(status, "ok" | "warn" | "error"),
            "bad status {status}"
        );
        if status == "error" {
            any_error = true;
        }
    }
    assert_eq!(v["ok"].as_bool().unwrap(), !any_error);
}

// ---- workflow graph ------------------------------------------------------

fn llm_code_review_dir() -> PathBuf {
    repo_root().join("examples/workflows/llm_code_review")
}

#[test]
fn workflow_graph_json_facts_are_consistent() {
    let dir = llm_code_review_dir();
    let out = Command::new(boruna_bin())
        .args(["workflow", "graph", "--json"])
        .arg(&dir)
        .output()
        .expect("invoke boruna");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");

    assert_eq!(v["is_dag"], true);
    let nodes = v["nodes"].as_array().unwrap();
    let topo: Vec<&str> = v["topological_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(nodes.len(), topo.len(), "every node appears in topo order");
    assert_eq!(v["node_count"].as_u64().unwrap() as usize, nodes.len());

    // Every edge (a,b) must place a before b in the topological order.
    for edge in v["edges"].as_array().unwrap() {
        let a = edge[0].as_str().unwrap();
        let b = edge[1].as_str().unwrap();
        let ia = topo.iter().position(|x| *x == a).unwrap();
        let ib = topo.iter().position(|x| *x == b).unwrap();
        assert!(ia < ib, "edge {a}->{b} violates topo order");
    }

    assert!(!v["roots"].as_array().unwrap().is_empty());
    assert!(!v["leaves"].as_array().unwrap().is_empty());
}

#[test]
fn workflow_graph_detects_a_cycle() {
    // Start from a real workflow def and introduce a back-dependency.
    let src = fs::read_to_string(llm_code_review_dir().join("workflow.json")).unwrap();
    let mut def: Value = serde_json::from_str(&src).unwrap();
    // fetch_diff is the root; make it depend on the terminal step `report`.
    def["steps"]["fetch_diff"]["depends_on"] = serde_json::json!(["report"]);
    if let Some(edges) = def["edges"].as_array_mut() {
        edges.push(serde_json::json!(["report", "fetch_diff"]));
    }

    let dir = tempdir().unwrap();
    fs::write(dir.path().join("workflow.json"), def.to_string()).unwrap();

    let out = Command::new(boruna_bin())
        .args(["workflow", "graph"])
        .arg(dir.path())
        .output()
        .expect("invoke boruna");
    assert_eq!(out.status.code(), Some(1), "cyclic graph must exit 1");
}

#[test]
fn workflow_graph_missing_dir_fails_cleanly() {
    let out = run(&["workflow", "graph", "/nonexistent/workflow/dir"]);
    assert!(!out.status.success());
}

// ---- size ----------------------------------------------------------------

#[test]
fn size_json_totals_are_consistent() {
    let hello = repo_root().join("examples/hello.ax");
    let out = Command::new(boruna_bin())
        .args(["size", "--json"])
        .arg(&hello)
        .output()
        .expect("invoke boruna");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");

    assert!(v["bytecode_bytes"].as_u64().unwrap() > 0);
    assert_eq!(v["bytecode_format"], "axbc");

    let functions = v["functions"].as_array().unwrap();
    let sum_ops: u64 = functions
        .iter()
        .map(|f| f["op_count"].as_u64().unwrap())
        .sum();
    assert_eq!(v["totals"]["total_ops"].as_u64().unwrap(), sum_ops);
    assert_eq!(
        v["totals"]["function_count"].as_u64().unwrap() as usize,
        functions.len()
    );
}

#[test]
fn size_missing_file_fails_cleanly() {
    let out = run(&["size", "/nonexistent/file.ax"]);
    assert!(!out.status.success());
}

// ---- skills --------------------------------------------------------------

#[test]
fn skills_list_json_has_all_skills() {
    let out = run(&["skills", "list", "--json"]);
    assert!(out.status.success());
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let skills = v["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 4);
}

#[test]
fn skills_get_returns_body() {
    let out = run(&["skills", "get", "ax-language"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("# Skill: The .ax Language"));
}

#[test]
fn skills_get_json_has_content() {
    let out = run(&["skills", "get", "diagnostics", "--json"]);
    assert!(out.status.success());
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(v["name"], "diagnostics");
    assert!(v["content"].as_str().unwrap().len() > 100);
}

#[test]
fn skills_get_unknown_exits_1() {
    let out = run(&["skills", "get", "no-such-skill"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown skill"));
}
