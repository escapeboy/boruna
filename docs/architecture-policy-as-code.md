# Architecture — Policy management as code (sprint 0.4-S15)

Companion to `docs/design-policy-as-code.md`. This doc covers
*how*, not *what* — file layout, types, error mapping, and CLI
wiring decisions.

## Module placement

New module: `crates/llmvm/src/policy_validate.rs` (in `boruna-vm`).

Justification: `Policy`, `NetPolicy`, and `PolicyRule` already live
in `boruna-vm::capability_gateway`. The validator operates on the
same struct family and needs `Capability::from_name` (which lives
in `boruna-bytecode`, already a dep of `boruna-vm`). Putting the
validator in `boruna-vm` keeps everything cohesive and avoids
adding a dep to `boruna-orchestrator` or the CLI.

`pub use boruna_vm::policy_validate::{parse, parse_file,
PolicyParseError}` exported from the crate root.

## Public API

```rust
pub fn parse(json: &str) -> Result<Policy, PolicyParseError>;
pub fn parse_file(path: &std::path::Path) -> Result<Policy, PolicyParseError>;

#[derive(Debug)]
pub enum PolicyParseError {
    Io { path: PathBuf, source: std::io::Error },
    Parse(serde_json::Error),
    UnknownSchemaVersion(u32),
    UnknownField { path: String, found: String },
    InvalidCapability { found: String, hint: Option<String> },
    InvalidNetPolicy { field: &'static str, reason: String },
}

impl PolicyParseError {
    /// Stable string per project convention #2; locked forever.
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

impl std::fmt::Display for PolicyParseError { ... }
impl std::error::Error for PolicyParseError { ... }
```

Locked: variant names, `error_kind()` strings, the requirement that
every variant has a stable `error_kind`. Adding new variants is
additive; renaming or removing is a breaking change.

## Validation pipeline

`parse(json)` runs three passes:

1. **Lexical parse** — `serde_json::from_str::<serde_json::Value>(json)`.
   Failure → `PolicyParseError::Parse`.
2. **Field discrimination** — walk the JSON object manually,
   checking each key against an allow-list. Sub-objects (`rules`
   children, `net_policy`) handled recursively. Failure →
   `UnknownField { path: "net_policy.foo", found: "foo" }`.
3. **Semantic validation** — typed deserialization into a strict
   `PolicyFileV1` struct (with `#[serde(deny_unknown_fields)]` as a
   second-line defense), then:
   - Reject `schema_version` ≠ `1` (and absent counts as 1).
   - For each rule key, look up via `Capability::from_name`. Reject
     if `None`. Reject if matched-but-not-canonical (e.g. `"net"`)
     with a hint to the canonical name.
   - Validate `net_policy.max_response_bytes > 0`,
     `net_policy.timeout_ms > 0`, `net_policy.allowed_methods` all
     ∈ {GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS} (canonical
     upper-case; lower-case rejected).
4. **Convert** to `Policy` via `From<PolicyFileV1>`.

The two-stage check (manual walk + `deny_unknown_fields` on the
struct) is deliberate: `deny_unknown_fields` alone gives a
`serde_json::Error` that is awkward to map back to a stable
`error_kind` (string-matching on the message). The manual walk
gives precise paths and stable kinds.

## Capability rule keys: canonical only

`Capability::from_name` accepts both canonical (`"net.fetch"`) and
short aliases (`"net"`). The validator rejects aliases — it accepts
only what `Capability::name()` would emit. Reasoning: round-tripping
a policy through serde should preserve keys, and at gateway-check
time only canonical names are matched. Today an alias in a policy
file is silently a no-op (footgun per convention #1).

The error message includes the canonical name as a hint:
`unknown capability "net" — did you mean "net.fetch"?`

## NetPolicy method canonicalization

Methods must be upper-case (`"GET"`, not `"get"`). We don't auto-
upper-case during parse because the same string appears in
`PolicyFileV1` (deserialized) and `Policy` (validated form), and
silently transforming would mean the file-on-disk value differs
from the in-memory value — convention #1. Reject lower-case with
a clear message.

## CLI surface

```
boruna policy validate <file> [--json]
boruna policy show <file>
```

In `crates/llmvm-cli/src/main.rs`:

- New top-level `Command::Policy { command: PolicyCommand }`.
- `enum PolicyCommand { Validate { file, json }, Show { file } }`.
- `fn cmd_policy_validate(file, json) -> ExitCode` — calls
  `boruna_vm::policy_validate::parse_file`, prints either
  human-readable or `{ ok: true|false, errors: [...] }`.
- `fn cmd_policy_show(file)` — parses, then writes a denormalized
  textual report to stdout. Format:
  ```
  Schema version: 1
  Default behavior: deny / allow
  Rules:
    net.fetch    allow   budget=10
    fs.read      deny    -
  Net policy:
    allowed_domains: api.example.com, *.googleapis.com
    allowed_methods: GET, POST
    max_response_bytes: 10485760 (10 MB)
    timeout_ms: 30000
    allow_redirects: true
  ```

`make_gateway` (line 1528) is rewritten to call
`policy_validate::parse_file` for the file path, mapping errors
to `Box<dyn Error>` while preserving the `error_kind` string in
the message via `Display`.

## MCP wiring

`crates/boruna-mcp/src/tools/run.rs::parse_policy` (line 535) is
rewritten to call `boruna_vm::policy_validate::parse` for the
structured-object case. The string-magic cases (`"allow-all"`,
`"deny-all"`, paths) keep their behavior; the JSON-content case is
the strict path.

New MCP tool: `boruna_policy_validate(file: String) -> { ok,
schema_version?, errors: [{ error_kind, path?, message }] }`.

`error_kind` strings in MCP tool responses are already part of the
locked taxonomy (convention #2 + #4 + the
`protocol_version_tests` regression suite). The new
`policy.*` strings are additive.

## File-size hard limit

Out of scope this sprint. Policy files are read by trusted CLI
operators or via the MCP `policy: { ... }` argument which is
already capped at the MCP source-size limit (1 MB). No additional
limit.

## Backwards compatibility

- Existing `Policy` derive on `serde::Deserialize` is **unchanged**.
  We keep the lenient deserializer for `policy_json` round-trips
  inside Boruna's own audit / evidence pipeline (replay-verified
  per convention #15; written by us, read by us).
- The strict path is opt-in via `policy_validate::parse`.
- Policy files that previously parsed via the lenient path *and*
  conform to the strict rules continue to work end-to-end.
- Files that relied on lenient acceptance (alias keys, lower-case
  methods, unknown fields) will fail validate. Per convention #1
  this is the desired direction; per the design doc this is
  explicitly accepted.

## Logging

The CLI's `validate` command exits with code 0 on ok, code 2 on
validation error, code 1 on file IO error. (Convention: 0 = ok,
1 = system error, 2 = user error.)

Telemetry / OTel attributes — none added this sprint. Validate is
a CLI-only concern; runtime parse failures already surface through
existing channels.

## File diff summary (estimated)

| File | Change |
|---|---|
| `crates/llmvm/src/policy_validate.rs` | NEW |
| `crates/llmvm/src/lib.rs` | re-export `policy_validate` |
| `crates/llmvm/src/capability_gateway.rs` | no change |
| `crates/llmvm-cli/src/main.rs` | new `Policy` subcommand + rewrite `make_gateway` policy parsing |
| `crates/boruna-mcp/src/tools/run.rs` | rewrite structured-object path of `parse_policy` |
| `crates/boruna-mcp/src/tools/mod.rs` | register `boruna_policy_validate` tool |
| `crates/boruna-mcp/src/tools/policy_validate.rs` | NEW |
| `docs/reference/policy-schema.md` | NEW |
| `CHANGELOG.md` | `[Unreleased]` entries |
| `tests/cli_policy.rs` (or extend existing CLI tests) | new tests |
