//! Record/replay for the `NetFetch` capability.
//!
//! Sprint 0.5-S7 (FleetQ #7). Boruna scripts are deterministic by design;
//! external HTTP is not. This module bridges the gap by recording each
//! `(method, url, request_body) → response_body` transaction to a sidecar
//! tape file, then replaying from it without touching the network.
//!
//! Two handlers:
//! - [`RecordingHttpHandler`] wraps a real [`HttpHandler`], records every call,
//!   and writes a tape file via [`RecordingHttpHandler::save_tape`].
//! - [`ReplayingHttpHandler`] serves responses from a loaded tape, never
//!   touches the network. Strict ordered match on `(method, url, request_body)`.
//!
//! Tape format and CLI surface documented in
//! `docs/design-net-record-replay.md`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use boruna_bytecode::{Capability, Value};

use crate::capability_gateway::{CapabilityHandler, MockHandler};
use crate::http_handler::{parse_net_fetch_args, HttpHandler};

/// Wire-format version of the tape file. Bumped on **breaking** shape
/// changes (field rename, removal, type change). Additive changes
/// (new optional field) keep the same `format_version`.
pub const TAPE_FORMAT_VERSION: u32 = 1;

/// One recorded HTTP transaction: request descriptor + response body.
///
/// `request_body` is `None` when the script made the call without a body
/// (typical GET). The match key during replay is
/// `(method, url, request_body)`; headers are intentionally excluded
/// from the key because auth tokens and user-agents change between
/// recording and replay sessions and would generate false mismatches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetTransaction {
    pub method: String,
    pub url: String,
    pub request_body: Option<String>,
    pub response_body: String,
}

/// A complete tape file: ordered list of recorded transactions plus a
/// wire-format version for forward compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetTape {
    pub format_version: u32,
    pub transactions: Vec<NetTransaction>,
}

impl NetTape {
    /// Empty tape ready for recording.
    pub fn new() -> Self {
        NetTape {
            format_version: TAPE_FORMAT_VERSION,
            transactions: Vec::new(),
        }
    }

    /// Load from a JSON file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let data = fs::read_to_string(path)
            .map_err(|e| format!("failed to read tape '{}': {e}", path.display()))?;
        let tape: NetTape = serde_json::from_str(&data)
            .map_err(|e| format!("failed to parse tape '{}': {e}", path.display()))?;
        if tape.format_version != TAPE_FORMAT_VERSION {
            return Err(format!(
                "tape '{}' has format_version={} but this build supports {}",
                path.display(),
                tape.format_version,
                TAPE_FORMAT_VERSION,
            ));
        }
        Ok(tape)
    }

    /// Write to a JSON file (pretty-printed for human-readability of fixtures).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize tape: {e}"))?;
        fs::write(path, json)
            .map_err(|e| format!("failed to write tape '{}': {e}", path.display()))?;
        Ok(())
    }
}

impl Default for NetTape {
    fn default() -> Self {
        NetTape::new()
    }
}

/// Wrapping HTTP handler that records each call.
///
/// All network I/O still goes through the inner [`HttpHandler`] — recording
/// requires real calls. CLI flag `--record-net-to <FILE>` requires `--live`.
///
/// Two ways to persist the tape:
/// - Construct with [`new`](RecordingHttpHandler::new) and call
///   [`save_tape`](RecordingHttpHandler::save_tape) explicitly when ready.
/// - Construct with [`with_save_path`](RecordingHttpHandler::with_save_path)
///   to enable **save-on-drop**: the tape is automatically written when the
///   handler is dropped (typically when the VM run finishes and the gateway
///   goes out of scope). Save errors during Drop are logged to stderr,
///   since `Drop` cannot return a `Result`. **Callers that need the save
///   error to surface in process exit codes must probe write access ahead
///   of time** (e.g. write an empty tape during gateway construction); the
///   CLI does this for `--record-net-to`.
///
/// **Crash-during-record limitation:** if the VM panics mid-recording (or
/// the process is killed), Drop may not run (especially under
/// `panic = "abort"`), so the tape is lost. This is a known v1 limitation
/// — a streaming append-only tape format is the future fix. Document this
/// behavior to integrators relying on partial-tape capture for repro work.
pub struct RecordingHttpHandler {
    inner: HttpHandler,
    tape: NetTape,
    /// If set, the tape is saved to this path on `Drop`.
    tape_path: Option<PathBuf>,
}

impl RecordingHttpHandler {
    /// Wrap a real HTTP handler; start with an empty tape. No save-on-drop;
    /// caller must invoke [`save_tape`](RecordingHttpHandler::save_tape).
    pub fn new(inner: HttpHandler) -> Self {
        RecordingHttpHandler {
            inner,
            tape: NetTape::new(),
            tape_path: None,
        }
    }

    /// Wrap a real HTTP handler and arm save-on-drop to the given path.
    /// The tape is automatically written when this handler is dropped.
    pub fn with_save_path(inner: HttpHandler, tape_path: PathBuf) -> Self {
        RecordingHttpHandler {
            inner,
            tape: NetTape::new(),
            tape_path: Some(tape_path),
        }
    }

    /// Snapshot of the recorded tape so far.
    pub fn tape(&self) -> &NetTape {
        &self.tape
    }

    /// Persist the recorded tape to disk explicitly.
    pub fn save_tape(&self, path: &Path) -> Result<(), String> {
        self.tape.save(path)
    }

    /// Number of recorded transactions so far. Useful for tests + status.
    pub fn len(&self) -> usize {
        self.tape.transactions.len()
    }

    /// True when nothing has been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.tape.transactions.is_empty()
    }
}

impl Drop for RecordingHttpHandler {
    fn drop(&mut self) {
        if let Some(path) = &self.tape_path {
            if let Err(e) = self.tape.save(path) {
                // Drop cannot return a Result. Log loudly and continue —
                // the user explicitly opted into save-on-drop with
                // `with_save_path`; surfacing the failure to stderr is the
                // best we can do without panicking on a destructor.
                eprintln!(
                    "warning: failed to save net tape to '{}': {e}",
                    path.display()
                );
            }
        }
    }
}

impl CapabilityHandler for RecordingHttpHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::NetFetch => {
                let descriptor = describe_net_fetch_request(args);
                // Make the real call.
                let response = self.inner.handle_net_fetch(args)?;
                // Record only on success — see design doc, errors are not
                // taped in v1 (re-recording is the user's responsibility).
                let response_body = match &response {
                    Value::String(s) => s.clone(),
                    other => format!("{other}"),
                };
                self.tape.transactions.push(NetTransaction {
                    method: descriptor.method,
                    url: descriptor.url,
                    request_body: descriptor.request_body,
                    response_body,
                });
                Ok(response)
            }
            other => MockHandler.handle(other, args),
        }
    }
}

/// Replaying HTTP handler. Serves responses from a loaded tape in strict
/// order; never touches the network.
///
/// On mismatch (next call's `(method, url, request_body)` differs from the
/// next tape entry), returns a typed error naming the position and the
/// differing field(s). On exhaustion (script asks for more calls than the
/// tape has), returns a typed error.
///
/// Under-consumption (script makes fewer calls than tape entries) is
/// silently OK — trailing entries are simply unused.
pub struct ReplayingHttpHandler {
    tape: NetTape,
    cursor: usize,
}

impl ReplayingHttpHandler {
    pub fn new(tape: NetTape) -> Self {
        ReplayingHttpHandler { tape, cursor: 0 }
    }

    /// Position in the tape — number of transactions consumed so far.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// True when the tape has no remaining transactions.
    pub fn exhausted(&self) -> bool {
        self.cursor >= self.tape.transactions.len()
    }

    /// Total recorded transactions in the tape.
    pub fn total(&self) -> usize {
        self.tape.transactions.len()
    }

    fn handle_net_fetch(&mut self, args: &[Value]) -> Result<Value, String> {
        let descriptor = describe_net_fetch_request(args);
        let position = self.cursor;
        let expected = self.tape.transactions.get(position).ok_or_else(|| {
            format!(
                "net.fetch replay tape exhausted ({} transactions consumed, \
                 script asked for more — at position {})",
                self.tape.transactions.len(),
                position,
            )
        })?;

        // Strict ordered match on (method, url, request_body). Headers
        // are intentionally excluded from the match key.
        let mut diffs: Vec<String> = Vec::new();
        if expected.method != descriptor.method {
            diffs.push(format!(
                "method differs: expected '{}', got '{}'",
                expected.method, descriptor.method
            ));
        }
        if expected.url != descriptor.url {
            diffs.push(format!(
                "url differs: expected '{}', got '{}'",
                expected.url, descriptor.url
            ));
        }
        if expected.request_body != descriptor.request_body {
            let exp = describe_body(&expected.request_body);
            let got = describe_body(&descriptor.request_body);
            diffs.push(format!("request_body differs: expected {exp}, got {got}"));
        }

        if !diffs.is_empty() {
            return Err(format!(
                "net.fetch replay mismatch at position {position}:\n  {}",
                diffs.join("\n  ")
            ));
        }

        // Match — advance and serve.
        let response = expected.response_body.clone();
        self.cursor += 1;
        Ok(Value::String(response))
    }
}

impl CapabilityHandler for ReplayingHttpHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::NetFetch => self.handle_net_fetch(args),
            other => MockHandler.handle(other, args),
        }
    }
}

/// Brief description of an `Option<String>` body for error messages.
fn describe_body(body: &Option<String>) -> String {
    match body {
        None => "None".to_string(),
        Some(s) => format!("Some(<{} bytes>)", s.len()),
    }
}

/// Internal projection of the `args` array passed to `net.fetch` into the
/// `(method, url, request_body)` tuple we use both for recording and for
/// matching. Headers are NOT included by design (see design doc).
struct NetRequestDescriptor {
    method: String,
    url: String,
    request_body: Option<String>,
}

/// Use the SAME parser the real `HttpHandler` uses, so the recorder and the
/// replay layer derive byte-identical descriptors. Headers are intentionally
/// dropped here — they are not part of the match key (auth tokens etc.
/// change between sessions).
fn describe_net_fetch_request(args: &[Value]) -> NetRequestDescriptor {
    match parse_net_fetch_args(args) {
        Some(parsed) => NetRequestDescriptor {
            method: parsed.method,
            url: parsed.url,
            request_body: parsed.body,
        },
        None => NetRequestDescriptor {
            // No URL arg → empty descriptor. The real handler would have
            // returned an error, so this code path only fires when the
            // recorder is invoked outside the normal CapabilityHandler flow
            // (e.g. test harness). Tape entries with empty URLs aren't
            // useful, but we still produce a descriptor for consistency.
            method: "GET".to_string(),
            url: String::new(),
            request_body: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(u: &str) -> Value {
        Value::String(u.into())
    }
    fn s(v: &str) -> Value {
        Value::String(v.into())
    }

    fn tape_with(entries: Vec<NetTransaction>) -> NetTape {
        NetTape {
            format_version: TAPE_FORMAT_VERSION,
            transactions: entries,
        }
    }

    fn tx(method: &str, url: &str, request: Option<&str>, response: &str) -> NetTransaction {
        NetTransaction {
            method: method.to_string(),
            url: url.to_string(),
            request_body: request.map(String::from),
            response_body: response.to_string(),
        }
    }

    // ── tape format ──

    #[test]
    fn tape_round_trip_preserves_transactions() {
        let original = tape_with(vec![
            tx("GET", "https://api.example.com/a", None, "{\"a\":1}"),
            tx(
                "POST",
                "https://api.example.com/b",
                Some("{\"x\":42}"),
                "{\"ok\":true}",
            ),
        ]);
        let json = serde_json::to_string(&original).unwrap();
        let parsed: NetTape = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn tape_load_rejects_wrong_format_version() {
        // Build a tape with an unsupported version, write it, then attempt to
        // load — must fail with a clear error message.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.tape.json");
        let bad = serde_json::json!({
            "format_version": 999,
            "transactions": [],
        });
        std::fs::write(&path, bad.to_string()).unwrap();
        let err = NetTape::load(&path).unwrap_err();
        assert!(
            err.contains("format_version=999"),
            "error must name the bad version: {err}"
        );
    }

    #[test]
    fn tape_save_then_load_round_trip_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.tape.json");
        let original = tape_with(vec![tx("GET", "https://example.com/", None, "hi")]);
        original.save(&path).unwrap();
        let loaded = NetTape::load(&path).unwrap();
        assert_eq!(original, loaded);
    }

    // ── ReplayingHttpHandler — match / mismatch / exhaustion ──

    #[test]
    fn replay_serves_in_order_on_exact_match() {
        let tape = tape_with(vec![
            tx("GET", "https://example.com/a", None, "first"),
            tx("GET", "https://example.com/b", None, "second"),
        ]);
        let mut h = ReplayingHttpHandler::new(tape);
        let r1 = h
            .handle(&Capability::NetFetch, &[url("https://example.com/a")])
            .unwrap();
        assert_eq!(r1, Value::String("first".into()));
        let r2 = h
            .handle(&Capability::NetFetch, &[url("https://example.com/b")])
            .unwrap();
        assert_eq!(r2, Value::String("second".into()));
        assert_eq!(h.cursor(), 2);
        assert!(h.exhausted());
    }

    #[test]
    fn replay_mismatch_method_returns_typed_error() {
        let tape = tape_with(vec![tx("GET", "https://example.com/a", None, "x")]);
        let mut h = ReplayingHttpHandler::new(tape);
        let err = h
            .handle(
                &Capability::NetFetch,
                &[url("https://example.com/a"), s("DELETE")],
            )
            .unwrap_err();
        assert!(err.contains("position 0"), "{err}");
        assert!(err.contains("method differs"), "{err}");
        assert!(err.contains("'GET'") && err.contains("'DELETE'"), "{err}");
    }

    #[test]
    fn replay_mismatch_url_returns_typed_error() {
        let tape = tape_with(vec![tx("GET", "https://example.com/a", None, "x")]);
        let mut h = ReplayingHttpHandler::new(tape);
        let err = h
            .handle(&Capability::NetFetch, &[url("https://example.com/b")])
            .unwrap_err();
        assert!(err.contains("url differs"), "{err}");
    }

    #[test]
    fn replay_mismatch_body_returns_typed_error() {
        let tape = tape_with(vec![tx("POST", "https://example.com/x", Some("a"), "ok")]);
        let mut h = ReplayingHttpHandler::new(tape);
        let err = h
            .handle(
                &Capability::NetFetch,
                &[url("https://example.com/x"), s("POST"), s("b")],
            )
            .unwrap_err();
        assert!(err.contains("request_body differs"), "{err}");
        assert!(err.contains("Some(<1 bytes>)"), "{err}");
    }

    #[test]
    fn replay_exhausted_returns_typed_error() {
        let tape = tape_with(vec![tx("GET", "https://example.com/a", None, "x")]);
        let mut h = ReplayingHttpHandler::new(tape);
        // Consume the only transaction.
        h.handle(&Capability::NetFetch, &[url("https://example.com/a")])
            .unwrap();
        // Next call has nothing to match against.
        let err = h
            .handle(&Capability::NetFetch, &[url("https://example.com/b")])
            .unwrap_err();
        assert!(err.contains("exhausted"), "{err}");
        assert!(err.contains("at position 1"), "{err}");
    }

    #[test]
    fn replay_under_consumption_is_silently_ok() {
        // Tape has 3 entries; script consumes only 1. No error.
        let tape = tape_with(vec![
            tx("GET", "https://example.com/a", None, "1"),
            tx("GET", "https://example.com/b", None, "2"),
            tx("GET", "https://example.com/c", None, "3"),
        ]);
        let mut h = ReplayingHttpHandler::new(tape);
        let r = h
            .handle(&Capability::NetFetch, &[url("https://example.com/a")])
            .unwrap();
        assert_eq!(r, Value::String("1".into()));
        // Cursor is at 1; tape has 3. Not exhausted yet.
        assert!(!h.exhausted());
        assert_eq!(h.cursor(), 1);
        assert_eq!(h.total(), 3);
    }

    #[test]
    fn replay_passes_non_net_caps_through_to_mock() {
        let tape = tape_with(vec![]);
        let mut h = ReplayingHttpHandler::new(tape);
        // TimeNow is mock'd — should succeed without consuming the tape.
        let r = h.handle(&Capability::TimeNow, &[]).unwrap();
        assert!(matches!(r, Value::Int(_)));
        assert_eq!(h.cursor(), 0); // not advanced
    }

    #[test]
    fn replay_default_method_get_when_omitted() {
        // When the script calls net.fetch with only a URL arg, the implicit
        // method is "GET". The replay match must use the same default — a
        // tape recorded against `GET` must match a script that omitted it.
        let tape = tape_with(vec![tx("GET", "https://example.com/", None, "ok")]);
        let mut h = ReplayingHttpHandler::new(tape);
        let r = h
            .handle(&Capability::NetFetch, &[url("https://example.com/")])
            .unwrap();
        assert_eq!(r, Value::String("ok".into()));
    }

    #[test]
    fn replay_back_to_back_identical_calls_consume_separate_entries() {
        // Polling scripts that call net.fetch on the same URL repeatedly must
        // each consume a separate tape entry — the cursor advances
        // unconditionally on a successful match. This locks the invariant
        // against a future "deduplicate by key" optimization that would
        // silently break polling.
        let tape = tape_with(vec![
            tx("GET", "https://example.com/poll", None, "first response"),
            tx("GET", "https://example.com/poll", None, "second response"),
            tx("GET", "https://example.com/poll", None, "third response"),
        ]);
        let mut h = ReplayingHttpHandler::new(tape);
        let r1 = h
            .handle(&Capability::NetFetch, &[url("https://example.com/poll")])
            .unwrap();
        let r2 = h
            .handle(&Capability::NetFetch, &[url("https://example.com/poll")])
            .unwrap();
        let r3 = h
            .handle(&Capability::NetFetch, &[url("https://example.com/poll")])
            .unwrap();
        assert_eq!(r1, Value::String("first response".into()));
        assert_eq!(r2, Value::String("second response".into()));
        assert_eq!(r3, Value::String("third response".into()));
        assert_eq!(h.cursor(), 3);
        assert!(h.exhausted());
    }

    #[test]
    fn describe_uses_same_parser_as_http_handler() {
        // describe_net_fetch_request and HttpHandler::handle_net_fetch must
        // interpret args identically — drift would silently produce
        // false-positive replay mismatches. This test exercises the agreed
        // shape across a few representative arg combinations.
        let cases: Vec<Vec<Value>> = vec![
            vec![url("https://example.com/")],
            vec![url("https://example.com/"), s("post"), s("body")],
            vec![url("https://example.com/"), s("DELETE")],
            vec![url("https://example.com/"), s("GET"), s("")], // empty body → None
        ];
        for args in cases {
            let recorder_view = describe_net_fetch_request(&args);
            let parser_view = parse_net_fetch_args(&args).unwrap();
            assert_eq!(recorder_view.url, parser_view.url, "url drift");
            assert_eq!(recorder_view.method, parser_view.method, "method drift");
            assert_eq!(
                recorder_view.request_body, parser_view.body,
                "request_body drift (recorder ignores headers; parser carries them — both must agree on body)"
            );
        }
    }

    #[test]
    fn replay_method_case_normalized_to_uppercase() {
        // Script passes "post"; recording layer normalized to "POST". Replay
        // must do the same so the case-insensitive match holds.
        let tape = tape_with(vec![tx("POST", "https://example.com/x", None, "ok")]);
        let mut h = ReplayingHttpHandler::new(tape);
        let r = h
            .handle(
                &Capability::NetFetch,
                &[url("https://example.com/x"), s("post")],
            )
            .unwrap();
        assert_eq!(r, Value::String("ok".into()));
    }

    // ── RecordingHttpHandler — save-on-drop ──

    #[test]
    fn recording_handler_saves_tape_on_drop_when_path_set() {
        // Construct a recording handler that points at a tape file. We can't
        // easily test the network-call recording path without a real HTTP
        // handler, but we CAN test that the Drop flushes whatever's in the
        // in-memory tape. Push a transaction directly, then drop the handler,
        // then load the file and verify.
        use crate::capability_gateway::NetPolicy;
        use crate::http_handler::HttpHandler;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auto.tape.json");
        let inner = HttpHandler::new(NetPolicy::default());
        {
            let mut h = RecordingHttpHandler::with_save_path(inner, path.clone());
            // Inject a transaction by hand to simulate a recorded call.
            h.tape
                .transactions
                .push(tx("GET", "https://example.com/x", None, "ok"));
            // h goes out of scope here → Drop runs → tape saved to disk.
        }
        let loaded = NetTape::load(&path).expect("tape must be saved on Drop");
        assert_eq!(loaded.transactions.len(), 1);
        assert_eq!(loaded.transactions[0].url, "https://example.com/x");
    }

    #[test]
    fn recording_handler_drop_without_path_does_not_save() {
        // Without with_save_path, Drop must not attempt to write anywhere.
        // Compile-time: just exercising the no-path branch — there's nothing
        // to assert beyond "it doesn't panic and doesn't create a file".
        use crate::capability_gateway::NetPolicy;
        use crate::http_handler::HttpHandler;
        let inner = HttpHandler::new(NetPolicy::default());
        let h = RecordingHttpHandler::new(inner);
        drop(h); // Drop runs; no path → no file. Test passes if no panic.
    }

    // ── describe_net_fetch_request ──

    #[test]
    fn describe_extracts_url_method_body() {
        let d = describe_net_fetch_request(&[url("https://example.com/x"), s("PUT"), s("payload")]);
        assert_eq!(d.url, "https://example.com/x");
        assert_eq!(d.method, "PUT");
        assert_eq!(d.request_body, Some("payload".to_string()));
    }

    #[test]
    fn describe_treats_empty_string_body_as_none() {
        // Mirrors HttpHandler::handle_net_fetch's own logic.
        let d = describe_net_fetch_request(&[url("https://example.com/"), s("POST"), s("")]);
        assert_eq!(d.request_body, None);
    }
}
