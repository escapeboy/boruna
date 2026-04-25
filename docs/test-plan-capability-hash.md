# Test Plan: Versioned Capability Identity

**Sprint:** `0.3-S11`

## Unit tests — `crates/llmbc/src/capability.rs`

| ID | Test | Assertion |
|---|---|---|
| U1 | `test_capability_all_is_sorted_by_name` | `Capability::ALL` is sorted ascending by `name()`. |
| U2 | `test_capability_all_has_no_duplicates` | All 10 entries unique. |
| U3 | `test_capability_all_covers_every_variant` | Every `Capability` variant appears in `ALL` exactly once (loop `from_id(0..10)`). |
| U4 | `test_capability_version_is_one_for_all_shipped` | Every variant returns `"1"`. (Locks current contract; future bumps must update this test deliberately.) |
| U5 | `test_capability_set_hash_is_deterministic` | Two calls return identical `capability_set_hash`. |
| U6 | `test_capability_set_hash_known_value` | `capability_set_report("0.2.0").capability_set_hash` equals a hardcoded golden hash. Regression guard against accidental algorithm change. |
| U7 | `test_capability_set_report_shape` | Report has 10 capabilities, each with non-empty `name` and `version`, name field == `"boruna"`, version field == passed `"0.2.0"`. |
| U8 | `test_capability_set_hash_changes_on_version_bump` | Use a test-only helper that rebuilds the report from a custom `(name, version)` slice; assert hash differs from baseline when one version changes. |

## Integration tests

| ID | Test | Where | Assertion |
|---|---|---|---|
| I1 | `test_cli_capability_list_json` | `crates/llmvm-cli/tests/` (or inline in main if no test dir) | Run `boruna capability list --json`, parse stdout, assert 10 caps, hash starts with `"sha256:"`, hex length 64. |
| I2 | `test_mcp_capability_list_tool` | `crates/boruna-mcp/tests/` | Call `tools::capability::list_capabilities()`, parse JSON, assert `success: true` + same shape as CLI. |
| I3 | `test_mcp_and_cli_agree` | Either crate's tests | `tools::capability::list_capabilities()` JSON contains identical `capability_set_hash` as CLI helper output. |

## Workspace gates (must pass)

- `cargo test --workspace` — 557+ existing tests still pass, +new tests above.
- `cargo clippy --workspace -- -D warnings` — zero warnings.
- `cargo fmt --all -- --check` — clean.
- (Optional) `cargo test --workspace --features boruna-vm/http` — http path still builds.

## Manual smoke

```bash
cargo run --bin boruna -- capability list
cargo run --bin boruna -- capability list --json | jq .
cargo run --bin boruna-mcp <<<'{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"boruna_capability_list","arguments":{}}}'
```

Expected: human table for first command; pretty JSON with `capability_set_hash: "sha256:..."` for second; MCP returns same hash.

## Edge cases

- **Empty input** to MCP tool — no params accepted, so impossible. No need to test.
- **Concurrent calls** — the function is pure (no global state, no I/O). Safe by construction.
- **Unicode in capability names** — current names are ASCII-only; the `\t`/`\n` separators are unambiguous as long as names don't contain them. Asserted in U1/U2 implicitly (names are static `&str` constants).

## Acceptance criteria mapping (from design doc)

| Criterion | Covered by |
|---|---|
| 1. CLI prints JSON, exits 0 | I1, manual smoke |
| 2. MCP tool returns same shape | I2, I3 |
| 3. Hash deterministic across runs | U5, U6 |
| 4. Hash changes if `(name, version)` changes | U8 |
| 5. Documentation in `docs/reference/capability-identity.md` | (Build phase artifact, reviewed in Review phase) |
| 6. Per-cap version defaults to `"1"` | U4 |
