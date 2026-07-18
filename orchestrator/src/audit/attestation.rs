//! Interop layer: emit an evidence bundle's provenance ALSO as a
//! standard in-toto Statement wrapped in a DSSE envelope, so Boruna
//! evidence is consumable by the supply-chain ecosystem (`cosign
//! verify-blob`, `in-toto-verify`) WITHOUT changing the native bundle
//! format. This is purely ADDITIVE — nothing here touches the manifest,
//! its `bundle_hash`, or the existing verify path.
//!
//! Two artifacts are produced from a finalized [`BundleManifest`]:
//!
//! 1. an **in-toto Statement v1** — `_type`,
//!    `subject[]` (the run's component files by SHA-256, plus the bundle
//!    itself keyed by `bundle_hash`), `predicateType`
//!    (`https://boruna.dev/runtime-provenance/v1`), and a `predicate`
//!    that maps the manifest fields into a SLSA-provenance-shaped
//!    `buildDefinition` / `runDetails` structure; and
//! 2. a **DSSE envelope** wrapping that Statement, whose signature is an
//!    ed25519 signature over the DSSE **PAE** pre-authentication
//!    encoding of `(payloadType, payload)`.
//!
//! The signing key is the SAME ed25519 key used for manifest signing
//! (`EvidenceBundleBuilder::with_signing_key`); the `keyid` is the hex
//! public key, matching `ManifestSignature.public_key`. No new keypair
//! is introduced.
//!
//! Determinism: the Statement is serialized with canonical (sorted-key /
//! fixed field-order) JSON via `serde_json`, and those exact bytes are
//! what get base64-encoded into the payload and signed. Same run →
//! same manifest → same Statement bytes → same PAE → same signature.
//!
//! See `docs/spec/runtime-provenance-predicate-1.0.md` for the predicate
//! schema and a worked `cosign` / `in-toto-verify` example.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::audit::evidence::BundleManifest;
use crate::audit::fingerprint::EnvFingerprint;

/// in-toto Statement `_type` for the v1 spec.
pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";

/// Boruna's runtime-provenance predicate type (versioned).
pub const PREDICATE_TYPE: &str = "https://boruna.dev/runtime-provenance/v1";

/// DSSE payload type for an in-toto Statement, per the in-toto spec.
pub const DSSE_PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";

/// Build type recorded in the predicate's `buildDefinition.buildType`.
pub const BUILD_TYPE: &str = "https://boruna.dev/workflow-run/v1";

/// Errors from building or verifying an attestation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestError {
    /// The supplied signing seed was not 64 hex chars / 32 bytes.
    BadSigningKey(String),
    /// A DSSE artifact could not be (de)serialized.
    Serialization(String),
    /// The DSSE payload was not valid base64.
    BadPayloadBase64(String),
    /// The envelope's `payloadType` was not the expected in-toto type.
    UnexpectedPayloadType { found: String },
    /// A signature's `sig` or `keyid` was malformed hex/base64.
    BadSignatureEncoding(String),
    /// The envelope had no signatures to verify.
    NoSignatures,
    /// ed25519 verification failed over the PAE for every signature.
    SignatureInvalid,
    /// A trusted key was pinned but no signature was made by it.
    UntrustedKey { pinned: String },
}

impl std::fmt::Display for AttestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttestError::BadSigningKey(m) => write!(f, "invalid signing key: {m}"),
            AttestError::Serialization(m) => write!(f, "attestation serialization failed: {m}"),
            AttestError::BadPayloadBase64(m) => write!(f, "DSSE payload is not valid base64: {m}"),
            AttestError::UnexpectedPayloadType { found } => write!(
                f,
                "unexpected DSSE payloadType {found:?} (expected {DSSE_PAYLOAD_TYPE:?})"
            ),
            AttestError::BadSignatureEncoding(m) => write!(f, "bad signature encoding: {m}"),
            AttestError::NoSignatures => write!(f, "DSSE envelope has no signatures"),
            AttestError::SignatureInvalid => {
                write!(f, "ed25519 signature does not verify over the DSSE PAE")
            }
            AttestError::UntrustedKey { pinned } => {
                write!(f, "no signature was made by the pinned key {pinned}")
            }
        }
    }
}

impl std::error::Error for AttestError {}

/// A single in-toto `subject`: a named artifact by digest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Subject {
    pub name: String,
    /// Digest algorithm → lowercase-hex digest. Always contains
    /// `sha256`. `BTreeMap` for deterministic key ordering.
    pub digest: BTreeMap<String, String>,
}

/// SLSA-shaped `buildDefinition` half of the predicate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildDefinition {
    #[serde(rename = "buildType")]
    pub build_type: String,
    /// Operator-facing inputs that identify the run: the workflow and
    /// policy the run was bound to (by content hash) plus the workflow
    /// name.
    #[serde(rename = "externalParameters")]
    pub external_parameters: BTreeMap<String, String>,
    /// Environment fingerprint captured at finalize time.
    #[serde(rename = "internalParameters")]
    pub internal_parameters: InternalParameters,
}

/// Internal (platform-captured) parameters for the run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InternalParameters {
    #[serde(rename = "borunaVersion")]
    pub boruna_version: String,
    #[serde(rename = "envFingerprint")]
    pub env_fingerprint: EnvFingerprint,
}

/// SLSA-shaped `runDetails` half of the predicate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunDetails {
    pub builder: Builder,
    pub metadata: RunMetadata,
    /// Non-artifact outputs of the run that are still evidence: the
    /// hash-chained audit-log hash and the native bundle hash.
    /// `BTreeMap` for deterministic ordering.
    pub byproducts: BTreeMap<String, String>,
}

/// Identifies the builder (the Boruna engine) that produced the run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Builder {
    pub id: String,
}

/// Run invocation metadata (SLSA `runDetails.metadata` shape).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunMetadata {
    #[serde(rename = "invocationId")]
    pub invocation_id: String,
    #[serde(rename = "startedOn")]
    pub started_on: String,
    #[serde(rename = "finishedOn")]
    pub finished_on: String,
}

/// The `predicate` body: a documented mapping of the manifest fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeProvenancePredicate {
    #[serde(rename = "buildDefinition")]
    pub build_definition: BuildDefinition,
    #[serde(rename = "runDetails")]
    pub run_details: RunDetails,
}

/// A full in-toto Statement (v1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InTotoStatement {
    #[serde(rename = "_type")]
    pub type_: String,
    pub subject: Vec<Subject>,
    #[serde(rename = "predicateType")]
    pub predicate_type: String,
    pub predicate: RuntimeProvenancePredicate,
}

/// One DSSE signature.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DsseSignature {
    /// base64(ed25519 signature over `PAE(payloadType, payload)`).
    pub sig: String,
    /// Lowercase-hex ed25519 public key (matches
    /// `ManifestSignature.public_key`).
    pub keyid: String,
}

/// A DSSE envelope wrapping a base64 in-toto Statement payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DsseEnvelope {
    /// base64(canonical Statement JSON bytes).
    pub payload: String,
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    pub signatures: Vec<DsseSignature>,
}

/// DSSE Pre-Authentication Encoding of `(payload_type, payload)`.
///
/// `PAE(type, body) = "DSSEv1" SP LEN(type) SP type SP LEN(body) SP body`
/// where `SP` is a single ASCII space and `LEN` is the ASCII-decimal
/// byte length. This is the exact string that gets signed — it binds
/// the payload type into the signature so a signature over one type
/// cannot be replayed as another. See the DSSE spec (`protocol.md`).
pub fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + payload_type.len() + 32);
    out.extend_from_slice(b"DSSEv1 ");
    out.extend_from_slice(payload_type.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload_type.as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload);
    out
}

/// Build the in-toto Statement for a finalized bundle manifest.
///
/// `subject[]` is every component file the manifest checksums (by its
/// SHA-256), plus a synthetic subject for the bundle itself keyed by
/// `bundle_hash`. The predicate maps the manifest's provenance fields
/// into a SLSA-shaped structure. Fully deterministic: `file_checksums`
/// is a `BTreeMap` so subjects come out in sorted-name order.
pub fn statement_from_manifest(manifest: &BundleManifest, boruna_version: &str) -> InTotoStatement {
    let mut subject: Vec<Subject> = manifest
        .file_checksums
        .iter()
        .map(|(name, sha)| {
            let mut digest = BTreeMap::new();
            digest.insert("sha256".to_string(), sha.clone());
            Subject {
                name: name.clone(),
                digest,
            }
        })
        .collect();
    // The bundle itself as a subject, keyed by its manifest bundle_hash.
    {
        let mut digest = BTreeMap::new();
        digest.insert("sha256".to_string(), manifest.bundle_hash.clone());
        subject.push(Subject {
            name: format!("boruna-bundle:{}", manifest.run_id),
            digest,
        });
    }

    let mut external_parameters = BTreeMap::new();
    external_parameters.insert("workflowName".to_string(), manifest.workflow_name.clone());
    external_parameters.insert("workflowHash".to_string(), manifest.workflow_hash.clone());
    external_parameters.insert("policyHash".to_string(), manifest.policy_hash.clone());

    let mut byproducts = BTreeMap::new();
    byproducts.insert("auditLogHash".to_string(), manifest.audit_log_hash.clone());
    byproducts.insert("bundleHash".to_string(), manifest.bundle_hash.clone());

    InTotoStatement {
        type_: STATEMENT_TYPE.to_string(),
        subject,
        predicate_type: PREDICATE_TYPE.to_string(),
        predicate: RuntimeProvenancePredicate {
            build_definition: BuildDefinition {
                build_type: BUILD_TYPE.to_string(),
                external_parameters,
                internal_parameters: InternalParameters {
                    boruna_version: boruna_version.to_string(),
                    env_fingerprint: manifest.env_fingerprint.clone(),
                },
            },
            run_details: RunDetails {
                builder: Builder {
                    id: format!("https://boruna.dev/boruna@{boruna_version}"),
                },
                metadata: RunMetadata {
                    invocation_id: manifest.run_id.clone(),
                    started_on: manifest.started_at.clone(),
                    finished_on: manifest.completed_at.clone(),
                },
                byproducts,
            },
        },
    }
}

/// Serialize a Statement to its canonical (deterministic) JSON bytes.
///
/// `serde_json` emits struct fields in declaration order and `BTreeMap`
/// keys in sorted order, so the output is byte-stable for a given
/// Statement — exactly what the DSSE payload/signature require. These
/// bytes (not a re-serialization) are what get base64-encoded into the
/// payload and fed through the PAE.
pub fn statement_to_canonical_bytes(statement: &InTotoStatement) -> Result<Vec<u8>, AttestError> {
    serde_json::to_vec(statement).map_err(|e| AttestError::Serialization(e.to_string()))
}

/// Parse a 32-byte ed25519 signing seed from 64 hex chars.
pub fn parse_seed_hex(hex: &str) -> Result<[u8; 32], AttestError> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(AttestError::BadSigningKey(format!(
            "expected 64 hex chars (32 bytes), got {}",
            hex.len()
        )));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk)
            .map_err(|_| AttestError::BadSigningKey("non-utf8".to_string()))?;
        out[i] = u8::from_str_radix(s, 16)
            .map_err(|_| AttestError::BadSigningKey("non-hex digit".to_string()))?;
    }
    Ok(out)
}

/// Produce a signed DSSE envelope for a manifest, using the ed25519 key
/// derived from `signing_seed` (the SAME key machinery as manifest
/// signing). The signature is over `PAE(payloadType, payload)`; the
/// `keyid` is the hex public key.
pub fn attest(
    manifest: &BundleManifest,
    boruna_version: &str,
    signing_seed: &[u8; 32],
) -> Result<DsseEnvelope, AttestError> {
    let statement = statement_from_manifest(manifest, boruna_version);
    let payload_bytes = statement_to_canonical_bytes(&statement)?;
    sign_statement_bytes(&payload_bytes, signing_seed)
}

/// Sign already-serialized Statement bytes into a DSSE envelope. Split
/// out from [`attest`] so tests can exercise the PAE/signature path
/// against known payload bytes.
pub fn sign_statement_bytes(
    payload_bytes: &[u8],
    signing_seed: &[u8; 32],
) -> Result<DsseEnvelope, AttestError> {
    use ed25519_dalek::Signer;
    let sk = ed25519_dalek::SigningKey::from_bytes(signing_seed);
    let to_sign = pae(DSSE_PAYLOAD_TYPE, payload_bytes);
    let sig = sk.sign(&to_sign);

    let b64 = base64::engine::general_purpose::STANDARD;
    Ok(DsseEnvelope {
        payload: b64.encode(payload_bytes),
        payload_type: DSSE_PAYLOAD_TYPE.to_string(),
        signatures: vec![DsseSignature {
            sig: b64.encode(sig.to_bytes()),
            keyid: to_hex(sk.verifying_key().as_bytes()),
        }],
    })
}

/// Verify a DSSE envelope: check the `payloadType`, then verify each
/// signature's ed25519 sig over `PAE(payloadType, payload)` using the
/// signature's own `keyid` as the public key. Returns the decoded
/// Statement on success.
///
/// When `trusted_pubkey` is `Some`, verification additionally requires
/// that at least one VALID signature was made by that pinned key —
/// otherwise an attacker who re-signs a mutated payload with their own
/// key would pass. Without a pin, a valid self-consistent signature is
/// accepted (the caller vouches for the key out of band, e.g. via the
/// manifest's `signature.public_key`).
pub fn verify_envelope(
    envelope: &DsseEnvelope,
    trusted_pubkey: Option<&str>,
) -> Result<InTotoStatement, AttestError> {
    if envelope.payload_type != DSSE_PAYLOAD_TYPE {
        return Err(AttestError::UnexpectedPayloadType {
            found: envelope.payload_type.clone(),
        });
    }
    if envelope.signatures.is_empty() {
        return Err(AttestError::NoSignatures);
    }

    let b64 = base64::engine::general_purpose::STANDARD;
    let payload_bytes = b64
        .decode(envelope.payload.as_bytes())
        .map_err(|e| AttestError::BadPayloadBase64(e.to_string()))?;
    let to_verify = pae(&envelope.payload_type, &payload_bytes);

    let mut any_valid = false;
    let mut pinned_valid = false;
    for s in &envelope.signatures {
        let pk_bytes = match decode_hex_array::<32>(&s.keyid) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let sig_bytes = match b64.decode(s.sig.as_bytes()) {
            Ok(b) if b.len() == 64 => {
                let mut arr = [0u8; 64];
                arr.copy_from_slice(&b);
                arr
            }
            _ => continue,
        };
        let vk = match ed25519_dalek::VerifyingKey::from_bytes(&pk_bytes) {
            Ok(vk) => vk,
            Err(_) => continue,
        };
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        use ed25519_dalek::Verifier;
        if vk.verify(&to_verify, &signature).is_ok() {
            any_valid = true;
            if let Some(pin) = trusted_pubkey {
                if pin.eq_ignore_ascii_case(&s.keyid) {
                    pinned_valid = true;
                }
            }
        }
    }

    if !any_valid {
        return Err(AttestError::SignatureInvalid);
    }
    if let Some(pin) = trusted_pubkey {
        if !pinned_valid {
            return Err(AttestError::UntrustedKey {
                pinned: pin.to_string(),
            });
        }
    }

    let statement: InTotoStatement = serde_json::from_slice(&payload_bytes)
        .map_err(|e| AttestError::Serialization(e.to_string()))?;
    Ok(statement)
}

/// Lowercase-hex encode bytes.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Decode a fixed-length hex string into `[u8; N]`.
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
    use std::path::Path;

    fn signing_seed(base: u8) -> [u8; 32] {
        let mut s = [0u8; 32];
        for (i, b) in s.iter_mut().enumerate() {
            *b = base.wrapping_add((i as u8).wrapping_mul(7));
        }
        s
    }

    fn pubkey_hex(seed: &[u8; 32]) -> String {
        let sk = ed25519_dalek::SigningKey::from_bytes(seed);
        to_hex(sk.verifying_key().as_bytes())
    }

    fn build_manifest(dir: &Path) -> BundleManifest {
        let mut builder = EvidenceBundleBuilder::new(dir, "run-attest-001", "attest-test").unwrap();
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
            total_duration_ms: 7,
        });
        builder.finalize(&audit).unwrap()
    }

    #[test]
    fn pae_matches_dsse_spec_known_vector() {
        // From the DSSE spec (protocol.md) worked example:
        //   payloadType = "http://example.com/HelloWorld"
        //   payload     = "hello world"
        //   PAE = "DSSEv1 29 http://example.com/HelloWorld 11 hello world"
        let got = pae("http://example.com/HelloWorld", b"hello world");
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "DSSEv1 29 http://example.com/HelloWorld 11 hello world"
        );
    }

    #[test]
    fn pae_binds_payload_type_and_length() {
        // Empty payload still encodes the "0" length token.
        assert_eq!(
            String::from_utf8(pae("application/vnd.in-toto+json", b"")).unwrap(),
            "DSSEv1 28 application/vnd.in-toto+json 0 "
        );
    }

    #[test]
    fn statement_has_correct_types_and_subject_digests() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_manifest(dir.path());
        let stmt = statement_from_manifest(&manifest, "9.9.9");

        assert_eq!(stmt.type_, STATEMENT_TYPE);
        assert_eq!(stmt.predicate_type, PREDICATE_TYPE);

        // Every manifest file checksum appears as a subject sha256.
        for (name, sha) in &manifest.file_checksums {
            let subj = stmt
                .subject
                .iter()
                .find(|s| &s.name == name)
                .unwrap_or_else(|| panic!("missing subject {name}"));
            assert_eq!(subj.digest.get("sha256"), Some(sha));
        }
        // Plus the synthetic bundle subject keyed by bundle_hash.
        let bundle_subj = stmt
            .subject
            .iter()
            .find(|s| s.name == "boruna-bundle:run-attest-001")
            .expect("bundle subject present");
        assert_eq!(
            bundle_subj.digest.get("sha256"),
            Some(&manifest.bundle_hash)
        );

        // Predicate maps the manifest fields.
        let ext = &stmt.predicate.build_definition.external_parameters;
        assert_eq!(ext.get("workflowHash"), Some(&manifest.workflow_hash));
        assert_eq!(ext.get("policyHash"), Some(&manifest.policy_hash));
        assert_eq!(
            stmt.predicate.run_details.byproducts.get("auditLogHash"),
            Some(&manifest.audit_log_hash)
        );
        assert_eq!(
            stmt.predicate.run_details.metadata.invocation_id,
            manifest.run_id
        );
    }

    #[test]
    fn statement_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_manifest(dir.path());
        let stmt = statement_from_manifest(&manifest, "1.2.3");
        let bytes = statement_to_canonical_bytes(&stmt).unwrap();
        let back: InTotoStatement = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stmt, back);
    }

    #[test]
    fn statement_bytes_are_deterministic() {
        // Same manifest → identical canonical bytes (→ identical sig).
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_manifest(dir.path());
        let a = statement_to_canonical_bytes(&statement_from_manifest(&manifest, "1.0.0")).unwrap();
        let b = statement_to_canonical_bytes(&statement_from_manifest(&manifest, "1.0.0")).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn attest_then_verify_passes() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_manifest(dir.path());
        let seed = signing_seed(3);
        let env = attest(&manifest, "1.0.0", &seed).unwrap();

        assert_eq!(env.payload_type, DSSE_PAYLOAD_TYPE);
        assert_eq!(env.signatures.len(), 1);
        assert_eq!(env.signatures[0].keyid, pubkey_hex(&seed));

        // Unpinned verify passes and returns the Statement.
        let stmt = verify_envelope(&env, None).unwrap();
        assert_eq!(stmt.type_, STATEMENT_TYPE);

        // Pinned to the correct key passes.
        let pk = pubkey_hex(&seed);
        let stmt2 = verify_envelope(&env, Some(&pk)).unwrap();
        assert_eq!(stmt2, stmt);
    }

    #[test]
    fn verify_fails_on_mutated_payload() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_manifest(dir.path());
        let seed = signing_seed(3);
        let mut env = attest(&manifest, "1.0.0", &seed).unwrap();

        // Mutate the payload: decode, tamper a byte, re-encode. The
        // signature is over the ORIGINAL payload's PAE, so it must fail.
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut raw = b64.decode(env.payload.as_bytes()).unwrap();
        // Flip a byte well inside the JSON body.
        raw[10] ^= 0xFF;
        env.payload = b64.encode(&raw);

        let err = verify_envelope(&env, None).unwrap_err();
        assert_eq!(err, AttestError::SignatureInvalid);
    }

    #[test]
    fn verify_fails_on_wrong_pinned_key() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_manifest(dir.path());
        let seed = signing_seed(3);
        let env = attest(&manifest, "1.0.0", &seed).unwrap();

        let wrong = pubkey_hex(&signing_seed(42));
        let err = verify_envelope(&env, Some(&wrong)).unwrap_err();
        assert_eq!(err, AttestError::UntrustedKey { pinned: wrong });
    }

    #[test]
    fn verify_rejects_wrong_payload_type() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_manifest(dir.path());
        let seed = signing_seed(3);
        let mut env = attest(&manifest, "1.0.0", &seed).unwrap();
        env.payload_type = "application/json".to_string();
        let err = verify_envelope(&env, None).unwrap_err();
        assert!(matches!(err, AttestError::UnexpectedPayloadType { .. }));
    }

    #[test]
    fn parse_seed_hex_roundtrip_and_errors() {
        let seed = signing_seed(5);
        let hex = to_hex(&seed);
        assert_eq!(parse_seed_hex(&hex).unwrap(), seed);
        assert!(matches!(
            parse_seed_hex("abc"),
            Err(AttestError::BadSigningKey(_))
        ));
        assert!(matches!(
            parse_seed_hex(&"zz".repeat(32)),
            Err(AttestError::BadSigningKey(_))
        ));
    }
}
