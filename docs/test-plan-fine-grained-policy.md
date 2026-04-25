# Test Plan: Fine-Grained Capability Policy in `boruna_run`

**Sprint A** · See `docs/architecture-fine-grained-policy.md` for design.

## Test matrix

**Scope discipline:** `.ax` source cannot directly emit `CapCall` opcodes — capability invocations happen via the framework `Effect` system, not user-callable builtins. So end-to-end runtime denial (a script that *triggers* a denied capability) cannot run through `run_source(source: &str)`. That layer is already tested in `crates/llmvm/src/tests.rs` (`test_capability_denied`, `test_capability_budget`) — those tests will continue to pass and prove the underlying behavior. Sprint A tests therefore cover the **plumbing layer** that this sprint changes: parsing, serde round-trip, error path.

| # | Scenario | Input | Expected output | Type | Location |
|---|---|---|---|---|---|
| 1 | Default (no policy field) → allow-all | `parse_policy(None)` | `Ok(Policy { default_allow: true, .. })` | unit | `tools/run.rs` |
| 2 | Legacy `"allow-all"` string | `parse_policy(Some(&"allow-all"))` | `Ok(Policy { default_allow: true, .. })` | unit | `tools/run.rs` |
| 3 | Legacy `"deny-all"` string | `parse_policy(Some(&"deny-all"))` | `Ok(Policy { default_allow: false, .. })` | unit | `tools/run.rs` |
| 4 | Unknown string | `parse_policy(Some(&"garbage"))` | `Err` with message guiding to object form | unit | `tools/run.rs` |
| 5 | Wrong JSON type (array) | `parse_policy(Some(&[]))` | `Err` | unit | `tools/run.rs` |
| 6 | Malformed object (bad field type) | `parse_policy(Some(&{"default_allow": "yes"}))` | `Err` (serde failure) | unit | `tools/run.rs` |
| 7 | Structured Policy object | `parse_policy(Some(&{"default_allow": true, "rules": {"net.fetch": {"allow": false, "budget": 0}}}))` | `Ok(Policy)` with rule populated | unit | `tools/run.rs` |
| 8 | Round-trip a Policy through serde | Build Policy in Rust → `serde_json::to_value` → `parse_policy` → assert structural equality on key fields | unit | `tools/run.rs` |
| 9 | `run_source` with valid structured policy | Run a pure `.ax` program (no caps), pass full Policy object | `success: true` in JSON output | unit | `tools/run.rs` |
| 10 | `run_source` with invalid policy | Run a pure `.ax` program with `policy: 42` (number) | JSON contains `"success": false, "error_kind": "invalid_policy"` | unit | `tools/run.rs` |
| 11 | `run_source` legacy default still works | No policy field, pure program | `success: true` (proves backwards compat) | unit | `tools/run.rs` |
| 12 | Schema doc examples parse | Each JSON example in `docs/reference/policy-schema.md` round-trips via `serde_json::from_str::<Policy>` | unit | `tools/run.rs` (or `tests/`) |

**Deferred (already covered by `boruna-vm` tests, not duplicated here):** runtime per-capability denial, runtime budget exhaustion, net domain allowlist enforcement.

## Source fixture (minimal `.ax` program — pure, no caps)

```text
fn main() -> Int {
    1 + 2
}
```

This is enough for tests #9–#11 because we are exercising **policy parsing**, not capability invocation.

## Regression tests preserved

- All existing `boruna-mcp` tests must still pass without modification.
- `cargo test -p boruna-vm` — capability gateway tests untouched (these prove runtime denial/budget behavior).
- `cargo test -p boruna-cli` — CLI `--policy` parsing untouched.

## Edge cases explicitly out of scope (deferred)

- Concurrent `boruna_run` calls with different policies — not relevant; each call gets a fresh gateway.
- Policy schema versioning beyond `schema_version: 1` — current field already exists; bumping is a future migration.
- LlmCall via real backend — gated by `boruna-effect`, not changed in this sprint.

## Acceptance gates (before Ship)

- [ ] All 12 tests above pass
- [ ] `cargo test --workspace` green (557+ tests including new ones)
- [ ] `cargo clippy --workspace -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] `docs/reference/policy.schema.json` validates as JSON Schema 2020-12 (visual inspection or `ajv`)
- [ ] CHANGELOG.md updated under `Unreleased`
