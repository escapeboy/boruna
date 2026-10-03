//! Built-in LLM providers for `llm.call` (`llm_call(prompt, "provider/model")` in `.ax`).
//!
//! The configuration ([`LlmProviders`]) is always available. The handlers that talk to the
//! providers need the `http` feature. [`LlmProviders::build_router`] turns a configuration into
//! an [`LlmRouterHandler`] that routes each call by the `provider/` prefix of its model argument
//! and passes every other capability to a fallback handler.
//!
//! `providers.json`:
//!
//! ```json
//! {
//!   "providers": {
//!     "openai":    { "kind": "openai",    "api_key_env": "OPENAI_API_KEY" },
//!     "anthropic": { "kind": "anthropic", "api_key_env": "ANTHROPIC_API_KEY" },
//!     "local":     { "kind": "ollama",    "base_url": "http://localhost:11434" },
//!     "vllm":      { "kind": "openai_compat", "base_url": "http://gpu-box:8000/v1" },
//!     "bedrock":   { "kind": "bedrock",   "region": "us-east-1" }
//!   }
//! }
//! ```
//!
//! The provider name is what `.ax` code writes before the slash: with the config above,
//! `llm_call(p, "local/llama3.1")` goes to Ollama with model `llama3.1`.
//!
//! Secrets are read from environment variables when the router is built and are never part of
//! a call's arguments or result, so they never reach the event log or evidence. Bedrock uses
//! `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and optionally `AWS_SESSION_TOKEN`.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::capability_gateway::{CapabilityHandler, LlmRouterHandler};

/// Which API a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// api.openai.com `chat/completions`.
    Openai,
    /// Any OpenAI-compatible endpoint (vLLM, OpenRouter, Together, Groq, LiteLLM, ...).
    OpenaiCompat,
    /// api.anthropic.com `v1/messages`.
    Anthropic,
    /// Ollama `api/chat`.
    Ollama,
    /// AWS Bedrock Converse API, signed with SigV4.
    Bedrock,
}

/// One provider's settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    /// Endpoint base. Defaults: OpenAI `https://api.openai.com/v1`, Anthropic
    /// `https://api.anthropic.com`, Ollama `http://localhost:11434`, Bedrock
    /// `https://bedrock-runtime.<region>.amazonaws.com`. Required for `openai_compat`.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Name of the environment variable holding the API key (not the key itself).
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// AWS region for Bedrock (falls back to `AWS_REGION`).
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Per-request timeout in milliseconds (default 120000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// The `providers` map of a `providers.json`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmProviders {
    pub providers: BTreeMap<String, ProviderConfig>,
}

impl LlmProviders {
    pub fn from_json(json: &str) -> Result<Self, String> {
        let cfg: LlmProviders =
            serde_json::from_str(json).map_err(|e| format!("invalid providers config: {e}"))?;
        if cfg.providers.is_empty() {
            return Err("providers config has no providers".into());
        }
        for (name, p) in &cfg.providers {
            if name.is_empty() || name.contains('/') {
                return Err(format!(
                    "provider name {name:?} must be non-empty and contain no '/'"
                ));
            }
            if p.kind == ProviderKind::OpenaiCompat && p.base_url.is_none() {
                return Err(format!("provider {name:?} (openai_compat) needs base_url"));
            }
        }
        Ok(cfg)
    }

    /// One line per provider, safe to log: names environment variables, never their values.
    pub fn describe(&self) -> String {
        self.providers
            .iter()
            .map(|(name, p)| {
                let mut s = format!("{name} -> {:?}", p.kind);
                if let Some(u) = &p.base_url {
                    s.push_str(&format!(", base_url={u}"));
                }
                if let Some(k) = &p.api_key_env {
                    s.push_str(&format!(", key_env={k}"));
                }
                if let Some(r) = &p.region {
                    s.push_str(&format!(", region={r}"));
                }
                s
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Build a router that sends `llm.call` to the configured providers and every other
    /// capability to `fallback`. Reads API keys from the environment now, so a missing key is
    /// reported before the program runs.
    #[cfg(feature = "http")]
    pub fn build_router(
        &self,
        fallback: Box<dyn CapabilityHandler>,
    ) -> Result<LlmRouterHandler, String> {
        let mut handlers: BTreeMap<String, Box<dyn CapabilityHandler>> = BTreeMap::new();
        for (name, cfg) in &self.providers {
            handlers.insert(name.clone(), http::handler_for(name, cfg)?);
        }
        Ok(LlmRouterHandler::new(handlers, fallback))
    }

    /// Without the `http` feature there is no network client: building a router fails.
    #[cfg(not(feature = "http"))]
    pub fn build_router(
        &self,
        _fallback: Box<dyn CapabilityHandler>,
    ) -> Result<LlmRouterHandler, String> {
        Err("LLM providers need a build with the `http` feature".into())
    }
}

#[cfg(feature = "http")]
mod http {
    use super::{ProviderConfig, ProviderKind};
    use crate::capability_gateway::CapabilityHandler;
    use boruna_bytecode::{Capability, Value};
    use serde_json::{json, Value as Json};
    use std::time::Duration;

    pub(super) fn handler_for(
        name: &str,
        cfg: &ProviderConfig,
    ) -> Result<Box<dyn CapabilityHandler>, String> {
        let key = match &cfg.api_key_env {
            Some(var) => Some(std::env::var(var).map_err(|_| {
                format!("provider {name:?}: environment variable {var} is not set")
            })?),
            None => None,
        };
        let needs_key = matches!(cfg.kind, ProviderKind::Openai | ProviderKind::Anthropic);
        if needs_key && key.is_none() {
            return Err(format!(
                "provider {name:?}: {:?} needs api_key_env",
                cfg.kind
            ));
        }
        let aws = if cfg.kind == ProviderKind::Bedrock {
            let region = cfg
                .region
                .clone()
                .or_else(|| std::env::var("AWS_REGION").ok())
                .ok_or_else(|| format!("provider {name:?}: bedrock needs region or AWS_REGION"))?;
            let access = std::env::var("AWS_ACCESS_KEY_ID")
                .map_err(|_| format!("provider {name:?}: AWS_ACCESS_KEY_ID is not set"))?;
            let secret = std::env::var("AWS_SECRET_ACCESS_KEY")
                .map_err(|_| format!("provider {name:?}: AWS_SECRET_ACCESS_KEY is not set"))?;
            let token = std::env::var("AWS_SESSION_TOKEN").ok();
            Some(super::sigv4::Credentials {
                access_key_id: access,
                secret_access_key: secret,
                session_token: token,
                region,
            })
        } else {
            None
        };
        Ok(Box::new(ProviderHandler {
            name: name.to_string(),
            cfg: cfg.clone(),
            key,
            aws,
        }))
    }

    struct ProviderHandler {
        name: String,
        cfg: ProviderConfig,
        key: Option<String>,
        aws: Option<super::sigv4::Credentials>,
    }

    impl CapabilityHandler for ProviderHandler {
        fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
            if !matches!(cap, Capability::LlmCall) {
                return Err(format!("provider {}: only handles llm.call", self.name));
            }
            let prompt = match args.first() {
                Some(Value::String(s)) => s.clone(),
                Some(other) => format!("{other}"),
                None => return Err("llm.call needs a prompt (args[0])".into()),
            };
            let model = match args.get(1) {
                Some(Value::String(s)) => s
                    .split_once('/')
                    .map(|(_, m)| m.to_string())
                    .unwrap_or_else(|| s.clone()),
                _ => return Err("llm.call needs a model (args[1])".into()),
            };
            if model.is_empty() {
                return Err(format!("provider {}: empty model name", self.name));
            }
            self.complete(&prompt, &model).map(Value::String)
        }
    }

    impl ProviderHandler {
        fn complete(&self, prompt: &str, model: &str) -> Result<String, String> {
            let c = &self.cfg;
            let msgs = json!([{ "role": "user", "content": prompt }]);
            match c.kind {
                ProviderKind::Openai | ProviderKind::OpenaiCompat => {
                    let base = c
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.openai.com/v1".into());
                    let mut body = json!({ "model": model, "messages": msgs });
                    if let Some(t) = c.temperature {
                        body["temperature"] = json!(t);
                    }
                    if let Some(m) = c.max_tokens {
                        body["max_tokens"] = json!(m);
                    }
                    let mut headers = vec![];
                    if let Some(k) = &self.key {
                        headers.push(("authorization".to_string(), format!("Bearer {k}")));
                    }
                    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
                    let resp = self.post_json(&url, &headers, &body)?;
                    string_at(&resp, &["choices", "0", "message", "content"])
                        .ok_or_else(|| self.bad_shape(&resp))
                }
                ProviderKind::Anthropic => {
                    let base = c
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.anthropic.com".into());
                    let mut body = json!({
                        "model": model,
                        "max_tokens": c.max_tokens.unwrap_or(1024),
                        "messages": msgs,
                    });
                    if let Some(t) = c.temperature {
                        body["temperature"] = json!(t);
                    }
                    let headers = vec![
                        (
                            "x-api-key".to_string(),
                            self.key.clone().unwrap_or_default(),
                        ),
                        ("anthropic-version".to_string(), "2023-06-01".to_string()),
                    ];
                    let url = format!("{}/v1/messages", base.trim_end_matches('/'));
                    let resp = self.post_json(&url, &headers, &body)?;
                    let text: String = resp["content"]
                        .as_array()
                        .map(|blocks| {
                            blocks
                                .iter()
                                .filter(|b| b["type"] == "text")
                                .filter_map(|b| b["text"].as_str())
                                .collect()
                        })
                        .unwrap_or_default();
                    if text.is_empty() {
                        Err(self.bad_shape(&resp))
                    } else {
                        Ok(text)
                    }
                }
                ProviderKind::Ollama => {
                    let base = c
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "http://localhost:11434".into());
                    let mut body = json!({ "model": model, "messages": msgs, "stream": false });
                    if let Some(t) = c.temperature {
                        body["options"] = json!({ "temperature": t });
                    }
                    let url = format!("{}/api/chat", base.trim_end_matches('/'));
                    let resp = self.post_json(&url, &[], &body)?;
                    string_at(&resp, &["message", "content"]).ok_or_else(|| self.bad_shape(&resp))
                }
                ProviderKind::Bedrock => {
                    let creds = self.aws.as_ref().expect("checked when built");
                    let base = c.base_url.clone().unwrap_or_else(|| {
                        format!("https://bedrock-runtime.{}.amazonaws.com", creds.region)
                    });
                    let mut inference = json!({ "maxTokens": c.max_tokens.unwrap_or(1024) });
                    if let Some(t) = c.temperature {
                        inference["temperature"] = json!(t);
                    }
                    let body = json!({
                        "messages": [{ "role": "user", "content": [{ "text": prompt }] }],
                        "inferenceConfig": inference,
                    });
                    let path = format!("/model/{}/converse", super::sigv4::uri_encode(model));
                    let url = format!("{}{path}", base.trim_end_matches('/'));
                    let host = url
                        .split("://")
                        .nth(1)
                        .and_then(|r| r.split('/').next())
                        .unwrap_or("")
                        .to_string();
                    let payload = body.to_string();
                    let signed = super::sigv4::sign(
                        creds,
                        "bedrock",
                        "POST",
                        &host,
                        &path,
                        &[("content-type".into(), "application/json".into())],
                        payload.as_bytes(),
                        &super::sigv4::amz_date_now(),
                    );
                    let resp = self.post_raw(&url, &signed, &payload)?;
                    string_at(&resp, &["output", "message", "content", "0", "text"])
                        .ok_or_else(|| self.bad_shape(&resp))
                }
            }
        }

        fn post_json(
            &self,
            url: &str,
            headers: &[(String, String)],
            body: &Json,
        ) -> Result<Json, String> {
            let mut all = vec![("content-type".to_string(), "application/json".to_string())];
            all.extend_from_slice(headers);
            self.post_raw(url, &all, &body.to_string())
        }

        fn post_raw(
            &self,
            url: &str,
            headers: &[(String, String)],
            body: &str,
        ) -> Result<Json, String> {
            let timeout = Duration::from_millis(self.cfg.timeout_ms.unwrap_or(120_000));
            let mut req = ureq::post(url).timeout(timeout);
            for (k, v) in headers {
                req = req.set(k, v);
            }
            let text = match req.send_string(body) {
                Ok(r) => r
                    .into_string()
                    .map_err(|e| format!("provider {}: reading response: {e}", self.name))?,
                Err(ureq::Error::Status(code, r)) => {
                    let detail = r.into_string().unwrap_or_default();
                    let detail: String = detail.chars().take(300).collect();
                    return Err(format!("provider {}: HTTP {code}: {detail}", self.name));
                }
                Err(e) => return Err(format!("provider {}: request failed: {e}", self.name)),
            };
            serde_json::from_str(&text)
                .map_err(|e| format!("provider {}: response is not JSON: {e}", self.name))
        }

        fn bad_shape(&self, resp: &Json) -> String {
            let s: String = resp.to_string().chars().take(300).collect();
            format!("provider {}: unexpected response shape: {s}", self.name)
        }
    }

    /// Follow a path of object keys / array indexes and return the string there.
    fn string_at(v: &Json, path: &[&str]) -> Option<String> {
        let mut cur = v;
        for p in path {
            cur = match p.parse::<usize>() {
                Ok(i) => cur.get(i)?,
                Err(_) => cur.get(*p)?,
            };
        }
        cur.as_str().map(str::to_string)
    }
}

/// AWS Signature Version 4 (header signing) for Bedrock. Pure functions; tested against the
/// official aws-signing-test-suite vectors.
#[cfg(feature = "http")]
pub(crate) mod sigv4 {
    use sha2::{Digest, Sha256};

    pub struct Credentials {
        pub access_key_id: String,
        pub secret_access_key: String,
        pub session_token: Option<String>,
        pub region: String,
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn sha256_hex(data: &[u8]) -> String {
        hex(&Sha256::digest(data))
    }

    fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
        const BLOCK: usize = 64;
        let mut k = if key.len() > BLOCK {
            Sha256::digest(key).to_vec()
        } else {
            key.to_vec()
        };
        k.resize(BLOCK, 0);
        let mut inner = Sha256::new();
        inner.update(k.iter().map(|b| b ^ 0x36).collect::<Vec<_>>());
        inner.update(msg);
        let mut outer = Sha256::new();
        outer.update(k.iter().map(|b| b ^ 0x5c).collect::<Vec<_>>());
        outer.update(inner.finalize());
        outer.finalize().to_vec()
    }

    /// RFC 3986 percent-encoding of everything except unreserved characters (and `/`, which
    /// separates path segments).
    pub fn uri_encode(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    }

    /// `YYYYMMDDTHHMMSSZ` for the current UTC time.
    pub fn amz_date_now() -> String {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        amz_date(secs)
    }

    pub fn amz_date(unix_secs: u64) -> String {
        let days = (unix_secs / 86_400) as i64;
        let rem = unix_secs % 86_400;
        // Civil-from-days (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!(
            "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
            rem / 3600,
            (rem / 60) % 60,
            rem % 60
        )
    }

    /// Sign a request. `path` is the request path as sent (already percent-encoded per
    /// segment); like other non-S3 services the canonical URI encodes it once more.
    /// `extra_headers` are lower-case names. Returns every header to send, including
    /// `host`, `x-amz-date`, `x-amz-content-sha256`, the session token and `authorization`.
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        creds: &Credentials,
        service: &str,
        method: &str,
        host: &str,
        path: &str,
        extra_headers: &[(String, String)],
        payload: &[u8],
        amz_date: &str,
    ) -> Vec<(String, String)> {
        sign_with(
            creds,
            service,
            method,
            host,
            path,
            extra_headers,
            payload,
            amz_date,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sign_with(
        creds: &Credentials,
        service: &str,
        method: &str,
        host: &str,
        path: &str,
        extra_headers: &[(String, String)],
        payload: &[u8],
        amz_date: &str,
        content_sha256_header: bool,
    ) -> Vec<(String, String)> {
        let payload_hash = sha256_hex(payload);
        let mut headers: Vec<(String, String)> = extra_headers.to_vec();
        headers.push(("host".into(), host.into()));
        headers.push(("x-amz-date".into(), amz_date.into()));
        if content_sha256_header {
            headers.push(("x-amz-content-sha256".into(), payload_hash.clone()));
        }
        if let Some(t) = &creds.session_token {
            headers.push(("x-amz-security-token".into(), t.clone()));
        }
        headers.sort_by(|a, b| a.0.cmp(&b.0));
        let canonical_headers: String = headers
            .iter()
            .map(|(k, v)| format!("{k}:{}\n", v.trim()))
            .collect();
        let signed_headers = headers
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_uri = uri_encode(path);
        let canonical_request = format!(
            "{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );
        let date = &amz_date[..8];
        let scope = format!("{date}/{}/{service}/aws4_request", creds.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            sha256_hex(canonical_request.as_bytes())
        );
        let k_date = hmac(
            format!("AWS4{}", creds.secret_access_key).as_bytes(),
            date.as_bytes(),
        );
        let k_region = hmac(&k_date, creds.region.as_bytes());
        let k_service = hmac(&k_region, service.as_bytes());
        let k_signing = hmac(&k_service, b"aws4_request");
        let signature = hex(&hmac(&k_signing, string_to_sign.as_bytes()));
        headers.push((
            "authorization".into(),
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                creds.access_key_id
            ),
        ));
        headers
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn example_creds() -> Credentials {
            Credentials {
                access_key_id: "AKIDEXAMPLE".into(),
                secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
                session_token: None,
                region: "us-east-1".into(),
            }
        }

        fn signature(headers: &[(String, String)]) -> String {
            let auth = &headers
                .iter()
                .find(|(k, _)| k == "authorization")
                .unwrap()
                .1;
            auth.rsplit("Signature=").next().unwrap().to_string()
        }

        /// aws-signing-test-suite v4/post-vanilla (unsigned body, no content-sha256 header).
        #[test]
        fn post_vanilla_vector() {
            let h = sign_with(
                &example_creds(),
                "service",
                "POST",
                "example.amazonaws.com",
                "/",
                &[],
                b"",
                "20150830T123600Z",
                false,
            );
            assert_eq!(
                signature(&h),
                "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
            );
        }

        /// aws-signing-test-suite v4/post-x-www-form-urlencoded (signed body).
        #[test]
        fn post_form_urlencoded_vector() {
            let h = sign_with(
                &example_creds(),
                "service",
                "POST",
                "example.amazonaws.com",
                "/",
                &[
                    ("content-length".into(), "13".into()),
                    (
                        "content-type".into(),
                        "application/x-www-form-urlencoded".into(),
                    ),
                ],
                b"Param1=value1",
                "20150830T123600Z",
                true,
            );
            assert_eq!(
                signature(&h),
                "d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b"
            );
        }

        #[test]
        fn amz_date_formats_utc() {
            // 2015-08-30T12:36:00Z
            assert_eq!(amz_date(1_440_938_160), "20150830T123600Z");
            assert_eq!(amz_date(0), "19700101T000000Z");
        }

        #[test]
        fn model_ids_with_colons_are_encoded_twice_in_the_canonical_uri() {
            assert_eq!(
                uri_encode("anthropic.claude-v1:0"),
                "anthropic.claude-v1%3A0"
            );
            assert_eq!(
                uri_encode("/model/a%3A0/converse"),
                "/model/a%253A0/converse"
            );
        }
    }
}

#[cfg(all(test, feature = "http"))]
mod tests {
    use super::*;
    use crate::capability_gateway::MockHandler;
    use boruna_bytecode::{Capability, Value};
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    struct Captured {
        request_line: String,
        headers: BTreeMap<String, String>,
        body: String,
    }

    /// Serve one HTTP request on localhost with `status` and `json`, and return what was sent.
    fn serve_once(status: u16, json: &'static str) -> (String, JoinHandle<Captured>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut headers = BTreeMap::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
            let len: usize = headers
                .get("content-length")
                .map(|v| v.parse().unwrap())
                .unwrap_or(0);
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                json.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            Captured {
                request_line: request_line.trim_end().to_string(),
                headers,
                body: String::from_utf8(body).unwrap(),
            }
        });
        (base, handle)
    }

    fn call(router: &mut LlmRouterHandler, model: &str) -> Result<Value, String> {
        router.handle(
            &Capability::LlmCall,
            &[
                Value::String("hi there".into()),
                Value::String(model.into()),
            ],
        )
    }

    fn router(json: String) -> LlmRouterHandler {
        LlmProviders::from_json(&json)
            .unwrap()
            .build_router(Box::new(MockHandler))
            .unwrap()
    }

    #[test]
    fn openai_request_and_reply() {
        std::env::set_var("BORUNA_TEST_OPENAI_KEY", "sk-test-1");
        let (base, srv) = serve_once(
            200,
            r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#,
        );
        let mut r = router(format!(
            r#"{{"providers":{{"openai":{{"kind":"openai","api_key_env":"BORUNA_TEST_OPENAI_KEY","base_url":"{base}","temperature":0}}}}}}"#
        ));
        assert_eq!(
            call(&mut r, "openai/gpt-x").unwrap(),
            Value::String("hello".into())
        );
        let req = srv.join().unwrap();
        assert_eq!(req.request_line, "POST /chat/completions HTTP/1.1");
        assert_eq!(req.headers["authorization"], "Bearer sk-test-1");
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["messages"][0]["content"], "hi there");
        assert_eq!(body["temperature"], 0.0);
    }

    #[test]
    fn openai_compat_without_a_key_sends_no_authorization() {
        let (base, srv) = serve_once(200, r#"{"choices":[{"message":{"content":"ok"}}]}"#);
        let mut r = router(format!(
            r#"{{"providers":{{"vllm":{{"kind":"openai_compat","base_url":"{base}/v1"}}}}}}"#
        ));
        assert_eq!(
            call(&mut r, "vllm/qwen").unwrap(),
            Value::String("ok".into())
        );
        let req = srv.join().unwrap();
        assert_eq!(req.request_line, "POST /v1/chat/completions HTTP/1.1");
        assert!(!req.headers.contains_key("authorization"));
    }

    #[test]
    fn anthropic_request_and_reply() {
        std::env::set_var("BORUNA_TEST_ANTHROPIC_KEY", "ak-test-2");
        let (base, srv) = serve_once(
            200,
            r#"{"content":[{"type":"text","text":"a"},{"type":"tool_use"},{"type":"text","text":"b"}]}"#,
        );
        let mut r = router(format!(
            r#"{{"providers":{{"anthropic":{{"kind":"anthropic","api_key_env":"BORUNA_TEST_ANTHROPIC_KEY","base_url":"{base}"}}}}}}"#
        ));
        assert_eq!(
            call(&mut r, "anthropic/claude-x").unwrap(),
            Value::String("ab".into())
        );
        let req = srv.join().unwrap();
        assert_eq!(req.request_line, "POST /v1/messages HTTP/1.1");
        assert_eq!(req.headers["x-api-key"], "ak-test-2");
        assert_eq!(req.headers["anthropic-version"], "2023-06-01");
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["model"], "claude-x");
        assert_eq!(body["max_tokens"], 1024);
    }

    #[test]
    fn ollama_request_and_reply() {
        let (base, srv) = serve_once(200, r#"{"message":{"role":"assistant","content":"local"}}"#);
        let mut r = router(format!(
            r#"{{"providers":{{"local":{{"kind":"ollama","base_url":"{base}"}}}}}}"#
        ));
        assert_eq!(
            call(&mut r, "local/llama3.1").unwrap(),
            Value::String("local".into())
        );
        let req = srv.join().unwrap();
        assert_eq!(req.request_line, "POST /api/chat HTTP/1.1");
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["model"], "llama3.1");
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn bedrock_request_is_signed_and_reply_parsed() {
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIDTEST");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "secret-test");
        std::env::remove_var("AWS_SESSION_TOKEN");
        let (base, srv) = serve_once(
            200,
            r#"{"output":{"message":{"role":"assistant","content":[{"text":"from bedrock"}]}}}"#,
        );
        let mut r = router(format!(
            r#"{{"providers":{{"bedrock":{{"kind":"bedrock","region":"eu-west-1","base_url":"{base}"}}}}}}"#
        ));
        assert_eq!(
            call(&mut r, "bedrock/anthropic.claude-v1:0").unwrap(),
            Value::String("from bedrock".into())
        );
        let req = srv.join().unwrap();
        assert_eq!(
            req.request_line,
            "POST /model/anthropic.claude-v1%3A0/converse HTTP/1.1"
        );
        let auth = &req.headers["authorization"];
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDTEST/")
                && auth.contains("/eu-west-1/bedrock/aws4_request"),
            "{auth}"
        );
        use sha2::{Digest, Sha256};
        let hash: String = Sha256::digest(req.body.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(req.headers["x-amz-content-sha256"], hash);
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["text"], "hi there");
    }

    #[test]
    fn http_errors_are_reported_without_the_key() {
        std::env::set_var("BORUNA_TEST_ERR_KEY", "sk-must-not-leak");
        let (base, srv) = serve_once(500, r#"{"error":"boom"}"#);
        let mut r = router(format!(
            r#"{{"providers":{{"openai":{{"kind":"openai","api_key_env":"BORUNA_TEST_ERR_KEY","base_url":"{base}"}}}}}}"#
        ));
        let err = call(&mut r, "openai/gpt-x").unwrap_err();
        srv.join().unwrap();
        assert!(err.contains("HTTP 500") && err.contains("boom"), "{err}");
        assert!(!err.contains("sk-must-not-leak"), "{err}");
    }

    #[test]
    fn missing_key_variable_fails_at_build_time_and_names_only_the_variable() {
        std::env::remove_var("BORUNA_TEST_UNSET_KEY");
        let cfg = LlmProviders::from_json(
            r#"{"providers":{"openai":{"kind":"openai","api_key_env":"BORUNA_TEST_UNSET_KEY"}}}"#,
        )
        .unwrap();
        let err = match cfg.build_router(Box::new(MockHandler)) {
            Err(e) => e,
            Ok(_) => panic!("expected an error"),
        };
        assert!(err.contains("BORUNA_TEST_UNSET_KEY"), "{err}");
    }

    #[test]
    fn config_validation() {
        assert!(LlmProviders::from_json(r#"{"providers":{}}"#).is_err());
        assert!(LlmProviders::from_json(r#"{"providers":{"a/b":{"kind":"ollama"}}}"#).is_err());
        assert!(
            LlmProviders::from_json(r#"{"providers":{"x":{"kind":"openai_compat"}}}"#).is_err()
        );
        assert!(
            LlmProviders::from_json(r#"{"providers":{"x":{"kind":"ollama","api_key":"literal"}}}"#)
                .is_err(),
            "unknown fields such as a literal key are rejected"
        );
        let ok = LlmProviders::from_json(
            r#"{"providers":{"x":{"kind":"anthropic","api_key_env":"K"}}}"#,
        )
        .unwrap();
        assert!(ok.describe().contains("key_env=K"));
    }

    #[test]
    fn other_capabilities_go_to_the_fallback() {
        let mut r = router(
            r#"{"providers":{"local":{"kind":"ollama","base_url":"http://127.0.0.1:9"}}}"#.into(),
        );
        let v = r
            .handle(&Capability::NetFetch, &[Value::String("u".into())])
            .unwrap();
        assert!(matches!(v, Value::String(s) if s.contains("\"mock\": true")));
    }
}
