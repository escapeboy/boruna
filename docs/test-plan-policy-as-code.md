# Test plan — Policy management as code (sprint 0.4-S15)

Companion to `docs/design-policy-as-code.md` and
`docs/architecture-policy-as-code.md`.

## boruna-vm: `policy_validate` unit tests

In `crates/llmvm/src/policy_validate.rs` (`#[cfg(test)] mod tests`).

### Happy path

| # | Test | Input | Expectation |
|---|---|---|---|
| 1 | `parse_minimal_object` | `{}` | `Policy` with `default_allow=false`, no rules, no net_policy |
| 2 | `parse_explicit_default_allow` | `{"default_allow": true}` | matches `Policy::allow_all()` semantically |
| 3 | `parse_explicit_schema_version_1` | `{"schema_version": 1}` | ok |
| 4 | `parse_with_rules` | `{"rules": {"net.fetch": {"allow": true, "budget": 10}}}` | rule registered |
| 5 | `parse_with_net_policy` | full net_policy block | bounds preserved |
| 6 | `parse_round_trip` | serialize via `serde_json::to_string` then re-validate | second parse succeeds |
| 7 | `parse_file_valid` | tempfile with valid JSON | round-trip ok |
| 8 | `parse_zero_budget_means_unlimited` | `{"rules": {"net.fetch": {"allow": true, "budget": 0}}}` | ok (existing semantics) |

### Schema version

| # | Test | Input | Expectation |
|---|---|---|---|
| 9 | `reject_schema_version_2` | `{"schema_version": 2}` | `UnknownSchemaVersion(2)`, error_kind `policy.unknown_schema_version` |
| 10 | `reject_schema_version_0` | `{"schema_version": 0}` | `UnknownSchemaVersion(0)` |

### Unknown fields

| # | Test | Input | Expectation |
|---|---|---|---|
| 11 | `reject_unknown_top_level_field` | `{"foo": 1}` | `UnknownField{path:"foo"}`, kind `policy.unknown_field` |
| 12 | `reject_unknown_net_policy_field` | `{"net_policy": {"foo": 1}}` | path `net_policy.foo` |
| 13 | `reject_typo_default_alow` | `{"default_alow": true}` | path `default_alow`, regression for the silent-default footgun |

### Capability names

| # | Test | Input | Expectation |
|---|---|---|---|
| 14 | `reject_alias_capability_net` | `{"rules": {"net": {...}}}` | `InvalidCapability{found:"net", hint:Some("net.fetch")}` |
| 15 | `reject_alias_capability_db` | `{"rules": {"db": {...}}}` | hint `db.query` |
| 16 | `reject_unknown_capability` | `{"rules": {"future.cap": {...}}}` | `InvalidCapability{found:"future.cap", hint:None}` |
| 17 | `accept_all_canonical_capabilities` | one rule per `Capability::ALL` | all eleven accepted |

### NetPolicy bounds

| # | Test | Input | Expectation |
|---|---|---|---|
| 18 | `reject_max_response_zero` | `{"net_policy":{"max_response_bytes":0}}` | `InvalidNetPolicy{field:"max_response_bytes"}` |
| 19 | `reject_timeout_zero` | `{"net_policy":{"timeout_ms":0}}` | `InvalidNetPolicy{field:"timeout_ms"}` |
| 20 | `reject_lowercase_method` | `{"net_policy":{"allowed_methods":["get"]}}` | rejected with hint to use upper-case |
| 21 | `reject_unknown_method` | `{"net_policy":{"allowed_methods":["JUMP"]}}` | rejected |
| 22 | `accept_canonical_method_set` | `["GET","POST","PUT","DELETE","PATCH","HEAD","OPTIONS"]` | all accepted |

### Parse / IO errors

| # | Test | Input | Expectation |
|---|---|---|---|
| 23 | `parse_malformed_json` | `{` | `Parse(_)`, kind `policy.parse_error` |
| 24 | `parse_file_missing` | nonexistent path | `Io{..}`, kind `policy.io_error` |

### Error kind taxonomy (locked)

| # | Test | Expectation |
|---|---|---|
| 25 | `error_kind_strings_locked` | every `PolicyParseError` variant returns the documented string in the design doc; if a variant is added without updating this test, it fails |

## CLI integration tests

Either in `crates/llmvm-cli/tests/` or extending an existing CLI
test file.

| # | Test | Cmd | Expectation |
|---|---|---|---|
| 26 | `cli_policy_validate_ok` | `boruna policy validate fixtures/valid.json` | exit 0, prints `OK` |
| 27 | `cli_policy_validate_fail_human` | `boruna policy validate fixtures/unknown_field.json` | exit 2, stderr contains `policy.unknown_field` |
| 28 | `cli_policy_validate_fail_json` | `... validate --json fixtures/...json` | exit 2, stdout parses to `{ ok: false, errors: [{ error_kind: "...", ... }] }` |
| 29 | `cli_policy_show_ok` | `boruna policy show fixtures/valid.json` | exit 0, output contains `Schema version: 1` and rule list |
| 30 | `cli_policy_show_fail` | `boruna policy show fixtures/unknown_field.json` | exit 2 (validate-fails-show-fails — same gate) |
| 31 | `cli_run_invalid_policy_returns_kind` | `boruna run script.ax --policy fixtures/unknown_field.json` | non-zero exit, stderr contains `policy.unknown_field` (regression for "validate vs run drift") |

Test fixtures under `crates/llmvm-cli/tests/fixtures/policies/`:
- `valid_minimal.json` — `{}`
- `valid_full.json` — schema_version + rules + net_policy
- `invalid_unknown_field.json` — `{"foo": 1}`
- `invalid_schema_version.json` — `{"schema_version": 2}`
- `invalid_capability_alias.json` — `{"rules":{"net":{"allow":true,"budget":0}}}`
- `invalid_net_policy.json` — `{"net_policy":{"timeout_ms":0}}`

## MCP integration tests

In `crates/boruna-mcp/tests/`.

| # | Test | Expectation |
|---|---|---|
| 32 | `boruna_run_policy_unknown_field` | call `boruna_run` with `policy: {"foo":1}` → `success: false`, `error_kind: "policy.unknown_field"` |
| 33 | `boruna_run_policy_invalid_capability` | structured policy with `rules:{"net":...}` → `error_kind: "policy.invalid_capability"` |
| 34 | `boruna_policy_validate_tool_ok` | new tool, valid file → `success: true, schema_version: 1` |
| 35 | `boruna_policy_validate_tool_fail` | invalid file → `success: true` (per convention: domain errors are tool successes), `errors[0].error_kind: "policy.unknown_field"` |
| 36 | `protocol_version_present_for_policy_validate` | extends existing `protocol_version_tests` suite — new tool's success and failure paths both carry `protocol_version: 1` |

## End-to-end smoke (manual)

After CI passes, run locally:

```sh
cargo run --bin boruna -- policy validate \
    crates/llmvm-cli/tests/fixtures/policies/valid_full.json
# → exit 0, "OK"

cargo run --bin boruna -- policy show \
    crates/llmvm-cli/tests/fixtures/policies/valid_full.json
# → denormalized policy

cargo run --bin boruna -- policy validate \
    crates/llmvm-cli/tests/fixtures/policies/invalid_capability_alias.json
# → exit 2, stderr "policy.invalid_capability ... did you mean net.fetch?"

cargo run --bin boruna -- run examples/hello.ax \
    --policy crates/llmvm-cli/tests/fixtures/policies/invalid_net_policy.json
# → non-zero exit, error includes "policy.invalid_net_policy"
```

## Regression tests carried forward

Per convention #11 — **`#[serde(default)]` on every new metadata field**:
this sprint adds zero new persisted fields; nothing to lock.

Per convention #15 — **replay-verified vs. operational annotation**:
this sprint adds zero new persisted columns; nothing to annotate.

Per the locked `protocol_version_tests` regression suite: extend
with the new tool's success + failure paths (test #36 above).

## Adversarial review focus areas

When the adversarial reviewer runs (`ce-correctness-reviewer`,
`ce-data-integrity-guardian` — convention #29), specifically prompt
for:

1. **Validate-vs-run drift** — can a file pass `policy validate`
   and then fail at `boruna run`? Find any code path that bypasses
   `policy_validate::parse` for a JSON file.
2. **Strict-deserializer drift** — does `Policy`'s lenient
   `Deserialize` ever reach a file path it shouldn't? Audit
   `serde_json::from_str::<Policy>` call sites (esp. in the
   orchestrator and audit pipeline).
3. **Error message PII / path leakage** — `Io { path: PathBuf }`
   includes the user's local path; confirm we don't expose this
   over MCP without redaction.
4. **Round-trip stability** — serialize → re-validate idempotence.
5. **Capability catalog skew** — when a new capability is added in
   the future, what test fails first? (Should be test #17,
   `accept_all_canonical_capabilities`.)
