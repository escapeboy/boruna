//! `--live --providers providers.json` sends `llm_call` to a configured provider. A local
//! OpenAI-compatible server stands in for the provider. Needs the `http` feature:
//! `cargo test -p boruna-cli --features http --test cli_llm_providers`.
#![cfg(feature = "http")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};
use std::thread::JoinHandle;

fn boruna(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(args)
        .output()
        .expect("run boruna")
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Answer `n` chat-completions requests with `reply`; return the request bodies.
fn fake_provider(n: usize, reply: &'static str) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let mut bodies = Vec::new();
        // Fail instead of hanging forever if boruna never calls the provider.
        listener.set_nonblocking(true).unwrap();
        for _ in 0..n {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(conn) => break conn,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "boruna never called the provider"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let l = line.trim_end().to_ascii_lowercase();
                if l.is_empty() {
                    break;
                }
                if let Some(v) = l.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            bodies.push(String::from_utf8(body).unwrap());
            let json = format!(r#"{{"choices":[{{"message":{{"content":"{reply}"}}}}]}}"#);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                json.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
        }
        bodies
    });
    (base, h)
}

fn providers_file(dir: &Path, base: &str) -> std::path::PathBuf {
    let f = dir.join("providers.json");
    std::fs::write(
        &f,
        format!(r#"{{"providers":{{"local":{{"kind":"openai_compat","base_url":"{base}"}}}}}}"#),
    )
    .unwrap();
    f
}

const PROGRAM: &str = "fn ask(q: String) -> String !{llm.call} {\n    llm_call(q, \"local/tiny-model\")\n}\nfn main() -> String {\n    ask(\"ping\")\n}\n";

#[test]
fn run_live_with_providers_calls_the_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let (base, srv) = fake_provider(1, "pong from provider");
    let providers = providers_file(tmp.path(), &base);
    let prog = tmp.path().join("ask.ax");
    std::fs::write(&prog, PROGRAM).unwrap();

    let out = boruna(&["run", p(&prog), "--live", "--providers", p(&providers)]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("pong from provider"), "{stdout}");
    let bodies = srv.join().unwrap();
    let body: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(body["model"], "tiny-model");
    assert_eq!(body["messages"][0]["content"], "ping");
}

#[test]
fn without_live_the_mock_answers_and_no_request_is_made() {
    let tmp = tempfile::tempdir().unwrap();
    // A provider URL nothing listens on: any request would fail the run.
    let providers = providers_file(tmp.path(), "http://127.0.0.1:9/v1");
    let prog = tmp.path().join("ask.ax");
    std::fs::write(&prog, PROGRAM).unwrap();
    let out = boruna(&["run", p(&prog), "--providers", p(&providers)]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"mock\": true"));
}

#[test]
fn missing_api_key_is_reported_before_running() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("providers.json");
    std::fs::write(
        &f,
        r#"{"providers":{"openai":{"kind":"openai","api_key_env":"BORUNA_TEST_DEFINITELY_UNSET"}}}"#,
    )
    .unwrap();
    let prog = tmp.path().join("ask.ax");
    std::fs::write(&prog, PROGRAM).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_boruna"))
        .args(["run", p(&prog), "--live", "--providers", p(&f)])
        .env_remove("BORUNA_TEST_DEFINITELY_UNSET")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("BORUNA_TEST_DEFINITELY_UNSET"));
}

#[test]
fn workflow_live_records_the_provider_reply_and_the_call() {
    let tmp = tempfile::tempdir().unwrap();
    let (base, srv) = fake_provider(1, "reviewed");
    let providers = providers_file(tmp.path(), &base);
    let wf = tmp.path().join("wf");
    std::fs::create_dir_all(wf.join("steps")).unwrap();
    std::fs::write(
        wf.join("steps/review.ax"),
        "fn main() -> String !{llm.call} {\n    llm_call(\"review this\", \"local/m\")\n}\n",
    )
    .unwrap();
    std::fs::write(
        wf.join("workflow.json"),
        r#"{"schema_version":1,"name":"llm-live","version":"1.0.0","steps":{"review":{"kind":"source","source":"steps/review.ax","capabilities":["llm.call"]}},"edges":[]}"#,
    )
    .unwrap();
    let data = tmp.path().join("data");
    let evidence = tmp.path().join("evidence");
    let out = boruna(&[
        "workflow",
        "run",
        p(&wf),
        "--policy",
        "allow-all",
        "--live",
        "--providers",
        p(&providers),
        "--record",
        "--data-dir",
        p(&data),
        "--evidence-dir",
        p(&evidence),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    srv.join().unwrap();
    let bundle = std::fs::read_dir(&evidence)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let audit = std::fs::read_to_string(bundle.join("audit_log.json")).unwrap();
    assert!(audit.contains("\"llm.call\""), "{audit}");
    let verify = boruna(&["evidence", "verify", p(&bundle)]);
    assert!(verify.status.success());
    // The step output is the provider's reply.
    let output = std::fs::read_to_string(data.join("outputs").join("review").join("result.json"))
        .or_else(|_| {
            // Fallback: search the data dir for the stored output.
            let mut found = String::new();
            for e in walk(&data) {
                let t = std::fs::read_to_string(&e).unwrap_or_default();
                if t.contains("reviewed") {
                    found = t;
                    break;
                }
            }
            Ok::<_, std::io::Error>(found)
        })
        .unwrap();
    assert!(output.contains("reviewed"), "step output: {output}");
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

fn llm_workflow(root: &Path, with_gate: bool) -> std::path::PathBuf {
    let wf = root.join("wf");
    std::fs::create_dir_all(wf.join("steps")).unwrap();
    std::fs::write(
        wf.join("steps/review.ax"),
        "fn main() -> String !{llm.call} {\n    llm_call(\"review this\", \"local/m\")\n}\n",
    )
    .unwrap();
    std::fs::write(wf.join("steps/first.ax"), "fn main() -> Int {\n    1\n}\n").unwrap();
    let def = if with_gate {
        r#"{"schema_version":1,"name":"llm-gate","version":"1.0.0","steps":{
            "first":{"kind":"source","source":"steps/first.ax"},
            "gate":{"kind":"approval_gate","required_role":"reviewer","depends_on":["first"]},
            "review":{"kind":"source","source":"steps/review.ax","capabilities":["llm.call"],"depends_on":["gate"]}},
            "edges":[["first","gate"],["gate","review"]]}"#
    } else {
        r#"{"schema_version":1,"name":"llm-eval","version":"1.0.0","steps":{"review":{"kind":"source","source":"steps/review.ax","capabilities":["llm.call"]}},"edges":[]}"#
    };
    std::fs::write(wf.join("workflow.json"), def).unwrap();
    wf
}

#[test]
fn workflow_eval_live_calls_each_sides_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let (base_a, srv_a) = fake_provider(1, "answer a");
    let (base_b, srv_b) = fake_provider(1, "answer b");
    let dir_a = tmp.path().join("a");
    let dir_b = tmp.path().join("b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let prov_a = providers_file(&dir_a, &base_a);
    let prov_b = providers_file(&dir_b, &base_b);
    let wf = llm_workflow(tmp.path(), false);
    let out = boruna(&[
        "workflow",
        "eval",
        p(&wf),
        "--providers-a",
        p(&prov_a),
        "--providers-b",
        p(&prov_b),
        "--live",
        "--data-dir",
        p(&tmp.path().join("data")),
        "--json",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Each fake server got exactly its side's request.
    assert_eq!(srv_a.join().unwrap().len(), 1);
    assert_eq!(srv_b.join().unwrap().len(), 1);
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["provider_a"]["successes"], 1);
    assert_eq!(report["provider_b"]["successes"], 1);
    // Both files are named providers.json; the sides still get distinct names.
    assert_ne!(report["provider_a"]["name"], report["provider_b"]["name"]);
}

#[test]
fn workflow_resume_live_with_providers_calls_the_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let wf = llm_workflow(tmp.path(), true);
    let data = tmp.path().join("data");
    let out = boruna(&[
        "workflow",
        "run",
        p(&wf),
        "--policy",
        "allow-all",
        "--data-dir",
        p(&data),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(stdout.contains("Paused"), "{stdout}");
    let rid = stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("run_id: ").map(str::to_string))
        .expect("run_id line");
    let out = boruna(&["workflow", "approve", &rid, "gate", "--data-dir", p(&data)]);
    assert!(out.status.success());

    let (base, srv) = fake_provider(1, "approved review");
    let providers = providers_file(tmp.path(), &base);
    let out = boruna(&[
        "workflow",
        "resume",
        &rid,
        "--data-dir",
        p(&data),
        "--workflow-dir",
        p(&wf),
        "--policy",
        "allow-all",
        "--live",
        "--providers",
        p(&providers),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("'review': Completed"), "{stdout}");
    let bodies = srv.join().unwrap();
    let body: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(body["messages"][0]["content"], "review this");
}
