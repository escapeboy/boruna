# Security Review — W6-A (mTLS) + W6-B (bundle encryption)

**Reviewer:** automated security-review pass (Opus 4.7, security-engineer persona)
**Date:** 2026-04-28
**Repo state:** master @ `2a392b3` (merge: W6-A mTLS + per-worker keys)
**Scope:** Sprint W6-A (mTLS auth + per-worker client certificates) and Sprint W6-B
(evidence bundle envelope encryption with AES-256-GCM). Both surfaces ship in v1.0 GA
and are LTS-protected from 1.0 forward.

## 1. Executive summary

Both sprints land with strong cryptographic primitives (rustls 0.23 + `aws_lc_rs`,
AES-256-GCM via the `aes-gcm` crate, OS CSPRNG via `getrandom`) and adequate
adversarial test coverage. mTLS uses `WebPkiClientVerifier` correctly; bundle
encryption uses authenticated encryption with a fresh per-bundle DEK and proper
AAD-bound DEK wrapping. **No HIGH-severity findings.** Several MEDIUM findings are
worth resolving before GA — primarily documentation/spec gaps for the LTS contract
on encryption fields, a missing CRL/OCSP path that should be flagged as an
acknowledged limitation, and one small UX gate (`inspect --decrypt`) that is
currently a no-op surface (the gate is wired but inspect never prints decrypted
bodies, so the wiring is correct but advertised as future-proofing). LOW findings
are mostly hardening polish.

Counts: **HIGH 0 / MEDIUM 6 / LOW 7.**

## 2. HIGH findings

None.

## 3. MEDIUM findings

### **MEDIUM-1** — Evidence bundle 1.0 spec does not document the `encryption` field

- **File:** `docs/spec/evidence-bundle-1.0.md` (entire file — no encryption section)
- **Code that depends on it:** `orchestrator/src/audit/evidence.rs:42-60` (manifest field), `orchestrator/src/audit/encryption.rs:47-63` (`EncryptionInfo`).
- **Issue:** The frozen 1.0 spec does not mention the `encryption` block, AES-256-GCM,
  the per-file nonce derivation, the AAD-bound DEK wrap, or the new
  `evidence.encryption_key_required / evidence.encryption_key_mismatch /
  evidence.cipher_tag_invalid` error kinds. The 1.x LTS contract (`docs/lts.md`
  §B.3) commits to "Evidence bundle format 1.x — every 1.0 bundle is verifiable
  byte-identically by every 1.y reader" — but a 1.0 reader that does not know
  about `encryption` will (correctly) ignore it as an unknown field per §1 of the
  spec, then fail integrity verification because the on-disk bytes hash to the
  ciphertext, not the plaintext referenced by `file_checksums`. The compat story
  needs to be written down BEFORE GA so downstream readers (Python decoders,
  third-party verifiers) implement it consistently. Pre-W6-B 1.0-rc1 readers
  already exist in the wild.
- **Exploit / failure scenario:** Auditor with a third-party 1.0 reader receives a
  W6-B-encrypted bundle and gets a "checksum mismatch" with no actionable message
  pointing them at the encryption field. Audit signoff blocks. (Not exploitable
  by an attacker; this is a compliance/operational risk.)
- **Recommended fix:** Add a §8 "Optional encryption envelope (1.x additive)" to
  `docs/spec/evidence-bundle-1.0.md` describing: the `encryption` block JSON shape,
  the algorithm/kek_id/wrapped_dek/wrapped_dek_nonce/files fields, the per-file
  nonce derivation, the AAD-binding of the DEK wrap, the replay-verified vs.
  operational classification (already covered in `docs/design-bundle-encryption.md`
  but the design doc is internal), and the three new error kinds. Also document
  reader behavior: a 1.0 reader that doesn't know about encryption MUST report
  "checksum mismatch" with a hint to upgrade.

### **MEDIUM-2** — New `error_kind` values are not in the documented stable taxonomy

- **File:** `docs/reference/policy-schema.md:120-133` lists the stable
  `error_kind` taxonomy for policy errors; there is no equivalent canonical list
  for the coord HTTP surface or the evidence bundle surface. The new W6 strings
  `coord.identity_mismatch`, `evidence.encryption_key_required`,
  `evidence.encryption_key_mismatch`, `evidence.cipher_tag_invalid` only appear in
  the design docs (`docs/design-coord-mtls.md:222`,
  `docs/design-bundle-encryption.md:147-151`).
- **Impact:** §B.4 of the LTS contract (`docs/lts.md`) commits to stable
  `error_kind` strings. New strings are added in W6 but not added to a
  reference-grade taxonomy file, so an integrator switching on them today has
  only the design-doc string to rely on. Renaming or repurposing later breaks
  silently.
- **Recommended fix:** Add a `docs/reference/error-kinds.md` (or extend
  `docs/reference/cli.md`) that lists every stable `error_kind` the binary emits,
  including the four new W6 entries. Cross-link from `docs/lts.md` §B.4.

### **MEDIUM-3** — TLS 1.2 still enabled; no explicit cipher-suite pinning

- **File:** `Cargo.toml:39` (`features = ["std", "tls12", "aws_lc_rs"]`),
  `Cargo.toml:41` (tokio-rustls with `tls12`).
- **Issue:** rustls 0.23's `aws_lc_rs` provider with `tls12` enabled accepts
  TLS 1.2 alongside TLS 1.3. For a 1.0 GA compliance-sensitive product the
  default ought to pin TLS 1.3 only, or at least the design doc should justify
  the 1.2 inclusion. rustls 0.23 already excludes TLS 1.0/1.1 entirely
  (a positive — confirmed by the absence of `tls10`/`tls11` features) and the
  `aws_lc_rs` provider only ships strong cipher suites (AEAD-only, no RC4, no
  3DES, no CBC-without-AEAD). The current configuration is NOT weak — but the
  posture deserves an explicit decision for an auditor.
- **Recommended fix:** Either drop the `tls12` feature in `Cargo.toml:39,41` and
  document TLS 1.3 only as the v1 posture, OR add a paragraph to
  `docs/design-coord-mtls.md` explaining why TLS 1.2 is permitted (operator-side
  compat with older client toolchains) and that strong AEAD ciphers are enforced
  by `aws_lc_rs`. Either is fine; the silent default is the issue.

### **MEDIUM-4** — No client-cert revocation check (no CRL, no OCSP, no short-lived cert enforcement)

- **File:** `crates/llmvm-cli/src/coordinator.rs:340-367` (`build_server_tls`
  uses `WebPkiClientVerifier::builder(roots).build()` with no revocation
  configuration).
- **Issue:** `rustls-webpki` supports CRL distribution points via
  `ClientCertVerifierBuilder::with_crls(...)`. The current configuration trusts
  any cert chained to the configured CA for the cert's full validity period.
  An exfiltrated worker private key is valid until natural expiry. There is also
  no enforcement of short-lived certs (e.g. ≤24h validity).
- **Exploit:** Worker host compromise leaks the per-worker private key. The
  attacker uses it from any network-reachable position to claim work as that
  worker indefinitely (until cert expiry), bypassing the per-worker identity
  story. Operators must either rotate the CA or wait for natural expiry.
- **Recommended fix:** v1 posture is documented as "operator owns cert renewal"
  (`docs/design-coord-mtls.md:46-48`). For GA, ADD an explicit "Limitations"
  subsection in `docs/guides/coord-mtls.md` listing: (a) no CRL check, (b) no
  OCSP check, (c) recommended cert lifetime ≤24h with automated renewal via
  step-ca / cert-manager. Track CRL support in the 0.6.x roadmap.

### **MEDIUM-5** — Case-insensitive CN match uses ASCII-only comparison; not Unicode-normalized

- **File:** `crates/llmvm-cli/src/coordinator.rs:1141-1152` (uses
  `cn.eq_ignore_ascii_case(body_id)`).
- **Issue:** `eq_ignore_ascii_case` only folds ASCII A-Z ↔ a-z. The design doc
  (`docs/design-coord-mtls.md:155-156`) explicitly assumes "CNs are typically
  ASCII so no special Unicode normalization is needed." For a strict ASCII
  policy this is correct — but there is no enforcement of an ASCII-only CN at
  cert validation time. A CA could legitimately issue a cert with CN
  `worker-Я` (Cyrillic) and a body_id of `worker-Я` (matching), where the
  ASCII-fold leaves them byte-equal and the comparison passes. That is fine.
  The corner case is a CN containing a Latin-script lookalike that an
  attacker could exploit if they could ALSO obtain a cert with a matching
  homoglyph CN — but that requires CA cooperation, in which case the attacker
  has a far easier path. So the practical risk is LOW; the CONCERN is that the
  "ASCII assumption" is undocumented at the validation point.
- **Recommended fix:** Add a comment at line 1143 noting the ASCII-only fold
  semantics and recommending operators issue ASCII CNs via the CA's profile.
  Optionally, reject non-ASCII CNs at the listener with a typed error.

### **MEDIUM-6** — `inspect --decrypt` flag is wired but never reads decrypted content

- **File:** `crates/llmvm-cli/src/main.rs:3392-3401`.
- **Issue:** The `--decrypt` flag is parsed and `bundle_encryption_key` is
  collected, but the inspect path never reads or prints decrypted file bodies
  (it only prints manifest-level fields, which are plaintext by design). The
  `let _ = (decrypt, bundle_encryption_key);` line silently discards the inputs,
  and the conditional warning is shown only when `decrypt == false`. This is
  documented as future-proofing in the comment ("wire it now so the surface
  lands with the encryption sprint"), and the LTS commitment is to NOT remove
  flags. So functionally this is correct. The risk is that a future maintainer
  adds plaintext printing without realizing the gate has never been exercised
  end-to-end, and accidentally prints content even when `--decrypt=false`.
- **Recommended fix:** Replace the discarded `let _` with a typed
  `decrypt_requested: bool` capture that gates a future plaintext-print branch,
  AND add a unit test that asserts the inspect command in encrypted mode WITHOUT
  `--decrypt` does not exec any decrypt path. Today no such test exists; the
  integration test in `orchestrator/tests/bundle_encryption.rs:182-206` only
  asserts the manifest-level read.

## 4. LOW findings

### **LOW-1** — KEK hex parsing is not constant-time

- **File:** `orchestrator/src/audit/encryption.rs:241-256`. `parse_kek_hex` uses
  `u8::from_str_radix` per nibble pair with early-return on non-hex.
- **Risk:** Negligible — a KEK is parsed exactly once at startup from operator
  input (env or flag); a network attacker has no influence on the bytes parsed.
  No constant-time guarantee is needed.
- **Note for the record:** documenting this as "intentionally not constant-time;
  KEK parsing is not in an attacker-observable timing path" would close the
  question for an auditor.

### **LOW-2** — `cn_from_cert_der` returns the first CN attribute; multi-CN certs are silently truncated

- **File:** `crates/llmvm-cli/src/coordinator.rs:606-633`.
- **Issue:** A subject DN can technically contain multiple CN attributes
  (rare, but legal). The parser returns the first one it encounters.
- **Risk:** Low — `WebPkiClientVerifier` accepts multi-CN certs; an attacker
  who controls a CA could issue a cert with `CN=worker-A, CN=admin` to claim
  worker-A identity while accidentally smuggling a different identifier.
  The matching logic compares against the first CN, so the attack does not
  succeed via this path. But a defensive parser would either reject multi-CN
  or use the SAN dNSName instead.
- **Note:** Document that the first-CN policy is intentional and that operators
  should issue single-CN certs.

### **LOW-3** — `EncryptionInfo` files list is operational metadata; cannot be tampered to bypass decryption

- **File:** `orchestrator/src/audit/encryption.rs:60-63`,
  `orchestrator/src/audit/evidence.rs:165-171`.
- **Confirmed safe:** `encryption.files` is purely informational. The verify
  loop iterates `manifest.file_checksums` (replay-verified) and decrypts every
  entry that has a present envelope. Tampering `encryption.files` after the
  fact does not change what gets decrypted or hashed. This is a documentation
  win — but worth a one-line assertion in the design doc that "tampering
  encryption.files has no integrity impact."

### **LOW-4** — `verify.rs:226` swallows decrypt errors on `audit_log.json` to avoid double-reporting

- **File:** `orchestrator/src/audit/verify.rs:218-228` —
  `env.decrypt_file("audit_log.json", &raw).unwrap_or_default()`.
- **Issue:** On AES-GCM tag failure for `audit_log.json`, the decrypt error is
  intentionally consumed (the error already surfaces via the `file_checksums`
  loop, line 195). If `audit_log.json` is the FIRST file iterated and a future
  refactor removes `audit_log.json` from `file_checksums` (it is currently
  always added), the tag failure would be silently masked.
- **Recommended fix:** Add a comment + a defensive `assert!` that
  `audit_log.json` is in `file_checksums` (it is, today, via `write_file` in
  `evidence.rs:154`), OR refactor to track the decrypt error explicitly and
  ensure at least one `evidence.cipher_tag_invalid` is emitted.

### **LOW-5** — `Envelope::encrypt_file` panics on AES-GCM error rather than returning Result

- **File:** `orchestrator/src/audit/encryption.rs:208-211`. The `expect("AES-GCM
  encrypt should not fail on bounded input")` is per the comment — AES-GCM only
  fails on extreme allocation. Acceptable for v1; flag for completeness. A
  future refactor encrypting larger streamed payloads would need to revisit.

### **LOW-6** — Health-check route bypasses both mTLS identity check and bearer auth

- **File:** `crates/llmvm-cli/src/coordinator.rs:744-746`. `/api/health`
  bypasses auth entirely (W2 design). With mTLS enabled, the TLS handshake
  still requires a valid client cert (rustls layer is BEFORE axum), so an
  attacker without a cert cannot reach `/api/health`. With bearer-only, the
  health probe is intentionally public. Confirmed safe; documenting here for
  the auditor.

### **LOW-7** — `eprintln!` of registered worker_id contains attacker-influenced data

- **File:** `crates/llmvm-cli/src/coordinator.rs:1164` (`coordinator:
  registering worker '{worker_id}' via mTLS cert`). The `worker_id` comes from
  the cert CN and is therefore attacker-controlled if the attacker controls a
  CA-signed cert. Risk is log-injection (newline in CN → forged log lines).
- **Fix:** Use `{worker_id:?}` instead of `'{worker_id}'` to debug-format the
  string with quoting + escape sequences. Same fix at any other `eprintln!`
  that interpolates worker_id (none others identified in this review).

## 5. Convention checks

### §1 — Reject at parse, don't silently override

**PASS** for both surfaces.

- `ServerTlsPaths::from_optional` rejects partial flag sets at startup
  (`coordinator.rs:150-164`); test
  `mtls_partial_flags_are_rejected_at_startup` confirms.
- `ClientTlsPaths::from_optional` (`worker.rs:34-49`) symmetric.
- `--encrypt-bundle` without `--record` rejected at parse (`main.rs:2483-2487`).
- `--encrypt-bundle` without a KEK source rejected at parse
  (`main.rs:2652-2659`).
- Encryption algorithm is hardcoded to `aes-256-gcm`; future algorithms would
  require an explicit constant change. No silent fall-through to a weaker
  algorithm.

### §2 — Typed `error_kind` strings, stable

**PARTIAL PASS** — see MEDIUM-2. The strings exist and are emitted consistently;
they just are not enumerated in a reference taxonomy file.

- `coord.identity_mismatch` emitted at `coordinator.rs:1144-1152`, asserted in
  test `handle_register_rejects_cn_worker_id_mismatch` (`coordinator.rs:3720-3749`)
  and integration test `mtls_cn_mismatch_returns_identity_mismatch`
  (`cli_coordinator_mtls.rs:380-408`).
- `evidence.encryption_key_required` emitted at `verify.rs:151-155`, asserted
  in `bundle_verify_fails_with_no_kek` (`bundle_encryption.rs:111-134`).
- `evidence.encryption_key_mismatch` emitted at `verify.rs:164-168`, asserted
  in `bundle_verify_fails_with_wrong_kek` (`bundle_encryption.rs:91-108`).
- `evidence.cipher_tag_invalid` emitted at `verify.rs:196-199`, asserted in
  `bundle_tampered_ciphertext_fails_at_verify` (`bundle_encryption.rs:137-161`).

### §15 — Replay-verified vs operational metadata classification

**PASS.**

- `encryption.algorithm`, `kek_id`, `wrapped_dek`, `wrapped_dek_nonce` all live
  inside the manifest JSON that feeds `bundle_hash` (replay-verified). Confirmed
  by code-walk of `evidence.rs:189-192` (`sha256_str(&manifest_json)`) — the
  manifest is serialized BEFORE `bundle_hash` is computed, with the encryption
  block already present (line 186 sets `encryption: encryption_info` in the
  intermediate manifest).
- `encryption.files` is operational only; it is part of the same manifest and
  therefore IS in fact in the bundle hash today (contradicting the design doc's
  classification claim at `docs/design-bundle-encryption.md:115-120`). **This
  is a doc/code mismatch worth flagging as a NIT** but functionally harmless
  — `files` is sorted+deduped before insertion (`evidence.rs:166-169`), so it
  is deterministic.
- `advertised_capabilities` (W3-A) classified as operational at
  `coordinator.rs:856-861`; confirmed not in `capability_set_hash`.

**Documentation NIT for §15:** the design doc claims `encryption.files` is
operational/not-in-hash, but the implementation puts the whole `EncryptionInfo`
including `files` into the manifest that is then hashed. The classification
should be updated to "all encryption fields are replay-verified" OR `files`
should be moved to a separate operational sidecar. Recommend the former since
the field is deterministic and the bundle-hash invariant is cheaper to reason
about when "everything in the manifest is hashed."

### §29 — Adversarial cases as unit tests

**PASS.**

- mTLS: 5 integration tests cover (1) no client cert, (2) wrong-CA client cert,
  (3) valid cert + CN drives identity, (4) CN/body mismatch, (5) partial flags
  rejected at startup. Plus unit tests for `cn_from_cert_der` (round-trip),
  `auth_middleware` rejecting when mTLS required + no identity, and
  `handle_register` rejecting CN/body mismatch.
- Bundle encryption: 7 integration tests in `bundle_encryption.rs` cover
  round-trip, wrong KEK, missing KEK, tampered ciphertext, plaintext
  backwards-compat, inspect refusal contract, format-version invariant. Plus
  6 unit tests in `encryption.rs` covering DEK wrap/unwrap, tag tampering,
  filename-binding (per-file nonce), and KEK-hex parse errors.

## 6. Out-of-scope follow-ups

These are real concerns but require their own sprint and are out of scope for
the W6 GA gate:

1. **CRL / OCSP support** for client cert revocation. (MEDIUM-4.) Track in
   0.6.x roadmap.
2. **KMS / cloud-KMS integration** for KEK retrieval (currently env / flag
   only). Documented as a non-goal in `docs/design-bundle-encryption.md:24`.
3. **Key rotation tooling** (`boruna evidence rotate-key`). Documented as
   non-goal in design doc; the manifest shape supports it.
4. **TLS 1.3-only enforcement**. (MEDIUM-3.)
5. **Streaming encryption** for very large bundles. Documented as non-goal.
6. **Built-in CA tooling** for cert provisioning. Operators use step-ca /
   cfssl / openssl per the dev recipe.
7. **Memory zeroization** of the DEK and KEK after use. Today, `dek` is a
   `[u8; 32]` array on the stack/heap; `Drop` does not zeroize. The `aes-gcm`
   crate's `Aes256Gcm` struct similarly does not zeroize on drop. For
   compliance scenarios that demand RAM hygiene, a future hardening pass with
   `zeroize::Zeroizing<[u8; 32]>` is warranted.
8. **rcgen as dev-only dep** — confirmed correct: `rcgen` is in
   `crates/llmvm-cli/Cargo.toml:67` under `[dev-dependencies]`, and is the
   only crate that depends on it (workspace dep at `Cargo.toml:43`). The
   production binary does NOT pull `rcgen`.
9. **Crypto provider mixing (`aws_lc_rs` + `ring` via reqwest)** — operationally
   benign per the design doc; both providers ship strong AEAD-only suites.
   Worth a one-line note in `docs/design-coord-mtls.md` confirming the
   review accepted the mixed-provider posture.

---

## Appendix A — files reviewed

W6-A:
- `crates/llmvm-cli/src/coordinator.rs` (lines 50-633, 700-770, 1096-1190, 3650-3768)
- `crates/llmvm-cli/src/worker.rs` (entire file)
- `crates/llmvm-cli/src/main.rs` (W6 flag declarations + handlers)
- `crates/llmvm-cli/tests/cli_coordinator_mtls.rs` (entire file)
- `crates/llmvm-cli/Cargo.toml` (rcgen classification)
- `Cargo.toml` (rustls features)
- `docs/design-coord-mtls.md`, `docs/guides/coord-mtls.md`

W6-B:
- `orchestrator/src/audit/encryption.rs` (entire file)
- `orchestrator/src/audit/evidence.rs` (entire file)
- `orchestrator/src/audit/verify.rs` (entire file)
- `orchestrator/tests/bundle_encryption.rs` (entire file)
- `crates/llmvm-cli/src/main.rs` (encrypt-bundle / decrypt flow)
- `docs/design-bundle-encryption.md`, `docs/spec/evidence-bundle-1.0.md`,
  `docs/lts.md`

## Appendix B — finding tags for downstream filtering

```
HIGH: 0
MEDIUM: 6 (MEDIUM-1, MEDIUM-2, MEDIUM-3, MEDIUM-4, MEDIUM-5, MEDIUM-6)
LOW: 7 (LOW-1, LOW-2, LOW-3, LOW-4, LOW-5, LOW-6, LOW-7)
```

`ce-pr-comment-resolver` may filter on the bold `**HIGH**` / `**MEDIUM**` /
`**LOW**` markers in the headings under §2/§3/§4.

---

## W7 follow-up review (2026-04-28)

**Reviewer:** automated security-review pass (Opus 4.7, security-engineer persona)
**Date:** 2026-04-28
**Repo state:** master @ `0cda1fd` (merge: W7-Docs spec + taxonomy + mTLS doc polish)
**Scope:** verify closure of the six MEDIUM findings raised in the W6 review and surface any new issues introduced by W7. W7 surface: 1 code commit (`c14f7ae`, +1 test file, +stdlib doc note) and 1 docs commit (`fc29066`, spec amendment + new taxonomy file + mTLS guide additions + CHANGELOG `Decided` entry).

### 1. Closure status of W6 MEDIUM findings

#### M-1 — Evidence bundle 1.0 spec documents `encryption` block — ⚠️ PARTIAL

`docs/spec/evidence-bundle-1.0.md:130-197` adds §8 with the field shape, replay-verified vs. operational classification (§15-compliant), reader contract, and the §1 reject-at-parse contract. The 1.0-without-encryption fallback story (§8.2 last paragraph, lines 191-197) is correctly written.

**Closure gap (also see N-1 below):** §8.1 lines 170-171 commit the reader to "MUST reject `encryption.algorithm` values other than `\"aes-256-gcm\"` with `evidence.unsupported_algorithm`" — but the implementation in `orchestrator/src/audit/verify.rs:130-180` and `orchestrator/src/audit/encryption.rs:152-197` (`Envelope::unwrap`) NEVER reads `info.algorithm`. A bundle with `algorithm: "chacha20"` and otherwise-valid AES-GCM-wrapped DEK would be accepted by the current reader. The spec promises behavior the code does not exhibit.

The §1 reject-at-parse compliance for encryption fields IS otherwise good (lines 180-189) — wrapping/length errors do reject — but the algorithm gate is missing.

#### M-2 — Canonical `error_kind` taxonomy — ⚠️ PARTIAL

`docs/reference/error-kinds.md` exists, lists 19 `coord.*` (matched 1:1 against `crates/llmvm-cli/src/coordinator.rs` literals at lines 711, 1018, 1624, 1668, 1958, 1983, 2035 and `crates/llmvm-cli/src/worker.rs:221,358,440`), 4 `evidence.*`, 3 `workflow.*` (matched against `orchestrator/src/workflow/definition.rs:34-40`), 6 `policy.*` (matched against `crates/llmvm/src/policy_validate.rs:100-141`), and 3 MCP-layer kinds. HTTP statuses, sprint of origin, file:function references all present. LTS commitment language is at lines 7-17.

**Closure gap (also see N-2 below):** the doc claims at lines 21-25 that "every entry below is verified against a literal-string grep of the `crates/` and `orchestrator/` trees" — the grep is NOT actually exhaustive. The MCP-layer section omits 7 `error_kind` literals that are emitted by the production binary today:
- `framework_error` — `crates/boruna-mcp/src/tools/framework.rs:43,68,100`
- `runtime_error` — `crates/boruna-mcp/src/tools/run.rs:246,1108`
- `validation_failed` — `crates/boruna-mcp/src/tools/run.rs:520`
- `template_error` — `crates/boruna-mcp/src/tools/template.rs:35,90`
- `invalid_args` — `crates/boruna-mcp/src/tools/template.rs:55`
- `parse_error` — `crates/boruna-mcp/src/tools/workflow.rs:15`
- `validation_error` — `crates/boruna-mcp/src/tools/workflow.rs:51`
- `serialization_error` — `crates/boruna-mcp/src/tools/compile.rs:51`
- `limit_exceeded` — `crates/boruna-mcp/src/tools/run.rs:347` (this one IS referenced inline in the `boruna_run` description at `crates/boruna-mcp/src/server.rs:58,204` and gets a passing mention in the prose at `error-kinds.md:113`, but is not in the LTS-protected taxonomy table).

`evidence.unsupported_algorithm` is listed as `(reserved)` at line 69 — see N-1: under §B.6 LTS commitment, this is a promise that the CODE does not yet keep, so the doc is technically correct ("reserved string") but the spec amendment in M-1 commits to the reader emitting it. The two artifacts contradict each other on the same release.

#### M-3 — TLS 1.2 GA decision in CHANGELOG — ✅ CLOSED

`CHANGELOG.md:56-68` adds a `### Decided` entry naming rustls 0.23 + `aws_lc_rs`, AEAD-only ciphers, ECDHE key exchange, and the rationale (older HTTP load balancers / worker hosts). The decision is grounded — `aws_lc_rs`'s TLS 1.2 cipher suite list is in fact AEAD-only (no CBC, no RC4, no export) and the wording does not overclaim. Re-evaluation at 2.0 is committed.

#### M-4 — No-CRL/OCSP limitation documented — ✅ CLOSED

`docs/guides/coord-mtls.md:198-220` adds a "Limitations" section with a "No CRL or OCSP" subsection (lines 200-206) and a "Mitigation: short-lived certs" subsection (lines 208-220) recommending ≤24h cert lifetimes and naming step-ca / cfssl / cron with operator's existing PKI. CRL/OCSP support is committed to the post-1.0 roadmap (line 220). This is actionable, not just a note.

#### M-5 — CN comparison case-fold semantics documented — ✅ CLOSED

`docs/guides/coord-mtls.md:83-99` adds a "CN comparison semantics" subsection. The doc explicitly names `eq_ignore_ascii_case`, walks through ASCII-case-folds-as-expected and non-ASCII-is-case-sensitive, and gives operators a clear two-option recommendation (lines 95-99): same case in cert + config, OR ASCII-only CNs. Correctly grounded against `crates/llmvm-cli/src/coordinator.rs:1141-1152`.

#### M-6 — Inspect plaintext-leak gate test — ✅ CLOSED

`crates/llmvm-cli/tests/cli_evidence_inspect_decrypt.rs:1-176` ships two tests:

- `inspect_does_not_leak_plaintext_without_decrypt_flag` (lines 65-124): builds a real encrypted bundle via `EvidenceBundleBuilder::with_encryption` (line 41), embeds a fixed canary string in a step output (line 50), pre-asserts the canary IS NOT present in on-disk ciphertext (lines 75-81 — guards against the bundle silently being unencrypted), runs the production `boruna evidence inspect` binary, and asserts the canary appears in NEITHER stdout (line 105) NOR stderr (line 110). Also pins the operator-discoverability hint "is encrypted" at line 120.
- `inspect_decrypt_flag_is_inert_when_payload_display_unimplemented` (lines 126-176): pins current behavior so a future change adding payload preview is forced to revisit both assertions.

Convention §29 (adversarial cases as unit tests) is satisfied. The canary is a real plaintext that genuinely lives in the encrypted file; the bundle is built via the real encryption builder, not a mocked manifest.

### 2. New findings introduced by W7

#### **NEW-1 (MEDIUM)** — Spec promises an unimplemented reader behavior for `evidence.unsupported_algorithm`

- **File:** `docs/spec/evidence-bundle-1.0.md:170-171` (§8.1 reader contract).
- **Code that should implement it:** `orchestrator/src/audit/verify.rs:130-180` (envelope resolution path) and `orchestrator/src/audit/encryption.rs:152-197` (`Envelope::unwrap`).
- **Issue:** The spec says a 1.x reader "MUST reject `encryption.algorithm` values other than `\"aes-256-gcm\"` with `evidence.unsupported_algorithm`". The reader code never reads `info.algorithm`. Forwarding a manifest with `algorithm: "rot13"` and a real AES-GCM-wrapped DEK would be silently processed. Not exploitable today (only AES-GCM is implemented in `Envelope::wrap`/`encrypt_file`, so only AES-GCM bundles exist), but if a future sprint adds a second algorithm and forgets to update `unwrap`, the spec's algorithm gate is missing entirely. Worse: a malicious/buggy producer could write a manifest with the wrong algorithm string and the reader would happily decrypt with AES-GCM, producing a "valid" bundle whose audit trail says it's a different cipher.
- **Fix:** add an early `if info.algorithm != ALGORITHM { return Err(...) }` in `Envelope::unwrap` (`orchestrator/src/audit/encryption.rs:152`) and surface the failure as a new `EncryptionError::UnsupportedAlgorithm { found: String }` variant; promote `evidence.unsupported_algorithm` from `(reserved)` → wired in `docs/reference/error-kinds.md:69`. ALTERNATIVELY: weaken the spec wording in `evidence-bundle-1.0.md:170-171` from MUST to "is reserved for future algorithms; a 1.x reader MAY reject any value other than `\"aes-256-gcm\"`" to match current behavior, then promote to MUST when the gate ships. The first option is correct for an LTS contract; the second is the smaller diff but punts the work.
- **Convention impact:** §1 (reject at parse, don't silently override) is currently violated for the `algorithm` field — the reader silently accepts unknown algorithms.

#### **NEW-2 (MEDIUM)** — Taxonomy claims completeness but omits 8+ MCP-layer `error_kind` strings

- **File:** `docs/reference/error-kinds.md:21-25` (claim) and `docs/reference/error-kinds.md:106-117` (MCP-layer table).
- **Issue:** the doc explicitly states "every entry below is verified against a literal-string grep of the `crates/` and `orchestrator/` trees". The MCP-layer table lists 3 entries (`invalid_policy`, `invalid_output_schema`, `unsupported_limit`). A literal grep finds 8 additional MCP-emitted strings (enumerated under M-2 above) that are emitted by `crates/boruna-mcp/src/tools/{framework,run,template,workflow,compile}.rs`. Under the LTS contract committed at lines 7-17, these strings — once shipped in 1.0 GA — are STABLE FOREVER. Operators relying on the doc and switching only on the 3 listed kinds will get unhandled-error fallthroughs for the other 8.
- **Risk:** moderate. The 8 missing kinds are NOT under-spec'd in the source: they ARE emitted today and their renaming would be a breach of the LTS contract. The risk is the OPPOSITE direction — Boruna may accidentally rename one of them in a 1.x patch release because the maintainer didn't realize it was LTS-protected (the doc said the catalogue was complete, so anything missing must be unstable).
- **Fix:** either (a) add the missing 8 kinds to the MCP-layer table at `error-kinds.md:106-117`, OR (b) explicitly mark the MCP-layer kinds as not-LTS-protected in §B.6 of `docs/lts.md` and add a "future stabilization" note in `error-kinds.md`. (a) is the correct posture for GA. (b) is acceptable if there's an active intent to redesign the MCP error surface in 1.x — but that intent is not documented anywhere I checked.
- **Convention impact:** §2 (typed `error_kind`, stable forever) — partially satisfied at the surface but the LTS guarantee is silently weaker than the doc claims.

#### **NEW-3 (LOW)** — `EncryptionInfo::files` classified inconsistently (carryover from W6 §15 NIT)

- **Files:** `docs/spec/evidence-bundle-1.0.md:147,151-155` ("`files` is OPERATIONAL metadata") and W6 review §5 (the doc/code mismatch noted there is now codified in the spec).
- **Issue:** the spec amendment at line 151 calls `files` operational, while the W6 §15 review confirmed it IS in fact in the bundle hash (because the whole `EncryptionInfo` is serialized into the manifest before hashing). The spec is still strictly correct because it says "tampering with `files` cannot bypass decryption" — but a precise reader of §15 (replay-verified vs. operational) would interpret "operational" as "not in `bundle_hash`", which is wrong for `files`.
- **Risk:** documentation-only. No exploitable surface.
- **Fix:** clarify line 151 to say "`files` is informational metadata for human inspection; it participates in `bundle_hash` because it lives inside `manifest.json`, but the verify loop iterates `manifest.file_checksums` and so tampering with `files` cannot bypass decryption."

### 3. Convention re-check

- **§1 (reject at parse, don't silently override)** — **PASS for spec text, FAIL for code.** The spec amendment at `evidence-bundle-1.0.md:178-189` correctly applies §1 to the encryption metadata. The code at `orchestrator/src/audit/encryption.rs:152-197` does NOT reject unknown `algorithm` values — see NEW-1.
- **§2 (typed `error_kind`, stable forever)** — **PARTIAL.** Closure of M-2 advances the convention but does not fully satisfy it; see NEW-2 (MCP layer omissions).
- **§15 (replay-verified vs. operational classification)** — **PASS with NIT.** The spec amendment at `evidence-bundle-1.0.md:149-155` correctly classifies the four bundle-hash fields as replay-verified. The `files` field carryover (NEW-3) is a doc precision issue, not a violation.
- **§29 (adversarial cases as unit tests)** — **PASS.** The M-6 test at `crates/llmvm-cli/tests/cli_evidence_inspect_decrypt.rs:65-176` is well-constructed: real encrypted bundle, real CLI process, sanity-check on the on-disk ciphertext, both stdout AND stderr asserted, and a regression-pinning second test. Better than the bar set by `bundle_encryption.rs`.

### 4. GA readiness recommendation

**Verdict: do NOT cut 1.0 GA on the current `master`. Two MEDIUM findings (NEW-1, NEW-2) materially impact LTS commitments and should be resolved first.**

What blocks GA, in priority order:

1. **NEW-1 (algorithm gate)** — the spec promises rejection of unknown `algorithm` values; the reader does not enforce it. Either ship the 5-line gate in `Envelope::unwrap` plus the new `EncryptionError::UnsupportedAlgorithm` variant and the new `error_kind` literal (preferred: makes `evidence.unsupported_algorithm` real, not "(reserved)"), OR weaken the spec wording from MUST to MAY and document that the gate ships in 1.1. Either fix is small. Shipping with the contradiction means a 1.x reader written to spec will diverge from the reference Boruna reader.
2. **NEW-2 (MCP error_kind taxonomy completeness)** — adding 8 missing rows to `docs/reference/error-kinds.md` is a 5-minute documentation change. Pre-GA this is the right place to make it; post-GA those strings are LTS-frozen and the doc is misleading.

Lower-priority (do not block GA, but track for 1.1):

- NEW-3 (`files` field §15 wording precision).
- The M-1 spec/code algorithm gap will resolve itself when NEW-1 is fixed.
- The M-2 partial closure resolves cleanly when NEW-2 is fixed.

After those two are in, M-1 and M-2 transition from PARTIAL → CLOSED, all six W6 MEDIUMs are CLOSED, no new MEDIUMs remain, and the 1.0 GA gate is clean.

**Closure summary: 4 CLOSED / 2 PARTIAL / 0 NOT CLOSED out of 6.**
**New findings: 2 MEDIUM, 1 LOW.**

### 5. Files reviewed (W7 follow-up)

- `docs/spec/evidence-bundle-1.0.md` (entire file; new §8 at lines 130-197)
- `docs/reference/error-kinds.md` (entire file)
- `docs/guides/coord-mtls.md` (entire file; new lines 83-99 and 198-220)
- `CHANGELOG.md:7-68` (Unreleased section)
- `crates/llmvm-cli/tests/cli_evidence_inspect_decrypt.rs` (entire file)
- `orchestrator/src/audit/verify.rs:130-230` (encryption verify path)
- `orchestrator/src/audit/encryption.rs:40-200` (envelope unwrap path)
- Source-side grep verification of every `error_kind` literal in `crates/` and `orchestrator/`
