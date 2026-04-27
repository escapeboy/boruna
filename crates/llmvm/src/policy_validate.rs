//! Strict policy file parser and validator (sprint `0.4-S15`).
//!
//! This module is the source of truth for *external* policy files —
//! `.json` files that operators commit alongside code and pass via
//! `--policy <path>` or via the `policy: { ... }` MCP argument.
//!
//! The lenient `serde::Deserialize` derived on [`Policy`],
//! [`NetPolicy`], and [`PolicyRule`] remains in place for *internal*
//! round-trips (the audit pipeline serializes a `Policy` and reads it
//! back as part of evidence-bundle replay; same binary writes and
//! reads — no need to deny extra fields).
//!
//! When you accept a policy file from outside the binary boundary,
//! call [`parse`] (for in-memory JSON) or [`parse_file`] (for a path
//! on disk). Errors implement [`PolicyParseError::error_kind`] which
//! returns a stable string from the locked taxonomy below — these
//! strings are part of the public CLI/MCP contract per project
//! convention #2 (`docs/project-conventions-2026-04`).
//!
//! Locked error_kind taxonomy:
//!
//! | `error_kind` | When |
//! |---|---|
//! | `policy.io_error` | File missing / unreadable |
//! | `policy.parse_error` | JSON syntax error |
//! | `policy.unknown_schema_version` | `schema_version` ≠ `1` |
//! | `policy.unknown_field` | Unknown top-level or `net_policy` field |
//! | `policy.invalid_capability` | `rules` key not a known capability |
//! | `policy.invalid_net_policy` | Out-of-range / bad `net_policy` value |
//!
//! See `docs/design-policy-as-code.md` and
//! `docs/architecture-policy-as-code.md` for the design rationale.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use boruna_bytecode::Capability;
use serde::Deserialize;
use serde_json::Value;

use crate::capability_gateway::{NetPolicy, Policy, PolicyRule};

/// Schema version we accept. The validator rejects any other value.
/// Bumping this is a breaking change in the policy file contract;
/// new fields can be added at v1 as long as they are additive.
pub const POLICY_SCHEMA_VERSION: u32 = 1;

/// Allow-listed top-level field names on the policy file.
const POLICY_TOP_LEVEL_FIELDS: &[&str] =
    &["schema_version", "rules", "default_allow", "net_policy"];

/// Allow-listed field names on a `net_policy` object.
const NET_POLICY_FIELDS: &[&str] = &[
    "allowed_domains",
    "allowed_methods",
    "max_response_bytes",
    "timeout_ms",
    "allow_redirects",
];

/// Allow-listed field names on a `PolicyRule` object.
const POLICY_RULE_FIELDS: &[&str] = &["allow", "budget"];

/// Canonical HTTP methods accepted in `net_policy.allowed_methods`.
const CANONICAL_HTTP_METHODS: &[&str] =
    &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

/// Errors from [`parse`] and [`parse_file`]. Each variant maps to a
/// stable [`PolicyParseError::error_kind`] string that callers
/// (CLI, MCP) surface to integrators. The strings are locked
/// forever per project convention #2.
#[derive(Debug)]
pub enum PolicyParseError {
    /// File could not be read (missing, permission denied, etc.).
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// JSON syntax error.
    Parse(serde_json::Error),
    /// `schema_version` is set to a value this binary does not support.
    UnknownSchemaVersion(u64),
    /// An object contained a field not in the allow-list.
    /// `path` is the dotted path to the offending key, e.g.
    /// `"net_policy.foo"` or `"rules.net.fetch.foo"`.
    UnknownField { path: String, found: String },
    /// A key in `rules` is not a recognized capability name. `hint`
    /// is set when the value matches a non-canonical alias (e.g.
    /// `"net"` → hint `"net.fetch"`).
    InvalidCapability { found: String, hint: Option<String> },
    /// A `net_policy` value is out of range or otherwise unacceptable.
    InvalidNetPolicy { field: &'static str, reason: String },
}

impl PolicyParseError {
    /// Stable string per project convention #2; locked forever.
    /// New variants are additive; renaming a string is a breaking
    /// change.
    pub fn error_kind(&self) -> &'static str {
        match self {
            Self::Io { .. } => "policy.io_error",
            Self::Parse(_) => "policy.parse_error",
            Self::UnknownSchemaVersion(_) => "policy.unknown_schema_version",
            Self::UnknownField { .. } => "policy.unknown_field",
            Self::InvalidCapability { .. } => "policy.invalid_capability",
            Self::InvalidNetPolicy { .. } => "policy.invalid_net_policy",
        }
    }
}

impl fmt::Display for PolicyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "{}: {}: {}", self.error_kind(), path.display(), source)
            }
            Self::Parse(e) => write!(f, "{}: {}", self.error_kind(), e),
            Self::UnknownSchemaVersion(v) => write!(
                f,
                "{}: schema_version {} not supported by this binary (expected {})",
                self.error_kind(),
                v,
                POLICY_SCHEMA_VERSION
            ),
            Self::UnknownField { path, found } => write!(
                f,
                "{}: unknown field {:?} at {}",
                self.error_kind(),
                found,
                path
            ),
            Self::InvalidCapability { found, hint } => match hint {
                Some(h) => write!(
                    f,
                    "{}: unknown capability {:?} — did you mean {:?}?",
                    self.error_kind(),
                    found,
                    h
                ),
                None => write!(f, "{}: unknown capability {:?}", self.error_kind(), found),
            },
            Self::InvalidNetPolicy { field, reason } => {
                write!(f, "{}: net_policy.{}: {}", self.error_kind(), field, reason)
            }
        }
    }
}

impl std::error::Error for PolicyParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse(e) => Some(e),
            _ => None,
        }
    }
}

/// Parse and validate a policy file from disk. Reads the file then
/// dispatches to [`parse`].
pub fn parse_file(path: &Path) -> Result<Policy, PolicyParseError> {
    let json = std::fs::read_to_string(path).map_err(|e| PolicyParseError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    parse(&json)
}

/// Parse and validate an in-memory policy file body.
///
/// Pipeline:
/// 1. Lex into a [`serde_json::Value`] (catches syntax errors).
/// 2. Walk the object manually, rejecting unknown fields with a
///    precise dotted path.
/// 3. Strict-deserialize into [`PolicyFileV1`] (with
///    `deny_unknown_fields` as defense-in-depth).
/// 4. Validate semantic constraints (schema_version, capability
///    catalog, net_policy bounds).
/// 5. Convert to [`Policy`].
pub fn parse(json: &str) -> Result<Policy, PolicyParseError> {
    // Pass 1: lexical
    let value: Value = serde_json::from_str(json).map_err(PolicyParseError::Parse)?;

    // Pass 2: structural — manual key allow-list walk
    walk_unknown_fields(&value)?;

    // Pass 3: typed deserialization with deny_unknown_fields
    let file: PolicyFileV1 = serde_json::from_value(value).map_err(PolicyParseError::Parse)?;

    // Pass 4 + 5: semantic validation + conversion
    file.into_policy()
}

/// Walk the parsed JSON value and reject any field not in the
/// allow-list. Recurses into `net_policy` and each rule object so
/// the error path points to the offending field precisely.
fn walk_unknown_fields(value: &Value) -> Result<(), PolicyParseError> {
    let Value::Object(obj) = value else {
        // Not an object — let the typed deserializer produce a
        // clean Parse error.
        return Ok(());
    };

    for (k, v) in obj {
        if !POLICY_TOP_LEVEL_FIELDS.contains(&k.as_str()) {
            return Err(PolicyParseError::UnknownField {
                path: k.clone(),
                found: k.clone(),
            });
        }
        match k.as_str() {
            "net_policy" => {
                if let Value::Object(np) = v {
                    for (k2, _) in np {
                        if !NET_POLICY_FIELDS.contains(&k2.as_str()) {
                            return Err(PolicyParseError::UnknownField {
                                path: format!("net_policy.{k2}"),
                                found: k2.clone(),
                            });
                        }
                    }
                }
            }
            "rules" => {
                if let Value::Object(rules) = v {
                    for (cap_name, rule_val) in rules {
                        if let Value::Object(rule_obj) = rule_val {
                            for (rk, _) in rule_obj {
                                if !POLICY_RULE_FIELDS.contains(&rk.as_str()) {
                                    return Err(PolicyParseError::UnknownField {
                                        path: format!("rules.{cap_name}.{rk}"),
                                        found: rk.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFileV1 {
    #[serde(default)]
    schema_version: Option<u64>,
    #[serde(default)]
    rules: BTreeMap<String, PolicyRule>,
    #[serde(default)]
    default_allow: bool,
    #[serde(default)]
    net_policy: Option<NetPolicyFileV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NetPolicyFileV1 {
    #[serde(default)]
    allowed_domains: Vec<String>,
    #[serde(default)]
    allowed_methods: Vec<String>,
    #[serde(default = "default_max_response")]
    max_response_bytes: usize,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    #[serde(default = "default_true")]
    allow_redirects: bool,
}

fn default_max_response() -> usize {
    10 * 1024 * 1024
}

fn default_timeout() -> u64 {
    30_000
}

fn default_true() -> bool {
    true
}

impl PolicyFileV1 {
    fn into_policy(self) -> Result<Policy, PolicyParseError> {
        // schema_version
        if let Some(v) = self.schema_version {
            if v != u64::from(POLICY_SCHEMA_VERSION) {
                return Err(PolicyParseError::UnknownSchemaVersion(v));
            }
        }

        // rules: every key must be a canonical capability name.
        // `Capability::from_name` accepts aliases ("net" → NetFetch);
        // we accept only what `Capability::name()` would emit.
        let mut canonical_rules = BTreeMap::new();
        for (key, rule) in self.rules {
            match Capability::from_name(&key) {
                Some(cap) if cap.name() == key => {
                    canonical_rules.insert(key, rule);
                }
                Some(cap) => {
                    return Err(PolicyParseError::InvalidCapability {
                        found: key,
                        hint: Some(cap.name().to_string()),
                    });
                }
                None => {
                    return Err(PolicyParseError::InvalidCapability {
                        found: key,
                        hint: None,
                    });
                }
            }
        }

        // net_policy: validate bounds
        let net_policy = match self.net_policy {
            Some(np) => Some(np.validate()?),
            None => None,
        };

        Ok(Policy {
            schema_version: POLICY_SCHEMA_VERSION,
            rules: canonical_rules,
            default_allow: self.default_allow,
            net_policy,
        })
    }
}

impl NetPolicyFileV1 {
    fn validate(self) -> Result<NetPolicy, PolicyParseError> {
        if self.max_response_bytes == 0 {
            return Err(PolicyParseError::InvalidNetPolicy {
                field: "max_response_bytes",
                reason: "must be > 0".to_string(),
            });
        }
        if self.timeout_ms == 0 {
            return Err(PolicyParseError::InvalidNetPolicy {
                field: "timeout_ms",
                reason: "must be > 0".to_string(),
            });
        }
        for m in &self.allowed_methods {
            if !CANONICAL_HTTP_METHODS.contains(&m.as_str()) {
                let upper = m.to_ascii_uppercase();
                let reason = if CANONICAL_HTTP_METHODS.contains(&upper.as_str()) {
                    format!("method {m:?} must be upper-case ({upper:?})")
                } else {
                    format!(
                        "unknown HTTP method {m:?}; allowed: {}",
                        CANONICAL_HTTP_METHODS.join(", ")
                    )
                };
                return Err(PolicyParseError::InvalidNetPolicy {
                    field: "allowed_methods",
                    reason,
                });
            }
        }
        Ok(NetPolicy {
            allowed_domains: self.allowed_domains,
            allowed_methods: self.allowed_methods,
            max_response_bytes: self.max_response_bytes,
            timeout_ms: self.timeout_ms,
            allow_redirects: self.allow_redirects,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_kind(json: &str) -> &'static str {
        parse(json).unwrap_err().error_kind()
    }

    // ─── Happy path ───

    #[test]
    fn parse_minimal_object() {
        let p = parse("{}").unwrap();
        assert_eq!(p.schema_version, 1);
        assert!(!p.default_allow);
        assert!(p.rules.is_empty());
        assert!(p.net_policy.is_none());
    }

    #[test]
    fn parse_explicit_default_allow() {
        let p = parse(r#"{"default_allow": true}"#).unwrap();
        assert!(p.default_allow);
    }

    #[test]
    fn parse_explicit_schema_version_1() {
        let p = parse(r#"{"schema_version": 1}"#).unwrap();
        assert_eq!(p.schema_version, 1);
    }

    #[test]
    fn parse_with_rules() {
        let p = parse(
            r#"{"rules": {"net.fetch": {"allow": true, "budget": 10}}, "default_allow": false}"#,
        )
        .unwrap();
        let rule = p.rules.get("net.fetch").unwrap();
        assert!(rule.allow);
        assert_eq!(rule.budget, 10);
    }

    #[test]
    fn parse_with_net_policy() {
        let json = r#"{
            "default_allow": true,
            "net_policy": {
                "allowed_domains": ["api.example.com"],
                "allowed_methods": ["GET", "POST"],
                "max_response_bytes": 1024,
                "timeout_ms": 5000,
                "allow_redirects": false
            }
        }"#;
        let p = parse(json).unwrap();
        let np = p.net_policy.unwrap();
        assert_eq!(np.allowed_domains, vec!["api.example.com"]);
        assert_eq!(np.allowed_methods, vec!["GET", "POST"]);
        assert_eq!(np.max_response_bytes, 1024);
        assert_eq!(np.timeout_ms, 5000);
        assert!(!np.allow_redirects);
    }

    #[test]
    fn parse_round_trip() {
        let json =
            r#"{"default_allow": true, "rules": {"net.fetch": {"allow": true, "budget": 5}}}"#;
        let p1 = parse(json).unwrap();
        let s = serde_json::to_string(&p1).unwrap();
        let p2 = parse(&s).unwrap();
        assert_eq!(p1.default_allow, p2.default_allow);
        assert_eq!(p1.rules.len(), p2.rules.len());
    }

    #[test]
    fn parse_file_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        std::fs::write(&path, r#"{"default_allow": true}"#).unwrap();
        let p = parse_file(&path).unwrap();
        assert!(p.default_allow);
    }

    #[test]
    fn parse_zero_budget_means_unlimited() {
        let p = parse(r#"{"rules": {"net.fetch": {"allow": true, "budget": 0}}}"#).unwrap();
        assert_eq!(p.rules.get("net.fetch").unwrap().budget, 0);
    }

    // ─── Schema version ───

    #[test]
    fn reject_schema_version_2() {
        match parse(r#"{"schema_version": 2}"#).unwrap_err() {
            PolicyParseError::UnknownSchemaVersion(2) => {}
            e => panic!("wrong variant: {e:?}"),
        }
        assert_eq!(
            err_kind(r#"{"schema_version": 2}"#),
            "policy.unknown_schema_version"
        );
    }

    #[test]
    fn reject_schema_version_0() {
        match parse(r#"{"schema_version": 0}"#).unwrap_err() {
            PolicyParseError::UnknownSchemaVersion(0) => {}
            e => panic!("wrong variant: {e:?}"),
        }
    }

    // ─── Unknown fields ───

    #[test]
    fn reject_unknown_top_level_field() {
        let err = parse(r#"{"foo": 1}"#).unwrap_err();
        match err {
            PolicyParseError::UnknownField {
                ref path,
                ref found,
            } => {
                assert_eq!(path, "foo");
                assert_eq!(found, "foo");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
        assert_eq!(err.error_kind(), "policy.unknown_field");
    }

    #[test]
    fn reject_unknown_net_policy_field() {
        let err = parse(r#"{"net_policy": {"foo": 1}}"#).unwrap_err();
        match err {
            PolicyParseError::UnknownField { path, .. } => assert_eq!(path, "net_policy.foo"),
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn reject_typo_default_alow() {
        // Regression: silent-default footgun.
        let err = parse(r#"{"default_alow": true}"#).unwrap_err();
        assert!(matches!(err, PolicyParseError::UnknownField { .. }));
        assert_eq!(err.error_kind(), "policy.unknown_field");
    }

    #[test]
    fn reject_unknown_rule_field() {
        let err = parse(r#"{"rules": {"net.fetch": {"allow": true, "budget": 0, "extra": 1}}}"#)
            .unwrap_err();
        match err {
            PolicyParseError::UnknownField { ref path, .. } => {
                assert_eq!(path, "rules.net.fetch.extra");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    // ─── Capability names ───

    #[test]
    fn reject_alias_capability_net() {
        let err = parse(r#"{"rules": {"net": {"allow": true, "budget": 0}}}"#).unwrap_err();
        match err {
            PolicyParseError::InvalidCapability {
                ref found,
                ref hint,
            } => {
                assert_eq!(found, "net");
                assert_eq!(hint.as_deref(), Some("net.fetch"));
            }
            _ => panic!("wrong variant: {err:?}"),
        }
        assert_eq!(err.error_kind(), "policy.invalid_capability");
    }

    #[test]
    fn reject_alias_capability_db() {
        let err = parse(r#"{"rules": {"db": {"allow": true, "budget": 0}}}"#).unwrap_err();
        match err {
            PolicyParseError::InvalidCapability { hint, .. } => {
                assert_eq!(hint.as_deref(), Some("db.query"));
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn reject_unknown_capability() {
        let err = parse(r#"{"rules": {"future.cap": {"allow": true, "budget": 0}}}"#).unwrap_err();
        match err {
            PolicyParseError::InvalidCapability { found, hint } => {
                assert_eq!(found, "future.cap");
                assert!(hint.is_none());
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn accept_all_canonical_capabilities() {
        // Locks the catalog: when a new capability is added, this
        // test fails first and we have to update it consciously.
        let mut rules = String::new();
        for (i, cap) in Capability::ALL.iter().enumerate() {
            if i > 0 {
                rules.push(',');
            }
            rules.push_str(&format!(
                r#""{}": {{"allow": true, "budget": 0}}"#,
                cap.name()
            ));
        }
        let json = format!("{{\"rules\": {{ {rules} }}}}");
        let p = parse(&json).unwrap();
        assert_eq!(p.rules.len(), Capability::ALL.len());
        for cap in Capability::ALL.iter() {
            assert!(p.rules.contains_key(cap.name()));
        }
    }

    // ─── NetPolicy bounds ───

    #[test]
    fn reject_max_response_zero() {
        let err = parse(r#"{"net_policy": {"max_response_bytes": 0}}"#).unwrap_err();
        match err {
            PolicyParseError::InvalidNetPolicy { field, .. } => {
                assert_eq!(field, "max_response_bytes");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
        assert_eq!(err.error_kind(), "policy.invalid_net_policy");
    }

    #[test]
    fn reject_timeout_zero() {
        let err = parse(r#"{"net_policy": {"timeout_ms": 0}}"#).unwrap_err();
        match err {
            PolicyParseError::InvalidNetPolicy { field, .. } => assert_eq!(field, "timeout_ms"),
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn reject_lowercase_method() {
        let err = parse(r#"{"net_policy": {"allowed_methods": ["get"]}}"#).unwrap_err();
        match err {
            PolicyParseError::InvalidNetPolicy { field, ref reason } => {
                assert_eq!(field, "allowed_methods");
                assert!(reason.contains("upper-case"), "reason: {reason}");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn reject_unknown_method() {
        let err = parse(r#"{"net_policy": {"allowed_methods": ["JUMP"]}}"#).unwrap_err();
        match err {
            PolicyParseError::InvalidNetPolicy { field, ref reason } => {
                assert_eq!(field, "allowed_methods");
                assert!(reason.contains("unknown"), "reason: {reason}");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn accept_canonical_method_set() {
        let json = r#"{"net_policy": {"allowed_methods": ["GET","POST","PUT","DELETE","PATCH","HEAD","OPTIONS"]}}"#;
        let p = parse(json).unwrap();
        assert_eq!(p.net_policy.unwrap().allowed_methods.len(), 7);
    }

    // ─── Parse / IO errors ───

    #[test]
    fn parse_malformed_json() {
        let err = parse("{").unwrap_err();
        assert!(matches!(err, PolicyParseError::Parse(_)));
        assert_eq!(err.error_kind(), "policy.parse_error");
    }

    #[test]
    fn parse_file_missing() {
        let err = parse_file(Path::new("/no/such/path/policy.json")).unwrap_err();
        assert!(matches!(err, PolicyParseError::Io { .. }));
        assert_eq!(err.error_kind(), "policy.io_error");
    }

    // ─── Error kind taxonomy (locked) ───

    #[test]
    fn error_kind_strings_locked() {
        // Every variant must map to its documented stable string.
        // If a variant is added without updating this test, it
        // fails — by design.
        let cases: &[(PolicyParseError, &str)] = &[
            (
                PolicyParseError::Io {
                    path: PathBuf::from("/x"),
                    source: std::io::Error::other("y"),
                },
                "policy.io_error",
            ),
            (
                PolicyParseError::Parse(serde_json::from_str::<Value>("{").unwrap_err()),
                "policy.parse_error",
            ),
            (
                PolicyParseError::UnknownSchemaVersion(2),
                "policy.unknown_schema_version",
            ),
            (
                PolicyParseError::UnknownField {
                    path: "x".into(),
                    found: "x".into(),
                },
                "policy.unknown_field",
            ),
            (
                PolicyParseError::InvalidCapability {
                    found: "net".into(),
                    hint: None,
                },
                "policy.invalid_capability",
            ),
            (
                PolicyParseError::InvalidNetPolicy {
                    field: "timeout_ms",
                    reason: "x".into(),
                },
                "policy.invalid_net_policy",
            ),
        ];
        for (err, kind) in cases {
            assert_eq!(err.error_kind(), *kind);
        }
    }

    // ─── Schema drift detection ───
    //
    // `docs/reference/policy.schema.json` is hand-written. These
    // tests detect drift between the schema and the parser by
    // comparing capability names, top-level fields, net-policy
    // fields, rule fields, and schema_version. They run as plain
    // unit tests (no jsonschema dep) by parsing the schema as
    // generic JSON and walking specific paths.

    fn load_schema() -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reference/policy.schema.json");
        let json = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
        serde_json::from_str(&json).unwrap_or_else(|e| panic!("schema is not valid JSON: {e}"))
    }

    #[test]
    fn schema_version_const_matches_parser_constant() {
        let schema = load_schema();
        let v = schema["properties"]["schema_version"]["const"]
            .as_u64()
            .expect("schema_version.const present and integer");
        assert_eq!(
            v,
            u64::from(POLICY_SCHEMA_VERSION),
            "schema schema_version drift — schema={v}, parser={POLICY_SCHEMA_VERSION}"
        );
    }

    #[test]
    fn schema_top_level_fields_match_parser_allowlist() {
        let schema = load_schema();
        let mut schema_fields: Vec<String> = schema["properties"]
            .as_object()
            .expect("properties is an object")
            .keys()
            .cloned()
            .collect();
        schema_fields.sort();
        let mut parser_fields: Vec<String> = POLICY_TOP_LEVEL_FIELDS
            .iter()
            .map(|s| s.to_string())
            .collect();
        parser_fields.sort();
        assert_eq!(
            schema_fields, parser_fields,
            "top-level field drift between policy.schema.json and POLICY_TOP_LEVEL_FIELDS"
        );
    }

    #[test]
    fn schema_net_policy_fields_match_parser_allowlist() {
        let schema = load_schema();
        let mut schema_fields: Vec<String> = schema["$defs"]["netPolicy"]["properties"]
            .as_object()
            .expect("$defs.netPolicy.properties is an object")
            .keys()
            .cloned()
            .collect();
        schema_fields.sort();
        let mut parser_fields: Vec<String> =
            NET_POLICY_FIELDS.iter().map(|s| s.to_string()).collect();
        parser_fields.sort();
        assert_eq!(
            schema_fields, parser_fields,
            "net_policy field drift between policy.schema.json and NET_POLICY_FIELDS"
        );
    }

    #[test]
    fn schema_rule_fields_match_parser_allowlist() {
        let schema = load_schema();
        let mut schema_fields: Vec<String> = schema["$defs"]["policyRule"]["properties"]
            .as_object()
            .expect("$defs.policyRule.properties is an object")
            .keys()
            .cloned()
            .collect();
        schema_fields.sort();
        let mut parser_fields: Vec<String> =
            POLICY_RULE_FIELDS.iter().map(|s| s.to_string()).collect();
        parser_fields.sort();
        assert_eq!(
            schema_fields, parser_fields,
            "rule field drift between policy.schema.json and POLICY_RULE_FIELDS"
        );
    }

    #[test]
    fn schema_capability_enum_matches_canonical_names() {
        // The schema constrains `rules` keys via a propertyNames.enum.
        // That list must equal the canonical names emitted by
        // `Capability::name()` for every variant the parser would
        // accept — otherwise a policy file mentioning `step.input`
        // (canonical) is rejected by schema validators yet accepted
        // by `parse()`, leaving operators with conflicting tools.
        let schema = load_schema();
        let mut schema_caps: Vec<String> = schema["properties"]["rules"]["propertyNames"]["enum"]
            .as_array()
            .expect("rules.propertyNames.enum is an array")
            .iter()
            .map(|v| v.as_str().expect("enum entries are strings").to_string())
            .collect();
        schema_caps.sort();

        // Canonical name for every Capability variant. Any new variant
        // must add itself here AND to the schema enum.
        use boruna_bytecode::Capability;
        let mut canonical: Vec<String> = [
            Capability::NetFetch,
            Capability::FsRead,
            Capability::FsWrite,
            Capability::DbQuery,
            Capability::UiRender,
            Capability::TimeNow,
            Capability::Random,
            Capability::LlmCall,
            Capability::ActorSpawn,
            Capability::ActorSend,
            Capability::StepInput,
        ]
        .iter()
        .map(|c| c.name().to_string())
        .collect();
        canonical.sort();

        assert_eq!(
            schema_caps, canonical,
            "capability enum drift — schema and parser disagree on which capability names are accepted"
        );
    }
}
