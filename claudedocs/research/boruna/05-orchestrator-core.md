# Boruna Orchestrator Core — Research (Slice 05)

Scope: `orchestrator/src/{workflow,engine,conflict,patch,simulate,adapters,storage}/*.rs` + `lib.rs`, `metrics.rs`.
Excluded (other agents): `audit/`, `persistence/`, `cli/`, `main.rs`.
Method: every claim cites `path:line` from the actual source read in-session. Runner (`workflow/runner.rs`, 11266 lines) was read surgically at the cited ranges, not end-to-end — flagged in COVERAGE.

---

## 1. Purpose & Architecture

The orchestrator core is two overlapping subsystems sharing a crate:

1. **Workflow DAG execution** (`workflow/`) — the enterprise path. A `WorkflowDef` (`definition.rs`) is a `schema_version`-gated JSON DAG of `StepDef`s. `WorkflowValidator` (`validator.rs`) checks structure + acyclicity (Kahn's algorithm) and computes topological order / wave-levels. `WorkflowRunner` (`runner.rs`) executes steps: an **ephemeral** `run()` path (tempdir, no checkpoints) and a `persist-sqlite`-gated `run_persistent()`/`resume()` path with approval-gate and external-trigger pause/resume. `DataStore` (`data_flow.rs`) moves per-step outputs (atomic-rename + fsync) and resolves `step.output` input refs.

2. **Multi-agent coordination** (`engine/`, `conflict/`, `patch/`, `adapters/`, `storage/`) — a separate `WorkGraph`/`Scheduler` (`engine/`) with role-owned nodes, a module-level `LockTable` (`conflict/`), filesystem `PatchBundle` apply/rollback (`patch/`), shell-out gate adapters (`adapters/`), and JSON file `Store` (`storage/`). This is a distinct DAG type from the workflow DAG — the two do not share validation code.

3. **Simulation** (`simulate/`) — property-based repeated execution of a workflow with an invariant DSL + witness counters, borrowed from Quint.

`lib.rs:1-13` gates `metrics`/`persistence` behind `feature = "persist-sqlite"`; `simulate`, `storage`, `engine`, `conflict`, `patch`, `adapters`, `workflow` are always compiled.

---

## 2. Component Inventory

| File | Responsibility | Key types / fns | Status |
|---|---|---|---|
| `workflow/definition.rs` | Workflow schema + typed parse gate | `WorkflowDef`, `StepDef`, `StepKind` (Source/ApprovalGate/ExternalTrigger), `RetryPolicy`, `WorkflowRunResult`, `from_json` | Real, complete |
| `workflow/validator.rs` | DAG validation + topo order/levels | `WorkflowValidator::{validate, topological_order, topological_levels}` | Real, solid |
| `workflow/data_flow.rs` | Inter-step output store + input resolution | `DataStore::{store_output, resolve_input, hash_value}`, `fullsync_file` | Real, durability-hardened |
| `workflow/runner.rs` | Step execution, retry, approval/trigger pause-resume | `WorkflowRunner::{run, run_persistent, resume, execute_steps*}`, `retry_with_backoff`, `generate_trigger_token` | Real, large, mostly complete |
| `engine/graph.rs` | Multi-agent work-graph data model | `WorkGraph`, `WorkNode`, `Role`, `NodeStatus` | Real (data only) |
| `engine/mod.rs` | Multi-agent DAG scheduler | `Scheduler::{validate, ready_nodes, assign_next, topological_order}` | Real; **validate() has a bypass — see §3/§4** |
| `conflict/mod.rs` | Module-level advisory locks | `LockTable::{acquire, release, check_conflicts}` | Real but minimal (no expiry, no deadlock/merge) |
| `patch/mod.rs` | Patch bundle validate/apply/rollback | `PatchBundle::{validate, apply, content_hash}` | Real, path-traversal defended |
| `simulate/mod.rs` | Repeated-run simulator harness | `Simulator::{new, run}`, `SimulationOptions` | Real harness; **seed not threaded — see §3** |
| `simulate/invariant.rs` | Invariant DSL parser + evaluator | `Invariant::{parse, check}`, recursive-descent `Parser` | Real, not a stub |
| `simulate/witness.rs` | Witness frequency counters | `WitnessSpec`, `WitnessTracker` | Real; one `unreachable!()` landmine (§4) |
| `adapters/mod.rs` | Gate adapters shelling to cargo/boruna | `GateAdapter` trait + 7 impls, `run_gates`, `parse_test_counts` | Real |
| `storage/mod.rs` | JSON file store for graphs/locks/gates | `Store::{save_graph, load_graph, save_gate_result, ...}` | Real; **no path-traversal defense on IDs — see §4** |
| `lib.rs` | Crate module wiring | feature gates | Real |
| `metrics.rs` | (behind `persist-sqlite`; not deep-read) | — | Not verified (feature-gated, out of critical path) |

---

## 3. GAPS

- **G1 — Simulator never varies its input; `seed` is cosmetic.** `simulate/mod.rs:196-197` loops `WorkflowRunner::run(self.def, &self.base_run_options)` with the **same** `base_run_options` every iteration. The module doc (`simulate/mod.rs:9`) claims runs execute "under a seeded RNG-derived per-trace seed", but `options.seed` is only stored into the report (`mod.rs:224`) and never threaded into `RunOptions` or the VM. For a deterministic workflow all N traces are byte-identical; invariant/witness stats are meaningless unless a capability under the active policy is itself non-deterministic. Input fuzzing is explicitly deferred (`mod.rs:16-22`), but the "seeded per-trace" claim overstates what ships. Severity: **Medium** (misleading contract; feature under-delivers, not unsafe).

- **G2 — `StepDef.outputs` is unenforced; only a single hardcoded `"result"` output is ever produced.** Every source step stores exactly one output named `"result"` (`runner.rs:3014`, and on resume paths `:1664, :1819, :1969, :2507`). The declared `outputs: BTreeMap` on `StepDef` (`definition.rs:191-192`) is never used to emit named outputs. An input ref like `foo.summary` is accepted by the validator (it only checks the *step* exists, `validator.rs:91-99`) but fails at **runtime** with "output not found" (`data_flow.rs:173`) because only `foo.result` exists. Severity: **Medium** (validation/runtime mismatch; multi-output steps are silently impossible).

- **G3 — Validator does not tie input refs to dependency edges.** `validator.rs:88-110` verifies a referenced step exists but not that it is an upstream dependency (edge or `depends_on`). A step may reference `other.result` without declaring a dependency on `other`; if `other` is in the same/later wave, `resolve_input` fails at runtime rather than validation. Data-flow edges are not derived from input refs. Severity: **Medium**.

- **G4 — `conflict::LockTable` is prevention-only, not resolution.** `conflict/mod.rs` is a whole-module mutual-exclusion table: no lock expiry (`acquired_at` stored `:13` but never read), no deadlock detection, no lock ordering, and no actual merge/rebase conflict *resolution*. "Multi-agent conflict resolution" here means "coarse module locking." Severity: **Low-Medium** (completeness vs. the platform's stated ambition).

- **G5 — `DataStore.store_output` builds paths from unvalidated `step_id`/`output_name`.** `data_flow.rs:119,126` join `step_id` and `{output_name}.json` directly into the path with no `..`/absolute check. Mitigated in practice because step IDs are workflow-JSON map keys and output name is the constant `"result"`, but the layer itself has no defense. Severity: **Low** (see also SEC-4).

- **G6 — Engine `validate()` reports dangling deps as "cycle detected".** `engine/mod.rs:22-71`: a `WorkNode.dependencies` entry pointing at a non-existent node inflates the visited count and yields `Err("cycle detected: ...")` — a misleading message for what is actually a dangling-reference error. The workflow validator does this correctly (`validator.rs:77-86` has a dedicated `UnknownStep`); the engine has no equivalent. Severity: **Low** (DX), but it is the same root cause as SEC-2 below.

- **G7 — `PatchBundle::apply` cannot create new files and rewrites line endings.** `apply` calls `file_path.canonicalize()` (`patch/mod.rs:139`), which requires the target to already exist, so patches can only modify existing files. On write it does `new_lines.join("\n") + "\n"` (`:200`), normalizing CRLF and forcing a trailing newline — can corrupt files that legitimately lack one. Severity: **Low** (correctness/robustness).

---

## 4. Security (in scope)

**Path traversal — patch bundle:**
- `patch/mod.rs:92-104` (`validate`) rejects `..` components and absolute paths. `patch/mod.rs:137-150` (`apply`) is defense-in-depth: `canonicalize()` the joined path + `canonical.starts_with(canonical_base)`. Symlinks pointing outside base resolve and are rejected. **[SAFE]** for the checked surface.
- Minor: `apply` does not itself call `validate` first — it relies solely on the canonicalize+starts_with check; and the post-canonicalize `fs::read_to_string(&file_path)` uses the non-canonical path (`:151`), a theoretical TOCTOU if a symlink is swapped between check and read. `patch/mod.rs:151` **[NEEDS-REVIEW]** (very low practical risk).

**Path traversal — storage layer (NO defense):**
- `storage/mod.rs:32-34, 44-45, 116-118, 128-129` build paths via `format!("{}.json", graph.id)` / `format!("{graph_id}.json")` / `format!("{node_id}.gate.json")` joined onto `base_dir` with **no** `..`/absolute validation. `load_graph("../../etc/passwd")` reads `base_dir/graphs/../../etc/passwd.json`; `save_graph` with a crafted `graph.id` writes outside `graphs/`. Reachability depends on whether IDs come from untrusted input (callers in `cli/`/`persistence/` are out of slice). `storage/mod.rs:41-48` and `:30-38` **[NEEDS-REVIEW]** — confirmed missing defense; exploitability gated by caller trust boundary.

**DAG-validation bypass — engine Scheduler:**
- `engine/mod.rs:22-71` `Scheduler::validate()` can pass a **cyclic** graph as valid when nodes carry dangling dependencies. Worked example: nodes A,B,C in a cycle (A dep C, B dep A, C dep B) each *also* declaring a distinct phantom dep (p1,p2,p3). The phantom deps enter `in_degree` at 0 (`:28`), get queued as sources, and bump `visited` to 3 while the three real cyclic nodes never reach in-degree 0 — `visited (3) == nodes.len() (3)` returns `Ok`. `WorkGraph` is `Deserialize` from external JSON (`engine/graph.rs:85-92`) with no referential-integrity check, so a malformed/hostile graph file bypasses cycle detection. `engine/mod.rs:63-70` **[CONFIRMED]** (logic proven by construction; severity Medium — the *workflow* validator at `validator.rs` is NOT affected, only the multi-agent engine path).

**DAG-validation — workflow path (safe):**
- `workflow/validator.rs:151-200` builds `in_degree` strictly from `def.steps.keys()` and validates unknown deps/edges separately (`:61-86`), so the phantom-node inflation cannot occur. `run()` validates before executing (`runner.rs:318`) and refuses `ExternalTrigger` on the ephemeral path (`:335-343`). **[SAFE]**

**Panics:**
- `runner.rs:3508` `last_err.expect("loop runs at least once")` — **unreachable**: `max_attempts = policy.map(|p| p.max_attempts.max(1)).unwrap_or(1)` (`:3449`) clamps 0→1, so `1..=max_attempts` always iterates once. A hostile `retry.max_attempts = 0` cannot trigger it. **[SAFE]**
- `witness.rs:139-146` `impl From<InvariantParseError> for String { fn from(_) -> Self { unreachable!() } }` — a coherence-shim that panics if ever invoked via `.into()`. Not called today; a refactor that routes an `InvariantParseError` through `String::from`/`?` would panic. `witness.rs:145` **[NEEDS-REVIEW]** (latent landmine, not currently reachable).
- `runner.rs:441, 754, 1450` `serde_json::to_string(&policy).unwrap_or_default()` — non-panicking. `now_unix_ms` uses `.unwrap_or(0)` (`:3560`). **[SAFE]**

**Entropy / trigger-token boundary:**
- `runner.rs:3583-3597` `generate_trigger_token` reads 16 bytes from `/dev/urandom` and **fails loudly** (no low-entropy fallback) — deliberate, documented at `:3573-3581`. The token binds a `workflow trigger` call to a pause instance. **[SAFE / good]**

**Command execution — adapters:**
- `adapters/mod.rs` uses `std::process::Command` with fixed argv (no shell); the only variable args are `file` paths passed as direct arguments (`:163-176, :232-236`), so no shell-injection surface. Config-controlled, not request-controlled. **[SAFE]**

---

## 5. Coverage Statement

Fully read (100%, incl. tests): `validator.rs`, `patch/mod.rs`, `conflict/mod.rs`, `definition.rs`, `data_flow.rs`, `simulate/{mod,invariant,witness}.rs`, `engine/{mod,graph}.rs`, `adapters/mod.rs`, `storage/mod.rs`, `lib.rs`. **Not read:** `metrics.rs` (801 lines, `persist-sqlite`-gated, outside the critical execution path — declared not-verified). `workflow/runner.rs` (11266 lines) was read **surgically** at cited ranges (entry `run`/`run_persistent` `:305-425`, the hardcoded-`result` output store `:2940-3020`, retry/entropy/store-guard helpers `:3420-3597`) plus a full symbol grep (`pub fn`, `"result"`, `seed`, `Awaiting*`, `resume`, `unwrap`/`expect`/`panic`, `Instant::now`, `rand`). The approval-gate and external-trigger resume synthesis (`:1109-1970`) was confirmed present and substantive (real, not stub) by grep + spot-reads but not line-audited end-to-end — a deeper pass on the resume state machine and the concurrent wave executor (`execute_steps_concurrent`, ~`:2190-2600`) would be the highest-value follow-up. All §3/§4 findings are grounded in directly-read lines.
