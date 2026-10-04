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
//!     "bedrock":   { "kind": "bedrock",   "region": "us-east-1" },
//!     "claude":    { "kind": "command",
//!                    "command": ["claude", "-p", "--tools", "", "--model", "{model}"] }
//!   }
//! }
//! ```
//!
//! A `command` provider runs a local program for each call (no shell): the prompt goes to its
//! stdin, its stdout is the reply, and `{model}` in an argument is replaced by the model. It
//! needs no `http` feature. Whoever can edit `providers.json` can run programs as the user.
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
    /// A local program (e.g. a CLI such as `claude -p`): prompt on stdin, reply on stdout.
    Command,
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
    /// Per-request timeout in milliseconds (default 120000; 300000 for `command`).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// `command` only: the program and its arguments. `{model}` is replaced by the model.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// `command` only: largest reply accepted, in bytes (default 4 MiB).
    #[serde(default)]
    pub max_output_bytes: Option<u64>,
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
            if p.kind == ProviderKind::Command {
                match &p.command {
                    Some(argv) if argv.first().is_some_and(|prog| !prog.is_empty()) => {}
                    _ => {
                        return Err(format!(
                            "provider {name:?} (command) needs a non-empty \"command\" list"
                        ))
                    }
                }
                for (field, set) in [
                    ("api_key_env", p.api_key_env.is_some()),
                    ("base_url", p.base_url.is_some()),
                    ("region", p.region.is_some()),
                ] {
                    if set {
                        return Err(format!("provider {name:?} (command) does not take {field}"));
                    }
                }
            } else if p.command.is_some() || p.max_output_bytes.is_some() {
                return Err(format!(
                    "provider {name:?}: command and max_output_bytes are only for kind \"command\""
                ));
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
                if let Some(prog) = p.command.as_ref().and_then(|c| c.first()) {
                    s.push_str(&format!(", program={prog}"));
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
            let handler = if cfg.kind == ProviderKind::Command {
                command::handler_for(name, cfg)
            } else {
                http::handler_for(name, cfg)?
            };
            handlers.insert(name.clone(), handler);
        }
        Ok(LlmRouterHandler::new(handlers, fallback))
    }

    /// Without the `http` feature there is no network client: only `command` providers work.
    #[cfg(not(feature = "http"))]
    pub fn build_router(
        &self,
        fallback: Box<dyn CapabilityHandler>,
    ) -> Result<LlmRouterHandler, String> {
        let mut handlers: BTreeMap<String, Box<dyn CapabilityHandler>> = BTreeMap::new();
        for (name, cfg) in &self.providers {
            if cfg.kind != ProviderKind::Command {
                return Err(format!(
                    "provider {name:?} ({:?}) needs a build with the `http` feature",
                    cfg.kind
                ));
            }
            handlers.insert(name.clone(), command::handler_for(name, cfg));
        }
        Ok(LlmRouterHandler::new(handlers, fallback))
    }
}

/// `kind: "command"`: run a local program per call. No shell; prompt on stdin, reply on stdout.
mod command {
    use super::ProviderConfig;
    use crate::capability_gateway::CapabilityHandler;
    use boruna_bytecode::{Capability, Value};
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const DEFAULT_TIMEOUT_MS: u64 = 300_000;
    const DEFAULT_MAX_OUTPUT: u64 = 4 * 1024 * 1024;
    const STDERR_TAIL: usize = 500;

    pub(super) fn handler_for(name: &str, cfg: &ProviderConfig) -> Box<dyn CapabilityHandler> {
        Box::new(CommandHandler {
            name: name.to_string(),
            argv: cfg.command.clone().unwrap_or_default(),
            timeout: Duration::from_millis(cfg.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
            max_output: cfg.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT),
        })
    }

    pub(super) struct CommandHandler {
        pub(super) name: String,
        pub(super) argv: Vec<String>,
        pub(super) timeout: Duration,
        pub(super) max_output: u64,
    }

    impl CapabilityHandler for CommandHandler {
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
            self.run(&prompt, &model).map(Value::String)
        }
    }

    impl CommandHandler {
        pub(super) fn run(&self, prompt: &str, model: &str) -> Result<String, String> {
            // The model comes from `.ax` code. Substituted into a CLI's arguments, a value such
            // as `--dangerously-skip-permissions` would be read as a flag and could switch the
            // tool's own tools back on, so refuse anything flag-like or with control characters.
            if model.starts_with('-') || model.chars().any(char::is_control) {
                return Err(format!(
                    "provider {}: model {model:?} is not allowed for a command provider",
                    self.name
                ));
            }
            let argv: Vec<String> = self
                .argv
                .iter()
                .map(|a| a.replace("{model}", model))
                .collect();
            let (program, rest) = argv
                .split_first()
                .ok_or_else(|| format!("provider {}: empty command", self.name))?;
            let mut cmd = Command::new(program);
            cmd.args(rest)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            // Own process group, so a timeout can stop the program and anything it started.
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            let mut child = cmd
                .spawn()
                .map_err(|e| format!("provider {}: cannot run {program}: {e}", self.name))?;

            // Feed stdin and drain both outputs on their own threads, so neither side can
            // block the other on a full pipe.
            let mut stdin = child.stdin.take().expect("piped");
            let input = prompt.as_bytes().to_vec();
            // The CLI restores the default SIGPIPE action (so `| head` exits quietly), which
            // would kill the whole process when a program exits without reading all of its
            // stdin. Make that write fail with EPIPE instead: macOS has a per-pipe flag;
            // on Linux SIGPIPE for a pipe write goes to the writing thread, so blocking it there
            // and consuming the pending signal is enough.
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            {
                use std::os::unix::io::AsRawFd;
                // <sys/fcntl.h>: F_SETNOSIGPIPE (not exported by every libc crate version).
                const F_SETNOSIGPIPE: libc::c_int = 73;
                // SAFETY: fcntl on a pipe descriptor we own; F_SETNOSIGPIPE only sets a flag.
                unsafe {
                    libc::fcntl(stdin.as_raw_fd(), F_SETNOSIGPIPE, 1);
                }
            }
            // Not joined: a process that keeps the pipe open without reading must not hold the
            // call past its timeout. The thread ends when the pipe closes.
            std::thread::spawn(move || {
                #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
                // SAFETY: signal-mask calls for the current thread on a zeroed sigset_t.
                let set = unsafe {
                    let mut set: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut set);
                    libc::sigaddset(&mut set, libc::SIGPIPE);
                    libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                    set
                };
                // A program that exits without reading all of stdin is not an error here.
                let result = stdin.write_all(&input);
                #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
                if matches!(&result, Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe) {
                    let zero = libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    };
                    // SAFETY: takes the pending, blocked SIGPIPE off this thread.
                    unsafe {
                        libc::sigtimedwait(&set, std::ptr::null_mut(), &zero);
                    }
                }
                drop(result);
            });
            let limit = self.max_output;
            let (out_tx, out_rx) = std::sync::mpsc::channel();
            let stdout = child.stdout.take().expect("piped");
            std::thread::spawn(move || {
                let _ = out_tx.send(read_capped(stdout, limit));
            });
            let (err_tx, err_rx) = std::sync::mpsc::channel();
            let stderr = child.stderr.take().expect("piped");
            std::thread::spawn(move || {
                let _ = err_tx.send(read_capped(stderr, 64 * 1024));
            });

            // The timeout covers the whole call: the program's exit and the end of its output,
            // which a process it left running could otherwise hold open indefinitely.
            let deadline = Instant::now() + self.timeout;
            let timed_out = |child: &mut std::process::Child| {
                kill_tree(child);
                format!(
                    "provider {}: {program} did not finish within {} ms",
                    self.name,
                    self.timeout.as_millis()
                )
            };
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) if Instant::now() >= deadline => return Err(timed_out(&mut child)),
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(e) => {
                        kill_tree(&mut child);
                        return Err(format!(
                            "provider {}: waiting for {program}: {e}",
                            self.name
                        ));
                    }
                }
            };
            let remaining = |deadline: Instant| deadline.saturating_duration_since(Instant::now());
            let Ok((out, out_truncated)) = out_rx.recv_timeout(remaining(deadline)) else {
                return Err(timed_out(&mut child));
            };
            let Ok((err, _)) = err_rx.recv_timeout(remaining(deadline)) else {
                return Err(timed_out(&mut child));
            };

            if !status.success() {
                let err = String::from_utf8_lossy(&err);
                let tail: String = {
                    let t = err.trim_end();
                    let start = t
                        .char_indices()
                        .rev()
                        .nth(STDERR_TAIL - 1)
                        .map_or(0, |(i, _)| i);
                    t[start..].to_string()
                };
                let code = status
                    .code()
                    .map_or_else(|| "a signal".to_string(), |c| format!("code {c}"));
                return Err(format!(
                    "provider {}: {program} exited with {code}: {tail}",
                    self.name
                ));
            }
            if out_truncated {
                return Err(format!(
                    "provider {}: {program} wrote more than {limit} bytes",
                    self.name
                ));
            }
            Ok(String::from_utf8_lossy(&out).trim_end().to_string())
        }
    }

    /// Stop the program and, on Unix, everything in its process group.
    fn kill_tree(child: &mut std::process::Child) {
        #[cfg(unix)]
        // SAFETY: signals the process group created for this child (pgid == child pid).
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Read until EOF, keeping at most `limit` bytes; the flag says whether more arrived.
    /// Keeps draining past the limit so the child never blocks on a full pipe.
    fn read_capped(mut r: impl Read, limit: u64) -> (Vec<u8>, bool) {
        let mut kept = Vec::new();
        let mut over = false;
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = (limit as usize).saturating_sub(kept.len());
                    if n > room {
                        over = true;
                    }
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
            }
        }
        (kept, over)
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
                // Routed to `command::handler_for` by `build_router`; never reaches here.
                ProviderKind::Command => Err(format!(
                    "provider {}: command providers do not use HTTP",
                    self.name
                )),
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

#[cfg(test)]
mod command_tests {
    use super::*;
    use crate::capability_gateway::MockHandler;
    use boruna_bytecode::{Capability, Value};

    fn handler(argv: &[&str], timeout_ms: u64, max_output: u64) -> command::CommandHandler {
        command::CommandHandler {
            name: "t".into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            timeout: std::time::Duration::from_millis(timeout_ms),
            max_output,
        }
    }

    // V1-V3: what a command provider may and may not contain.
    #[test]
    fn command_config_is_validated() {
        let ok = r#"{"providers":{"c":{"kind":"command","command":["cat"],"timeout_ms":1000}}}"#;
        assert!(LlmProviders::from_json(ok).is_ok());
        for bad in [
            r#"{"providers":{"c":{"kind":"command"}}}"#,
            r#"{"providers":{"c":{"kind":"command","command":[]}}}"#,
            r#"{"providers":{"c":{"kind":"command","command":[""]}}}"#,
            r#"{"providers":{"c":{"kind":"command","command":["cat"],"api_key_env":"K"}}}"#,
            r#"{"providers":{"c":{"kind":"command","command":["cat"],"base_url":"http://x"}}}"#,
            r#"{"providers":{"c":{"kind":"command","command":["cat"],"region":"r"}}}"#,
            r#"{"providers":{"c":{"kind":"openai","api_key_env":"K","command":["cat"]}}}"#,
            r#"{"providers":{"c":{"kind":"ollama","max_output_bytes":10}}}"#,
        ] {
            assert!(LlmProviders::from_json(bad).is_err(), "accepted: {bad}");
        }
    }

    // V4: the log line names the program only, never the arguments.
    #[test]
    fn describe_names_the_program() {
        let cfg = LlmProviders::from_json(
            r#"{"providers":{"c":{"kind":"command","command":["claude","-p","--secret-flag"]}}}"#,
        )
        .unwrap();
        let d = cfg.describe();
        assert!(d.contains("program=claude"), "{d}");
        assert!(!d.contains("--secret-flag"), "{d}");
    }

    // R1: a command-only config builds a router in every build.
    #[test]
    fn command_only_config_builds_a_router() {
        let cfg =
            LlmProviders::from_json(r#"{"providers":{"c":{"kind":"command","command":["cat"]}}}"#)
                .unwrap();
        assert!(cfg.build_router(Box::new(MockHandler)).is_ok());
    }

    #[cfg(not(feature = "http"))]
    #[test]
    fn non_http_build_rejects_http_providers() {
        let cfg = LlmProviders::from_json(
            r#"{"providers":{"c":{"kind":"command","command":["cat"]},"o":{"kind":"ollama"}}}"#,
        )
        .unwrap();
        let Err(err) = cfg.build_router(Box::new(MockHandler)) else {
            panic!("a non-command provider must need the http feature");
        };
        assert!(err.contains("http"), "{err}");
    }

    #[test]
    fn missing_program_is_a_clear_error() {
        let err = handler(&["boruna-no-such-program-xyz"], 5_000, 1024)
            .run("p", "m")
            .unwrap_err();
        assert!(
            err.contains("cannot run boruna-no-such-program-xyz"),
            "{err}"
        );
    }

    // C1, C7, C8 and routing through the router by provider name.
    #[cfg(unix)]
    #[test]
    fn prompt_goes_in_on_stdin_and_reply_comes_from_stdout() {
        let cfg = LlmProviders::from_json(
            r#"{"providers":{"echo":{"kind":"command","command":["cat"]}}}"#,
        )
        .unwrap();
        let mut router = cfg.build_router(Box::new(MockHandler)).unwrap();
        let v = router
            .handle(
                &Capability::LlmCall,
                &[
                    Value::String("hello\n\n".into()),
                    Value::String("echo/any".into()),
                ],
            )
            .unwrap();
        assert_eq!(v, Value::String("hello".into())); // trailing whitespace trimmed

        let big = "x".repeat(2 * 1024 * 1024);
        let out = handler(&["cat"], 30_000, 4 * 1024 * 1024)
            .run(&big, "m")
            .unwrap();
        assert_eq!(out.len(), big.len());
    }

    // C2: `{model}` is replaced, also inside a longer argument.
    #[cfg(unix)]
    #[test]
    fn model_placeholder_is_replaced() {
        let h = handler(
            &[
                "sh",
                "-c",
                "printf '%s|%s' \"$1\" \"$2\"",
                "sh",
                "{model}",
                "--model={model}",
            ],
            5_000,
            1024,
        );
        assert_eq!(h.run("", "m1").unwrap(), "m1|--model=m1");
    }

    // A model from .ax code cannot smuggle a flag into the program's arguments.
    #[test]
    fn flag_like_models_are_refused() {
        let h = handler(&["cat", "{model}"], 5_000, 1024);
        for model in ["--dangerously-skip-permissions", "-p", "a\nb", "x\u{0}"] {
            let err = h.run("p", model).unwrap_err();
            assert!(err.contains("is not allowed"), "{model:?}: {err}");
        }
    }

    // A program that exits without reading its stdin must not take boruna down (SIGPIPE).
    #[cfg(unix)]
    #[test]
    fn program_that_ignores_stdin_does_not_kill_boruna() {
        let big = "x".repeat(4 * 1024 * 1024);
        let out = handler(&["sh", "-c", "echo early"], 10_000, 1024)
            .run(&big, "m")
            .unwrap();
        assert_eq!(out, "early");
    }

    // C3: a failing program reports its exit code and the end of stderr.
    #[cfg(unix)]
    #[test]
    fn non_zero_exit_reports_code_and_stderr() {
        let err = handler(
            &["sh", "-c", "echo 'not logged in' >&2; exit 3"],
            5_000,
            1024,
        )
        .run("p", "m")
        .unwrap_err();
        assert!(
            err.contains("code 3") && err.contains("not logged in"),
            "{err}"
        );
    }

    // C4: a program that hangs is killed at the timeout.
    #[cfg(unix)]
    #[test]
    fn hanging_program_is_killed_at_the_timeout() {
        let start = std::time::Instant::now();
        let err = handler(&["sleep", "5"], 200, 1024)
            .run("p", "m")
            .unwrap_err();
        assert!(err.contains("did not finish within 200 ms"), "{err}");
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
    }

    // The timeout covers output held open by a process the program left behind, and kills it.
    #[cfg(unix)]
    #[test]
    fn timeout_covers_leftover_processes_holding_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let script = format!("(sleep 3; touch '{}') & echo hi", marker.display());
        let start = std::time::Instant::now();
        let err = handler(&["sh", "-c", &script], 500, 1024)
            .run("p", "m")
            .unwrap_err();
        assert!(err.contains("did not finish within 500 ms"), "{err}");
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        std::thread::sleep(std::time::Duration::from_secs(4));
        assert!(!marker.exists(), "the leftover process was not killed");
    }

    // C5: a reply over the size limit is refused.
    #[cfg(unix)]
    #[test]
    fn oversized_reply_is_refused() {
        let err = handler(&["sh", "-c", "head -c 5000 /dev/zero"], 5_000, 1000)
            .run("p", "m")
            .unwrap_err();
        assert!(err.contains("more than 1000 bytes"), "{err}");
    }
}
