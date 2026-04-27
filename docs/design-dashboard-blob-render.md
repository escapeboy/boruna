# Dashboard Blob-Aware Output Rendering — Sprint 0.5-S7b

**Status:** Design (Think phase)
**Sprint:** 0.5-S7b (small followup to 0.5-S7)

## Problem audit

The 0.5-S7 retro flagged the dashboard's per-run detail page as a
hidden read of `cp.output_json` that would silently break for
blob-stored outputs. A grep of the actual rendering code shows
something different:

- `dashboard.rs::render_detail` does NOT render step outputs at all
  in HTML. It shows step_id, status, attempts, started, ended,
  error — but no output column. This has been the layout since
  0.4-S16.
- The JSON API endpoint `GET /api/runs/{id}` returns
  `Vec<StepCheckpoint>` — and 0.5-S7 added `output_blob_ref:
  Option<String>` to that struct with `#[serde(default)]`. JSON
  consumers already see both fields and can fetch the blob via
  the S7 route.

So the retro overstated the issue. There is no silent-failure bug
in the dashboard. **But** there is a UX gap: an operator looking
at a completed step on the HTML page cannot see the output value
or know whether it was offloaded to a blob. They have to either
shell into the data-dir or call the JSON API.

## Forcing questions (compressed)

### Who needs this?

Operators triaging a workflow run via the read-only dashboard.
Today they see "step `s1` completed at 14:32:01" but nothing about
what `s1` returned. For small outputs, that means leaving the
dashboard to inspect via CLI (`boruna run --resume` etc.). For
large outputs, the only lever is the new blob route, but they have
to know to construct the URL by hand.

### Narrowest MVP

Add an "Output" column to the per-run detail page's steps table:

- **Inline output** (≤ truncation_threshold of 256 chars): render
  the JSON inline in a `<code>` block.
- **Inline output longer than 256 chars**: render the first 256
  chars plus `… (truncated)`. No expand-toggle (would require JS;
  out of scope).
- **Blob-stored output**: render `<a href="/api/runs/{run_id}/blobs/{hash}"><code>[blob: hash[..16]…]</code></a>`
  so the operator gets a one-click fetch.
- **No output yet** (Pending/Running/paused/Failed-without-output):
  render `—`.

The reads go through `store.read_step_output(run_id, step_id)` for
inline cases; for the blob-display case we skip the actual byte
fetch (don't slurp 5 MB into a dashboard render — just show the
ref + link).

### "Whoa" factor

Operator click-path goes from "look at JSON via curl" → "click the
blob link in the dashboard." That's tiny but real for triage flow.

### What's out of scope

- **In-page expand/collapse for truncated inline outputs** —
  requires JavaScript. Dashboard is intentionally JS-free per
  0.4-S16.
- **Rendering output in the index/list endpoint** — list endpoint
  uses the slim `RunSummary` projection (per-S7 convention §0.4-S16,
  list endpoints aggressively project away high-disclosure fields).
  Outputs stay on detail only.
- **Pagination of long step lists** — already a carried debt.
- **Blob bytes streamed inline as part of the JSON detail
  response** — operators can call the blob route separately. Keeping
  detail JSON small.

## Scope

| Area | Change |
|------|--------|
| `dashboard.rs::render_detail` | Add Output column; render via accessor; truncate inline at 256 chars; show blob link for ref |
| `dashboard.rs::handle_run_detail` / `load_run_detail` | Resolve outputs via `read_step_output` per step |
| `dashboard.rs::handle_api_run_detail` | No change (StepCheckpoint already serializes both fields) |
| Tests | +2 HTML render tests (small inline, blob ref shows link) |
| Docs | Brief CHANGELOG entry under [Unreleased] |

**Read-path consistency check (per project convention §37):** after
S7b, every persistence-layer reader of step outputs goes through
`read_step_output`. Audited:
- `workflow/runner.rs` resume path → switched in S7
- `workflow/runner.rs` create_bundle → switched in S7 (review fix H1)
- `workflow/data_flow.rs::store_output` → reads from in-memory store, not the persistent column directly; orthogonal
- `dashboard.rs` HTML detail → switched this sprint (S7b)
- `dashboard.rs` JSON detail → returns `Vec<StepCheckpoint>`
  intentionally; consumers handle the both-fields shape

## Constraints

1. The dashboard handler is async; the orchestrator's persistence
   uses a sync `Mutex<Connection>`. Pattern: lock, call
   `read_step_output` for each step, drop lock before returning to
   the renderer. Same pattern as the S7 blob handler.

2. Truncation must NOT mutate JSON in a way that leaves invalid
   shape. Just clip and append `…` to the displayed text — the
   underlying value is untouched in the DB.

3. HTML output must continue to escape every value via
   `html_escape`. Output JSON often contains `<`, `>`, `"`, `&`.

## Acceptance criteria

- Per-run detail HTML table has an Output column.
- Small completed step shows truncated/inline output.
- Blob-stored step shows `[blob: <hash[..16]>…]` linked to
  `/api/runs/{run_id}/blobs/{hash}`.
- Pending/Running/paused/Failed steps show `—`.
- 2 new HTML rendering tests (inline + blob).
- 1 regression test asserting JSON API still serializes both
  fields correctly (post-S7 invariant; not new behavior).
- All workspace tests green.
- Clippy + fmt clean.
