# Verify-B — Distributed Coordinator/Worker Re-Audit (independent, read-only)

Independent verification pass. Every verdict below was reached by opening the cited
`path:line` in this repo (branch `ci/reduce-artifact-storage`) — not by trusting the
prior notes. No code was modified.

Prior sources re-audited: `07-orchestrator-distributed.md` (S1–S15) and `11-security-sweep.md` F5.

---

## Section 1 — Verdict Table

| # | Claim (prior) | Verdict | Evidence |
|---|---|---|---|
| 1 | **S6** Any authenticated worker can complete/fail/extend ANOTHER worker's in-flight step; terminal CAS keys only on `(run_id, step_id, claim_id)` with NO `worker_id`; `claim_id` is a per-step counter from 1 | **CONFIRMED-ACCURATE** | `handle_complete/fail/extend_lease` gate only on `validate_session(&state, &req.worker_id, &req.session_token)` — the *caller's own* session (coordinator.rs:1571, 1599, 1626). `validate_session` only checks `workers.get(worker_id).session_token == session_token` (coordinator.rs:1733-1734). CAS predicate is `current_claim_id == claim_id && status==Running` — **no worker_id** (mod.rs:1954, UPDATE `WHERE run_id AND step_id AND claim_id` at 1977; extend_lease identical at 2079/2088). `claim_step` sets `new_claim_id = current_claim_id + 1` (mod.rs:1607) and schema default is `claim_id INTEGER NOT NULL DEFAULT 0` (schema_v1.sql:68) → **first claim = 1**, fully predictable. `run_id`/`step_id` enumerable. Attack uses attacker's *own* valid session + (under mTLS) own valid cert. |
| 2 | **S9** `/api/runs/{run_id}/approve` gated by the same `auth_middleware`, no per-step token; any bearer/cert holder can approve any HITL gate; contrast `/trigger` which requires a per-step token | **CONFIRMED-ACCURATE (arguably UNDERSTATED — see §2 N1)** | `approve` route mounted under the shared middleware (coordinator.rs:814). `handle_approve_run` takes only `{decision, step_id, reason}` — no token (coordinator.rs:1941-1982). `record_approval_decision_in_store` has **no token/secret parameter** and no submitter check (runner.rs:3980-3986). By contrast `handle_trigger_run` forwards `req.token` (coordinator.rs:2001) and `record_external_trigger_in_store` does a constant-time compare against a per-step stashed token (runner.rs:4266-4285). Approve is a strict authorization downgrade from trigger. |
| 3 | **S1/F5** `auth_middleware` is pass-through with no secret + no mTLS; non-loopback bind only warns; all mutating routes served unauthenticated | **CONFIRMED-ACCURATE** | `auth_middleware`: mTLS check skipped when `!mtls_required`, bearer check skipped when `shared_secret==None`, falls through to `next.run` (coordinator.rs:756-773). Non-loopback + no auth only `eprintln!("[WARNING]…")`, still binds and serves (coordinator.rs:273-280). All of submit/approve/trigger/work-* mounted under this one middleware (coordinator.rs:799-840, single listener at 330-334). |
| 4 | **G2/S12** `blob_store.write()` does NOT verify `hash==SHA256(bytes)`; `complete_step_cas` passes worker `output_hash` straight through | **CONFIRMED-ACCURATE** | `write()` docstring: "The hash is taken as-is and is NOT verified to match the bytes" (blob_store.rs:102-105); body only `validate_hash` (format) then writes (blob_store.rs:118-119). `complete_step_cas` writes `bs.write(output_hash, output_json.as_bytes())` and stores `output_hash` unchanged into the row (mod.rs:1683-1685, 1695). Content-addressing is caller-trusted. |
| 5 (S3) | mTLS client-cert verification genuinely enforced; no route mounted outside the TLS listener | **CONFIRMED-SAFE** | Single listener: `serve_with_tls(listener, app, tls)` xor `axum::serve(listener, app)` on the *same* `app` (coordinator.rs:330-334). `WebPkiClientVerifier` + `with_client_cert_verifier` (coordinator.rs:360-367). Failed handshake `return`s without an HTTP response — connection dropped (coordinator.rs:446-456). Middleware also 401s if `mtls_required` and `ClientIdentity` missing (coordinator.rs:756-758). No bypass found. |
| 5 (S10) | Blob hash path-traversal hardened (64-hex before FS) | **CONFIRMED-SAFE** | HTTP handler validates `hash.len()==64 && all hex` before store access (coordinator.rs:2041-2053); every `BlobStore` method calls `validate_hash` first (blob_store.rs:119,169,186,211). Shard = first 2 already-validated hex chars (blob_store.rs:98). |
| 5 (S11) | Blob route run-scoped; no IDOR across runs | **CONFIRMED-SAFE** | `handle_get_blob` returns bytes only if `run_owns_blob_ref(run_id, hash)` (coordinator.rs:2063-2079); parameterized `COUNT(*) … WHERE run_id=?1 AND output_blob_ref=?2` (mod.rs:1713-1717). 404 does not disambiguate run existence. |
| 5 (S13) | No SQL injection; only `format!`-SQL is `PRAGMA table_info` with hardcoded literal | **CONFIRMED-SAFE** | Only `format!`-into-SQL is `column_exists`'s `PRAGMA table_info({table})` (mod.rs:2156); all 3 callers pass the string literal `"step_checkpoints"` (mod.rs:655,671,687). `column` is compared in Rust, never interpolated. All mutators use `params![…]`. |
| 5 (S14) | Claim/complete/fail concurrency correct (BEGIN IMMEDIATE + CAS, double-claim prevented) | **CONFIRMED-SAFE** | All mutators `BEGIN IMMEDIATE` then SELECT-check-UPDATE under `with_busy_retry` (claim_step mod.rs:1572-1639, terminal_cas_inner 1922-2004, extend_lease_cas 2052-2106). `claim_step` only transitions `Pending→Running` (1604-1607). Late/duplicate terminal writes rejected via `claim_id`-mismatch-or-not-Running → `LeaseExpired` (1954-1958). Sweep uses strict `<` (2024) → HA-safe. |

**Score: 4/4 flagged findings held (S6, S9, S1/F5, G2/S12). 6/6 "SAFE" claims held (S3, S10, S11, S13, S14). Zero overturned.**

---

## Section 2 — NEW findings / refinements

**N1 — S9 is UNDERSTATED: the approval gate is first-writer-wins, so any peer can *pre-empt* the legitimate approver (forced decision + DoS on the gate).** Severity: **High** (reinforces S9).
`record_approval_decision_in_store` rejects a second decision with `StepAlreadyDecided` (runner.rs:4028-4034). Combined with S9 (no approver identity, no per-gate secret), any bearer/worker-cert holder can `POST …/approve {decision:"approved"|"rejected"}` **before** the real approver. The attacker both (a) forces the compliance decision to a value of their choosing and (b) locks out the genuine approver, who then receives `StepAlreadyDecided`. For a platform sold on "policy-gated, auditable approval workflows" this converts S9 from "can approve" into "can unilaterally seize every HITL gate." The audit trail will record the decision as validly bearer-authenticated, with no principal attribution to distinguish attacker from operator.

**N2 — No principal/ownership model anywhere: submit, approve, trigger, and terminal-CAS all conflate every authenticated caller into one identity.** Severity: **High** (systemic root of S6 + S9; not a separate bug so much as the missing control that both exploit).
`handle_submit_run` records **no submitter identity** on the run (coordinator.rs:1774-1817; the store has no owner column). Under mTLS a *worker* client cert satisfies `auth_middleware` for the operator routes too — there is no worker-vs-approver role on the cert (CN is reconciled to `worker_id` only at `register`, and approve/submit/trigger never inspect `ClientIdentity`). Net: with one authorized credential a principal can submit a run, approve its gates, and complete/fail any step of any other run. "IDOR on approve" (Step-2 question) is therefore vacuously true — there is no ownership to violate because none is ever recorded. Fixing S6/S9 in isolation (adding worker_id to the CAS, a per-gate approver secret) is necessary but the real gap is the absent identity/authorization layer.

**N3 — `session_token` is generated/held only in coordinator memory (`workers: HashMap`), so cross-worker hijack (S6) survives a coordinator restart trivially.** Severity: informational (amplifies S6 exploitability).
`validate_session` reads `state.workers` (in-memory, coordinator.rs:1729-1734), which is empty after restart / on a peer coord in the HA `runs.db`-sharing model. An attacker simply re-`register`s (a valid worker action) to obtain a fresh session, then drives terminal-CAS against victim claims. No persistent binding of claim→worker exists to check against, so the token being ephemeral does not raise the bar.

No new SQLi, traversal, or crypto issues found beyond the prior sweep. Blob content-trust (N/A here beyond G2), TLS wiring, and CAS concurrency are genuinely solid.

---

## Section 3 — Could NOT fully verify this pass

- **`dashboard.rs` merged routes** — confirmed they are merged under the same `auth_middleware` (coordinator.rs:790-840) and are GET-only today, but I did not read `dashboard.rs` interior to confirm none mutate or leak cross-run blob refs outside `run_owns_blob_ref`.
- **`handle_register` CN↔worker_id reconciliation** (S7) — not re-read this pass; the S6/S9 conclusions do not depend on it (attacker uses their own registration), but the exact register-time binding was not independently re-confirmed.
- **`WorkflowRunner::submit_with_inline_sources` interior** — verified the handler passes body through with no auth/ownership; did not audit the runner's inline-source persistence for injection into the metadata blob.
- **`record_approval_decision_in_store` tail (4046→end)** and CAS commit path — read through the gate-state checks (status must be `AwaitingApproval`, 4046-4048) and the no-token/no-owner signature; did not read the final metadata write/commit lines.
