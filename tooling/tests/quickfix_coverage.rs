//! Quickfix-coverage CI gate.
//!
//! Invariant: every stable diagnostic code is *classified* — it is either
//! auto-fixable (the toolchain emits a `SuggestedPatch` carrying at least one
//! `TextEdit`, so `boruna lang repair` can apply it mechanically) or it is on an
//! explicit ALLOWLIST of codes that are intentionally not auto-fixed, each with a
//! documented reason.
//!
//! This turns "some diagnostics have fixes" into an enforced property:
//!
//! * The registry (`diagnostics::registry`) is the enumerable source of truth for
//!   every code the system can emit; a separate drift test in that module keeps it
//!   1:1 with the `E0NN` constants. So enumerating codes here is complete.
//! * `FIXABLE` pairs each auto-fixable code with a triggering `.ax` snippet. The
//!   test drives that snippet through the *real* repair entry point
//!   (`DiagnosticCollector` → `RepairTool`) and asserts a patch with a real edit
//!   is produced and applied. This is a behavioral guarantee, not a static claim.
//! * `ALLOWLIST` names every code that is *not* auto-fixed, with a one-line reason.
//! * The partition assertion requires `FIXABLE ∪ ALLOWLIST == every registry code`
//!   with no overlap. Adding a new code to the registry therefore fails this test
//!   until the author either ships a fixable fixture or allowlists it with a
//!   reason — forcing the discipline on every future change.

use std::collections::BTreeSet;

use boruna_tooling::diagnostics::collector::DiagnosticCollector;
use boruna_tooling::diagnostics::registry::registry;
use boruna_tooling::repair::{RepairStrategy, RepairTool};

/// Codes that ship an auto-applicable quickfix, each paired with source that
/// triggers the code with a mechanical (non-empty-edit) patch.
///
/// Each snippet is run through `DiagnosticCollector::collect()` — the exact path
/// `boruna lang check`/`repair` uses — so this proves the fix reaches the repair
/// loop end-to-end, not merely that a helper function exists.
const FIXABLE: &[(&str, &str)] = &[
    // E003 undefined-variable → rename to the closest in-scope name.
    (
        "E003",
        "fn main() -> Int {\n    let count = 10\n    countt\n}\n",
    ),
    // E005 non-exhaustive-match → insert stub arms for the missing variants.
    (
        "E005",
        "enum Action { Add, Remove, Clear }\n\
         type State { count: Int }\n\
         \n\
         fn update(state: State, action: Action) -> State {\n\
         \x20   match action {\n\
         \x20       Add => State { count: state.count + 1 }\n\
         \x20   }\n\
         }\n\
         \n\
         fn init() -> State { State { count: 0 } }\n\
         fn view(state: State) -> String { \"ok\" }\n",
    ),
    // E006 unknown-field → rename the field to the closest real field.
    (
        "E006",
        "type State { count: Int, name: String }\n\
         \n\
         fn init() -> State {\n\
         \x20   State { countt: 0, name: \"test\" }\n\
         }\n",
    ),
    // E007 capability-violation → strip the illegal capability annotation.
    (
        "E007",
        "type State { count: Int }\n\
         type Msg { tag: String }\n\
         \n\
         fn init() -> State { State { count: 0 } }\n\
         fn update(state: State, msg: Msg) -> State !{fs.read} { state }\n\
         fn view(state: State) -> String { \"ok\" }\n",
    ),
];

/// Codes intentionally NOT auto-fixed by the repair loop. Each entry documents
/// *why* an automatic edit is unsafe or unavailable. A code here must never carry
/// an applicable quickfix expectation — it requires human judgement.
const ALLOWLIST: &[(&str, &str)] = &[
    (
        "E001",
        // Lexer: an invalid character/token has no unique correct rewrite.
        "invalid tokens have no single mechanical repair; the typo must be fixed by a human",
    ),
    (
        "E002",
        // Parser: malformed syntax is ambiguous (missing brace vs. missing expr).
        "malformed syntax has no safe mechanical edit; the intended structure is ambiguous",
    ),
    (
        "E004",
        // Not currently emitted end-to-end: the compiler classifies an undefined
        // callee as E003 (undefined variable), never E004. The rename repair path
        // exists (suggest::enhance_undefined_fn, unit-tested in
        // repair::tests::repair_e004_near_miss_function) but is unreachable until the
        // compiler distinguishes fn-call errors. Move E004 into FIXABLE when it does.
        "not emitted by the current compiler (undefined callees surface as E003); \
         repair path exists but is unreachable end-to-end",
    ),
    (
        "E008",
        // Codegen: lowering failures reflect unsupported/internal constructs with no
        // source-level mechanical fix.
        "codegen/lowering failures have no source-level mechanical fix",
    ),
    (
        "E009",
        // Type errors only produce a textual conversion hint (empty edits,
        // Confidence::Low); picking the right coercion needs human intent.
        "type mismatches yield only a textual hint (no edit); the correct coercion needs human intent",
    ),
];

fn registry_codes() -> BTreeSet<String> {
    registry().iter().map(|c| c.code.to_string()).collect()
}

fn fixable_codes() -> BTreeSet<String> {
    FIXABLE.iter().map(|(c, _)| c.to_string()).collect()
}

fn allowlist_codes() -> BTreeSet<String> {
    ALLOWLIST.iter().map(|(c, _)| c.to_string()).collect()
}

/// The gate: every registry code is classified exactly once, as either fixable
/// or allowlisted. A new code that is neither fails here.
#[test]
fn every_diagnostic_code_is_classified() {
    let all = registry_codes();
    let fixable = fixable_codes();
    let allow = allowlist_codes();

    let overlap: Vec<_> = fixable.intersection(&allow).cloned().collect();
    assert!(
        overlap.is_empty(),
        "codes appear in both FIXABLE and ALLOWLIST (must be one or the other): {overlap:?}"
    );

    let classified: BTreeSet<String> = fixable.union(&allow).cloned().collect();

    let unclassified: Vec<_> = all.difference(&classified).cloned().collect();
    assert!(
        unclassified.is_empty(),
        "diagnostic codes are neither fixable nor allowlisted — ship a quickfix or \
         add an ALLOWLIST entry with a reason: {unclassified:?}"
    );

    let stale: Vec<_> = classified.difference(&all).cloned().collect();
    assert!(
        stale.is_empty(),
        "FIXABLE/ALLOWLIST reference codes not in the registry (typo or removed code): {stale:?}"
    );
}

/// Every allowlist entry carries a non-empty reason.
#[test]
fn allowlist_entries_have_reasons() {
    for (code, reason) in ALLOWLIST {
        assert!(
            !reason.trim().is_empty(),
            "allowlisted code {code} must document why it is not auto-fixable"
        );
    }
}

/// Behavioral proof: each fixable code, when triggered, actually produces an
/// applicable quickfix through the real collector → repair path.
#[test]
fn fixable_codes_produce_applicable_quickfix() {
    for (code, source) in FIXABLE {
        let ds = DiagnosticCollector::new("coverage.ax", source).collect();

        let diag = ds
            .diagnostics
            .iter()
            .find(|d| d.id == *code)
            .unwrap_or_else(|| {
                panic!(
                    "fixture for {code} did not emit that diagnostic; \
                     emitted: {:?}",
                    ds.diagnostics.iter().map(|d| &d.id).collect::<Vec<_>>()
                )
            });

        let has_applicable_edit = diag
            .suggested_patches
            .iter()
            .any(|p| p.edits.iter().any(|e| !e.new_text.is_empty()));
        assert!(
            has_applicable_edit,
            "{code} is listed FIXABLE but its diagnostic carries no patch with a \
             non-empty TextEdit — the repair loop cannot act on it"
        );

        // And the repair tool must actually apply at least one patch.
        let (repaired, result) =
            RepairTool::repair("coverage.ax", source, &ds, RepairStrategy::Best, None);
        assert!(
            !result.applied.is_empty(),
            "{code}: RepairTool applied no patch despite an applicable edit being present"
        );
        assert_ne!(
            repaired, *source,
            "{code}: repaired source is unchanged despite a patch being applied"
        );
    }
}
