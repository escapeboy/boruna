# Boruna Research 09 — Developer-Facing Tooling

Scope: 4 crates — `boruna-tooling` (dir `tooling/`), `boruna-pkg` (dir `packages/`),
`boruna-mcp` (`crates/boruna-mcp/`), `boruna-lsp` (`crates/boruna-lsp/`).
Every claim below is grounded in a file I read; citations are `path:line`.
Read-only research; no code was modified.

---

## 1. Purpose & Architecture

| Crate | Dir | Role |
|---|---|---|
| **boruna-tooling** | `tooling/` | Library of developer tooling: structured diagnostics + AST analyzer, auto-repair, trace→tests (record/generate/minimize), stdlib test runner, template engine, formatter, literate/ITF/migrations helpers. |
| **boruna-pkg** | `packages/` | Deterministic package ecosystem: manifest spec + validation, SHA-256 content hashing, exact-version resolver with topo sort, filesystem registry, lockfile, `boruna-pkg` CLI. |
| **boruna-mcp** | `crates/boruna-mcp/` | MCP (JSON-RPC/stdio) server exposing the toolchain to AI coding agents. `rmcp`-based `ToolRouter`. Source is passed as **strings**, not paths. |
| **boruna-lsp** | `crates/boruna-lsp/` | `tower-lsp` language server: diagnostics, completion, formatting, hover. |

Dependency direction: `boruna-mcp` and `boruna-lsp` are thin adapters over
`boruna-tooling` + `boruna-compiler` + `boruna-vm` + `boruna-framework` +
`boruna-orchestrator`. `boruna-tooling` sits above compiler/vm/framework;
`boruna-pkg` depends only on `boruna-compiler` + `boruna-bytecode`.

MCP process launch args `--templates-dir` / `--libs-dir` are operator-controlled
(`crates/boruna-mcp/src/main.rs:10-17`); `libs_dir` is stored but `#[allow(dead_code)]`
(`server.rs:153-154`) — no MCP tool consumes it.

---

## 2. Component Inventory

| File | Responsibility | Key types / fns | Status |
|---|---|---|---|
| `crates/boruna-mcp/src/server.rs` | MCP tool router, param structs, 1 MB source guard, streaming progress | `BorunaMcpServer`, `validate_source` (`:14`), 12 `#[tool]` methods | Complete |
| `crates/boruna-mcp/src/tools/mod.rs` | Response protocol version + regression tests | `TOOL_RESPONSE_PROTOCOL_VERSION=1` (`:21`) | Complete |
| `crates/boruna-mcp/src/tools/run.rs` | Compile+run under policy, limits, output-schema gate, progress callbacks | `run_source` (`:75`), `run_source_with_progress` (`:113`), `drive_vm` (`:278`), `parse_policy` (`:561`), `validate_output_against_schema` (`:443`) | Complete, hardened |
| `crates/boruna-mcp/src/tools/compile.rs` | compile / AST tools, 100 KB AST cap | `compile_source`, `parse_ast` (`:30`), `compile_error_json` (`:79`) | Complete |
| `crates/boruna-mcp/src/tools/check.rs` | diagnostics + repair (string-in/string-out) | `check_source`, `repair_source` (`:56`) | Complete |
| `crates/boruna-mcp/src/tools/framework.rs` | App-protocol validate + message-sequence test | `validate_app`, `test_app` (`:52`), `parse_message` (`:122`) | Complete |
| `crates/boruna-mcp/src/tools/workflow.rs` | Workflow DAG validate + topo order | `validate_workflow` | Complete |
| `crates/boruna-mcp/src/tools/template.rs` | List/apply templates (delegates to tooling) | `list_templates`, `apply_template` (`:43`) | Complete; passes name through unsanitized (§4) |
| `crates/boruna-mcp/src/tools/policy.rs` | Strict policy validation tool | `validate_policy` (`:18`) | Complete |
| `crates/boruna-mcp/src/tools/capability.rs` | Capability-set identity hash | `list_capabilities` | Complete |
| `crates/boruna-mcp/src/main.rs` | CLI entrypoint, stdio transport | `Args` | Complete |
| `tooling/src/templates/mod.rs` | `{{var}}` template engine + manifest | `apply_template` (`:80`), `load_template` (`:69`), `substitute` (`:120`) | Complete; traversal + naive substitution (§3/§4) |
| `tooling/src/repair/mod.rs` | Patch-selection strategies + text-edit apply | `RepairTool::repair` (`:49`), `repair_file` (`:129`), `apply_patch` (`:219`) | Complete; multi-line edits unverified (§3) |
| `tooling/src/trace2tests/mod.rs` | Record trace → gen test → run → ddmin minimize | `record_trace` (`:94`), `generate_test` (`:181`), `run_test` (`:232`), `minimize_trace` (`:376`) | Complete, well-tested |
| `tooling/src/diagnostics/analyzer.rs` | AST passes: match-exhaustiveness, record-fields, capability-purity | `Analyzer::analyze` (`:46`) | Complete (3 passes) |
| `tooling/src/diagnostics/suggest.rs` | Near-miss rename suggestions (E003/E004/E009) | `enhance_compiler_diagnostic` (`:6`) | Complete |
| `tooling/src/diagnostics/collector.rs` | Orchestrates compiler + analyzer + suggest | `DiagnosticCollector` | Complete |
| `tooling/src/diagnostics/registry.rs` | Stable `E0NN` code registry (agent-facing) | `REGISTRY` (`:25`) | Complete |
| `tooling/src/stdlib/mod.rs` | Library compile/run/determinism harness | `run_library` (`:6`), `verify_determinism` (`:26`), `load_library_source` (`:53`) | Complete |
| `tooling/src/format/mod.rs` | Source formatter (968 lines) | `format_source` | Complete (idempotent, used by LSP) |
| `packages/src/spec/mod.rs` | Manifest/lockfile/policy + SHA-256 content hash | `PackageManifest::validate` (`:33`), `compute_content_hash` (`:216`), `verify_hash` (`:262`) | Complete; hash omits `bytecode/` (§4) |
| `packages/src/resolver/mod.rs` | Exact-version resolve, conflict detect, topo sort | `resolve` (`:17`), `topological_sort` (`:125`), `generate_lockfile` (`:76`) | Complete, deterministic |
| `packages/src/storage/mod.rs` | Filesystem registry publish/verify/list | `Registry::publish` (`:69`), `verify_all` (`:115`), `copy_dir_recursive` (`:200`) | Complete; symlink-follow on copy (§4) |
| `packages/src/cli/mod.rs` | pkg CLI: init/add/remove/resolve/install/publish/verify/tree | `cmd_*` | Complete |
| `crates/boruna-lsp/src/main.rs` | LSP server | `compute_diagnostics` (`:44`), `completion_items` (`:116`), `hover_info` (`:187`), `formatting` (`:287`) | Functional; no go-to-def/refs/rename |

**Advertised vs implemented MCP tools:** all 12 `#[tool]` methods in `server.rs`
have real implementations (verified each in `tools/*`). See §3 gap #1 re: the
count discrepancy with docs.

---

## 3. Gaps

**G1 — CLAUDE.md / AGENTS.md advertise "10 MCP tools"; the server exposes 12.**
`server.rs` defines: `boruna_compile`, `boruna_ast`, `boruna_run`, `boruna_check`,
`boruna_repair`, `boruna_validate_app`, `boruna_framework_test`,
`boruna_workflow_validate`, `boruna_template_list`, `boruna_template_apply`
(`server.rs:169-436`) **plus** `boruna_capability_list` (`:448`) and
`boruna_policy_validate` (`:460`). The two extra tools are fully implemented and
tested. Doc drift, not a functional gap. Severity: **low** (docs).

**G2 — `apply_patch` does not verify `old_text` for multi-line edits.**
`tooling/src/repair/mod.rs:242-252`: the `old_text == actual` guard runs only when
`old_count == 1`. For multi-line patches it splices `idx..idx+old_count`
(`:255-257`) without confirming the spanned lines match the expected `old_text`.
A stale/mis-located multi-line patch can silently corrupt source. Mitigated by the
post-repair re-run of diagnostics (`:121-123`) which sets `verify_passed`, but the
corrupted text is still returned/written. Severity: **medium** (correctness).

**G3 — Package content hash excludes compiled bytecode.**
`compute_content_hash` walks only `pkg_dir/src` (`packages/src/spec/mod.rs:223-236`),
plus the manifest and dep hashes. The `bytecode/*.axbc` artifacts written during
`publish` (`storage/mod.rs:98,177-197`) are **not** covered by `HASH`. Bytecode is
derived from source so determinism holds, but a post-publish tamper of an `.axbc`
in the registry would pass `verify_all`. Whether any consumer loads the precompiled
bytecode is outside this slice — flagged for the pkg/vm owners. Severity:
**medium** if bytecode is consumed; **low** if always recompiled.

**G4 — LSP surface is intentionally minimal.**
`compute_diagnostics` + `completion_items` + `hover_info` + `formatting` only
(`crates/boruna-lsp/src/main.rs`). No go-to-definition, find-references, rename,
or semantic tokens. Completion and hover are line-scan heuristics
(`:167-205`), not AST-driven, so they miss nested/shadowed symbols. `did_change`
uses `TextDocumentSyncKind::FULL` and takes only the last change (`:273-278`) —
correct for FULL sync. Severity: **low** (feature completeness, documented as
early-stage by capability set).

**G5 — Template `{{var}}` substitution is order-dependent and can chain.**
`substitute` (`tooling/src/templates/mod.rs:120-127`) iterates the `BTreeMap` in
key order doing successive `String::replace`. A value that itself contains
`{{laterKey}}` will be re-substituted on a later iteration, so an earlier arg can
inject a later arg's placeholder. Deterministic (BTreeMap order) but surprising;
no escaping of `{{`/`}}`. Severity: **low**.

---

## 4. Security

**S1 — MCP 1 MB source cap + `spawn_blocking`: CONFIRMED PRESENT / SAFE.**
`validate_source` rejects `> 1_048_576` bytes (`server.rs:11-19`) and is called at
the top of every source-taking tool: compile `:176`, ast `:193`, run `:211`, check
`:325`, repair `:342`, validate_app `:364`, framework_test `:379`. Note
`boruna_workflow_validate` (`:394`) and `boruna_template_apply` (`:422`) do **not**
call `validate_source` — they take `workflow_json` / template args, not `source`;
those inputs are unbounded at the MCP layer (workflow JSON parsed by serde;
template args substituted). Every synchronous Boruna call is wrapped in
`tokio::task::spawn_blocking` (e.g. `:180,195,272,300,329,347,366,383,400,413,430,449,465`).
Tag: **SAFE** for the source path; **NEEDS-REVIEW** that `workflow_json`/template
args have no size cap.

**S2 — MCP tools take no filesystem paths from requests and never exec — CONFIRMED.**
All request params are strings/objects (`server.rs:23-145`); the only filesystem
roots (`templates_dir`, `libs_dir`) come from process launch args, not requests
(`main.rs`, `server.rs:159-165`). No `Command`/process spawn anywhere in
`crates/boruna-mcp/`. Tag: **SAFE**.

**S3 — Template-name path traversal via `boruna_template_apply` — NEEDS-REVIEW.**
`template_name` flows request → `tools::template::apply_template(dir, name, …)`
(`server.rs:427-431`, `tools/template.rs:43-62`) →
`boruna_tooling::templates::apply_template(path, name, …)` →
`load_template` which does `templates_dir.join(name).join("template.json")`
(`tooling/src/templates/mod.rs:70`) and `templates_dir.join(name).join("app.ax.template")`
(`:95`) with **no** `..`/sanitization check. A caller can set
`template_name = "../../../../some/dir"` to read a `template.json` / `app.ax.template`
outside `templates_dir`; the file contents are returned in the tool response
(`tools/template.rs:70-71`). Read-only and constrained to those two fixed
filenames, but a real traversal that discloses file contents. Tag: **NEEDS-REVIEW**
(bounded-impact path traversal, no write/exec). Same pattern in
`stdlib::load_library_source` (`tooling/src/stdlib/mod.rs:53-56`) but that is not
reachable from MCP.

**S4 — Package name path-traversal: mitigated by regex validation — CONFIRMED SAFE.**
`Registry::package_dir` joins `base_dir/name/version` (`storage/mod.rs:20-22`) and
`name` reaches it from manifests/dependencies. `PackageManifest::validate` requires
names to match `^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)*$` (`spec/mod.rs:37-43,303-327`),
which rejects `/`, `.` runs, and `..`; versions must be `MAJOR.MINOR.PATCH`
(`:277-280`). `publish` validates before use (`storage/mod.rs:72-74`); `resolve`
validates every loaded manifest and each parent validates its dependency names
before they are enqueued (`resolver/mod.rs:40-42,56-61`); root is validated in
`cmd_resolve`/`cmd_install` (`cli/mod.rs:67,85`). Tag: **SAFE** (traversal blocked
transitively by the name grammar).

**S5 — SHA-256 content hashing integrity — CONFIRMED, with the §4/G3 caveat.**
Hash = sorted `src/` file rel-paths+contents + integrity-stripped manifest +
sorted dep hashes (`spec/mod.rs:216-259`); prefixed domain separators
(`MANIFEST:`, `DEP:`) prevent field-confusion. `verify_hash` recomputes and
string-compares against `HASH` (`:262-273`). Deterministic (BTreeMap + sorted
file list). Gap: `bytecode/` not hashed (G3). Compare is not constant-time but the
registry is local and the value is a public integrity digest, not a secret. Tag:
**CONFIRMED** (sound for source); bytecode coverage **NEEDS-REVIEW**.

**S6 — Lockfile / resolution determinism & cycle safety — CONFIRMED SAFE.**
Resolver uses exact versions only, detects same-name/different-version conflicts
(`resolver/mod.rs:44-53`), and `topological_sort` sorts every frontier for stable
order and returns `"circular dependency detected"` when `order.len() != packages.len()`
(`:139-173`). Lockfile hashes computed bottom-up in install order
(`:82-108`); `test_lockfile_deterministic` locks byte-identical output
(`:326-341`). `Lockfile::load` rejects unknown versions (`spec/mod.rs:124-129`).
Tag: **SAFE**.

**S7 — `publish` copy follows symlinks — NEEDS-REVIEW (low).**
`copy_dir_recursive` recurses on `src_path.is_dir()` (`storage/mod.rs:210`), which
follows symlinks; a source package with a symlinked directory under `src/` would
copy arbitrary readable dirs into the registry (and then hash them). Local
dev-tool blast radius, requires a malicious source tree the operator already ran
`publish` on. Tag: **NEEDS-REVIEW**.

**S8 — `boruna_run` resource-limit hardening — CONFIRMED SAFE (defensive).**
`max_memory_mb` is rejected at parse time rather than silently ignored
(`tools/run.rs:129-143`) — explicitly to avoid an integrator believing memory is
bounded. `output_schema` is size-capped at 256 KB before compilation
(`:449-464`), non-2020-12 `$schema` is rejected (`:466-484`), validation errors are
capped at 100 (`:507-524`), output serialization is byte-budgeted with a cheap
giant-string pre-check (`:398-419`), and a large `Value::String` is rejected before
double-allocation (`:406-410`). `deny-all`/`allow-all` shorthands and object
policies route through the shared strict validator (`:561-601`). Tag: **SAFE**
(notably careful).

**S9 — LSP formatting won't corrupt on parse failure — CONFIRMED SAFE.**
`formatting` returns an empty edit set when `format_source` errors
(`crates/boruna-lsp/src/main.rs:293-297`), and `did_change` stores text after
analysis. No filesystem writes; documents held in-memory only (`:9-33`). Tag:
**SAFE**.

---

## 5. Coverage

Read in full: all 12 files under `crates/boruna-mcp/src/` (server, main, tools/*);
`tooling/src/templates/mod.rs`, `tooling/src/repair/mod.rs`,
`tooling/src/trace2tests/mod.rs`; `packages/src/spec/mod.rs`,
`packages/src/resolver/mod.rs`, `packages/src/storage/mod.rs`,
`packages/src/cli/mod.rs`; `crates/boruna-lsp/src/main.rs`. Read partially (head /
enough to characterize): `tooling/src/diagnostics/{analyzer,suggest,registry}.rs`,
`tooling/src/stdlib/mod.rs`.

**Not opened** (enumerated via `find`, not read line-by-line — status inferred from
name/size, treat as *not verified*): `tooling/src/format/mod.rs` (968 lines — only
confirmed it exists and LSP calls `format_source` and it is idempotent per LSP
tests), `tooling/src/diagnostics/{mod,collector}.rs` (behavior inferred from
callers), `tooling/src/literate/mod.rs`, `tooling/src/trace/{mod,itf,audit_to_itf}.rs`
(ITF export), `tooling/src/migrations/*` (workflow_json / evidence_bundle
migrations), `tooling/src/import_resolver.rs`, `tooling/tests/*`,
`packages/tests/integration.rs`, `packages/src/main.rs`. These are the honest
coverage boundary for this slice; none were required to answer the focus questions,
but `format/mod.rs` and `import_resolver.rs` (import path handling — potential
traversal surface) are the highest-value follow-ups if a deeper pass is warranted.
