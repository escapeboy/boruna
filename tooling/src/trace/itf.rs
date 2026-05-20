//! Informal Trace Format (ITF) v0.15 — emit-only.
//!
//! See `docs/architecture-itf-traces.md` and the ITF spec at
//! <https://apalache-mc.org/docs/adr/015adr-trace.html>.
//!
//! Boruna uses ITF as an *export* format so traces are consumable by the
//! ITF Trace Viewer VS Code extension and other formal-methods tooling.
//! Internal evidence-bundle and trace2tests formats are unchanged.

use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};

use boruna_bytecode::Value;

/// Frozen ITF format version emitted by this builder.
///
/// A bump signals the vendored schema in `itf-v0.15.schema.json` is out of
/// date — drift test in `tests/itf_schema_drift.rs` fails first.
pub const ITF_FORMAT_VERSION: &str = "0.15";

/// URL of the canonical ITF spec — emitted in `#meta.format-description`.
pub const ITF_SPEC_URL: &str = "https://apalache-mc.org/docs/adr/015adr-trace.html";

/// Top-level ITF document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItfDoc {
    /// Meta block (required by the ITF spec).
    #[serde(rename = "#meta")]
    pub meta: ItfMeta,
    /// Names of state variables, in stable order.
    pub vars: Vec<String>,
    /// Sequence of states. `states[0]` is the initial state.
    pub states: Vec<ItfState>,
}

/// `#meta` block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItfMeta {
    /// Always the literal `"ITF"` — required by the spec.
    pub format: String,
    /// Human-readable spec URL.
    #[serde(rename = "format-description")]
    pub description: String,
    /// Producer identifier — `"boruna <version>"`.
    pub source: String,
    /// `ok` | `violation` | `unknown`.
    pub status: ItfStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ItfStatus {
    /// No invariant violation observed across the trace.
    Ok,
    /// The final state violates an invariant.
    Violation,
    /// Status could not be determined.
    Unknown,
}

/// One state in the trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItfState {
    /// Optional per-state metadata (e.g., action name, step index).
    #[serde(rename = "#meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<ItfStateMeta>,
    /// Variable bindings. Order is preserved because `BTreeMap` is stable.
    #[serde(flatten)]
    pub vars: BTreeMap<String, ItfValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ItfStateMeta {
    /// Index of this state in the trace, 0-based.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    /// Name of the action that produced this state, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

/// ITF value variant. The `#tag` keys are required by the ITF spec to
/// disambiguate the otherwise overlapping JSON encodings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ItfValue {
    /// Boolean.
    Bool(bool),
    /// Signed integer (i64 range).
    Int(i64),
    /// String / large-int-as-string.
    Str(String),
    /// Big integer beyond i64.
    BigInt {
        #[serde(rename = "#bigint")]
        value: String,
    },
    /// Tuple / sequence.
    Tup {
        #[serde(rename = "#tup")]
        items: Vec<ItfValue>,
    },
    /// Set.
    Set {
        #[serde(rename = "#set")]
        items: Vec<ItfValue>,
    },
    /// Map: list of [key, value] pairs.
    Map {
        #[serde(rename = "#map")]
        entries: Vec<(ItfValue, ItfValue)>,
    },
    /// Record: named-field object (no special tag, just a plain JSON map).
    /// Field name keys are normal strings; this variant is the catch-all for
    /// `BTreeMap<String, ItfValue>`.
    Record(BTreeMap<String, ItfValue>),
    /// Value that has no faithful ITF representation. The descriptor preserves
    /// enough info to identify the original (e.g. `"FnRef:42"`).
    Unserializable {
        #[serde(rename = "#unserializable")]
        descriptor: String,
    },
}

impl ItfDoc {
    /// Build an empty doc with default meta for the given producer source.
    pub fn new(source: impl Into<String>, status: ItfStatus) -> Self {
        Self {
            meta: ItfMeta {
                format: "ITF".to_string(),
                description: ITF_SPEC_URL.to_string(),
                source: source.into(),
                status,
            },
            vars: Vec::new(),
            states: Vec::new(),
        }
    }

    /// Compute the union of variable names across all states, sorted.
    ///
    /// Call this after populating `states` if you want `vars` populated
    /// automatically.
    pub fn derive_vars(&mut self) {
        let mut names = std::collections::BTreeSet::new();
        for st in &self.states {
            for name in st.vars.keys() {
                names.insert(name.clone());
            }
        }
        self.vars = names.into_iter().collect();
    }
}

impl ItfState {
    pub fn new() -> Self {
        Self {
            meta: None,
            vars: BTreeMap::new(),
        }
    }

    pub fn with_index(mut self, index: u64) -> Self {
        let mut meta = self.meta.take().unwrap_or_default();
        meta.index = Some(index);
        self.meta = Some(meta);
        self
    }

    pub fn with_action(mut self, action: impl Into<String>) -> Self {
        let mut meta = self.meta.take().unwrap_or_default();
        meta.action = Some(action.into());
        self.meta = Some(meta);
        self
    }

    pub fn set(&mut self, var: impl Into<String>, value: ItfValue) {
        self.vars.insert(var.into(), value);
    }
}

impl Default for ItfState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Value conversion ─────────────────────────────────────────────

impl From<&Value> for ItfValue {
    fn from(v: &Value) -> Self {
        match v {
            Value::Unit => ItfValue::Record(BTreeMap::new()),
            Value::Bool(b) => ItfValue::Bool(*b),
            Value::Int(i) => ItfValue::Int(*i),
            // ITF has no float type — encode as decimal string with the same
            // formatting Boruna uses for Display. Round-trip-identifiable
            // because no other Boruna value emits this exact pattern.
            Value::Float(f) => ItfValue::Str(f.to_string()),
            Value::String(s) => ItfValue::Str(s.clone()),
            Value::None => record_with_tag("None", None),
            Value::Some(inner) => record_with_tag("Some", Some(inner.as_ref())),
            Value::Ok(inner) => record_with_tag("Ok", Some(inner.as_ref())),
            Value::Err(inner) => record_with_tag("Err", Some(inner.as_ref())),
            Value::Record { type_id, fields } => {
                let mut m = BTreeMap::new();
                m.insert("__type_id".to_string(), ItfValue::Int(*type_id as i64));
                for (i, f) in fields.iter().enumerate() {
                    m.insert(format!("f{i}"), ItfValue::from(f));
                }
                ItfValue::Record(m)
            }
            Value::Enum {
                type_id,
                variant,
                payload,
            } => {
                let mut m = BTreeMap::new();
                m.insert("__type_id".to_string(), ItfValue::Int(*type_id as i64));
                m.insert("variant".to_string(), ItfValue::Int(*variant as i64));
                m.insert("payload".to_string(), ItfValue::from(payload.as_ref()));
                ItfValue::Record(m)
            }
            Value::List(items) => ItfValue::Tup {
                items: items.iter().map(ItfValue::from).collect(),
            },
            Value::Map(entries) => ItfValue::Map {
                entries: entries
                    .iter()
                    .map(|(k, v)| (ItfValue::Str(k.clone()), ItfValue::from(v)))
                    .collect(),
            },
            Value::ActorId(id) => ItfValue::Unserializable {
                descriptor: format!("ActorId:{id}"),
            },
            Value::FnRef(idx) => ItfValue::Unserializable {
                descriptor: format!("FnRef:{idx}"),
            },
        }
    }
}

fn record_with_tag(tag: &str, payload: Option<&Value>) -> ItfValue {
    let mut m = BTreeMap::new();
    m.insert("tag".to_string(), ItfValue::Str(tag.to_string()));
    if let Some(p) = payload {
        m.insert("value".to_string(), ItfValue::from(p));
    }
    ItfValue::Record(m)
}

// ─── Serialization ────────────────────────────────────────────────

/// Write the ITF document to `out` as pretty-printed JSON.
pub fn write_itf_doc<W: io::Write>(doc: &ItfDoc, mut out: W) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(doc).map_err(io::Error::other)?;
    out.write_all(&bytes)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Convenience: serialize to a JSON string (pretty-printed).
pub fn to_string_pretty(doc: &ItfDoc) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_format_is_literal_itf() {
        let doc = ItfDoc::new("boruna-test", ItfStatus::Ok);
        let json = serde_json::to_value(&doc).unwrap();
        assert_eq!(json["#meta"]["format"], "ITF");
        assert_eq!(json["#meta"]["status"], "ok");
        assert!(json["#meta"]["source"]
            .as_str()
            .unwrap()
            .starts_with("boruna"));
    }

    #[test]
    fn bool_value_serializes_as_bare_bool() {
        let v = ItfValue::Bool(true);
        assert_eq!(serde_json::to_value(&v).unwrap(), serde_json::json!(true));
    }

    #[test]
    fn int_serializes_as_bare_number() {
        let v = ItfValue::Int(42);
        assert_eq!(serde_json::to_value(&v).unwrap(), serde_json::json!(42));
    }

    #[test]
    fn tup_uses_hash_tup_key() {
        let v = ItfValue::Tup {
            items: vec![ItfValue::Int(1), ItfValue::Int(2)],
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["#tup"], serde_json::json!([1, 2]));
    }

    #[test]
    fn map_uses_hash_map_key_as_pairs() {
        let v = ItfValue::Map {
            entries: vec![(ItfValue::Str("a".into()), ItfValue::Int(1))],
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["#map"], serde_json::json!([["a", 1]]));
    }

    #[test]
    fn unserializable_uses_hash_unserializable_key() {
        let v = ItfValue::Unserializable {
            descriptor: "FnRef:7".into(),
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["#unserializable"], "FnRef:7");
    }

    #[test]
    fn value_int_converts() {
        let v: ItfValue = (&Value::Int(7)).into();
        assert!(matches!(v, ItfValue::Int(7)));
    }

    #[test]
    fn value_string_converts() {
        let v: ItfValue = (&Value::String("hi".into())).into();
        assert!(matches!(v, ItfValue::Str(s) if s == "hi"));
    }

    #[test]
    fn value_unit_converts_to_empty_record() {
        let v: ItfValue = (&Value::Unit).into();
        match v {
            ItfValue::Record(m) => assert!(m.is_empty()),
            _ => panic!("expected Record"),
        }
    }

    #[test]
    fn value_none_carries_tag_none() {
        let v: ItfValue = (&Value::None).into();
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["tag"], "None");
    }

    #[test]
    fn value_some_carries_tag_some_and_value() {
        let v: ItfValue = (&Value::Some(Box::new(Value::Int(7)))).into();
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["tag"], "Some");
        assert_eq!(json["value"], 7);
    }

    #[test]
    fn value_ok_carries_tag_ok() {
        let v: ItfValue = (&Value::Ok(Box::new(Value::Int(1)))).into();
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["tag"], "Ok");
    }

    #[test]
    fn value_err_carries_tag_err() {
        let v: ItfValue = (&Value::Err(Box::new(Value::String("boom".into())))).into();
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["tag"], "Err");
        assert_eq!(json["value"], "boom");
    }

    #[test]
    fn value_list_converts_to_tup() {
        let v: ItfValue = (&Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])).into();
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["#tup"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn value_map_converts_to_hash_map() {
        let mut m = BTreeMap::new();
        m.insert("a".to_string(), Value::Int(1));
        m.insert("b".to_string(), Value::Int(2));
        let v: ItfValue = (&Value::Map(m)).into();
        let json = serde_json::to_value(&v).unwrap();
        // map is sorted because of BTreeMap iteration order
        assert_eq!(json["#map"], serde_json::json!([["a", 1], ["b", 2]]));
    }

    #[test]
    fn value_actor_id_unserializable() {
        let v: ItfValue = (&Value::ActorId(42)).into();
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["#unserializable"], "ActorId:42");
    }

    #[test]
    fn value_fn_ref_unserializable() {
        let v: ItfValue = (&Value::FnRef(3)).into();
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["#unserializable"], "FnRef:3");
    }

    #[test]
    fn derive_vars_collects_state_keys_sorted() {
        let mut doc = ItfDoc::new("boruna-test", ItfStatus::Ok);
        let mut s0 = ItfState::new();
        s0.set("balance", ItfValue::Int(0));
        s0.set("alice", ItfValue::Int(5));
        let mut s1 = ItfState::new();
        s1.set("balance", ItfValue::Int(7));
        s1.set("carol", ItfValue::Int(1));
        doc.states.push(s0);
        doc.states.push(s1);
        doc.derive_vars();
        assert_eq!(doc.vars, vec!["alice", "balance", "carol"]);
    }

    #[test]
    fn state_with_index_sets_meta() {
        let s = ItfState::new().with_index(3);
        assert_eq!(s.meta.unwrap().index, Some(3));
    }

    #[test]
    fn state_with_action_sets_meta() {
        let s = ItfState::new().with_action("Transfer");
        assert_eq!(s.meta.unwrap().action.as_deref(), Some("Transfer"));
    }

    #[test]
    fn write_itf_doc_emits_valid_json_with_trailing_newline() {
        let mut doc = ItfDoc::new("boruna-test 1.4.0", ItfStatus::Ok);
        let mut s0 = ItfState::new().with_index(0).with_action("Init");
        s0.set("n", ItfValue::Int(0));
        doc.states.push(s0);
        doc.derive_vars();

        let mut buf = Vec::new();
        write_itf_doc(&doc, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.ends_with('\n'));
        // Round-trip through serde_json — proves the trailing newline is the only
        // non-JSON content and the rest is parseable.
        let trimmed = text.trim_end();
        let _: serde_json::Value =
            serde_json::from_str(trimmed).expect("emitted ITF must be valid JSON");
    }

    #[test]
    fn record_value_emits_with_type_id() {
        let v: ItfValue = (&Value::Record {
            type_id: 7,
            fields: vec![Value::Int(1), Value::String("hi".into())],
        })
            .into();
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["__type_id"], 7);
        assert_eq!(json["f0"], 1);
        assert_eq!(json["f1"], "hi");
    }

    #[test]
    fn enum_value_emits_with_variant_and_payload() {
        let v: ItfValue = (&Value::Enum {
            type_id: 2,
            variant: 1,
            payload: Box::new(Value::Int(42)),
        })
            .into();
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["__type_id"], 2);
        assert_eq!(json["variant"], 1);
        assert_eq!(json["payload"], 42);
    }

    #[test]
    fn float_value_encoded_as_string() {
        let v: ItfValue = (&Value::Float(1.5)).into();
        let json = serde_json::to_value(&v).unwrap();
        assert!(json.is_string());
        assert_eq!(json.as_str().unwrap(), "1.5");
    }

    #[test]
    fn itf_version_constant_is_zero_dot_fifteen() {
        // Drift fence — if ITF bumps, the schema and this constant both must move.
        assert_eq!(ITF_FORMAT_VERSION, "0.15");
    }
}
