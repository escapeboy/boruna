//! E010: reassigning a binding declared without `mut` (or a parameter or `for` variable).
//! The compiler accepts it today, so it is a warning with a `lang repair` fix.

use boruna_tooling::diagnostics::collector::DiagnosticCollector;
use boruna_tooling::diagnostics::{Confidence, Diagnostic, Severity, E010_ASSIGN_IMMUTABLE};
use boruna_tooling::repair::{RepairStrategy, RepairTool};

fn e010(source: &str) -> Vec<Diagnostic> {
    let all = DiagnosticCollector::new("t.ax", source)
        .collect()
        .diagnostics;
    // A program that does not compile may skip the analyzer, which would make an
    // "is empty" assertion pass for the wrong reason.
    let errors: Vec<_> = all
        .iter()
        .filter(|d| d.severity == Severity::Error)
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
    assert_eq!(d[0].severity, Severity::Warning);
    assert_eq!(d[0].location.as_ref().unwrap().line, 2);
    assert_eq!(d[0].suggested_patches[0].confidence, Confidence::High);

    let set = DiagnosticCollector::new("t.ax", src).collect();
    let (fixed, result) = RepairTool::repair("t.ax", src, &set, RepairStrategy::Best, None);
    assert!(result.verify_passed);
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
    assert!(d.iter().any(|d| d.message.starts_with("parameter 'n'")));
    assert!(d.iter().any(|d| d.message.starts_with("loop variable 'v'")));
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
