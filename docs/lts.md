# Long-Term Support (LTS) and Deprecation Policy

This document is the contract Boruna offers to operators, integrators, and
auditors who deploy 1.x in production. It is the single source of truth for:

- Which release tracks are supported and for how long.
- What we promise will not break inside the 1.x line.
- What we explicitly reserve the right to change inside 1.x.
- How we handle deprecations and migrations into 2.x.
- How we backport security fixes.
- How and when we end-of-life a major version.

The 1.0.0 release closes the spec on every surface listed in section B. After
1.0 ships, those surfaces are LTS-protected for the duration of 1.x.

## A. Support windows

| Track | Status | First release | Last release | Active support | Security support |
|-------|--------|---------------|--------------|----------------|------------------|
| 1.x | **LTS — IN FORCE** | 2026-04-28 ([v1.0.0](../CHANGELOG.md#100--2026-04-28)) | TBD | through 2027-11-15 (18 months from GA) | through 2028-05-15 (24 months from GA) |
| 0.x | EOL | 2026-02-21 (LLM-Lang) → 2026-04-28 ([v0.5.0](../CHANGELOG.md#050--2026-04-28)) | 2026-04-28 (1.0 GA) | EOL | None |

Definitions:

- **Active support** — receives bug fixes, security fixes, and additive
  features (new CLI flags, new MCP fields, new `error_kind` values). Operators
  on the latest 1.y minor get the full active-support stream.
- **Security support** — receives security fixes only. New 1.y.z patch
  releases are cut on the affected line; no functional or performance changes
  ride along. After active support ends, 1.x continues to receive security
  patches for an additional 6 months.
- **0.x EOL** — once 1.0 ships, the 0.x line receives no further fixes. Users
  on 0.x are expected to upgrade to 1.x using the migration tooling shipped
  in sprint W5-C (see `docs/roadmap.md` 1.0.0 section).

Dates marked `TBD` are pinned at 1.0 tag time. The policy described in this
document is in force regardless of those dates being filled in.

## B. What we commit to in the 1.x line

Inside 1.x, the following surfaces are **stable**. Every 1.0 program, workflow,
bundle, integration, and tool invocation works unchanged on 1.y for any
supported `y`. Additive changes (new fields, new flags, new variants) are
allowed; removals and renames are not.

### B.1 Language

- **`.ax` `language_version: "1.x"`** — every 1.0 `.ax` source file
  type-checks, compiles, and runs on every 1.y.z. The stable surface
  includes: token grammar, type system (records, enums, generics-free
  monomorphic types), pattern matching, capability annotations, the standard
  type set (`Int`, `Float`, `String`, `Bool`, `Unit`, `Option<T>`,
  `Result<T,E>`, `List<T>`, `Map<K,V>`).

### B.2 Workflow DAG schema

- **`workflow.json` schema 1.x** — every 1.0 workflow validates with a 1.y
  validator. The stable surface includes: required/optional fields, their
  types and value domains, the DAG validation rules (acyclic, topological
  ordering), and the per-step `inputs` / `outputs` contracts. New optional
  fields may be added in minor releases.

### B.3 Evidence bundle format

- **Evidence bundle format 1.x** — every 1.0 bundle is verifiable
  byte-identically by every 1.y reader. The stable surface includes: the
  on-disk directory layout, the canonical JSON encoding of `audit_log.json`
  / `events.json` / `manifest.json`, the SHA-256 hash-chain construction,
  and the genesis-entry contents. New optional metadata fields may be added,
  but they are not part of the chained hashes.

### B.4 MCP tool response shapes

- **MCP `protocol_version: 1` response shapes** — the keys, types, and
  semantics of every documented MCP tool response are stable. Field
  additions are allowed; removals and type changes are not. The 10
  documented tools (`boruna_compile`, `boruna_ast`, `boruna_run`,
  `boruna_check`, `boruna_repair`, `boruna_validate_app`,
  `boruna_framework_test`, `boruna_workflow_validate`, `boruna_template_list`,
  `boruna_template_apply`) are LTS-protected.

### B.5 CLI commands and flags

- **CLI commands and their flag names** — every documented `boruna`
  subcommand and every documented flag continues to work with the same
  semantics across 1.x. New subcommands and new flags are allowed; removing
  an existing subcommand or renaming a flag requires 2.0.

### B.6 Error taxonomy

- **`error_kind` strings** — the strings emitted in CLI errors and MCP error
  responses are LTS-protected: an `error_kind` that
  exists in 1.0 will exist with the same meaning in every 1.y. New
  `error_kind` values may be introduced in minor releases.

### B.7 Standard library packages (`libs/`)

The following `std-*` packages are 1.0-stable and LTS-protected from **v1.2.0**:

| Package | Stable since | Reference docs |
|---------|-------------|----------------|
| `std-ui` | v1.2.0 | [`docs/reference/stdlib/std-ui.md`](./reference/stdlib/std-ui.md) |
| `std-validation` | v1.2.0 | [`docs/reference/stdlib/std-validation.md`](./reference/stdlib/std-validation.md) |
| `std-forms` | v1.2.0 | [`docs/reference/stdlib/std-forms.md`](./reference/stdlib/std-forms.md) |
| `std-authz` | v1.2.0 | [`docs/reference/stdlib/std-authz.md`](./reference/stdlib/std-authz.md) |
| `std-http` | v1.2.0 | [`docs/reference/stdlib/std-http.md`](./reference/stdlib/std-http.md) |
| `std-db` | v1.2.0 | [`docs/reference/stdlib/std-db.md`](./reference/stdlib/std-db.md) |
| `std-sync` | v1.2.0 | [`docs/reference/stdlib/std-sync.md`](./reference/stdlib/std-sync.md) |
| `std-routing` | v1.2.0 | [`docs/reference/stdlib/std-routing.md`](./reference/stdlib/std-routing.md) |
| `std-storage` | v1.2.0 | [`docs/reference/stdlib/std-storage.md`](./reference/stdlib/std-storage.md) |
| `std-notifications` | v1.2.0 | [`docs/reference/stdlib/std-notifications.md`](./reference/stdlib/std-notifications.md) |
| `std-testing` | v1.2.0 | [`docs/reference/stdlib/std-testing.md`](./reference/stdlib/std-testing.md) |
| `std-llm` | v1.3.0 | [`docs/reference/stdlib/std-llm.md`](./reference/stdlib/std-llm.md) |
| `std-json` | v1.3.0 | [`docs/reference/stdlib/std-json.md`](./reference/stdlib/std-json.md) |

LTS guarantees for these packages: function signatures, parameter types, and return types are frozen. New functions may be added in minor releases. Capability requirements in `package.ax.json` are frozen.

## What CAN change in 1.x

The following are **not** part of the LTS contract and may evolve in minor
releases:

- **Internal Rust APIs.** Boruna ships as a CLI binary. We do not commit to
  a stable Rust library API. Crates
  (`boruna-vm`, `boruna-compiler`, `boruna-orchestrator`, etc.) may change
  signatures and module structures freely.
- **Performance characteristics.** Throughput, latency, and resource
  consumption may change between minors. Performance commitments live in
  `docs/PERFORMANCE.md` (sprint W5-A) — what we publish there is what we
  hold ourselves to; everything else is best-effort.
- **Default values.** Operator-visible defaults (default policy, default
  step limits, default concurrency) may change. Such changes are called
  out in `CHANGELOG.md` under `### Changed`.
- **Logging output format.** stderr log lines are informational, not
  contractual. Tools that parse log lines should switch to structured
  events (the JSON event log, MCP responses, or `--json` flags) for stable
  consumption.
- **Internal database schema.** The on-disk SQLite layout in
  `<data-dir>/boruna.db` is not a public surface. Schema upgrades are
  handled transparently by the migration tooling (sprint W5-C); operators
  do not need to migrate manually.

## C. Deprecation policy

Boruna will introduce breaking changes in 2.x via the following process. Any
breaking change in a 1.x-LTS-protected surface (section B) follows all four
steps:

1. **Announce in a 1.y minor release.** Mark the feature deprecated in
   `CHANGELOG.md` under `### Deprecated`, including `deprecated_in: "1.y"`
   and `removed_in: "2.0"` annotations. Update the relevant docs page to
   call the feature deprecated and link the migration path.
2. **Surface a runtime warning.** Whenever the deprecated path is exercised
   at runtime (CLI, MCP, HTTP API, `.ax` runtime), emit a one-line warning
   to stderr: `warning: <feature> is deprecated and will be removed in 2.0;
   see <link>`. Warnings are emitted at most once per process per
   deprecation. They never cause exit-non-zero.
3. **Honor the 6-month notice period.** At least 6 months elapse between
   the first 1.y minor that announces a deprecation and the 2.0 GA that
   removes it. This gives downstream integrators a real upgrade window.
4. **Provide migration tooling where mechanical.** Sprint W5-C delivers the
   `boruna migrate` tool. Where a migration is mechanically derivable —
   file-format-to-file-format, deprecated-flag-to-new-equivalent,
   workflow.json schema upgrade, evidence-bundle format upgrade — the
   tool performs it automatically. Migrations that require human judgment
   (e.g. semantic policy changes) are documented in
   `docs/migrations/2.0.md` with worked examples.

A 2.0 release SHALL NOT remove a feature that has not gone through this
deprecation cycle.

## D. Security fix backports

Security fixes are the highest-priority category of release work and the
only category that interrupts the normal release cadence.

### D.1 Backport target lines

- Fixes for vulnerabilities are backported to **every supported 1.y minor
  line** for which the vulnerability applies.
- Fix versions are cut as patch releases (e.g. `1.3.4`, `1.4.2`, `1.5.1`)
  on each affected line. Patch releases contain the security fix and any
  trivially related test or doc changes — they do not bundle unrelated
  features.
- The latest 1.y minor always receives the fix; older 1.y lines receive
  the fix while they remain in active or security support (see section A).

### D.2 Severity assessment

- Severity follows [CVSS v4](https://www.first.org/cvss/v4-0/).
- **CRITICAL or HIGH** — fix released within **7 days** of confirmed
  disclosure. If the fix cannot be released within 7 days, an interim
  advisory with mitigations is published instead.
- **MEDIUM** — fix released within 30 days of confirmed disclosure.
- **LOW** — bundled with the next scheduled patch release on each
  supported line.

### D.3 Disclosure

The reporter, disclosure, and advisory process is governed by
[`SECURITY.md`](../SECURITY.md). Backports always ship together with the
GitHub Security Advisory.

## E. Communication channels

- **Deprecation announcements** — `CHANGELOG.md` `### Deprecated` section
  ([Keep a Changelog](https://keepachangelog.com/en/1.1.0/) convention).
  This is the authoritative source.
- **Critical security advisories** — [`SECURITY.md`](../SECURITY.md) plus
  GitHub Security Advisories on
  [escapeboy/boruna](https://github.com/escapeboy/boruna/security/advisories).
- **Release announcements** — GitHub Releases (binaries + signed
  `SHA256SUMS`) and the version badge in [`README.md`](../README.md).
- **Roadmap** — [`docs/roadmap.md`](./roadmap.md) tracks scheduled work,
  but is not a contract; the LTS contract lives in this document.

## F. What "production-ready" means for 1.0

The 1.0 GA marks the point at which the surfaces listed in section B become
LTS-protected. It does *not* mark every component as stable. Refer to
[`docs/stability.md`](./stability.md) for the per-component stability tier:

- **Stable** surfaces (the section B list) are LTS-protected for the full
  1.x line.
- **Experimental** surfaces are clearly marked and may break in 1.x minors.
  Operators choosing to depend on them do so explicitly.
- **Alpha** surfaces are under active development and may break frequently.
- **Out-of-scope-for-1.0 additions** — items such as evidence-bundle
  encryption, additional storage adapters, and the LLM provider registry
  are pre-LTS additions. They enter 1.x as Experimental, graduate through
  Experimental → Stable across the 1.x minor releases as their interfaces
  prove out, and are LTS-protected from the minor in which they graduate
  forward.

The known constraints listed in [`docs/limitations.md`](./limitations.md)
remain accurate against the 1.0 commitment: nothing in this LTS document
contradicts the limitations file. Where a limitation is later removed
(e.g. evidence-bundle encryption shipping as a 1.y addition), the addition
is itself stable from its graduation point.

## G. End-of-life procedure

When a major version reaches end-of-life:

1. **12-month notice.** A planned EOL date is announced at least 12 months
   in advance via `CHANGELOG.md` `### Deprecated`, the README badge, and
   GitHub Releases. The notice names the recommended successor track.
2. **Community migration window.** During the 12 months between the EOL
   announcement and the EOL date, operators upgrade to the successor
   track. Migration tooling (`boruna migrate`) is updated to cover the
   full path.
3. **EOL takes effect.** After the EOL date, the project releases no
   further fixes for the EOL'd major version — including security fixes.
   Community-maintained forks are welcome.
4. **Documentation archival.** The EOL'd version's docs are preserved on
   GitHub but tagged as historical. Cross-links from the current docs are
   updated to point at the successor track.

## Cross-references

- [`docs/stability.md`](./stability.md) — per-component stability tiers
  (LTS-protected vs. experimental vs. alpha vs. planned).
- [`docs/roadmap.md`](./roadmap.md) — scheduled work toward 1.0 and beyond.
- [`docs/limitations.md`](./limitations.md) — what is intentionally out of
  scope; cross-checked against this document for consistency.
- [`SECURITY.md`](../SECURITY.md) — vulnerability disclosure policy and
  backport contract (section D mirrors and extends).
- [`CHANGELOG.md`](../CHANGELOG.md) — release history and the
  authoritative source for deprecation announcements.
- [`docs/QUICKSTART.md`](./QUICKSTART.md) — 10-minute onboarding.
- [`LICENSE`](../LICENSE) — MIT.
