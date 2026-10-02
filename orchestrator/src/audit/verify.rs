use sha2::{Digest, Sha256};
use std::path::Path;

use crate::audit::encryption::{EncryptionError, Envelope, KEY_LEN};
use crate::audit::evidence::{BundleJson, BundleManifest, ManifestSignature};
use crate::audit::log::AuditLog;
use crate::audit::BUNDLE_FORMAT_VERSION;

/// Errors raised by the bundle reader gate that callers may want to
/// match on programmatically (vs. the free-form strings in
/// `VerifyResult::errors`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceError {
    /// `bundle.json` is missing or has no `format_version` field, OR the
    /// version's major component does not match the reader's supported
    /// major (semver-like compat: `1.x` is forward-compat for a `1.0`
    /// reader; `2.x` is rejected).
    UnsupportedFormat { found: String, expected: String },
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvidenceError::UnsupportedFormat { found, expected } => {
                write!(
                    f,
                    "unsupported evidence bundle format_version: found `{found}`, expected major `{expected}`"
                )
            }
        }
    }
}

impl std::error::Error for EvidenceError {}

/// Verification result.
#[derive(Debug)]
pub struct VerifyResult {
    pub valid: bool,
    pub errors: Vec<String>,
    /// Sequence numbers of audit-log entries that carry an authorized
    /// redaction. Populated once the audit log is parsed; empty when the
    /// bundle is rejected before that point. A validly-redacted bundle is
    /// still `valid: true` with these entries listed — that is how a
    /// reader sees a redaction is present (and recorded), as distinct
    /// from a tamper (which flips `valid` to false).
    pub redacted_entries: Vec<u64>,
}

impl VerifyResult {
    /// Build an invalid result with no redaction info (used for the
    /// early-return rejection paths before the audit log is parsed).
    fn invalid(errors: Vec<String>) -> Self {
        VerifyResult {
            valid: false,
            errors,
            redacted_entries: Vec::new(),
        }
    }
}

/// Read and validate the top-level `bundle.json` format gate.
///
/// Implements §1 ("reject at parse, don't silently override"):
/// - Legacy bundles (pre-1.0, no `bundle.json`) → reject with a clear
///   migration hint pointing at `boruna migrate evidence-bundle` (W5-C).
/// - Future-major bundles (`2.x`) → reject — the reader can't safely
///   interpret an unknown major.
/// - Same-major bundles (`1.x`) → accept (forward-compat: unknown
///   fields are ignored).
pub fn check_bundle_format(bundle_dir: &Path) -> Result<BundleJson, EvidenceError> {
    let bundle_path = bundle_dir.join("bundle.json");
    let raw = match std::fs::read_to_string(&bundle_path) {
        Ok(s) => s,
        Err(_) => {
            return Err(EvidenceError::UnsupportedFormat {
                found: "missing bundle.json (legacy bundle from pre-1.0 release; use `boruna migrate evidence-bundle` to upgrade)".to_string(),
                expected: BUNDLE_FORMAT_VERSION.to_string(),
            });
        }
    };
    let parsed: BundleJson = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            return Err(EvidenceError::UnsupportedFormat {
                found: format!("invalid bundle.json: {e}"),
                expected: BUNDLE_FORMAT_VERSION.to_string(),
            });
        }
    };
    let major = parsed.format_version.split('.').next().unwrap_or("");
    let expected_major = BUNDLE_FORMAT_VERSION.split('.').next().unwrap_or("1");
    if major.is_empty() || major != expected_major {
        return Err(EvidenceError::UnsupportedFormat {
            found: parsed.format_version.clone(),
            expected: format!("{expected_major}.x"),
        });
    }
    Ok(parsed)
}

/// Options controlling bundle verification.
///
/// Constructed with `..Default::default()` so callers set only the
/// knobs they use. All fields are additive over the base integrity
/// check; the default (`kek: None`, no anchor, no signature
/// requirements) reproduces the original `verify_bundle` behavior.
///
/// The two independent hardening axes compose freely — an operator may
/// anchor a plaintext bundle out-of-band (`expected_bundle_hash`), pin a
/// signer key (`trusted_pubkey` / `require_signature`), or both.
#[derive(Debug, Default, Clone)]
pub struct VerifyOptions<'a> {
    /// Explicit KEK for decrypting an encrypted bundle. Falls back to
    /// `BORUNA_BUNDLE_KEK` when `None`.
    pub kek: Option<&'a [u8; KEY_LEN]>,
    /// External anchor: an operator-supplied, out-of-band `bundle_hash`.
    /// The manifest (and thus its self-`bundle_hash`) is rewritable by
    /// anyone with bundle write access, so internal recomputation alone
    /// proves nothing against a motivated attacker — the external anchor
    /// is what makes a plaintext bundle genuinely tamper-evident. When
    /// set, the recomputed hash must equal it.
    pub expected_bundle_hash: Option<&'a str>,
    /// When true, a bundle whose manifest has no `encryption` block
    /// fails — blocking a downgrade-to-plaintext strip of an encrypted
    /// bundle.
    pub require_encryption: bool,
    /// verify-A: when set, the bundle's `signature.public_key` MUST
    /// equal this hex-encoded ed25519 public key. This is what roots
    /// trust — without it an attacker can re-sign a tampered bundle
    /// with their own key. Case-insensitive hex compare.
    pub trusted_pubkey: Option<&'a str>,
    /// verify-A: when true, an unsigned bundle fails with
    /// `evidence.signature_required`.
    pub require_signature: bool,
}

/// Verify an evidence bundle directory for integrity.
///
/// For unencrypted bundles this is a pure on-disk check. For
/// encrypted bundles (sprint W6-B), the KEK is resolved from
/// `BORUNA_BUNDLE_KEK` env var; if absent the result includes an
/// `evidence.encryption_key_required` error and verification stops.
pub fn verify_bundle(bundle_dir: &Path) -> VerifyResult {
    verify_bundle_with_opts(bundle_dir, &VerifyOptions::default())
}

/// Verify an evidence bundle, optionally with an explicit KEK
/// supplied by the caller (CLI flag path). Falls back to env var
/// when `kek` is `None`.
pub fn verify_bundle_with_kek(bundle_dir: &Path, kek: Option<&[u8; KEY_LEN]>) -> VerifyResult {
    verify_bundle_with_opts(
        bundle_dir,
        &VerifyOptions {
            kek,
            ..Default::default()
        },
    )
}

/// Verify an evidence bundle with full options. The base integrity
/// check (format gate, file checksums, audit-log chain, required
/// files) always runs; the hardening checks in `opts` are additive
/// and accumulate into `errors` so they compose:
///
/// - external anchor + `bundle_hash` consistency + `require_encryption`
///   (plaintext tamper-evidence, downgrade protection); and
/// - verify-A ed25519 signature verification (`verify_manifest_signature`,
///   trusted-pubkey pin, `require_signature`).
pub fn verify_bundle_with_opts(bundle_dir: &Path, opts: &VerifyOptions) -> VerifyResult {
    let mut errors = Vec::new();

    // 0. Format-version gate: reject pre-1.0 legacy bundles and any
    //    bundle from a future incompatible major. This runs FIRST so
    //    we don't try to read content we can't safely interpret.
    if let Err(e) = check_bundle_format(bundle_dir) {
        return VerifyResult::invalid(vec![e.to_string()]);
    }

    // 1. Load and parse manifest
    let manifest_path = bundle_dir.join("manifest.json");
    let manifest_json = match std::fs::read_to_string(&manifest_path) {
        Ok(j) => j,
        Err(e) => {
            return VerifyResult::invalid(vec![format!("cannot read manifest.json: {e}")]);
        }
    };

    let manifest: BundleManifest = match serde_json::from_str(&manifest_json) {
        Ok(m) => m,
        Err(e) => {
            return VerifyResult::invalid(vec![format!("invalid manifest.json: {e}")]);
        }
    };

    // 1b. Encryption-required + bundle_hash consistency / external anchor.
    if opts.require_encryption && manifest.encryption.is_none() {
        errors.push(
            "evidence.encryption_required: --require-encryption is set but the manifest has no \
             encryption block (possible downgrade-to-plaintext strip)"
                .to_string(),
        );
    }
    match recompute_bundle_hash(&manifest) {
        Ok(recomputed) => {
            // Internal consistency: catches a naive edit that forgot to
            // recompute bundle_hash. Not tamper-proof on its own (a motivated
            // attacker recomputes it too) — that is what the anchor below is for.
            if recomputed != manifest.bundle_hash {
                errors.push(format!(
                    "evidence.bundle_hash_inconsistent: manifest.bundle_hash {} does not match \
                     the recomputed {} (manifest was edited)",
                    manifest.bundle_hash, recomputed
                ));
            }
            // External anchor: the real tamper-evidence check.
            if let Some(anchor) = opts.expected_bundle_hash {
                if recomputed != anchor {
                    errors.push(format!(
                        "evidence.bundle_hash_anchor_mismatch: recomputed bundle_hash {recomputed} \
                         does not match the expected anchor {anchor}"
                    ));
                }
            }
        }
        Err(e) => errors.push(format!("cannot recompute bundle_hash: {e}")),
    }

    // 1.5 verify-A: optional ed25519 signature over `bundle_hash`.
    //     Runs independently of encryption/anchor; accumulates into
    //     `errors` rather than early-returning so it composes with the
    //     checksum checks below.
    match &manifest.signature {
        Some(sig) => verify_manifest_signature(&manifest, sig, opts.trusted_pubkey, &mut errors),
        None => {
            if opts.require_signature {
                errors.push(
                    "evidence.signature_required: bundle is unsigned but a signature was required"
                        .to_string(),
                );
            }
        }
    }

    // 2. Resolve envelope when the bundle is encrypted.
    let envelope = match &manifest.encryption {
        Some(info) => {
            let resolved_kek = match opts.kek {
                Some(k) => Some(*k),
                None => match crate::audit::encryption::resolve_kek(None) {
                    Ok(k) => k,
                    Err(e) => {
                        return VerifyResult::invalid(vec![format!("invalid KEK: {e}")]);
                    }
                },
            };
            let key = match resolved_kek {
                Some(k) => k,
                None => {
                    return VerifyResult::invalid(vec![format!(
                        "evidence.encryption_key_required: bundle is encrypted (kek_id={}); \
                         supply --bundle-encryption-key <hex> or set BORUNA_BUNDLE_KEK",
                        info.kek_id
                    )]);
                }
            };
            match Envelope::unwrap(info, &key) {
                Ok(env) => Some(env),
                Err(EncryptionError::UnsupportedAlgorithm { found, expected }) => {
                    return VerifyResult::invalid(vec![format!(
                        "evidence.unsupported_algorithm: bundle declares algorithm={found:?}; \
                         reader supports only {expected:?}"
                    )]);
                }
                Err(EncryptionError::EncryptionKeyMismatch) => {
                    return VerifyResult::invalid(vec![format!(
                        "evidence.encryption_key_mismatch: supplied KEK does not unwrap the \
                         bundle's wrapped_dek (kek_id={})",
                        info.kek_id
                    )]);
                }
                Err(e) => {
                    return VerifyResult::invalid(vec![format!(
                        "invalid encryption metadata: {e}"
                    )]);
                }
            }
        }
        None => None,
    };

    // 3. Verify file checksums (decrypt-in-memory if needed).
    for (filename, expected_hash) in &manifest.file_checksums {
        let file_path = bundle_dir.join(filename);
        let raw = match std::fs::read(&file_path) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("cannot read {filename}: {e}"));
                continue;
            }
        };
        let plaintext_bytes = match &envelope {
            Some(env) => match env.decrypt_file(filename, &raw) {
                Ok(pt) => pt,
                Err(EncryptionError::CipherTagInvalid { file }) => {
                    errors.push(format!(
                        "evidence.cipher_tag_invalid: {file} failed AES-GCM authentication \
                         (bundle tampered or wrong key)"
                    ));
                    continue;
                }
                Err(e) => {
                    errors.push(format!("decrypt {filename}: {e}"));
                    continue;
                }
            },
            None => raw,
        };
        let actual_hash = sha256_bytes(&plaintext_bytes);
        if actual_hash != *expected_hash {
            errors.push(format!(
                "checksum mismatch for {filename}: expected {expected_hash}, got {actual_hash}"
            ));
        }
    }

    // 4. Verify audit log chain integrity (decrypt-then-parse).
    let mut redacted_entries: Vec<u64> = Vec::new();
    let mut audit_gate_records: Vec<crate::confidence::GateRecord> = Vec::new();
    let mut audit_content_hashes: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut redacted_policy_hashes: Vec<String> = Vec::new();
    let audit_path = bundle_dir.join("audit_log.json");
    match std::fs::read(&audit_path) {
        Ok(raw) => {
            let audit_pt = match &envelope {
                // On decrypt failure here we return Vec::new() so the
                // chain check is skipped — the same tamper is already
                // reported via the file_checksums loop, no
                // double-error.
                Some(env) => env.decrypt_file("audit_log.json", &raw).unwrap_or_default(),
                None => raw,
            };
            if !audit_pt.is_empty() {
                match std::str::from_utf8(&audit_pt) {
                    Ok(audit_json) => match AuditLog::from_json(audit_json) {
                        Ok(audit_log) => {
                            // A valid redaction preserves the chain and
                            // audit_log_hash; a content tamper breaks one
                            // of them. So these checks are exactly what
                            // separates "redacted" from "tampered".
                            if let Err(bad_seq) = audit_log.verify() {
                                errors.push(format!("audit log chain broken at entry {bad_seq}"));
                            }
                            if audit_log.hash() != manifest.audit_log_hash {
                                errors.push(format!(
                                    "audit log hash mismatch: manifest says {}, actual is {}",
                                    manifest.audit_log_hash,
                                    audit_log.hash()
                                ));
                            }
                            redacted_entries = audit_log.redacted_sequences();
                            audit_gate_records =
                                crate::workflow::confidence_gate::records_from_audit(
                                    audit_log.entries(),
                                );
                            redacted_policy_hashes = audit_log
                                .entries()
                                .iter()
                                .filter(|e| {
                                    e.redacted.is_some()
                                        && matches!(
                                            e.event,
                                            crate::audit::log::AuditEvent::PolicyEvaluated { .. }
                                        )
                                })
                                .map(|e| e.content_sha256.clone())
                                .collect();
                            audit_content_hashes = audit_log
                                .entries()
                                .iter()
                                .filter(|e| !e.content_sha256.is_empty())
                                .map(|e| e.content_sha256.clone())
                                .collect();
                        }
                        Err(e) => {
                            errors.push(format!("invalid audit_log.json: {e}"));
                        }
                    },
                    Err(_) => {
                        errors.push("audit_log.json is not valid UTF-8".into());
                    }
                }
            }
        }
        Err(_) => errors.push("missing audit_log.json".into()),
    }

    // 4b. Confidence gates: recompute each recorded decision from the embedded calibration
    //     file and cross-check it against the hash-chained audit log.
    verify_confidence_gates(
        bundle_dir,
        envelope.as_ref(),
        &audit_gate_records,
        &audit_content_hashes,
        &redacted_policy_hashes,
        &mut errors,
    );

    // 5. Verify required files exist
    for required in &[
        "bundle.json",
        "manifest.json",
        "workflow.json",
        "policy.json",
        "audit_log.json",
        "env_fingerprint.json",
    ] {
        if !bundle_dir.join(required).exists() {
            errors.push(format!("missing required file: {required}"));
        }
    }

    VerifyResult {
        valid: errors.is_empty(),
        errors,
        redacted_entries,
    }
}

/// Read a bundle file, decrypting in memory when the bundle is encrypted.
fn read_plain(
    bundle_dir: &Path,
    name: &str,
    envelope: Option<&Envelope>,
) -> Result<Vec<u8>, String> {
    let raw =
        std::fs::read(bundle_dir.join(name)).map_err(|e| format!("cannot read {name}: {e}"))?;
    match envelope {
        Some(env) => env
            .decrypt_file(name, &raw)
            .map_err(|e| format!("decrypt {name}: {e}")),
        None => Ok(raw),
    }
}

/// True when the bundle's `workflow.json` declares at least one `confidence_gate`. When the
/// file cannot be read or parsed this answers true: the caller only uses it to be stricter.
fn workflow_declares_confidence_gates(bundle_dir: &Path, envelope: Option<&Envelope>) -> bool {
    let Ok(bytes) = read_plain(bundle_dir, "workflow.json", envelope) else {
        return true;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return true;
    };
    v.get("steps")
        .and_then(|s| s.as_object())
        .map(|steps| {
            steps
                .values()
                .any(|s| s.get("confidence_gate").is_some_and(|g| g.is_object()))
        })
        .unwrap_or(false)
}

/// Check `confidence_gates.json` against its embedded calibration files and the audit chain.
///
/// Each gate record must (1) match the calibration file's hash, (2) recompute to the same
/// threshold and decision, and (3) match a decision the audit chain committed to. That match
/// uses the chain's content hash, which survives redaction of the entry.
///
/// Omission is checked too. A readable audit decision missing from the file is an error, and
/// so is a missing file. When the workflow declares confidence gates, every redacted
/// `PolicyEvaluated` entry must be explained by a record in the file: a gate record carries
/// no personal data, so redacting its entry and then dropping the record would otherwise hide
/// an auto-approval.
fn verify_confidence_gates(
    bundle_dir: &Path,
    envelope: Option<&Envelope>,
    audit_records: &[crate::confidence::GateRecord],
    audit_content_hashes: &std::collections::BTreeSet<String>,
    redacted_policy_hashes: &[String],
    errors: &mut Vec<String>,
) {
    use crate::confidence::{CalibrationSet, GateRecord};
    use crate::workflow::confidence_gate::{audit_event_for_record, is_safe_step_file_name};
    let gates_file = "confidence_gates.json";

    let mut records: Vec<GateRecord> = Vec::new();
    if bundle_dir.join(gates_file).exists() {
        match read_plain(bundle_dir, gates_file, envelope) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(r) => records = r,
                Err(e) => {
                    errors.push(format!(
                        "evidence.confidence_gate: invalid {gates_file}: {e}"
                    ));
                    return;
                }
            },
            Err(e) => {
                errors.push(format!("evidence.confidence_gate: {e}"));
                return;
            }
        }
    } else if !audit_records.is_empty() {
        errors.push(format!(
            "evidence.confidence_gate: the audit log records {} gate decision(s) but \
             {gates_file} is missing",
            audit_records.len()
        ));
    }

    for rec in &records {
        if !is_safe_step_file_name(&rec.step_id) {
            errors.push(format!(
                "evidence.confidence_gate: gate step id '{}' is not a safe file name",
                rec.step_id
            ));
            continue;
        }
        let cal_name = format!("confidence/{}.calibration.json", rec.step_id);
        let raw = match read_plain(bundle_dir, &cal_name, envelope) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("evidence.confidence_gate: {e}"));
                continue;
            }
        };
        let set = match std::str::from_utf8(&raw)
            .map_err(|e| e.to_string())
            .and_then(|t| CalibrationSet::from_json(t).map_err(|e| e.to_string()))
        {
            Ok(set) => set,
            Err(e) => {
                errors.push(format!(
                    "evidence.confidence_gate: gate '{}': bad calibration: {e}",
                    rec.step_id
                ));
                continue;
            }
        };
        if let Err(msg) = rec.verify(&set, &raw) {
            errors.push(format!("evidence.confidence_gate: {msg}"));
        }
        // The chain commits to SHA-256 of the event JSON, and keeps that hash even when the
        // entry is redacted. Recompute it from this record: a match proves the record is what
        // the run committed, with or without redaction elsewhere in the log.
        let committed = crate::audit::log::AuditLog::content_hash(&audit_event_for_record(rec));
        if !audit_content_hashes.contains(&committed) {
            errors.push(format!(
                "evidence.confidence_gate: gate '{}' in {gates_file} does not match any \
                 decision committed in the audit chain",
                rec.step_id
            ));
        }
    }

    // The other direction: a readable decision in the chain that the file leaves out.
    for a in audit_records {
        if !records.iter().any(|r| r == a) {
            errors.push(format!(
                "evidence.confidence_gate: the audit chain records a decision for gate '{}' \
                 that {gates_file} does not contain",
                a.step_id
            ));
        }
    }

    // Redacted entries that no record accounts for.
    if !redacted_policy_hashes.is_empty()
        && workflow_declares_confidence_gates(bundle_dir, envelope)
    {
        let explained: std::collections::BTreeSet<String> = records
            .iter()
            .map(|r| crate::audit::log::AuditLog::content_hash(&audit_event_for_record(r)))
            .collect();
        let unexplained = redacted_policy_hashes
            .iter()
            .filter(|h| !explained.contains(*h))
            .count();
        if unexplained > 0 {
            errors.push(format!(
                "evidence.confidence_gate: {unexplained} redacted policy entr{} in the audit \
                 chain {} not accounted for by {gates_file}; this workflow declares confidence \
                 gates, so a gate decision may have been dropped",
                if unexplained == 1 { "y" } else { "ies" },
                if unexplained == 1 { "is" } else { "are" },
            ));
        }
    }
}

fn sha256_bytes(b: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b);
    format!("{:x}", hasher.finalize())
}

/// verify-A: check the manifest's ed25519 signature over `bundle_hash`.
///
/// The chain of trust is: signature (over `bundle_hash`, under the
/// embedded/pinned public key) → `bundle_hash` (recomputed to match
/// the manifest content) → `file_checksums` / `audit_log_hash` (checked
/// against the actual files elsewhere). `trusted_pubkey`, when set, is
/// what actually roots trust — otherwise any key that re-signs a
/// tampered bundle would pass.
fn verify_manifest_signature(
    manifest: &BundleManifest,
    sig: &ManifestSignature,
    trusted_pubkey: Option<&str>,
    errors: &mut Vec<String>,
) {
    if sig.algorithm != "ed25519" {
        errors.push(format!(
            "evidence.signature_invalid: unsupported signature algorithm {:?} \
             (reader supports only \"ed25519\")",
            sig.algorithm
        ));
        return;
    }

    // Trust root: pinned key must match the signer's key.
    if let Some(trusted) = trusted_pubkey {
        if !trusted.eq_ignore_ascii_case(&sig.public_key) {
            errors.push(format!(
                "evidence.signature_untrusted_key: bundle signed with {} but --verify-key pins {}",
                sig.public_key, trusted
            ));
            return;
        }
    }

    // The signed `bundle_hash` must bind the actual manifest content;
    // recompute it (excluding bundle_hash + signature, matching
    // finalize) and require a match.
    match recompute_bundle_hash(manifest) {
        Ok(expected) if expected != manifest.bundle_hash => {
            errors.push(format!(
                "evidence.signature_invalid: manifest bundle_hash {} does not match \
                 recomputed {}",
                manifest.bundle_hash, expected
            ));
            return;
        }
        Ok(_) => {}
        Err(e) => {
            errors.push(format!(
                "evidence.signature_invalid: cannot recompute bundle_hash: {e}"
            ));
            return;
        }
    }

    let pk_bytes = match decode_hex_array::<32>(&sig.public_key) {
        Ok(b) => b,
        Err(e) => {
            errors.push(format!(
                "evidence.signature_invalid: bad public_key hex: {e}"
            ));
            return;
        }
    };
    let sig_bytes = match decode_hex_array::<64>(&sig.signature) {
        Ok(b) => b,
        Err(e) => {
            errors.push(format!(
                "evidence.signature_invalid: bad signature hex: {e}"
            ));
            return;
        }
    };
    let vk = match ed25519_dalek::VerifyingKey::from_bytes(&pk_bytes) {
        Ok(vk) => vk,
        Err(e) => {
            errors.push(format!(
                "evidence.signature_invalid: invalid public_key: {e}"
            ));
            return;
        }
    };
    let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
    use ed25519_dalek::Verifier;
    if vk
        .verify(manifest.bundle_hash.as_bytes(), &signature)
        .is_err()
    {
        errors.push(
            "evidence.signature_invalid: ed25519 signature does not verify over bundle_hash"
                .to_string(),
        );
    }
}

/// Recompute a manifest's `bundle_hash` the way
/// `EvidenceBundleBuilder::finalize`/`rotate` do: clone, clear
/// `bundle_hash` and `signature`, pretty-print, sha256. Used by both
/// the external-anchor consistency check and the signature check, so
/// both bind the identical manifest content.
fn recompute_bundle_hash(manifest: &BundleManifest) -> Result<String, serde_json::Error> {
    let mut clone = manifest.clone();
    clone.bundle_hash = String::new();
    clone.signature = None;
    let json = serde_json::to_string_pretty(&clone)?;
    Ok(sha256_bytes(json.as_bytes()))
}

/// Decode a fixed-length lowercase/uppercase hex string into `[u8; N]`.
fn decode_hex_array<const N: usize>(hex: &str) -> Result<[u8; N], String> {
    if hex.len() != N * 2 {
        return Err(format!("expected {} hex chars, got {}", N * 2, hex.len()));
    }
    let mut out = [0u8; N];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).map_err(|_| "non-utf8".to_string())?;
        out[i] = u8::from_str_radix(s, 16).map_err(|_| "non-hex digit".to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::evidence::EvidenceBundleBuilder;
    use crate::audit::log::{AuditEvent, AuditLog};

    fn build_valid_bundle(dir: &Path) -> BundleManifest {
        let mut builder = EvidenceBundleBuilder::new(dir, "run-verify-001", "verify-test").unwrap();
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        builder
            .add_step_output("s1", "result", r#"{"value":1}"#)
            .unwrap();

        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        audit.append(AuditEvent::StepStarted {
            step_id: "s1".into(),
            input_hash: "inp".into(),
        });
        audit.append(AuditEvent::StepCompleted {
            step_id: "s1".into(),
            output_hash: "out".into(),
            duration_ms: 50,
        });
        audit.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 60,
        });

        builder.finalize(&audit).unwrap()
    }

    #[test]
    fn test_verify_valid_bundle() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        let result = verify_bundle(&bundle_dir);
        assert!(result.valid, "errors: {:?}", result.errors);
        assert!(result.redacted_entries.is_empty());
    }

    // ---- verifiable redaction at the bundle level ----

    /// Build a bundle whose audit log carries PII (an approver email at
    /// entry 1) so redaction has something to remove.
    fn build_bundle_with_pii(dir: &Path, run_id: &str) -> BundleManifest {
        let mut builder = EvidenceBundleBuilder::new(dir, run_id, "pii-test").unwrap();
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        audit.append(AuditEvent::ApprovalGranted {
            step_id: "s1".into(),
            approver: "alice.privacy@example.com".into(),
        });
        audit.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 60,
        });
        builder.finalize(&audit).unwrap()
    }

    #[test]
    fn redact_bundle_still_verifies_and_reports_redaction() {
        use crate::audit::evidence::redact_bundle;
        let dir = tempfile::tempdir().unwrap();
        let orig = build_bundle_with_pii(dir.path(), "run-redact-001");
        let bundle_dir = dir.path().join("run-redact-001");

        // Sanity: PII is present on disk before redaction.
        let before = std::fs::read_to_string(bundle_dir.join("audit_log.json")).unwrap();
        assert!(before.contains("alice.privacy@example.com"));

        let outcome = redact_bundle(&bundle_dir, 1, None, Some("GDPR erasure".into())).unwrap();

        // audit_log_hash is INVARIANT under redaction; bundle_hash changed.
        assert_eq!(outcome.audit_log_hash, orig.audit_log_hash);
        assert_ne!(outcome.new_bundle_hash, orig.bundle_hash);
        assert_eq!(outcome.redacted_sequence, 1);

        // Bundle STILL verifies and reports the redacted entry.
        let res = verify_bundle(&bundle_dir);
        assert!(
            res.valid,
            "redacted bundle must verify; errors: {:?}",
            res.errors
        );
        assert_eq!(res.redacted_entries, vec![1]);

        // The original PII is gone; the commitment + marker remain.
        let after = std::fs::read_to_string(bundle_dir.join("audit_log.json")).unwrap();
        assert!(!after.contains("alice.privacy@example.com"));
        assert!(after.contains(&outcome.content_sha256));
        assert!(after.contains("[REDACTED]"));
    }

    #[test]
    fn redact_bundle_then_tamper_placeholder_fails() {
        use crate::audit::evidence::redact_bundle;
        use crate::audit::log::AuditLog as Log;
        let dir = tempfile::tempdir().unwrap();
        build_bundle_with_pii(dir.path(), "run-redact-002");
        let bundle_dir = dir.path().join("run-redact-002");
        redact_bundle(&bundle_dir, 1, None, None).unwrap();
        assert!(verify_bundle(&bundle_dir).valid);

        // Forge a different original by corrupting the redacted entry's
        // committed hash in the file. The entry's own `content_sha256`
        // field serializes before its `redacted` marker, so replacing the
        // first occurrence splits the commitment from the marker AND from
        // entry_hash — the chain no longer recomputes.
        let audit_json = std::fs::read_to_string(bundle_dir.join("audit_log.json")).unwrap();
        let log = Log::from_json(&audit_json).unwrap();
        let corrupted = audit_json.replacen(&log.entries()[1].content_sha256, &"0".repeat(64), 1);
        std::fs::write(bundle_dir.join("audit_log.json"), &corrupted).unwrap();

        let res = verify_bundle(&bundle_dir);
        assert!(!res.valid, "redact-then-tamper must fail");
    }

    #[test]
    fn tamper_after_redact_content_fails_via_checksum() {
        use crate::audit::evidence::redact_bundle;
        let dir = tempfile::tempdir().unwrap();
        build_bundle_with_pii(dir.path(), "run-redact-003");
        let bundle_dir = dir.path().join("run-redact-003");
        redact_bundle(&bundle_dir, 1, None, None).unwrap();
        assert!(verify_bundle(&bundle_dir).valid);

        // A naive tamper of the audit log (without updating the manifest
        // checksum) is caught by the file_checksums loop.
        std::fs::write(
            bundle_dir.join("audit_log.json"),
            r#"[{"sequence":0,"event":{"WorkflowStarted":{"workflow_hash":"x","policy_hash":"y"}},"entry_hash":"z"}]"#,
        )
        .unwrap();
        assert!(!verify_bundle(&bundle_dir).valid);
    }

    #[test]
    fn redact_encrypted_bundle_is_rejected() {
        use crate::audit::evidence::{redact_bundle, BundleRedactError};
        let dir = tempfile::tempdir().unwrap();
        let kek = [7u8; KEY_LEN];
        let mut builder = EvidenceBundleBuilder::new(dir.path(), "run-redact-enc", "enc")
            .unwrap()
            .with_encryption(&kek, "default")
            .unwrap();
        builder.add_workflow_def(r#"{"name":"x"}"#).unwrap();
        builder.add_policy(r#"{}"#).unwrap();
        let mut audit = AuditLog::new();
        audit.append(AuditEvent::ApprovalGranted {
            step_id: "s1".into(),
            approver: "bob@example.com".into(),
        });
        builder.finalize(&audit).unwrap();
        let bundle_dir = dir.path().join("run-redact-enc");
        assert!(matches!(
            redact_bundle(&bundle_dir, 0, None, None),
            Err(BundleRedactError::EncryptedUnsupported)
        ));
    }

    #[test]
    fn test_verify_correct_anchor_passes() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        let result = verify_bundle_with_opts(
            &bundle_dir,
            &VerifyOptions {
                expected_bundle_hash: Some(&manifest.bundle_hash),
                ..Default::default()
            },
        );
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_verify_wrong_anchor_fails() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        let anchor = "a".repeat(64);
        let result = verify_bundle_with_opts(
            &bundle_dir,
            &VerifyOptions {
                expected_bundle_hash: Some(&anchor),
                ..Default::default()
            },
        );
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("bundle_hash_anchor_mismatch")));
    }

    #[test]
    fn test_verify_anchor_detects_forged_manifest() {
        // Motivated-attacker scenario: edit an output AND rewrite every checksum
        // + bundle_hash so the manifest is internally consistent. Without an
        // anchor this verifies; WITH the operator's out-of-band anchor it fails.
        let dir = tempfile::tempdir().unwrap();
        let good = build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        // Forge: rewrite a step output and re-hash it into the manifest, then
        // recompute bundle_hash so the bundle is self-consistent.
        let forged_output = r#"{"value":999}"#;
        let out_rel = "outputs/s1/result.json";
        std::fs::write(bundle_dir.join(out_rel), forged_output).unwrap();
        let mut manifest = good.clone();
        manifest
            .file_checksums
            .insert(out_rel.to_string(), sha256_bytes(forged_output.as_bytes()));
        manifest.bundle_hash = recompute_bundle_hash(&manifest).unwrap();
        std::fs::write(
            bundle_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        // Self-consistent → plain verify passes (the F1 weakness).
        assert!(verify_bundle(&bundle_dir).valid);
        // But the out-of-band anchor from the ORIGINAL bundle catches it.
        let anchored = verify_bundle_with_opts(
            &bundle_dir,
            &VerifyOptions {
                expected_bundle_hash: Some(&good.bundle_hash),
                ..Default::default()
            },
        );
        assert!(!anchored.valid);
        assert!(anchored
            .errors
            .iter()
            .any(|e| e.contains("bundle_hash_anchor_mismatch")));
    }

    #[test]
    fn test_verify_require_encryption_fails_on_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        let result = verify_bundle_with_opts(
            &bundle_dir,
            &VerifyOptions {
                require_encryption: true,
                ..Default::default()
            },
        );
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("encryption_required")));
    }

    #[test]
    fn test_verify_detects_tampered_file() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        // Tamper with workflow.json
        std::fs::write(bundle_dir.join("workflow.json"), r#"{"name":"TAMPERED"}"#).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("checksum mismatch")));
    }

    #[test]
    fn test_verify_detects_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        // Remove a required file
        std::fs::remove_file(bundle_dir.join("policy.json")).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("cannot read policy.json")));
    }

    #[test]
    fn test_verify_detects_tampered_audit_log() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        // Read and tamper with the audit log
        let audit_path = bundle_dir.join("audit_log.json");
        let mut audit_json = std::fs::read_to_string(&audit_path).unwrap();
        audit_json = audit_json.replace("abc", "TAMPERED");
        std::fs::write(&audit_path, &audit_json).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        // Should detect either chain break or checksum mismatch
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_verify_nonexistent_dir() {
        let result = verify_bundle(Path::new("/nonexistent/path"));
        assert!(!result.valid);
    }

    #[test]
    fn verify_rejects_missing_format_version() {
        // A legacy bundle has no bundle.json. The reader gate must
        // reject it before any further checks run.
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        std::fs::remove_file(bundle_dir.join("bundle.json")).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        let joined = result.errors.join(" ");
        assert!(
            joined.contains("legacy bundle") || joined.contains("missing bundle.json"),
            "expected legacy-bundle hint, got: {:?}",
            result.errors
        );
        assert!(
            joined.contains("boruna migrate evidence-bundle"),
            "expected migration hint, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn verify_rejects_future_major_version() {
        // A 2.x bundle MUST be rejected — major-version bumps are
        // breaking by spec.
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        let bundle_path = bundle_dir.join("bundle.json");
        let raw = std::fs::read_to_string(&bundle_path).unwrap();
        let mut parsed: BundleJson = serde_json::from_str(&raw).unwrap();
        parsed.format_version = "2.0".to_string();
        std::fs::write(&bundle_path, serde_json::to_string_pretty(&parsed).unwrap()).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(!result.valid);
        let joined = result.errors.join(" ");
        assert!(
            joined.contains("unsupported evidence bundle format_version") && joined.contains("2.0"),
            "expected unsupported-format error, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn verify_accepts_minor_bump() {
        // A 1.5 bundle MUST be accepted by a 1.0 reader — same-major
        // means forward-compat (unknown fields ignored).
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        let bundle_path = bundle_dir.join("bundle.json");
        let raw = std::fs::read_to_string(&bundle_path).unwrap();
        let mut parsed: BundleJson = serde_json::from_str(&raw).unwrap();
        parsed.format_version = "1.5".to_string();
        std::fs::write(&bundle_path, serde_json::to_string_pretty(&parsed).unwrap()).unwrap();

        let result = verify_bundle(&bundle_dir);
        assert!(
            result.valid,
            "1.5 bundle should pass 1.0 reader; got errors: {:?}",
            result.errors
        );
    }

    // ---- verify-A: ed25519 manifest signing ----

    fn signing_seed(base: u8) -> [u8; 32] {
        let mut s = [0u8; 32];
        for (i, b) in s.iter_mut().enumerate() {
            *b = base.wrapping_add((i as u8).wrapping_mul(7));
        }
        s
    }

    fn pubkey_hex(seed: &[u8; 32]) -> String {
        let sk = ed25519_dalek::SigningKey::from_bytes(seed);
        sk.verifying_key()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn build_signed_bundle(dir: &Path, seed: &[u8; 32]) -> BundleManifest {
        let mut builder = EvidenceBundleBuilder::new(dir, "run-sign-001", "sign-test")
            .unwrap()
            .with_signing_key(seed);
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        builder
            .add_step_output("s1", "result", r#"{"value":1}"#)
            .unwrap();

        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        audit.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 5,
        });
        builder.finalize(&audit).unwrap()
    }

    #[test]
    fn verify_signed_bundle_passes_and_records_signature() {
        let dir = tempfile::tempdir().unwrap();
        let seed = signing_seed(9);
        let manifest = build_signed_bundle(dir.path(), &seed);
        let bundle_dir = dir.path().join("run-sign-001");

        // Manifest carries the signature over bundle_hash.
        let sig = manifest
            .signature
            .expect("signed bundle must have signature");
        assert_eq!(sig.algorithm, "ed25519");
        assert_eq!(sig.public_key, pubkey_hex(&seed));

        // Plain verify (no signature requirements) passes.
        assert!(verify_bundle(&bundle_dir).valid);

        // Verify with the correct pinned key passes.
        let pk = pubkey_hex(&seed);
        let res = verify_bundle_with_opts(
            &bundle_dir,
            &VerifyOptions {
                trusted_pubkey: Some(&pk),
                require_signature: true,
                ..Default::default()
            },
        );
        assert!(res.valid, "errors: {:?}", res.errors);
    }

    #[test]
    fn verify_signed_bundle_tamper_fails() {
        let dir = tempfile::tempdir().unwrap();
        let seed = signing_seed(9);
        build_signed_bundle(dir.path(), &seed);
        let bundle_dir = dir.path().join("run-sign-001");

        // Tamper a covered file. bundle_hash in the manifest is stale;
        // the checksum loop catches the tamper regardless of signature.
        std::fs::write(bundle_dir.join("workflow.json"), r#"{"name":"EVIL"}"#).unwrap();

        let pk = pubkey_hex(&seed);
        let res = verify_bundle_with_opts(
            &bundle_dir,
            &VerifyOptions {
                trusted_pubkey: Some(&pk),
                ..Default::default()
            },
        );
        assert!(!res.valid);
        assert!(
            res.errors.iter().any(|e| e.contains("checksum mismatch")),
            "expected checksum mismatch, got: {:?}",
            res.errors
        );
    }

    #[test]
    fn verify_signed_bundle_hash_tamper_breaks_signature() {
        // Rewrite the manifest's bundle_hash without re-signing: the
        // recompute check + the ed25519 verify over the new bytes both
        // fail → signature_invalid.
        let dir = tempfile::tempdir().unwrap();
        let seed = signing_seed(9);
        build_signed_bundle(dir.path(), &seed);
        let bundle_dir = dir.path().join("run-sign-001");

        let manifest_path = bundle_dir.join("manifest.json");
        let raw = std::fs::read_to_string(&manifest_path).unwrap();
        let mut m: BundleManifest = serde_json::from_str(&raw).unwrap();
        m.bundle_hash = "0".repeat(64);
        std::fs::write(&manifest_path, serde_json::to_string_pretty(&m).unwrap()).unwrap();

        let res = verify_bundle(&bundle_dir);
        assert!(!res.valid);
        assert!(
            res.errors
                .iter()
                .any(|e| e.contains("evidence.signature_invalid")),
            "expected signature_invalid, got: {:?}",
            res.errors
        );
    }

    #[test]
    fn verify_signed_bundle_wrong_pinned_key_untrusted() {
        let dir = tempfile::tempdir().unwrap();
        let seed = signing_seed(9);
        build_signed_bundle(dir.path(), &seed);
        let bundle_dir = dir.path().join("run-sign-001");

        // Pin a DIFFERENT key than the one that signed the bundle.
        let wrong_pk = pubkey_hex(&signing_seed(42));
        let res = verify_bundle_with_opts(
            &bundle_dir,
            &VerifyOptions {
                trusted_pubkey: Some(&wrong_pk),
                ..Default::default()
            },
        );
        assert!(!res.valid);
        assert!(
            res.errors
                .iter()
                .any(|e| e.contains("evidence.signature_untrusted_key")),
            "expected signature_untrusted_key, got: {:?}",
            res.errors
        );
    }

    #[test]
    fn verify_require_signature_on_unsigned_fails() {
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");

        let res = verify_bundle_with_opts(
            &bundle_dir,
            &VerifyOptions {
                require_signature: true,
                ..Default::default()
            },
        );
        assert!(!res.valid);
        assert!(
            res.errors
                .iter()
                .any(|e| e.contains("evidence.signature_required")),
            "expected signature_required, got: {:?}",
            res.errors
        );
    }

    #[test]
    fn verify_unsigned_bundle_without_require_still_passes() {
        // The signature feature is additive: an unsigned bundle still
        // verifies when no signature is required.
        let dir = tempfile::tempdir().unwrap();
        build_valid_bundle(dir.path());
        let bundle_dir = dir.path().join("run-verify-001");
        assert!(verify_bundle(&bundle_dir).valid);
    }

    /// A gate record whose audit entry was redacted is still bound by the chain's content
    /// hash. Forging the score (with a decision that is consistent with it) must be caught
    /// even though no readable audit record exists for the gate.
    #[test]
    fn forged_gate_record_is_rejected_even_when_its_audit_entry_is_redacted() {
        use crate::confidence::{
            calibration_sha256, decide, risk_threshold, CalibrationSet, GateRecord,
        };
        use crate::workflow::confidence_gate::audit_event_for_record;

        let ex: Vec<serde_json::Value> = (0..100u32)
            .map(|i| serde_json::json!({"score": i * 10, "correct": i * 10 >= 500}))
            .collect();
        let cal =
            serde_json::to_string(&serde_json::json!({"version": 1, "examples": ex})).unwrap();
        let set = CalibrationSet::from_json(&cal).unwrap();
        let threshold = risk_threshold(&set, 50).unwrap();
        let record = |score: u32| GateRecord {
            step_id: "gate".into(),
            source_step: "classify".into(),
            alpha_permille: 50,
            calibration_sha256: calibration_sha256(cal.as_bytes()),
            calibration_examples: set.examples.len(),
            threshold_permille: threshold,
            score_permille: Some(score),
            decision: decide(threshold, Some(score)),
        };

        let run = |written: &GateRecord, committed: &GateRecord| -> Vec<String> {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("confidence")).unwrap();
            std::fs::write(
                dir.path().join("confidence_gates.json"),
                serde_json::to_string(&vec![written]).unwrap(),
            )
            .unwrap();
            std::fs::write(dir.path().join("confidence/gate.calibration.json"), &cal).unwrap();
            // The chain committed to `committed`; its entry is redacted, so no readable
            // record is passed, only the content hash.
            let hashes: std::collections::BTreeSet<String> =
                [AuditLog::content_hash(&audit_event_for_record(committed))].into();
            let mut errors = Vec::new();
            verify_confidence_gates(dir.path(), None, &[], &hashes, &[], &mut errors);
            errors
        };

        // The record the run really committed verifies.
        assert!(run(&record(870), &record(870)).is_empty());
        // The same file with the score forged to 999 (decision still consistent) is caught.
        let errs = run(&record(999), &record(870));
        assert!(
            errs.iter()
                .any(|e| e.contains("does not match any decision committed")),
            "{errs:?}"
        );
    }

    /// The gate's audit entry is redacted AND its record is dropped from the file (or the
    /// file is removed). With nothing readable left in the chain, only the redacted entry's
    /// hash shows something was committed. A workflow that declares gates must account for it.
    #[test]
    fn dropping_a_gate_record_behind_a_redacted_entry_is_rejected() {
        use crate::confidence::{
            calibration_sha256, decide, risk_threshold, CalibrationSet, GateRecord,
        };
        use crate::workflow::confidence_gate::audit_event_for_record;

        let ex: Vec<serde_json::Value> = (0..100u32)
            .map(|i| serde_json::json!({"score": i * 10, "correct": i * 10 >= 500}))
            .collect();
        let cal =
            serde_json::to_string(&serde_json::json!({"version": 1, "examples": ex})).unwrap();
        let set = CalibrationSet::from_json(&cal).unwrap();
        let threshold = risk_threshold(&set, 50).unwrap();
        let rec = GateRecord {
            step_id: "gate".into(),
            source_step: "classify".into(),
            alpha_permille: 50,
            calibration_sha256: calibration_sha256(cal.as_bytes()),
            calibration_examples: set.examples.len(),
            threshold_permille: threshold,
            score_permille: Some(870),
            decision: decide(threshold, Some(870)),
        };
        let redacted_hash = AuditLog::content_hash(&audit_event_for_record(&rec));
        let hashes: std::collections::BTreeSet<String> = [redacted_hash.clone()].into();

        let run = |declares_gates: bool, file: Option<&str>| -> Vec<String> {
            let dir = tempfile::tempdir().unwrap();
            let wf = if declares_gates {
                r#"{"steps":{"gate":{"kind":"approval_gate","confidence_gate":{"source_step":"c","calibration":"c.json","alpha_permille":50}}}}"#
            } else {
                r#"{"steps":{"gate":{"kind":"approval_gate"}}}"#
            };
            std::fs::write(dir.path().join("workflow.json"), wf).unwrap();
            if let Some(f) = file {
                std::fs::write(dir.path().join("confidence_gates.json"), f).unwrap();
                std::fs::create_dir_all(dir.path().join("confidence")).unwrap();
                std::fs::write(dir.path().join("confidence/gate.calibration.json"), &cal).unwrap();
            }
            let mut errors = Vec::new();
            verify_confidence_gates(
                dir.path(),
                None,
                &[],
                &hashes,
                &[redacted_hash.clone()],
                &mut errors,
            );
            errors
        };

        let with_record = serde_json::to_string(&vec![&rec]).unwrap();
        // Honest bundle: record present, entry redacted -> fine.
        assert!(run(true, Some(&with_record)).is_empty());
        // Record dropped (empty list) or file removed: rejected.
        for file in [Some("[]"), None] {
            let errs = run(true, file);
            assert!(
                errs.iter().any(|e| e.contains("may have been dropped")),
                "{file:?}: {errs:?}"
            );
        }
        // A workflow with no confidence gates is not affected by an unrelated redaction.
        assert!(run(false, None).is_empty());
    }
}
