pub mod encryption;
pub mod evidence;
pub mod fingerprint;
pub mod log;
pub mod verify;

pub use encryption::{
    parse_kek_hex, resolve_kek, EncryptionError, EncryptionInfo, Envelope,
    ALGORITHM as ENCRYPTION_ALGORITHM, DEFAULT_KEK_ID, KEY_LEN, NONCE_LEN,
};
pub use evidence::*;
pub use fingerprint::*;
pub use log::*;
pub use verify::*;

/// Evidence bundle format version emitted by the current build.
///
/// Semver-like compatibility rule for readers:
/// - Same major (`1.x`) → forward-compatible: a 1.0 reader MUST accept a
///   1.5 bundle (unknown fields are ignored).
/// - Different major (`2.x`) → breaking: readers MUST reject.
///
/// See `docs/spec/evidence-bundle-1.0.md` for the full spec.
pub const BUNDLE_FORMAT_VERSION: &str = "1.0";
