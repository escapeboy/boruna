pub mod anchor;
pub mod attestation;
pub mod encryption;
pub mod evidence;
pub mod fingerprint;
pub mod log;
pub mod otel;
pub mod report;
pub mod rotate;
pub mod storage;
#[cfg(feature = "azure")]
pub mod storage_azure;
#[cfg(feature = "gcs")]
pub mod storage_gcs;
#[cfg(feature = "s3")]
pub mod storage_s3;
pub mod verify;

pub use encryption::{
    parse_kek_hex, resolve_kek, EncryptionError, EncryptionInfo, Envelope,
    ALGORITHM as ENCRYPTION_ALGORITHM, DEFAULT_KEK_ID, KEY_LEN, NONCE_LEN,
};
pub use evidence::*;
pub use fingerprint::*;
pub use log::*;
pub use report::{generate_report, ComplianceFramework, ReportFormat};
pub use verify::*;

/// Evidence bundle format version emitted by the current build.
///
/// Semver-like compatibility rule for readers:
/// - Same major (`1.x`) → forward-compatible: a 1.0 reader MUST accept a
///   1.5 bundle (unknown fields are ignored).
/// - Different major (`2.x`) → breaking: readers MUST reject.
///
/// See `docs/spec/evidence-bundle-1.0.md` for the full spec.
///
/// Bumped to `1.1` for the commitment-chain audit log that enables
/// verifiable redaction (`orchestrator/docs/verifiable-redaction.md`).
/// This is a MINOR bump: same-major, so a 1.0 reader still accepts a 1.1
/// bundle, and the current reader verifies both legacy (1.0, chain over
/// raw event JSON) and commitment-form (1.1) audit logs — the form is
/// detected per entry by the presence of `content_sha256`.
pub const BUNDLE_FORMAT_VERSION: &str = "1.1";
