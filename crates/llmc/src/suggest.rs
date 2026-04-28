//! Levenshtein-distance-1 typo suggestions for parser and typeck
//! diagnostics (post1-T-1.5).
//!
//! Suggestions are appended to error messages only when a single
//! candidate within distance 1 exists. Multiple equally-close
//! candidates would produce noise (`did you mean: 'fn' or 'in'?`)
//! that operators rightly ignore — silence is the right behavior.

use strsim::levenshtein;

/// Every keyword the lexer recognises. Kept in sync with the
/// `#[token("...")]` annotations on `TokenKind`. A drift test in
/// `tests.rs` asserts every TokenKind keyword variant has its
/// canonical spelling here.
pub const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "if", "else", "match", "return", "type", "enum", "module", "import",
    "export", "true", "false", "None", "Some", "Ok", "Err", "requires", "ensures", "spawn", "send",
    "receive", "emit", "while", "for", "in",
];

/// Returns the unique keyword within Levenshtein distance ≤ 1 of
/// `typo`, or `None` if there is no candidate or more than one tie.
pub fn keyword_suggestion(typo: &str) -> Option<&'static str> {
    suggest_unique_within_1(typo, KEYWORDS.iter().copied())
        .map(|s| KEYWORDS.iter().copied().find(|kw| *kw == s).unwrap())
}

/// Returns the unique candidate from `candidates` within Levenshtein
/// distance ≤ 1 of `typo`. Returns `None` if there is no candidate
/// or more than one equally-close candidate.
///
/// `candidates` is consumed once; pass an iterator over `&str`
/// borrowed from a long-lived buffer (e.g. a HashSet of locals).
pub fn suggestion_from<'a, I>(typo: &str, candidates: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    suggest_unique_within_1(typo, candidates)
}

fn suggest_unique_within_1<'a, I>(typo: &str, candidates: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut best: Option<&str> = None;
    let mut tied = false;
    for cand in candidates {
        if cand == typo {
            // Exact match — not a suggestion case. Skip; the caller
            // shouldn't be asking for a suggestion when the typo IS
            // the candidate.
            continue;
        }
        let d = levenshtein(typo, cand);
        if d > 1 {
            continue;
        }
        match best {
            None => best = Some(cand),
            Some(_) => {
                tied = true;
            }
        }
    }
    if tied {
        None
    } else {
        best.map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_suggests_fn_for_def() {
        assert_eq!(keyword_suggestion("fnn"), Some("fn"));
    }

    #[test]
    fn keyword_no_suggestion_for_distance_2() {
        // 'xyz' is distance 3 from every keyword.
        assert_eq!(keyword_suggestion("xyz"), None);
    }

    #[test]
    fn keyword_no_suggestion_when_tied() {
        // 'in' and 'if' are both distance 1 from 'i', so we MUST
        // emit no suggestion to avoid `did you mean: 'in' or 'if'?`.
        assert_eq!(keyword_suggestion("i"), None);
    }

    #[test]
    fn suggestion_from_iter_works() {
        let names = ["foo", "bar", "qux"];
        assert_eq!(
            suggestion_from("fooo", names.iter().copied()),
            Some("foo".to_string())
        );
        assert_eq!(suggestion_from("zzz", names.iter().copied()), None);
    }

    #[test]
    fn suggestion_from_skips_exact_match() {
        // The caller should never ask for a suggestion for a name
        // that already exists; defensively, exact matches are
        // skipped so we never emit `did you mean: 'foo'?` when the
        // typo IS 'foo'.
        let names = ["foo", "foo2"];
        assert_eq!(
            suggestion_from("foo", names.iter().copied()),
            Some("foo2".to_string())
        );
    }
}
