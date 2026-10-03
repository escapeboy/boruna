//! `Some(x)`, `Ok(x)` and `Err(x)` written in source must behave like the values builtins
//! return: they match their patterns and compare equal. Before the fix they compiled to a
//! generic enum value that matched no `Some`/`Ok`/`Err` arm ("no match found for value").

use boruna_bytecode::Value;
use boruna_compiler::compile;
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::vm::Vm;

fn run(source: &str) -> Value {
    let module = compile("test", source).expect("compilation failed");
    let mut vm = Vm::new(module, CapabilityGateway::new(Policy::allow_all()));
    vm.run().expect("runtime error")
}

#[test]
fn some_literal_bound_with_let_matches_some_arm() {
    let src = "fn main() -> Int {\n    let o: Option<Int> = Some(2)\n    match o {\n        Some(x) => x + 1,\n        None => 0,\n    }\n}\n";
    assert_eq!(run(src), Value::Int(3));
}

#[test]
fn some_literal_returned_from_a_function_matches_some_arm() {
    let src = "fn f() -> Option<Int> {\n    Some(2)\n}\nfn main() -> Int {\n    match f() {\n        Some(x) => x + 1,\n        None => 0,\n    }\n}\n";
    assert_eq!(run(src), Value::Int(3));
}

#[test]
fn some_literal_is_not_taken_by_a_wildcard_before_its_own_arm() {
    let src = "fn main() -> Int {\n    match Some(2) {\n        Some(x) => x,\n        _ => 0,\n    }\n}\n";
    assert_eq!(run(src), Value::Int(2));
}

#[test]
fn ok_and_err_literals_match_their_arms() {
    let ok = "fn main() -> Int {\n    match Ok(5) {\n        Ok(n) => n,\n        Err(e) => 0,\n    }\n}\n";
    assert_eq!(run(ok), Value::Int(5));
    let err = "fn main() -> Int {\n    match Err(7) {\n        Ok(n) => 0,\n        Err(e) => e,\n    }\n}\n";
    assert_eq!(run(err), Value::Int(7));
}

#[test]
fn literals_produce_the_same_values_builtins_return() {
    assert_eq!(
        run("fn main() -> Option<Int> {\n    Some(2)\n}\n"),
        Value::Some(Box::new(Value::Int(2)))
    );
    assert_eq!(
        run("fn main() -> Result<Int, String> {\n    Ok(1)\n}\n"),
        Value::Ok(Box::new(Value::Int(1)))
    );
    assert_eq!(
        run("fn main() -> Result<Int, String> {\n    Err(\"no\")\n}\n"),
        Value::Err(Box::new(Value::String("no".into())))
    );
    let eq =
        "fn main() -> Int {\n    if __builtin_int_parse(\"2\") == Some(2) { 1 } else { 0 }\n}\n";
    assert_eq!(run(eq), Value::Int(1));
}

#[test]
fn nested_literals_match_at_every_level() {
    let src = "fn main() -> Int {\n    match Some(Ok(3)) {\n        Some(r) => match r {\n            Ok(n) => n,\n            Err(e) => 0,\n        },\n        None => 0,\n    }\n}\n";
    assert_eq!(run(src), Value::Int(3));
}

#[test]
fn user_enums_and_none_are_unchanged() {
    let src = "enum Color { Red, Green }\nfn main() -> Int {\n    let c: Color = Color::Green\n    let n: Option<Int> = None\n    let a: Int = match c {\n        Red => 1,\n        Green => 2,\n    }\n    let b: Int = match n {\n        Some(x) => x,\n        None => 10,\n    }\n    a + b\n}\n";
    assert_eq!(run(src), Value::Int(12));
}

// Integer literal patterns used to go through the tag table, where every Int has the
// wildcard tag, so only `_` ever matched. They now compile to equality checks like strings.

fn run_err(source: &str) -> String {
    let module = compile("test", source).expect("compilation failed");
    let mut vm = Vm::new(module, CapabilityGateway::new(Policy::allow_all()));
    format!("{:?}", vm.run().expect_err("expected a runtime error"))
}

#[test]
fn integer_patterns_match_their_value() {
    let src = "fn main() -> Int {\n    match 3 {\n        1 => 10,\n        3 => 30,\n        _ => 0,\n    }\n}\n";
    assert_eq!(run(src), Value::Int(30));
}

#[test]
fn integer_match_without_wildcard_still_works_when_an_arm_matches() {
    let src = "fn main() -> Int {\n    match 3 {\n        1 => 10,\n        3 => 30,\n    }\n}\n";
    assert_eq!(run(src), Value::Int(30));
}

#[test]
fn literal_match_with_no_matching_arm_is_a_runtime_error_not_a_silent_unit() {
    let ints = "fn main() -> Int {\n    let r: Int = match 9 {\n        1 => 10,\n        3 => 30,\n    }\n    r + 1\n}\n";
    assert!(run_err(ints).contains("MatchExhausted"));
    let strings = "fn main() -> Int {\n    let r: Int = match \"z\" {\n        \"a\" => 1,\n        \"b\" => 2,\n    }\n    r + 1\n}\n";
    assert!(run_err(strings).contains("MatchExhausted"));
}

#[test]
fn nested_integer_match_inside_an_arm() {
    let src = "fn main() -> Int {\n    let x: Int = 3\n    match x {\n        3 => match x {\n            3 => 7,\n            _ => 0,\n        },\n        _ => 0,\n    }\n}\n";
    assert_eq!(run(src), Value::Int(7));
}

// A `match` inside an arm of a table-dispatched `match` used to take the outer match's
// table index, so the outer match ran against the inner arms.
#[test]
fn nested_table_matches_use_their_own_arms() {
    let src = "fn main() -> Int {\n    match true {\n        true => match Ok(3) {\n            Ok(n) => n,\n            Err(e) => 0,\n        },\n        false => 0,\n    }\n}\n";
    assert_eq!(run(src), Value::Int(3));
    let enums = "enum A { X, Y }\nenum B { P, Q }\nfn main() -> Int {\n    let a: A = A::Y\n    let b: B = B::P\n    match a {\n        X => 0,\n        Y => match b {\n            P => 5,\n            Q => 6,\n        },\n    }\n}\n";
    assert_eq!(run(enums), Value::Int(5));
}
