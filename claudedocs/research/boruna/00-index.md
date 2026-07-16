# Boruna — Codebase Research Knowledge Base

**Repo:** `/Users/katsarov/htdocs/ai-lang` · **Branch:** `ci/reduce-artifact-storage` · **Workspace version:** 1.9.0
**Index slug (codebase-memory-mcp):** `Users-katsarov-htdocs-ai-lang` · **Date:** 2026-07-16
**Method:** 12 parallel grounded read-only agents (10 subsystem + 2 cross-cutting lenses) → 4 independent verification agents re-checking the highest-severity claims at `path:line`. Every finding cites real code; "SAFE" claims were re-checked hardest.

---

## One-line verdict

Boruna is a **real, substantial, working platform** — the language compiles/runs, all 13 stdlib libs + 5 templates + 11 workflows validate, the capability gateway is a clean choke point, and the crypto/TLS/SQL/path-handling primitives are genuinely well-built — **but its two headline guarantees (tamper-evident evidence, policy-gated approvals) have confirmed High-severity holes**, and the `.ax` language is a thinner MVP than "statically typed" implies. None of this is stub/TODO rot (code is clean); it's design gaps in the security-critical seams plus stale docs.

---

## Navigation map

**Subsystem notes (Phase 1):**
- [[01-frontend-compiler]] — bytecode + compiler (`crates/llmbc`, `crates/llmc`)
- [[02-vm]] — VM, capability gateway, actors, replay (`crates/llmvm`)
- [[03-framework]] — Elm-architecture runtime, policy wiring (`crates/llmfw`) ⚠️ *see Corrections*
- [[04-effect]] — LLM integration, cache, context store (`crates/llm-effect`)
- [[05-orchestrator-core]] — workflow/engine/conflict/patch/simulate (`orchestrator/src`)
- [[06-orchestrator-audit]] — audit log, evidence bundles, encryption, storage backends
- [[07-orchestrator-distributed]] — coordinator/worker HTTP cluster, persistence, mTLS
- [[08-cli]] — the `boruna` binary, dashboard, serve (`crates/llmvm-cli`)
- [[09-dev-tooling]] — tooling + pkg + MCP + LSP
- [[10-language-surface]] — 13 stdlib libs, 5 templates, examples, web dashboard UI

**Cross-cutting lenses (Phase 1):**
- [[11-security-sweep]] — whole-repo security pass
- [[12-gaps-sweep]] — whole-repo gaps + doc-drift

**Verification (Phase 2):**
- [[verify-A-evidence]] — audit/evidence re-audit (4/4 held; +encryption-strip downgrade)
- [[verify-B-distributed]] — coordinator/worker re-audit (4/4 + 6/6 SAFE held; +ownership-model root cause)
- [[verify-C-vm-framework]] — VM + framework re-audit (framework HIGH → MED; F8 → info)
- [[verify-D-gaps-docdrift]] — doc-drift ground-truth (all drift confirmed with real numbers)

---

## Top things to fix / know (severity-ranked, post-verification)

### 🔴 HIGH — the guarantees the product is sold on

1. **Plaintext evidence bundles are not tamper-evident** — the default. `verify_bundle` performs only self-referential checks over an attacker-rewritable `manifest.json`; the audit chain is an **unkeyed** SHA-256 (no HMAC/signature), and `manifest.bundle_hash` is never even checked on the verify path. Anyone with FS write access edits an output, recomputes every checksum + the chain, and `evidence verify` still returns **PASS**. → `audit/log.rs:189`, `audit/verify.rs:89-284`. Source: [[06-orchestrator-audit]] §4.1, [[11-security-sweep]] F1/F2/F9; **verify-A CONFIRMED**.
   - **Even encrypted bundles are defeatable** (verify-A NEW-1): strip the `encryption` block from the manifest, keep the ciphertext, set `file_checksums = SHA256(ciphertext)` → verifies as a valid *plaintext* bundle. The GCM crypto is sound, but nothing *requires* encryption at verify. **Root fix = an external anchor / manifest signature**, not more self-hashing.

2. **Coordinator has no principal/ownership model** (verify-B N2 — systemic root). One authorized credential (shared bearer *or any* worker mTLS cert) can submit a run, approve its human-in-the-loop gates, and complete/fail any step of any other run. Concrete confirmed exploits of this gap:
   - **Fails open by default** — no `--shared-secret` + no mTLS ⇒ auth disabled on *all* mutating routes; non-loopback bind only warns. → `coordinator.rs:737-774`. ([[07-orchestrator-distributed]] S1, [[11-security-sweep]] F5)
   - **Cross-worker claim hijack** — terminal CAS keys on `(run_id, step_id, claim_id)` with **no `worker_id`**; `claim_id` is a predictable per-step counter from 1. Any worker completes/fails/sabotages another's step. → `persistence/mod.rs:1954,1977`. ([[07-orchestrator-distributed]] S6)
   - **Approval-gate seizure** — `/api/runs/{id}/approve` takes no per-gate token and is first-writer-wins, so any peer can **pre-empt** the real approver (force the decision + lock them out with `StepAlreadyDecided`). Contrast `/trigger`, which *does* require a per-step token. → `coordinator.rs:1941-1982`, `runner.rs:4028`. ([[07-orchestrator-distributed]] S9 + verify-B N1)

3. **SSRF in the live HTTP handler** (`--live` + `http` feature). `validate_url_safety` only checks *literal* IP hosts, so (a) a hostname that DNS-resolves to `169.254.169.254`/`127.0.0.1` is fetched; (b) `allow_redirects` defaults true and redirect hops are not re-validated; (c) the entire IPv6-private-range branch (`fc00::`, `fe80::`, IPv4-mapped) is unreachable for real URL hosts. Default empty allowlist = allow-all. Impact: cloud-metadata credential theft. → `http_handler.rs:167-218`, `capability_gateway.rs:53-64`. ([[02-vm]] §4, [[11-security-sweep]] F3/F4; **verify-C CONFIRMED**).

### 🟠 MEDIUM

4. **Framework fail-open policy defaults** *(downgraded from HIGH by verify-C)*. An app that declares no `policies()` — or an empty capability set — silently gets **allow-all** (`policy.rs:106`, `runtime.rs:81-86`); the test named `test_policy_default_is_restrictive` actually asserts the permissive state. The declared PolicySet *does* gate execution via `check_batch` before the executor (so "not wired" was overstated), but the defaults fail open and `HostEffectExecutor::execute` is `pub` (direct-call bypass). → [[03-framework]], verify-C.
5. **Content-addressing is caller-trusted** — `blob_store.write()` does not verify `hash == SHA256(bytes)`; the coordinator stores a worker-supplied `output_hash` (which feeds the audit chain) unchanged. A dishonest worker commits output whose hash lies until replay. → `blob_store.rs:104-118`, `persistence/mod.rs:1683`. ([[07-orchestrator-distributed]] G2/S12)
6. **Stored XSS in `evidence serve`** — bundle fields (step outputs, filenames, errors) are `format!`-interpolated into HTML with **no escaping** (unlike `dashboard.rs`/`serve.rs`, which escape). Inspecting a malicious bundle runs JS in the operator's browser against a `127.0.0.1` origin that also serves `/api/bundle`. → `evidence_serve.rs:152-370`. ([[08-cli]] §4.2)
7. **The `.ax` language is a thin MVP behind a "statically typed" claim** (compiler cluster): the type checker does **no** type/arity checking (`Int + String` compiles); `ensures` postconditions are parsed but **never emitted** (silent no-op); higher-order/indirect calls dispatch to function #0 (broken); enum-variant match tags all collapse to `-1`; `for` loops parse-fail; `Map<K,V>`/`Fn(..)` types are unspellable; a `while`-body trailing expression leaks a stack slot per iteration; `.len() as u8` truncates list/record/call counts > 255. → [[01-frontend-compiler]] G1–G11, S1.
8. **VM correctness/DoS**: unchecked `i64` Add/Sub/Mul → debug-build overflow **panic** (DoS) / release **silent wrap** (breaks the determinism guarantee); `i64::MIN / -1` panics past the div-zero guard. Plus the crate's one production `.expect()` panics on a crafted `SpawnActor` bad function index. Actor opcodes (`SpawnActor`/`SendMsg`) also bypass the capability gateway entirely (bounded — in-VM only, no host I/O). → `vm.rs:526-546,621-688`, `actor.rs:225`. ([[02-vm]] §3/§4, [[11-security-sweep]] F6/F7)
9. **Two latent path issues** (defense-in-depth gaps, not reachable today): storage `ref_to_run_id` allows a single `..` → `get()`’s `remove_dir_all` can wipe the temp dir (no CLI `get` sink exists — library-API-only); MCP `boruna_template_apply` template-name has no `..` sanitization → bounded file-read disclosure. → `audit/storage_s3.rs:418`, `tooling/templates/mod.rs:70`. (verify-A 6.1; [[09-dev-tooling]] S3)
10. **Feature-honesty**: `workflow eval`’s "A/B provider comparison" runs the **same** local VM for both providers unless a real `http` handler is externally wired — the registry only labels. → `workflow_eval.rs:58-68`. ([[08-cli]] G8)

### 🟡 LOW / hygiene
DEK/KEK not zeroized on drop; extra unregistered bundle files invisible to verify; plaintext manifest is a hash-confirmation oracle; `apply_patch` skips `old_text` verification on multi-line edits; package content hash excludes `bytecode/`; `serve.rs` mutex-poison DoS; `--live` silently falls back to mock without `http`; opt-in wall-clock timeout is a determinism leak; `rust_version`/`hostname` in the compliance fingerprint are spoofable/"unknown"; `session_token` sent in the claim URL query string.

### 📄 DOC-DRIFT (cheap, high-value — misleads evaluators; verify-D confirmed with real numbers)
- `README.md:161` "Boruna is at **v1.4.0**" → workspace is **1.9.0** (5 minors stale; CHANGELOG top is 1.4.0 too).
- `README.md:34` "**27 built-in functions**" → actually **33** distinct `__builtin_*`.
- `CLAUDE.md` "**10 MCP tools / 9 crates / 557+ tests / std-llm+std-json 0.1.0 Experimental**" → **12 tools, 11 crates (12 members), ~1613 test fns, both libs 1.0.0**.
- `docs/stability.md:49` "all 13 std-* are 1.0-stable" sits **under an "Experimental" heading** (self-contradiction).
- `libs/std-forms/package.ax.json:5` pins `std.validation "0.1.0"` but std-validation is **1.0.0** (unsatisfiable; harmless today).
- README is **correct** on: 12 MCP tools, 13 stdlib packages.

---

## Corrections (verification overturned or refined these)

- **Framework "PolicySet ≠ VM Policy" HIGH → MEDIUM.** verify-C showed the declared PolicySet *does* gate execution via `check_batch` **before** the executor; the executor's `allow_all()` is a redundant layer behind that gate, not an open door. The real (narrower) issue is fail-open defaults + a `pub` direct-executor edge. A correction banner is on [[03-framework]].
- **Crafted-bytecode panic (F8) HIGH → informational.** `.axbc` *is* loaded from untrusted input (`main.rs:2729`), but every index access is `.get()`-guarded, so no reachable panic. (The separate `actor.rs:225` `.expect()` panic remains real — see finding 8.)
- **"Encrypted ⇒ tamper-evident" understated.** verify-A NEW-1: encrypted bundles are downgrade-defeatable (strip encryption block). The distinction "plaintext bad / encrypted good" is weaker than [[06-orchestrator-audit]] first framed.
- **Storage `..` traversal depth overstated.** Only a *single* `..` level is reachable (`../..` contains `/` and is rejected); and it is latent (no CLI `get` sink). Nonce-reuse at-rest exploitability is also narrower than first stated (overwrite keeps only the last ciphertext).

---

## Verified-SAFE (re-checked, genuinely defended)

Capability gateway is the single choke point — only `Op::CapCall` reaches a real handler, double-gated (static decl + dynamic Policy). VM is bounded (10M steps, 4096 stack, 256 call depth). AES-256-GCM crypto is sound (fresh per-bundle DEK, per-file tag checked, deterministic nonce safe, algorithm pinned). `Envelope::unwrap` is panic-free on crafted input. PatchBundle / BlobStore / LlmCache / ContextStore / package-name path handling all reject `..`/traversal. mTLS is genuinely enforced (`WebPkiClientVerifier`, failed handshake dropped, no `danger_accept_invalid_certs`). Blob route is 64-hex-validated + run-scoped (no traversal, no IDOR). SQL is fully parameterized (only `format!`-SQL is `PRAGMA` with a hardcoded literal). Claim/complete concurrency is correct (`BEGIN IMMEDIATE` + CAS, double-claim prevented, HA-safe). No command injection, no hardcoded secrets. Worker RCE containment (coordinator-supplied `.ax` runs under `MockHandler`). Replay `verify_full` covers all event types. Determinism: `BTreeMap` throughout, no HashMap iteration in execution paths. Web dashboard has no XSS (React text interpolation). Code is clean: zero `todo!()`/`unimplemented!()`.

---

## What was NOT verified this pass
External-integrator callers of `BundleStorage::get` (6.1 real-world reachability); a build script populating `RUSTC_VERSION`; `object_store` internal TLS/SSRF; `dashboard.rs` interior mutation/leak surface; full `tooling/format`, `import_resolver`, `literate`, `migrations`, ITF export; a runtime PoC for the SSRF and integer-overflow findings (all confirmed by inspection, not executed).
