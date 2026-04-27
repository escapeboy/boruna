//! Integration tests for `boruna_tooling::format`.
//!
//! - Golden fixtures: hand-crafted ugly inputs paired with expected output.
//! - Idempotency roundtrip: every comment-free `.ax` example produces a
//!   stable fixed-point under `format_source`.
//! - Parse-failure error case: a malformed program returns
//!   `FormatError::ParseFailed`.

use std::fs;
use std::path::{Path, PathBuf};

use boruna_tooling::format::{check_source, format_source, FormatError};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("format_fixtures")
}

fn golden(name: &str) {
    let dir = fixtures_dir();
    let input = fs::read_to_string(dir.join(format!("{name}.input.ax"))).unwrap();
    let expected = fs::read_to_string(dir.join(format!("{name}.expected.ax"))).unwrap();
    let actual = format_source(&input).unwrap();
    assert_eq!(
        actual, expected,
        "golden mismatch for {name}\n--- expected ---\n{expected}\n--- actual ---\n{actual}"
    );
    // The expected output should be its own fixed point.
    assert!(
        check_source(&expected).unwrap(),
        "expected output for {name} is not idempotent"
    );
}

#[test]
fn golden_hello() {
    golden("hello");
}

#[test]
fn golden_record_and_match() {
    golden("record_and_match");
}

#[test]
fn golden_capabilities() {
    golden("capabilities");
}

#[test]
fn idempotency_roundtrip_on_examples() {
    // Walk a representative set of comment-free programs synthesized from
    // the example inputs (we strip comments first since v1 of the
    // formatter does not preserve them — see module docs).
    let programs: &[&str] = &[
        "fn main() -> Int { 0 }",
        "fn add(a: Int, b: Int) -> Int { a + b }\nfn main() -> Int { add(1, 2) }",
        "type Point { x: Int, y: Int }\nfn main() -> Int {\nlet p: Point = Point { x: 1, y: 2 }\np.x\n}",
        "fn fact(n: Int) -> Int { if n == 0 { 1 } else { n * fact(n - 1) } }\nfn main() -> Int { fact(5) }",
        "enum Color { Red, Green, Blue }\nfn main() -> Int { 0 }",
        "fn main() -> Int {\nlet xs: List<Int> = [1, 2, 3]\n0\n}",
        "fn classify(n: Int) -> String {\nmatch n { 0 => \"zero\", _ => \"other\" }\n}\nfn main() -> Int { 0 }",
    ];
    for src in programs {
        let once = format_source(src).expect("first format");
        let twice = format_source(&once).expect("second format");
        assert_eq!(once, twice, "format(format(x)) != format(x) for: {src}");
    }
}

#[test]
fn parse_failure_returns_typed_error() {
    let bad = "fn main( -> Int { 0 }";
    let err = format_source(bad).unwrap_err();
    match err {
        FormatError::ParseFailed { line, message, .. } => {
            assert!(line > 0, "expected nonzero line, got {line}");
            assert!(!message.is_empty());
        }
    }
}

#[test]
fn check_distinguishes_canonical_from_unformatted() {
    let unformatted = "fn main() -> Int {\nlet x: Int = 1\nx\n}\n";
    assert!(!check_source(unformatted).unwrap());

    let canonical = format_source(unformatted).unwrap();
    assert!(check_source(&canonical).unwrap());
}
