# Boruna Research — 06: Orchestrator Audit & Evidence Subsystem

READ-ONLY security research. Slice: `orchestrator/src/audit/**` + `orchestrator/tests/bundle_encryption.rs`.
Every claim cites `path:line`. Unverified items are marked as such rather than guessed.

Crate: `boruna-orchestrator` (dir `orchestrator/`). All paths below are relative to repo root
`/Users/katsarov/htdocs/ai-lang`.

---

## 1. Purpose & Architecture + Threat Model

The audit subsystem is the compliance core: it turns a workflow run into a **hash-chained audit
log** and a **tamper-evident evidence bundle** on disk, optionally **AES-256-GCM envelope-encrypted**,
optionally shipped to **S3 / GCS / Azure**, with a **verifier** (`verify_bundle`) that an auditor runs
to confirm integrity.

Data flow at finalize (`evidence.rs:184-280`):

1. Builder writes component files (`workflow.json`, `policy.json`, per-step outputs, `intents.json`,
   `model_invoking_steps.json`) — each registered in `file_checksums` as `SHA-256(plaintext)`
   (`evidence.rs:287-297`, `:126-141`).
2. `finalize` writes `audit_log.json` and `env_fingerprint.json` (also checksummed), then builds the
   `manifest.json` containing `file_checksums`, `audit_log_hash`, `workflow_hash`, `policy_hash`,
   `env_fingerprint`, optional `encryption` block, and `bundle_hash` (`evidence.rs:208-236`).
3. `bundle_hash = SHA-256(pretty(manifest with bundle_hash="") )` (`evidence.rs:220-231`).
4. `bundle.json` (format-version gate) is written LAST via atomic write + parent-dir fsync
   (`evidence.rs:264-277`, `:326-356`).

**Hash chain** (`log.rs`): each `AuditEntry.entry_hash = SHA-256(seq_le || prev_hash || event_json)`
with genesis `prev_hash = "0"*64` (`log.rs:94-112`, `:189-196`). `verify()` walks the chain
(`log.rs:115-130`).

**Encryption** (`encryption.rs`): per-bundle random DEK (`OsRng`, `:125-126`) wrapped by an
operator-supplied KEK with AES-256-GCM, AAD = `kek_id` (`:131-142`). Files are encrypted under the DEK
with a **deterministic per-file nonce = `SHA-256(filename)[..12]`** (`:220-228`, `:291-298`).
`file_checksums` remain over plaintext, so `verify` decrypts then hashes (`evidence.rs:136-139`,
`verify.rs:201-224`).

### Stated threat model (from `encryption.rs:1-24` + `bundle_encryption.rs`)

- KEK is supplied out-of-band; Boruna does **not** store/manage/rotate keys.
- Tampering ciphertext ⇒ GCM tag failure ⇒ `CipherTagInvalid` (`bundle_encryption.rs:137-161`).
- Wrong/absent KEK ⇒ `EncryptionKeyMismatch` / `encryption_key_required`
  (`bundle_encryption.rs:91-134`).
- Manifest-level read must not leak plaintext (checksums are one-way; `encryption.files` is names
  only) (`bundle_encryption.rs:183-206`).

### Threat-model reality (this review)

The bundle's integrity model is **only as strong as an external anchor for `bundle_hash`** — and for
**plaintext (unencrypted) bundles that anchor is neither checked nor exposed** (Finding 4.1). Encrypted
bundles get genuine tamper-evidence rooted in the out-of-band KEK (Finding 5.1). The distinction is the
single most important takeaway of this slice.

---

## 2. Component Inventory

| File | Responsibility | Key types / fns | Status |
|---|---|---|---|
| `log.rs` | Append-only hash-chained audit log | `AuditEntry`, `AuditEvent`, `AuditLog::{append,verify,hash,from_entries_verified,compute_hash}` | Works; chain is self-referential (no external anchor) |
| `evidence.rs` | Build bundle on disk | `BundleManifest`, `BundleJson`, `EvidenceBundleBuilder::{add_*,finalize,write_file,encrypt_if_needed}` | Works; `bundle_hash` computed but never verified downstream |
| `verify.rs` | Verify bundle integrity | `verify_bundle`, `verify_bundle_with_kek`, `check_bundle_format`, `EvidenceError` | Detects naive tamper + ciphertext tamper; does NOT verify `bundle_hash`, `workflow_hash`, genesis binding |
| `encryption.rs` | AES-256-GCM envelope | `Envelope::{new,unwrap,encrypt_file,decrypt_file,rewrap}`, `derive_nonce`, `resolve_kek`, `parse_kek_hex` | Crypto sound; deterministic-nonce invariant unenforced; no key zeroization |
| `fingerprint.rs` | Env capture (no secrets) | `EnvFingerprint::capture` | Works; `rust_version` is `option_env!` → often `"unknown"` |
| `rotate.rs` | KEK rotation (manifest-only) | `rotate_bundle`, `rotate_dir`, `compute_bundle_hash`, `RotateOptions` | Works; recomputes `bundle_hash` verify never checks |
| `storage.rs` | Trait + LocalFs + `from_uri` | `BundleStorage`, `LocalFs`, `StorageRef`, `from_uri` | Works; `LocalFs::get` no `..` guard |
| `storage_s3.rs` | S3 adapter (`object_store`) | `S3Bucket`, `S3Uri`, `ref_to_run_id`, `classify_os_error` | Works; `ref_to_run_id` allows `..` |
| `storage_gcs.rs` | GCS adapter | `GcsBucket`, `GsUri`, same shape | Works; same `..` gap |
| `storage_azure.rs` | Azure Blob adapter | `AzureBlobBucket`, `AzUri`, same shape | Works; same `..` gap |
| `mod.rs` | Re-exports + `BUNDLE_FORMAT_VERSION="1.0"` | — | OK |

---

## 3. GAPS (functional / integrity completeness)

**3.1 `bundle_hash` is computed and recomputed but NEVER verified.** — `evidence.rs:220-231`,
`rotate.rs:144-145`, `rotate.rs:210-219`; consumed only for **display** (`crates/llmvm-cli/src/main.rs:3348,4397,4577`,
`crates/llmvm-cli/src/evidence_serve.rs:201-218`). `verify_bundle`/`verify_bundle_with_kek` never
recompute or compare it (whole of `verify.rs:89-284`). Grep confirms zero verification site
(`grep -rn bundle_hash orchestrator/src` → only build/rotate/display). Severity: **High** (this is the
field that would bind the manifest to an external anchor; see 4.1).

**3.2 Manifest `workflow_hash` / `policy_hash` are not cross-checked at verify.** — Declared in the
manifest (`evidence.rs:212-214`) and in the audit `WorkflowStarted` event (`log.rs:16-19`) but
`verify_bundle` never asserts `manifest.workflow_hash == SHA-256(workflow.json)` nor that the audit
genesis event's `workflow_hash` matches. `verify.rs:192-224` only checks the separate
`file_checksums["workflow.json"]`. Severity: **Medium** (genesis-to-workflow binding unenforced).

**3.3 No genesis-entry requirement.** — An empty audit log passes `verify()`
(`log.rs:277-280` test `test_empty_log_verifies`); `verify_bundle` skips the chain check when
`audit_pt.is_empty()` (`verify.rs:238`). A bundle can lack any `WorkflowStarted` genesis and still
verify. Severity: **Low/Medium**.

**3.4 Extra/unregistered files are not detected.** — `verify_bundle` iterates only
`manifest.file_checksums` (`verify.rs:192`); it never enumerates files actually on disk. An added file
(e.g. an extra `outputs/…`) is invisible to verification. Severity: **Low** for encrypted (attacker
can't forge existing ciphertext) / subsumed by 4.1 for plaintext.

**3.5 `EnvFingerprint.rust_version` is usually `"unknown"`.** — `fingerprint.rs:26-31` reads
`option_env!("RUSTC_VERSION")`; no build script sets it (not verified to exist, but the code path
defaults to `"unknown"`). `hostname` reads `$HOSTNAME`/`$HOST`, else `"unknown"` (`:33-37`) — attacker/
env-spoofable, and part of the compliance record. Severity: **Low**.

**3.6 Manifest itself is plaintext even for encrypted bundles.** — `manifest.json` (with
`file_checksums = SHA-256(plaintext)` and the encryption block) is written unencrypted
(`evidence.rs:234-236`). Plaintext SHA-256 hashes act as a confirmation oracle for low-entropy step
outputs. Severity: **Low** (confidentiality).

---

## 4. SECURITY — Tamper-Evidence

**4.1 Plaintext evidence bundles are NOT tamper-evident against an attacker with write access to the
bundle directory. [CONFIRMED]** — `verify.rs:89-284`.
`verify_bundle` performs only **internal self-consistency** checks:
- file bytes vs `manifest.file_checksums` (`verify.rs:192-224`) — the checksum map lives *inside* the
  same rewritable `manifest.json`;
- audit chain vs its own hashes and `audit_log.hash() == manifest.audit_log_hash` (`verify.rs:226-264`)
  — again self-referential;
- required-file existence (`verify.rs:266-278`).

An attacker edits `workflow.json` → recomputes its SHA-256 → overwrites
`manifest.file_checksums["workflow.json"]`; if they touch the audit log they recompute the whole chain
(`log.rs:94-112` is public and deterministic) and update `manifest.audit_log_hash`. `verify_bundle`
then returns `valid:true`. The only field binding the entire manifest — `bundle_hash` — is **never
checked** (3.1), and `verify_bundle` exposes **no `--expected-hash` / anchor parameter**
(signatures `verify.rs:89`, `:96`; CLI `crates/llmvm-cli/src/main.rs:4407-4413`). The existing tests
only cover *naive* tamper (edit a file, leave the manifest — `verify.rs:339-353`,
`evidence.rs:440-447`), which is why the gap is green in CI.
Note: recomputing `bundle_hash` *inside* verify would add **zero** tamper resistance (the attacker
recomputes it too, exactly as `rotate.rs:144` does) — genuine tamper-evidence requires an **external
anchor** (published/notarized `bundle_hash` or a manifest signature) plus a verify path that checks it.
**Impact:** for unencrypted bundles the product's "tamper-evident" guarantee reduces to detecting
accidental corruption and unsophisticated edits, not a motivated adversary. This is the top finding.

**4.2 Genesis / workflow-def binding not enforced at verify. [CONFIRMED]** — see 3.2. Even under an
"internally consistent" reading, nothing ties the audit genesis (`WorkflowStarted{workflow_hash,
policy_hash}`, `log.rs:16-19`) to the stored `workflow.json`/`policy.json` or to `manifest.workflow_hash`.
`bundle.json.workflow_hash` (`evidence.rs:264-270`) is likewise unchecked beyond `format_version`
(`check_bundle_format`, `verify.rs:52-81`).

---

## 5. SECURITY — AES-GCM / KEK / DEK

**5.1 Encrypted bundles ARE genuinely tamper-evident (with caveats). [CONFIRMED-adequate]** — Any
ciphertext change fails the GCM tag and is reported per-file (`verify.rs:201-215`,
`bundle_encryption.rs:137-161`). Forging a passing file needs a valid ciphertext ⇒ the DEK ⇒ the KEK,
held out-of-band. Swapping the whole envelope for one wrapped under an attacker KEK fails unwrap under
the verifier's legitimate KEK (`EncryptionKeyMismatch`, `encryption.rs:191-199`). **Caveats:** (a) the
verifier must supply the *correct trusted* KEK and not be induced to fetch a key by the manifest's
attacker-controlled `kek_id` (`encryption.rs:51-54`); (b) integrity still rides on KEK secrecy, not on
any signature.

**5.2 Deterministic-nonce invariant ("each filename encrypted ≤ once per bundle") is UNENFORCED —
nonce-reuse risk. [NEEDS-REVIEW]** — `derive_nonce = SHA-256(filename)[..12]` (`encryption.rs:291-298`)
with a per-bundle DEK is safe *only* if no filename is encrypted twice under the same DEK.
`encrypt_if_needed` re-encrypts on every `write_file`/`add_file`/`add_step_output`
(`evidence.rs:302-311`, `:287-297`, `:144-146`, `:126-141`) with no guard against a repeated `name`.
Two writes of the same name with different content ⇒ identical `(DEK, nonce)` ⇒ **AES-GCM keystream
reuse** (plaintext XOR leak + the classic GCM forbidden-attack enabling authentication-key recovery /
tag forgery). Builder methods are each normally called once, but nothing enforces it (e.g. two
`add_file("x", …)` calls, or a retried step re-emitting the same `outputs/<step>/<name>`). Recommend
either random per-file nonces stored in the manifest, or an explicit "filename written once" assertion.

**5.3 No key-material zeroization. [NEEDS-REVIEW / defense-in-depth]** — `Envelope.dek: [u8; 32]`
(`encryption.rs:106`), the `dek`/`dek_bytes` locals (`:125`, `:191-209`), the KEK `[u8;32]`
(`parse_kek_hex`, `:301-316`), and the `BORUNA_BUNDLE_KEK` hex `String` (`resolve_kek`, `:321-329`) are
never wiped on drop — no `zeroize`/`Drop`. The module doc's "DEK … dropped when the builder is consumed"
(`evidence.rs:100-103`) conflates drop with erasure. For a compliance-grade product handling KEK/DEK
this is a real gap. (The `Debug` redaction at `:112-118` is good but only covers logging.)

**5.4 `Envelope::unwrap` is panic-free on crafted input. [SAFE]** — The "fan-in 685 unwrap hotspot" is
the crypto envelope unwrap, NOT `Option::unwrap`. It returns `Result` throughout: algorithm gate
(`:164-169`), base64 decode errors (`:170-181`), nonce length check *before* `Nonce::from_slice`
(`:182-188`), decrypt→`EncryptionKeyMismatch` (`:191-199`), DEK length check (`:200-206`). `kek` is a
fixed `&[u8;32]` so `Key::from_slice` cannot panic. The only `expect` is in `encrypt_file`
(`:225-227`) over our *own* bounded plaintext (build side, not attacker input). The verify path's audit
decrypt uses `unwrap_or_default()` (`verify.rs:235`). No panic-on-attacker-input found.

**5.5 Per-file encryption uses no AAD. [NEEDS-REVIEW, minor]** — `encrypt_file` passes plaintext
directly (`encryption.rs:225-227`); the filename is bound only via the nonce derivation. A 96-bit
truncated-SHA-256 nonce collision would make two files swappable without tag failure — astronomically
unlikely, but binding the filename (and ideally `run_id`/`kek_id`) into AAD would be strictly stronger.
DEK-wrap AAD is correctly set to `kek_id` (`:137`, `:196`).

---

## 6. SECURITY — Storage Backends (SSRF / creds / path injection / TLS)

**6.1 `..` path traversal in `ref_to_run_id` → arbitrary recursive deletion in `get`. [NEEDS-REVIEW]**
— `ref_to_run_id` rejects `/` and empty but NOT `..`/`.`
(`storage_s3.rs:409-425`, `storage_gcs.rs:436-452`, `storage_azure.rs:466-482`). A `StorageRef` ending
in `/..` yields `run_id = ".."`; `get` then does
`cache_dir = self.cache_root.join(run_id); if cache_dir.exists() { fs::remove_dir_all(&cache_dir) }`
(`storage_s3.rs:260-269`, `storage_gcs.rs:295-301`, `storage_azure.rs:327-333`). With default
`cache_root = <temp>/boruna-bundle-cache`, `join("..")` = `<temp>` → `remove_dir_all` **wipes the
system temp dir** (and, with longer `../..` suffixes, higher directories). `LocalFs::get` similarly
does no `..` canonicalization (`storage.rs:179-188`) — unlike the `PatchBundle`/`ContextStore`
hardening recorded in project memory. **Reachability today:** the remote `BundleStorage::get` sink is
**not wired into the CLI `evidence verify`** (that path takes a local dir directly —
`crates/llmvm-cli/src/main.rs:4407-4413`; grep found no CLI call site passing an operator ref into
`.get`). So this is currently a **latent** defense-in-depth gap exploitable via the library API or any
future wiring where the `StorageRef`/`run_id` is attacker-influenced (e.g. a `run_id` containing `..`
flows into the ref at `put`, `storage_s3.rs:257`). Recommend rejecting `..`/`.`/separators in
`ref_to_run_id` and `run_id`, matching the codebase's existing path-traversal defenses.

**6.2 Endpoint / SSRF surface is operator-config only, not attacker-controlled via the bundle.
[SAFE, noted]** — S3 endpoint/scheme come from `AWS_ENDPOINT_URL` + `AWS_ALLOW_HTTP` via
`AmazonS3Builder::from_env` (`storage_s3.rs:208-210`, module doc `:24-32`). GCS emulator override is a
**builder hook** injecting a service-account JSON (`storage_gcs.rs:227-247`); production uses
`from_env`. Azure emulator/endpoint likewise builder-only, and only the builder path forces
`with_allow_http(true)` (`storage_azure.rs:274-279`); `from_env` is the production path
(`:271-273`). None of these are reachable from bundle/manifest content, so there is **no
bundle-driven SSRF**. TLS is `object_store`'s default (HTTPS) unless the operator explicitly opts into
HTTP via env/emulator.

**6.3 Credential handling. [SAFE, noted]** — All adapters source creds from standard provider env vars
via `from_env` (S3 `:208`, GCS `:246`, Azure `:271`); no credentials are logged. Errors are formatted
from `object_store::Error` (`classify_os_error`, `storage_s3.rs:441-455` et al.) — not verified to be
credential-free, but object_store does not embed secret material in its error `Display`. The GCS
emulator path writes empty `private_key`/`client_email` into an in-memory SA JSON
(`storage_gcs.rs:234-241`) — test/emulator only.

**6.4 Bucket/key parsing rejects obvious injection; keys derive from local dir walk. [SAFE]** — URI
parsers reject whitespace/`:`/missing components (`storage_s3.rs:90-120`, `storage_gcs.rs:100-131`,
`storage_azure.rs:107-154`). Object keys come from `walk_files` relative paths under the bundle dir
with symlinks skipped (`storage_s3.rs:379-402` and mirrors), and `object_store::path::Path`
normalizes/encodes segments. No key-injection vector found.

**6.5 `put`/`get` failures are best-effort and swallowed by design.** — Storage runs *after* local
finalize and a storage failure is logged, not fatal (`storage.rs:26-33`). Acceptable, but means a
silently failed remote upload leaves only the local copy — an availability/durability consideration for
the compliance record, not an integrity bug.

---

## 7. COVERAGE

Read in full: `orchestrator/src/audit/{log,evidence,verify,encryption,fingerprint,rotate,mod,storage,
storage_s3,storage_gcs,storage_azure}.rs` and `orchestrator/tests/bundle_encryption.rs` (every `.rs`
in the slice). Cross-checked `bundle_hash` consumers workspace-wide via grep
(`orchestrator/src`, `crates/llmvm-cli/src`) and confirmed no verification site. Confirmed the remote
`BundleStorage::get` sink is not reached by the CLI `evidence verify` path. **Not verified** (outside
slice, flagged where relevant): whether any downstream/integrator code or the orchestrator runner calls
`BundleStorage::get` with an untrusted ref (6.1 reachability); whether a build script populates
`RUSTC_VERSION` (3.5); whether run metadata that stores the emitted `StorageRef` can be
attacker-influenced. `object_store` internal TLS/SSRF behavior is trusted as a dependency, not audited
here.
