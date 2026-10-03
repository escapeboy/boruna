//! `match` arms accept negative integer literals.

use boruna_bytecode::Value;
use boruna_compiler::compile;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::vm::Vm;

fn run(source: &str) -> Value {
    let module = compile("test", source).expect("compilation failed");
    Vm::new(module, CapabilityGateway::new(Policy::deny_all()))
        .run()
        .unwrap()
}

fn sign_program(n: i64) -> String {
    format!(
        "fn name(n: Int) -> String {{\n    match n {{\n        -1 => \"minus one\"\n        0 => \"zero\"\n        -42 => \"minus forty-two\"\n        _ => \"other\"\n    }}\n}}\nfn main() -> String {{\n    name(0 - {})\n}}\n",
        -n
    )
}

#[test]
fn negative_patterns_match_their_values() {
    assert_eq!(run(&sign_program(-1)), Value::String("minus one".into()));
    assert_eq!(
        run(&sign_program(-42)),
        Value::String("minus forty-two".into())
    );
    assert_eq!(run(&sign_program(0)), Value::String("zero".into()));
    assert_eq!(run(&sign_program(1)), Value::String("other".into()));
}

#[test]
fn a_negative_pattern_after_an_int_arm_is_not_parsed_as_subtraction() {
    // `0 => 5` then `-1 => 6` on the next line must stay two arms, not `5 - 1`.
    let src = "fn f(n: Int) -> Int {\n    match n {\n        0 => 5\n        -1 => 6\n        _ => 7\n    }\n}\nfn main() -> Int {\n    f(0 - 1)\n}\n";
    assert_eq!(run(src), Value::Int(6));
}
