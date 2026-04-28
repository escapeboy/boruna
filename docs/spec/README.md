# Boruna Specifications

Frozen on-disk and on-the-wire formats. Each spec carries a
`schema_version` (or equivalent) and a forward-compatibility
commitment. Code readers MUST gate on the version field at parse
time; missing or unsupported versions reject with a typed error.

| Spec                                            | Version | Status | Sprint | Reader constant                                   |
|-------------------------------------------------|---------|--------|--------|---------------------------------------------------|
| [Workflow DAG](./workflow-dag-1.0.md)           | 1.0     | stable | W4     | `boruna_orchestrator::WORKFLOW_DAG_SCHEMA_VERSION` |

Conventions:

- **Single-integer major versioning.** A spec field named
  `schema_version` (or `format_version` for evidence bundles) is
  an integer. Minor revisions add fields additively; major
  revisions are reserved for breaking changes.
- **Forward-compat within a major.** A reader built for `N.x`
  MUST accept `N.y` documents (y >= x) and silently ignore
  unknown additive fields.
- **Hard reject across a major.** A reader built for `N.x` MUST
  refuse `N+1.0` documents with a typed
  `Unsupported*Version` error rather than guess.
- **Replay invariant.** Versions feed into the canonical-JSON
  serialization that produces `workflow_hash`/bundle hashes,
  binding evidence to a specific schema generation.
