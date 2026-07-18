//! OpenTelemetry (OTLP/JSON) export of an evidence bundle's execution.
//!
//! Turns a sealed run into a set of OpenTelemetry spans encoded in the
//! OTLP/JSON wire format — the exact shape any OTel collector ingests via
//! its OTLP/HTTP receiver. This is deliberately a **file emitter**, not an
//! SDK exporter: no `opentelemetry` crate, no async runtime, no network.
//! It keeps Boruna v3.0 local-only while still letting a run show up in the
//! observability stack a buyer already runs (Jaeger, Tempo, Honeycomb,
//! Datadog, …) simply by POSTing the emitted document, or piping it through
//! the collector's `otlpjson` file receiver.
//!
//! ## Why this matters: Boruna as the NOTARIZED upstream
//!
//! CRITICAL — the whole point of this exporter is the tamper-evidence
//! carried on the **root span's attributes**:
//!   * `boruna.bundle_hash` — SHA-256 over the manifest (file checksums +
//!     audit_log_hash). Recomputable by `boruna evidence verify`.
//!   * `boruna.audit_log_hash` — head of the hash-chained audit log.
//!   * `boruna.signature.keyid` — ed25519 public key (hex) that signed
//!     `bundle_hash`, when the bundle is signed.
//!
//! A span in a buyer's tracing backend therefore links back to a
//! independently verifiable record: anyone can take these attributes,
//! re-run `evidence verify` against the bundle, and prove the trace was
//! produced by an untampered run. Boruna is the notarized source feeding
//! the observability graph — not just another emitter of unattested spans.
//!
//! ## Determinism
//!
//! Wall clocks and RNGs are unavailable in the deterministic core, and the
//! export must be byte-stable (same bundle → same bytes). So:
//!   * trace/span IDs are derived from `sha256(run_id [":" index])` — never
//!     random. The 16-byte trace id and 8-byte span ids are stable slices
//!     of those digests.
//!   * span start/end times are ordinal: a base nanosecond anchor (parsed
//!     from the manifest's `started_at`, or 0 if unparseable) plus the
//!     event index. They encode ordering, not measured latency.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::audit::evidence::BundleManifest;
use boruna_vm::replay::{Event, EventLog};

/// Errors emitting an OTLP/JSON document from a bundle.
#[derive(Debug, thiserror::Error)]
pub enum OtelExportError {
    #[error("cannot read {file}: {source}")]
    Io {
        file: String,
        source: std::io::Error,
    },
    #[error("invalid manifest.json: {0}")]
    BadManifest(serde_json::Error),
    #[error("invalid event_log.json: {0}")]
    BadEventLog(String),
    #[error("serialization failed: {0}")]
    Serialize(serde_json::Error),
}

/// OTLP/JSON `TracesData` document.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TracesData {
    resource_spans: Vec<ResourceSpans>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceSpans {
    resource: Resource,
    scope_spans: Vec<ScopeSpans>,
}

#[derive(Serialize)]
struct Resource {
    attributes: Vec<KeyValue>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScopeSpans {
    scope: Scope,
    spans: Vec<Span>,
}

#[derive(Serialize)]
struct Scope {
    name: String,
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Span {
    trace_id: String,
    span_id: String,
    /// Empty string for the root span (OTLP convention).
    parent_span_id: String,
    name: String,
    /// SPAN_KIND_INTERNAL = 1, SPAN_KIND_CLIENT = 3.
    kind: u32,
    start_time_unix_nano: String,
    end_time_unix_nano: String,
    attributes: Vec<KeyValue>,
    events: Vec<SpanEvent>,
    status: Status,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpanEvent {
    time_unix_nano: String,
    name: String,
    attributes: Vec<KeyValue>,
}

#[derive(Serialize)]
struct Status {
    /// STATUS_CODE_UNSET = 0, OK = 1, ERROR = 2.
    code: u32,
}

#[derive(Serialize)]
struct KeyValue {
    key: String,
    value: AnyValue,
}

/// OTLP `AnyValue` oneof, JSON-encoded. int64 is a string per proto3 JSON.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct AnyValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    string_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bool_value: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    int_value: Option<String>,
}

fn str_val(s: impl Into<String>) -> AnyValue {
    AnyValue {
        string_value: Some(s.into()),
        ..Default::default()
    }
}
fn bool_val(b: bool) -> AnyValue {
    AnyValue {
        bool_value: Some(b),
        ..Default::default()
    }
}
fn int_val(i: u64) -> AnyValue {
    AnyValue {
        int_value: Some(i.to_string()),
        ..Default::default()
    }
}
fn kv(key: &str, value: AnyValue) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value,
    }
}

/// Read `bundle_dir` and emit its execution as an OTLP/JSON traces
/// document (pretty-printed, deterministic bytes).
///
/// Root span: `boruna.run` carrying the run identity + tamper-evidence
/// attributes drawn from `manifest.json`. Child spans: one per VM event
/// in `event_log.json` (when present) — `llm.*` capability calls become
/// `gen_ai.*` spans (GenAI semantic conventions), other effects become
/// `boruna.capability` / `boruna.*` spans. `ContractCheck` events are
/// recorded as span events on the root span.
pub fn bundle_to_otlp_json(bundle_dir: &Path) -> Result<String, OtelExportError> {
    let manifest_path = bundle_dir.join("manifest.json");
    let manifest_json =
        std::fs::read_to_string(&manifest_path).map_err(|e| OtelExportError::Io {
            file: "manifest.json".to_string(),
            source: e,
        })?;
    let manifest: BundleManifest =
        serde_json::from_str(&manifest_json).map_err(OtelExportError::BadManifest)?;

    // event_log.json is optional: a bundle sealed without a VM event log
    // (e.g. `evidence create` from a persisted run) still exports a root
    // span carrying the tamper-evidence anchor.
    let event_log = match std::fs::read_to_string(bundle_dir.join("event_log.json")) {
        Ok(s) => Some(EventLog::from_json(&s).map_err(OtelExportError::BadEventLog)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(OtelExportError::Io {
                file: "event_log.json".to_string(),
                source: e,
            })
        }
    };

    let doc = build_traces(&manifest, event_log.as_ref());
    serde_json::to_string_pretty(&doc).map_err(OtelExportError::Serialize)
}

/// Base nanosecond anchor for ordinal span times: the manifest's
/// `started_at` parsed as RFC3339, or 0 when it can't be parsed.
fn base_nanos(manifest: &BundleManifest) -> u64 {
    chrono::DateTime::parse_from_rfc3339(&manifest.started_at)
        .ok()
        .and_then(|dt| dt.timestamp_nanos_opt())
        .map(|n| n.max(0) as u64)
        .unwrap_or(0)
}

fn build_traces(manifest: &BundleManifest, event_log: Option<&EventLog>) -> TracesData {
    let trace_id = trace_id_from(&manifest.run_id);
    let root_span_id = span_id_from(&manifest.run_id, "root");
    let base = base_nanos(manifest);

    let events = event_log.map(|l| l.events()).unwrap_or(&[]);
    // A CapResult consumed into its matching CapCall span does not get a
    // span of its own — track which indices were folded in.
    let mut consumed = vec![false; events.len()];

    // ContractCheck events fold onto the root span as span events; child
    // spans come from every other VM event.
    let mut root_events: Vec<SpanEvent> = Vec::new();
    let mut child_spans: Vec<Span> = Vec::new();

    for (i, ev) in events.iter().enumerate() {
        if consumed[i] {
            continue;
        }
        match ev {
            Event::ContractCheck {
                function,
                kind,
                index,
                passed,
            } => {
                root_events.push(SpanEvent {
                    time_unix_nano: (base + i as u64).to_string(),
                    name: "boruna.contract_check".to_string(),
                    attributes: vec![
                        kv("boruna.contract.function", str_val(function.clone())),
                        kv("boruna.contract.kind", str_val(kind.clone())),
                        kv("boruna.contract.index", int_val(*index as u64)),
                        kv("boruna.contract.passed", bool_val(*passed)),
                    ],
                });
            }
            Event::CapCall { capability, args } => {
                // Fold the next matching, not-yet-consumed CapResult into
                // this span so one operation = one span.
                let result_idx = events
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .find_map(|(j, e)| match e {
                        Event::CapResult { capability: c, .. }
                            if c == capability && !consumed[j] =>
                        {
                            Some(j)
                        }
                        _ => None,
                    });
                let mut passed = true;
                if let Some(j) = result_idx {
                    consumed[j] = true;
                    if let Event::CapResult { result, .. } = &events[j] {
                        // A Value::Err result marks the operation failed.
                        passed = !matches!(result, boruna_bytecode::Value::Err(_));
                    }
                }
                child_spans.push(capability_span(
                    &trace_id,
                    &root_span_id,
                    &manifest.run_id,
                    i,
                    base,
                    capability,
                    args.len(),
                    passed,
                ));
            }
            Event::CapResult { capability, result } => {
                // An unpaired CapResult (no preceding CapCall) still gets a
                // span so the trace loses nothing.
                let passed = !matches!(result, boruna_bytecode::Value::Err(_));
                child_spans.push(generic_span(
                    &trace_id,
                    &root_span_id,
                    &manifest.run_id,
                    i,
                    base,
                    "boruna.capability_result",
                    vec![kv("boruna.capability", str_val(capability.clone()))],
                    passed,
                ));
            }
            Event::ActorSpawn { actor_id, function } => {
                child_spans.push(generic_span(
                    &trace_id,
                    &root_span_id,
                    &manifest.run_id,
                    i,
                    base,
                    "boruna.actor_spawn",
                    vec![
                        kv("boruna.actor.id", int_val(*actor_id)),
                        kv("boruna.actor.function", str_val(function.clone())),
                    ],
                    true,
                ));
            }
            Event::MessageSend { from, to, .. } => {
                child_spans.push(generic_span(
                    &trace_id,
                    &root_span_id,
                    &manifest.run_id,
                    i,
                    base,
                    "boruna.message_send",
                    vec![
                        kv("boruna.message.from", int_val(*from)),
                        kv("boruna.message.to", int_val(*to)),
                    ],
                    true,
                ));
            }
            Event::MessageReceive { actor_id, .. } => {
                child_spans.push(generic_span(
                    &trace_id,
                    &root_span_id,
                    &manifest.run_id,
                    i,
                    base,
                    "boruna.message_receive",
                    vec![kv("boruna.actor.id", int_val(*actor_id))],
                    true,
                ));
            }
            Event::UiEmit { .. } => {
                child_spans.push(generic_span(
                    &trace_id,
                    &root_span_id,
                    &manifest.run_id,
                    i,
                    base,
                    "boruna.ui_emit",
                    vec![],
                    true,
                ));
            }
            Event::SchedulerTick {
                round,
                active_actor,
            } => {
                child_spans.push(generic_span(
                    &trace_id,
                    &root_span_id,
                    &manifest.run_id,
                    i,
                    base,
                    "boruna.scheduler_tick",
                    vec![
                        kv("boruna.scheduler.round", int_val(*round)),
                        kv("boruna.scheduler.active_actor", int_val(*active_actor)),
                    ],
                    true,
                ));
            }
        }
    }

    let root = Span {
        trace_id,
        span_id: root_span_id,
        parent_span_id: String::new(),
        name: "boruna.run".to_string(),
        kind: 1, // INTERNAL
        start_time_unix_nano: base.to_string(),
        // Root ends after the last event so children nest inside it.
        end_time_unix_nano: (base + events.len() as u64 + 1).to_string(),
        attributes: root_attributes(manifest),
        events: root_events,
        status: Status { code: 1 },
    };

    // Root first, then children (their parentSpanId points back at root).
    let mut spans = Vec::with_capacity(1 + child_spans.len());
    spans.push(root);
    spans.extend(child_spans);

    let version = env!("CARGO_PKG_VERSION").to_string();
    TracesData {
        resource_spans: vec![ResourceSpans {
            resource: Resource {
                attributes: vec![
                    kv("service.name", str_val("boruna")),
                    kv("service.version", str_val(version.clone())),
                    kv("boruna.run_id", str_val(manifest.run_id.clone())),
                ],
            },
            scope_spans: vec![ScopeSpans {
                scope: Scope {
                    name: "boruna.evidence".to_string(),
                    version,
                },
                spans,
            }],
        }],
    }
}

/// Tamper-evidence + identity attributes on the root span. This is the
/// span that links the trace back to a verifiable record — see the module
/// docs. `boruna.signature.keyid` is emitted only for signed bundles.
fn root_attributes(manifest: &BundleManifest) -> Vec<KeyValue> {
    let mut attrs = vec![
        kv("boruna.run_id", str_val(manifest.run_id.clone())),
        kv(
            "boruna.workflow_name",
            str_val(manifest.workflow_name.clone()),
        ),
        kv(
            "boruna.workflow_hash",
            str_val(manifest.workflow_hash.clone()),
        ),
        kv("boruna.policy_hash", str_val(manifest.policy_hash.clone())),
        // --- tamper-evidence: recomputable/verifiable by `evidence verify` ---
        kv("boruna.bundle_hash", str_val(manifest.bundle_hash.clone())),
        kv(
            "boruna.audit_log_hash",
            str_val(manifest.audit_log_hash.clone()),
        ),
    ];
    if let Some(sig) = &manifest.signature {
        attrs.push(kv(
            "boruna.signature.algorithm",
            str_val(sig.algorithm.clone()),
        ));
        // The public key is the key id a verifier pins with
        // `evidence verify --verify-key`.
        attrs.push(kv(
            "boruna.signature.keyid",
            str_val(sig.public_key.clone()),
        ));
    }
    attrs
}

/// A capability-call span. `llm.*` capabilities map to the OTel GenAI
/// semantic conventions (`gen_ai.*`); everything else is a generic
/// `boruna.capability` span.
#[allow(clippy::too_many_arguments)]
fn capability_span(
    trace_id: &str,
    parent: &str,
    run_id: &str,
    index: usize,
    base: u64,
    capability: &str,
    args_count: usize,
    passed: bool,
) -> Span {
    let mut attributes = vec![
        kv("boruna.capability", str_val(capability.to_string())),
        kv("boruna.capability.args_count", int_val(args_count as u64)),
    ];
    let (name, kind) = if let Some(op) = gen_ai_operation(capability) {
        // GenAI semantic conventions: gen_ai.system + gen_ai.operation.name.
        attributes.push(kv("gen_ai.system", str_val("boruna")));
        attributes.push(kv("gen_ai.operation.name", str_val(op.clone())));
        (format!("gen_ai.{op}"), 3u32) // CLIENT
    } else {
        ("boruna.capability".to_string(), 1u32) // INTERNAL
    };
    Span {
        trace_id: trace_id.to_string(),
        span_id: span_id_from(run_id, &index.to_string()),
        parent_span_id: parent.to_string(),
        name,
        kind,
        start_time_unix_nano: (base + index as u64).to_string(),
        end_time_unix_nano: (base + index as u64 + 1).to_string(),
        attributes,
        events: Vec::new(),
        status: Status {
            code: if passed { 1 } else { 2 },
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn generic_span(
    trace_id: &str,
    parent: &str,
    run_id: &str,
    index: usize,
    base: u64,
    name: &str,
    attributes: Vec<KeyValue>,
    passed: bool,
) -> Span {
    Span {
        trace_id: trace_id.to_string(),
        span_id: span_id_from(run_id, &index.to_string()),
        parent_span_id: parent.to_string(),
        name: name.to_string(),
        kind: 1, // INTERNAL
        start_time_unix_nano: (base + index as u64).to_string(),
        end_time_unix_nano: (base + index as u64 + 1).to_string(),
        attributes,
        events: Vec::new(),
        status: Status {
            code: if passed { 1 } else { 2 },
        },
    }
}

/// Map an `llm.*` capability to a GenAI `gen_ai.operation.name`. Returns
/// `None` for non-LLM capabilities. The operation is normalized toward the
/// GenAI convention's vocabulary where the suffix is recognizable, else the
/// raw suffix is passed through.
fn gen_ai_operation(capability: &str) -> Option<String> {
    let suffix = capability.strip_prefix("llm.")?;
    let op = match suffix {
        "complete" | "completion" | "completions" => "text_completion",
        // `llm.call` is Boruna's single generic LLM capability; map it to
        // the convention's most common operation.
        "chat" | "chat_completion" | "call" => "chat",
        "embed" | "embedding" | "embeddings" => "embeddings",
        "" => "chat",
        other => other,
    };
    Some(op.to_string())
}

/// 16-byte (32 hex) trace id derived from the run id. Deterministic, never
/// random — see module docs.
fn trace_id_from(run_id: &str) -> String {
    let digest = Sha256::digest(run_id.as_bytes());
    to_hex(&digest[..16])
}

/// 8-byte (16 hex) span id derived from `run_id` + a per-span tag (the
/// event index, or "root"). Deterministic.
fn span_id_from(run_id: &str, tag: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(run_id.as_bytes());
    hasher.update(b":");
    hasher.update(tag.as_bytes());
    let digest = hasher.finalize();
    to_hex(&digest[..8])
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::evidence::EvidenceBundleBuilder;
    use crate::audit::log::AuditLog;
    use boruna_bytecode::{Capability, ContractKind, Value};
    use boruna_vm::replay::EventLog;

    /// Build a bundle on disk carrying a VM event log with a mix of
    /// events, including an `llm.*` capability call. Returns the bundle
    /// directory path (inside `dir`).
    fn make_bundle(dir: &Path, run_id: &str) -> std::path::PathBuf {
        let mut log = EventLog::new();
        // an LLM capability call + result → should become a gen_ai span
        log.log_cap_call(&Capability::LlmCall, &[Value::String("prompt".into())]);
        log.log_cap_result(&Capability::LlmCall, &Value::String("answer".into()));
        // a non-LLM capability → generic boruna.capability span
        log.log_cap_call(
            &Capability::NetFetch,
            &[Value::String("https://example.com".into())],
        );
        log.log_cap_result(&Capability::NetFetch, &Value::String("<html/>".into()));
        let event_log_json = log.to_json().unwrap();

        let mut builder = EvidenceBundleBuilder::new(dir, run_id, "otel-wf").unwrap();
        builder.add_workflow_def(r#"{"name":"otel"}"#).unwrap();
        builder.add_policy(r#"{"default_allow":true}"#).unwrap();
        builder.add_file("event_log.json", &event_log_json).unwrap();

        let mut audit = AuditLog::new();
        audit.append(crate::audit::log::AuditEvent::WorkflowStarted {
            workflow_hash: "wfh".into(),
            policy_hash: "plh".into(),
        });
        audit.append(crate::audit::log::AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 5,
        });
        builder.finalize(&audit).unwrap();
        dir.join(run_id)
    }

    fn parse(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    /// Collect (name, attributes-map) for every span in the document.
    fn spans(doc: &serde_json::Value) -> Vec<serde_json::Value> {
        doc["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn attr<'a>(span: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        span["attributes"]
            .as_array()?
            .iter()
            .find(|kv| kv["key"] == key)
            .map(|kv| &kv["value"])
    }

    #[test]
    fn root_span_carries_identity_and_tamper_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = make_bundle(dir.path(), "run-otel-001");
        let out = bundle_to_otlp_json(&bundle).unwrap();
        let doc = parse(&out);
        let spans = spans(&doc);

        let root = &spans[0];
        assert_eq!(root["name"], "boruna.run");
        assert_eq!(root["parentSpanId"], "");
        // trace id is 32 hex chars, span id 16 hex chars
        assert_eq!(root["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(root["spanId"].as_str().unwrap().len(), 16);

        // run_id present
        assert_eq!(
            attr(root, "boruna.run_id").unwrap()["stringValue"],
            "run-otel-001"
        );
        // the tamper-evidence anchors are present and non-empty
        let bundle_hash = attr(root, "boruna.bundle_hash").unwrap()["stringValue"]
            .as_str()
            .unwrap()
            .to_string();
        let audit_hash = attr(root, "boruna.audit_log_hash").unwrap()["stringValue"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(bundle_hash.len(), 64);
        assert_eq!(audit_hash.len(), 64);

        // and they match the manifest exactly (the span links back to a
        // verifiable record).
        let manifest: BundleManifest =
            serde_json::from_str(&std::fs::read_to_string(bundle.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(bundle_hash, manifest.bundle_hash);
        assert_eq!(audit_hash, manifest.audit_log_hash);
    }

    #[test]
    fn capability_events_produce_child_spans() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = make_bundle(dir.path(), "run-otel-002");
        let doc = parse(&bundle_to_otlp_json(&bundle).unwrap());
        let spans = spans(&doc);

        // root + 2 capability spans (each CapCall folds its CapResult).
        assert_eq!(spans.len(), 3, "expected root + 2 capability spans");

        // every child points at the root
        let root_id = spans[0]["spanId"].as_str().unwrap();
        for child in &spans[1..] {
            assert_eq!(child["parentSpanId"], root_id);
        }

        // the non-LLM call is a generic boruna.capability span
        let net = spans
            .iter()
            .find(|s| {
                attr(s, "boruna.capability").map(|v| v["stringValue"] == "net.fetch") == Some(true)
            })
            .unwrap();
        assert_eq!(net["name"], "boruna.capability");
    }

    #[test]
    fn llm_capability_produces_gen_ai_span() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = make_bundle(dir.path(), "run-otel-003");
        let doc = parse(&bundle_to_otlp_json(&bundle).unwrap());
        let spans = spans(&doc);

        let genai = spans
            .iter()
            .find(|s| s["name"].as_str().map(|n| n.starts_with("gen_ai.")) == Some(true))
            .expect("an llm.* call must produce a gen_ai.* span");

        assert_eq!(genai["name"], "gen_ai.chat");
        assert_eq!(
            attr(genai, "gen_ai.operation.name").unwrap()["stringValue"],
            "chat"
        );
        assert_eq!(
            attr(genai, "gen_ai.system").unwrap()["stringValue"],
            "boruna"
        );
        // GenAI spans are CLIENT kind
        assert_eq!(genai["kind"], 3);
    }

    #[test]
    fn contract_checks_become_root_span_events() {
        // Build a bundle whose event log includes a ContractCheck.
        let dir = tempfile::tempdir().unwrap();
        let mut log = EventLog::new();
        log.log_contract_check("main", ContractKind::Requires, 0, true);
        let mut builder = EvidenceBundleBuilder::new(dir.path(), "run-otel-004", "wf").unwrap();
        builder
            .add_file("event_log.json", &log.to_json().unwrap())
            .unwrap();
        builder.finalize(&AuditLog::new()).unwrap();

        let doc = parse(&bundle_to_otlp_json(&dir.path().join("run-otel-004")).unwrap());
        let root = &spans(&doc)[0];
        let events = root["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["name"], "boruna.contract_check");
        let a = &events[0]["attributes"];
        let function = a
            .as_array()
            .unwrap()
            .iter()
            .find(|kv| kv["key"] == "boruna.contract.function")
            .unwrap();
        assert_eq!(function["value"]["stringValue"], "main");
    }

    #[test]
    fn signed_bundle_emits_signature_keyid() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = EventLog::new();
        log.log_cap_call(&Capability::LlmCall, &[]);
        let mut builder = EvidenceBundleBuilder::new(dir.path(), "run-otel-005", "wf")
            .unwrap()
            .with_signing_key(&[7u8; 32]);
        builder
            .add_file("event_log.json", &log.to_json().unwrap())
            .unwrap();
        builder.finalize(&AuditLog::new()).unwrap();

        let doc = parse(&bundle_to_otlp_json(&dir.path().join("run-otel-005")).unwrap());
        let root = &spans(&doc)[0];
        let keyid = attr(root, "boruna.signature.keyid").expect("signed bundle emits keyid");
        assert_eq!(keyid["stringValue"].as_str().unwrap().len(), 64);
        assert_eq!(
            attr(root, "boruna.signature.algorithm").unwrap()["stringValue"],
            "ed25519"
        );
    }

    #[test]
    fn output_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = make_bundle(dir.path(), "run-otel-006");
        let a = bundle_to_otlp_json(&bundle).unwrap();
        let b = bundle_to_otlp_json(&bundle).unwrap();
        assert_eq!(a, b, "same bundle must export byte-identical OTLP/JSON");
    }

    #[test]
    fn bundle_without_event_log_still_exports_root() {
        // No event_log.json → root span only, still carrying the anchors.
        let dir = tempfile::tempdir().unwrap();
        let mut builder = EvidenceBundleBuilder::new(dir.path(), "run-otel-007", "wf").unwrap();
        builder.add_workflow_def(r#"{"name":"x"}"#).unwrap();
        builder.finalize(&AuditLog::new()).unwrap();

        let doc = parse(&bundle_to_otlp_json(&dir.path().join("run-otel-007")).unwrap());
        let spans = spans(&doc);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0]["name"], "boruna.run");
        assert!(attr(&spans[0], "boruna.bundle_hash").is_some());
    }
}
