use std::collections::BTreeMap;

use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};

use boruna_bytecode::Value;
use boruna_framework::runtime::{AppMessage, CycleRecord};
use boruna_framework::testing::TestHarness;

// ─── Trace Schema ──────────────────────────────────────────────

/// Version of the trace file format.
pub const TRACE_VERSION: u32 = 1;

/// A complete execution trace of a framework app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceFile {
    pub version: u32,
    pub source_file: String,
    pub source_hash: String,
    pub cycles: Vec<TraceCycle>,
    pub final_state_hash: String,
    pub trace_hash: String,
}

/// One cycle in the trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceCycle {
    pub cycle: u64,
    pub message: TraceMessage,
    pub state_before_hash: String,
    pub state_after_hash: String,
    pub state_after: serde_json::Value,
    pub effects: Vec<TraceEffect>,
    pub ui_tree_hash: Option<String>,
}

/// A message in the trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceMessage {
    pub tag: String,
    pub payload: serde_json::Value,
}

/// An effect in the trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEffect {
    pub kind: String,
    pub payload_hash: String,
    pub callback_tag: String,
}

// ─── Hashing ──────────────────────────────────────────────────

/// Compute SHA-256 of a string, return hex-encoded.
pub fn sha256_hex(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Hash a Value by serializing to canonical JSON.
pub fn hash_value(value: &Value) -> String {
    let json = serde_json::to_string(value).unwrap_or_default();
    sha256_hex(&json)
}

/// Compute the trace fingerprint (stable string of all cycles).
pub fn trace_fingerprint(cycles: &[TraceCycle]) -> String {
    let mut parts = Vec::new();
    for c in cycles {
        let effects_str: Vec<String> = c.effects.iter()
            .map(|e| format!("{}:{}", e.kind, e.callback_tag))
            .collect();
        parts.push(format!(
            "c{}:msg={}:{},before={},after={},effects=[{}],ui={}",
            c.cycle,
            c.message.tag,
            c.message.payload,
            c.state_before_hash,
            c.state_after_hash,
            effects_str.join(","),
            c.ui_tree_hash.as_deref().unwrap_or("none"),
        ));
    }
    parts.join("|")
}

// ─── Recording ────────────────────────────────────────────────

/// Record an execution trace by running an app with a sequence of messages.
pub fn record_trace(
    source: &str,
    source_file: &str,
    messages: Vec<AppMessage>,
) -> Result<TraceFile, String> {
    let mut harness = TestHarness::from_source(source)
        .map_err(|e| format!("failed to create harness: {e}"))?;

    for msg in &messages {
        harness.send(msg.clone())
            .map_err(|e| format!("cycle {} failed: {e}", harness.cycle()))?;
    }

    trace_from_cycle_log(source, source_file, harness.cycle_log(), harness.state())
}

/// Build a TraceFile from an existing cycle log and final state.
pub fn trace_from_cycle_log(
    source: &str,
    source_file: &str,
    cycle_log: &[CycleRecord],
    final_state: &Value,
) -> Result<TraceFile, String> {
    let source_hash = sha256_hex(source);

    let cycles: Vec<TraceCycle> = cycle_log.iter().map(|cr| {
        TraceCycle {
            cycle: cr.cycle,
            message: TraceMessage {
                tag: cr.message.tag.clone(),
                payload: serde_json::to_value(&cr.message.payload)
                    .unwrap_or(serde_json::Value::Null),
            },
            state_before_hash: hash_value(&cr.state_before),
            state_after_hash: hash_value(&cr.state_after),
            state_after: serde_json::to_value(&cr.state_after)
                .unwrap_or(serde_json::Value::Null),
            effects: cr.effects.iter().map(|e| TraceEffect {
                kind: e.kind.as_str().to_string(),
                payload_hash: hash_value(&e.payload),
                callback_tag: e.callback_tag.clone(),
            }).collect(),
            ui_tree_hash: cr.ui_tree.as_ref().map(|v| hash_value(v)),
        }
    }).collect();

    let final_state_hash = hash_value(final_state);
    let fingerprint = trace_fingerprint(&cycles);
    let trace_hash = sha256_hex(&fingerprint);

    Ok(TraceFile {
        version: TRACE_VERSION,
        source_file: source_file.to_string(),
        source_hash,
        cycles,
        final_state_hash,
        trace_hash,
    })
}

// ─── Test Generation ──────────────────────────────────────────

/// A generated test specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSpec {
    pub version: u32,
    pub name: String,
    pub source_file: String,
    pub source_hash: String,
    pub messages: Vec<TraceMessage>,
    pub assertions: Vec<TestAssertion>,
}

/// An assertion in a test spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestAssertion {
    pub kind: String,
    pub expected: String,
    pub description: String,
}

/// Generate a test spec from a trace file.
pub fn generate_test(trace: &TraceFile, name: &str) -> TestSpec {
    let messages: Vec<TraceMessage> = trace.cycles.iter()
        .map(|c| c.message.clone())
        .collect();

    let assertions = vec![
        TestAssertion {
            kind: "final_state_hash".to_string(),
            expected: trace.final_state_hash.clone(),
            description: "final state matches recorded execution".to_string(),
        },
        TestAssertion {
            kind: "trace_hash".to_string(),
            expected: trace.trace_hash.clone(),
            description: "complete trace matches recorded execution".to_string(),
        },
        TestAssertion {
            kind: "cycle_count".to_string(),
            expected: trace.cycles.len().to_string(),
            description: format!("exactly {} cycles executed", trace.cycles.len()),
        },
    ];

    TestSpec {
        version: TRACE_VERSION,
        name: name.to_string(),
        source_file: trace.source_file.clone(),
        source_hash: trace.source_hash.clone(),
        messages,
        assertions,
    }
}

// ─── Test Execution ───────────────────────────────────────────

/// Result of running a test spec.
#[derive(Debug)]
pub struct TestResult {
    pub passed: bool,
    pub assertion_results: Vec<AssertionResult>,
    pub error: Option<String>,
}

/// Result of a single assertion check.
#[derive(Debug)]
pub struct AssertionResult {
    pub kind: String,
    pub passed: bool,
    pub expected: String,
    pub actual: String,
}

/// Run a test spec against source code.
pub fn run_test(spec: &TestSpec, source: &str) -> TestResult {
    let mut harness = match TestHarness::from_source(source) {
        Ok(h) => h,
        Err(e) => return TestResult {
            passed: false,
            assertion_results: Vec::new(),
            error: Some(format!("failed to create harness: {e}")),
        },
    };

    for msg in &spec.messages {
        let payload = value_from_json(&msg.payload);
        let app_msg = AppMessage::new(&msg.tag, payload);
        if let Err(e) = harness.send(app_msg) {
            return TestResult {
                passed: false,
                assertion_results: Vec::new(),
                error: Some(format!("message send failed: {e}")),
            };
        }
    }

    // Build actual trace data
    let cycle_log = harness.cycle_log();
    let actual_cycles: Vec<TraceCycle> = cycle_log.iter().map(|cr| {
        TraceCycle {
            cycle: cr.cycle,
            message: TraceMessage {
                tag: cr.message.tag.clone(),
                payload: serde_json::to_value(&cr.message.payload)
                    .unwrap_or(serde_json::Value::Null),
            },
            state_before_hash: hash_value(&cr.state_before),
            state_after_hash: hash_value(&cr.state_after),
            state_after: serde_json::to_value(&cr.state_after)
                .unwrap_or(serde_json::Value::Null),
            effects: cr.effects.iter().map(|e| TraceEffect {
                kind: e.kind.as_str().to_string(),
                payload_hash: hash_value(&e.payload),
                callback_tag: e.callback_tag.clone(),
            }).collect(),
            ui_tree_hash: cr.ui_tree.as_ref().map(|v| hash_value(v)),
        }
    }).collect();

    let actual_final_hash = hash_value(harness.state());
    let actual_fingerprint = trace_fingerprint(&actual_cycles);
    let actual_trace_hash = sha256_hex(&actual_fingerprint);

    // Check assertions
    let mut results = Vec::new();
    for assertion in &spec.assertions {
        let actual = match assertion.kind.as_str() {
            "final_state_hash" => actual_final_hash.clone(),
            "trace_hash" => actual_trace_hash.clone(),
            "cycle_count" => actual_cycles.len().to_string(),
            other => format!("unknown assertion kind: {other}"),
        };
        results.push(AssertionResult {
            kind: assertion.kind.clone(),
            passed: actual == assertion.expected,
            expected: assertion.expected.clone(),
            actual,
        });
    }

    let all_passed = results.iter().all(|r| r.passed);

    TestResult {
        passed: all_passed,
        assertion_results: results,
        error: None,
    }
}

// ─── Value Conversion ─────────────────────────────────────────

/// Convert serde_json::Value back to boruna_bytecode::Value via serde deserialization.
/// Falls back to simple type mapping if serde roundtrip fails.
pub fn value_from_json(json: &serde_json::Value) -> Value {
    // Try serde roundtrip first (handles tagged enum format)
    if let Ok(v) = serde_json::from_value::<Value>(json.clone()) {
        return v;
    }
    // Fallback: simple type mapping for CLI-style values
    match json {
        serde_json::Value::Null => Value::Unit,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::Int(0)
            }
        }
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Array(arr) => {
            Value::List(arr.iter().map(value_from_json).collect())
        }
        serde_json::Value::Object(map) => {
            let btree: BTreeMap<String, Value> = map.iter()
                .map(|(k, v)| (k.clone(), value_from_json(v)))
                .collect();
            Value::Map(btree)
        }
    }
}

/// Convert TraceMessages to AppMessages.
pub fn messages_to_app(messages: &[TraceMessage]) -> Vec<AppMessage> {
    messages.iter()
        .map(|m| AppMessage::new(&m.tag, value_from_json(&m.payload)))
        .collect()
}

// ─── Minimization (Delta Debugging) ──────────────────────────

/// Outcome of a predicate test on a message sequence.
#[derive(Debug, PartialEq, Eq)]
pub enum PredicateOutcome {
    /// The failure still reproduces with this sequence.
    Fail,
    /// The sequence does not trigger the failure.
    Pass,
    /// The sequence causes an unrelated error.
    Unresolved,
}

/// Minimize a failing message sequence using delta debugging.
///
/// `predicate` returns `PredicateOutcome::Fail` if the failure reproduces.
/// Returns the minimal sequence that still triggers the failure.
/// The result is 1-minimal: removing any single message stops the failure.
pub fn minimize_trace(
    source: &str,
    messages: &[TraceMessage],
    predicate: &dyn Fn(&str, &[TraceMessage]) -> PredicateOutcome,
) -> Vec<TraceMessage> {
    let mut current = messages.to_vec();

    // Verify the original fails
    if predicate(source, &current) != PredicateOutcome::Fail {
        return current;
    }

    if current.len() <= 1 {
        return current;
    }

    // Phase 1: chunk-based reduction (ddmin)
    let mut granularity = 2;
    while granularity <= current.len() {
        let chunk_size = (current.len() + granularity - 1) / granularity;
        if chunk_size == 0 { break; }

        let mut reduced = false;
        let mut offset = 0;

        while offset < current.len() {
            let end = (offset + chunk_size).min(current.len());

            // Try removing this chunk
            let without_chunk: Vec<TraceMessage> = current[..offset].iter()
                .chain(current[end..].iter())
                .cloned()
                .collect();

            if !without_chunk.is_empty()
                && predicate(source, &without_chunk) == PredicateOutcome::Fail
            {
                current = without_chunk;
                reduced = true;
                // Don't advance offset — try same position again
            } else {
                offset += chunk_size;
            }
        }

        if reduced {
            granularity = 2; // restart with coarse chunks
        } else {
            granularity *= 2;
        }
    }

    // Phase 2: 1-minimal (try removing each individual message)
    let mut i = 0;
    while i < current.len() {
        if current.len() <= 1 { break; }

        let without_i: Vec<TraceMessage> = current[..i].iter()
            .chain(current[i+1..].iter())
            .cloned()
            .collect();

        if predicate(source, &without_i) == PredicateOutcome::Fail {
            current = without_i;
            // Don't advance i
        } else {
            i += 1;
        }
    }

    current
}

/// Built-in predicate: failure = runtime error during message processing.
pub fn predicate_runtime_error(source: &str, messages: &[TraceMessage]) -> PredicateOutcome {
    let mut harness = match TestHarness::from_source(source) {
        Ok(h) => h,
        Err(_) => return PredicateOutcome::Unresolved,
    };

    for msg in messages {
        let payload = value_from_json(&msg.payload);
        let app_msg = AppMessage::new(&msg.tag, payload);
        match harness.send(app_msg) {
            Ok(_) => {}
            Err(_) => return PredicateOutcome::Fail,
        }
    }

    PredicateOutcome::Pass
}

/// Create a predicate that checks for state mismatch.
/// Returns Fail if the final state hash differs from `expected_hash`.
pub fn make_state_mismatch_predicate(
    expected_hash: String,
) -> Box<dyn Fn(&str, &[TraceMessage]) -> PredicateOutcome> {
    Box::new(move |source, messages| {
        let mut harness = match TestHarness::from_source(source) {
            Ok(h) => h,
            Err(_) => return PredicateOutcome::Unresolved,
        };

        for msg in messages {
            let payload = value_from_json(&msg.payload);
            let app_msg = AppMessage::new(&msg.tag, payload);
            if harness.send(app_msg).is_err() {
                return PredicateOutcome::Unresolved;
            }
        }

        let actual_hash = hash_value(harness.state());
        if actual_hash != expected_hash {
            PredicateOutcome::Fail
        } else {
            PredicateOutcome::Pass
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const COUNTER_APP: &str = r#"
type State { count: Int, label: String }
type Msg { tag: String, payload: Int }
type Effect { kind: String, payload: String, callback_tag: String }
type UpdateResult { state: State, effects: List<Effect> }
type UINode { tag: String, text: String }

fn init() -> State {
    State { count: 0, label: "counter" }
}

fn update(state: State, msg: Msg) -> UpdateResult {
    if msg.tag == "increment" {
        UpdateResult {
            state: State { count: state.count + 1, label: state.label },
            effects: [],
        }
    } else {
        if msg.tag == "decrement" {
            UpdateResult {
                state: State { count: state.count - 1, label: state.label },
                effects: [],
            }
        } else {
            UpdateResult {
                state: state,
                effects: [],
            }
        }
    }
}

fn view(state: State) -> UINode {
    UINode { tag: "text", text: state.label }
}

fn main() -> Int {
    let s: State = init()
    s.count
}
"#;

    fn make_messages(tags: &[&str]) -> Vec<AppMessage> {
        tags.iter()
            .map(|t| AppMessage::new(*t, Value::Int(0)))
            .collect()
    }

    fn make_trace_messages(tags: &[&str]) -> Vec<TraceMessage> {
        tags.iter()
            .map(|t| TraceMessage {
                tag: t.to_string(),
                payload: serde_json::to_value(&Value::Int(0)).unwrap(),
            })
            .collect()
    }

    #[test]
    fn test_sha256_deterministic() {
        let h1 = sha256_hex("hello world");
        let h2 = sha256_hex("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // 256 bits = 64 hex chars
    }

    #[test]
    fn test_hash_value_deterministic() {
        let v = Value::Int(42);
        let h1 = hash_value(&v);
        let h2 = hash_value(&v);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_record_trace() {
        let msgs = make_messages(&["increment", "increment", "decrement"]);
        let trace = record_trace(COUNTER_APP, "test.ax", msgs).unwrap();

        assert_eq!(trace.version, 1);
        assert_eq!(trace.source_file, "test.ax");
        assert_eq!(trace.cycles.len(), 3);
        assert!(!trace.source_hash.is_empty());
        assert!(!trace.final_state_hash.is_empty());
        assert!(!trace.trace_hash.is_empty());

        // Verify cycle data
        assert_eq!(trace.cycles[0].cycle, 1);
        assert_eq!(trace.cycles[0].message.tag, "increment");
        assert_eq!(trace.cycles[1].cycle, 2);
        assert_eq!(trace.cycles[2].cycle, 3);
        assert_eq!(trace.cycles[2].message.tag, "decrement");
    }

    #[test]
    fn test_trace_determinism() {
        let msgs1 = make_messages(&["increment", "decrement", "increment"]);
        let msgs2 = make_messages(&["increment", "decrement", "increment"]);

        let trace1 = record_trace(COUNTER_APP, "test.ax", msgs1).unwrap();
        let trace2 = record_trace(COUNTER_APP, "test.ax", msgs2).unwrap();

        assert_eq!(trace1.trace_hash, trace2.trace_hash);
        assert_eq!(trace1.final_state_hash, trace2.final_state_hash);
        assert_eq!(trace1.source_hash, trace2.source_hash);

        // Cycle-level determinism
        for (c1, c2) in trace1.cycles.iter().zip(trace2.cycles.iter()) {
            assert_eq!(c1.state_before_hash, c2.state_before_hash);
            assert_eq!(c1.state_after_hash, c2.state_after_hash);
        }
    }

    #[test]
    fn test_trace_json_roundtrip() {
        let msgs = make_messages(&["increment", "increment"]);
        let trace = record_trace(COUNTER_APP, "test.ax", msgs).unwrap();

        let json = serde_json::to_string_pretty(&trace).unwrap();
        let restored: TraceFile = serde_json::from_str(&json).unwrap();

        assert_eq!(trace.version, restored.version);
        assert_eq!(trace.trace_hash, restored.trace_hash);
        assert_eq!(trace.final_state_hash, restored.final_state_hash);
        assert_eq!(trace.cycles.len(), restored.cycles.len());
    }

    #[test]
    fn test_generate_test() {
        let msgs = make_messages(&["increment", "increment", "decrement"]);
        let trace = record_trace(COUNTER_APP, "test.ax", msgs).unwrap();
        let spec = generate_test(&trace, "counter_regression");

        assert_eq!(spec.version, 1);
        assert_eq!(spec.name, "counter_regression");
        assert_eq!(spec.messages.len(), 3);
        assert_eq!(spec.assertions.len(), 3);

        // Check assertion kinds
        let kinds: Vec<&str> = spec.assertions.iter().map(|a| a.kind.as_str()).collect();
        assert!(kinds.contains(&"final_state_hash"));
        assert!(kinds.contains(&"trace_hash"));
        assert!(kinds.contains(&"cycle_count"));
    }

    #[test]
    fn test_run_test_pass() {
        let msgs = make_messages(&["increment", "increment"]);
        let trace = record_trace(COUNTER_APP, "test.ax", msgs).unwrap();
        let spec = generate_test(&trace, "test");
        let result = run_test(&spec, COUNTER_APP);

        assert!(result.passed, "test should pass: {:?}", result);
        assert!(result.error.is_none());
        assert_eq!(result.assertion_results.len(), 3);
        for ar in &result.assertion_results {
            assert!(ar.passed, "assertion {} failed: expected={}, actual={}",
                ar.kind, ar.expected, ar.actual);
        }
    }

    #[test]
    fn test_run_test_fail_on_different_source() {
        let msgs = make_messages(&["increment"]);
        let trace = record_trace(COUNTER_APP, "test.ax", msgs).unwrap();
        let spec = generate_test(&trace, "test");

        // Modify the source so state hashes change
        let modified = COUNTER_APP.replace("count: 0", "count: 100");
        let result = run_test(&spec, &modified);

        assert!(!result.passed, "test should fail with modified source");
    }

    #[test]
    fn test_value_from_json_serde_roundtrip() {
        let values = vec![
            Value::Int(42),
            Value::String("hello".into()),
            Value::Bool(true),
            Value::Float(3.14),
            Value::Unit,
            Value::List(vec![Value::Int(1), Value::Int(2)]),
        ];

        for v in &values {
            let json = serde_json::to_value(v).unwrap();
            let restored = value_from_json(&json);
            assert_eq!(v, &restored, "roundtrip failed for {:?}", v);
        }
    }

    #[test]
    fn test_value_from_json_simple() {
        // Test fallback simple conversion
        let json_int = serde_json::Value::Number(serde_json::Number::from(42));
        // This may use serde roundtrip or fallback, either way should produce Int
        let v = value_from_json(&json_int);
        // The simple JSON number 42 doesn't match serde's tagged format {"Int": 42},
        // so it falls back to simple mapping
        assert_eq!(v, Value::Int(42));
    }

    #[test]
    fn test_minimize_no_reduction_needed() {
        // Single message — already minimal
        let msgs = make_trace_messages(&["increment"]);
        let pred = |_src: &str, _msgs: &[TraceMessage]| -> PredicateOutcome {
            PredicateOutcome::Fail
        };

        let minimal = minimize_trace(COUNTER_APP, &msgs, &pred);
        assert_eq!(minimal.len(), 1);
    }

    #[test]
    fn test_minimize_removes_unnecessary() {
        // 10 messages where only the 5th causes the "failure"
        // Predicate: fails if any message has tag "bad"
        let tags: Vec<&str> = vec![
            "increment", "increment", "increment", "increment",
            "bad",
            "increment", "increment", "increment", "increment", "increment",
        ];
        let msgs = make_trace_messages(&tags);

        let pred = |_src: &str, msgs: &[TraceMessage]| -> PredicateOutcome {
            if msgs.iter().any(|m| m.tag == "bad") {
                PredicateOutcome::Fail
            } else {
                PredicateOutcome::Pass
            }
        };

        let minimal = minimize_trace(COUNTER_APP, &msgs, &pred);
        // Should reduce to just the "bad" message
        assert_eq!(minimal.len(), 1);
        assert_eq!(minimal[0].tag, "bad");
    }

    #[test]
    fn test_minimize_preserves_ordering() {
        // Predicate: fails if "a" appears before "b"
        let msgs = make_trace_messages(&[
            "increment", "a", "increment", "increment", "b", "increment",
        ]);

        let pred = |_src: &str, msgs: &[TraceMessage]| -> PredicateOutcome {
            let has_a = msgs.iter().position(|m| m.tag == "a");
            let has_b = msgs.iter().position(|m| m.tag == "b");
            match (has_a, has_b) {
                (Some(a_pos), Some(b_pos)) if a_pos < b_pos => PredicateOutcome::Fail,
                _ => PredicateOutcome::Pass,
            }
        };

        let minimal = minimize_trace(COUNTER_APP, &msgs, &pred);
        // Should reduce to ["a", "b"]
        assert_eq!(minimal.len(), 2);
        assert_eq!(minimal[0].tag, "a");
        assert_eq!(minimal[1].tag, "b");
    }

    #[test]
    fn test_minimize_deterministic() {
        let tags: Vec<&str> = vec![
            "increment", "increment", "bad", "increment", "bad", "increment",
        ];
        let msgs = make_trace_messages(&tags);

        let pred = |_src: &str, msgs: &[TraceMessage]| -> PredicateOutcome {
            let bad_count = msgs.iter().filter(|m| m.tag == "bad").count();
            if bad_count >= 2 { PredicateOutcome::Fail } else { PredicateOutcome::Pass }
        };

        let min1 = minimize_trace(COUNTER_APP, &msgs, &pred);
        let min2 = minimize_trace(COUNTER_APP, &msgs, &pred);

        // Same input → same output
        assert_eq!(min1.len(), min2.len());
        for (a, b) in min1.iter().zip(min2.iter()) {
            assert_eq!(a.tag, b.tag);
        }
    }

    #[test]
    fn test_minimize_original_passes() {
        // If the original doesn't fail, return it unchanged
        let msgs = make_trace_messages(&["increment", "increment"]);

        let pred = |_src: &str, _msgs: &[TraceMessage]| -> PredicateOutcome {
            PredicateOutcome::Pass
        };

        let result = minimize_trace(COUNTER_APP, &msgs, &pred);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_predicate_runtime_error() {
        // Counter app doesn't crash on any valid message
        let msgs = make_trace_messages(&["increment", "decrement"]);
        let outcome = predicate_runtime_error(COUNTER_APP, &msgs);
        assert_eq!(outcome, PredicateOutcome::Pass);
    }

    #[test]
    fn test_state_mismatch_predicate() {
        // Record expected state with 2 increments
        let msgs = make_messages(&["increment", "increment"]);
        let trace = record_trace(COUNTER_APP, "test.ax", msgs).unwrap();

        let pred = make_state_mismatch_predicate(trace.final_state_hash.clone());

        // Same messages → state matches → Pass
        let same_msgs = make_trace_messages(&["increment", "increment"]);
        assert_eq!(pred(COUNTER_APP, &same_msgs), PredicateOutcome::Pass);

        // Different messages → state differs → Fail
        let diff_msgs = make_trace_messages(&["increment", "increment", "increment"]);
        assert_eq!(pred(COUNTER_APP, &diff_msgs), PredicateOutcome::Fail);
    }

    #[test]
    fn test_e2e_record_generate_run() {
        // Full pipeline: record → generate → run
        let msgs = make_messages(&[
            "increment", "increment", "decrement", "increment",
        ]);
        let trace = record_trace(COUNTER_APP, "test.ax", msgs).unwrap();
        let spec = generate_test(&trace, "e2e_test");
        let result = run_test(&spec, COUNTER_APP);

        assert!(result.passed, "e2e pipeline should pass: {:?}", result);
    }

    #[test]
    fn test_e2e_record_minimize_generate_run() {
        // Full pipeline: record → minimize → generate → run
        // Create a sequence with extra messages, minimize to essential
        let msgs = make_messages(&[
            "increment", "increment", "increment", "increment", "increment",
        ]);
        let trace = record_trace(COUNTER_APP, "test.ax", msgs).unwrap();

        // Minimize with state mismatch predicate (count=5)
        let pred = make_state_mismatch_predicate(trace.final_state_hash.clone());
        let trace_msgs: Vec<TraceMessage> = trace.cycles.iter()
            .map(|c| c.message.clone())
            .collect();

        // All messages produce the same tag, so minimizer can't reduce
        // (removing any increment changes the final count)
        let minimal = minimize_trace(COUNTER_APP, &trace_msgs, &*pred);
        assert_eq!(minimal.len(), 5); // all increments are necessary for count=5

        // Generate and run from minimal
        let app_msgs = messages_to_app(&minimal);
        let min_trace = record_trace(COUNTER_APP, "test.ax", app_msgs).unwrap();
        let spec = generate_test(&min_trace, "minimal_test");
        let result = run_test(&spec, COUNTER_APP);
        assert!(result.passed);
    }

    #[test]
    fn test_trace_empty_messages() {
        let trace = record_trace(COUNTER_APP, "test.ax", vec![]).unwrap();
        assert_eq!(trace.cycles.len(), 0);
        assert!(!trace.final_state_hash.is_empty());
        assert!(!trace.trace_hash.is_empty());
    }

    #[test]
    fn test_test_spec_json_roundtrip() {
        let msgs = make_messages(&["increment"]);
        let trace = record_trace(COUNTER_APP, "test.ax", msgs).unwrap();
        let spec = generate_test(&trace, "roundtrip_test");

        let json = serde_json::to_string_pretty(&spec).unwrap();
        let restored: TestSpec = serde_json::from_str(&json).unwrap();

        assert_eq!(spec.name, restored.name);
        assert_eq!(spec.messages.len(), restored.messages.len());
        assert_eq!(spec.assertions.len(), restored.assertions.len());
    }
}
