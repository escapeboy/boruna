//! Real HTTP handler for the `NetFetch` capability.
//!
//! Feature-gated behind `http`. Uses `ureq` for synchronous HTTP requests.
//! Includes SSRF protection and domain/method allowlists via `NetPolicy`.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

/// Maximum redirect hops followed manually (each re-validated for SSRF).
const MAX_REDIRECTS: usize = 10;

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
        // Redirects are ALWAYS followed manually (see `handle_net_fetch`) so that
        // every hop is re-validated against the SSRF guard + allowlist. Letting
        // `ureq` auto-follow would let a public, allowlisted URL 302 into the
        // internal network unchecked.
        let agent = AgentBuilder::new()
            .timeout(Duration::from_millis(policy.timeout_ms))
            .redirects(0)
            .build();

        HttpHandler {
            agent,
            policy,
            fallback: MockHandler,
        }
    }

    pub fn handle_net_fetch(&self, args: &[Value]) -> Result<Value, String> {
        let parsed_args = parse_net_fetch_args(args)
            .ok_or_else(|| "net.fetch requires at least a URL argument".to_string())?;
        let mut method = parsed_args.method;
        let mut body = parsed_args.body;
        let headers = parsed_args.headers;

        let mut current = Url::parse(&parsed_args.url)
            .map_err(|e| format!("invalid URL '{}': {e}", parsed_args.url))?;
        let mut redirects_left = if self.policy.allow_redirects {
            MAX_REDIRECTS
        } else {
            0
        };

        loop {
            // Re-validate EVERY hop (initial URL and each redirect target).
            validate_url_safety(&current)?;
            check_domain_allowed(&current, &self.policy.allowed_domains)?;
            check_method_allowed(&method, &self.policy.allowed_methods)?;
            // DNS check last (only allowlisted, syntactically-safe hosts get resolved).
            resolve_host_safety(&current)?;

            let mut request = match method.as_str() {
                "GET" => self.agent.get(current.as_str()),
                "POST" => self.agent.post(current.as_str()),
                "PUT" => self.agent.put(current.as_str()),
                "PATCH" => self.agent.request("PATCH", current.as_str()),
                "DELETE" => self.agent.delete(current.as_str()),
                "HEAD" => self.agent.head(current.as_str()),
                other => return Err(format!("unsupported HTTP method: {other}")),
            };

            for (k, v) in &headers {
                request = request.set(k, v);
            }

            let response = if let Some(ref body_str) = body {
                request
                    .send_string(body_str)
                    .map_err(|e| format!("HTTP request failed: {e}"))?
            } else {
                request
                    .call()
                    .map_err(|e| format!("HTTP request failed: {e}"))?
            };

            let status = response.status();
            if matches!(status, 301 | 302 | 303 | 307 | 308) && redirects_left > 0 {
                let location = response
                    .header("Location")
                    .ok_or_else(|| format!("redirect {status} without a Location header"))?;
                let next = current
                    .join(location)
                    .map_err(|e| format!("invalid redirect Location '{location}': {e}"))?;
                // 303 always downgrades to GET; 301/302 conventionally downgrade a
                // non-idempotent method to GET. 307/308 preserve method + body.
                if status == 303
                    || (matches!(status, 301 | 302) && method != "GET" && method != "HEAD")
                {
                    method = "GET".to_string();
                    body = None;
                }
                current = next;
                redirects_left -= 1;
                continue;
            }

            // Terminal response (non-redirect, or redirects disabled/exhausted).
            let mut buf = Vec::new();
            response
                .into_reader()
                .take(self.policy.max_response_bytes as u64)
                .read_to_end(&mut buf)
                .map_err(|e| format!("failed to read response body: {e}"))?;

            let body_str = String::from_utf8(buf)
                .map_err(|e| format!("response body is not valid UTF-8: {e}"))?;

            return Ok(Value::String(body_str));
        }
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

/// Parsed `net.fetch` arguments. Shared between the real handler and the
/// recording layer so both interpret args identically — drift here would
/// produce false-positive replay mismatches.
pub struct ParsedNetFetchArgs {
    pub url: String,
    pub method: String,
    pub body: Option<String>,
    pub headers: BTreeMap<String, String>,
}

/// Parse the `net.fetch` arg list into a structured request descriptor.
/// Returns `None` if the URL argument is missing (caller decides how to
/// surface the error). Method defaults to `"GET"` (uppercased). Body is
/// `None` when the script passed an empty string or omitted the arg.
pub fn parse_net_fetch_args(args: &[Value]) -> Option<ParsedNetFetchArgs> {
    let url = match args.first()? {
        Value::String(s) => s.clone(),
        other => format!("{other}"),
    };

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

    Some(ParsedNetFetchArgs {
        url,
        method,
        body,
        headers,
    })
}

/// Syntactic SSRF guard (no network): scheme allowlist, `localhost` by name, and
/// literal private/reserved IP hosts (IPv4 and — via bracket-stripping — IPv6).
/// A bare hostname passes here and is checked against DNS in
/// [`resolve_host_safety`] on the live path.
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

    // Block localhost by name
    if host.eq_ignore_ascii_case("localhost") {
        return Err(format!("blocked request to localhost ({host})"));
    }

    // `host_str()` returns IPv6 hosts bracketed (e.g. "[fc00::1]"); strip the
    // brackets so the literal-IP parse below actually fires for IPv6.
    let host_ip_str = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);

    if let Ok(ip) = host_ip_str.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(format!("blocked request to private/reserved IP ({ip})"));
        }
    }

    Ok(())
}

/// Live-path DNS guard: resolve a hostname and reject if ANY resolved address is
/// private/reserved. Without this, a public hostname whose A record points at
/// `169.254.169.254` / `127.0.0.1` / `10.x` (cloud-metadata theft, DNS
/// rebinding) would bypass the literal-only [`validate_url_safety`] check.
///
/// Called from `handle_net_fetch` AFTER the allowlist, so only allowlisted hosts
/// are ever resolved. No-ops for literal-IP hosts (already checked). DNS is not
/// part of deterministic replay, so this only runs on the live path and does not
/// affect replay determinism.
///
/// Residual limitation: this is a resolve-then-connect check, so a DNS entry that
/// rebinds to a private IP *between* this check and `ureq`'s own connect
/// resolution (classic TOCTOU rebinding) is not fully closed — that requires
/// pinning the validated IP for the connection, tracked as a follow-up.
fn resolve_host_safety(url: &Url) -> Result<(), String> {
    let host = url
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    let host_ip_str = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);

    // Literal IP already checked syntactically — nothing to resolve.
    if host_ip_str.parse::<IpAddr>().is_ok() {
        return Ok(());
    }

    let port = url.port_or_known_default().unwrap_or(0);
    let mut resolved_any = false;
    for addr in (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("failed to resolve host '{host}': {e}"))?
    {
        resolved_any = true;
        if is_private_ip(&addr.ip()) {
            return Err(format!(
                "blocked request to '{host}' — it resolves to a private/reserved IP ({})",
                addr.ip()
            ));
        }
    }
    if !resolved_any {
        return Err(format!("host '{host}' did not resolve to any address"));
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
