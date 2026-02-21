#[cfg(test)]
mod tests {
    use crate::lexer;
    use crate::parser;
    use crate::typeck;
    use crate::codegen;
    use crate::compile;
    use crate::ast::*;
    use boruna_bytecode::Value;
    use boruna_vm::vm::Vm;
    use boruna_vm::capability_gateway::{CapabilityGateway, Policy};

    fn run_source(source: &str) -> Value {
        let module = compile("test", source).expect("compilation failed");
        let gateway = CapabilityGateway::new(Policy::allow_all());
        let mut vm = Vm::new(module, gateway);
        vm.run().expect("runtime error")
    }

    // --- Lexer Tests ---

    #[test]
    fn test_lex_basic() {
        let tokens = lexer::lex("fn main() { 42 }").unwrap();
        assert!(tokens.iter().any(|t| matches!(t.kind, lexer::TokenKind::Fn)));
        assert!(tokens.iter().any(|t| matches!(t.kind, lexer::TokenKind::IntLit(42))));
    }

    #[test]
    fn test_lex_string() {
        let tokens = lexer::lex(r#""hello world""#).unwrap();
        assert!(tokens.iter().any(|t| matches!(&t.kind, lexer::TokenKind::StringLit(s) if s == "hello world")));
    }

    #[test]
    fn test_lex_operators() {
        let tokens = lexer::lex("+ - * / == != <= >= && || ++ =>").unwrap();
        assert!(tokens.len() >= 11);
    }

    #[test]
    fn test_lex_comments() {
        let tokens = lexer::lex("42 // this is a comment\n43").unwrap();
        let ints: Vec<_> = tokens.iter().filter(|t| matches!(t.kind, lexer::TokenKind::IntLit(_))).collect();
        assert_eq!(ints.len(), 2);
    }

    // --- Parser Tests ---

    #[test]
    fn test_parse_simple_function() {
        let tokens = lexer::lex("fn main() -> Int { 42 }").unwrap();
        let program = parser::parse(tokens).unwrap();
        assert_eq!(program.items.len(), 1);
        if let Item::Function(f) = &program.items[0] {
            assert_eq!(f.name, "main");
            assert_eq!(f.params.len(), 0);
        } else {
            panic!("expected function");
        }
    }

    #[test]
    fn test_parse_function_with_params() {
        let tokens = lexer::lex("fn add(a: Int, b: Int) -> Int { a + b }").unwrap();
        let program = parser::parse(tokens).unwrap();
        if let Item::Function(f) = &program.items[0] {
            assert_eq!(f.params.len(), 2);
            assert_eq!(f.params[0].name, "a");
            assert_eq!(f.params[1].name, "b");
        } else {
            panic!("expected function");
        }
    }

    #[test]
    fn test_parse_capabilities() {
        let tokens = lexer::lex("fn fetch() !{net} { 0 }").unwrap();
        let program = parser::parse(tokens).unwrap();
        if let Item::Function(f) = &program.items[0] {
            assert_eq!(f.capabilities, vec!["net"]);
        } else {
            panic!("expected function");
        }
    }

    #[test]
    fn test_parse_type_def() {
        let tokens = lexer::lex("type User { name: String, age: Int }").unwrap();
        let program = parser::parse(tokens).unwrap();
        if let Item::TypeDef(t) = &program.items[0] {
            assert_eq!(t.name, "User");
            if let TypeDefKind::Record(fields) = &t.kind {
                assert_eq!(fields.len(), 2);
            } else {
                panic!("expected record");
            }
        } else {
            panic!("expected type def");
        }
    }

    #[test]
    fn test_parse_if_else() {
        let tokens = lexer::lex("fn main() { if true { 1 } else { 2 } }").unwrap();
        let program = parser::parse(tokens).unwrap();
        assert_eq!(program.items.len(), 1);
    }

    #[test]
    fn test_parse_match() {
        let source = r#"
fn main() {
    match 42 {
        0 => "zero",
        _ => "other",
    }
}
"#;
        let tokens = lexer::lex(source).unwrap();
        let program = parser::parse(tokens).unwrap();
        assert_eq!(program.items.len(), 1);
    }

    #[test]
    fn test_parse_while() {
        let source = "fn main() { let mut x: Int = 0\n while x < 10 { x = x + 1 } }";
        let tokens = lexer::lex(source).unwrap();
        let program = parser::parse(tokens).unwrap();
        assert_eq!(program.items.len(), 1);
    }

    // --- Type Checker Tests ---

    #[test]
    fn test_typeck_undefined_variable() {
        let source = "fn main() { x }";
        let tokens = lexer::lex(source).unwrap();
        let program = parser::parse(tokens).unwrap();
        assert!(typeck::check(&program).is_err());
    }

    #[test]
    fn test_typeck_valid_let() {
        let source = "fn main() { let x: Int = 42\n x }";
        let tokens = lexer::lex(source).unwrap();
        let program = parser::parse(tokens).unwrap();
        assert!(typeck::check(&program).is_ok());
    }

    #[test]
    fn test_typeck_function_reference() {
        let source = "fn helper() -> Int { 1 }\nfn main() { helper() }";
        let tokens = lexer::lex(source).unwrap();
        let program = parser::parse(tokens).unwrap();
        assert!(typeck::check(&program).is_ok());
    }

    // --- Codegen Tests ---

    #[test]
    fn test_codegen_simple() {
        let source = "fn main() -> Int { 42 }";
        let tokens = lexer::lex(source).unwrap();
        let program = parser::parse(tokens).unwrap();
        typeck::check(&program).unwrap();
        let module = codegen::emit("test", &program).unwrap();
        assert_eq!(module.functions.len(), 1);
        assert_eq!(module.functions[0].name, "main");
    }

    // --- End-to-End Tests ---

    #[test]
    fn test_e2e_integer_literal() {
        assert_eq!(run_source("fn main() -> Int { 42 }"), Value::Int(42));
    }

    #[test]
    fn test_e2e_arithmetic() {
        assert_eq!(run_source("fn main() -> Int { 2 + 3 * 4 }"), Value::Int(14));
    }

    #[test]
    fn test_e2e_string() {
        assert_eq!(
            run_source(r#"fn main() -> String { "hello" ++ " world" }"#),
            Value::String("hello world".into()),
        );
    }

    #[test]
    fn test_e2e_boolean() {
        assert_eq!(run_source("fn main() -> Bool { 1 < 2 }"), Value::Bool(true));
        assert_eq!(run_source("fn main() -> Bool { 1 > 2 }"), Value::Bool(false));
    }

    #[test]
    fn test_e2e_if_then() {
        assert_eq!(
            run_source("fn main() -> Int { if true { 1 } else { 2 } }"),
            Value::Int(1),
        );
        assert_eq!(
            run_source("fn main() -> Int { if false { 1 } else { 2 } }"),
            Value::Int(2),
        );
    }

    #[test]
    fn test_e2e_let_binding() {
        assert_eq!(
            run_source("fn main() -> Int { let x: Int = 10\n let y: Int = 20\n x + y }"),
            Value::Int(30),
        );
    }

    #[test]
    fn test_e2e_function_call() {
        assert_eq!(
            run_source("fn double(n: Int) -> Int { n * 2 }\nfn main() -> Int { double(21) }"),
            Value::Int(42),
        );
    }

    #[test]
    fn test_e2e_recursion() {
        assert_eq!(
            run_source(r#"
fn fib(n: Int) -> Int {
    if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
}
fn main() -> Int { fib(10) }
"#),
            Value::Int(55),
        );
    }

    #[test]
    fn test_e2e_while_loop() {
        assert_eq!(
            run_source(r#"
fn main() -> Int {
    let mut sum: Int = 0
    let mut i: Int = 1
    while i <= 10 {
        sum = sum + i
        i = i + 1
    }
    sum
}
"#),
            Value::Int(55),
        );
    }

    #[test]
    fn test_e2e_record() {
        assert_eq!(
            run_source(r#"
type Point { x: Int, y: Int }
fn main() -> Int {
    let p: Point = Point { x: 3, y: 4 }
    p.x + p.y
}
"#),
            Value::Int(7),
        );
    }

    #[test]
    fn test_e2e_nested_calls() {
        assert_eq!(
            run_source(r#"
fn inc(n: Int) -> Int { n + 1 }
fn double(n: Int) -> Int { n * 2 }
fn main() -> Int { double(inc(5)) }
"#),
            Value::Int(12),
        );
    }

    #[test]
    fn test_e2e_comparison_chain() {
        assert_eq!(
            run_source("fn main() -> Bool { 1 < 2 && 3 > 2 }"),
            Value::Bool(true),
        );
    }

    #[test]
    fn test_e2e_negation() {
        assert_eq!(run_source("fn main() -> Int { -42 }"), Value::Int(-42));
    }

    #[test]
    fn test_e2e_not() {
        assert_eq!(run_source("fn main() -> Bool { !true }"), Value::Bool(false));
    }

    // ── List builtin E2E tests ──

    #[test]
    fn test_e2e_list_literal() {
        assert_eq!(
            run_source("fn main() -> Int { let xs: List<Int> = [10, 20, 30]\n list_len(xs) }"),
            Value::Int(3),
        );
    }

    #[test]
    fn test_e2e_list_get() {
        assert_eq!(
            run_source("fn main() -> Int { let xs: List<Int> = [10, 20, 30]\n list_get(xs, 1) }"),
            Value::Int(20),
        );
    }

    #[test]
    fn test_e2e_list_push() {
        assert_eq!(
            run_source("fn main() -> Int { let xs: List<Int> = [1, 2]\n let ys: List<Int> = list_push(xs, 3)\n list_len(ys) }"),
            Value::Int(3),
        );
    }

    #[test]
    fn test_e2e_list_push_preserves_original() {
        // list_push returns new list, original unchanged
        assert_eq!(
            run_source("fn main() -> Int { let xs: List<Int> = [1, 2]\n let ys: List<Int> = list_push(xs, 3)\n list_len(xs) }"),
            Value::Int(2),
        );
    }

    #[test]
    fn test_e2e_empty_list() {
        assert_eq!(
            run_source("fn main() -> Int { let xs: List<Int> = []\n list_len(xs) }"),
            Value::Int(0),
        );
    }

    #[test]
    fn test_e2e_list_push_then_get() {
        assert_eq!(
            run_source("fn main() -> Int { let xs: List<Int> = []\n let ys: List<Int> = list_push(xs, 42)\n list_get(ys, 0) }"),
            Value::Int(42),
        );
    }

    // ── String builtin E2E tests ──

    #[test]
    fn test_e2e_parse_int() {
        assert_eq!(
            run_source(r#"fn main() -> Int { parse_int("42") }"#),
            Value::Int(42),
        );
    }

    #[test]
    fn test_e2e_parse_int_invalid() {
        assert_eq!(
            run_source(r#"fn main() -> Int { parse_int("abc") }"#),
            Value::Int(0),
        );
    }

    #[test]
    fn test_e2e_try_parse_int_valid() {
        let result = run_source(r#"
            fn main() -> Int {
                match try_parse_int("42") {
                    Ok(n) => n,
                    Err(_) => -1,
                }
            }
        "#);
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn test_e2e_try_parse_int_invalid() {
        let result = run_source(r#"
            fn main() -> Int {
                match try_parse_int("abc") {
                    Ok(n) => n,
                    Err(_) => -1,
                }
            }
        "#);
        assert_eq!(result, Value::Int(-1));
    }

    #[test]
    fn test_e2e_str_contains() {
        assert_eq!(
            run_source(r#"fn main() -> Bool { str_contains("hello world", "world") }"#),
            Value::Bool(true),
        );
    }

    #[test]
    fn test_e2e_str_contains_false() {
        assert_eq!(
            run_source(r#"fn main() -> Bool { str_contains("hello", "xyz") }"#),
            Value::Bool(false),
        );
    }

    #[test]
    fn test_e2e_str_starts_with() {
        assert_eq!(
            run_source(r#"fn main() -> Bool { str_starts_with("conflict:5", "conflict") }"#),
            Value::Bool(true),
        );
    }

    #[test]
    fn test_e2e_str_starts_with_false() {
        assert_eq!(
            run_source(r#"fn main() -> Bool { str_starts_with("ok", "conflict") }"#),
            Value::Bool(false),
        );
    }

    // ── Record spread E2E tests ──

    #[test]
    fn test_e2e_record_spread_basic() {
        let src = r#"
type Pt { x: Int, y: Int }
fn main() -> Int {
    let p: Pt = Pt { x: 1, y: 2 }
    let q: Pt = Pt { ..p, x: 10 }
    q.x
}
"#;
        assert_eq!(run_source(src), Value::Int(10));
    }

    #[test]
    fn test_e2e_record_spread_preserves_unset() {
        let src = r#"
type Pt { x: Int, y: Int }
fn main() -> Int {
    let p: Pt = Pt { x: 1, y: 2 }
    let q: Pt = Pt { ..p, x: 10 }
    q.y
}
"#;
        assert_eq!(run_source(src), Value::Int(2));
    }

    #[test]
    fn test_e2e_record_spread_all_fields() {
        // Spread with all fields overridden is valid
        let src = r#"
type Pt { x: Int, y: Int }
fn main() -> Int {
    let p: Pt = Pt { x: 1, y: 2 }
    let q: Pt = Pt { ..p, x: 10, y: 20 }
    q.x + q.y
}
"#;
        assert_eq!(run_source(src), Value::Int(30));
    }

    #[test]
    fn test_e2e_record_spread_no_overrides() {
        // Spread with no overrides is effectively a clone
        let src = r#"
type Pt { x: Int, y: Int }
fn main() -> Int {
    let p: Pt = Pt { x: 5, y: 7 }
    let q: Pt = Pt { ..p }
    q.x + q.y
}
"#;
        assert_eq!(run_source(src), Value::Int(12));
    }

    #[test]
    fn test_e2e_record_spread_many_fields() {
        // Test with a larger record (State-like)
        let src = r#"
type State { a: Int, b: Int, c: Int, d: String, e: Int }
fn main() -> Int {
    let s: State = State { a: 1, b: 2, c: 3, d: "hello", e: 5 }
    let s2: State = State { ..s, c: 99 }
    s2.a + s2.b + s2.c + s2.e
}
"#;
        assert_eq!(run_source(src), Value::Int(1 + 2 + 99 + 5));
    }

    // ── String match E2E tests ──

    #[test]
    fn test_e2e_string_match_basic() {
        let src = r#"
fn main() -> Int {
    let tag: String = "add"
    match tag {
        "add" => 1,
        "sub" => 2,
        _ => 0,
    }
}
"#;
        assert_eq!(run_source(src), Value::Int(1));
    }

    #[test]
    fn test_e2e_string_match_second_arm() {
        let src = r#"
fn main() -> Int {
    let tag: String = "sub"
    match tag {
        "add" => 1,
        "sub" => 2,
        _ => 0,
    }
}
"#;
        assert_eq!(run_source(src), Value::Int(2));
    }

    #[test]
    fn test_e2e_string_match_wildcard() {
        let src = r#"
fn main() -> Int {
    let tag: String = "unknown"
    match tag {
        "add" => 1,
        "sub" => 2,
        _ => 0,
    }
}
"#;
        assert_eq!(run_source(src), Value::Int(0));
    }

    #[test]
    fn test_e2e_string_match_many_arms() {
        let src = r#"
fn main() -> Int {
    let tag: String = "delete"
    match tag {
        "create" => 1,
        "read" => 2,
        "update" => 3,
        "delete" => 4,
    }
}
"#;
        assert_eq!(run_source(src), Value::Int(4));
    }

    #[test]
    fn test_e2e_string_match_with_field_access() {
        let src = r#"
type Msg { tag: String, payload: Int }
fn main() -> Int {
    let m: Msg = Msg { tag: "add", payload: 42 }
    match m.tag {
        "add" => m.payload,
        "sub" => 0 - m.payload,
        _ => 0,
    }
}
"#;
        assert_eq!(run_source(src), Value::Int(42));
    }
}
