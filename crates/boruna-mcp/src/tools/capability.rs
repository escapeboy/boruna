/// Return the running binary's capability surface as JSON.
///
/// Shape: MCP envelope (`success: true`) flat-merged with the
/// `CapabilitySetReport` fields (`protocol_version`, `name`, `version`,
/// `capabilities`, `capability_set_hash`). The CLI `boruna capability list
/// --json` returns the same `CapabilitySetReport` shape WITHOUT the `success`
/// field — the CLI conveys success via process exit code instead.
///
/// Documented in `docs/reference/capability-identity.md`.
pub fn list_capabilities() -> String {
    let report = boruna_bytecode::capability_set_report("boruna", env!("CARGO_PKG_VERSION"));
    let payload = serde_json::json!({
        "success": true,
        "protocol_version": report.protocol_version,
        "name": report.name,
        "version": report.version,
        "capabilities": report.capabilities,
        "capability_set_hash": report.capability_set_hash,
    });
    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_capabilities_returns_success_with_hash() {
        let json: serde_json::Value = serde_json::from_str(&list_capabilities()).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(
            json["protocol_version"],
            boruna_bytecode::CAPABILITY_REPORT_PROTOCOL_VERSION
        );
        assert_eq!(json["name"], "boruna");
        // 0.3-S14: bumped to 11 with the new step.input capability.
        assert_eq!(json["capabilities"].as_array().unwrap().len(), 11);
        assert!(json["capability_set_hash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }

    #[test]
    fn list_capabilities_matches_library_report() {
        let json: serde_json::Value = serde_json::from_str(&list_capabilities()).unwrap();
        let report = boruna_bytecode::capability_set_report("boruna", env!("CARGO_PKG_VERSION"));
        assert_eq!(
            json["capability_set_hash"].as_str().unwrap(),
            report.capability_set_hash
        );
        assert_eq!(
            json["protocol_version"].as_u64().unwrap() as u32,
            report.protocol_version
        );
    }
}
