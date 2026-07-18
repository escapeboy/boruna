//! `boruna_run_sealed` — run an `.ax` program and return a **verifiable
//! execution record**, not just the result value.
//!
//! ## What "sealed" means here (honest scope)
//!
//! A full, signed, hash-chained **evidence bundle** is a workflow-directory
//! artifact produced by the orchestrator (`boruna workflow run --record` →
//! `boruna evidence verify`). It requires a workflow definition, a
//! DataStore, and a filesystem output directory — none of which exist at the
//! scope of a single stateless MCP `source`-string call.
//!
//! What a single MCP run *can* honestly produce is the strongest artifact the
//! run actually generates: the VM's [`boruna_vm::replay::EventLog`] plus a **deterministic
//! replay proof**. This tool:
//!
//! 1. compiles + runs the source under the requested policy, capturing the
//!    original [`boruna_vm::replay::EventLog`] (every capability call/result, actor event, UI
//!    emit, and `requires`/`ensures` contract check);
//! 2. re-executes the same module a second time, feeding the recorded
//!    capability results back through a [`ReplayHandler`] so the replay is
//!    hermetic (no re-invocation of side effects);
//! 3. compares the two logs with [`ReplayEngine::verify_full`] — every event
//!    must recur in the same order with identical payloads;
//! 4. returns `replay_verified`, the full event log, an ordered list of
//!    capability calls, and a SHA-256 digest of the canonical event log that
//!    acts as a stable **seal handle** a caller can pin.
//!
//! So the seal here is a **replay-verified event log**, deliberately *not* a
//! signed bundle. The response says so in its `seal.kind` /`seal.note` fields
//! and points at the workflow path for callers who need the full bundle. We
//! never fabricate a bundle.

use boruna_bytecode::Value;
use boruna_vm::capability_gateway::{CapabilityGateway, ReplayHandler};
use boruna_vm::error::VmError;
use boruna_vm::replay::{Event, ReplayEngine, ReplayResult};
use boruna_vm::vm::Vm;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use super::TOOL_RESPONSE_PROTOCOL_VERSION;
use crate::tools::run::{format_value, parse_policy};

/// Cap on the number of events embedded in the `event_log` array and the
/// `capability_calls` array, mirroring `run.rs`'s `TRACE_LIMIT`. The SHA-256
/// digest is always computed over the FULL log, so truncating the embedded
/// view never weakens the seal — a caller can still detect tampering via the
/// digest even when the array is clipped for transport.
const EVENT_LOG_LIMIT: usize = 1000;

/// Human-facing description of the seal semantics. Kept in one place so the
/// response and the docs stay in lockstep.
const SEAL_NOTE: &str = "This seal is a deterministic replay proof over the VM EventLog \
    (ReplayEngine::verify_full), not a signed evidence bundle. A signed, hash-chained \
    evidence bundle is a workflow-directory artifact produced by the orchestrator — see \
    `boruna workflow run --record` and `boruna evidence verify`.";

/// Compile, run, replay-verify, and seal `.ax` source.
///
/// `policy` uses the exact same shape [`boruna_run`](crate::tools::run) accepts
/// (`None` → allow-all, `"allow-all"`/`"deny-all"` shorthand, or a strict
/// Policy object). `max_steps` is the deterministic execution ceiling applied
/// to BOTH the original and the replay run.
///
/// Domain failures follow the crate convention — returned as a successful tool
/// response with `success: false` and a stable `error_kind`:
///   - compile failure → the compiler's own error JSON (`error_kind: "parse_error"` etc.)
///   - `invalid_policy` / `policy.*` → policy parse/validation failure
///   - `capability_denied` → the run hit a `CapabilityDenied`/budget error
///     (the denied capability name is reflected in `capability`)
///   - `runtime_error` → any other VM error
pub fn run_sealed(source: &str, policy: Option<&JsonValue>, max_steps: u64) -> String {
    // Compile once; clone the module for the replay run so both executions
    // start from a bit-identical program.
    let module = match boruna_compiler::compile("module", source) {
        Ok(m) => m,
        Err(e) => return compile_error_json(&e),
    };

    // Resolve policy through the SAME parser boruna_run uses.
    let gw_policy = match parse_policy(policy) {
        Ok(p) => p,
        Err(err) => {
            return serde_json::json!({
                "success": false,
                "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
                "error_kind": err.error_kind,
                "message": err.message,
            })
            .to_string();
        }
    };

    // ── Original run ──
    let gateway = CapabilityGateway::new(gw_policy.clone());
    let mut vm = Vm::new(module.clone(), gateway);
    vm.set_max_steps(max_steps);
    let value = match vm.run() {
        Ok(v) => v,
        Err(e) => return vm_error_json(&e, vm.step_count()),
    };
    let original_log = vm.event_log().clone();

    // ── Replay run ──
    // Feed the recorded capability RESULTS back through a ReplayHandler so the
    // second execution never touches a real side effect — it must reproduce
    // the identical event sequence purely from the recorded outcomes.
    let recorded: Vec<Value> = original_log.capability_results();
    let replay_gateway =
        CapabilityGateway::with_handler(gw_policy, Box::new(ReplayHandler::new(recorded)));
    let mut replay_vm = Vm::new(module, replay_gateway);
    replay_vm.set_max_steps(max_steps);

    let (replay_verified, replay_divergence): (bool, Option<String>) = match replay_vm.run() {
        Ok(_) => match ReplayEngine::verify_full(&original_log, replay_vm.event_log()) {
            ReplayResult::Identical => (true, None),
            ReplayResult::Diverged { reason } => (false, Some(reason)),
        },
        // A replay that itself errors (e.g. the recorded log was exhausted
        // because the replay diverged onto a path with more capability calls)
        // is a genuine divergence — surface it, don't claim verified.
        Err(e) => (false, Some(format!("replay execution failed: {e}"))),
    };

    // ── Build the sealed artifact ──
    let events = original_log.events();
    let event_count = events.len();

    // Digest over the FULL canonical log (never the truncated view).
    let canonical = original_log.to_json().unwrap_or_default();
    let digest = sha256_hex(canonical.as_bytes());

    let truncated = event_count > EVENT_LOG_LIMIT;
    let embedded: Vec<serde_json::Value> = events
        .iter()
        .take(EVENT_LOG_LIMIT)
        .map(event_json)
        .collect();

    let capability_calls: Vec<serde_json::Value> = events
        .iter()
        .filter_map(cap_call_json)
        .take(EVENT_LOG_LIMIT)
        .collect();

    let json = serde_json::json!({
        "success": true,
        "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
        "result": format_value(&value),
        "steps": vm.step_count(),
        "replay_verified": replay_verified,
        "replay_divergence_reason": replay_divergence,
        "event_count": event_count,
        "capability_calls": capability_calls,
        "event_log": {
            "version": original_log.version(),
            "events": embedded,
            "truncated": truncated,
        },
        "event_log_sha256": digest,
        "seal": {
            "kind": "replay-verified-event-log",
            "verified": replay_verified,
            "digest_alg": "sha256",
            "digest": digest,
            "note": SEAL_NOTE,
        },
    });

    serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".into())
}

/// Map a compile-time [`CompileError`] to this tool's typed domain-error
/// envelope. Unlike `boruna_run` (which reuses `compile::compile_error_json`'s
/// `errors[]`/`code` shape), `boruna_run_sealed` speaks a flat `error_kind`
/// taxonomy consistently across every failure path — `parse_error` for
/// lex/parse failures (mirroring `boruna_symbols`), `compile_error` for
/// type/codegen failures that occur before any execution.
fn compile_error_json(err: &boruna_compiler::CompileError) -> String {
    use boruna_compiler::CompileError;
    let (kind, message, line, col) = match err {
        CompileError::Lexer { line, col, msg } => {
            ("parse_error", msg.clone(), Some(*line), Some(*col))
        }
        CompileError::Parse { line, msg } => ("parse_error", msg.clone(), Some(*line), None),
        CompileError::Type(msg) | CompileError::Codegen(msg) => {
            ("compile_error", msg.clone(), None, None)
        }
    };
    serde_json::json!({
        "success": false,
        "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
        "error_kind": kind,
        "error": message,
        "line": line,
        "col": col,
    })
    .to_string()
}

/// Map a terminal [`VmError`] to the crate's domain-error envelope.
///
/// `CapabilityDenied` / `CapabilityBudgetExceeded` get their own
/// `capability_denied` kind (with the offending capability reflected) so a
/// caller running under a restrictive policy can distinguish a policy block
/// from an ordinary runtime fault. Everything else stays `runtime_error`,
/// matching `boruna_run`.
fn vm_error_json(err: &VmError, steps: u64) -> String {
    match err {
        VmError::CapabilityDenied(cap) | VmError::CapabilityBudgetExceeded(cap) => {
            serde_json::json!({
                "success": false,
                "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
                "error_kind": "capability_denied",
                "capability": cap.name(),
                "message": format!("{err}"),
                "steps": steps,
            })
            .to_string()
        }
        _ => serde_json::json!({
            "success": false,
            "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
            "error_kind": "runtime_error",
            "message": format!("{err}"),
            "steps": steps,
        })
        .to_string(),
    }
}

/// Render one [`Event`] as JSON. Capability args/results and payloads reuse
/// `format_value` so the shapes match `boruna_run`'s `result`.
fn event_json(event: &Event) -> serde_json::Value {
    match event {
        Event::CapCall { capability, args } => serde_json::json!({
            "event": "cap_call",
            "capability": capability,
            "args": args.iter().map(format_value).collect::<Vec<_>>(),
        }),
        Event::CapResult { capability, result } => serde_json::json!({
            "event": "cap_result",
            "capability": capability,
            "result": format_value(result),
        }),
        Event::ActorSpawn { actor_id, function } => serde_json::json!({
            "event": "actor_spawn",
            "actor_id": actor_id,
            "function": function,
        }),
        Event::MessageSend { from, to, payload } => serde_json::json!({
            "event": "message_send",
            "from": from,
            "to": to,
            "payload": format_value(payload),
        }),
        Event::MessageReceive { actor_id, payload } => serde_json::json!({
            "event": "message_receive",
            "actor_id": actor_id,
            "payload": format_value(payload),
        }),
        Event::UiEmit { tree } => serde_json::json!({
            "event": "ui_emit",
            "tree": format_value(tree),
        }),
        Event::SchedulerTick {
            round,
            active_actor,
        } => serde_json::json!({
            "event": "scheduler_tick",
            "round": round,
            "active_actor": active_actor,
        }),
        Event::ContractCheck {
            function,
            kind,
            index,
            passed,
        } => serde_json::json!({
            "event": "contract_check",
            "function": function,
            "kind": kind,
            "index": index,
            "passed": passed,
        }),
    }
}

/// Project only the `CapCall` events into the `capability_calls` summary.
fn cap_call_json(event: &Event) -> Option<serde_json::Value> {
    match event {
        Event::CapCall { capability, args } => Some(serde_json::json!({
            "capability": capability,
            "args": args.iter().map(format_value).collect::<Vec<_>>(),
        })),
        _ => None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).expect("valid JSON")
    }

    const PURE_SOURCE: &str = "fn main() -> Int {\n    1 + 2\n}\n";

    // A program that makes a capability call at the surface level. `step_input`
    // is the one builtin that compiles to `Op::CapCall`; under the MockHandler
    // it deterministically returns an empty string, so the run records a
    // CapCall + CapResult and the replay reproduces them exactly.
    const CAP_SOURCE: &str = r#"
fn main() -> Int !{step.input} {
    let _x: String = step_input("upstream")
    7
}
"#;

    #[test]
    fn pure_run_is_success_and_replay_verified() {
        let out = run_sealed(PURE_SOURCE, None, 1_000_000);
        let v = parse(&out);
        assert_eq!(v["success"], true, "output: {out}");
        assert_eq!(v["result"], json!(3));
        assert_eq!(v["replay_verified"], true);
        assert!(v["replay_divergence_reason"].is_null());
        // A pure program logs no events; the seal is still well-formed.
        assert_eq!(v["event_count"], 0);
        assert_eq!(v["capability_calls"].as_array().unwrap().len(), 0);
        assert_eq!(v["seal"]["kind"], "replay-verified-event-log");
        assert_eq!(v["seal"]["verified"], true);
        // Digest is a 64-char hex string and is echoed in both places.
        let digest = v["event_log_sha256"].as_str().unwrap();
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(v["seal"]["digest"], v["event_log_sha256"]);
    }

    #[test]
    fn capability_run_records_and_verifies() {
        let out = run_sealed(CAP_SOURCE, None, 1_000_000);
        let v = parse(&out);
        assert_eq!(v["success"], true, "output: {out}");
        assert_eq!(v["result"], json!(7));
        assert_eq!(v["replay_verified"], true, "output: {out}");

        // The run recorded a step.input CapCall + its CapResult (2 events).
        assert_eq!(v["event_count"], 2);
        let caps = v["capability_calls"].as_array().unwrap();
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0]["capability"], "step.input");

        // The embedded event log carries the call and the result.
        let events = v["event_log"]["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event"], "cap_call");
        assert_eq!(events[0]["capability"], "step.input");
        assert_eq!(events[1]["event"], "cap_result");
        assert_eq!(v["event_log"]["truncated"], false);
    }

    #[test]
    fn runtime_error_is_success_false() {
        // list_get out of bounds → a runtime trap (not a compile error).
        let src = r#"
fn main() -> Int {
    let xs: List<Int> = [1, 2, 3]
    list_get(xs, 99)
}
"#;
        let out = run_sealed(src, None, 1_000_000);
        let v = parse(&out);
        assert_eq!(v["success"], false, "output: {out}");
        assert_eq!(v["error_kind"], "runtime_error");
        assert!(v["message"].is_string());
    }

    #[test]
    fn parse_error_is_success_false_parse_error() {
        let out = run_sealed("@@@ not valid .ax", None, 1_000_000);
        let v = parse(&out);
        assert_eq!(v["success"], false, "output: {out}");
        assert_eq!(v["error_kind"], "parse_error");
    }

    #[test]
    fn capability_denied_is_reflected() {
        // deny-all blocks the step.input capability → the denial surfaces
        // with a typed error_kind and the offending capability name.
        let out = run_sealed(CAP_SOURCE, Some(&json!("deny-all")), 1_000_000);
        let v = parse(&out);
        assert_eq!(v["success"], false, "output: {out}");
        assert_eq!(v["error_kind"], "capability_denied");
        assert_eq!(v["capability"], "step.input");
        assert!(v["message"].as_str().unwrap().contains("capability denied"));
    }

    #[test]
    fn invalid_policy_is_reflected() {
        let bad = json!(42);
        let out = run_sealed(PURE_SOURCE, Some(&bad), 1_000_000);
        let v = parse(&out);
        assert_eq!(v["success"], false, "output: {out}");
        assert_eq!(v["error_kind"], "invalid_policy");
    }

    #[test]
    fn seal_note_documents_bundle_boundary() {
        // The honesty contract: the response must state that this is a replay
        // proof, not a signed bundle, and point at the workflow path.
        let out = run_sealed(PURE_SOURCE, None, 1_000_000);
        let v = parse(&out);
        let note = v["seal"]["note"].as_str().unwrap();
        assert!(note.contains("not a signed evidence bundle"));
        assert!(note.contains("workflow"));
    }

    #[test]
    fn every_response_carries_protocol_version() {
        for out in [
            run_sealed(PURE_SOURCE, None, 1_000_000),
            run_sealed("@@@", None, 1_000_000),
            run_sealed(CAP_SOURCE, Some(&json!("deny-all")), 1_000_000),
            run_sealed(PURE_SOURCE, Some(&json!(42)), 1_000_000),
        ] {
            let v = parse(&out);
            assert_eq!(
                v["protocol_version"],
                json!(TOOL_RESPONSE_PROTOCOL_VERSION),
                "missing protocol_version in: {out}"
            );
        }
    }
}
