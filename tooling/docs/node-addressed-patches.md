# Design note: node-addressed repair patches

**Status:** design only — not implemented in this slice.
**Scope:** `tooling/src/diagnostics` (patch model) + `tooling/src/repair` (applier).

## Current model: text-span patches

A `SuggestedPatch` (`diagnostics/mod.rs`) carries a list of `TextEdit`:

```rust
pub struct TextEdit {
    pub file: String,
    pub start_line: usize,   // 1-based line number
    pub old_text: String,    // expected current line(s)
    pub new_text: String,    // replacement
}
```

`RepairTool::apply_patch` (`repair/mod.rs`) splits the source into lines, sorts
edits by `start_line` descending, and for each edit:

1. indexes the line at `start_line - 1`,
2. verifies `old_text` still matches that line (trim-tolerant),
3. splices in `new_text`.

Patches are addressed by **absolute line number** plus an `old_text` guard.

## The failure it has: span drift

Line numbers are only valid against the *exact* source the diagnostics were
collected from. Any edit that inserts or deletes lines above a pending patch
shifts every later line number, invalidating the address.

Within a single `RepairTool::repair` call this is handled by applying edits
**bottom-up** (descending `start_line`), so earlier edits never move the target
of a later one. But that safety does not extend across:

- **Two repair passes.** Collect diagnostics → apply pass 1 (which adds/removes
  lines, e.g. E005 inserting match arms) → the still-open pass-2 patches now
  point at stale line numbers. The `old_text` guard turns this into a *skip*
  (mismatch → `Err`) rather than a corruption, so today's outcome is a silently
  dropped fix, not a broken file.
- **Concurrent edits.** An agent edits the file (or a formatter reflows it)
  between `lang check` and `lang repair`. Same drift, same silent skip.
- **Multi-line `new_text`.** An edit whose `new_text` changes the line count
  shifts everything below it; only the descending-sort ordering saves the
  current single-pass case.

Net effect: repairs are reliable exactly once, against pristine source. The
agent-repair loop (check → repair → re-check → repair …) is where this bites,
because pass N+1 runs against source pass N already reflowed.

## What node-addressing changes

Address each edit to an **AST node identity** instead of a line span. The
compiler already produces a `Program` AST (parsed in `DiagnosticCollector`);
give every node a stable id (a path such as `item[3].body.stmt[2].expr`, or an
interned `NodeId` assigned during parse). A patch then says "replace the
expression at node `N` with this text/subtree", and the applier:

1. re-parses the (possibly already-edited) current source,
2. locates node `N` by its structural path,
3. maps the node back to its current byte/line span via the span table,
4. applies the replacement there.

Because the address is structural, an unrelated insert elsewhere no longer
moves it — the node is re-located against whatever the source looks like *now*.
A patch only fails if the targeted node genuinely no longer exists (the thing it
fixed was already removed), which is the correct outcome.

## Rough effort

Medium — roughly:

- **Parser/AST:** assign stable node ids or a deterministic structural-path
  scheme + retain per-node spans. The AST already carries line info for
  diagnostics; this widens it to full spans and an addressing scheme. (largest piece)
- **Patch model:** add a node-addressed edit variant alongside `TextEdit`
  (keep both during migration).
- **Suggest layer:** `suggest.rs` helpers currently compute line numbers +
  `old_text`; they would instead reference the node they already have in hand
  (each `enhance_*` / `suggest_*` already walks the AST).
- **Applier:** re-parse + resolve node → span + splice by byte range, replacing
  the line-splice logic.
- **Tests:** the existing `repair/mod.rs` and `analyzer.rs` suites plus the new
  `quickfix_coverage` gate exercise the behavior; add drift-specific cases
  (apply pass 1, then pass 2 against reflowed source).

Ballpark: a focused multi-day change, mostly in the compiler AST and the
applier; the suggest sites are mechanical once node handles are available.

## Migration sketch

1. Add `NodeId`/structural-path + spans to the AST; keep `TextEdit` untouched.
2. Add `EditTarget::Node { id, new_text }` as an alternative to the current
   line-addressed edit. `SuggestedPatch.edits` accepts either.
3. Teach `RepairTool::apply_patch` to resolve node-addressed edits by
   re-parsing and mapping id → current span; leave the line-addressed path as-is.
4. Migrate suggest sites one code at a time (E003/E005/E006/E007), each guarded
   by the `quickfix_coverage` behavioral test, so a regression is caught immediately.
5. Once all emitters are node-addressed, deprecate the line-addressed variant
   (or keep it for externally-supplied patches / non-AST files).

No behavior change is forced on consumers until step 4; the two addressing modes
coexist through the migration.
