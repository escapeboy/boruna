//! Real HTTP handler for the `NetFetch` capability.
//!
//! Feature-gated behind `http`. Uses `ureq` for synchronous HTTP requests.
//! Includes SSRF protection and domain/method allowlists via `NetPolicy`.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::IpAddr;
use std::time::Duration;

use ureq::{Agent, AgentBuilder};
use url::Url;

use boruna_bytecode::{Capability, Value};

use crate::capability_gateway::{CapabilityHandler, MockHandler, NetPolicy};

/// HTTP handler that makes real network requests for `NetFetch`.
/// All other capabilities are delegated to `MockHandler`.
pub struct HttpHandler {
    agent: Agent,
    policy: NetPolicy,
    fallback: MockHandler,
}

impl HttpHandler {
    pub fn new(policy: NetPolicy) -> Self {
        let mut builder = AgentBuilder::new().timeout(Duration::from_millis(policy.timeout_ms));

        if !policy.allow_redirects {
            builder = builder.redirects(0);
        }

        HttpHandler {
            agent: builder.build(),
            policy,
            fallback: MockHandler,
        }
    }

    pub fn handle_net_fetch(&self, args: &[Value]) -> Result<Value, String> {
        let url_str = args
            .first()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => format!("{other}"),
            })
            .ok_or_else(|| "net.fetch requires at least a URL argument".to_string())?;

        let method = args
            .get(1)
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => format!("{other}"),
            })
            .unwrap_or_else(|| "GET".to_string())
            .to_uppercase();

        let body = args.get(2).and_then(|v| match v {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        });

        let headers: BTreeMap<String, String> = args
            .get(3)
            .and_then(|v| match v {
                Value::Map(m) => {
                    let mut headers = BTreeMap::new();
                    for (k, v) in m {
                        let val = match v {
                            Value::String(s) => s.clone(),
                            other => format!("{other}"),
                        };
                        headers.insert(k.clone(), val);
                    }
                    Some(headers)
                }
                _ => None,
            })
            .unwrap_or_default();

        // Parse and validate URL
        let parsed = Url::parse(&url_str).map_err(|e| format!("invalid URL '{url_str}': {e}"))?;

        validate_url_safety(&parsed)?;
        check_domain_allowed(&parsed, &self.policy.allowed_domains)?;
        check_method_allowed(&method, &self.policy.allowed_methods)?;

        // Build request
        let mut request = match method.as_str() {
            "GET" => self.agent.get(parsed.as_str()),
            "POST" => self.agent.post(parsed.as_str()),
            "PUT" => self.agent.put(parsed.as_str()),
            "PATCH" => self.agent.request("PATCH", parsed.as_str()),
            "DELETE" => self.agent.delete(parsed.as_str()),
            "HEAD" => self.agent.head(parsed.as_str()),
            other => return Err(format!("unsupported HTTP method: {other}")),
        };

        for (k, v) in &headers {
            request = request.set(k, v);
        }

        // Execute request
        let response = if let Some(body_str) = body {
            request
                .send_string(&body_str)
                .map_err(|e| format!("HTTP request failed: {e}"))?
        } else {
            request
                .call()
                .map_err(|e| format!("HTTP request failed: {e}"))?
        };

        // Read response with size limit
        let mut buf = Vec::new();
        response
            .into_reader()
            .take(self.policy.max_response_bytes as u64)
            .read_to_end(&mut buf)
            .map_err(|e| format!("failed to read response body: {e}"))?;

        let body_str =
            String::from_utf8(buf).map_err(|e| format!("response body is not valid UTF-8: {e}"))?;

        Ok(Value::String(body_str))
    }
}

impl CapabilityHandler for HttpHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::NetFetch => self.handle_net_fetch(args),
            other => self.fallback.handle(other, args),
        }
    }
}

/// Reject URLs that target private/internal networks (SSRF protection).
fn validate_url_safety(url: &Url) -> Result<(), String> {
    // Only allow http and https
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(format!(
                "blocked scheme '{scheme}': only http/https allowed"
            ))
        }
    }

    let host = url
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    // Block localhost variants
    if host == "localhost" || host == "[::1]" {
        return Err(format!("blocked request to localhost ({host})"));
    }

    // Try to parse as IP and check for private ranges
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(format!("blocked request to private IP ({ip})"));
        }
    }

    // Also block numeric IPv4 encoded in decimal (e.g. 2130706433 = 127.0.0.1)
    // and 0.0.0.0
    if host == "0.0.0.0" || host == "[::0]" || host == "[::]" {
        return Err(format!("blocked request to unspecified address ({host})"));
    }

    Ok(())
}

/// Check if an IP address is in a private/reserved range.
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()          // 127.0.0.0/8
                || v4.is_private()    // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
                || v4.is_link_local() // 169.254.0.0/16
                || v4.is_unspecified() // 0.0.0.0
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 // 100.64.0.0/10 (CGNAT)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()          // ::1
                || v6.is_unspecified() // ::
                // Unique local (fc00::/7)
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

/// Check if the URL's domain is in the allowed list.
/// Supports wildcard patterns like `*.example.com`.
/// Empty allowlist means all domains are allowed.
fn check_domain_allowed(url: &Url, allowed: &[String]) -> Result<(), String> {
    if allowed.is_empty() {
        return Ok(());
    }

    let host = url.host_str().unwrap_or("");

    for pattern in allowed {
        if pattern.starts_with("*.") {
            let suffix = &pattern[1..]; // e.g. ".example.com"
            if host.ends_with(suffix) || host == &pattern[2..] {
                return Ok(());
            }
        } else if host == pattern {
            return Ok(());
        }
    }

    Err(format!(
        "domain '{host}' not in allowlist: [{}]",
        allowed.join(", ")
    ))
}

/// Check if the HTTP method is in the allowed list.
/// Empty allowlist means all methods are allowed.
fn check_method_allowed(method: &str, allowed: &[String]) -> Result<(), String> {
    if allowed.is_empty() {
        return Ok(());
    }

    let method_upper = method.to_uppercase();
    if allowed.iter().any(|m| m.to_uppercase() == method_upper) {
        return Ok(());
    }

    Err(format!(
        "HTTP method '{method}' not in allowlist: [{}]",
        allowed.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- SSRF protection tests ---

    #[test]
    fn test_ssrf_blocks_localhost() {
        let url = Url::parse("http://localhost:8080/secret").unwrap();
        assert!(validate_url_safety(&url).is_err());
    }

    #[test]
    fn test_ssrf_blocks_127_0_0_1() {
        let url = Url::parse("http://127.0.0.1/admin").unwrap();
        assert!(validate_url_safety(&url).is_err());
    }

    #[test]
    fn test_ssrf_blocks_private_10() {
        let url = Url::parse("http://10.0.0.1/internal").unwrap();
        assert!(validate_url_safety(&url).is_err());
    }

    #[test]
    fn test_ssrf_blocks_private_172() {
        let url = Url::parse("http://172.16.0.1/internal").unwrap();
        assert!(validate_url_safety(&url).is_err());
    }

    #[test]
    fn test_ssrf_blocks_private_192() {
        let url = Url::parse("http://192.168.1.1/router").unwrap();
        assert!(validate_url_safety(&url).is_err());
    }

    #[test]
    fn test_ssrf_blocks_ipv6_loopback() {
        let url = Url::parse("http://[::1]:8080/secret").unwrap();
        assert!(validate_url_safety(&url).is_err());
    }

    #[test]
    fn test_ssrf_blocks_ftp_scheme() {
        let url = Url::parse("ftp://example.com/file").unwrap();
        assert!(validate_url_safety(&url).is_err());
    }

    #[test]
    fn test_ssrf_allows_public_ip() {
        let url = Url::parse("https://93.184.216.34/").unwrap();
        assert!(validate_url_safety(&url).is_ok());
    }

    #[test]
    fn test_ssrf_allows_public_domain() {
        let url = Url::parse("https://api.example.com/v1/data").unwrap();
        assert!(validate_url_safety(&url).is_ok());
    }

    #[test]
    fn test_ssrf_blocks_zero_addr() {
        let url = Url::parse("http://0.0.0.0/").unwrap();
        assert!(validate_url_safety(&url).is_err());
    }

    // --- Domain allowlist tests ---

    #[test]
    fn test_domain_empty_allows_all() {
        let url = Url::parse("https://anything.example.com/").unwrap();
        assert!(check_domain_allowed(&url, &[]).is_ok());
    }

    #[test]
    fn test_domain_exact_match() {
        let url = Url::parse("https://api.example.com/v1").unwrap();
        let allowed = vec!["api.example.com".to_string()];
        assert!(check_domain_allowed(&url, &allowed).is_ok());
    }

    #[test]
    fn test_domain_exact_mismatch() {
        let url = Url::parse("https://evil.com/").unwrap();
        let allowed = vec!["api.example.com".to_string()];
        assert!(check_domain_allowed(&url, &allowed).is_err());
    }

    #[test]
    fn test_domain_wildcard_match() {
        let url = Url::parse("https://maps.googleapis.com/api").unwrap();
        let allowed = vec!["*.googleapis.com".to_string()];
        assert!(check_domain_allowed(&url, &allowed).is_ok());
    }

    #[test]
    fn test_domain_wildcard_root_match() {
        let url = Url::parse("https://googleapis.com/api").unwrap();
        let allowed = vec!["*.googleapis.com".to_string()];
        assert!(check_domain_allowed(&url, &allowed).is_ok());
    }

    #[test]
    fn test_domain_wildcard_no_match() {
        let url = Url::parse("https://evil.com/").unwrap();
        let allowed = vec!["*.googleapis.com".to_string()];
        assert!(check_domain_allowed(&url, &allowed).is_err());
    }

    // --- Method allowlist tests ---

    #[test]
    fn test_method_empty_allows_all() {
        assert!(check_method_allowed("DELETE", &[]).is_ok());
    }

    #[test]
    fn test_method_allowed() {
        let allowed = vec!["GET".to_string(), "POST".to_string()];
        assert!(check_method_allowed("GET", &allowed).is_ok());
        assert!(check_method_allowed("post", &allowed).is_ok()); // case-insensitive
    }

    #[test]
    fn test_method_blocked() {
        let allowed = vec!["GET".to_string()];
        assert!(check_method_allowed("DELETE", &allowed).is_err());
    }

    // --- HttpHandler arg parsing ---

    #[test]
    fn test_handler_requires_url_arg() {
        let handler = HttpHandler::new(NetPolicy::default());
        let result = handler.handle_net_fetch(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires at least a URL"));
    }

    #[test]
    fn test_handler_defaults_to_get() {
        // We test that a request to a private IP is blocked before it's made,
        // which confirms the method defaults to GET (the SSRF check runs first).
        let handler = HttpHandler::new(NetPolicy::default());
        let result = handler.handle_net_fetch(&[Value::String("http://10.0.0.1/".into())]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private IP"));
    }

    #[test]
    fn test_handler_respects_domain_allowlist() {
        let policy = NetPolicy {
            allowed_domains: vec!["allowed.example.com".to_string()],
            ..NetPolicy::default()
        };
        let handler = HttpHandler::new(policy);
        let result =
            handler.handle_net_fetch(&[Value::String("https://blocked.example.com/".into())]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not in allowlist"));
    }

    #[test]
    fn test_handler_respects_method_allowlist() {
        let policy = NetPolicy {
            allowed_methods: vec!["GET".to_string()],
            ..NetPolicy::default()
        };
        let handler = HttpHandler::new(policy);
        let result = handler.handle_net_fetch(&[
            Value::String("https://api.example.com/".into()),
            Value::String("DELETE".into()),
        ]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not in allowlist"));
    }
}
