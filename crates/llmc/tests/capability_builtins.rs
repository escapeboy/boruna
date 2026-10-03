//! Capability built-ins (`net_fetch`, `net_request`, `llm_call`): they compile to gateway calls,
//! need the capability declared on the calling function, are decided by the policy and are
//! recorded in the VM event log so a run replays exactly.

use boruna_bytecode::Value;
use boruna_compiler::compile;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::error::VmError;
use boruna_vm::replay::Event;
use boruna_vm::vm::Vm;

fn run_with(source: &str, policy: Policy) -> (Result<Value, VmError>, Vec<Event>) {
    let module = compile("test", source).expect("compilation failed");
    let mut vm = Vm::new(module, CapabilityGateway::new(policy));
    let result = vm.run();
    (result, vm.event_log().events().to_vec())
}

const LLM: &str = "fn ask(q: String) -> String !{llm.call} {\n    llm_call(\"Q: \" ++ q, \"openai/gpt-4o-mini\")\n}\nfn main() -> String {\n    ask(\"why\")\n}\n";

#[test]
fn llm_call_goes_through_the_gateway_and_is_logged() {
    let (result, events) = run_with(LLM, Policy::allow_all());
    let reply = match result.unwrap() {
        Value::String(s) => s,
        other => panic!("llm_call must return a String, got {other:?}"),
    };
    assert!(reply.contains("\"mock\": true") && reply.contains("openai/gpt-4o-mini"));
    match &events[..] {
        [Event::CapCall { capability, args }, Event::CapResult { .. }] => {
            assert_eq!(capability, "llm.call");
            assert_eq!(
                args,
                &vec![
                    Value::String("Q: why".into()),
                    Value::String("openai/gpt-4o-mini".into())
                ]
            );
        }
        other => panic!("expected one llm.call call/result pair, got {other:?}"),
    }
}

#[test]
fn net_fetch_and_net_request_pass_their_arguments_in_order() {
    let src = "fn get() -> String !{net.fetch} {\n    net_fetch(\"u1\")\n}\nfn post() -> String !{net.fetch} {\n    net_request(\"u2\", \"POST\", \"body\")\n}\nfn main() -> Int {\n    __builtin_string_len(get() ++ post())\n}\n";
    let (result, events) = run_with(src, Policy::allow_all());
    assert!(matches!(result.unwrap(), Value::Int(n) if n > 0));
    let calls: Vec<(&String, &Vec<Value>)> = events
        .iter()
        .filter_map(|e| match e {
            Event::CapCall { capability, args } => Some((capability, args)),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "net.fetch");
    assert_eq!(calls[0].1, &vec![Value::String("u1".into())]);
    assert_eq!(
        calls[1].1,
        &vec![
            Value::String("u2".into()),
            Value::String("POST".into()),
            Value::String("body".into())
        ]
    );
}

#[test]
fn calling_a_builtin_without_declaring_the_capability_does_not_compile() {
    let src =
        "fn ask(q: String) -> String {\n    llm_call(q, \"openai/x\")\n}\nfn main() -> Int { 0 }\n";
    let err = compile("test", src).unwrap_err().to_string();
    assert!(err.contains("capability not declared"), "{err}");
    assert!(err.contains("llm.call"), "{err}");
    // Declaring a different capability is not enough.
    let wrong = "fn ask(q: String) -> String !{net.fetch} {\n    llm_call(q, \"openai/x\")\n}\nfn main() -> Int { 0 }\n";
    let e = compile("test", wrong).unwrap_err().to_string();
    assert!(e.contains("capability not declared"), "{e}");
    // Nested inside a loop or branch is still found.
    let nested = "fn f() -> Int {\n    let mut n: Int = 0\n    while n < 1 {\n        if true { net_fetch(\"u\") } else { \"\" }\n        n = n + 1\n    }\n    n\n}\nfn main() -> Int { f() }\n";
    let e = compile("test", nested).unwrap_err().to_string();
    assert!(e.contains("capability not declared"), "{e}");
}

#[test]
fn the_policy_decides_at_runtime() {
    let (result, events) = run_with(LLM, Policy::deny_all());
    assert!(
        matches!(result, Err(VmError::CapabilityDenied(_))),
        "{result:?}"
    );
    assert!(events.is_empty(), "a denied call is not executed or logged");
}

#[test]
fn wrong_number_of_arguments_is_a_compile_error() {
    let src = "fn f() -> String !{net.fetch} {\n    net_fetch(\"u\", \"GET\")\n}\nfn main() -> Int { 0 }\n";
    assert!(compile("test", src).is_err());
}

/// std-llm (a stable library) defines its own `llm_call(req, tag) -> Effect`. A function the
/// program defines must win over the built-in, so adding built-ins never breaks old code.
#[test]
fn a_function_defined_in_the_program_shadows_the_builtin() {
    let src = "fn llm_call(a: Int, b: Int) -> Int {\n    a + b\n}\nfn main() -> Int {\n    llm_call(40, 2)\n}\n";
    let (result, events) = run_with(src, Policy::deny_all());
    assert_eq!(result.unwrap(), Value::Int(42));
    assert!(events.is_empty(), "the user function ran, not the gateway");
}
