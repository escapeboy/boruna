use serde::{Deserialize, Serialize};

/// Environment fingerprint captured at runtime (no secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvFingerprint {
    pub boruna_version: String,
    pub rust_version: String,
    pub os: String,
    pub arch: String,
    pub hostname: String,
}

impl EnvFingerprint {
    /// Capture the current environment.
    pub fn capture() -> Self {
        EnvFingerprint {
            boruna_version: env!("CARGO_PKG_VERSION").to_string(),
            rust_version: rustc_version(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            hostname: hostname(),
        }
    }
}

fn rustc_version() -> String {
    // Use the version baked at compile time
    option_env!("RUSTC_VERSION")
        .unwrap_or("unknown")
        .to_string()
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture() {
        let fp = EnvFingerprint::capture();
        assert!(!fp.boruna_version.is_empty());
        assert!(!fp.os.is_empty());
        assert!(!fp.arch.is_empty());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let fp = EnvFingerprint::capture();
        let json = serde_json::to_string(&fp).unwrap();
        let restored: EnvFingerprint = serde_json::from_str(&json).unwrap();
        assert_eq!(fp.boruna_version, restored.boruna_version);
        assert_eq!(fp.os, restored.os);
        assert_eq!(fp.arch, restored.arch);
    }
}
