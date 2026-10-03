//! E010: reassigning a binding declared without `mut` (or a parameter or `for` variable).
//! Language 2.0: the compiler rejects it, and `lang check` reports it as an error with a
//! `lang repair` fix where the fix is safe.

use boruna_tooling::diagnostics::collector::DiagnosticCollector;
use boruna_tooling::diagnostics::{Confidence, Diagnostic, Severity, E010_ASSIGN_IMMUTABLE};
use boruna_tooling::repair::{RepairStrategy, RepairTool};

fn e010(source: &str) -> Vec<Diagnostic> {
    let all = DiagnosticCollector::new("t.ax", source)
        .collect()
        .diagnostics;
    // A program that fails for another reason may skip the analyzer, which would make an
    // "is empty" assertion pass for the wrong reason.
    let errors: Vec<_> = all
        .iter()
        .filter(|d| d.severity == Severity::Error && d.id != E010_ASSIGN_IMMUTABLE)
        .collect();
    assert!(errors.is_empty(), "test program must compile: {errors:?}");
    all.into_iter()
        .filter(|d| d.id == E010_ASSIGN_IMMUTABLE)
        .collect()
}

#[test]
fn reassigned_let_warns_with_a_high_confidence_fix_that_repair_applies() {
    let src = "fn main() -> Int {\n    let x: Int = 1\n    x = 2\n    x\n}\n";
    let d = e010(src);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].severity, Severity::Error);
    assert_eq!(d[0].location.as_ref().unwrap().line, 2);
    assert_eq!(d[0].suggested_patches[0].confidence, Confidence::High);
    let err = boruna_compiler::compile("t", src).unwrap_err().to_string();
    assert!(err.contains("cannot reassign 'x'"), "{err}");

    let set = DiagnosticCollector::new("t.ax", src).collect();
    let (fixed, result) = RepairTool::repair("t.ax", src, &set, RepairStrategy::Best, None);
    assert!(result.verify_passed);
    assert!(boruna_compiler::compile("t", &fixed).is_ok());
    assert!(fixed.contains("    let mut x: Int = 1\n"));
    assert!(e010(&fixed).is_empty());
}

#[test]
fn mut_binding_does_not_warn() {
    let src = "fn main() -> Int {\n    let mut x: Int = 1\n    x = 2\n    x\n}\n";
    assert!(e010(src).is_empty());
}

#[test]
fn reassignment_inside_loops_and_branches_is_found() {
    let src = "fn main() -> Int {\n    let i: Int = 0\n    let t: Int = 0\n    while i < 3 {\n        if i == 1 {\n            t = t + 1\n        }\n        i = i + 1\n    }\n    t\n}\n";
    let mut names: Vec<String> = e010(src)
        .iter()
        .map(|d| d.message.split('\'').nth(1).unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["i", "t"]);
}

#[test]
fn one_warning_per_variable_even_when_assigned_twice() {
    let src = "fn main() -> Int {\n    let x: Int = 1\n    x = 2\n    x = 3\n    x\n}\n";
    assert_eq!(e010(src).len(), 1);
}

#[test]
fn parameters_and_loop_variables_warn_without_an_automatic_fix() {
    let src = "fn f(n: Int) -> Int {\n    n = n + 1\n    n\n}\nfn main() -> Int {\n    let mut s: Int = 0\n    for v in [1, 2] {\n        v = v + 1\n        s = s + v\n    }\n    f(s)\n}\n";
    let d = e010(src);
    assert_eq!(d.len(), 2);
    assert!(d
        .iter()
        .any(|d| d.message.starts_with("cannot reassign parameter 'n'")));
    assert!(d
        .iter()
        .any(|d| d.message.starts_with("cannot reassign loop variable 'v'")));
    assert!(d.iter().all(|d| d.suggested_patches.is_empty()));
}

#[test]
fn a_name_bound_by_a_match_pattern_is_not_reported() {
    let src = "fn main() -> Int {\n    let mut t: Int = 0\n    let x: Int = 1\n    match Some(5) {\n        Some(x) => {\n            x = x + 1\n            t = x\n        },\n        None => {\n            t = 0\n        },\n    }\n    t\n}\n";
    assert!(e010(src).is_empty());
}

#[test]
fn the_fix_targets_the_let_in_the_right_function() {
    let src = "fn a() -> Int {\n    let i: Int = 0\n    i\n}\nfn b() -> Int {\n    let i: Int = 0\n    i = 1\n    i\n}\nfn main() -> Int {\n    a() + b()\n}\n";
    let d = e010(src);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].location.as_ref().unwrap().line, 6);
    assert_eq!(d[0].suggested_patches[0].edits[0].start_line, 6);
}

#[test]
fn a_shadowed_name_gets_a_warning_but_no_guessed_fix() {
    let src = "fn main() -> Int {\n    let x: Int = 1\n    let x: Int = 2\n    x = 3\n    x\n}\n";
    let d = e010(src);
    assert_eq!(d.len(), 1);
    assert!(d[0].suggested_patches.is_empty());
}

#[test]
fn findings_point_at_the_offending_line() {
    use boruna_tooling::diagnostics::E009_TYPE_ERROR;
    let src = "fn g(s: Int) -> Int {\n    s\n}\nfn main() -> Int {\n    let mut z: Int = 1\n    let y: Int = 2\n    z = \"t\"\n    let q: Int = g(\"q\")\n    while 3 {\n    }\n    for v in [1] {\n        v = 2\n    }\n    0\n}\n";
    let lines: Vec<(String, usize)> = DiagnosticCollector::new("t.ax", src)
        .collect()
        .diagnostics
        .into_iter()
        .filter(|d| d.id == E009_TYPE_ERROR || d.id == E010_ASSIGN_IMMUTABLE)
        .map(|d| (d.id, d.location.unwrap().line))
        .collect();
    assert_eq!(
        lines,
        [
            ("E009".to_string(), 7),
            ("E009".to_string(), 8),
            ("E009".to_string(), 9),
            ("E010".to_string(), 12)
        ]
    );
}

#[test]
fn a_repeated_call_or_a_same_line_loop_assignment_points_at_the_right_line() {
    use boruna_tooling::diagnostics::E009_TYPE_ERROR;
    let src = "fn g(a: Int) -> Int {\n    a\n}\nfn main() -> Int {\n    let ok: Int = g(1)\n    let bad: Int = g(\"x\")\n    for i in [1] { i = 3 }\n    ok\n}\n";
    let lines: Vec<(String, usize)> = DiagnosticCollector::new("t.ax", src)
        .collect()
        .diagnostics
        .into_iter()
        .filter(|d| d.id == E009_TYPE_ERROR || d.id == E010_ASSIGN_IMMUTABLE)
        .map(|d| (d.id, d.location.unwrap().line))
        .collect();
    assert_eq!(lines, [("E009".to_string(), 6), ("E010".to_string(), 7)]);
}
