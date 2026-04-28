//! Snapshot tests for parser/typeck typo suggestions (post1-T-1.5).
//!
//! Six fixture cases (3 keyword typos in the parser, 3 identifier
//! typos in typeck) snapshot-tested via `insta`. Each fixture
//! compiles a small `.ax` source string and snapshots the rendered
//! `CompileError` (the same string `boruna run` would print on
//! stderr).
//!
//! Update snapshots after intentional message changes with
//! `INSTA_UPDATE=always cargo test -p boruna-compiler`.

use boruna_compiler::compile;

fn render_error(source: &str) -> String {
    match compile("test", source) {
        Ok(_) => "(no error — fixture should fail to compile)".to_string(),
        Err(e) => format!("{e}"),
    }
}

// Keyword typos — distance-1 from a known keyword, parser-detected.

#[test]
fn keyword_fnn_for_fn() {
    // `fnn` is distance 1 from `fn` (extra trailing 'n').
    // Detected at top-level item dispatch.
    insta::assert_snapshot!(render_error("fnn main() -> Int {\n    1\n}\n"));
}

#[test]
fn keyword_lett_for_let() {
    // `lett` is distance 1 from `let`, but at this position the
    // parser is happy to accept any expression-starting identifier,
    // so the typo is consumed as an Ident expression and the
    // failure surfaces later at the type annotation. Documents the
    // boundary: keyword-typo suggestions kick in only at sites
    // where a specific keyword (or item) is expected.
    insta::assert_snapshot!(render_error(
        "fn main() -> Int {\n    lett x: Int = 1\n    x\n}\n"
    ));
}

#[test]
fn keyword_matc_for_match() {
    // Same boundary as `keyword_lett_for_let` — `matc` is
    // statement-position so no specific-keyword expectation fires.
    insta::assert_snapshot!(render_error(
        "fn main() -> Int {\n    matc x { _ => 0 }\n}\n"
    ));
}

// Identifier typos — typeck-detected, suggesting from in-scope
// locals + functions.

#[test]
fn ident_typo_local() {
    // `value_one` is in scope; `value_ono` is one substitution
    // away (e → o).
    insta::assert_snapshot!(render_error(
        "fn main() -> Int {\n    let value_one: Int = 1\n    value_ono\n}\n"
    ));
}

#[test]
fn ident_typo_function() {
    // `helper` is a function; `helpr` is one deletion away.
    insta::assert_snapshot!(render_error(
        "fn helper(x: Int) -> Int { x + 1 }\nfn main() -> Int {\n    helpr(2)\n}\n"
    ));
}

#[test]
fn ident_no_suggestion_when_distant() {
    // `xyz` is not within distance 1 of any in-scope name.
    insta::assert_snapshot!(render_error(
        "fn main() -> Int {\n    let value_one: Int = 1\n    xyz\n}\n"
    ));
}
