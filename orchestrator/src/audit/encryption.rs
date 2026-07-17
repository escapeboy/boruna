//! Evidence bundle envelope encryption (sprint W6-B).
//!
//! Encrypts evidence bundle file contents with AES-256-GCM, using a
//! per-bundle data-encryption key (DEK) wrapped by an operator-supplied
//! key-encryption-key (KEK). The KEK is supplied out-of-band — Boruna
//! does NOT store, manage, or rotate keys. See
//! `docs/design-bundle-encryption.md` for the threat model.
//!
//! Field classification (project-conventions §15):
//! - `algorithm`, `kek_id`, `wrapped_dek`, `wrapped_dek_nonce` are
//!   REPLAY-VERIFIED (they participate in the bundle hash via
//!   `manifest.json`).
//! - `files` is OPERATIONAL (informational; does not feed the hash).
//!
//! Nonce derivation: per-file nonces are deterministic
//! `SHA-256(file_path) -> first 12 bytes`. The DEK is fresh per
//! bundle, so per-file nonces never repeat across bundles. Using
//! deterministic nonces avoids storing per-file nonces in the
//! manifest.
//!
//! AES-GCM is constant-time; tag failures surface as
//! [`EncryptionError::CipherTagInvalid`].
//!
//! Key sizes: AES-256-GCM uses 32-byte keys and 12-byte nonces.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

/// AES-256 key length in bytes.
pub const KEY_LEN: usize = 32;
/// GCM nonce length in bytes.
pub const NONCE_LEN: usize = 12;

/// Algorithm identifier embedded in the manifest.
pub const ALGORITHM: &str = "aes-256-gcm";

/// Default kek_id when the operator does not supply one.
pub const DEFAULT_KEK_ID: &str = "default";

/// Encryption metadata embedded in `manifest.json` when the bundle is
/// encrypted. Absence of this field means plaintext bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptionInfo {
    /// Cipher identifier — currently always `"aes-256-gcm"`.
    pub algorithm: String,
    /// Operator-supplied identifier for the KEK used to wrap the DEK.
    /// Boruna does not interpret it; verifiers use it to look up the
    /// right KEK in the operator's key store.
    pub kek_id: String,
    /// Base64-encoded DEK ciphertext (DEK encrypted under the KEK).
    pub wrapped_dek: String,
    /// Base64-encoded nonce used to wrap the DEK.
    pub wrapped_dek_nonce: String,
    /// Names of files in the bundle that are encrypted. OPERATIONAL —
    /// informational; not part of the bundle hash. The set of
    /// encrypted files is derivable from the file_checksums anyway.
    pub files: Vec<String>,
}

/// Encryption errors surfaced to callers.
#[derive(Debug, thiserror::Error)]
pub enum EncryptionError {
    /// Bundle is encrypted but no KEK was supplied (env or flag).
    #[error("evidence bundle is encrypted but no key-encryption-key supplied (kek_id={kek_id})")]
    EncryptionKeyRequired { kek_id: String },
    /// KEK was supplied but does not unwrap the DEK (wrong key, or
    /// the DEK ciphertext was tampered).
    #[error("key-encryption-key does not match the bundle's wrapped_dek")]
    EncryptionKeyMismatch,
    /// AES-GCM authentication tag failed — bundle has been tampered.
    #[error("AES-GCM authentication tag invalid for {file}: bundle has been tampered")]
    CipherTagInvalid { file: String },
    /// KEK hex string is the wrong length or not valid hex.
    #[error("invalid KEK: expected 64 hex chars (32 bytes), got {0}")]
    InvalidKekHex(String),
    /// Base64 decoding failed for a manifest field.
    #[error("invalid base64 in encryption metadata field {field}: {source}")]
    InvalidBase64 {
        field: String,
        #[source]
        source: base64::DecodeError,
    },
    /// Length error after decoding (e.g. DEK is not 32 bytes).
    #[error("decoded {field} has wrong length: expected {expected}, got {got}")]
    InvalidLength {
        field: String,
        expected: usize,
        got: usize,
    },
    /// Manifest declares an `algorithm` value the reader does not
    /// support. Sprint W7: the spec at `docs/spec/evidence-bundle-1.0.md`
    /// commits the 1.x reader to ONLY `aes-256-gcm`; reader rejects
    /// anything else at parse time per project §1.
    #[error("evidence.unsupported_algorithm: bundle declares algorithm={found:?}; reader supports only {expected:?}")]
    UnsupportedAlgorithm { found: String, expected: String },
}

/// Envelope holding an unwrapped DEK plus the manifest metadata. The
/// DEK lives in memory only — never written to disk.
pub struct Envelope {
    dek: [u8; KEY_LEN],
    pub info: EncryptionInfo,
}

// Manual Debug impl: never print DEK material (security-conscious;
// derive(Debug) would leak the raw key into logs).
impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("dek", &"<redacted>")
            .field("info", &self.info)
            .finish()
    }
}

// F10: wipe the DEK from memory on drop so freed pages don't retain
// key bytes. `info` holds no secret (only wrapped/ciphertext metadata)
// so it is left untouched.
impl Drop for Envelope {
    fn drop(&mut self) {
        self.dek.zeroize();
    }
}

impl Envelope {
    /// Generate a fresh DEK and wrap it with `kek`. Used at bundle
    /// build time.
    pub fn new(kek: &[u8; KEY_LEN], kek_id: &str) -> Result<Self, EncryptionError> {
        let mut dek = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut dek);

        let mut wrap_nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut wrap_nonce);

        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(kek));
        let wrapped = cipher
            .encrypt(
                Nonce::from_slice(&wrap_nonce),
                Payload {
                    msg: &dek,
                    aad: kek_id.as_bytes(),
                },
            )
            .map_err(|_| EncryptionError::CipherTagInvalid {
                file: "<dek-wrap>".to_string(),
            })?;

        Ok(Envelope {
            dek,
            info: EncryptionInfo {
                algorithm: ALGORITHM.to_string(),
                kek_id: kek_id.to_string(),
                wrapped_dek: B64.encode(&wrapped),
                wrapped_dek_nonce: B64.encode(wrap_nonce),
                files: Vec::new(),
            },
        })
    }

    /// Reconstruct a DEK from manifest metadata using the supplied
    /// KEK. Used at verify time.
    pub fn unwrap(info: &EncryptionInfo, kek: &[u8; KEY_LEN]) -> Result<Self, EncryptionError> {
        // Sprint W7 (NEW-1 from W7 security review): reject at parse —
        // the `evidence-bundle-1.0.md` spec commits the 1.x reader to
        // ONLY `aes-256-gcm`. Anything else is a tampered or future-
        // major-version manifest; refuse to decrypt rather than fall
        // through to a key-mismatch error which would mask the cause.
        if info.algorithm != ALGORITHM {
            return Err(EncryptionError::UnsupportedAlgorithm {
                found: info.algorithm.clone(),
                expected: ALGORITHM.to_string(),
            });
        }
        let wrapped =
            B64.decode(&info.wrapped_dek)
                .map_err(|e| EncryptionError::InvalidBase64 {
                    field: "wrapped_dek".into(),
                    source: e,
                })?;
        let wrap_nonce_bytes =
            B64.decode(&info.wrapped_dek_nonce)
                .map_err(|e| EncryptionError::InvalidBase64 {
                    field: "wrapped_dek_nonce".into(),
                    source: e,
                })?;
        if wrap_nonce_bytes.len() != NONCE_LEN {
            return Err(EncryptionError::InvalidLength {
                field: "wrapped_dek_nonce".into(),
                expected: NONCE_LEN,
                got: wrap_nonce_bytes.len(),
            });
        }

        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(kek));
        let mut dek_bytes = cipher
            .decrypt(
                Nonce::from_slice(&wrap_nonce_bytes),
                Payload {
                    msg: &wrapped,
                    aad: info.kek_id.as_bytes(),
                },
            )
            .map_err(|_| EncryptionError::EncryptionKeyMismatch)?;
        if dek_bytes.len() != KEY_LEN {
            let got = dek_bytes.len();
            dek_bytes.zeroize();
            return Err(EncryptionError::InvalidLength {
                field: "dek".into(),
                expected: KEY_LEN,
                got,
            });
        }

        let mut dek = [0u8; KEY_LEN];
        dek.copy_from_slice(&dek_bytes);
        // F10: wipe the decrypted DEK copy the AEAD returned; the live
        // copy now lives in `dek` (cleared on Envelope drop).
        dek_bytes.zeroize();
        Ok(Envelope {
            dek,
            info: info.clone(),
        })
    }

    /// Encrypt `plaintext` for the given filename. Returns the GCM
    /// ciphertext (with the authentication tag appended). The nonce
    /// is derived deterministically from the filename — see module
    /// docs.
    pub fn encrypt_file(&self, filename: &str, plaintext: &[u8]) -> Vec<u8> {
        let nonce = derive_nonce(filename);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.dek));
        // AES-GCM only fails on extreme allocation failures; for our
        // bounded bundle sizes this path doesn't fail in practice.
        cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .expect("AES-GCM encrypt should not fail on bounded input")
    }

    /// Re-wrap the DEK held by this envelope under a new KEK + new
    /// `kek_id` (post1-T-2.4 KEK rotation).
    ///
    /// The DEK itself is unchanged, so per-file AES-GCM tags inside
    /// the bundle remain valid — only the manifest's `wrapped_dek`,
    /// `wrapped_dek_nonce`, and `kek_id` fields change. A fresh
    /// random `wrap_nonce` is used for the new wrap.
    ///
    /// Returns a new envelope with the same DEK and a new
    /// `EncryptionInfo`. The `files` list is preserved verbatim.
    pub fn rewrap(
        &self,
        new_kek: &[u8; KEY_LEN],
        new_kek_id: &str,
    ) -> Result<Envelope, EncryptionError> {
        let mut wrap_nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut wrap_nonce);

        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(new_kek));
        let wrapped = cipher
            .encrypt(
                Nonce::from_slice(&wrap_nonce),
                Payload {
                    msg: &self.dek,
                    aad: new_kek_id.as_bytes(),
                },
            )
            .map_err(|_| EncryptionError::CipherTagInvalid {
                file: "<dek-rewrap>".to_string(),
            })?;

        Ok(Envelope {
            dek: self.dek,
            info: EncryptionInfo {
                algorithm: ALGORITHM.to_string(),
                kek_id: new_kek_id.to_string(),
                wrapped_dek: B64.encode(&wrapped),
                wrapped_dek_nonce: B64.encode(wrap_nonce),
                files: self.info.files.clone(),
            },
        })
    }

    /// Decrypt `ciphertext` for the given filename. Tag failures →
    /// `CipherTagInvalid`.
    pub fn decrypt_file(
        &self,
        filename: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, EncryptionError> {
        let nonce = derive_nonce(filename);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.dek));
        cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext)
            .map_err(|_| EncryptionError::CipherTagInvalid {
                file: filename.to_string(),
            })
    }
}

/// Deterministic per-file nonce: first 12 bytes of SHA-256(filename).
fn derive_nonce(filename: &str) -> [u8; NONCE_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(filename.as_bytes());
    let digest = hasher.finalize();
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&digest[..NONCE_LEN]);
    nonce
}

/// Parse a 64-char hex string into a 32-byte key.
pub fn parse_kek_hex(hex: &str) -> Result<[u8; KEY_LEN], EncryptionError> {
    if hex.len() != KEY_LEN * 2 {
        return Err(EncryptionError::InvalidKekHex(format!(
            "{} chars",
            hex.len()
        )));
    }
    let mut out = [0u8; KEY_LEN];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk)
            .map_err(|_| EncryptionError::InvalidKekHex("non-utf8".into()))?;
        out[i] = u8::from_str_radix(s, 16)
            .map_err(|_| EncryptionError::InvalidKekHex("non-hex digits".into()))?;
    }
    Ok(out)
}

/// Look up the operator's KEK from the `BORUNA_BUNDLE_KEK` env var
/// or a CLI-supplied hex string. Returns `Ok(None)` if no source is
/// configured (caller decides whether absence is fatal).
pub fn resolve_kek(cli_hex: Option<&str>) -> Result<Option<[u8; KEY_LEN]>, EncryptionError> {
    if let Some(hex) = cli_hex {
        return Ok(Some(parse_kek_hex(hex)?));
    }
    match std::env::var("BORUNA_BUNDLE_KEK") {
        Ok(mut hex) if !hex.is_empty() => {
            // F10: wipe the KEK hex from the env-var copy regardless of
            // parse outcome; the parsed key bytes flow to the caller.
            let parsed = parse_kek_hex(&hex);
            hex.zeroize();
            Ok(Some(parsed?))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_kek() -> [u8; KEY_LEN] {
        let mut k = [0u8; KEY_LEN];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    }

    #[test]
    fn round_trip_encrypt_decrypt() {
        let kek = fixed_kek();
        let env = Envelope::new(&kek, "default").unwrap();
        let plaintext = b"hello evidence world";
        let ct = env.encrypt_file("audit_log.json", plaintext);
        // ciphertext != plaintext
        assert_ne!(&ct, plaintext);
        // round trip with same envelope
        let pt = env.decrypt_file("audit_log.json", &ct).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn unwrap_with_correct_kek_succeeds() {
        let kek = fixed_kek();
        let env = Envelope::new(&kek, "default").unwrap();
        let plaintext = b"payload";
        let ct = env.encrypt_file("workflow.json", plaintext);

        let env2 = Envelope::unwrap(&env.info, &kek).unwrap();
        assert_eq!(env2.decrypt_file("workflow.json", &ct).unwrap(), plaintext);
    }

    #[test]
    fn unwrap_with_wrong_kek_fails() {
        let kek = fixed_kek();
        let env = Envelope::new(&kek, "default").unwrap();
        let mut bad = kek;
        bad[0] ^= 0xFF;
        let err = Envelope::unwrap(&env.info, &bad).unwrap_err();
        assert!(matches!(err, EncryptionError::EncryptionKeyMismatch));
    }

    #[test]
    fn tampered_ciphertext_fails_tag() {
        let kek = fixed_kek();
        let env = Envelope::new(&kek, "default").unwrap();
        let mut ct = env.encrypt_file("policy.json", b"data");
        let last = ct.len() - 1;
        ct[last] ^= 0x01;
        let err = env.decrypt_file("policy.json", &ct).unwrap_err();
        assert!(matches!(err, EncryptionError::CipherTagInvalid { .. }));
    }

    #[test]
    fn wrong_filename_changes_nonce() {
        let kek = fixed_kek();
        let env = Envelope::new(&kek, "default").unwrap();
        let ct = env.encrypt_file("a.json", b"x");
        // Decrypting under a different filename → tag fails.
        let err = env.decrypt_file("b.json", &ct).unwrap_err();
        assert!(matches!(err, EncryptionError::CipherTagInvalid { .. }));
    }

    #[test]
    fn parse_kek_hex_lengths() {
        assert!(parse_kek_hex("00").is_err());
        assert!(parse_kek_hex(&"0".repeat(64)).is_ok());
        assert!(parse_kek_hex(&"z".repeat(64)).is_err());
    }
}
