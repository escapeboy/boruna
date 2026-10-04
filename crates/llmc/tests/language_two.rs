//! Language 2.0: E009 and E010 reject the program, `break` / `continue`, and an `if` without
//! `else` used as a statement.

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

fn rejected(source: &str) -> String {
    compile("test", source)
        .expect_err("must not compile")
        .to_string()
}

#[test]
fn reassigning_a_binding_that_is_not_mut_is_rejected() {
    let err = rejected("fn main() -> Int {\n    let x: Int = 1\n    x = 2\n    x\n}\n");
    assert!(
        err.contains("in function 'main'") && err.contains("cannot reassign 'x'"),
        "{err}"
    );
    let err = rejected(
        "fn f(n: Int) -> Int {\n    n = n + 1\n    n\n}\nfn main() -> Int {\n    f(1)\n}\n",
    );
    assert!(err.contains("cannot reassign parameter 'n'"), "{err}");
    let err =
        rejected("fn main() -> Int {\n    for v in [1, 2] {\n        v = 3\n    }\n    0\n}\n");
    assert!(err.contains("cannot reassign loop variable 'v'"), "{err}");
}

#[test]
fn type_mismatches_are_rejected() {
    let err = rejected("fn main() -> Int {\n    let x: Int = \"a\"\n    0\n}\n");
    assert!(err.contains("annotated 'Int'"), "{err}");
    let err =
        rejected("fn add(a: Int) -> Int {\n    a\n}\nfn main() -> Int {\n    add(\"x\")\n}\n");
    assert!(err.contains("argument 1 expects 'Int'"), "{err}");
    let err = rejected("fn main() -> Int {\n    let mut x: Int = 1\n    x = \"a\"\n    0\n}\n");
    assert!(err.contains("assigned a value of type 'String'"), "{err}");
    let err = rejected("fn main() -> Int {\n    while 1 {\n    }\n    0\n}\n");
    assert!(err.contains("while condition must be Bool"), "{err}");
}

#[test]
fn several_issues_name_the_first_and_count_the_rest() {
    let err = rejected(
        "fn main() -> Int {\n    let x: Int = 1\n    x = 2\n    let y: Int = \"s\"\n    x\n}\n",
    );
    assert!(
        err.contains("(and 1 more; run `boruna lang check`"),
        "{err}"
    );
}

#[test]
fn shadowing_and_match_bindings_do_not_cause_false_type_errors() {
    // `let s` shadows an Int with a value of unknown type; a match arm binds `n` anew.
    let src = "fn main() -> Int {\n    let n: Int = 1\n    let s: Int = 2\n    let s = __builtin_string_len(\"abc\")\n    let o: Option<String> = Some(\"x\")\n    let m: Int = match o {\n        Some(n) => __builtin_string_len(n)\n        None => 0\n    }\n    n + s + m\n}\n";
    assert_eq!(run(src), Value::Int(5));
}

#[test]
fn break_and_continue_in_while_and_for() {
    let src = "fn main() -> Int {\n    let mut i: Int = 0\n    let mut sum: Int = 0\n    while true {\n        i = i + 1\n        if i > 10 {\n            break\n        }\n        if i == 3 {\n            continue\n        }\n        sum = sum + i\n    }\n    let mut acc: Int = 0\n    for x in [1, 2, 3, 4, 5, 6, 7] {\n        if x == 6 {\n            break\n        }\n        if x == 2 {\n            continue\n        } else {\n            acc = acc + x\n        }\n    }\n    sum * 100 + acc\n}\n";
    // while: 1..10 without 3 = 52; for: 1 + 3 + 4 + 5 = 13.
    assert_eq!(run(src), Value::Int(5213));
}

#[test]
fn break_leaves_only_the_innermost_loop() {
    let src = "fn main() -> Int {\n    let mut total: Int = 0\n    for a in [1, 2, 3] {\n        for b in [10, 20, 30] {\n            if b == 20 {\n                break\n            }\n            total = total + a * b\n        }\n    }\n    total\n}\n";
    assert_eq!(run(src), Value::Int(60));
}

#[test]
fn continue_many_times_does_not_grow_the_stack() {
    let src = "fn main() -> Int {\n    let mut i: Int = 0\n    let mut n: Int = 0\n    while i < 5000 {\n        i = i + 1\n        if i > 0 {\n            continue\n        }\n        n = n + 1\n    }\n    i + n\n}\n";
    assert_eq!(run(src), Value::Int(5000));
}

#[test]
fn break_outside_a_loop_or_inside_an_expression_is_rejected() {
    let err = rejected("fn main() -> Int {\n    break\n    0\n}\n");
    assert!(err.contains("`break` outside of a loop"), "{err}");
    let err = rejected("fn main() -> Int {\n    let mut i: Int = 0\n    while i < 3 {\n        i = i + match i {\n            1 => {\n                continue\n            }\n            _ => 1\n        }\n    }\n    i\n}\n");
    assert!(err.contains("`continue` must be a statement"), "{err}");
}

#[test]
fn if_without_else_as_a_statement_runs_on_both_paths() {
    // Used to fail with "stack underflow" when the condition was false.
    let src = "fn f(c: Bool) -> Int {\n    let a: Int = 7\n    if c {\n        a + 1\n    }\n    a\n}\nfn main() -> Int {\n    f(false) * 10 + f(true)\n}\n";
    assert_eq!(run(src), Value::Int(77));
    let src = "fn main() -> Int {\n    let mut i: Int = 0\n    let mut n: Int = 100\n    while i < 5 {\n        if i == 2 {\n            n + 1\n        }\n        i = i + 1\n    }\n    n + i\n}\n";
    assert_eq!(run(src), Value::Int(105));
}

#[test]
fn break_and_continue_are_keywords() {
    let err = compile(
        "test",
        "fn main() -> Int {\n    let break: Int = 1\n    0\n}\n",
    )
    .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("break"), "{err}");
}

#[test]
fn names_bound_in_a_match_arm_or_block_end_there() {
    // These used to leak: the arm's `n` (and the block's `n`) replaced the outer one for the
    // rest of the function.
    let src = "fn main() -> Int {\n    let n: Int = 1\n    let o: Option<Int> = Some(40)\n    let m: Int = match o {\n        Some(n) => n\n        None => 0\n    }\n    n + m\n}\n";
    assert_eq!(run(src), Value::Int(41));
    let src = "fn main() -> Int {\n    let n: Int = 1\n    if true {\n        let n: Int = 50\n        n\n    }\n    n\n}\n";
    assert_eq!(run(src), Value::Int(1));
    let src = "fn main() -> Int {\n    let x: Int = 3\n    let mut total: Int = 0\n    for x in [10, 20] {\n        total = total + x\n    }\n    total + x\n}\n";
    assert_eq!(run(src), Value::Int(33));
}

#[test]
fn assignment_inside_a_block_still_updates_the_outer_binding() {
    let src = "fn main() -> Int {\n    let mut n: Int = 1\n    if true {\n        n = 5\n    }\n    let o: Option<Int> = Some(2)\n    match o {\n        Some(v) => {\n            n = n + v\n        }\n        None => {}\n    }\n    n\n}\n";
    assert_eq!(run(src), Value::Int(7));
}

#[test]
fn a_function_parameter_shadows_a_top_level_function_of_the_same_name() {
    // 3.x called the top-level `step` (returned 100); the parameter must win. A different
    // signature for the top-level function must not cause a false E009 either.
    let src = "fn step(x: String) -> Int {\n    100\n}\nfn plus_one(x: Int) -> Int {\n    x + 1\n}\nfn apply(step: Fn(Int) -> Int, v: Int) -> Int {\n    step(v)\n}\nfn main() -> Int {\n    apply(plus_one, 1)\n}\n";
    assert_eq!(run(src), Value::Int(2));
}

#[test]
fn a_local_that_is_not_a_function_does_not_hide_a_top_level_function() {
    // Same results as 3.x: only a local declared with a `Fn` type takes over the call.
    let src = "fn size(x: Int) -> Int {\n    x * 2\n}\nfn main() -> Int {\n    let size: Int = 3\n    size(size)\n}\n";
    assert_eq!(run(src), Value::Int(6));
    let src = "fn item(n: Int) -> Int {\n    n * 3\n}\nfn main() -> Int {\n    let mut s: Int = 0\n    for item in [1, 2] {\n        s = s + item(item)\n    }\n    s\n}\n";
    assert_eq!(run(src), Value::Int(9));
    let src = "fn v(n: Int) -> Int {\n    n + 1\n}\nfn main() -> Int {\n    let o: Option<Int> = Some(4)\n    match o {\n        Some(v) => v(v)\n        None => 0\n    }\n}\n";
    assert_eq!(run(src), Value::Int(5));
}

#[test]
fn a_call_that_reaches_the_top_level_function_through_a_local_name_checks_its_arity() {
    // The type checker skips arity for a name that is also a local; the call still goes to the
    // top-level `size`, so a wrong argument count must not compile (it used to drop the 99).
    let err = rejected("fn size(x: Int) -> Int {\n    x * 2\n}\nfn main() -> Int {\n    let size: Int = 3\n    size(size, 99)\n}\n");
    assert!(
        err.contains("function 'size' expects 1 argument, got 2"),
        "{err}"
    );
}

#[test]
fn a_function_parameter_shadows_a_capability_builtin_of_the_same_name() {
    // The parameter must be called, not the `random_int` built-in.
    let src = "fn seven(a: Int, b: Int) -> Int {\n    7\n}\nfn apply(random_int: Fn(Int, Int) -> Int) -> Int !{random} {\n    random_int(1, 1000000)\n}\nfn main() -> Int {\n    apply(seven)\n}\n";
    assert_eq!(run(src), Value::Int(7));
}

#[test]
fn an_untyped_local_shadows_a_capability_builtin_of_the_same_name() {
    // A function value bound by `let` or a match pattern, without a type, is called too.
    let let_src = "fn seven(a: Int, b: Int) -> Int {\n    7\n}\nfn main() -> Int !{random} {\n    let random_int = seven\n    random_int(1, 1000000)\n}\n";
    assert_eq!(run(let_src), Value::Int(7));
    let match_src = "fn seven(a: Int, b: Int) -> Int {\n    7\n}\nfn main() -> Int !{random} {\n    match Some(seven) {\n        Some(random_int) => random_int(1, 1000000),\n        _ => 0,\n    }\n}\n";
    assert_eq!(run(match_src), Value::Int(7));
}

#[test]
fn plus_concatenates_two_strings() {
    // Spec: `+` on two Strings yields a String (4.0 accepted it, then failed at run time).
    let src = "fn main() -> String {\n    let who = \"world\"\n    \"hello, \" + who + \"!\"\n}\n";
    assert_eq!(run(src), Value::String("hello, world!".into()));
}
