# Test plan — ITF trace export

For ITF emission from `trace2tests --out-itf` and `evidence inspect --format=itf`.
Architecture: `docs/architecture-itf-traces.md`.

## Acceptance criteria

1. `boruna evidence inspect <bundle> --format=itf` produces a JSON document that conforms to
   ITF v0.15.
2. `boruna trace2tests <bundle> --out-itf=<dir>` produces one `out_<n>.itf.json` per minimized
   failing trace.
3. Every Boruna `Value` variant has a defined `ItfValue` mapping.
4. Schema drift test passes against the vendored `itf-v0.15.schema.json`.
5. `jq` can extract fields from the emitted ITF (i.e., it's well-formed JSON, not
   serde-tag-mangled garbage).
6. No internal format changes — existing `evidence inspect --format=json` byte-for-byte
   unchanged.

## Unit tests (in `tooling/tests/itf_conversion.rs`)

### Value mapping coverage

For every `Value` variant, assert the produced `ItfValue` matches the documented shape:

| ID | Boruna `Value` | Expected `ItfValue` |
|---|---|---|
| I01 | `Value::Bool(true)` | `ItfValue::Bool(true)` |
| I02 | `Value::Int(42)` | `ItfValue::Int(42)` |
| I03 | `Value::Float(3.14)` | `ItfValue::Str("3.14")` |
| I04 | `Value::String("hello")` | `ItfValue::Str("hello")` |
| I05 | `Value::Unit` | empty Record |
| I06 | `Value::None` | Record `{"tag": "None"}` |
| I07 | `Value::Some(Int(7))` | Record `{"tag":"Some", "value": 7}` |
| I08 | `Value::Ok(...)` | Record `{"tag":"Ok", ...}` |
| I09 | `Value::Err(...)` | Record `{"tag":"Err", ...}` |
| I10 | `Value::Record({"a": Int(1)})` | Record `{"a": 1}` |
| I11 | `Value::Enum{tag, payload}` | Record `{"tag": tag, "value": payload}` |
| I12 | `Value::List([1,2,3])` | `#tup` of `[1,2,3]` |
| I13 | `Value::Map({"a": 1})` | `#map` of `[["a", 1]]` |
| I14 | `Value::ActorId(7)` | `#unserializable: "ActorId:7"` |
| I15 | `Value::FnRef(3)` | `#unserializable: "FnRef:3"` |

### Document-level shape

| ID | Scenario | Expected |
|---|---|---|
| I16 | Doc has `#meta.format = "ITF"` | always |
| I17 | Doc has `#meta.source` mentioning "boruna" + version | always |
| I18 | `vars` array is the sorted set of state-variable names | always |
| I19 | `states` is in trace order, not sorted | always |

### Schema drift test (`tooling/tests/itf_schema_drift.rs`)

Walks the vendored `itf-v0.15.schema.json` and asserts:

| ID | Schema invariant |
|---|---|
| D01 | `#meta.format` is the literal `"ITF"` |
| D02 | `states` is required at the top level |
| D03 | Required `#meta.source` field |
| D04 | `#tup`, `#set`, `#map`, `#bigint`, `#unserializable` are present in the value-variant union |

If Apalache bumps the ITF version, this test fails and signals time to vendor a new schema.
Per conventions §33.

## CLI tests (in `orchestrator/tests/evidence_inspect_itf.rs`)

| ID | Command | Expected |
|---|---|---|
| C01 | `evidence inspect <bundle> --format=itf` | exit 0, valid JSON to stdout |
| C02 | `evidence inspect <bundle> --format=itf --out file.json` | file.json created, stdout empty |
| C03 | `evidence inspect <bundle> --format=json` | byte-for-byte unchanged from pre-sprint output |
| C04 | `evidence inspect <bundle> --format=human` | byte-for-byte unchanged from pre-sprint output |
| C05 | `evidence inspect <bundle> --format=unknown` | exit 2, clap error |
| C06 | `evidence inspect <missing-bundle> --format=itf` | exit 1, error_kind: `evidence.not_found` |

## Trace2tests integration tests

| ID | Command | Expected |
|---|---|---|
| T01 | `trace2tests <bundle> --out-itf=traces/` | traces/out_0.itf.json exists per failing trace |
| T02 | `trace2tests <bundle>` (without `--out-itf`) | existing behavior unchanged |
| T03 | each emitted file passes the schema drift check | always |

## End-to-end (`jq` interop)

A test that:

1. Runs `boruna evidence inspect <bundle> --format=itf`
2. Pipes output through `jq '.states[0].vars'`
3. Asserts non-empty output

(Skipped on CI runners without `jq`; gated on `cfg!(target_os = "linux")` + which-check.)

## Regression: ensure no internal format change

Snapshot test: produce an evidence bundle, call `evidence inspect --format=json` before and
after this sprint's changes. They must be byte-for-byte identical.

## Out of scope (deferred)

- ITF import / parse-back
- Streaming output for huge traces
- ITF extensions beyond v0.15
- `boruna evidence diff --format=itf`
