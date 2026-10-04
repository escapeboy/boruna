# Support and Deprecation Policy

This document says which Boruna releases are supported, what stays
compatible within a major line, how features are deprecated and removed,
and how security fixes are released.

The short version: **only the latest major line is supported.** Today that
is **4.x**. The current release is 4.0.0 (2026-10-04).

## A. Support windows

| Line | Status | First release | Support |
|------|--------|---------------|---------|
| 4.x | **Supported** | 2026-10-04 ([v4.0.0](../CHANGELOG.md)) | Bug fixes and security fixes in new 4.y releases |
| 3.x | End of life since 2026-10-03 | 2026-07-18 (v3.0.0) | None |
| 2.x | End of life since 2026-07-18 | 2026-07-17 (v2.0.0) | None |
| 1.x | End of life since 2026-07-18 | 2026-04-28 (v1.0.0) | None |
| 0.x | End of life since 2026-04-28 | 2026-02-21 (v0.1.0) | None |

Rules:

- Only the latest major line receives fixes.
- Within that line, fixes ship in the next minor or patch release of the
  line. There are no backports to older minor releases. If you run 4.0 and
  a fix ships in 4.2.1, the fix is in 4.2.1, not in a 4.0.x release.
- When a new major line ships, the previous major line is end of life on
  the same day (see section G).

### If you are on 1.x, 2.x or 3.x

1.x and 2.x reached end of life when 3.0.0 shipped on 2026-07-18, and 3.x
when 4.0.0 shipped on 2026-10-04. They get no further releases, including
security fixes. Upgrade to the latest 4.x release.

What changed on the way:

- **2.0.0** made integer overflow a runtime error, made framework policy
  defaults fail closed, and rejects codegen operand counts above 255. See
  the "Breaking changes" list in [`CHANGELOG.md`](../CHANGELOG.md) under
  2.0.0.
- **3.0.0** removed the HTTP server layer: the coordinator, distributed
  workers, HA, the workflow dashboard, the evidence web viewer, the
  approval console, the `serve` cargo feature, the `coordinator`,
  `dashboard`, `worker` and `evidence serve` commands, and the
  `--coordinator` / `--coord-token` flags. Approval and trigger gates are
  handled locally with `boruna workflow approve/reject/trigger` and
  `boruna workflow resume`. If you used any of the removed parts, there is
  no replacement in 3.x.
- **4.0.0** moved the `.ax` language to 2.0: type mismatches the checker
  can name (`E009`) and reassigning a binding that is not `mut` (`E010`)
  are compile errors, `break` and `continue` are keywords, and bindings
  declared in a block or `match` arm end there. See
  [Upgrading to 4.0](./guides/upgrading-to-4.md).

`workflow.json` files, policies and evidence bundles from any earlier line
need no conversion: their format versions did not change major version (see
section B). `.ax` programs may need the small fixes listed in the 4.0
upgrade guide; `boruna lang check` finds every place and `boruna lang
repair` fixes the missing `mut` automatically.

[`docs/guides/migration.md`](./guides/migration.md) and `boruna migrate`
only cover artifacts from before 1.0 (bundles without `bundle.json`,
workflows without `schema_version`). They are not needed for a 1.x or 2.x
upgrade.

## B. What stays compatible within the current major line

Within the current major line, the surfaces below do not break. A program,
workflow, bundle or integration that works on one 4.y release works on
every later 4.y release. Additions (new fields, new flags, new values) are
allowed. Removals, renames and type changes are not.

### Release version and format versions are separate

The Boruna release version (4.0.0) is not the version of the language or
of the file formats. Each has its own version:

| Surface | Current version | Defined in |
|---------|-----------------|------------|
| `.ax` language | 2.0 | `boruna_compiler::LANGUAGE_VERSION` |
| Bytecode | 1.1 | `boruna_bytecode::BYTECODE_VERSION` |
| Evidence bundle format | 1.1 | `boruna_orchestrator::BUNDLE_FORMAT_VERSION` |
| Workflow DAG schema | 1 | `boruna_orchestrator::WORKFLOW_DAG_SCHEMA_VERSION` |
| MCP tool responses | `protocol_version: 1` | `boruna-mcp` |

These versions move by their own rules. A breaking change to one of them
ships only in a new Boruna major release: Boruna 4.0.0 moved the language
to 2.0. Boruna 2.0.0 and 3.0.0 did not bump any of them to a new major.

### B.1 Language

The `.ax` language follows the rule in
[`spec/ax-language-1.0.md`](./spec/ax-language-1.0.md) §1.2: within
language 2.x, changes are additive only. No renames, no removed builtins,
no tightened type rules. A program that compiles under language 2.x
compiles under every later 2.y. Breaking language changes wait for
language 3.0 and a new Boruna major release.

New checks start as warnings in a minor release and become errors only in
the next major. `E009` (type mismatch) and `E010` (reassigning a binding
without `mut`) went this way: warnings in language 1.x, errors in 2.0.

### B.2 Workflow DAG schema

Every `workflow.json` with `schema_version: 1` that validates on one 4.y
release validates on every later 4.y. This covers the required and
optional fields, their types, the DAG rules (acyclic, topological order)
and the per-step inputs and outputs. New optional fields may be added in
minor releases (3.3.0 added `confidence_gate` on approval gates this way).
Spec: [`spec/workflow-dag-1.0.md`](./spec/workflow-dag-1.0.md).

### B.3 Evidence bundle format

Every bundle with a 1.x `format_version` verifies with every 4.y reader.
This covers the directory layout, the canonical JSON encoding, and the
SHA-256 hash-chain construction. Format 1.1 (3.2.0) added per-entry
content hashes for redaction and still reads 1.0 bundles. Spec:
[`spec/evidence-bundle-1.0.md`](./spec/evidence-bundle-1.0.md).

### B.4 MCP tool response shapes

Every response from `boruna-mcp` carries `protocol_version: 1`. The keys,
types and meaning of each documented response stay the same. Fields may
be added. The 14 tools covered are `boruna_compile`, `boruna_ast`,
`boruna_run`, `boruna_check`, `boruna_repair`, `boruna_validate_app`,
`boruna_framework_test`, `boruna_workflow_validate`,
`boruna_template_list`, `boruna_template_apply`,
`boruna_capability_list`, `boruna_policy_validate`, `boruna_symbols` and
`boruna_run_sealed`. Reference:
[`reference/mcp-server.md`](./reference/mcp-server.md).

### B.5 CLI commands and flags

The commands listed as Stable in [`stability.md`](./stability.md) and
their flags keep working with the same meaning. New commands and flags
may be added. Removing a command or renaming a flag needs a new major
release and a prior deprecation (section C).

### B.6 Error taxonomy

`error_kind` strings in CLI errors and MCP error responses keep their
meaning. An `error_kind` that exists in a 4.y release exists in every
later 4.y. New values may be added in minor releases, so integrators must
tolerate values they do not recognize. List:
[`reference/error-kinds.md`](./reference/error-kinds.md).

### B.7 Standard library packages (`libs/`)

These packages are stable. Function signatures, parameter types and
return types do not change within the major line, and the capabilities
declared in each `package.ax.json` do not change. New functions may be
added in minor releases.

| Package | Stable since | Reference |
|---------|--------------|-----------|
| `std-ui` | v1.2.0 | [`std-ui.md`](./reference/stdlib/std-ui.md) |
| `std-validation` | v1.2.0 | [`std-validation.md`](./reference/stdlib/std-validation.md) |
| `std-forms` | v1.2.0 | [`std-forms.md`](./reference/stdlib/std-forms.md) |
| `std-authz` | v1.2.0 | [`std-authz.md`](./reference/stdlib/std-authz.md) |
| `std-http` | v1.2.0 | [`std-http.md`](./reference/stdlib/std-http.md) |
| `std-db` | v1.2.0 | [`std-db.md`](./reference/stdlib/std-db.md) |
| `std-sync` | v1.2.0 | [`std-sync.md`](./reference/stdlib/std-sync.md) |
| `std-routing` | v1.2.0 | [`std-routing.md`](./reference/stdlib/std-routing.md) |
| `std-storage` | v1.2.0 | [`std-storage.md`](./reference/stdlib/std-storage.md) |
| `std-notifications` | v1.2.0 | [`std-notifications.md`](./reference/stdlib/std-notifications.md) |
| `std-testing` | v1.2.0 | [`std-testing.md`](./reference/stdlib/std-testing.md) |
| `std-llm` | v1.3.0 | [`std-llm.md`](./reference/stdlib/std-llm.md) |
| `std-json` | v1.3.0 | [`std-json.md`](./reference/stdlib/std-json.md) |

`std-guard` (added in 3.1.0) is not on this list yet. It has no reference
page and no graduation record in
[`stdlib-graduation-tracker.md`](./stdlib-graduation-tracker.md).

## What can change within a major line

These are not covered by section B and may change in minor releases:

- **Rust crate APIs.** Boruna ships as binaries. The crates
  (`boruna-vm`, `boruna-compiler`, `boruna-orchestrator` and others) may
  change signatures and module layout.
- **Performance.** Throughput, latency and resource use may change. The
  published budget is in [`PERFORMANCE.md`](./PERFORMANCE.md).
- **Defaults.** Default policy, step limits and concurrency may change.
  Such changes are listed in `CHANGELOG.md` under `### Changed`.
- **Log output.** stderr log lines are not a contract. Use the JSON event
  log, MCP responses or `--json` output instead.
- **The persistent run store.** The SQLite database in the data directory
  is not a public surface.
- **Experimental and Alpha components** listed in
  [`stability.md`](./stability.md).
- **Bytecode and module hashes when a bug is fixed.** A compiler fix can
  change the bytecode of affected programs. 3.5.0 did this for integer
  patterns and nested `match`. The change is listed in `CHANGELOG.md`.

## C. Deprecation policy

A change that breaks a surface in section B goes through these steps:

1. **Deprecate in a minor release.** List the feature in `CHANGELOG.md`
   under `### Deprecated`, name the release that will remove it (the next
   major), and update the relevant docs page with the replacement.
2. **Warn at runtime.** When the deprecated path is used, print a one-line
   warning to stderr that names the feature and the replacement. The
   warning does not change the exit code.
3. **Remove only in the next major release.** A feature deprecated in 4.y
   is removed no earlier than 5.0.0.

Where an upgrade can be done mechanically (a file format change, a renamed
flag), `boruna migrate` should cover it.

The 3.0.0 release did not follow this process. It removed the HTTP server
layer one day after 2.0.0, with no deprecation release in between.

## D. Security fixes

Security fixes are released only for the **latest release of the current
major line**. A fix ships as a new patch or minor release of 4.x. There
are no backports to older 4.y releases and no fixes for 1.x, 2.x or 3.x.

Severity follows [CVSS v4](https://www.first.org/cvss/v4-0/):

- **CRITICAL or HIGH**: fix released within 7 days of confirmed
  disclosure. If that is not possible, an advisory with mitigations is
  published instead.
- **MEDIUM**: fix released within 30 days of confirmed disclosure.
- **LOW**: included in the next regular release.

Reporting and disclosure follow [`SECURITY.md`](../SECURITY.md). Reports
go through GitHub Security Advisories, not public issues.

### External security audit

No external security audit has been done. An audit of the VM and
capability enforcement is planned (see [`roadmap.md`](./roadmap.md)). It
has not happened yet and has no date. Do not rely on Boruna where a third-party audit attestation is
required.

## E. Communication channels

- **Deprecations**: `CHANGELOG.md` `### Deprecated` sections. This is the
  authoritative source.
- **Security advisories**: GitHub Security Advisories on
  [escapeboy/boruna](https://github.com/escapeboy/boruna/security/advisories).
- **Releases**: GitHub Releases, with binaries and a `SHA256SUMS` file.
- **Roadmap**: [`roadmap.md`](./roadmap.md). It is a plan, not a
  commitment.

## F. Stability tiers

Section B covers the Stable tier only. Experimental and Alpha components
may change in minor releases. The per-component list is in
[`stability.md`](./stability.md). Known constraints are in
[`limitations.md`](./limitations.md).

## G. End of life

When a new major release ships, the previous major line is end of life on
the same day. It gets no further releases, including security fixes. Its
docs remain in git history and its tags remain on GitHub.

To keep receiving fixes, upgrade to the new major line. The
`CHANGELOG.md` entry for each major release lists what was removed or
changed.

## Cross-references

- [`stability.md`](./stability.md): per-component stability tiers.
- [`roadmap.md`](./roadmap.md): planned work.
- [`limitations.md`](./limitations.md): known constraints.
- [`SECURITY.md`](../SECURITY.md): how to report a vulnerability.
- [`CHANGELOG.md`](../CHANGELOG.md): release history and deprecations.
- [`spec/README.md`](./spec/README.md): versioned specifications.
- [`LICENSE`](../LICENSE): MIT.
