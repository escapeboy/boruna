# Boruna CLI (`boruna-cli`) — Research Report

**Scope:** `crates/llmvm-cli/src/` — the `boruna` binary. Read-only audit.
**Crate dir → name:** `crates/llmvm-cli` = `boruna-cli`.
**Coverage:** All **15** `.rs` source files read in full (the brief's "27 files" counted the doubled `wc` glob listing plus 4 embedded `.md` skill docs). `main.rs` (5341 LOC) and `coordinator.rs` (4136 LOC) analyzed by dedicated sub-agents reading the full files; the other 13 read directly by this agent. Line citations verified against source.

---

## 1. Purpose & Architecture + Subcommand Map

`boruna-cli` is the single operator-facing binary. `main.rs` defines the clap `Command` enum (`main.rs:60-317`) and dispatches it (`main.rs:1404-1631`). Sibling modules implement the heavier subcommands. Three cargo features gate optional surfaces:

- `persist-sqlite` — SQLite-backed workflow persistence (approve/reject/resume/metrics, dashboard/coordinator inner logic).
- `serve` — axum HTTP servers (`serve`, `dashboard`, `coordinator`, `worker`, `framework serve`, `evidence serve`).
- `http` — real outbound HTTP handler for `--live`.

**Every subcommand is fully wired.** There are **no** `unimplemented!()`, `todo!()`, `panic!`, "coming soon", `TODO`, or `FIXME` in production code across the crate (grep-confirmed; only `#[cfg(test)]` `panic!` helpers exist). Feature-gated paths return a **typed error** when the feature is absent, not a stub.

### Subcommand status table

| Subcommand | Handler | Status |
|---|---|---|
| `compile` | `main.rs:1405` | Wired |
| `run` (`--live`, `--watch`, `--record`) | `run_once`/`run_watch_loop` `main.rs:2874/2924` | Wired |
| `trace` | `main.rs:1462` (forces `allow_all`) | Wired |
| `replay` | `main.rs:1478` | Wired |
| `inspect` / `ast` / `fmt` | `main.rs:1498/1529/1536` | Wired (`fmt`→`format.rs`) |
| `framework {new,validate,test,inspect-state,simulate,inspect,diag,trace-hash,replay,serve}` | `main.rs:2320-2707` (`serve` gated) | Wired |
| `lang {check,repair,codes,caps}` | `main.rs:1978-2146` | Wired |
| `doctor` | `doctor.rs:141` | Wired |
| `size` | `size.rs:40` | Wired |
| `skills {list,get}` | `skills.rs:51/70` | Wired |
| `trace2tests {record,generate,run,minimize}` | `main.rs:2150-2277` | Wired (`minimize` shells out — §4) |
| `template {list,apply}` | `main.rs:5025` | Wired |
| `literate extract` | `main.rs:4985` | Wired |
| `repl` | `repl.rs:275` (default policy `deny-all`) | Wired |
| `simulate` | `main.rs:4896` | Wired (`--seed` is a reporting-only no-op, `main.rs:201-205`) |
| `new` | `scaffold.rs:49` | Wired |
| `workflow {validate,run,approve,reject,trigger,show,list,resume,schedule,eval,find,graph}` | `main.rs:3060-3894` | Wired (mutation paths `persist-sqlite`/`serve`-gated; `eval`→`workflow_eval.rs`) |
| `evidence {create,verify,inspect,gc-blobs,rotate-kek,serve,diff}` | `main.rs:4380-4674` | Wired (`serve`→`evidence_serve.rs`, `diff`→`evidence_diff.rs`) |
| `capability list` / `metrics export` / `policy {validate,show}` | `main.rs:1955/1926/1811` | Wired (`metrics` gated) |
| `dashboard` | `dashboard.rs:53` (serve-gated) | Wired |
| `coordinator {serve,wait}` | `coordinator.rs:189/2561` (serve-gated) | Wired |
| `worker` | `worker.rs:170` (serve-gated) | Wired |
| `migrate` | `main.rs:1635` | Wired |

---

## 2. Component Inventory

| File | LOC | Responsibility | Key types / fns | Status |
|---|---|---|---|---|
| `main.rs` | 5341 | clap defs + dispatch + most handlers | `Command`, `run_once`, `run_workflow`, `run_evidence`, `make_gateway`, `validate_env_name` | Complete |
| `coordinator.rs` | 4136 | Distributed-execution HTTP coordinator | `run_serve`, `build_router`, `CoordinatorState`, `auth_middleware`, blob route | Complete |
| `dashboard.rs` | 1082 | Read-only HTTP view over `runs.db` | `run_serve`, `dashboard_routes`, `handle_api_runs`, `html_escape` | Complete |
| `worker.rs` | 722 | Distributed worker (polls coordinator, runs `.ax`) | `run_worker`, `execute_step`, `parse_coordinator_urls` | Complete |
| `evidence_serve.rs` | 576 | `evidence serve` bundle inspector (HTTP) | `serve`, `load_bundle`, `render_*_page`, `open_browser` | Complete — **XSS gap §4** |
| `scaffold.rs` | 534 | `boruna new` interactive scaffold | `run_new`, `NewArgs`, `ScaffoldOutcome` | Complete |
| `serve.rs` | 499 | Framework dev server (Elm harness UI) | `run_serve`, `handle_send`, `escape_html`, `discover_message_tags` | Complete — **poison-DoS §4** |
| `repl.rs` | 494 | Interactive `.ax` REPL | `ReplSession`, `run_loop`, `dispatch_meta` | Complete |
| `workflow_eval.rs` | 476 | `workflow eval` (2-provider A/B compare) | `run_workflow_eval`, `EvalReport`, `run_one` | Complete |
| `evidence_diff.rs` | 394 | `evidence diff` (bundle comparison) | `build_diff`, `DiffReport`, `evidence_diff` | Complete |
| `provider_registry.rs` | 181 | `providers.json` loader (LLM BYOH) | `ProviderRegistry`, `ProviderConfig`, `describe` | Complete |
| `doctor.rs` | 182 | `boruna doctor` env health | `run`, `Report`, `Check`, `check_rust_toolchain` | Complete |
| `skills.rs` | 120 | Embedded agent docs (`include_str!`) | `SKILLS`, `run_list`, `run_get` | Complete |
| `size.rs` | 102 | Bytecode size report | `run`, `SizeReport` | Complete |
| `format.rs` | 80 | `fmt` CLI wrapper | `run_fmt`, `first_diff_line` | Complete |

**`--json` completeness:** `doctor` (`doctor.rs:157`), `size` (`size.rs:74`), `skills` (`skills.rs:52/76`), `policy validate`, `capability list`, `lang {check,codes,caps}`, `framework {inspect,diag}`, `simulate`, `workflow {show,list,find,graph,eval}`, `evidence {inspect,gc-blobs,diff}` — all serialize full structured reports (many hand-built with intentionally stable field names, `main.rs:3656-3659`). String previews are deliberately truncated (documented). No incomplete/malformed JSON found. `skills.rs:17` intentionally `#[serde(skip)]`s the body in `list`; `get` includes it as `content`.

---

## 3. Gaps

| # | Location | Gap | Severity |
|---|---|---|---|
| G1 | `main.rs:3866`, `4184` | `workflow schedule --max-concurrency` is destructured-and-ignored; concurrency hardcoded to `1`. Arg accepted but has no effect. | Low |
| G2 | `main.rs:201-205`, `4928` | `simulate --seed` is a reporting-only no-op (passed to `SimulationOptions`, no runtime effect yet, per its own doc). | Low |
| G3 | `main.rs:4236-4263` | `ctrlc_handler` installs **no** OS handler — body is effectively `let _ = running;`. Name is misleading; Ctrl-C responsiveness relies solely on the 1s poll-loop sleep chunks (`main.rs:4159-4171`). | Info |
| G4 | `main.rs` (6 sites, e.g. `2766`, `3144`, `3415`, `4120`, `4914`, `repl` `1566`) | Policy-string parsing (`allow-all`/`deny-all`/file) duplicated inline across 6 call sites instead of one helper. | Info (dup) |
| G5 | `coordinator.rs:1113-1130` | `parse_major_minor`/`semver_gte`/`version_compatible` are `#[cfg(test)]`-only — dead in production (claim path uses exact-equality `worker_covers_required`, `:1160`). `>=` version ordering deferred to 2.0. | Info (dead) |
| G6 | `coordinator.rs:66-67`, `83-84`, `1466` | `#[allow(dead_code)]` reserved fields: `workflow_dirs` (never populated), `WorkerSession.capability_set_hash`, `WorkItem.inputs_json` (always `None`). | Info |
| G7 | `worker.rs:136-137` | `WorkerHandle.poll_timeout_ms` `#[allow(dead_code)]` — reserved for 0.5-S2c. | Info |
| G8 | `provider_registry.rs` (whole) | Registry validates `providers.json` and `describe()`s it but does NOT instantiate handlers (doc `:8-10`). `workflow eval` reads provider config for **naming/labeling only** — both providers actually run the same local VM (`workflow_eval.rs:58-68` uses `Policy::allow_all()`, no provider dispatch). The "A/B provider comparison" compares identical execution paths unless a real `http` handler is wired externally. | Medium (feature honesty) |

---

## 4. Security

### 4.1 Command injection / shell-out — 3 sites, all SAFE

| Site | Command | Input source | Tag |
|---|---|---|---|
| `doctor.rs:66` | `rustc --version` | none (fixed args) | **SAFE** |
| `evidence_serve.rs:449-459` | `open`/`xdg-open`/`cmd /c start <url>` | `url = format!("http://localhost:{port}/bundle")`, `port: u16` — not attacker-controllable; uses `.arg()`, no shell | **SAFE** |
| `main.rs:2308-2311` | `trace2tests minimize --predicate <cmd>` runs `Command::new(parts[0]).args(...).arg(temp_path)` | `command` fully operator-supplied by design; split on whitespace, **no shell** (`main.rs:2303`), so no metachar injection. Operator explicitly names the binary. | **SAFE (by design)** |

No `sh -c` / shell interpolation anywhere. No untrusted-network or file-content data flows into a command name.

### 4.2 XSS in `evidence serve` — bundle data rendered UNESCAPED

**`evidence_serve.rs` has no `html_escape` function at all.** Every bundle field is `format!`-interpolated raw into HTML:
- `render_bundle_page` (`:152-231`): `run_id`, `workflow_name`, `started_at`, `completed_at`, all hashes, and **file-checksum keys** (`file_rows` `:223-228`, `{f}` is a filename from the manifest) — raw.
- `render_audit_page` (`:233-262`): step ids, event details (incl. `error` strings `:291`) via `describe_event` — raw.
- `render_outputs_page` (`:347-370`): step names as `<summary>{step}</summary>` and **full step output JSON** as `<pre>{json}</pre>` — raw.
- `verify_errors` rendered as `<li>{e}</li>` (`:165`) — raw.

Bundle content is attacker-influenceable: step outputs can carry LLM-generated text, `ExternalTriggerReceived` payloads, and arbitrary workflow strings. An operator who runs `boruna evidence serve <bundle>` on a bundle received from an untrusted source gets **stored XSS executing in their browser** against a `127.0.0.1` origin that also exposes `/api/bundle` (full bundle JSON). Contrast: `dashboard.rs` (`html_escape` `:309`, with regression tests `:701-714`, `:767-782`) and `serve.rs` (`escape_html` `:163`) escape everything. This module is the odd one out.
**Tag: CONFIRMED** (no escaping present). Severity: **Medium** — local-origin only (binds `127.0.0.1` `:440`, no `--bind` override), requires operator to inspect a malicious bundle, but the XSS itself is unmitigated.

### 4.3 Dashboard / coordinator / serve — auth & path traversal

- **Auth (all HTTP servers):** No authentication by default. Documented dev-grade posture. Default bind `127.0.0.1`; `--bind 0.0.0.0` emits a loud stderr warning (`dashboard.rs:70-80`, `coordinator.rs:240-246`) and (dashboard) an in-page banner (`dashboard.rs:347-356`). Coordinator adds two optional modes: shared-secret **Bearer** compared **constant-time** (`coordinator.rs:700-709`, applied `:769`) and **mTLS** with `WebPkiClientVerifier` + CN-vs-`worker_id` reconciliation (`coordinator.rs:1272-1283`). `/api/health` is intentionally auth-exempt and secret-free (`coordinator.rs:748-750`, test `:3939-3944`). **Tag: NEEDS-REVIEW** — operator must front with a reverse proxy or use the secret; the `--bind 0.0.0.0` + no-secret combination is a foot-gun but is warned about.
- **Blob-serve path traversal (`GET /api/runs/{run_id}/blobs/{hash}`, `coordinator.rs:2034`):** `hash` validated to exactly 64 lowercase hex chars **before** any FS access (`:2041-2053`), then `run_owns_blob_ref` ownership check (`:2063`), then `blob_store.read_bytes(&hash)`. `run_id` is only a SQL key, never a path segment. `dashboard.rs` blob links apply the same 64-hex validation before emitting (`:523-531`). **Tag: SAFE.**
- **Dashboard data disclosure:** `/api/runs` returns slim `RunSummary` (no `policy_json`/`metadata_json`) precisely so secrets in metadata aren't served in the no-auth list view (`dashboard.rs:120-153`, regression test `:983-1005`). Detail endpoint returns the full record (deliberate drill-in). **Tag: SAFE (by design).**
- **Body limits:** coordinator applies `DefaultBodyLimit::max(8 MiB)` to write routes (`coordinator.rs:56/837`); per-step source cap 256 KiB downstream. Dashboard/serve GET routes unlimited (read-only). **Tag: SAFE.**
- **`serve.rs` mutex-poison DoS:** every handler does `state.lock().unwrap()` (`serve.rs:82,93,135,151`). A panic inside any handler poisons the `Mutex`, after which **all** subsequent requests panic → 500. `dashboard.rs` avoids this by `map_err(|_| INTERNAL_SERVER_ERROR)` on lock (`:172-178`). Localhost dev tool, so low impact, but an inconsistency worth noting. **Tag: NEEDS-REVIEW.** Severity: **Low.**

### 4.4 `--live` gating — silent fallback

`--live` is parsed on `run`/`workflow run`/`resume`/`schedule` and forwarded to `make_gateway` (`main.rs:2760-2869`). The real HTTP handler is only built under `#[cfg(feature = "http")]` (`:2854-2861`). When `--live` is passed but `http` is not compiled in, it **does not error** — it prints `warning: --live requires the http feature; falling back to mock handler` and silently uses the mock (`:2862-2865`). By contrast `--record-net-to`/`--replay-net-from` **hard-error** without `http` (`:2806-2808`, `:2846-2848`). An operator could believe live calls happened when they were mocked. **Tag: NEEDS-REVIEW.** Severity: **Low/Info.**

### 4.5 Temp files

- `main.rs:2280-2318` (`external_predicate`) uses `tempfile::NamedTempFile::new()` (`:2285`) — non-predictable, safe. No `env::temp_dir`/`/tmp`/PID-based paths anywhere. **Tag: SAFE.**
- `doctor.rs:97-100` writes a writability probe to `data_dir.join(".boruna-doctor-probe")` (predictable name) then removes it. `data_dir` is the operator's own directory; negligible (a pre-planted symlink there is the operator's own dir). **Tag: SAFE.**
- `evidence_serve.rs`, `coordinator.rs`, `worker.rs` create no temp files in production (only `#[cfg(test)]` `tempfile::tempdir()`). mTLS cert/key paths are **read** from operator CLI flags, never written. **Tag: SAFE.**

### 4.6 Arg-parsing / path handling

- **`--env` validation:** `validate_env_name` enforces `[A-Za-z0-9_-]`, len 1-64 (`main.rs:2344-2367`) specifically to block `../..` traversal into `resolve_data_dir`'s `base.join(name)` (`:4321-4337`). Called `:1397-1399`. **Tag: SAFE.**
- **General path args** (`compile`, workflow.json reads, payload files, policy files, `repl :load`, `evidence` dirs, provider configs): read directly via `fs::read_to_string`/`read` on operator-supplied `PathBuf` with no traversal check. This is intended — a local CLI operating on operator paths, no sandbox boundary crossed. Lower-layer traversal defenses (PatchBundle, blob store hex validation) live in `boruna-orchestrator`, not here. **Tag: SAFE (by design).**
- **Deserialization of network input:** coordinator axum `Json<T>` extractors (`coordinator.rs:1230,1324,1569,1597,1624,1776,1944,1990`) — `handle_submit_run` deserializes a full `WorkflowDef`+`Policy` from the network (`:1776`), the largest untrusted surface; bounded by the 8 MiB limit and gated by auth when configured; validation delegated to `WorkflowRunner`. Worker deserializes `WorkItem` from the coordinator and `execute_step` compiles+runs the coordinator-supplied `.ax` source under the item's policy (`worker.rs:417-435`) — this is the distributed-execution design (the coordinator is a trust boundary the worker opts into via `--coordinator`). **Tag: NEEDS-REVIEW** (worker runs coordinator-supplied code — expected but worth stating).
- **Session-token compare:** worker/coord `session_token` compared with plain `==` (`coordinator.rs:1331,1361,1734`) not constant-time; tokens are random UUIDv4, so timing leak is minor. **Tag: NEEDS-REVIEW (minor).**

---

## 5. Coverage Statement

All 15 `.rs` files under `crates/llmvm-cli/src/` were read **in full**:

- **Read directly by this agent (13):** `dashboard.rs`, `evidence_serve.rs`, `serve.rs`, `worker.rs`, `doctor.rs`, `size.rs`, `skills.rs`, `format.rs`, `provider_registry.rs`, `scaffold.rs`, `repl.rs`, `workflow_eval.rs`, `evidence_diff.rs`.
- **Read in full by dedicated sub-agents (2):** `main.rs` (5341 LOC), `coordinator.rs` (4136 LOC) — findings cross-checked against grep for shell-out/stubs.
- **Not code (4, not analyzed as source):** `skills/{ax-language,cli,diagnostics,workflows}.md` — embedded documentation strings.

Stub/shell-out grep run crate-wide (`unimplemented!`/`todo!`/`TODO`/`FIXME`/`Command::new`): confirms zero stubs and exactly 3 shell-out sites (all in §4.1).
