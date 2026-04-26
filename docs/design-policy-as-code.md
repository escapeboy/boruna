# Design — Policy management as code (sprint 0.4-S15)

## Premise

Boruna's `Policy` controls what side effects an execution can perform.
Today, operators pass `--policy <file.json>` to `boruna run` and
`boruna workflow run`. The path is read with a blind
`serde_json::from_str` — no schema-version check, no typed errors, no
deny-extra, no validation of the values themselves. A typo or stale
field silently degrades to defaults.

The 0.4.0 cycle's premise is operations: making Boruna usable in
team environments. Policy is the most security-relevant configuration
artifact; it deserves to be a versioned, validated, code-reviewable
file format with the same care we give every other contract surface
(per project conventions #1, #2, #5).

## Who needs this

Operators / SREs running Boruna in dev/staging/prod environments.

Today they:
- Hand-write `policy.json`, eyeball the structure, hope `serde`
  accepts it.
- Discover failures only at runtime, when a workflow either over-
  permits (silent default) or denies a capability they expected to
  allow.
- Have no CI gate — they cannot fail a PR that touched
  `policies/prod.json` if the file is broken.

After this sprint they should be able to:
- Commit policy files alongside code, with `boruna policy validate`
  as a CI gate (exit code 0 ↔ valid).
- Trust that a passing validate means the running binary will accept
  the same file at runtime (no parse drift between validate and run).
- See the effective policy denormalized via `boruna policy show`
  before it ships.

## Narrowest MVP

Two CLI subcommands and one validator:

1. **`boruna policy validate <file> [--json]`**
   - Parses the file with deny-extra and a fixed `schema_version` set.
   - Validates value bounds (capability names, net_policy ranges).
   - Exits non-zero on any error; emits stable `error_kind` strings.
   - `--json` prints a structured `{ ok: bool, errors: [...] }` for CI.

2. **`boruna policy show <file>`**
   - Loads + validates, then prints the effective policy:
     - default behavior (allow vs deny)
     - rule list (capability → allow/deny + budget)
     - net_policy bounds, if set

3. **One shared validator** used by:
   - `boruna run --policy <file>`
   - `boruna workflow run --policy <file>`
   - `boruna policy validate <file>`
   - `boruna_run` MCP tool's `policy: { ... }` argument
   So passing validate and failing run is structurally impossible.

That's the MVP. Anything else ships in a follow-up sprint.

## What would make someone say "whoa"

- The MCP tool returns the **same** `error_kind` strings as the CLI.
  An LLM agent that gets `policy.unknown_schema_version` from
  `boruna_run` can `boruna policy validate` to confirm and present
  the user a path-of-action, with no string-equality drift.
- `boruna policy show` denormalizes implicit defaults (e.g. when
  `default_allow: false`, the list shows the explicit deny effect for
  every capability not listed). No more "is this `default_allow`
  honored or shadowed?"

## How this compounds

- Schema-version is now load-bearing. Future capability additions
  remain at v1 (additive); shape changes bump to v2 with a clear
  migration story. The current binary will reject v2 files cleanly,
  not pretend to understand them.
- Once `boruna policy validate` is in CI, policy review becomes a
  diff review like any other code change.
- The error_kind taxonomy becomes the foundation for richer tooling:
  future `boruna policy diff <a> <b>`, `boruna policy explain <file>
  <capability>`, `boruna policy lint --strict` (lint vs validate is
  intentionally a future split).

## Scope (what this sprint changes)

- New: `boruna policy validate` and `boruna policy show` subcommands.
- New: typed `PolicyParseError` with stable `error_kind` strings.
- New: deny-extra on top-level Policy fields and on NetPolicy fields.
- New: `schema_version` validation — only `1` is accepted (absent
  field defaults to 1 for back-compat with existing files).
- New: capability-name validation in `rules` keys against the known
  catalog.
- New: `NetPolicy` value-bound validation (`max_response_bytes > 0`,
  `timeout_ms > 0`, `allowed_methods` upper-cased and from a known
  set).
- New: `boruna_policy_validate` MCP tool exposing the same validator.
- Wiring: `boruna run` / `boruna workflow run` / `boruna_run` MCP all
  go through the new validator (replaces blind `from_str`).
- Docs: `docs/reference/policy-schema.md` describing the full v1
  shape, error kinds, and examples.

## Non-goals (deferred)

- No new policy DSL, YAML format, or composition / inheritance.
- No changes to the runtime semantics of `Policy` — same JSON shape
  is accepted, same enforcement at the gateway.
- No `boruna policy lint` (warnings — separate from errors). Lint is
  a future sprint; this one is errors only.
- No policy-bundle distribution (registry / download). Operators
  distribute their own files.
- No approval workflow for policy changes. That's a higher-level
  product feature, not core platform.
- No JSON-schema (`$schema`) export — the validator is the source of
  truth. We may add a generated JSON-schema file later as a
  convenience for editor tooling.
- No line/column attribution in errors beyond what `serde_json`
  provides natively. We may add this later if operators ask.

## Stable error kinds (locked in this sprint)

| `error_kind` | When it fires |
|---|---|
| `policy.io_error` | File missing / unreadable |
| `policy.parse_error` | JSON syntax error (`serde_json` failure) |
| `policy.unknown_schema_version` | `schema_version` is not `1` |
| `policy.unknown_field` | Unknown top-level or `net_policy` field |
| `policy.invalid_capability` | `rules` key is not a known capability name |
| `policy.invalid_net_policy` | `net_policy` value out of range or unknown method |

Per convention #2 these names are stable forever. Future kinds are
additive.

## Forwards-compatibility check

Existing committed `.json` policy files (none in repo today, but
operators have them) MUST continue to parse if and only if they were
already valid. The validator's stricter checks (deny-extra,
capability name catalog, net_policy bounds) are the new gate; we
accept that some previously-tolerant files will now fail validate.
This is the correct choice per convention #1: silent acceptance is
the footgun.

## Open question for the next sprint

Should `default_allow: true` plus an empty `rules` list emit a
**warning** (effectively allow-all-with-no-budget)? Today we just
silently accept it. This is lint territory, deferred per non-goals.

## Stability tier

Per `docs/stability.md` this lands as **stable**:
- The error_kind taxonomy is locked.
- The CLI subcommand names are locked.
- The schema_version contract (only `1` accepted, additive new
  fields permitted, shape changes bump version) is the API
  guarantee.
