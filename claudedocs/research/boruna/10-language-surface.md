# Boruna — Language Surface & Web UI Research

READ-ONLY research of the user-facing surface: standard libraries (`libs/`), app
templates (`templates/`), examples (`examples/`), and the web dashboard
(`host/web/`). Every claim is cited to `path:line` and, where behaviour was
verified, run against the local debug binary `target/debug/boruna` (built
2026-07-15). Unverified items are labelled explicitly.

---

## 1. Purpose & Architecture

The language surface is the part enterprise developers actually touch. It has
four layers:

- **Standard libraries (`libs/`)** — 13 packages, each a `package.ax.json`
  manifest + `src/core.ax` source. All are pure-functional: they build plain
  `Effect`/UI/state **data records**; none performs IO directly. Side effects are
  represented as *data* (an `Effect { kind, payload, callback_tag }` record) that
  a host later interprets — this is what lets the pure code stay deterministic
  (`libs/std-http/src/core.ax:2` "Only produces Effects. No execution in
  library."). Every `core.ax` carries its own `fn main()` so it compiles and runs
  standalone.

- **Templates (`templates/`)** — 5 scaffolds, each `template.json` (metadata +
  declared deps/capabilities + typed `args`) plus `app.ax.template` with
  `{{variable}}` placeholders. Applying a template does simple `{{var}}` string
  substitution and emits a self-contained Elm-architecture app
  (`init`/`update`/`view`/`policies`/`main`).

- **Examples (`examples/`)** — standalone `.ax` programs, framework apps, and
  DAG workflows. Workflows are a `workflow.json` (steps → `source:` `.ax` file,
  typed inputs/outputs, `edges`, `depends_on`) plus a `steps/` directory of
  per-step `.ax` files. Includes a `compliance/` set (HIPAA / SOC2 / financial)
  and a `literate/` markdown-as-spec demo.

- **Web dashboard (`host/web/`)** — a React 18 + Vite 6 SPA (3 source files) that
  fetches a program's emitted UI/result JSON from a backend and renders it. Self-
  described as a prototype: "In production, this would communicate with the
  runtime via IPC/WebSocket" (`host/web/src/App.jsx:25`).

**Verification performed:** compiled+ran all 13 libs, all 16 standalone/framework
examples; applied+validated all 5 templates; validated all 8 example workflows +
3 compliance workflows; ran diagnostics on the repair demo. Results in §3/§5.

---

## 2. Inventory Tables

### 2a. Standard libraries — lib → manifest capabilities → version → status

Verified by `boruna run libs/<lib>/src/core.ax --policy allow-all`. **All 13
compile AND execute clean.** Manifest version is from `package.ax.json`; no lib
source contains a `!{...}` capability annotation (see §4).

| Lib | Manifest `required_capabilities` | Manifest version | Compiles+runs | Notes |
|-----|----------------------------------|------------------|:-------------:|-------|
| std-authz | *(none)* | 1.0.0 | PASS | Pure role/level checks (`libs/std-authz/src/core.ax`) |
| std-db | `db.query` | 1.0.0 | PASS | Query-builder → `Effect{kind:"db_query"}` data only |
| std-forms | *(none)* | 1.0.0 | PASS | **Declares dep `std.validation:"0.1.0"`** — stale, see §3 |
| std-http | `net.fetch` | 1.0.0 | PASS | Effect wrappers; `parse_int` is a stub returning 0 (`:69`) |
| std-json | *(none)* | 1.0.0 | PASS | Uses `while`+local mutation, list/string builtins |
| std-llm | `llm.call` | 1.0.0 | PASS | Doc/memory still call it "0.1 experimental" — stale (§3) |
| std-notifications | `time.now` | 1.0.0 | PASS | **No time usage in source** — cap likely over-declared (§4) |
| std-routing | *(none)* | 1.0.0 | PASS | `route_match_first` hardcoded to 3 routes (`:39`) |
| std-storage | `fs.read`, `fs.write` | 1.0.0 | PASS | Effect wrappers matching emitted kinds |
| std-sync | `net.fetch` | 1.0.0 | PASS | Pure conflict-resolution state machine |
| std-testing | *(none)* | 1.0.0 | PASS | `test_summary` hardcoded to 3 results (`:63`) |
| std-ui | *(none)* | 1.0.0 | PASS | **`UINode{tag,text}` has no children** — cannot nest (§3) |
| std-validation | *(none)* | 1.0.0 | PASS | Uses `try_parse_int`/`__builtin_*` (registered builtins) |

**Stability reality:** every manifest is `1.0.0`. CHANGELOG confirms std-llm and
std-json were promoted to 1.0-stable in v1.3.0 (`CHANGELOG.md:92-93`). The
"std-llm / std-json = 0.1.0 Experimental" statement in `CLAUDE.md` and the
auto-memory `MEMORY.md` is **stale doc drift**, not a code gap (§3).

### 2b. Templates — apply + validate

Verified by `boruna template apply <name> --args ... --validate`.

| Template | Required args | Declared deps / caps | Applies + validates | Note |
|----------|---------------|----------------------|:-------------------:|------|
| auth-app | `app_name` | authz/http/storage · net.fetch,fs.* | PASS | — |
| crud-admin | `entity_name`, `fields` | ui/forms/authz/db/validation · db.query | PASS | `{{entity_name}}` interpolated into SQL payload strings (§4) |
| form-basic | `form_name`, `fields` | ui/forms/validation · none | PASS | `fields` arg used only in a comment |
| offline-sync | `entity_name` | sync/storage/http · net.fetch,fs.* | PASS | — |
| realtime-feed | `feed_name`, `poll_interval_ms?` | ui/http/notifications · net.fetch,time.now | PASS | arg is `feed_name`, not `entity_name` (§3) |

All emitted apps **redefine their own `Effect`/`UINode`/`State` types inline** and
do **not** import the declared `dependencies` libs — deps are metadata, not wired.

### 2c. Examples — compile / validate

| Example | Kind | Result |
|---------|------|:------:|
| hello, counter, todo, fibonacci, capabilities, pattern_matching, while_loop | standalone `.ax` | PASS (run) |
| admin_crud, offline_sync_todo, realtime_notifications, stdlib_demo, llm_patch_demo, trace_demo | app `.ax` | PASS (run) |
| framework/counter_app, todo_app, parallel_demo | framework | PASS (`framework validate` → "valid App protocol") |
| workflows/{api_routing, customer_support_triage, data_ingestion, document_processing, form_submission, json_data_transformer, llm_code_review, llm_content_generator} | DAG | PASS (all 8 `workflow validate`) |
| compliance/{financial_review_pipeline, hipaa_data_pipeline, soc2_audit_workflow} | DAG | PASS (all 3 `workflow validate`) |
| literate/hello_literate.md | literate spec | Read only (extract not run) |
| repair_demo/broken.ax | intentionally broken | Expected diagnostics E005/E006/E007 fire correctly |

---

## 3. GAPS (drift, limitations, doc↔reality)

- **[LOW · doc drift] std-llm / std-json mislabelled "0.1 experimental."**
  `CLAUDE.md` and `MEMORY.md` say these two libs are 0.1.0/Experimental. Reality:
  both manifests are `1.0.0` (`libs/std-llm/package.ax.json:3`,
  `libs/std-json/package.ax.json:3`) and CHANGELOG records their promotion to
  1.0-stable in v1.3.0 (`CHANGELOG.md:92-93`). Project docs are behind.

- **[LOW-MED · dependency drift] std-forms pins an unsatisfiable dep version.**
  `libs/std-forms/package.ax.json:5` declares `"std.validation": "0.1.0"`, but
  `libs/std-validation/package.ax.json:2` is `1.0.0`. The constraint targets a
  version that no longer exists. Harmless today because each `core.ax` is self-
  contained (forms doesn't actually call validation), but any real resolver run
  would fail or warn.

- **[MED · functional limitation] std-ui cannot represent a UI tree.**
  `libs/std-ui/src/core.ax:4` defines `UINode { tag: String, text: String }` —
  **no children field.** So `row(child1, child2)` (`:8`), `column`, `card`,
  `container` all discard their children and keep only `child1.text`/the title.
  The "declarative UI primitives" can emit a single node, not a nested layout.
  Every framework app's `view` likewise returns a flat `UINode{tag,text}`.

- **[MED · prototype] Web host renders raw VM values, not the std-ui UINode; is
  not interactive; backend absent.** `host/web/src/renderer.jsx` interprets
  low-level `Value` shapes (`tree.Record` → "Record #id / field_0…",
  `tree.Enum` → "Enum #id::variant_N", `:69-97`) — it has no case for the std-ui
  `{tag,text}` node, so semantic UI (button/input/card) is never rendered as such.
  `onEvent`/`sendEvent` is threaded through every render function but **attached to
  no element** (no `onClick`/`onChange` anywhere in `renderer.jsx`), so nothing is
  clickable. `App.jsx:27,41` fetch `/api/run` and `/api/event` from a backend that
  does not exist in `host/web/` (no server file, none in `package.json`). This is
  an explicitly-labelled prototype, but as shipped it is a read-only value dumper.

- **[LOW-MED · misleading schema] Workflow named outputs are cosmetic.**
  `examples/workflows/document_processing/workflow.json` declares e.g.
  `classify.outputs = {category: String}`, but downstream `merge.inputs` reference
  `classify.result` / `extract.result` / `summarize.result`. Per project memory
  the runner always stores a step's output under the hardcoded name `"result"`, so
  the descriptive `outputs` keys (`document`, `category`, `entities`, `summary`)
  are never used for wiring. Schema reads as typed data-flow but isn't enforced as
  such. (Runner-side behaviour taken from memory + input naming; not re-verified in
  runner source — belongs to the orchestrator slice.)

- **[LOW · doc drift] `CLAUDE.md` realtime example arg is wrong.** The CLI shows
  `realtime-feed` requires `feed_name` (`templates/realtime-feed/template.json`),
  and `--args "entity_name=..."` fails with "missing required argument: feed_name".
  Any doc/snippet using `entity_name` for this template is wrong.

- **[LOW · missing docs] 2 of 8 example workflows lack a README.**
  `json_data_transformer/` and `llm_content_generator/` have `workflow.json` +
  `steps/` but no `README.md`; the other 6 do. These two were added for the
  std-json / std-llm 1.0 promotion (`CHANGELOG.md:127`).

- **[LOW · stubs] Dead `parse_int` helpers.** `libs/std-http/src/core.ax:69-71`
  and `libs/std-validation/src/core.ax:112-115` both define `parse_int` returning a
  constant `0`. std-http's `http_parse_status` depends on it, so status parsing is
  a no-op. (std-validation instead uses the real `try_parse_int` builtin.)

- **[LOW · language ergonomics] Fixed-arity helpers pervade the stdlib.**
  `std-testing` `test_summary`/`test_all_passed_3` are hardcoded to 3 results
  (`:63,:77`); `std-routing` `route_match_first` to 3 routes (`:39`);
  `std-notifications` `NotificationQueue` tracks only `last_*` fields + a count, not
  an actual list (`:8-15`). These are consequences of the surface lacking
  generic fold/list ergonomics rather than bugs, but they cap real-world use.

- **[INFO] Doc claims "13 libraries" — accurate.** 13 dirs, 13 manifests, all
  compile. The count is right; only the per-lib stability labels drifted (above).

---

## 4. SECURITY

- **[SAFE] No XSS in the dashboard renderer.** `host/web/src/renderer.jsx` renders
  all untrusted program output through React text interpolation —
  `{tree.String}` (`:59`), `{tree.Int}` (`:56`), map keys/values `{k}` (`:47`),
  and the fallback `<pre>{JSON.stringify(tree,null,2)}</pre>` (`:66`). React
  escapes all of these by default. There is **no `dangerouslySetInnerHTML`, no
  `innerHTML`, no `eval`** anywhere in `host/web/` (grep-clean across the 3 source
  files). Untrusted step output (LLM text, document contents) cannot break out
  into markup. `App.jsx:73` `JSON.stringify(result)` inside `<code>` is likewise
  escaped.

- **[NEEDS-REVIEW · low] std-notifications over-declares `time.now`.**
  `libs/std-notifications/package.ax.json` requires `time.now`, but the source uses
  no time capability — `notification_dismiss_effect` (`src/core.ax:48-50`) merely
  builds an `Effect{kind:"timer"}` **data record** with no `!{time.now}`
  annotation. The other capability-declaring manifests (std-db `db.query`,
  std-http/std-sync `net.fetch`, std-storage `fs.read/fs.write`) at least match the
  `Effect.kind` strings their functions emit, so they read as "the effect the host
  will run needs this cap." `time.now` has no corresponding emission. Because **no
  lib function carries any `!{...}` capability annotation at all**, none of these
  declarations are enforced at the lib's own compile/run boundary — they are
  advisory metadata for whoever executes the effects. Worth a pass to confirm the
  intended semantics (documentation vs. enforcement).

- **[NEEDS-REVIEW · low] crud-admin template interpolates args into SQL strings.**
  `templates/crud-admin/app.ax.template` substitutes `{{entity_name}}` directly
  into payloads like `"INSERT INTO {{entity_name}}"`, `"DELETE FROM {{entity_name}}"`,
  `"SELECT * FROM {{entity_name}} WHERE name LIKE"`. This is build-time codegen and
  the payload is effect-as-*data* (the VM does not execute SQL), so there is no
  injection **within Boruna**. The risk is downstream: a host that later executes
  that payload as real SQL would be vulnerable to whatever `entity_name` the
  scaffolder passed (metacharacters/injection). Out of this slice's runtime scope —
  flagged for the host/db-executor review, not confirmed exploitable here.

- **[SAFE] Template substitution itself is inert.** `{{var}}` values verified to
  land only in comments (form-basic `fields`), string literals, or the SQL payload
  strings above; all 5 templates apply and the output passes `--validate` (parses +
  typechecks), so a benign arg cannot produce malformed/unparseable `.ax`.

- **[SAFE] Dashboard a11y/UX debt (not security).** The Map renderer uses bold
  `<td>` cells as headers instead of `<th scope>` (`renderer.jsx:47`), tables have
  no caption, and there are no ARIA roles; the two hardcoded action buttons
  (`App.jsx:60,63`) are reachable but the rendered tree has no interactive
  controls. Accessibility/UX debt consistent with prototype status.

---

## 5. COVERAGE

**Fully read + verified (compiled/validated on `target/debug/boruna`):** all 13
lib manifests and all 13 `src/core.ax` (13/13 compile+run); all 5 `template.json`
+ `app.ax.template` (5/5 apply+validate); all 8 example workflows + 3 compliance
workflows (11/11 validate); 16 standalone/framework example `.ax` files (16/16
compile+run); framework-protocol validation on 4 apps (4/4 "valid App protocol");
repair-demo diagnostics; all 3 `host/web/src/*.jsx` + `index.html` + `package.json`
(read line-by-line, grepped for XSS sinks). Workflow `source:` step references
cross-checked — validator resolves execution order for every workflow, so **no
dangling step refs**.

**Read once / sampled, not exhaustively:** individual workflow step `.ax` bodies
(only `document_processing/steps/ingest.ax` read in full; the rest are covered by
validator success, not line review); workflow `README.md` prose; the
`literate/hello_literate.md` spec (read, but `literate extract` not executed).

**Out of scope / not covered (belongs to other slices):** `examples/llm_handlers/`
provider configs + `router_setup.rs` (Rust/TOML host wiring, effect-crate
adjacent); the actual orchestrator runner semantics behind the "outputs are stored
as `result`" claim (asserted from memory + input naming, not re-read here); any
server backing `host/web` `/api/*` (none exists in-repo). No code was modified.
