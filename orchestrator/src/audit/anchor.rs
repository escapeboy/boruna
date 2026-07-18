//! Transparency-log anchoring for evidence bundles (Sigstore Rekor).
//!
//! ## Why this exists
//!
//! Boruna's native evidence bundle is *tamper-evidence*: a SHA-256
//! hash chain (`bundle_hash` over `file_checksums` + `audit_log_hash`)
//! proves internal consistency, and an optional ed25519
//! [`ManifestSignature`] roots that in an operator key. But the
//! key-holder can still regenerate and **backdate** the whole chain —
//! there is no external witness that the bundle existed at a given
//! time, so it is not *non-repudiation*.
//!
//! A **Sigstore Rekor** transparency log closes that hole. Rekor is an
//! append-only, externally-witnessed Merkle log (RFC 6962 / Trillian).
//! Submitting an entry gets back:
//!
//! - a `logIndex` + `integratedTime` (a trusted timestamp), and
//! - an **inclusion proof** (Merkle audit path) against a signed log
//!   root, plus a signed entry timestamp (SET).
//!
//! Anyone can later re-derive the log root from the entry + proof and
//! confirm the entry was present — the log operator cannot silently
//! drop or backdate it. This is the missing external witness.
//!
//! ## What this module does
//!
//! 1. Builds a Rekor **`hashedrekord`** entry payload from a bundle's
//!    `bundle_hash` + ed25519 [`ManifestSignature`] (or raw components):
//!    the canonical JSON a `POST /api/v1/log/entries` expects.
//! 2. Verifies a stored `rekor-entry.json` **offline**: recomputes the
//!    RFC 6962 Merkle root from the entry's leaf hash + inclusion proof
//!    and checks it against the proof's `rootHash`, and confirms the
//!    entry commits to the bundle's `bundle_hash`.
//!
//! The live HTTP submit path ([`submit`]) is behind the `rekor` cargo
//! feature, so the DEFAULT build (and all tests) are network-free. A
//! **private Rekor URL** may be supplied for air-gapped deployments —
//! nothing here hard-codes the public instance except the CLI default.
//!
//! ## Keyless (Fulcio) signing
//!
//! This slice deliberately anchors an *already-signed* bundle
//! (self-managed ed25519 key). Full keyless OIDC → Fulcio short-lived
//! certs is sketched in `orchestrator/docs/keyless-signing.md` and is
//! NOT implemented here.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::audit::evidence::BundleManifest;

/// Rekor `hashedrekord` schema version this module emits.
pub const HASHEDREKORD_API_VERSION: &str = "0.0.1";
/// Rekor entry kind.
pub const HASHEDREKORD_KIND: &str = "hashedrekord";
/// Default public Sigstore Rekor instance (the CLI default; any private
/// URL is accepted for air-gapped deployments).
pub const DEFAULT_REKOR_URL: &str = "https://rekor.sigstore.dev";

/// Errors from building or verifying a transparency-log anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorError {
    /// The manifest carried no ed25519 signature to anchor. Sign the
    /// bundle first (`EvidenceBundleBuilder::with_signing_key`).
    UnsignedManifest,
    /// A hex field (signature / public key / proof hash) was malformed.
    BadHex(String),
    /// A base64 field was malformed.
    BadBase64(String),
    /// (De)serialization of an entry / response failed.
    Serialization(String),
    /// The inclusion proof's `logIndex` was not `< treeSize`.
    IndexOutOfRange { index: u64, tree_size: u64 },
    /// The proof had fewer hashes than the entry's position requires.
    ProofTooShort { have: usize, need: usize },
    /// The Merkle root recomputed from the proof did not match the
    /// proof's stated `rootHash`. The entry is not provably in the log.
    InclusionProofMismatch { computed: String, expected: String },
    /// The entry commits to a different artifact hash than the bundle's
    /// `bundle_hash` — the anchor is for a different bundle.
    DataHashMismatch { entry: String, bundle: String },
    /// Live submission failed (only reachable with the `rekor` feature).
    Network(String),
}

impl std::fmt::Display for AnchorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnchorError::UnsignedManifest => write!(
                f,
                "manifest has no ed25519 signature to anchor (sign the bundle first)"
            ),
            AnchorError::BadHex(m) => write!(f, "invalid hex: {m}"),
            AnchorError::BadBase64(m) => write!(f, "invalid base64: {m}"),
            AnchorError::Serialization(m) => write!(f, "rekor entry serialization failed: {m}"),
            AnchorError::IndexOutOfRange { index, tree_size } => {
                write!(
                    f,
                    "inclusion proof index {index} out of range (treeSize {tree_size})"
                )
            }
            AnchorError::ProofTooShort { have, need } => {
                write!(
                    f,
                    "inclusion proof too short: have {have} hashes, need at least {need}"
                )
            }
            AnchorError::InclusionProofMismatch { computed, expected } => write!(
                f,
                "inclusion proof does not verify: recomputed root {computed} != rootHash {expected}"
            ),
            AnchorError::DataHashMismatch { entry, bundle } => write!(
                f,
                "anchor is for a different bundle: entry data hash {entry} != bundle_hash {bundle}"
            ),
            AnchorError::Network(m) => write!(f, "rekor submission failed: {m}"),
        }
    }
}

impl std::error::Error for AnchorError {}

// ---------------------------------------------------------------------------
// hashedrekord entry payload (the POST body)
// ---------------------------------------------------------------------------

/// `spec.data.hash`: the artifact digest the entry commits to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HashSpec {
    pub algorithm: String,
    pub value: String,
}

/// `spec.data`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataSpec {
    pub hash: HashSpec,
}

/// `spec.signature.publicKey`. Rekor expects a PEM-encoded key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicKeySpec {
    /// base64(PEM(SubjectPublicKeyInfo)) of the ed25519 public key.
    pub content: String,
}

/// `spec.signature`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignatureSpec {
    /// base64(raw 64-byte ed25519 signature over the artifact).
    pub content: String,
    #[serde(rename = "publicKey")]
    pub public_key: PublicKeySpec,
}

/// `spec` of a `hashedrekord`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HashedRekordSpec {
    pub data: DataSpec,
    pub signature: SignatureSpec,
}

/// A full Rekor `hashedrekord` proposed-entry payload.
///
/// This is the canonical JSON a `POST /api/v1/log/entries` accepts. The
/// entry commits to `{ data.hash, signature.content, publicKey.content }`
/// — i.e. "this public key signed the artifact whose SHA-256 is X".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RekorEntry {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub spec: HashedRekordSpec,
}

/// Build a `hashedrekord` entry from a finalized, **signed** bundle
/// manifest. The committed data hash is `manifest.bundle_hash`; the
/// signature + public key come from the manifest's [`ManifestSignature`]
/// (the ed25519 signature over `bundle_hash`).
pub fn hashedrekord_from_manifest(manifest: &BundleManifest) -> Result<RekorEntry, AnchorError> {
    let sig = manifest
        .signature
        .as_ref()
        .ok_or(AnchorError::UnsignedManifest)?;
    hashedrekord_entry(&manifest.bundle_hash, &sig.signature, &sig.public_key)
}

/// Build a `hashedrekord` entry from raw components: the artifact
/// SHA-256 (hex), the ed25519 signature over it (hex, 64 bytes), and the
/// ed25519 public key (hex, 32 bytes). The public key is re-encoded as a
/// PEM `SubjectPublicKeyInfo` (what Rekor stores), and both signature and
/// key are base64-wrapped per the `hashedrekord` schema.
pub fn hashedrekord_entry(
    data_sha256_hex: &str,
    signature_hex: &str,
    public_key_hex: &str,
) -> Result<RekorEntry, AnchorError> {
    let sig_bytes = decode_hex(signature_hex)?;
    let pk_bytes = decode_hex_array::<32>(public_key_hex)?;

    let b64 = base64::engine::general_purpose::STANDARD;
    let pem = ed25519_spki_pem(&pk_bytes);

    Ok(RekorEntry {
        api_version: HASHEDREKORD_API_VERSION.to_string(),
        kind: HASHEDREKORD_KIND.to_string(),
        spec: HashedRekordSpec {
            data: DataSpec {
                hash: HashSpec {
                    algorithm: "sha256".to_string(),
                    value: data_sha256_hex.trim().to_lowercase(),
                },
            },
            signature: SignatureSpec {
                content: b64.encode(&sig_bytes),
                public_key: PublicKeySpec {
                    content: b64.encode(pem.as_bytes()),
                },
            },
        },
    })
}

/// Serialize a proposed entry to its canonical JSON bytes (the POST
/// body). `serde_json` emits struct fields in declaration order, giving
/// byte-stable output for a given entry.
pub fn entry_to_bytes(entry: &RekorEntry) -> Result<Vec<u8>, AnchorError> {
    serde_json::to_vec(entry).map_err(|e| AnchorError::Serialization(e.to_string()))
}

// ---------------------------------------------------------------------------
// stored Rekor response (rekor-entry.json) + inclusion-proof verification
// ---------------------------------------------------------------------------

/// A Rekor inclusion proof (RFC 6962 audit path). Field names match the
/// Rekor `LogEntry.verification.inclusionProof` shape, so a real
/// response deserializes directly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InclusionProof {
    /// 0-based index of this entry's leaf within the tree.
    #[serde(rename = "logIndex")]
    pub log_index: u64,
    /// Number of leaves in the tree the proof is against.
    #[serde(rename = "treeSize")]
    pub tree_size: u64,
    /// Hex Merkle root the proof reconstructs.
    #[serde(rename = "rootHash")]
    pub root_hash: String,
    /// Sibling hashes (hex), leaf→root order (RFC 6962 audit path).
    pub hashes: Vec<String>,
    /// Signed tree head note. Opaque here; retained for round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
}

/// `verification` block of a Rekor log entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RekorVerification {
    /// base64 signed entry timestamp (SET). Retained but NOT verified
    /// here — verifying it needs Rekor's public key / trust root, which
    /// is out of scope for this offline slice.
    #[serde(
        rename = "signedEntryTimestamp",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub signed_entry_timestamp: Option<String>,
    #[serde(rename = "inclusionProof")]
    pub inclusion_proof: InclusionProof,
}

/// A stored Rekor log entry (`rekor-entry.json`). This is the single
/// entry object from a `POST /api/v1/log/entries` response (the response
/// is a `{ uuid: entry }` map; [`submit`] unwraps the one value).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RekorLogEntry {
    /// base64 of the canonicalized entry Rekor leaf-hashed.
    pub body: String,
    #[serde(rename = "logIndex")]
    pub log_index: u64,
    #[serde(rename = "integratedTime")]
    pub integrated_time: i64,
    #[serde(rename = "logID")]
    pub log_id: String,
    pub verification: RekorVerification,
}

/// Outcome of a successful offline anchor verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAnchor {
    /// The Merkle root the entry + proof reconstruct (hex).
    pub root_hash: String,
    /// Rekor's trusted timestamp (unix seconds).
    pub integrated_time: i64,
    /// Global log index of the entry.
    pub log_index: u64,
    /// The artifact hash the entry commits to (== bundle_hash).
    pub data_hash: String,
}

/// Verify a stored Rekor entry against a bundle's `bundle_hash`,
/// entirely offline. Two independent checks must pass:
///
/// 1. **Inclusion proof**: the RFC 6962 leaf hash of the entry `body`,
///    combined with the audit-path `hashes`, must reconstruct the
///    proof's `rootHash`. This proves the entry is in the log at the
///    stated position (the log operator committed to it).
/// 2. **Binding**: the `hashedrekord` inside `body` must commit to
///    exactly this bundle's `bundle_hash` — otherwise the anchor is for
///    a different artifact.
///
/// Returns the reconstructed root + trusted timestamp on success.
pub fn verify_entry(
    entry: &RekorLogEntry,
    bundle_hash: &str,
) -> Result<VerifiedAnchor, AnchorError> {
    let b64 = base64::engine::general_purpose::STANDARD;

    // 1. inclusion proof.
    let body_bytes = b64
        .decode(entry.body.as_bytes())
        .map_err(|e| AnchorError::BadBase64(e.to_string()))?;
    let leaf = rfc6962_leaf_hash(&body_bytes);

    let proof = &entry.verification.inclusion_proof;
    let mut siblings = Vec::with_capacity(proof.hashes.len());
    for h in &proof.hashes {
        siblings.push(decode_hex_array::<32>(h)?);
    }
    let computed = root_from_inclusion_proof(proof.log_index, proof.tree_size, leaf, &siblings)?;
    let computed_hex = to_hex(&computed);
    let expected = proof.root_hash.trim().to_lowercase();
    if computed_hex != expected {
        return Err(AnchorError::InclusionProofMismatch {
            computed: computed_hex,
            expected,
        });
    }

    // 2. binding: the entry commits to this bundle's hash.
    let parsed: RekorEntry = serde_json::from_slice(&body_bytes)
        .map_err(|e| AnchorError::Serialization(e.to_string()))?;
    let entry_hash = parsed.spec.data.hash.value.trim().to_lowercase();
    let bundle = bundle_hash.trim().to_lowercase();
    if entry_hash != bundle {
        return Err(AnchorError::DataHashMismatch {
            entry: entry_hash,
            bundle,
        });
    }

    Ok(VerifiedAnchor {
        root_hash: computed_hex,
        integrated_time: entry.integrated_time,
        log_index: entry.log_index,
        data_hash: entry_hash,
    })
}

/// RFC 6962 leaf hash: `SHA-256(0x00 || leaf_data)`.
pub fn rfc6962_leaf_hash(leaf_data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([0x00u8]);
    h.update(leaf_data);
    h.finalize().into()
}

/// RFC 6962 internal node hash: `SHA-256(0x01 || left || right)`.
fn rfc6962_node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([0x01u8]);
    h.update(left);
    h.update(right);
    h.finalize().into()
}

/// Reconstruct the Merkle tree root from an RFC 6962 inclusion proof,
/// following the `RootFromInclusionProof` decomposition
/// (transparency-dev/merkle). The audit path splits into `inner`
/// hashes (which pair on the left/right of the running hash according
/// to the bits of `index`) followed by `border` hashes (always on the
/// left, folding in the right-hand subtrees toward the root).
///
/// `inner = bitlen(index XOR (size-1))`. For a single-leaf tree the
/// proof is empty and the root is the leaf hash itself.
fn root_from_inclusion_proof(
    index: u64,
    size: u64,
    leaf_hash: [u8; 32],
    proof: &[[u8; 32]],
) -> Result<[u8; 32], AnchorError> {
    if index >= size {
        return Err(AnchorError::IndexOutOfRange {
            index,
            tree_size: size,
        });
    }
    let inner = inner_proof_size(index, size);
    if proof.len() < inner {
        return Err(AnchorError::ProofTooShort {
            have: proof.len(),
            need: inner,
        });
    }

    let mut res = leaf_hash;
    // `inner` hashes: pair left/right by the bits of `index`.
    for (i, sibling) in proof[..inner].iter().enumerate() {
        if (index >> i) & 1 == 0 {
            res = rfc6962_node_hash(&res, sibling);
        } else {
            res = rfc6962_node_hash(sibling, &res);
        }
    }
    // `border` hashes: always fold in on the left toward the root.
    for sibling in &proof[inner..] {
        res = rfc6962_node_hash(sibling, &res);
    }
    Ok(res)
}

/// Number of "inner" proof hashes for a leaf at `index` in a tree of
/// `size` leaves: the bit length of `index XOR (size - 1)`.
fn inner_proof_size(index: u64, size: u64) -> usize {
    let x = index ^ (size - 1);
    (u64::BITS - x.leading_zeros()) as usize
}

// ---------------------------------------------------------------------------
// live submission (network — behind the `rekor` feature)
// ---------------------------------------------------------------------------

/// Submit a proposed entry to a Rekor instance and return the created
/// log entry (with inclusion proof). Behind the `rekor` cargo feature so
/// the default build stays network-free.
///
/// `rekor_url` may be the public instance or any **private Rekor** URL
/// (air-gapped deployments). The `POST /api/v1/log/entries` response is a
/// `{ uuid: entry }` map with exactly one entry, which is unwrapped here.
#[cfg(feature = "rekor")]
pub fn submit(rekor_url: &str, entry: &RekorEntry) -> Result<RekorLogEntry, AnchorError> {
    use std::collections::BTreeMap;
    let body = entry_to_bytes(entry)?;
    let url = format!("{}/api/v1/log/entries", rekor_url.trim_end_matches('/'));
    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_bytes(&body)
        .map_err(|e| AnchorError::Network(e.to_string()))?;
    let text = resp
        .into_string()
        .map_err(|e| AnchorError::Network(format!("cannot read Rekor response: {e}")))?;
    let map: BTreeMap<String, RekorLogEntry> = serde_json::from_str(&text)
        .map_err(|e| AnchorError::Network(format!("cannot parse Rekor response: {e}")))?;
    map.into_values()
        .next()
        .ok_or_else(|| AnchorError::Network("Rekor returned an empty entry map".to_string()))
}

// ---------------------------------------------------------------------------
// small encoding helpers
// ---------------------------------------------------------------------------

/// Build a PEM `SubjectPublicKeyInfo` for an ed25519 public key.
///
/// The SPKI DER for Ed25519 is a fixed 12-byte prefix (SEQUENCE →
/// AlgorithmIdentifier{OID 1.3.101.112} → BIT STRING header) followed by
/// the 32 raw key bytes. This lets Rekor store the key in the PEM form
/// it expects without pulling in an X.509 dependency.
fn ed25519_spki_pem(pubkey: &[u8; 32]) -> String {
    // 30 2a  SEQUENCE(42)
    //   30 05  SEQUENCE(5)  06 03 2b 65 70  OID 1.3.101.112 (Ed25519)
    //   03 21 00  BIT STRING(33: 0 unused bits + 32 key bytes)
    const PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    let mut der = Vec::with_capacity(PREFIX.len() + 32);
    der.extend_from_slice(&PREFIX);
    der.extend_from_slice(pubkey);

    let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
    // 44-byte DER → 60 base64 chars, under the 64-char PEM line width.
    let mut pem = String::with_capacity(b64.len() + 64);
    pem.push_str("-----BEGIN PUBLIC KEY-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).expect("base64 is ascii"));
        pem.push('\n');
    }
    pem.push_str("-----END PUBLIC KEY-----\n");
    pem
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

/// Decode a hex string into a byte vector.
fn decode_hex(hex: &str) -> Result<Vec<u8>, AnchorError> {
    let hex = hex.trim();
    if !hex.len().is_multiple_of(2) {
        return Err(AnchorError::BadHex(format!("odd length {}", hex.len())));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().chunks(2) {
        let s = std::str::from_utf8(chunk).map_err(|_| AnchorError::BadHex("non-utf8".into()))?;
        out.push(
            u8::from_str_radix(s, 16).map_err(|_| AnchorError::BadHex("non-hex digit".into()))?,
        );
    }
    Ok(out)
}

/// Decode a fixed-length hex string into `[u8; N]`.
fn decode_hex_array<const N: usize>(hex: &str) -> Result<[u8; N], AnchorError> {
    let hex = hex.trim();
    if hex.len() != N * 2 {
        return Err(AnchorError::BadHex(format!(
            "expected {} hex chars, got {}",
            N * 2,
            hex.len()
        )));
    }
    let bytes = decode_hex(hex)?;
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
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

    /// Build a signed manifest so `hashedrekord_from_manifest` has a
    /// `ManifestSignature` to anchor.
    fn build_signed_manifest(dir: &Path, seed: &[u8; 32]) -> BundleManifest {
        let mut builder = EvidenceBundleBuilder::new(dir, "run-anchor-001", "anchor-test")
            .unwrap()
            .with_signing_key(seed);
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        let mut audit = AuditLog::new();
        audit.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        audit.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 3,
        });
        builder.finalize(&audit).unwrap()
    }

    #[test]
    fn entry_from_manifest_commits_to_bundle_hash() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_signed_manifest(dir.path(), &signing_seed(1));
        let entry = hashedrekord_from_manifest(&manifest).unwrap();

        assert_eq!(entry.api_version, HASHEDREKORD_API_VERSION);
        assert_eq!(entry.kind, HASHEDREKORD_KIND);
        assert_eq!(entry.spec.data.hash.algorithm, "sha256");
        // The committed data hash IS the bundle_hash.
        assert_eq!(entry.spec.data.hash.value, manifest.bundle_hash);
        // Signature + key are non-empty base64.
        assert!(!entry.spec.signature.content.is_empty());
        assert!(!entry.spec.signature.public_key.content.is_empty());
    }

    #[test]
    fn entry_public_key_is_pem_spki() {
        let seed = signing_seed(2);
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_signed_manifest(dir.path(), &seed);
        let entry = hashedrekord_from_manifest(&manifest).unwrap();

        let b64 = base64::engine::general_purpose::STANDARD;
        let pem = b64
            .decode(entry.spec.signature.public_key.content.as_bytes())
            .unwrap();
        let pem = String::from_utf8(pem).unwrap();
        assert!(pem.starts_with("-----BEGIN PUBLIC KEY-----"));
        assert!(pem.trim_end().ends_with("-----END PUBLIC KEY-----"));
        // 12-byte SPKI prefix + 32 key bytes = 44 bytes DER.
        let der_b64 = pem
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect::<String>();
        let der = b64.decode(der_b64.as_bytes()).unwrap();
        assert_eq!(der.len(), 44);
        assert_eq!(&der[9..12], &[0x03, 0x21, 0x00]); // BIT STRING header
    }

    #[test]
    fn unsigned_manifest_cannot_be_anchored() {
        // Bundle built WITHOUT a signing key → no ManifestSignature.
        let dir = tempfile::tempdir().unwrap();
        let mut builder =
            EvidenceBundleBuilder::new(dir.path(), "run-anchor-002", "anchor-test").unwrap();
        builder.add_workflow_def(r#"{"name":"test"}"#).unwrap();
        let manifest = builder.finalize(&AuditLog::new()).unwrap();
        assert_eq!(
            hashedrekord_from_manifest(&manifest).unwrap_err(),
            AnchorError::UnsignedManifest
        );
    }

    #[test]
    fn entry_bytes_are_deterministic() {
        let entry = hashedrekord_entry(
            "aa".repeat(32).as_str(),
            "bb".repeat(64).as_str(),
            "cc".repeat(32).as_str(),
        )
        .unwrap();
        assert_eq!(
            entry_to_bytes(&entry).unwrap(),
            entry_to_bytes(&entry).unwrap()
        );
    }

    // --- inclusion-proof math -------------------------------------------

    #[test]
    fn single_leaf_tree_root_is_leaf_hash() {
        let leaf = rfc6962_leaf_hash(b"only-leaf");
        let root = root_from_inclusion_proof(0, 1, leaf, &[]).unwrap();
        assert_eq!(root, leaf);
    }

    #[test]
    fn two_leaf_proof_verifies_both_positions() {
        let leaf0 = rfc6962_leaf_hash(b"leaf-zero");
        let leaf1 = rfc6962_leaf_hash(b"leaf-one");
        let root = rfc6962_node_hash(&leaf0, &leaf1);

        // index 0: sibling is leaf1 on the right.
        assert_eq!(
            root_from_inclusion_proof(0, 2, leaf0, &[leaf1]).unwrap(),
            root
        );
        // index 1: sibling is leaf0 on the left.
        assert_eq!(
            root_from_inclusion_proof(1, 2, leaf1, &[leaf0]).unwrap(),
            root
        );
    }

    #[test]
    fn index_out_of_range_is_rejected() {
        let leaf = rfc6962_leaf_hash(b"x");
        assert_eq!(
            root_from_inclusion_proof(2, 2, leaf, &[]).unwrap_err(),
            AnchorError::IndexOutOfRange {
                index: 2,
                tree_size: 2
            }
        );
    }

    /// Build a valid stored `RekorLogEntry` for `manifest` as leaf 0 of a
    /// 2-leaf tree, with a hand-constructed inclusion proof.
    fn mock_entry_two_leaf(manifest: &BundleManifest) -> RekorLogEntry {
        let entry = hashedrekord_from_manifest(manifest).unwrap();
        let body_bytes = entry_to_bytes(&entry).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let body = b64.encode(&body_bytes);

        let leaf0 = rfc6962_leaf_hash(&body_bytes);
        let leaf1 = rfc6962_leaf_hash(b"the-other-leaf");
        let root = rfc6962_node_hash(&leaf0, &leaf1);

        RekorLogEntry {
            body,
            log_index: 100,
            integrated_time: 1_700_000_000,
            log_id: "c0ffee".to_string(),
            verification: RekorVerification {
                signed_entry_timestamp: Some("c2lnbmF0dXJl".to_string()),
                inclusion_proof: InclusionProof {
                    log_index: 0,
                    tree_size: 2,
                    root_hash: to_hex(&root),
                    hashes: vec![to_hex(&leaf1)],
                    checkpoint: None,
                },
            },
        }
    }

    #[test]
    fn verify_entry_passes_for_valid_proof_and_binding() {
        let seed = signing_seed(7);
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_signed_manifest(dir.path(), &seed);
        let entry = mock_entry_two_leaf(&manifest);

        let ok = verify_entry(&entry, &manifest.bundle_hash).unwrap();
        assert_eq!(ok.data_hash, manifest.bundle_hash);
        assert_eq!(ok.integrated_time, 1_700_000_000);
        assert_eq!(ok.log_index, 100);
        assert_eq!(ok.root_hash, entry.verification.inclusion_proof.root_hash);

        // Round-trips through JSON like a real rekor-entry.json.
        let json = serde_json::to_string_pretty(&entry).unwrap();
        let back: RekorLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
        assert!(verify_entry(&back, &manifest.bundle_hash).is_ok());
    }

    #[test]
    fn verify_entry_fails_on_tampered_proof_hash() {
        let seed = signing_seed(7);
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_signed_manifest(dir.path(), &seed);
        let mut entry = mock_entry_two_leaf(&manifest);

        // Flip the sibling hash → recomputed root won't match rootHash.
        entry.verification.inclusion_proof.hashes[0] = "00".repeat(32);
        let err = verify_entry(&entry, &manifest.bundle_hash).unwrap_err();
        assert!(matches!(err, AnchorError::InclusionProofMismatch { .. }));
    }

    #[test]
    fn verify_entry_fails_on_tampered_root_hash() {
        let seed = signing_seed(7);
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_signed_manifest(dir.path(), &seed);
        let mut entry = mock_entry_two_leaf(&manifest);

        entry.verification.inclusion_proof.root_hash = "ab".repeat(32);
        let err = verify_entry(&entry, &manifest.bundle_hash).unwrap_err();
        assert!(matches!(err, AnchorError::InclusionProofMismatch { .. }));
    }

    #[test]
    fn verify_entry_fails_when_bundle_hash_differs() {
        // A valid proof, but the caller asks about a DIFFERENT bundle.
        let seed = signing_seed(7);
        let dir = tempfile::tempdir().unwrap();
        let manifest = build_signed_manifest(dir.path(), &seed);
        let entry = mock_entry_two_leaf(&manifest);

        let wrong = "de".repeat(32);
        let err = verify_entry(&entry, &wrong).unwrap_err();
        match err {
            AnchorError::DataHashMismatch {
                entry: e,
                bundle: b,
            } => {
                assert_eq!(e, manifest.bundle_hash);
                assert_eq!(b, wrong);
            }
            other => panic!("expected DataHashMismatch, got {other:?}"),
        }
    }
}
