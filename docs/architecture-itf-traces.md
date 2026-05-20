# Architecture — ITF (Informal Trace Format) trace export

Companion to `docs/design-itf-traces.md`. **Implemented this sprint.**

## Component map

| Component | Location | Role |
|---|---|---|
| ITF types | `tooling/src/trace/itf.rs` (new) | `ItfDoc`, `ItfState`, `ItfValue` |
| Value converter | same | `impl From<&Value> for ItfValue` |
| Serializer | same | `pub fn write_itf_doc(doc, out) -> io::Result<()>` |
| Vendored schema | `tooling/src/trace/itf-v0.15.schema.json` (new) | For drift test only |
| `trace2tests --out-itf` | `tooling/src/trace2tests/mod.rs` | New flag, emits per-test ITF file |
| `evidence inspect --format=itf` | `crates/llmvm-cli/src/main.rs` + `orchestrator::audit::evidence` | New format option |

## Data flow

```
[trace2tests path]
TraceMinimizer produces FailingTrace { states: Vec<State> }
  ↓ if --out-itf=traces/ flag set
ItfDoc::from(failing_trace)
  ↓
write_itf_doc(doc, "traces/out_0.itf.json")

[evidence inspect path]
boruna evidence inspect <bundle> --format=itf
  ↓
load EvidenceBundle::open(bundle)
  ↓
ItfDoc::from(bundle.audit_log)
  ↓
write_itf_doc(doc, stdout)
```

## ITF type model

ITF v0.15 schema in `tooling/src/trace/itf-v0.15.schema.json`. Rust types mirror it:

```rust
// tooling/src/trace/itf.rs

pub const ITF_FORMAT_VERSION: &str = "0.15";

#[derive(Debug, Serialize, Deserialize)]
pub struct ItfDoc {
    #[serde(rename = "#meta")]
    pub meta: ItfMeta,
    pub vars: Vec<String>,
    pub states: Vec<ItfState>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ItfMeta {
    pub format: &'static str,           // "ITF"
    #[serde(rename = "format-description")]
    pub description: String,
    pub source: String,                  // "boruna v1.5.0"
    pub status: ItfStatus,               // "ok" | "violation"
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ItfState {
    #[serde(rename = "#meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<ItfStateMeta>,
    #[serde(flatten)]
    pub vars: BTreeMap<String, ItfValue>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ItfValue {
    Bool(bool),
    Int(i64),
    Str(String),
    BigInt { #[serde(rename = "#bigint")] value: String },
    Tup { #[serde(rename = "#tup")] items: Vec<ItfValue> },
    Set { #[serde(rename = "#set")] items: Vec<ItfValue> },
    Map { #[serde(rename = "#map")] entries: Vec<(ItfValue, ItfValue)> },
    Record(BTreeMap<String, ItfValue>),
    Unserializable { #[serde(rename = "#unserializable")] descriptor: String },
}
```

`BTreeMap` (per the project conventions on determinism) for ordered iteration.

## `Value` → `ItfValue` conversion

```rust
impl From<&boruna_bytecode::Value> for ItfValue {
    fn from(v: &Value) -> Self {
        match v {
            Value::Bool(b) => ItfValue::Bool(*b),
            Value::Int(i) => ItfValue::Int(*i),
            Value::Float(f) => ItfValue::Str(f.to_string()),     // ITF has no float; encode as decimal string
            Value::String(s) => ItfValue::Str(s.clone()),
            Value::Unit => ItfValue::Record(BTreeMap::new()),
            Value::None => ItfValue::Record(/*{"tag":"None"}*/),
            Value::Some(inner) => ItfValue::Record(record_of("Some", inner)),
            Value::Ok(inner) => ItfValue::Record(record_of("Ok", inner)),
            Value::Err(inner) => ItfValue::Record(record_of("Err", inner)),
            Value::Record(fields) => ItfValue::Record(
                fields.iter().map(|(k, v)| (k.clone(), ItfValue::from(v))).collect()
            ),
            Value::Enum { tag, payload } => ItfValue::Record(/*{"tag":tag,"value":payload}*/),
            Value::List(items) => ItfValue::Tup {
                items: items.iter().map(ItfValue::from).collect()
            },
            Value::Map(entries) => ItfValue::Map {
                entries: entries.iter().map(|(k, v)|
                    (ItfValue::from(k), ItfValue::from(v))).collect()
            },
            Value::ActorId(id) => ItfValue::Unserializable {
                descriptor: format!("ActorId:{}", id)
            },
            Value::FnRef(idx) => ItfValue::Unserializable {
                descriptor: format!("FnRef:{}", idx)
            },
        }
    }
}
```

## CLI surface additions

```
# trace2tests gains an export flag
boruna trace2tests <bundle> --out-itf <DIR>

# evidence inspect gains a format option
boruna evidence inspect <bundle> --format=itf [--out <PATH>]
boruna evidence inspect <bundle> --format=json   # default, unchanged
boruna evidence inspect <bundle> --format=human  # default, unchanged
```

## File map (new files this sprint)

| File | LoC est. |
|---|---|
| `tooling/src/trace/mod.rs` (new module) | ~10 |
| `tooling/src/trace/itf.rs` | ~280 |
| `tooling/src/trace/itf-v0.15.schema.json` (vendored) | ~80 |
| `tooling/src/lib.rs` (add `pub mod trace;`) | +1 |
| `tooling/tests/itf_conversion.rs` | ~150 |
| `tooling/tests/itf_schema_drift.rs` | ~60 |
| `tooling/src/trace2tests/mod.rs` (add `--out-itf` plumbing) | +60 |
| `crates/llmvm-cli/src/main.rs` (add `--format=itf` to evidence inspect) | +40 |
| `orchestrator/src/audit/evidence.rs` (add `to_itf()` method on `AuditLog`) | +50 |
| `orchestrator/tests/evidence_inspect_itf.rs` | ~80 |

**Total: ~810 lines including tests.**

## Schema drift test (per conventions §33)

`tooling/tests/itf_schema_drift.rs` walks the vendored `itf-v0.15.schema.json` as
`serde_json::Value`, asserts:

1. `#meta.format` is the literal string `"ITF"`
2. `states` is required at the top level
3. Each known `ItfValue` variant has a corresponding `#tup`/`#set`/`#map`/`#bigint`/`#unserializable`
   path in the schema
4. We emit `format-description: "https://apalache-mc.org/docs/adr/015adr-trace.html"`

Cheap, no jsonschema dependency.

## Dependencies

Only `serde` + `serde_json` (already present). No new dependencies.

## Test plan reference

Test plan: `docs/test-plan-itf-traces.md` (written this sprint).
