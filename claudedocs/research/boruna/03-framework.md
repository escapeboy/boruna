# Boruna Research — 03: Framework Layer (`crates/llmfw` → `boruna-framework`)

Read-only research. Every claim cites `path:line` against the actual source. "Not verified" is stated where a fact could not be confirmed from this slice.

> ⚠️ **VERIFICATION CORRECTION (see [[verify-C-vm-framework]], 2026-07-16).** Finding #1 below — "PolicySet ≠ VM Policy — not wired (HIGH)" — is **OVERSTATED and downgraded to MEDIUM**. The declared PolicySet *does* gate execution: `send()` → `check_batch` → `check_effect` rejects any undeclared effect with `PolicyViolation` **before** `HostEffectExecutor::execute` runs. The executor's hardcoded `Policy::allow_all()` is a redundant second layer *behind* that gate, not an open door. The real, narrower weaknesses stand: **fail-open defaults** (empty capabilities ⇒ allow-all at `policy.rs:106`; no `policies()` ⇒ `PolicySet::allow_all()`) and a `pub HostEffectExecutor::execute` that a caller could invoke directly, skipping `check_batch`. Read finding #1 with this correction.

Scope: all 9 non-test `.rs` files in `crates/llmfw/src/` plus a coverage read of `tests.rs`. Cross-crate behavior (VM step budgeting, `Vm::run` internals) is out of slice and tagged NEEDS-REVIEW where it matters.

---

## 1. Purpose & Architecture

`boruna-framework` layers an **Elm-style architecture** (`init` / `update` / `view`) on top of the compiler + VM. An app is an ordinary `.ax` module that defines three functions; the framework drives them through a deterministic cycle and mediates side effects as *data* rather than direct calls.

The cycle (`runtime.rs:126-183`):

```
init() -> State                     (once, at AppRuntime::new)
send(msg):
  update(State, Msg) -> UpdateResult{state, effects}   (PURE — deny_all policy)
  PolicySet.check_batch(effects)                        (framework-level gate)
  StateMachine.transition(new_state)                    (records snapshot)
  view(State) -> UINode                                 (PURE — deny_all policy)
  log CycleRecord
effects are executed separately by an EffectExecutor, producing callback AppMessages
```

Two design pillars:
1. **Purity by construction** — `update`/`view` run under `Policy::deny_all()` inside the VM (`runtime.rs:234-238`), so a capability call inside them faults and is re-tagged `PurityViolation` (`runtime.rs:288-297`). This is enforced at *runtime*, independent of the static annotation check.
2. **Effects as data** — `update` returns a `[state, effects_list]` record; effects carry a `callback_tag` so their results re-enter as the next message (`effect.rs:89-136`, `executor.rs`).

There are **two distinct policy systems** that do not share state (detailed in §3, GAP-1): the framework `PolicySet` (`policy.rs`) and the VM `Policy` (`boruna_vm::capability_gateway::Policy`, used in `runtime.rs` and `executor.rs`).

---

## 2. Component Inventory

| File | Responsibility | Key types / fns | Status |
|---|---|---|---|
| `lib.rs` | Module wiring + public re-exports | `AppRuntime`, `TestHarness`, `AppValidator`, `PolicySet`, executors | Complete (`lib.rs:13-18`) |
| `validate.rs` | Static App-protocol conformance from AST | `AppValidator::validate` → `ValidationResult` (`validate.rs:29`) | Complete but shallow (see GAP-2) |
| `runtime.rs` | Drives init→update→effects→view; purity enforcement; cycle log; rewind/diff | `AppRuntime`, `AppMessage`, `CycleRecord`, `call_function` (`runtime.rs:57,128,223`) | Complete |
| `effect.rs` | Effect model + parsing update() return value | `Effect`, `EffectKind`, `parse_update_result`, `parse_effects` (`effect.rs:8,16,127,93`) | Complete |
| `executor.rs` | Execute effects via VM gateway → callbacks | `EffectExecutor` trait, `MockEffectExecutor`, `HostEffectExecutor` (`executor.rs:12,19,89`) | Complete; one dead branch (GAP-6) |
| `policy.rs` | Framework `PolicySet` parse + effect gating; error→JSON | `PolicySet`, `check_effect`, `check_batch`, `from_value` (`policy.rs:30,104,116,77`) | Complete; permissive-empty footgun (GAP-3) |
| `state.rs` | State lifecycle, history, diffing, time-travel | `StateMachine`, `StateDiff`, `diff_values`, `rewind` (`state.rs:24,16,103,145`) | Complete; shallow diff + rewind history bug (GAP-4) |
| `testing.rs` | Test harness: simulate sequences, assert, replay | `TestHarness`, `simulate`, `replay_verify` (`testing.rs:11,53,144`) | Complete; single-pass, effects not looped (GAP-5) |
| `ui.rs` | Value↔UINode conversion (pass-through, no render) | `UINode`, `value_to_ui_tree`, `ui_tree_to_value` (`ui.rs:8,38,68`) | Complete |
| `error.rs` | Error enum with `thiserror` | `FrameworkError` (11 variants) (`error.rs:4`) | Complete |

---

## 3. GAPS (file:line + severity)

**GAP-1 — `PolicySet` and the executor's VM `Policy` are not wired together. [HIGH]**
`AppRuntime.send` gates effects with `self.policy.check_batch(&effects)` (`runtime.rs:156`), but effect *execution* happens in a separate `EffectExecutor`. `HostEffectExecutor::new()` builds its own gateway with `Policy::allow_all()` (`executor.rs:100-107`) and there is **no code path that constructs the executor's `Policy` from the app's `PolicySet`**. So the app-declared `policies()` restrictions gate only the *pre-execution* check inside `send`; a caller who uses `send_with_executor` (`runtime.rs:190-198`) with a default `HostEffectExecutor` executes effects under allow-all regardless of the declared PolicySet. The two layers must be kept in sync manually via `HostEffectExecutor::with_handler(policy, ...)` (`executor.rs:110-115`). Divergence is real and undocumented.

**GAP-2 — Validator checks function *shape* only, never types or return values. [MEDIUM]**
`AppValidator::validate` (`validate.rs:29-132`) verifies presence + param count + capability-annotation absence for `init`/`update`/`view`/`policies`. It **never checks return types**: no verification that `init` returns `State`, `update` returns an `UpdateResult{state, effects}`, or `view` returns a `UINode`. `state_type`/`message_type` are detected purely by name suffix convention (`validate.rs:98-106`, `"…State"`, `"…Msg"/Message"`) and are **never required** — a missing `State`/`Msg` type produces no error. The documented App-protocol types (`State, Msg, Effect, UpdateResult, UINode`) are therefore *convention*, enforced only later by runtime shape-parsing (`effect.rs:127-136` returns `None` → generic `Effect` error at `runtime.rs:149-153`).

**GAP-3 — `PolicySet` with empty `capabilities` allows every effect; the "default" is permissive despite being named restrictive. [MEDIUM]**
`check_effect` short-circuits when the list is empty: `if !self.capabilities.is_empty() && !…contains` (`policy.rs:106`). `PolicySet::default()` has `capabilities: Vec::new()` (`policy.rs:42-49`), so the default gate passes all effects — confirmed by `test_policy_empty_capabilities_allows_all` (`tests.rs:399-413`). The test named `test_policy_default_is_restrictive` (`tests.rs:490-500`) only asserts the list is empty; it does **not** assert restriction — the name contradicts the behavior. Additionally `AppRuntime::new` falls back to `PolicySet::allow_all()` when no `policies()` is defined (`runtime.rs:81-86`), and `from_value` on a malformed/empty policy record yields empty capabilities → allow-all. Fail-open by default.

**GAP-4 — `rewind` does not truncate history; diffing is positional/shallow. [MEDIUM]**
`StateMachine::rewind` sets `current`/`cycle` from a found snapshot but leaves `history` intact (`state.rs:145-154`). A subsequent `transition` increments from the rewound `cycle` (`state.rs:60-73`), so `history` can contain **duplicate cycle numbers**; `diff_from_cycle`/`rewind` both use `find(|s| s.cycle == …)` (`state.rs:93,149`) which returns the *first* match, silently ignoring later duplicates — time-travel after a rewind is ambiguous. Separately, `diff_values` only compares top-level `Record` fields positionally, labels them synthetically `field_{i}` (`state.rs:120-125`), and does **not recurse** — a change deep in a nested record surfaces as the whole field changing. `max_history` eviction via `remove(0)` (`state.rs:69-71`) makes rewind to an evicted cycle fail with a `State` error.

**GAP-5 — `TestHarness` message execution is single-pass; effect callbacks are not automatically looped. [LOW]**
`simulate` (`testing.rs:53-60`) runs `update` but drops effects entirely (no executor). `send_with_effects` (`testing.rs:43-50`) runs one effect round and returns callbacks but does **not** feed them back — the caller must re-send manually. So effect-driven state convergence (callback → new message → new state) is not exercised by the sequence helpers; only manually in tests. `replay_verify` (`testing.rs:144-165`) likewise ignores effects and only compares `state_after` up to `min(messages, original_log)`.

**GAP-6 — Dead/misleading branch in `HostEffectExecutor`. [LOW]**
`executor.rs:162-172` handles `effect_to_capability(...) == None` with a comment "e.g., SpawnActor", but `effect_to_capability` maps **every** `EffectKind` to `Some(...)` including `SpawnActor → ActorSpawn` (`executor.rs:124-137`). The `None` arm is unreachable and the comment is stale.

**GAP-7 — `PolicySet.max_steps` is parsed but never applied to the VM. [MEDIUM — cross-crate]**
`PolicySet` carries `max_steps` (`policy.rs:38`, parsed at `policy.rs:88-91`), but `call_function` constructs `Vm::new(module, gateway)` and calls `vm.run()` with **no step budget passed** (`runtime.rs:240-285`). Nothing in `llmfw` forwards `max_steps` into VM execution. Whether `update`/`view` termination is bounded therefore depends entirely on a VM-internal default limit in `Vm::run` — **not verified** (VM slice). If the VM has no internal cap, a non-terminating `update` is unbounded per message despite the per-message `max_cycles` guard (`runtime.rs:132-134`).

---

## 4. Security (in scope) — policy enforcement gaps & panics on malformed input

**S-1 — Effects executed under allow-all when executor ≠ PolicySet. [CONFIRMED]**
Same root as GAP-1. `executor.rs:100-107` default `Policy::allow_all()`; `runtime.rs:156` only pre-checks. A declared `policies()` that forbids `fs.write` still executes an `fs.write` effect if the host uses a default `HostEffectExecutor`. Enforcement correctness depends on caller discipline, not on the framework. `runtime.rs:156`, `executor.rs:104`.

**S-2 — Fail-open policy defaults. [CONFIRMED]**
Empty-capabilities PolicySet allows all effects (`policy.rs:106`), and both the no-`policies()` path (`runtime.rs:85`) and malformed policy parse (`policy.rs:99`, `from_value` `_ => default()`) land on permissive states. A misconfigured or unparsable `policies()` silently grants full capabilities rather than denying. `policy.rs:106`, `runtime.rs:81-86`, `policy.rs:99`.

**S-3 — `init()` runs under `Policy::allow_all()`, ungated by the app's PolicySet. [CONFIRMED — by design, but note]**
`AppRuntime::new` calls `init` with `pure = false` → `Policy::allow_all()` (`runtime.rs:78`, `runtime.rs:234-238`), and `policies()` is loaded *after* init (`runtime.rs:78-86`). So `init` may perform any capability with no PolicySet restriction. The comment states this is intentional ("init may use capabilities", `runtime.rs:77`), but it means the declared policy never constrains initialization side effects.

**S-4 — Purity enforcement for `update`/`view` is robust. [SAFE]**
Both run under `Policy::deny_all()` (`runtime.rs:234-238`); any capability attempt faults and is mapped to `PurityViolation` via `wrap_purity_error`, which only special-cases `CapabilityDenied`/`CapabilityBudgetExceeded` and passes other VM errors through as `Runtime` (`runtime.rs:288-297`). Confirmed by `test_purity_update_denies_capabilities` / `_view_` (`tests.rs:928,1009`). This is annotation-independent — even an app with a missing/incorrect capability annotation cannot escape the deny-all sandbox at runtime.

**S-5 — No panics found on malformed app input. [SAFE — reviewed]**
All JSON serialization uses `unwrap_or_default()` (`state.rs:33,62,77`, `policy.rs:137,169`). Malformed `update` return → `parse_update_result` returns `None` → clean `FrameworkError::Effect` (`runtime.rs:149-153`), not a panic. Missing functions → `MissingFunction` error at `AppRuntime::new` (`runtime.rs:67-75`). `value_to_ui_tree` has a total match with a `_` fallback (`ui.rs:38-64`). One truncation risk, not a panic: `Op::Call(func_idx, args.len() as u8)` (`runtime.rs:261`) casts arg count to `u8` — harmless for the fixed-arity protocol (≤2 args) but would silently truncate at >255 args. [NEEDS-REVIEW only if arbitrary-arity call paths are added.]

**S-6 — Message encoding is convention, not typed. [NEEDS-REVIEW]**
`AppMessage::to_value` builds `Record { type_id: 0, [String(tag), payload] }` (`runtime.rs:28-33`). Whether a real `.ax` `Msg` enum pattern-matches correctly against this hand-built record (vs. a compiler-emitted `Enum` value) depends on the compiler's Msg representation — not verifiable from this slice. Tests pass with this shape (`tests.rs:189-230`), so it holds for tested apps, but there is no validator check binding `update`'s parameter type to this encoding (ties to GAP-2).

---

## 5. Coverage Statement

Read in full: `lib.rs`, `validate.rs`, `runtime.rs`, `executor.rs`, `state.rs`, `policy.rs`, `effect.rs`, `testing.rs`, `ui.rs`, `error.rs` (all 9 non-test source files, 1407 LOC). `tests.rs` (2392 LOC) was inventoried by test-name grep and spot-read (`tests.rs:399-518`) to confirm §3/§4 behavioral claims, not read line-by-line. Cross-crate items explicitly **not verified** here (belong to VM slice): whether `Vm::run` applies an internal step budget (GAP-7/termination), and the compiler's `Msg` enum runtime representation vs. `AppMessage::to_value` (S-6). All other claims are grounded in the cited framework source lines.
