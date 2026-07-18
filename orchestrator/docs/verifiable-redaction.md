# Verifiable Redaction (evidence bundle format 1.1)

Turns a liability — *a sealed evidence bundle can't have un-redactable PII* —
into a feature: PII can be removed from a sealed bundle **without breaking
verification**, and the removal is itself recorded and tamper-evident.

## The problem

The original audit log (format 1.0) hashed the raw event JSON directly into
the chain:

```text
entry_hash = SHA-256(sequence_le || prev_hash || event_json)
```

Removing any field from an event changes `event_json`, so `entry_hash`
changes, so the chain breaks and every downstream `prev_hash` is invalidated.
A sealed bundle was therefore append-only *and* un-erasable — a GDPR/CCPA
liability.

## The mechanism: commit-to-leaf-hash (format 1.1)

Each entry now commits to a **content hash** of its event, and the chain
links via that commitment rather than the event bytes:

```text
content_sha256 = SHA-256(event_json)
entry_hash     = SHA-256(sequence_le || prev_hash_ascii || content_sha256_ascii)
```

`content_sha256` is stored on the entry. Because the chain links through
`content_sha256` (not the event bytes), the event content can be replaced
in place while `content_sha256` — and therefore `entry_hash`, every
`prev_hash`, and the log's overall `audit_log_hash` — stays identical.

### Redaction

`AuditLog::redact_entry(index, field, reason)` blanks PII-bearing string
leaves in the event to the sentinel `"[REDACTED]"` (whole event, or a single
named field), and stamps the entry with a `Redaction { content_sha256,
reason }` marker. The event's serde **variant is preserved** (only string
*values* are blanked, never the enum tag/field keys), so every existing
reader — including the exhaustive `AuditEvent` match in the ITF exporter —
keeps compiling and working with no change.

A redacted entry's event is **non-authoritative**: the removed content is
proven only by `content_sha256`. Verification of a redacted entry therefore
checks the commitment, not the (now blanked) event.

## Verification: `verify()`

Per entry, the form is detected by the presence of `content_sha256`:

- **Commitment (1.1)** — `entry_hash` must equal
  `SHA-256(seq || prev || content_sha256)`, **and** the content must bind:
  - not redacted → `SHA-256(event_json) == content_sha256`;
  - redacted → the marker's `content_sha256 == entry.content_sha256`.
- **Legacy (1.0)** — empty `content_sha256` → original formula
  `SHA-256(seq || prev || event_json)`.

So a whole log is either legacy or commitment-form and **both verify**
(back-compat). New bundles are commitment-form; pre-1.1 bundles keep
verifying under the legacy rule.

### Redaction vs. tampering — the crux

A redaction is an **authorized, recorded transformation**; a tamper is not.
They are distinguished cryptographically, not by trust:

| | valid redaction | content tamper |
|---|---|---|
| `content_sha256` of the entry | unchanged | must change to alter content |
| `entry_hash` / chain | intact | broken (unless whole chain rewritten) |
| **`audit_log_hash`** | **invariant** | **changes** |
| `verify()` | passes | fails |
| entry reported as redacted | yes (marker) | n/a |

The load-bearing invariant is **`audit_log_hash` (the last `entry_hash`) does
not change under redaction**. To alter event *content* an attacker must change
a `content_sha256`, which changes that entry's `entry_hash`, which changes the
final `audit_log_hash`. So:

- An operator who anchored the original `audit_log_hash` out-of-band sees it
  **survive redaction but not a tamper**.
- Within the bundle, `verify_bundle` reports `redacted_entries` and the chain
  still verifies for a redaction but fails for a content tamper.
- A redact-then-tamper (rewrite the marker or the committed hash) splits the
  marker/commitment binding and/or the `entry_hash` recompute → fails.

## Bundle level: `redact_bundle(dir, index, field, reason)`

Redacting inside an evidence bundle mutates `audit_log.json`, so it also:

1. rewrites `audit_log.json` (event blanked + marker added);
2. updates `manifest.file_checksums["audit_log.json"]` to the new bytes' hash;
3. recomputes `manifest.bundle_hash` (so the bundle stays self-consistent and
   `evidence verify` passes);
4. drops any `manifest.signature` (it signed the *pre-redaction* `bundle_hash`).

`manifest.audit_log_hash` is **unchanged** (it equals the invariant
`audit_log_hash`).

### Which top-level hashes a redaction legitimately changes

| field | changes on redaction? | why |
|---|---|---|
| `audit_log_hash` | **no** | the whole point — the anchor that survives |
| `file_checksums["audit_log.json"]` | yes | the file bytes changed (event blanked) |
| `bundle_hash` | yes | it covers `file_checksums` |
| `signature` | dropped | it signed the old `bundle_hash` |

An operator anchoring on `audit_log_hash` (recommended) is unaffected by
redaction. An operator anchoring on `bundle_hash` must re-record it after a
redaction, and re-sign if they use signatures. This is inherent: redaction is
a post-seal transformation the original signer did not endorse.

## Scope / caveats (honest)

- **No new `AuditEvent` variant.** The redaction placeholder is a sibling
  `Redaction` marker on `AuditEntry` plus in-place string-blanking, *not* a
  distinct event variant. This is a deliberate design choice: a new variant
  would break the exhaustive `AuditEvent` match in the `boruna-tooling` ITF
  exporter (`tooling/src/trace/audit_to_itf.rs`), which is outside this
  slice's edit surface. The marker form is functionally equivalent (content
  gone, commitment kept, redaction recorded) and touches no other crate.
- **Field-level redaction marks the whole entry non-authoritative.** Because
  the commitment is over the *original full* event, a partial (`--field`)
  redaction cannot keep the untouched fields independently verifiable — once
  redacted, the event is advisory and only `content_sha256` is anchored. Use
  `--field` to minimize what is blanked for human readers; the trust boundary
  is still the whole-event commitment.
- **Encrypted bundles are not redactable here.** `redact_bundle` rejects
  bundles with an `encryption` block (`BundleRedactError::EncryptedUnsupported`).
  Decrypt/rotate first; redacting through the envelope is future work.
- **`reason` is advisory.** It is stored in the marker but is *not* part of any
  hash commitment, so it is not tamper-evident. Only `content_sha256` binds.

## CLI

```bash
# Remove PII from a sealed bundle; re-verifies automatically.
boruna evidence redact <bundle-dir> --event 3 --reason "GDPR erasure request"

# Redact only one field of the event.
boruna evidence redact <bundle-dir> --event 3 --field approver

# Verify reports which entries are redacted.
boruna evidence verify <bundle-dir>
#   evidence bundle is VALID
#     redacted entries: [3]
```
