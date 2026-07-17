# Verification Pass C — VM Capability Enforcement + Framework Policy Wiring

Read-only re-audit. Every verdict cites `path:line` opened during this pass. Repo root `/Users/katsarov/htdocs/ai-lang`, branch `ci/reduce-artifact-storage`.

---

## Section 1 — Verdict Table

| # | Claim | Verdict | Evidence |
|---|-------|---------|----------|
| 1 | FRAMEWORK: "PolicySet ≠ VM Policy — not wired. Declared policies do not gate actual execution." | **OVERSTATED** | Declared PolicySet DOES gate execution — see below. |
| 1a | send() pre-checks via `PolicySet.check_batch` (runtime.rs:156) | **CONFIRMED-ACCURATE** | `crates/llmfw/src/runtime.rs:156` `self.policy.check_batch(&effects)?` |
| 1b | HostEffectExecutor executes under its own `Policy::allow_all()` | **CONFIRMED-ACCURATE** | `crates/llmfw/src/executor.rs:102-107` `HostEffectExecutor::new` hardcodes `Policy::allow_all()` |
| 1c | Nothing derives the executor Policy from `policies()` | **CONFIRMED-ACCURATE** | No PolicySet→`vm::Policy` conversion exists anywhere; executor is passed in by the caller (`runtime.rs:190-198`) |
| 1-failopen | policy.rs:106 empty capabilities = allow-all | **CONFIRMED-ACCURATE** | `crates/llmfw/src/policy.rs:106` `if !self.capabilities.is_empty() && ...` — empty ⇒ short-circuit `Ok` |
| 1-test | `test_policy_default_is_restrictive` asserts the opposite | **CONFIRMED (UNDERSTATED)** | `crates/llmfw/src/tests.rs:490-500` asserts only `capabilities.is_empty()`; that empty state is fail-OPEN per policy.rs:106. Test name misrepresents behavior; it never asserts an effect is denied. |
| 2 | F3 SSRF via DNS-resolved hostname (literal-IP-only check) | **CONFIRMED-ACCURATE** | `crates/llmvm/src/http_handler.rs:188` `host.parse::<IpAddr>()` — DNS names skip `is_private_ip`; no post-resolution recheck |
| 3 | F4 `allow_redirects` defaults true; validate runs once, hops not re-validated | **CONFIRMED-ACCURATE** | `capability_gateway.rs:53-55,64` default true; `http_handler.rs:52` single validate before request; `http_handler.rs:30-32` only disables redirects when flag false |
| 4 | F6 `Op::SpawnActor`/`Op::SendMsg` bypass the gateway | **CONFIRMED-ACCURATE** | `crates/llmvm/src/vm.rs:526-546` mutate `spawn_requests`/`outgoing_messages` directly; only `Op::CapCall` (vm.rs:598-619) calls `self.gateway.call` |
| 5 | F7 bare i64 arithmetic → overflow panic/wrap; `i64::MIN/-1` past div guard | **CONFIRMED-ACCURATE** | `vm.rs:621` `x + y`, `:635` `x - y`, `:649` `x * y`, `:669` `x / y`, `:688` `x % y` — all unchecked |
| 6-choke | CapCall → gateway.call is the ONLY route to real host side effects | **CONFIRMED-ACCURATE** | Only `Op::CapCall` (vm.rs:616) reaches a `CapabilityHandler`; actor ops are in-VM only |
| 6-worker | Worker runs coordinator `.ax` under a mock/deny handler | **CONFIRMED (with `--live` caveat)** | `orchestrator/src/workflow/runner.rs:3142` default `MockHandler`; `:3128-3133` live path uses real `HttpHandler` only under `--live` + `http` feature |
| 6-steps | max_steps default 10M bounds infinite loops | **CONFIRMED-ACCURATE** | `vm.rs:115` default `10_000_000`; enforced `vm.rs:329-331` |
| 6-divzero | div/mod-by-zero guarded → DivisionByZero | **CONFIRMED-ACCURATE** | `vm.rs:664-666` (Div), `vm.rs:685-687` (Mod) — but see F7 for the `i64::MIN/-1` overflow that bypasses these guards |
| 6-F8 | `functions[func_idx]` panics on crafted Module | **OVERSTATED** | Untrusted `.axbc` IS loaded+run (main.rs:2729-2734), but the cited panic is unreachable — every attacker index is guarded (see below) |

### Claim 1 — why OVERSTATED (the load-bearing one)

The three mechanism sub-claims (1a/1b/1c) are individually accurate, but the **conclusion is wrong**. The normal framework flow is `send_with_executor` → `send()` (runtime.rs:195) → `check_batch` (runtime.rs:156) → then `executor.execute(effects)` (runtime.rs:196). `check_batch` → `check_effect` (policy.rs:104-113) rejects any effect whose `capability_name()` is absent from the declared `PolicySet.capabilities` with `PolicyViolation`, **before** the executor ever runs it. So a declared, non-empty PolicySet genuinely gates execution.

The executor's hardcoded `allow_all()` VM-Policy is a redundant second layer that sits *behind* the PolicySet gate — it does not widen the attack surface of the `send()` path. The real weaknesses are narrower than "not wired":
- **Fail-open defaults**: empty `capabilities` ⇒ allow-all (policy.rs:106); no `policies()` fn ⇒ `PolicySet::allow_all()` listing every capability (runtime.rs:81-86, policy.rs:54-72). An app that declares nothing gets everything.
- **Direct-executor bypass is theoretically possible**: `HostEffectExecutor::execute` is `pub`; a caller invoking it directly (not via `send`) skips `check_batch`. Not the framework's own flow, but an unguarded public API edge.

### Claim 6-F8 — why OVERSTATED

`.axbc` deserialization is a real untrusted-input surface (`crates/llmvm-cli/src/main.rs:2729-2734` → `Module::from_bytes`), so the premise holds. But no reachable crafted-module panic was found — the VM is defensively indexed throughout:
- `func_idx` enters a frame only via `call_function` which uses `.get().ok_or(InvalidFunction)` (`vm.rs:298-302`); `module.entry` flows through the same guard (`vm.rs:206-211`). Every later `functions[func_idx]` (vm.rs:363,451,580,603) uses that already-validated index.
- constants `.get` (vm.rs:392) · globals load `.get` (vm.rs:408) + store bounds-check (vm.rs:415-417) · locals load `.get` (vm.rs:1514) + store bounds-check (vm.rs:1520-1521) · match_tables `.get` (vm.rs:453) · `code[ip]` preceded by `ip < len` (vm.rs:364).
- `from_bytes` length-guards the payload slice: `if data.len() < 10 + len` (`crates/llmbc/src/module.rs:133`), so no slice panic.

Suggested severity: **low / informational** — no post-deserialization structural validation exists, but per-access guards cover the gap. `MAX_CALL_DEPTH` (vm.rs:304-306) bounds recursion DoS.

---

## Section 2 — NEW Findings

**N1 [MEDIUM] Wall-clock timeout is a real (opt-in) determinism leak.** `Vm::run` and `execute_bounded` set `start_time = Instant::now()` (`vm.rs:209,231`); `execute()` aborts with `WallTimeExceeded` when `start_time.elapsed() > max_wall_ms` (`vm.rs:336-343`). If `max_wall_ms` is configured, identical input can complete on a fast host and abort on a slow one — violating the "same input → same output" invariant. Default is `None` (vm.rs:116), so off by default, but any configured wall-limit trades determinism for liveness. Should be documented as non-deterministic (or excluded from replay-verified runs).

**N2 [LOW, extends F6] Actor capabilities are declared but have no enforcement point.** `Capability::ActorSpawn`/`ActorSend` exist (bytecode ids 8/9) and are listed in `PolicySet::allow_all` (policy.rs:66-67) and mapped in `effect_to_capability` (executor.rs:134-135), yet at the VM level `Op::SpawnActor`/`Op::SendMsg` never consult any policy (vm.rs:526-546). This is a declared-capability-with-no-enforcement gap. Impact is bounded: actor ops perform no host I/O (they mutate in-VM queues only) and are bounded by the scheduler/step limits — so it is a policy-fidelity gap, not an I/O escape.

**N3 [INFO] Stale/misleading comment in executor.** `executor.rs:164-165` comments the `None =>` arm as "Unsupported effect kind (e.g., SpawnActor)", but `effect_to_capability` returns `Some(ActorSpawn)` for `SpawnActor` (executor.rs:134) — the arm is actually unreachable for actor effects. Cosmetic, not a vuln.

**N4 [INFO] Framework `fn_map` uses HashMap — benign.** `runtime.rs:52` `HashMap<String,u32>` is a name→index lookup, never iterated into output, so no determinism impact. Noted for completeness against the HashMap-iteration hunt.

---

## Section 3 — Could NOT Fully Verify

- **F7 debug-vs-release behavior** confirmed by code inspection (bare `+`/`-`/`*`/`i64::MIN/-1`) but not exercised at runtime. Rust semantics are unambiguous (debug: overflow panic; release: two's-complement wrap; integer div-overflow panics in both profiles), so I rate this CONFIRMED on inspection alone; a runtime PoC was not run (read-only pass).
- **Direct-executor bypass (Claim 1 note)** — I confirmed `HostEffectExecutor::execute` is `pub` and that the in-tree flow always routes through `send()`; I did not exhaustively grep every external crate for a direct `executor.execute(...)` caller outside `send_with_executor`.
- **DNS-rebinding / TOCTOU depth on F3** — confirmed the literal-only check gap statically; did not run a live resolver to demonstrate `169.254.169.254` reach (requires `--live` + network, out of scope for read-only).
